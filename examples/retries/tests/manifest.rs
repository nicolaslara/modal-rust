//! Offline proof (zero Modal, zero network) that the retry knob on the decorator
//! rides into the planned `FunctionCreate` manifest.
//!
//! `#[modal_rust::function(retries = 5)]` records `retries = Some(5)` on the
//! entrypoint's config. The facade's public, network-free `App::dry_run` projects
//! exactly the request sequence `.remote()` WOULD send — so we assert that for the
//! decorated `fetch` entrypoint the planned `FunctionCreate` carries `retries == 5`.
//! No live Modal, no credentials.

use modal_rust::{App, PlannedRequest, RemoteConfig};

/// A deterministic RUN config for the projection. No cargo scoping so the projection
/// never shells out to `cargo metadata`; cache off so the manifest stays minimal.
fn run_cfg() -> RemoteConfig {
    RemoteConfig {
        package: "example-retries".to_string(),
        use_cargo_scoping: false,
        cache: false,
        ..RemoteConfig::default()
    }
}

#[test]
fn retries_ride_into_function_create() {
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
        .dry_run("fetch", &run_cfg())
        .expect("dry_run projects the RUN manifest");

    // The decorator's `retries = 5` rode into FunctionCreate as the retry count.
    let retries = manifest
        .requests
        .iter()
        .find_map(|r| match r {
            PlannedRequest::FunctionCreate { retries, .. } => Some(*retries),
            _ => None,
        })
        .expect("the manifest plans a FunctionCreate");
    assert_eq!(
        retries,
        Some(5),
        "the decorator's `retries = 5` rode into FunctionCreate's retry_policy"
    );
}

#[test]
fn body_self_heals_once_the_flaky_step_settles() {
    // The macro emits the user fn verbatim, so it stays a plain Rust fn. Early attempts
    // fail (transient); from SETTLES_AT on it succeeds — exactly the failure shape the
    // retry policy absorbs. The retry COUNT is metadata, not behavior, so the body
    // itself just returns Err then Ok across attempts.
    let fail = example_retries::fetch(example_retries::Request {
        resource: "weights.bin".to_string(),
        attempt: 1,
    });
    assert!(fail.is_err(), "an early attempt fails transiently");

    let ok = example_retries::fetch(example_retries::Request {
        resource: "weights.bin".to_string(),
        attempt: example_retries::SETTLES_AT,
    })
    .expect("the call heals once the flaky step settles");
    assert_eq!(ok.resource, "weights.bin");
    assert_eq!(ok.attempt, example_retries::SETTLES_AT);
}
