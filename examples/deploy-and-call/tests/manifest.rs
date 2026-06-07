//! Offline proof (zero live Modal) of the production model: `deploy` builds the
//! binary ONCE at image-build time, and a later `call` invokes the published
//! function with NO rebuild.
//!
//! This drives a REAL `modal_rust::App` through `deploy_with` + `call` against the
//! in-process `MockModal` backend (loopback only, dummy creds) and asserts:
//!
//! 1. the DEPLOY manifest carries the build boundary — the image's TOP layer runs
//!    `cargo build --release` (built at image-build time), the FunctionCreate
//!    attaches the CLIENT mount ONLY (the prebuilt `/app/modal_runner` is baked in
//!    the image — no runtime source mount), and the app is PUBLISHED as `deployed`;
//! 2. a subsequent `call` RESOLVES the published function by name (`FunctionGet`)
//!    and invokes it with NO new image, NO new mount, NO publish — i.e. no rebuild.
//!
//! A separate offline check (`App::dump_deploy_manifest`, no mock) is in the driver
//! binary `src/bin/deploy_and_call.rs`; this test exercises the genuine
//! deploy/call control-plane round-trip end to end.

use std::fs;

use modal_rust::{App, DeployConfig};
use modal_rust_testkit::prelude::*;

/// Build a tiny, self-contained cargo workspace for the DEPLOY source/build context
/// so the upload is small + deterministic and never reads the real workspace.
/// `use_cargo_scoping=false` forces the whole-dir path (no `cargo metadata`).
fn tiny_deploy_config(dir: &std::path::Path, app_name: &str) -> DeployConfig {
    fs::create_dir_all(dir.join("app/src")).unwrap();
    fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
    fs::write(
        dir.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(dir.join("app/src/lib.rs"), "// tiny\n").unwrap();

    DeployConfig {
        app_name: app_name.to_string(),
        local_root: dir.to_path_buf(),
        package: "app".to_string(),
        use_cargo_scoping: false,
        ..DeployConfig::for_app(app_name)
    }
}

#[tokio::test]
async fn deploy_builds_once_and_call_invokes_with_no_rebuild() {
    // The mock returns a canned `fib(10) == 55` envelope; loopback only, no creds.
    let mock = MockModal::builder()
        .function_result_value(serde_json::json!(55))
        .start()
        .await
        .expect("mock up");

    // Connect a REAL facade App at the mock, using THIS crate's inventory registry
    // (the SAME `#[modal_rust::function] fn fib` the runner serves).
    let tmp =
        std::env::temp_dir().join(format!("modal-rust-deploy-and-call-{}", std::process::id()));
    let app = App::connect_at(
        "deploy-and-call-driver",
        modal_rust::registry_from_inventory(),
        mock.url(),
    )
    .await
    .expect("connect at mock");

    // ---- DEPLOY: build the binary ONCE at image-build time --------------------------
    let deployed = app
        .deploy_with(tiny_deploy_config(&tmp, "deploy-and-call-app"))
        .await
        .expect("deploy");
    assert_eq!(deployed.name, "deploy-and-call-app");

    // Two image layers (base + top); the persistent app was get-or-created.
    assert_eq!(
        mock.took::<ImageGetOrCreateRequest>(),
        2,
        "deploy builds two image layers"
    );
    assert_eq!(mock.took::<AppGetOrCreateRequest>(), 1, "persistent app");

    // The TOP layer image builds at IMAGE-BUILD time (the deploy build boundary).
    let top = mock
        .requests::<ImageGetOrCreateRequest>()
        .last()
        .and_then(|r| r.image.clone())
        .expect("top image");
    assert!(
        top.dockerfile_commands
            .iter()
            .any(|c| c.contains("cargo build --release")),
        "deploy top layer runs `cargo build --release` at image-build time"
    );

    // The DEPLOY FunctionCreate attaches the CLIENT mount ONLY (the prebuilt
    // /app/modal_runner is baked into the image — NO runtime source mount).
    let fc = mock
        .last::<FunctionCreateRequest>()
        .expect("FunctionCreate sent");
    let function = fc.function.expect("FILE-mode function");
    assert_eq!(
        function.mount_ids.len(),
        1,
        "DEPLOY attaches the CLIENT mount ONLY (no source mount)"
    );
    assert_eq!(function.module_name, "modal_rust_deploy_wrapper");
    assert_eq!(function.function_name, "fib", "object tag = the entrypoint");

    // The app is PUBLISHED persistently as `deployed`.
    let publish = mock.last::<AppPublishRequest>().expect("AppPublish");
    assert_eq!(
        publish.app_state,
        AppState::Deployed as i32,
        "deployed publish"
    );

    // ---- CALL: resolve the published function and invoke it with NO rebuild ---------
    let before = mock.request_count();
    let out: u64 = app
        .call("deploy-and-call-app", "fib", 10u32)
        .await
        .expect("call");
    assert_eq!(out, 55, "call decoded the deployed function's result");

    // call resolved from_name + invoked, but built NOTHING new.
    assert_eq!(
        mock.took::<ImageGetOrCreateRequest>(),
        2,
        "call builds no image (no rebuild)"
    );
    assert_eq!(
        mock.took::<MountGetOrCreateRequest>(),
        3,
        "call uploads no new mount"
    );
    assert_eq!(
        mock.took::<AppPublishRequest>(),
        1,
        "call publishes nothing"
    );
    assert!(
        mock.took::<FunctionGetRequest>() >= 1,
        "call resolves the deployed function by name (FunctionGet)"
    );
    assert!(
        mock.request_count() > before,
        "call did fire the invoke RPCs"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// The function body is a plain Rust fn — callable with no Modal in the loop — and
/// it really computes Fibonacci. Asserts the REAL output properties (offline): the
/// known base/sequence values, the additive recurrence holds, the build-time-baked
/// result matches the call envelope (`fib(10) == 55`), and overflow past `u64` is a
/// clean error rather than a wraparound.
#[test]
fn fib_really_computes_fibonacci() {
    use example_deploy_and_call::fib;

    // Base cases and a few known values from the real sequence.
    assert_eq!(fib(0).unwrap(), 0);
    assert_eq!(fib(1).unwrap(), 1);
    assert_eq!(fib(2).unwrap(), 1);
    assert_eq!(fib(10).unwrap(), 55, "matches the deploy/call envelope");
    assert_eq!(fib(20).unwrap(), 6765);

    // The defining recurrence holds across the range: fib(k) = fib(k-1) + fib(k-2).
    for k in 2..=90u32 {
        assert_eq!(
            fib(k).unwrap(),
            fib(k - 1).unwrap() + fib(k - 2).unwrap(),
            "fib({k}) breaks the recurrence"
        );
    }

    // The largest value the loop returns before its checked add overflows u64
    // (the loop always probes fib(n+1), so fib(92) is the last `Ok`), and a clean
    // error rather than a wraparound once that probe exceeds u64::MAX.
    assert_eq!(fib(92).unwrap(), 7_540_113_804_746_346_429);
    assert!(
        fib(93).is_err(),
        "fib's checked add overflows u64 -> clean error"
    );
}
