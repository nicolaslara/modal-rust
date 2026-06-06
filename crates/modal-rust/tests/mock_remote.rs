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

use std::collections::BTreeMap;
use std::fs;

use example_add::{modal_registry, AddOutput};
use modal_rust::{App, DeployConfig, FunctionConfig, RemoteConfig};
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
                                                                // Object TAG = the entrypoint name (so per-entrypoint configs never collide); the
                                                                // in-container "handler" callable moves to `implementation_name`.
    assert_eq!(function.function_name, "add");
    assert_eq!(function.implementation_name, "handler");
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

/// Build a tiny DEPLOY config over the same temp source dir helper.
fn tiny_deploy_config(dir: &std::path::Path, app_name: &str) -> DeployConfig {
    let rc = tiny_source_config(dir);
    DeployConfig {
        app_name: app_name.to_string(),
        local_root: rc.local_root,
        package: rc.package,
        use_cargo_scoping: false,
        ..DeployConfig::for_app(app_name)
    }
}

/// Build the per-entrypoint decorator config map (the SAME data path
/// `#[function(...)]` flows through).
fn configs_for_add(cfg: FunctionConfig) -> BTreeMap<String, FunctionConfig> {
    let mut m = BTreeMap::new();
    m.insert("add".to_string(), cfg);
    m
}

