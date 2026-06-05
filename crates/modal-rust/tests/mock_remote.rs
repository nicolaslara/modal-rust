//! Facade end-to-end example test (regular `#[tokio::test]`): drive a REAL
//! `modal_rust::App` through `.remote()` against the in-process mock, OFFLINE.
//!
//! This exercises a MAIN feature end-to-end:
//!   - `App::connect_at` builds a real facade `App` whose SDK client dials the mock
//!     (ephemeral `AppCreate` + `ClientHello`), with NO Modal creds and NO network
//!     beyond loopback;
//!   - `.remote()` drives the whole `ensure_function` RUN manifest (cargo cache
//!     volume, client + python-standalone + source mounts, image get-or-create,
//!     function precreate, FunctionCreate FILE-mode, ephemeral AppPublish), then the
//!     CBOR invoke (FunctionMap -> FunctionGetOutputs);
//!   - the mock returns a CANNED success envelope which the facade DECODES into the
//!     typed `AddOutput`.
//!
//! We assert BOTH the decoded output AND the captured `FunctionCreate` manifest the
//! mock recorded — proving the rust-like typed request-query ergonomics
//! (`mock.last::<FunctionCreateRequest>()`).

use std::fs;

use example_add::{modal_registry, AddOutput};
use modal_rust::{App, RemoteConfig};
use modal_rust_testkit::prelude::*;

/// Build a tiny, self-contained source dir (a minimal cargo workspace) for the
/// RUN-path source mount, so the upload is small + deterministic and never reads
/// the real workspace. `use_cargo_scoping=false` forces the whole-dir upload path
/// (no `cargo metadata`), pruned only by the built-in ignore defaults.
fn tiny_source_config(dir: &std::path::Path) -> RemoteConfig {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
    fs::create_dir_all(dir.join("app/src")).unwrap();
    fs::write(
        dir.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(dir.join("app/src/lib.rs"), "// tiny\n").unwrap();

    RemoteConfig {
        local_root: dir.to_path_buf(),
        package: "app".to_string(),
        use_cargo_scoping: false,
        // Caching ON exercises the cargo-cache volume_get_or_create on the manifest,
        // proving that RPC is wired too. (The mock returns a canned vo-{n} id.)
        cache: true,
        ..RemoteConfig::default()
    }
}

#[tokio::test]
async fn remote_add_against_mock_records_manifest_and_decodes_output() {
    // 1. Start the in-process mock with a canned function result (the output the
    //    "remote add" would produce). No Modal creds, loopback only.
    let mock = MockModal::builder()
        .function_result_value(serde_json::json!({ "sum": 42 }))
        .start()
        .await
        .expect("mock up");

    // 2. Connect a REAL facade App at the mock (test-only seam). A tiny temp source
    //    dir keeps the RUN-path source upload small and deterministic.
    let tmp = std::env::temp_dir().join(format!("modal-rust-mock-remote-{}", std::process::id()));
    let app = App::connect_at_with(
        "mock-app",
        modal_registry(),
        mock.url(),
        tiny_source_config(&tmp),
    )
    .await
    .expect("connect at mock");

    // 3. Exercise a MAIN feature: .remote() drives the full ensure_function manifest
    //    + invoke, then DECODES the canned envelope into the typed output.
    let out: AddOutput = app
        .function("add")
        .remote(example_add::AddInput { a: 40, b: 2 })
        .await
        .expect("remote add");
    assert_eq!(out.sum, 42, "facade decoded the mock's canned envelope");

    // 4. Assert the captured manifest: the FunctionCreate the mock recorded.
    let fc = mock
        .last::<FunctionCreateRequest>()
        .expect("FunctionCreate sent");
    let function = fc
        .function
        .expect("FILE-mode sets `function` (XOR function_data)");
    assert!(
        fc.function_data.is_none(),
        "FILE-mode XOR invariant: function_data is unset"
    );
    assert_eq!(function.module_name, "modal_rust_run_wrapper"); // WRAPPER_MODULE
    assert_eq!(function.function_name, "handler"); // WRAPPER_CALLABLE
    assert_eq!(
        function.mount_ids.len(),
        2,
        "RUN path attaches client + source mounts"
    );
    // CPU path: no gpu_config on the resources.
    assert!(
        function.resources.and_then(|r| r.gpu_config).is_none(),
        "bare `add` entrypoint is CPU (no gpu_config)"
    );

    // 5. The full control-plane sequence fired and was recorded.
    assert_eq!(
        mock.took::<AppCreateRequest>(),
        1,
        "ephemeral RUN app created"
    );
    assert_eq!(mock.took::<ImageGetOrCreateRequest>(), 1);
    assert_eq!(mock.took::<FunctionPrecreateRequest>(), 1);
    assert_eq!(mock.took::<FunctionCreateRequest>(), 1);
    assert_eq!(mock.took::<AppPublishRequest>(), 1, "ephemeral publish");
    assert_eq!(
        mock.took::<VolumeGetOrCreateRequest>(),
        1,
        "cargo cache volume"
    );
    assert_eq!(mock.took::<FunctionMapRequest>(), 1, "invoke opened");
    assert!(
        mock.took::<FunctionGetOutputsRequest>() >= 1,
        "outputs polled"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// The mock also drives the facade's ENVELOPE ERROR taxonomy offline: a canned
/// error envelope is decoded by `parse_envelope` into a typed `Error::Runner`,
/// proving `.remote()`'s error path without any Modal/Python.
#[tokio::test]
async fn remote_error_envelope_decodes_to_runner_error() {
    let mock = MockModal::builder()
        .function_result_envelope(
            r#"{"ok":false,"error":{"kind":"function_error","message":"boom","details":null}}"#,
        )
        .start()
        .await
        .expect("mock up");

    let tmp = std::env::temp_dir().join(format!("modal-rust-mock-err-{}", std::process::id()));
    let app = App::connect_at_with(
        "mock-app-err",
        modal_registry(),
        mock.url(),
        tiny_source_config(&tmp),
    )
    .await
    .expect("connect at mock");

    let result: Result<AddOutput, _> = app
        .function("add")
        .remote(example_add::AddInput { a: 1, b: 2 })
        .await;
    let err = result.expect_err("error envelope surfaces as Err");
    // The facade reconstructed a Runner error from the canned envelope.
    assert!(
        format!("{err}").contains("boom") || matches!(err, modal_rust::Error::Runner(_)),
        "expected a runner error carrying the canned message, got: {err}"
    );

    let _ = fs::remove_dir_all(&tmp);
}
