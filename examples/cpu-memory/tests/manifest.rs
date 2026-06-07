//! Offline proof (zero Modal, zero network) that the two resource knobs on the
//! decorator ride into the planned `FunctionCreate` manifest.
//!
//! `#[modal_rust::function(cpu = 2.0, memory = 4096)]` records `milli_cpu = Some(2000)`
//! (`int(1000 * cpu)`, mirroring Modal) and `memory_mb = Some(4096)` on the
//! entrypoint's config. The facade's public, network-free `App::dry_run` projects
//! exactly the request sequence `.remote()` WOULD send — so we assert that for the
//! decorated `crunch` entrypoint the planned `FunctionCreate` carries `milli_cpu ==
//! 2000` and `memory_mb == 4096`. No live Modal, no credentials.

use modal_rust::{App, PlannedRequest, RemoteConfig};

/// A deterministic RUN config for the projection. No cargo scoping so the projection
/// never shells out to `cargo metadata`; cache off so the manifest stays minimal.
fn run_cfg() -> RemoteConfig {
    RemoteConfig {
        package: "example-cpu-memory".to_string(),
        use_cargo_scoping: false,
        cache: false,
        ..RemoteConfig::default()
    }
}

#[test]
fn cpu_and_memory_ride_into_function_create() {
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
        .dry_run("crunch", &run_cfg())
        .expect("dry_run projects the RUN manifest");

    // The decorator's `cpu = 2.0` / `memory = 4096` rode into FunctionCreate as
    // milli-cores / MiB.
    let (milli_cpu, memory_mb) = manifest
        .requests
        .iter()
        .find_map(|r| match r {
            PlannedRequest::FunctionCreate {
                milli_cpu,
                memory_mb,
                ..
            } => Some((*milli_cpu, *memory_mb)),
            _ => None,
        })
        .expect("the manifest plans a FunctionCreate");
    assert_eq!(
        milli_cpu, 2000,
        "the decorator's `cpu = 2.0` rode into FunctionCreate as milli_cpu = 2000"
    );
    assert_eq!(
        memory_mb, 4096,
        "the decorator's `memory = 4096` rode into FunctionCreate as memory_mb"
    );
}

#[test]
fn body_is_a_plain_rust_fn() {
    // The macro emits the user fn verbatim, so it stays a plain Rust fn callable with
    // no Modal in the loop — the resource knobs are metadata, not behavior.
    let summary = example_cpu_memory::crunch(example_cpu_memory::Batch { records: 0 })
        .expect("crunch runs locally");
    assert_eq!(summary.records, 0);
    assert_eq!(summary.checksum, 0);
}
