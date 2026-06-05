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
