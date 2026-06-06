//! Offline proof (zero Modal, zero network) that the named secret on the decorator
//! rides into the `FunctionCreate` manifest.
//!
//! `#[modal_rust::function(secrets = ["my-api-key"])]` records the secret on the
//! entrypoint's config. The facade's public, network-free `App::dry_run` projects
//! exactly the request sequence `.remote()` WOULD send — so we assert that for the
//! decorated `check_secret` entrypoint the planned manifest both RESOLVES the named
//! secret (`SecretGetOrCreate { name: "my-api-key" }`) and rides its id into the
//! `FunctionCreate` (`secret_count == 1`). No live Modal, no credentials.

use modal_rust::{App, PlannedRequest, RemoteConfig};

/// A deterministic RUN config for the dump: no cargo scoping (so the projection
/// never shells out to `cargo metadata`) and cache off (so the only secret/volume
/// in the manifest is the one the decorator names).
fn run_cfg() -> RemoteConfig {
    RemoteConfig {
        package: "example-secrets".to_string(),
        use_cargo_scoping: false,
        cache: false,
        ..RemoteConfig::default()
    }
}

#[test]
fn named_secret_rides_into_function_create() {
    // The example's OWN decorator submissions, collected from inventory — the SAME
    // (registry, configs) the runner assembles. `App::from_manifest` reads the
    // per-entrypoint config via the same `config_for` path `.remote()` uses.
    let (_registry, configs) = modal_rust::from_inventory_with_configs();
    let app = App::from_manifest(
        configs
            .into_iter()
            .map(|(name, options)| (name.to_string(), options)),
    );

    let manifest = app
        .dry_run("check_secret", &run_cfg())
        .expect("dry_run projects the RUN manifest");

    // 1. The named secret is resolved before FunctionCreate.
    let resolved = manifest
        .requests
        .iter()
        .any(|r| matches!(r, PlannedRequest::SecretGetOrCreate { name } if name == "my-api-key"));
    assert!(
        resolved,
        "the decorator's `my-api-key` secret is resolved (SecretGetOrCreate)"
    );

    // 2. Its id rides into the FunctionCreate manifest.
    let secret_count = manifest
        .requests
        .iter()
        .find_map(|r| match r {
            PlannedRequest::FunctionCreate { secret_count, .. } => Some(*secret_count),
            _ => None,
        })
        .expect("the manifest plans a FunctionCreate");
    assert_eq!(
        secret_count, 1,
        "the resolved secret id rode into FunctionCreate"
    );
}

#[test]
fn plain_fn_reads_missing_secret_as_absent() {
    // The macro emits the user fn verbatim, so it stays a plain Rust fn. With no
    // secret attached locally, `std::env::var` is absent and the report says so —
    // proving the body just reads the environment (no Modal in the loop).
    std::env::remove_var("MY_API_KEY");
    let report = example_secrets::check_secret(example_secrets::Request {}).unwrap();
    assert!(!report.present);
    assert_eq!(report.len, 0);
}
