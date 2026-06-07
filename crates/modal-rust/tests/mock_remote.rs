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

/// cpu/memory ride into FunctionCreate.resources. A RUN with
/// `cpu = 2.0` / `memory = 4096` on the decorator resolves to `milli_cpu = 2000`
/// (`int(1000 * cpu)`, mirroring Modal) and `memory_mb = 4096`, riding into the
/// `FunctionCreate` resources. ADDITIVE: an unset decorator leaves the server
/// default (0/0), so the wire is byte-identical (the `..._is_wire_identical` test
/// and the unchanged `mock_remote`/`mock_table` manifests prove that).
#[tokio::test]
async fn run_with_cpu_and_memory_ride_into_function_create() {
    let mock = MockModal::start().await.expect("mock up");
    let tmp = std::env::temp_dir().join(format!("modal-rust-mock-cpumem-{}", std::process::id()));
    let mut rc = tiny_source_config(&tmp);
    rc.cache = false; // keep the manifest minimal
    let app = App::connect_at_with_configs(
        "cpumem-app",
        modal_registry(),
        configs_for_add(FunctionConfig {
            cache: Some(false),
            // The wire units the macro resolves `cpu = 2.0` / `memory = 4096` to.
            milli_cpu: Some(2000),
            memory_mb: Some(4096),
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

    let function = mock
        .last::<FunctionCreateRequest>()
        .and_then(|fc| fc.function)
        .expect("function");
    let resources = function.resources.expect("resources always sent");
    assert_eq!(resources.milli_cpu, 2000, "cpu=2.0 -> milli_cpu=2000");
    assert_eq!(resources.memory_mb, 4096, "memory=4096 MiB");
    // CPU-only: no gpu_config rode along.
    assert!(
        resources.gpu_config.is_none(),
        "cpu/memory request is GPU-free"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// cpu/memory NEGATIVE — an unset decorator leaves the server defaults (0/0), so the
/// resources are wire-identical to before the feature. Pins the additive guarantee.
#[tokio::test]
async fn run_without_cpu_or_memory_is_wire_identical() {
    let mock = MockModal::start().await.expect("mock up");
    let tmp = std::env::temp_dir().join(format!("modal-rust-mock-nocpumem-{}", std::process::id()));
    let mut rc = tiny_source_config(&tmp);
    rc.cache = false;
    let app = App::connect_at_with("nocpumem-app", modal_registry(), mock.url(), rc)
        .await
        .expect("connect");
    let _: serde_json::Value = app
        .function("add")
        .remote(serde_json::json!({ "a": 1, "b": 2 }))
        .await
        .expect("remote");
    let resources = mock
        .last::<FunctionCreateRequest>()
        .and_then(|fc| fc.function)
        .and_then(|f| f.resources)
        .expect("resources always sent");
    assert_eq!(resources.milli_cpu, 0, "unset cpu => server default 0");
    assert_eq!(resources.memory_mb, 0, "unset memory => server default 0");
    let _ = fs::remove_dir_all(&tmp);
}

/// retries ride into FunctionCreate.retry_policy. A RUN with `retries = 3` on the
/// decorator rides into `Function.retry_policy` as Modal's fixed-interval policy
/// (`retries = 3`, `backoff_coefficient = 1.0`, `initial_delay = 1s`, `max_delay =
/// 60s`, mirroring `_parse_retries(int)`). ADDITIVE: an unset decorator leaves
/// `retry_policy` unset, so the wire is byte-identical (the negative test below + the
/// unchanged `mock_remote`/`mock_table` manifests prove that).
#[tokio::test]
async fn run_with_retries_rides_into_function_create() {
    let mock = MockModal::start().await.expect("mock up");
    let tmp = std::env::temp_dir().join(format!("modal-rust-mock-retries-{}", std::process::id()));
    let mut rc = tiny_source_config(&tmp);
    rc.cache = false; // keep the manifest minimal
    let app = App::connect_at_with_configs(
        "retries-app",
        modal_registry(),
        configs_for_add(FunctionConfig {
            cache: Some(false),
            retries: Some(3),
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

    let function = mock
        .last::<FunctionCreateRequest>()
        .and_then(|fc| fc.function)
        .expect("function");
    let policy = function
        .retry_policy
        .expect("retries=3 ⇒ retry_policy set on the manifest");
    assert_eq!(policy.retries, 3, "retries=3 rode into the retry policy");
    assert_eq!(
        policy.backoff_coefficient, 1.0,
        "bare int retries => fixed-interval backoff (Modal _parse_retries)"
    );
    assert_eq!(policy.initial_delay_ms, 1000, "1s initial delay");
    assert_eq!(policy.max_delay_ms, 60_000, "60s max delay");

    let _ = fs::remove_dir_all(&tmp);
}

/// retries NEGATIVE — an unset decorator leaves `retry_policy` unset, so the
/// FunctionCreate is wire-identical to before the feature. Pins the additive guarantee.
#[tokio::test]
async fn run_without_retries_is_wire_identical() {
    let mock = MockModal::start().await.expect("mock up");
    let tmp =
        std::env::temp_dir().join(format!("modal-rust-mock-noretries-{}", std::process::id()));
    let mut rc = tiny_source_config(&tmp);
    rc.cache = false;
    let app = App::connect_at_with("noretries-app", modal_registry(), mock.url(), rc)
        .await
        .expect("connect");
    let _: serde_json::Value = app
        .function("add")
        .remote(serde_json::json!({ "a": 1, "b": 2 }))
        .await
        .expect("remote");
    let function = mock
        .last::<FunctionCreateRequest>()
        .and_then(|fc| fc.function)
        .expect("function");
    assert!(
        function.retry_policy.is_none(),
        "unset retries => no retry_policy (wire-identical)"
    );
    let _ = fs::remove_dir_all(&tmp);
}

/// schedule rides into FunctionCreate.schedule. A decorator
/// `schedule = Cron("0 9 * * 1")` canonicalizes to the spec `"cron:UTC:0 9 * * 1"`,
/// which the facade parses into Modal's `Schedule.Cron { cron_string, timezone }` and
/// rides onto `Function.schedule` (proto field 72) in the FunctionCreate manifest.
/// ADDITIVE: an unset decorator leaves `schedule` unset (negative test below).
#[tokio::test]
async fn run_with_schedule_rides_into_function_create() {
    use modal_rust_testkit::prelude::schedule::ScheduleOneof;

    let mock = MockModal::start().await.expect("mock up");
    let tmp = std::env::temp_dir().join(format!("modal-rust-mock-sched-{}", std::process::id()));
    let mut rc = tiny_source_config(&tmp);
    rc.cache = false; // keep the manifest minimal
    let app = App::connect_at_with_configs(
        "schedule-app",
        modal_registry(),
        configs_for_add(FunctionConfig {
            cache: Some(false),
            schedule: Some("cron:UTC:0 9 * * 1"),
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

    let function = mock
        .last::<FunctionCreateRequest>()
        .and_then(|fc| fc.function)
        .expect("function");
    let schedule = function
        .schedule
        .and_then(|s| s.schedule_oneof)
        .expect("schedule set on the manifest");
    match schedule {
        ScheduleOneof::Cron(c) => {
            assert_eq!(
                c.cron_string, "0 9 * * 1",
                "cron expr rode onto the manifest"
            );
            assert_eq!(c.timezone, "UTC", "timezone defaults to UTC");
        }
        other => panic!("expected a Cron schedule, got {other:?}"),
    }

    let _ = fs::remove_dir_all(&tmp);
}

/// schedule NEGATIVE — an unset decorator leaves `Function.schedule` unset, so the
/// FunctionCreate is wire-identical to before the feature. Pins the additive guarantee.
#[tokio::test]
async fn run_without_schedule_is_wire_identical() {
    let mock = MockModal::start().await.expect("mock up");
    let tmp = std::env::temp_dir().join(format!("modal-rust-mock-nosched-{}", std::process::id()));
    let mut rc = tiny_source_config(&tmp);
    rc.cache = false;
    let app = App::connect_at_with("nosched-app", modal_registry(), mock.url(), rc)
        .await
        .expect("connect");
    let _: serde_json::Value = app
        .function("add")
        .remote(serde_json::json!({ "a": 1, "b": 2 }))
        .await
        .expect("remote");
    let function = mock
        .last::<FunctionCreateRequest>()
        .and_then(|fc| fc.function)
        .expect("function");
    assert!(
        function.schedule.is_none(),
        "unset schedule => no Function.schedule (wire-identical)"
    );
    let _ = fs::remove_dir_all(&tmp);
}

/// autoscaling rides into FunctionCreate.autoscaler_settings. A RUN with
/// `min_containers = 1`, `max_containers = 5`, `buffer_containers = 2`,
/// `scaledown_window = 120` on the decorator rides into `Function.autoscaler_settings`
/// (proto field 79) AND the deprecated mirror fields Modal still sets
/// (`warm_pool_size`/`concurrency_limit`/`_experimental_buffer_containers`/
/// `task_idle_timeout_secs`). ADDITIVE: an unset decorator leaves all of them unset
/// (negative test below).
#[tokio::test]
async fn run_with_autoscaling_rides_into_function_create() {
    let mock = MockModal::start().await.expect("mock up");
    let tmp = std::env::temp_dir().join(format!("modal-rust-mock-scale-{}", std::process::id()));
    let mut rc = tiny_source_config(&tmp);
    rc.cache = false; // keep the manifest minimal
    let app = App::connect_at_with_configs(
        "scale-app",
        modal_registry(),
        configs_for_add(FunctionConfig {
            cache: Some(false),
            min_containers: Some(1),
            max_containers: Some(5),
            buffer_containers: Some(2),
            scaledown_window: Some(120),
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

    let function = mock
        .last::<FunctionCreateRequest>()
        .and_then(|fc| fc.function)
        .expect("function");

    // The modern autoscaler_settings carries every knob.
    let settings = function
        .autoscaler_settings
        .expect("autoscaling set ⇒ autoscaler_settings on the manifest");
    assert_eq!(settings.min_containers, Some(1), "min_containers rode in");
    assert_eq!(settings.max_containers, Some(5), "max_containers rode in");
    assert_eq!(
        settings.buffer_containers,
        Some(2),
        "buffer_containers rode in"
    );
    assert_eq!(
        settings.scaledown_window,
        Some(120),
        "scaledown_window rode in"
    );

    // Modal also populates the deprecated mirror fields from the same values.
    assert_eq!(function.warm_pool_size, 1, "min -> warm_pool_size");
    assert_eq!(function.concurrency_limit, 5, "max -> concurrency_limit");
    assert_eq!(
        function.experimental_buffer_containers, 2,
        "buffer -> _experimental_buffer_containers"
    );
    assert_eq!(
        function.task_idle_timeout_secs, 120,
        "scaledown_window -> task_idle_timeout_secs"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// autoscaling NEGATIVE — an unset decorator leaves `autoscaler_settings` unset AND
/// every legacy mirror at 0, so the FunctionCreate is wire-identical to before the
/// feature. Pins the additive guarantee.
#[tokio::test]
async fn run_without_autoscaling_is_wire_identical() {
    let mock = MockModal::start().await.expect("mock up");
    let tmp = std::env::temp_dir().join(format!("modal-rust-mock-noscale-{}", std::process::id()));
    let mut rc = tiny_source_config(&tmp);
    rc.cache = false;
    let app = App::connect_at_with("noscale-app", modal_registry(), mock.url(), rc)
        .await
        .expect("connect");
    let _: serde_json::Value = app
        .function("add")
        .remote(serde_json::json!({ "a": 1, "b": 2 }))
        .await
        .expect("remote");
    let function = mock
        .last::<FunctionCreateRequest>()
        .and_then(|fc| fc.function)
        .expect("function");
    assert!(
        function.autoscaler_settings.is_none(),
        "unset autoscaling => no autoscaler_settings (wire-identical)"
    );
    assert_eq!(function.warm_pool_size, 0, "unset => legacy min 0");
    assert_eq!(function.concurrency_limit, 0, "unset => legacy max 0");
    assert_eq!(
        function.experimental_buffer_containers, 0,
        "unset => legacy buffer 0"
    );
    assert_eq!(
        function.task_idle_timeout_secs, 0,
        "unset => legacy window 0"
    );
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

/// `for_each` — the SIDE-EFFECT map: it fans N inputs out, WAITS for them, and
/// discards the outputs (returns `()`). Drive it against the mock with a steered
/// `function_get_outputs` that returns ALL N success outputs (one per input
/// ordinal) so the collect completes, then assert: the map opened a SYNC MAP call,
/// one `FunctionPutInputs` carried all N inputs with sequential idx, and the outputs
/// were polled. (The default responder returns a single idx-0 output, which would
/// never satisfy the N-output collect, so the steer is required.)
#[tokio::test]
async fn for_each_fans_out_n_inputs_waits_and_discards_outputs() {
    const N: usize = 3;
    let mock = MockModal::builder()
        .on_function_get_outputs(|_req| {
            // Return all N success outputs at once (idx 0..N), each a canned success
            // envelope. for_each decodes into IgnoredAny, so the value is irrelevant —
            // only the ok status matters.
            let outputs = (0..N as i32)
                .map(|idx| {
                    let envelope = r#"{"ok":true,"value":{"sum":0}}"#.to_string();
                    let cbor = modal_rust::sdk::codec::encode(&envelope).expect("encode envelope");
                    FunctionGetOutputsItem {
                        idx,
                        data_format: DataFormat::Cbor as i32,
                        result: Some(GenericResult {
                            status: generic_result::GenericStatus::Success as i32,
                            data_oneof: Some(generic_result::DataOneof::Data(cbor)),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }
                })
                .collect();
            Ok(FunctionGetOutputsResponse {
                outputs,
                last_entry_id: "1-0".to_string(),
                num_unfinished_inputs: 0,
                ..Default::default()
            })
        })
        .start()
        .await
        .expect("mock up");

    let tmp = std::env::temp_dir().join(format!("modal-rust-mock-foreach-{}", std::process::id()));
    let app = App::connect_at_with(
        "mock-app-foreach",
        modal_registry(),
        mock.url(),
        tiny_source_config(&tmp),
    )
    .await
    .expect("connect at mock");

    let inputs: Vec<_> = (0..N)
        .map(|i| example_add::AddInput { a: i as i64, b: 1 })
        .collect();

    // The whole point: for_each returns `()` — the caller never names an output type.
    let unit: () = app
        .function("add")
        .for_each(inputs)
        .await
        .expect("for_each over N inputs");
    assert_eq!(unit, (), "for_each yields unit, discarding outputs");

    // It used the proven MAP fan-out wire shape: a SYNC MAP call + ONE PutInputs
    // carrying all N inputs with sequential idx (the ordering key).
    let map = mock.last::<FunctionMapRequest>().expect("FunctionMap sent");
    assert_eq!(map.function_call_type, FunctionCallType::Map as i32);
    assert_eq!(
        map.function_call_invocation_type,
        FunctionCallInvocationType::Sync as i32,
        "for_each WAITS — the MAP call is SYNC"
    );
    let put = mock
        .last::<FunctionPutInputsRequest>()
        .expect("FunctionPutInputs sent");
    assert_eq!(put.inputs.len(), N, "all N inputs enqueued under one call");
    assert_eq!(
        put.inputs.iter().map(|i| i.idx).collect::<Vec<_>>(),
        (0..N as i32).collect::<Vec<_>>(),
        "inputs carry sequential idx 0..N (the ordering key)"
    );
    // for_each BLOCKS on completion, so outputs were polled.
    assert!(
        mock.took::<FunctionGetOutputsRequest>() >= 1,
        "for_each waits for outputs"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// `spawn_map` — the FIRE-AND-FORGET map: it enqueues N inputs under one ASYNC MAP
/// call and returns a `FunctionCall` handle IMMEDIATELY, WITHOUT polling any output.
/// Assert: the map opened an ASYNC MAP call, one `FunctionPutInputs` carried all N
/// inputs, the returned handle carries the map call id, and — the defining property
/// — NO `FunctionGetOutputs` poll fired (fire-and-forget). The default responder
/// works because spawn_map never reads outputs.
#[tokio::test]
async fn spawn_map_fans_out_n_inputs_async_without_polling_outputs() {
    const N: usize = 4;
    let mock = MockModal::start().await.expect("mock up");

    let tmp = std::env::temp_dir().join(format!("modal-rust-mock-spawnmap-{}", std::process::id()));
    let app = App::connect_at_with(
        "mock-app-spawnmap",
        modal_registry(),
        mock.url(),
        tiny_source_config(&tmp),
    )
    .await
    .expect("connect at mock");

    let inputs: Vec<_> = (0..N)
        .map(|i| example_add::AddInput { a: i as i64, b: 1 })
        .collect();

    let fc = app
        .function("add")
        .spawn_map(inputs)
        .await
        .expect("spawn_map over N inputs");
    // The handle carries the map call's id (the mock's canned `fc-1`).
    assert_eq!(fc.function_call_id(), "fc-1");

    // Fire-and-forget over N inputs: an ASYNC MAP call + ONE PutInputs of all N.
    let map = mock.last::<FunctionMapRequest>().expect("FunctionMap sent");
    assert_eq!(map.function_call_type, FunctionCallType::Map as i32);
    assert_eq!(
        map.function_call_invocation_type,
        FunctionCallInvocationType::Async as i32,
        "spawn_map is FIRE-AND-FORGET — the MAP call is ASYNC"
    );
    let put = mock
        .last::<FunctionPutInputsRequest>()
        .expect("FunctionPutInputs sent");
    assert_eq!(put.inputs.len(), N, "all N inputs enqueued under one call");

    // The defining property of spawn_map: it does NOT collect results.
    assert_eq!(
        mock.took::<FunctionGetOutputsRequest>(),
        0,
        "spawn_map never polls outputs (fire-and-forget)"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// Inject-bin wire delta (design B §5.2.2): a crate that DOES declare a `modal-rust`
/// facade dep and ships NO `modal_runner` bin must get the tooling-generated runner
/// injected into the SOURCE upload — exactly ONE extra `MountFile` at
/// `<remote_src>/<crate_rel>/src/bin/modal_runner.rs`, whose bytes equal
/// `render_runner_main(..)` (proven via the content-addressed `sha256_hex`, since the
/// mock confirms files by probe). Mount/RPC COUNTS are unchanged (the file rides INSIDE
/// the one source mount). Uses cargo SCOPING (the production path) over the REAL repo
/// workspace + the real `quickstart` example: `modal-rust` is a workspace MEMBER, so the
/// closure resolves it (no out-of-workspace error — see `run_errors_on_out_of_workspace_
/// path_dep`), and quickstart's lack of a `modal_runner` bin triggers generation.
#[tokio::test]
async fn run_injects_generated_runner_bin_when_absent() {
    use sha2::{Digest, Sha256};

    let mock = MockModal::builder()
        .function_result_value(serde_json::json!({ "sum": 42 }))
        .start()
        .await
        .expect("mock up");

    // The REAL repo workspace root (this test crate is crates/modal-rust). The target is
    // the real `quickstart` example: a pure library (no `modal_runner` bin) whose
    // `modal-rust` path-dep is a workspace member, so the upload closure resolves it.
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root is two levels above crates/modal-rust")
        .to_path_buf();

    let run_config = RemoteConfig {
        local_root: repo_root.clone(),
        package: "quickstart".to_string(),
        use_cargo_scoping: true, // production scoped path
        cache: false,            // keep the manifest minimal
        ..RemoteConfig::default()
    };
    let app = App::connect_at_with("inject-app", modal_registry(), mock.url(), run_config)
        .await
        .expect("connect at mock");

    let _: serde_json::Value = app
        .function("add")
        .remote(serde_json::json!({ "a": 40, "b": 2 }))
        .await
        .expect("remote add");

    // The SOURCE mount's recorded files: find the injected runner by path.
    let mounts = mock.requests::<MountGetOrCreateRequest>();
    let runner_files: Vec<&MountFile> = mounts
        .iter()
        .flat_map(|m| m.files.iter())
        .filter(|f| f.filename.ends_with("/src/bin/modal_runner.rs"))
        .collect();
    assert_eq!(
        runner_files.len(),
        1,
        "exactly ONE injected modal_runner.rs (the +1-file wire delta), got: {:?}",
        runner_files.iter().map(|f| &f.filename).collect::<Vec<_>>()
    );
    let runner = runner_files[0];
    // Path: <remote_src>/examples/quickstart/src/bin/modal_runner.rs (REMOTE_SRC = /src).
    assert_eq!(
        runner.filename,
        "/src/examples/quickstart/src/bin/modal_runner.rs"
    );

    // Bytes: content-address the EXACT generated body via `render_runner_main`.
    let target = modal_rust::resolve_runner_target(&repo_root, "quickstart")
        .expect("runner target resolves");
    assert!(
        target.is_generatable(),
        "no own bin + facade dep => generate"
    );
    assert_eq!(
        modal_rust::injected_runner_rel_path(&target),
        "examples/quickstart/src/bin/modal_runner.rs"
    );
    let expected_body = modal_rust::render_runner_main(&target);
    assert!(
        expected_body.contains("modal_rust::modal_runner!(quickstart);"),
        "generated body spells the facade extern + lib ident: {expected_body}"
    );
    let expected_sha = {
        let digest = Sha256::digest(expected_body.as_bytes());
        let mut s = String::with_capacity(digest.len() * 2);
        for b in digest {
            use std::fmt::Write as _;
            let _ = write!(&mut s, "{b:02x}");
        }
        s
    };
    assert_eq!(
        runner.sha256_hex, expected_sha,
        "the injected file's bytes equal render_runner_main(..)"
    );

    // Mount/RPC COUNTS unchanged: client + source on RUN (the extra file rides INSIDE
    // the source mount, it is not a new mount).
    let function = mock
        .last::<FunctionCreateRequest>()
        .and_then(|fc| fc.function)
        .expect("function");
    assert_eq!(
        function.mount_ids.len(),
        2,
        "RUN still attaches exactly client + source mounts (no new mount)"
    );
}

/// Fix (user-generality): a remote run for an external standalone crate that deps
/// `modal-rust` by a PATH OUTSIDE its own workspace must FAIL LOUDLY at upload time —
/// the closure can't carry that source, so the in-container `cargo build` would
/// otherwise fail with a cryptic "No such file" error. Mirrors the natural local-dev
/// layout (modal-rust is not on crates.io): `injectee` is its own ws root + sole member,
/// so the facade path-dep escapes the workspace. The error must name the offending crate
/// and point at the git/version fix. No control-plane RPCs should fire.
#[tokio::test]
async fn run_errors_on_out_of_workspace_path_dep() {
    let mock = MockModal::builder()
        .function_result_value(serde_json::json!({ "sum": 42 }))
        .start()
        .await
        .expect("mock up");

    let facade_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let tmp = std::env::temp_dir().join(format!("modal-rust-mock-oow-{}", std::process::id()));
    let crate_dir = tmp.join("injectee");
    fs::create_dir_all(crate_dir.join("src")).unwrap();
    fs::write(
        tmp.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"injectee\"]\n",
    )
    .unwrap();
    // The facade is depended on by an ABSOLUTE path OUTSIDE this synthetic workspace.
    fs::write(
        crate_dir.join("Cargo.toml"),
        format!(
            "[package]\nname = \"injectee\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n\
             [lib]\nname = \"injectee\"\npath = \"src/lib.rs\"\n\n\
             [dependencies]\nmodal-rust = {{ path = {facade:?} }}\n",
            facade = facade_dir
        ),
    )
    .unwrap();
    fs::write(
        crate_dir.join("src/lib.rs"),
        "// pure library, no runner bin\n",
    )
    .unwrap();

    let run_config = RemoteConfig {
        local_root: tmp.clone(),
        package: "injectee".to_string(),
        use_cargo_scoping: true,
        cache: false,
        ..RemoteConfig::default()
    };
    let app = App::connect_at_with("oow-app", modal_registry(), mock.url(), run_config)
        .await
        .expect("connect at mock");

    let result: Result<serde_json::Value, _> = app
        .function("add")
        .remote(serde_json::json!({ "a": 40, "b": 2 }))
        .await;
    let err = result.expect_err("out-of-workspace path-dep must fail loudly");
    let msg = err.to_string();
    assert!(
        msg.contains("modal-rust") && msg.contains("OUTSIDE the uploaded workspace"),
        "error names the offending crate + the cause: {msg}"
    );
    assert!(
        msg.contains("git or version"),
        "error points at the git/version fix: {msg}"
    );

    let _ = fs::remove_dir_all(&tmp);
}