/// Row 1 — DEPLOY + CALL end-to-end. `deploy_with` drives the full deploy manifest
/// (client+source(context)+python mounts, persistent AppGetOrCreate, TWO image
/// layers, precreate, the CLIENT-mount-only FunctionCreate, deployed AppPublish);
/// `call` then resolves from_name + invokes with NO upload/build/publish.
#[tokio::test]
async fn deploy_then_call_records_two_image_layers_and_client_only_mount() {
    let mock = MockModal::builder()
        .function_result_value(serde_json::json!({ "sum": 42 }))
        .start()
        .await
        .expect("mock up");

    let tmp = std::env::temp_dir().join(format!("modal-rust-mock-deploy-{}", std::process::id()));
    let app = App::connect_at("deploy-app", modal_registry(), mock.url())
        .await
        .expect("connect at mock");

    let deployed = app
        .deploy_with(tiny_deploy_config(&tmp, "my-deploy"))
        .await
        .expect("deploy");
    assert_eq!(deployed.name, "my-deploy");

    // Two image layers (base + top); the persistent app was get-or-created.
    assert_eq!(
        mock.took::<ImageGetOrCreateRequest>(),
        2,
        "deploy builds two image layers"
    );
    assert_eq!(mock.took::<AppGetOrCreateRequest>(), 1, "persistent app");
    // The DEPLOY build boundary: FunctionCreate attaches the CLIENT mount ONLY.
    let fc = mock
        .last::<FunctionCreateRequest>()
        .expect("FunctionCreate sent");
    let function = fc.function.expect("FILE-mode function");
    assert_eq!(
        function.mount_ids.len(),
        1,
        "DEPLOY attaches CLIENT mount ONLY (no source mount)"
    );
    assert_eq!(function.module_name, "modal_rust_deploy_wrapper");
    // The TOP layer image carries the cargo build (build at image-build time).
    let images = mock.requests::<ImageGetOrCreateRequest>();
    let top = images
        .last()
        .and_then(|r| r.image.clone())
        .expect("top image");
    assert!(
        top.dockerfile_commands
            .iter()
            .any(|c| c.contains("cargo build --release")),
        "deploy top layer builds at image-build time"
    );
    // Deployed publish (state == Deployed).
    let publish = mock.last::<AppPublishRequest>().expect("AppPublish");
    assert_eq!(publish.app_state, AppState::Deployed as i32);

    // CALL: no upload/build/publish — only from_name + invoke.
    let before = mock.request_count();
    let out: AddOutput = app
        .call("my-deploy", "add", example_add::AddInput { a: 40, b: 2 })
        .await
        .expect("call");
    assert_eq!(out.sum, 42);
    // call fired FunctionGet + the invoke RPCs, but NO new image/mount/publish.
    let images_after = mock.took::<ImageGetOrCreateRequest>();
    let mounts_after = mock.took::<MountGetOrCreateRequest>();
    let publishes_after = mock.took::<AppPublishRequest>();
    assert_eq!(images_after, 2, "call builds no image");
    assert_eq!(mounts_after, 3, "call uploads no mount");
    assert_eq!(publishes_after, 1, "call publishes nothing");
    assert!(
        mock.took::<FunctionGetRequest>() >= 1,
        "call resolves from_name"
    );
    assert!(
        mock.request_count() > before,
        "call did fire the invoke RPCs"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// Row 4 — secrets ride into FunctionCreate. A RUN with `secrets=["api-creds"]`
/// resolves SecretGetOrCreate (from_name) before FunctionCreate; the id rides into
/// `function.secret_ids`. The empty-secrets case stays wire-identical.
#[tokio::test]
async fn run_with_secret_resolves_and_rides_into_function_create() {
    let mock = MockModal::start().await.expect("mock up");
    let tmp = std::env::temp_dir().join(format!("modal-rust-mock-secret-{}", std::process::id()));
    let mut rc = tiny_source_config(&tmp);
    rc.cache = false; // keep the manifest minimal
    let app = App::connect_at_with_configs(
        "secret-app",
        modal_registry(),
        configs_for_add(FunctionConfig {
            cache: Some(false),
            secrets: &["api-creds"],
            ..FunctionConfig::default()
        }),
        mock.url(),
        rc,
    )
    .await
    .expect("connect");

    let _: serde_json::Value = app
        .function("add")
        .remote(serde_json::json!({ "a": 1, "b": 2 }))
        .await
        .expect("remote");

    assert_eq!(mock.took::<SecretGetOrCreateRequest>(), 1);
    let sec = mock
        .last::<SecretGetOrCreateRequest>()
        .expect("SecretGetOrCreate");
    assert_eq!(sec.deployment_name, "api-creds");
    // from_name: pure lookup (UNSPECIFIED) with no env_dict.
    assert_eq!(
        sec.object_creation_type,
        ObjectCreationType::Unspecified as i32
    );
    assert!(sec.env_dict.is_empty());
    // The resolved id rode into FunctionCreate.
    let fc = mock
        .last::<FunctionCreateRequest>()
        .expect("FunctionCreate");
    let function = fc.function.expect("function");
    assert_eq!(function.secret_ids, vec!["sc-1"]);

    let _ = fs::remove_dir_all(&tmp);
}

/// Row 4 (negative) — no secrets ⇒ no SecretGetOrCreate, empty secret_ids
/// (wire-identical to before).
#[tokio::test]
async fn run_without_secrets_is_wire_identical() {
    let mock = MockModal::start().await.expect("mock up");
    let tmp = std::env::temp_dir().join(format!("modal-rust-mock-nosec-{}", std::process::id()));
    let mut rc = tiny_source_config(&tmp);
    rc.cache = false;
    let app = App::connect_at_with("nosec-app", modal_registry(), mock.url(), rc)
        .await
        .expect("connect");
    let _: serde_json::Value = app
        .function("add")
        .remote(serde_json::json!({ "a": 1, "b": 2 }))
        .await
        .expect("remote");
    assert_eq!(mock.took::<SecretGetOrCreateRequest>(), 0);
    let function = mock
        .last::<FunctionCreateRequest>()
        .and_then(|fc| fc.function)
        .expect("function");
    assert!(function.secret_ids.is_empty());
    let _ = fs::remove_dir_all(&tmp);
}

/// Row 5 — user volume rides into FunctionCreate. A RUN with
/// `volumes=[("/data","my-vol")]` resolves VolumeGetOrCreate (V1, create) before
/// FunctionCreate; the id mounts at `/data`.
#[tokio::test]
async fn run_with_user_volume_resolves_v1_and_mounts() {
    let mock = MockModal::start().await.expect("mock up");
    let tmp = std::env::temp_dir().join(format!("modal-rust-mock-uservol-{}", std::process::id()));
    let mut rc = tiny_source_config(&tmp);
    rc.cache = false; // no cargo cache volume, so the ONLY volume is the user one
    let app = App::connect_at_with_configs(
        "uservol-app",
        modal_registry(),
        configs_for_add(FunctionConfig {
            cache: Some(false),
            volumes: &[("/data", "my-vol")],
            ..FunctionConfig::default()
        }),
        mock.url(),
        rc,
    )
    .await
    .expect("connect");

    let _: serde_json::Value = app
        .function("add")
        .remote(serde_json::json!({ "a": 1, "b": 2 }))
        .await
        .expect("remote");

    assert_eq!(mock.took::<VolumeGetOrCreateRequest>(), 1);
    let vol = mock
        .last::<VolumeGetOrCreateRequest>()
        .expect("VolumeGetOrCreate");
    assert_eq!(vol.deployment_name, "my-vol");
    // V1 (Unspecified == server default) + create.
    assert_eq!(vol.version, VolumeFsVersion::Unspecified as i32);
    assert_eq!(
        vol.object_creation_type,
        ObjectCreationType::CreateIfMissing as i32
    );
    // The id mounted at /data on the function.
    let function = mock
        .last::<FunctionCreateRequest>()
        .and_then(|fc| fc.function)
        .expect("function");
    assert_eq!(function.volume_mounts.len(), 1);
    assert_eq!(function.volume_mounts[0].mount_path, "/data");
    assert_eq!(function.volume_mounts[0].volume_id, "vo-1");

    let _ = fs::remove_dir_all(&tmp);
}

/// Row 6 (OFF) — cache off ⇒ NO VolumeGetOrCreate, empty volume_mounts
/// (byte-identical to pre-P6). The ON counterpart is the headline mock_remote test +
/// the table test; this pins the OFF path.
#[tokio::test]
async fn run_with_cache_off_attaches_no_cache_volume() {
    let mock = MockModal::start().await.expect("mock up");
    let tmp = std::env::temp_dir().join(format!("modal-rust-mock-cacheoff-{}", std::process::id()));
    let mut rc = tiny_source_config(&tmp);
    rc.cache = false;
    let app = App::connect_at_with("cacheoff-app", modal_registry(), mock.url(), rc)
        .await
        .expect("connect");
    let _: serde_json::Value = app
        .function("add")
        .remote(serde_json::json!({ "a": 1, "b": 2 }))
        .await
        .expect("remote");
    assert_eq!(
        mock.took::<VolumeGetOrCreateRequest>(),
        0,
        "cache off ⇒ no cargo cache volume"
    );
    let function = mock
        .last::<FunctionCreateRequest>()
        .and_then(|fc| fc.function)
        .expect("function");
    assert!(
        function.volume_mounts.is_empty(),
        "cache off ⇒ no /cache mount"
    );
    let _ = fs::remove_dir_all(&tmp);
}

/// Row 6 (ON) — cache on ⇒ VolumeGetOrCreate(V2, cargo-cache name) + a `/cache`
/// mount with background commits, through the FULL facade `.remote()` path.
#[tokio::test]
async fn run_with_cache_on_attaches_v2_cache_volume_at_cache_path() {
    let mock = MockModal::start().await.expect("mock up");
    let tmp = std::env::temp_dir().join(format!("modal-rust-mock-cacheon-{}", std::process::id()));
    // tiny_source_config sets cache: true.
    let app = App::connect_at_with(
        "cacheon-app",
        modal_registry(),
        mock.url(),
        tiny_source_config(&tmp),
    )
    .await
    .expect("connect");
    let _: serde_json::Value = app
        .function("add")
        .remote(serde_json::json!({ "a": 1, "b": 2 }))
        .await
        .expect("remote");
    assert_eq!(
        mock.took::<VolumeGetOrCreateRequest>(),
        1,
        "cargo cache volume"
    );
    let vol = mock
        .last::<VolumeGetOrCreateRequest>()
        .expect("VolumeGetOrCreate");
    assert_eq!(vol.deployment_name, "modal-rust-cargo-cache");
    assert_eq!(vol.version, VolumeFsVersion::V2 as i32);
    assert_eq!(
        vol.object_creation_type,
        ObjectCreationType::CreateIfMissing as i32
    );
    let function = mock
        .last::<FunctionCreateRequest>()
        .and_then(|fc| fc.function)
        .expect("function");
    let cache_mount = function
        .volume_mounts
        .iter()
        .find(|m| m.mount_path == "/cache")
        .expect("/cache mount");
    assert!(
        cache_mount.allow_background_commits,
        "cargo cache bg-commits ON"
    );
    let _ = fs::remove_dir_all(&tmp);
}

/// Rows 8a-8e — the FULL five-kind error taxonomy through the `.remote()` mock path.
/// A fresh mock per case (each on its own loopback port) serves a canned error
/// envelope; the facade's `parse_envelope` reconstructs the typed `RunnerError`.
#[tokio::test]
async fn remote_error_taxonomy_table_through_mock() {
    use modal_rust::{Error, RunnerError};

    // (kind envelope, assertion on the reconstructed error).
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, &str, Box<dyn Fn(&Error)>)> = vec![
        (
            "decode_error",
            r#"{"ok":false,"error":{"kind":"decode_error","message":"bad in","details":null}}"#,
            Box::new(|e| match e {
                Error::Runner(RunnerError::Decode(m)) => assert_eq!(m, "bad in"),
                other => panic!("expected Decode, got {other:?}"),
            }),
        ),
        (
            "unknown_entrypoint",
            r#"{"ok":false,"error":{"kind":"unknown_entrypoint","message":"no fn","details":null}}"#,
            Box::new(|e| match e {
                Error::Runner(RunnerError::UnknownEntrypoint(m)) => assert_eq!(m, "no fn"),
                other => panic!("expected UnknownEntrypoint, got {other:?}"),
            }),
        ),
        (
            "function_error",
            r#"{"ok":false,"error":{"kind":"function_error","message":"boom","details":{"code":7}}}"#,
            Box::new(|e| match e {
                Error::Runner(RunnerError::Function { message, details }) => {
                    assert_eq!(message, "boom");
                    assert_eq!(details, &Some(serde_json::json!({"code":7})));
                }
                other => panic!("expected Function, got {other:?}"),
            }),
        ),
        (
            "encode_error",
            r#"{"ok":false,"error":{"kind":"encode_error","message":"enc","details":null}}"#,
            Box::new(|e| match e {
                Error::Runner(RunnerError::Encode(m)) => assert_eq!(m, "enc"),
                other => panic!("expected Encode, got {other:?}"),
            }),
        ),
        (
            "panic",
            r#"{"ok":false,"error":{"kind":"panic","message":"oops","details":null,"backtrace":"f0\nf1"}}"#,
            Box::new(|e| match e {
                Error::Runner(RunnerError::Panic { message, backtrace }) => {
                    assert_eq!(message, "oops");
                    assert_eq!(backtrace, "f0\nf1");
                }
                other => panic!("expected Panic, got {other:?}"),
            }),
        ),
    ];

    for (i, (kind, envelope, assert_fn)) in cases.into_iter().enumerate() {
        let mock = MockModal::builder()
            .function_result_envelope(envelope)
            .start()
            .await
            .unwrap_or_else(|_| panic!("case {kind}: mock up"));
        let tmp = std::env::temp_dir().join(format!(
            "modal-rust-mock-errtax-{}-{}",
            std::process::id(),
            i
        ));
        let app = App::connect_at_with(
            "errtax-app",
            modal_registry(),
            mock.url(),
            tiny_source_config(&tmp),
        )
        .await
        .unwrap_or_else(|e| panic!("case {kind}: connect: {e}"));

        let res: Result<AddOutput, _> = app
            .function("add")
            .remote(example_add::AddInput { a: 1, b: 2 })
            .await;
        let err = res.expect_err(&format!("case {kind}: expected an error"));
        assert_fn(&err);

        let _ = fs::remove_dir_all(&tmp);
    }
}

/// Cross-check (spec §3.5) — the offline DUMP did NOT drift from the real wire: the
/// `dry_run` manifest's request TYPES/ORDER equal the mock's recorded-request
/// types/order for the SAME RUN. Proves the dump is built ON the same path.
#[tokio::test]
async fn dry_run_matches_the_mock_recorded_request_order() {
    use modal_rust::PlannedRequest;

    let mock = MockModal::start().await.expect("mock up");
    let tmp = std::env::temp_dir().join(format!("modal-rust-mock-dryxchk-{}", std::process::id()));
    // cache on, no secrets/user-volumes — the headline RUN shape.
    let rc = tiny_source_config(&tmp);
    let app = App::connect_at_with("dryxchk-app", modal_registry(), mock.url(), rc.clone())
        .await
        .expect("connect");

    // Drive a real .remote() so the mock records the live RUN sequence.
    let _: serde_json::Value = app
        .function("add")
        .remote(serde_json::json!({ "a": 1, "b": 2 }))
        .await
        .expect("remote");

    // The dump for the SAME config.
    let manifest = app.dry_run("add", &rc).expect("dry_run");

    // Map the dump's planned requests to RPC-type tags, in send order. (The dump's
    // AppCreate corresponds to the ephemeral app created at connect time, which the
    // mock recorded as AppCreate too.)
    let dump_tags: Vec<&str> = manifest
        .requests
        .iter()
        .map(|r| match r {
            PlannedRequest::AppCreate { .. } => "AppCreate",
            PlannedRequest::AppGetOrCreate { .. } => "AppGetOrCreate",
            PlannedRequest::VolumeGetOrCreate { .. } => "VolumeGetOrCreate",
            PlannedRequest::SecretGetOrCreate { .. } => "SecretGetOrCreate",
            PlannedRequest::MountGetOrCreate { .. } => "MountGetOrCreate",
            PlannedRequest::ImageGetOrCreate { .. } => "ImageGetOrCreate",
            PlannedRequest::FunctionPrecreate { .. } => "FunctionPrecreate",
            PlannedRequest::FunctionCreate { .. } => "FunctionCreate",
            PlannedRequest::AppPublish { .. } => "AppPublish",
        })
        .collect();

    // The mock recorded the create-phase RPCs (plus ClientHello/Environment/invoke).
    // Filter the recorded log to the create-phase RPC types the dump covers, in order.
    let recorded_tags: Vec<&str> = mock
        .all_requests()
        .into_iter()
        .filter_map(|r| match r {
            RecordedRequest::AppCreate(_) => Some("AppCreate"),
            RecordedRequest::AppGetOrCreate(_) => Some("AppGetOrCreate"),
            RecordedRequest::VolumeGetOrCreate(_) => Some("VolumeGetOrCreate"),
            RecordedRequest::SecretGetOrCreate(_) => Some("SecretGetOrCreate"),
            RecordedRequest::MountGetOrCreate(_) => Some("MountGetOrCreate"),
            RecordedRequest::ImageGetOrCreate(_) => Some("ImageGetOrCreate"),
            RecordedRequest::FunctionPrecreate(_) => Some("FunctionPrecreate"),
            RecordedRequest::FunctionCreate(_) => Some("FunctionCreate"),
            RecordedRequest::AppPublish(_) => Some("AppPublish"),
            _ => None, // ClientHello / Environment / invoke RPCs are not in the dump
        })
        .collect();

    assert_eq!(
        dump_tags, recorded_tags,
        "the dump's request order must equal the live wire's create-phase order"
    );

    let _ = fs::remove_dir_all(&tmp);
}
