//! Offline proof (zero Modal, zero network) that `enable_memory_snapshot = true` on the
//! `#[cls]` rides into the DEPLOY `FunctionCreate.checkpointing_enabled` — and ONLY on
//! deploy.
//!
//! Modal snapshots DEPLOYED apps, so the flag must take effect on `deploy`, not `run`:
//!
//!   - DEPLOY: every `Concordance.*` entrypoint's planned `FunctionCreate` carries
//!     `checkpointing_enabled = true` (the wire field Modal reads to snapshot the class).
//!   - RUN: the SAME entrypoint's planned `FunctionCreate` carries
//!     `checkpointing_enabled = false` — RUN never snapshots, so the wire stays
//!     byte-identical to a non-snapshot Cls.
//!
//! The facade's public, network-free `App::dump_deploy_manifest` / `App::dry_run` project
//! exactly the request sequence `deploy` / `.remote()` WOULD send, so we read the flag off
//! those manifests. No live Modal, no credentials.

use modal_rust::{App, DeployConfig, Manifest, PlannedRequest, RemoteConfig};
// Bring the generated `ConcordanceCls` trait into scope AND force the example lib (with
// its `#[cls]` inventory submissions) to be linked into this test binary — an integration
// test only links a dependency rlib when it references a symbol from it. The
// `force_link_inventory` helper below touches the generated handle so the per-method
// `Registration`s `from_inventory_with_configs` reads are actually present.
use snapshot_class::*;

/// Reference a symbol from the example lib so its inventory submissions link into this
/// test binary (rlibs are pulled in lazily, only when referenced). `app.concordance()` is
/// the generated handle, which lives alongside the `#[cls]` `inventory::submit!`s — so
/// touching it guarantees `from_inventory_with_configs` sees this crate's entrypoints.
fn force_link_inventory() {
    let app = App::local();
    let _ = app
        .concordance()
        .vocabulary()
        .local()
        .expect("vocabulary().local()");
}

/// Build an `App` from THIS crate's own `#[cls]` inventory submissions — the SAME
/// (registry, configs) the runner assembles. `App::from_manifest` reads the
/// per-entrypoint config via the same `config_for` path `.remote()`/`deploy` use.
fn app_from_inventory() -> App {
    force_link_inventory();
    let (_registry, configs) = modal_rust::from_inventory_with_configs();
    App::from_manifest(
        configs
            .into_iter()
            .map(|(name, options)| (name.to_string(), options)),
    )
}

/// A deterministic build config for the projection: no cargo scoping (so the projection
/// never shells out to `cargo metadata`), cache off (so the manifest stays minimal).
fn run_cfg() -> RemoteConfig {
    RemoteConfig {
        package: "snapshot-class".to_string(),
        use_cargo_scoping: false,
        cache: false,
        ..RemoteConfig::default()
    }
}

fn deploy_cfg() -> DeployConfig {
    DeployConfig {
        package: "snapshot-class".to_string(),
        base_image: "rust:1-slim".to_string(),
        use_cargo_scoping: false,
        ..DeployConfig::for_app("modal-rust-snapshot-class")
    }
}

/// The `checkpointing_enabled` flag of EVERY FunctionCreate in a manifest's request
/// sequence (`FunctionCreate` does not carry the dotted entrypoint name — `function` is
/// the wrapper callable — so we collect the flag across all of them).
fn checkpointing_flags(manifest: &Manifest) -> Vec<bool> {
    manifest
        .requests
        .iter()
        .filter_map(|r| match r {
            PlannedRequest::FunctionCreate {
                checkpointing_enabled,
                ..
            } => Some(*checkpointing_enabled),
            _ => None,
        })
        .collect()
}

#[test]
fn enable_memory_snapshot_rides_into_deploy_function_create() {
    // DEPLOY: the `#[cls(enable_memory_snapshot = true)]` opt-in rides into the wire field
    // Modal reads to snapshot the class — on EVERY entrypoint's FunctionCreate (the class
    // has two: Concordance.search and Concordance.vocabulary).
    let manifest = app_from_inventory()
        .dump_deploy_manifest(&deploy_cfg())
        .expect("dump_deploy_manifest projects the DEPLOY manifest");
    let flags = checkpointing_flags(&manifest);
    assert!(
        !flags.is_empty(),
        "the DEPLOY manifest plans at least one FunctionCreate"
    );
    assert!(
        flags.iter().all(|&on| on),
        "DEPLOY: enable_memory_snapshot rides into checkpointing_enabled on every \
         FunctionCreate, got {flags:?}"
    );
    assert!(
        manifest.render().contains("checkpointing_enabled=true"),
        "the DEPLOY render shows the flag on"
    );
}

#[test]
fn run_manifest_never_snapshots() {
    // RUN: the flag does NOT take effect — RUN never snapshots, so the FunctionCreate is
    // wire-identical to a non-snapshot Cls (checkpointing_enabled stays false). `dry_run`
    // projects ONE entrypoint at a time.
    let manifest = app_from_inventory()
        .dry_run("Concordance.search", &run_cfg())
        .expect("dry_run projects the RUN manifest");
    let flags = checkpointing_flags(&manifest);
    assert!(
        !flags.is_empty(),
        "the RUN manifest plans a FunctionCreate for Concordance.search"
    );
    assert!(
        flags.iter().all(|&on| !on),
        "RUN never snapshots — checkpointing_enabled stays false on the RUN manifest, \
         got {flags:?}"
    );
    assert!(
        manifest.render().contains("checkpointing_enabled=false"),
        "the RUN render shows the flag off"
    );
}
