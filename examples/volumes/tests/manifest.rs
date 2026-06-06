//! Offline proof (zero Modal, zero network) that the named volume on the decorator
//! rides into the `FunctionCreate` manifest at its mount path.
//!
//! `#[modal_rust::function(volumes = ["/data=my-vol"])]` records the `(mount_path,
//! name)` pair on the entrypoint's config. The facade's public, network-free
//! `App::dry_run` projects exactly the request sequence `.remote()` WOULD send — so
//! we assert that for the decorated `record_visit` entrypoint the planned manifest
//! both RESOLVES the named volume (`VolumeGetOrCreate { name: "my-vol", v2: false }`,
//! a V1 user volume) and rides its mount into the `FunctionCreate` at `/data`. No
//! live Modal, no credentials.

use modal_rust::{App, PlannedRequest, RemoteConfig};

/// A deterministic RUN config for the projection: no cargo scoping (so it never
/// shells out to `cargo metadata`) and cache off (so the cargo-cache V2 volume is
/// absent and the ONLY volume in the manifest is the user volume the decorator names).
fn run_cfg() -> RemoteConfig {
    RemoteConfig {
        package: "example-volumes".to_string(),
        use_cargo_scoping: false,
        cache: false,
        ..RemoteConfig::default()
    }
}

#[test]
fn mounted_volume_rides_into_function_create() {
    // Touch the library's public surface so the test binary links its
    // `#[modal_rust::function]` inventory submissions — the SAME (registry, configs)
    // the runner assembles. (Referencing a type from the crate is enough to pull in
    // its inventory.)
    let _ = std::mem::size_of::<example_volumes::Visit>();

    // The example's OWN decorator submissions, collected from inventory.
    // `App::from_manifest` reads the per-entrypoint config via the same `config_for`
    // path `.remote()` uses.
    let (_registry, configs) = modal_rust::from_inventory_with_configs();
    let app = App::from_manifest(
        configs
            .into_iter()
            .map(|(name, options)| (name.to_string(), options)),
    );

    let manifest = app
        .dry_run("record_visit", &run_cfg())
        .expect("dry_run projects the RUN manifest");

    // 1. The named volume is resolved before FunctionCreate as a V1 user volume
    //    (NOT the V2 cargo cache).
    let resolved = manifest.requests.iter().any(
        |r| matches!(r, PlannedRequest::VolumeGetOrCreate { name, v2 } if name == "my-vol" && !*v2),
    );
    assert!(
        resolved,
        "the decorator's `my-vol` volume is resolved (VolumeGetOrCreate, V1)"
    );

    // 2. Its mount rides into the FunctionCreate manifest at /data.
    let volume_mounts = manifest
        .requests
        .iter()
        .find_map(|r| match r {
            PlannedRequest::FunctionCreate { volume_mounts, .. } => Some(volume_mounts.clone()),
            _ => None,
        })
        .expect("the manifest plans a FunctionCreate");
    assert!(
        volume_mounts.iter().any(|(path, _)| path == "/data"),
        "the resolved volume mount rode into FunctionCreate at /data (got {volume_mounts:?})"
    );
}
