//! TABLE example test: assert the captured `FunctionCreate` manifest across a
//! table of RUN configs, OFFLINE. Proves table-test ergonomics — the mock + its
//! per-case config is a VALUE built fresh in a loop, each case on its OWN loopback
//! port with its OWN request log (no shared global state; the env-var path could
//! NOT do this).
//!
//! Each case drives ONE `.remote()` through the facade against a fresh mock and
//! asserts the gpu/timeout the mock recorded on the `FunctionCreate`.

use std::collections::BTreeMap;
use std::fs;

use example_add::modal_registry;
use modal_rust::{App, FunctionConfig, RemoteConfig};
use modal_rust_testkit::prelude::*;

/// One table row: a decorator config (gpu/timeout, exactly as
/// `#[function(gpu=.., timeout=..)]` would set) and the manifest fields it should
/// project onto the captured `FunctionCreate`.
struct Case {
    name: &'static str,
    gpu: Option<&'static str>,
    timeout: u32,
    expect_gpu_set: bool,
}

/// Tiny deterministic source dir for the RUN-path source mount (per case, so cases
/// stay independent). Whole-dir upload (no `cargo metadata`). Caching OFF keeps the
/// manifest minimal (no cargo-cache volume).
fn tiny_source_config(dir: &std::path::Path) -> RemoteConfig {
    fs::create_dir_all(dir.join("app/src")).unwrap();
    fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
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
        cache: false,
        ..RemoteConfig::default()
    }
}

/// Build the per-entrypoint decorator config map for the `add` entrypoint — the
/// SAME data path `#[function(gpu=.., timeout=..)]` flows through (the RUN path
/// re-derives gpu/timeout from this, `App::resolve_function`).
fn configs_for(case: &Case) -> BTreeMap<String, FunctionConfig> {
    let mut m = BTreeMap::new();
    m.insert(
        "add".to_string(),
        FunctionConfig {
            gpu: case.gpu,
            timeout_secs: Some(case.timeout),
            cache: Some(false),
            secrets: &[],
            volumes: &[],
        },
    );
    m
}

/// Build two per-entrypoint decorator configs for one app. The CPU entrypoint has
/// no GPU; the GPU entrypoint requests T4. Both disable cache so the captured
/// FunctionCreate sequence stays focused on the config keying bug.
fn divergent_entrypoint_configs() -> BTreeMap<String, FunctionConfig> {
    let mut m = BTreeMap::new();
    m.insert(
        "add".to_string(),
        FunctionConfig {
            gpu: None,
            timeout_secs: Some(600),
            cache: Some(false),
            secrets: &[],
            volumes: &[],
        },
    );
    m.insert(
        "add_gpu".to_string(),
        FunctionConfig {
            gpu: Some("T4"),
            timeout_secs: Some(1800),
            cache: Some(false),
            secrets: &[],
            volumes: &[],
        },
    );
    m
}

#[tokio::test]
async fn function_create_manifest_table() {
    let cases = [
        Case {
            name: "cpu",
            gpu: None,
            timeout: 600,
            expect_gpu_set: false,
        },
        Case {
            name: "t4",
            gpu: Some("T4"),
            timeout: 1800,
            expect_gpu_set: true,
        },
        Case {
            name: "a100",
            gpu: Some("A100"),
            timeout: 900,
            expect_gpu_set: true,
        },
    ];

    for c in cases {
        // Each case spins its OWN mock + its OWN facade App on an independent
        // loopback port — no shared global state.
        let mock = MockModal::start().await.expect("mock up");
        let tmp = std::env::temp_dir().join(format!(
            "modal-rust-mock-table-{}-{}",
            std::process::id(),
            c.name
        ));
        let app = App::connect_at_with_configs(
            "table-app",
            modal_registry(),
            configs_for(&c),
            mock.url(),
            tiny_source_config(&tmp),
        )
        .await
        .unwrap_or_else(|e| panic!("case {}: connect: {e}", c.name));

        // Drive ONE .remote() to emit the FunctionCreate (default echo result is
        // fine — this table asserts the REQUEST manifest, not the output).
        let _: serde_json::Value = app
            .function("add")
            .remote(serde_json::json!({ "a": 1, "b": 2 }))
            .await
            .unwrap_or_else(|e| panic!("case {}: remote: {e}", c.name));

        // Assert the captured manifest for this case.
        let fc = mock
            .last::<FunctionCreateRequest>()
            .unwrap_or_else(|| panic!("case {}: no FunctionCreate recorded", c.name));
        let f = fc
            .function
            .unwrap_or_else(|| panic!("case {}: no function", c.name));

        assert_eq!(f.timeout_secs, c.timeout, "case {}: timeout", c.name);

        let gpu = f.resources.and_then(|r| r.gpu_config);
        assert_eq!(
            gpu.is_some(),
            c.expect_gpu_set,
            "case {}: gpu_config presence",
            c.name
        );
        if let (Some(want), Some(got)) = (c.gpu, gpu.as_ref()) {
            assert_eq!(
                got.gpu_type,
                want.to_uppercase(),
                "case {}: gpu type",
                c.name
            );
        }

        let _ = fs::remove_dir_all(&tmp);
    }
}

