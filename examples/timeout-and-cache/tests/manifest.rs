//! Offline proof (zero Modal, zero network) that the two operational knobs on the
//! decorator ride into the planned `FunctionCreate` manifest.
//!
//! `#[modal_rust::function(timeout = 1800, cache = true)]` records `timeout_secs =
//! Some(1800)` and `cache = Some(true)` on the entrypoint's config. The facade's
//! public, network-free `App::dry_run` projects exactly the request sequence
//! `.remote()` WOULD send — so we assert that for the decorated `spin` entrypoint the
//! planned manifest does two things.
//!
//! 1. Carries the decorator's `timeout_secs == 1800` into the `FunctionCreate` (and
//!    that it OVERRIDES the run-path base timeout, proving the knob is read).
//! 2. Resolves the shared cargo BUILD cache (`VolumeGetOrCreate { name:
//!    "modal-rust-cargo-cache", v2: true }`) and rides its `/cache` mount into the
//!    `FunctionCreate` — i.e. `cache = true` is honored.
//!
//! No live Modal, no credentials.

use modal_rust::{App, PlannedRequest, RemoteConfig};

/// A deterministic RUN base config for the projection. `timeout_secs` is set to a
/// value DIFFERENT from the decorator's `1800` so the asserted `1800` can only come
/// from the decorator override (not this base). `cache: false` here is the run-path
/// BASE default; the decorator's `cache = true` overrides it, so the proof that the
/// cargo-cache volume rides comes from the decorator, not the base. No cargo scoping
/// so the projection never shells out to `cargo metadata`.
fn run_cfg() -> RemoteConfig {
    RemoteConfig {
        package: "example-timeout-and-cache".to_string(),
        use_cargo_scoping: false,
        timeout_secs: 300,
        cache: false,
        ..RemoteConfig::default()
    }
}

#[test]
fn timeout_and_cache_ride_into_function_create() {
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
        .dry_run("spin", &run_cfg())
        .expect("dry_run projects the RUN manifest");

    // 1. The decorator's timeout rides into FunctionCreate and OVERRIDES the base 300.
    let timeout_secs = manifest
        .requests
        .iter()
        .find_map(|r| match r {
            PlannedRequest::FunctionCreate { timeout_secs, .. } => Some(*timeout_secs),
            _ => None,
        })
        .expect("the manifest plans a FunctionCreate");
    assert_eq!(
        timeout_secs, 1800,
        "the decorator's `timeout = 1800` rode into FunctionCreate, overriding the base"
    );

    // 2. `cache = true` resolves the shared cargo BUILD cache as a V2 volume...
    let cache_volume = manifest.requests.iter().any(|r| {
        matches!(
            r,
            PlannedRequest::VolumeGetOrCreate { name, v2 }
                if name == "modal-rust-cargo-cache" && *v2
        )
    });
    assert!(
        cache_volume,
        "the build cache resolves the shared cargo-cache volume (VolumeGetOrCreate, V2)"
    );

    // ...and that cache mounts at /cache in the FunctionCreate.
    let volume_mounts = manifest
        .requests
        .iter()
        .find_map(|r| match r {
            PlannedRequest::FunctionCreate { volume_mounts, .. } => Some(volume_mounts.clone()),
            _ => None,
        })
        .expect("the manifest plans a FunctionCreate");
    assert!(
        volume_mounts.iter().any(|(path, _)| path == "/cache"),
        "the cargo-cache volume rode into FunctionCreate at /cache (got {volume_mounts:?})"
    );
}

#[test]
fn body_is_a_plain_rust_fn() {
    // The macro emits the user fn verbatim, so it stays a plain Rust fn callable with
    // no Modal in the loop — the knobs are operational metadata, not behavior.
    let done = example_timeout_and_cache::spin(example_timeout_and_cache::Job { iterations: 0 })
        .expect("spin runs locally");
    assert_eq!(done.iterations, 0);
    assert_eq!(done.checksum, 0);
}