/// Regression: one connected App with two entrypoints must not let whichever
/// entrypoint runs first freeze the wrapper config for every later entrypoint.
/// Calling CPU first then GPU second must produce a second FunctionCreate carrying
/// the GPU config.
#[tokio::test]
async fn divergent_entrypoint_configs_create_distinct_run_wrappers() {
    let mock = MockModal::start().await.expect("mock up");
    let tmp =
        std::env::temp_dir().join(format!("modal-rust-mock-divergent-{}", std::process::id()));
    let app = App::connect_at_with_configs(
        "divergent-app",
        modal_registry(),
        divergent_entrypoint_configs(),
        mock.url(),
        tiny_source_config(&tmp),
    )
    .await
    .expect("connect");

    let _: serde_json::Value = app
        .function("add")
        .remote(serde_json::json!({ "a": 1, "b": 2 }))
        .await
        .expect("cpu remote");
    let _: serde_json::Value = app
        .function("add_gpu")
        .remote(serde_json::json!({ "a": 1, "b": 2 }))
        .await
        .expect("gpu remote");

    let creates = mock.requests::<FunctionCreateRequest>();
    assert_eq!(
        creates.len(),
        2,
        "CPU-first then GPU-second must create two wrapper functions, not reuse the CPU wrapper"
    );

    let cpu = creates[0]
        .function
        .as_ref()
        .expect("cpu FunctionCreate function");
    // DISTINCT object tag = the entrypoint (NOT the shared "handler"): this is what
    // makes the two functions coexist in one app instead of clobbering each other.
    assert_eq!(cpu.function_name, "add", "cpu object tag = entrypoint");
    assert_eq!(
        cpu.implementation_name, "handler",
        "in-container callable stays the shared dispatch handler"
    );
    assert_eq!(cpu.timeout_secs, 600);
    assert!(
        cpu.resources
            .as_ref()
            .and_then(|r| r.gpu_config.as_ref())
            .is_none(),
        "first entrypoint is CPU (gpu=None)"
    );

    let gpu = creates[1]
        .function
        .as_ref()
        .expect("gpu FunctionCreate function");
    assert_eq!(gpu.function_name, "add_gpu", "gpu object tag = entrypoint");
    assert_eq!(gpu.implementation_name, "handler");
    assert_eq!(gpu.timeout_secs, 1800);
    let gpu_config = gpu
        .resources
        .as_ref()
        .and_then(|r| r.gpu_config.as_ref())
        .expect("second entrypoint requested GPU");
    assert_eq!(gpu_config.gpu_type, "T4");

    // The two object tags are DISTINCT — no override on the platform.
    assert_ne!(
        cpu.function_name, gpu.function_name,
        "distinct Modal object tags per entrypoint (no clobber)"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// DEPLOY counterpart: deploying one app with two divergent-config entrypoints must
/// publish BOTH (distinct object tags + correct per-function gpu/timeout) — the
/// divergent-config rejection is GONE. Asserts two FunctionCreate requests over one
/// shared deploy image, each tagged by its entrypoint and carrying its own config.
#[tokio::test]
async fn divergent_entrypoint_configs_deploy_distinct_functions() {
    use modal_rust::DeployConfig;

    let mock = MockModal::start().await.expect("mock up");
    let tmp = std::env::temp_dir().join(format!(
        "modal-rust-mock-deploy-divergent-{}",
        std::process::id()
    ));
    // Reuse the RUN tiny-source config for local_root/package; deploy reads them for
    // the build context. The divergent decorator configs drive the per-entrypoint plan.
    let run_cfg = tiny_source_config(&tmp);
    let app = App::connect_at_with_configs(
        "divergent-deploy-app",
        modal_registry(),
        divergent_entrypoint_configs(),
        mock.url(),
        run_cfg.clone(),
    )
    .await
    .expect("connect");

    let deploy_cfg = DeployConfig {
        app_name: "divergent-deploy-app".to_string(),
        local_root: run_cfg.local_root.clone(),
        package: run_cfg.package.clone(),
        use_cargo_scoping: false,
        modalignore_name: run_cfg.modalignore_name.clone(),
        base_image: run_cfg.base_image.clone(),
        timeout_secs: 300,
        install_rust: false,
        ..DeployConfig::for_app("divergent-deploy-app")
    };
    let _deployed = app.deploy_with(deploy_cfg).await.expect("deploy ok");

    let creates = mock.requests::<FunctionCreateRequest>();
    assert_eq!(
        creates.len(),
        2,
        "deploy must publish ONE function per entrypoint (no divergent-config rejection)"
    );

    // BTreeMap orders configs by name: "add" (CPU, 600) then "add_gpu" (T4, 1800).
    let add = creates[0].function.as_ref().expect("add function");
    assert_eq!(add.function_name, "add", "add object tag = entrypoint");
    assert_eq!(add.implementation_name, "handler");
    assert_eq!(add.timeout_secs, 600);
    assert!(
        add.resources
            .as_ref()
            .and_then(|r| r.gpu_config.as_ref())
            .is_none(),
        "add is CPU (gpu=None)"
    );

    let add_gpu = creates[1].function.as_ref().expect("add_gpu function");
    assert_eq!(add_gpu.function_name, "add_gpu");
    assert_eq!(add_gpu.implementation_name, "handler");
    assert_eq!(add_gpu.timeout_secs, 1800);
    assert_eq!(
        add_gpu
            .resources
            .as_ref()
            .and_then(|r| r.gpu_config.as_ref())
            .expect("add_gpu requested GPU")
            .gpu_type,
        "T4"
    );
    assert_ne!(add.function_name, add_gpu.function_name);

    let _ = fs::remove_dir_all(&tmp);
}

/// Invoke ROUTING: each entrypoint's `.remote()` must target ITS OWN function_id,
/// not whichever was created first. Drives a mock that returns a DISTINCT function_id
/// per created object tag (`fu-<tag>`), then asserts the two recorded `FunctionMap`
/// requests targeted the matching per-entrypoint function_ids.
#[tokio::test]
async fn divergent_entrypoint_invoke_routes_to_own_function_id() {
    // Return a function_id derived from the created object tag so routing is
    // observable: `add` -> `fu-add`, `add_gpu` -> `fu-add_gpu`.
    let mock = MockModal::builder()
        .on_function_create(|req| {
            let tag = req
                .function
                .as_ref()
                .map(|f| f.function_name.clone())
                .unwrap_or_default();
            Ok(FunctionCreateResponse {
                function_id: format!("fu-{tag}"),
                handle_metadata: Some(FunctionHandleMetadata {
                    definition_id: format!("de-{tag}"),
                    function_name: tag,
                    ..Default::default()
                }),
                ..Default::default()
            })
        })
        .start()
        .await
        .expect("mock up");
    let tmp = std::env::temp_dir().join(format!("modal-rust-mock-routing-{}", std::process::id()));
    let app = App::connect_at_with_configs(
        "routing-app",
        modal_registry(),
        divergent_entrypoint_configs(),
        mock.url(),
        tiny_source_config(&tmp),
    )
    .await
    .expect("connect");

    let _: serde_json::Value = app
        .function("add")
        .remote(serde_json::json!({ "a": 1, "b": 2 }))
        .await
        .expect("cpu remote");
    let _: serde_json::Value = app
        .function("add_gpu")
        .remote(serde_json::json!({ "a": 1, "b": 2 }))
        .await
        .expect("gpu remote");

    let maps = mock.requests::<FunctionMapRequest>();
    assert_eq!(maps.len(), 2, "two invokes => two FunctionMap requests");
    // FunctionMap fires in call order: `add` first, then `add_gpu`. Each routed to
    // ITS OWN function_id — no shared "handler" clobber.
    assert_eq!(
        maps[0].function_id, "fu-add",
        "add routed to its own function"
    );
    assert_eq!(
        maps[1].function_id, "fu-add_gpu",
        "add_gpu routed to its own function (not the CPU one)"
    );
    assert_ne!(maps[0].function_id, maps[1].function_id);

    let _ = fs::remove_dir_all(&tmp);
}

/// Row 6 (P6 cache, table form) — cache ON vs OFF across a 2-row table, each on its
/// OWN loopback port. ON ⇒ one VolumeGetOrCreate(V2, cargo-cache name) + a `/cache`
/// mount; OFF ⇒ zero VolumeGetOrCreate + empty volume_mounts (byte-identical to
/// pre-P6).
#[tokio::test]
async fn cache_on_off_volume_manifest_table() {
    struct CacheCase {
        name: &'static str,
        cache: bool,
        expect_volumes: usize,
    }

    let cases = [
        CacheCase {
            name: "cache-on",
            cache: true,
            expect_volumes: 1,
        },
        CacheCase {
            name: "cache-off",
            cache: false,
            expect_volumes: 0,
        },
    ];

    for c in cases {
        let mock = MockModal::start().await.expect("mock up");
        let tmp = std::env::temp_dir().join(format!(
            "modal-rust-mock-cachetable-{}-{}",
            std::process::id(),
            c.name
        ));
        let mut rc = tiny_source_config(&tmp);
        rc.cache = c.cache;
        // A bare decorator (cache=None) DEFERS to RemoteConfig.cache, so the per-case
        // `rc.cache` is what decides the cargo-cache volume (the override semantics).
        let mut decorator = BTreeMap::new();
        decorator.insert(
            "add".to_string(),
            FunctionConfig {
                gpu: None,
                timeout_secs: Some(600),
                cache: None,
                secrets: &[],
                volumes: &[],
            },
        );
        let app = App::connect_at_with_configs(
            "cache-table-app",
            modal_registry(),
            decorator,
            mock.url(),
            rc,
        )
        .await
        .unwrap_or_else(|e| panic!("case {}: connect: {e}", c.name));

        let _: serde_json::Value = app
            .function("add")
            .remote(serde_json::json!({ "a": 1, "b": 2 }))
            .await
            .unwrap_or_else(|e| panic!("case {}: remote: {e}", c.name));

        let function = mock
            .last::<FunctionCreateRequest>()
            .and_then(|fc| fc.function)
            .unwrap_or_else(|| panic!("case {}: no function", c.name));
        assert_eq!(
            mock.took::<VolumeGetOrCreateRequest>(),
            c.expect_volumes,
            "case {}: VolumeGetOrCreate count",
            c.name
        );
        assert_eq!(
            function.volume_mounts.len(),
            c.expect_volumes,
            "case {}: volume_mounts on the function",
            c.name
        );
        if c.cache {
            let vol = mock
                .last::<VolumeGetOrCreateRequest>()
                .unwrap_or_else(|| panic!("case {}: no volume", c.name));
            assert_eq!(vol.deployment_name, "modal-rust-cargo-cache");
            assert_eq!(vol.version, VolumeFsVersion::V2 as i32);
            assert_eq!(function.volume_mounts[0].mount_path, "/cache");
        }

        let _ = fs::remove_dir_all(&tmp);
    }
}
