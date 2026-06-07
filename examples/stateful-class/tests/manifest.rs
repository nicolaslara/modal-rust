//! Offline proof (zero Modal, zero network) that each `#[method]` of the `#[cls]` rides
//! into the planned `FunctionCreate` manifest as its OWN entrypoint under the dotted
//! `"<Class>.<method>"` name, carrying its fully-resolved (class-default +
//! method-override) config.
//!
//! `#[cls(gpu = "T4", timeout = 600)]` is the class default; `#[method(gpu = "A10G")]`
//! on `embed` overrides the gpu for that method only. The facade's public, network-free
//! `App::dry_run` projects exactly the request sequence `.remote()` WOULD send — so we
//! assert that:
//!
//!   - `Embedder.embed` -> FunctionCreate gpu == A10G  (method override)
//!   - `Embedder.dim`   -> FunctionCreate gpu == T4     (class default, inherited)
//!
//! Both inherit `timeout = 600` from the class. No live Modal, no credentials.

use modal_rust::{App, PlannedRequest, RemoteConfig};
// Bring the generated `EmbedderCls` trait into scope AND force the example lib (with its
// `#[cls]` inventory submissions) to be linked into this test binary — an integration
// test only links a dependency rlib when it references a symbol from it. The
// `force_link_inventory` helper below touches the generated handle so the per-method
// `Registration`s `from_inventory_with_configs` reads are actually present.
use stateful_class::*;

/// Reference a symbol from the example lib so its inventory submissions link into this
/// test binary (rlibs are pulled in lazily, only when referenced). `app.embedder()` is
/// the generated handle, which lives alongside the `#[cls]` `inventory::submit!`s — so
/// touching it guarantees `from_inventory_with_configs` sees this crate's entrypoints.
fn force_link_inventory() {
    let app = App::local();
    let _ = app.embedder().dim().local().expect("dim().local()");
}

/// A deterministic RUN config for the projection. No cargo scoping so the projection
/// never shells out to `cargo metadata`; cache off so the manifest stays minimal.
fn run_cfg() -> RemoteConfig {
    RemoteConfig {
        package: "stateful-class".to_string(),
        use_cargo_scoping: false,
        cache: false,
        ..RemoteConfig::default()
    }
}

/// The (gpu, timeout) the planned `FunctionCreate` carries for `entrypoint`.
fn function_create_gpu_timeout(entrypoint: &str) -> (Option<String>, u32) {
    // Ensure the example lib (and its `#[cls]` inventory submissions) is linked in.
    force_link_inventory();
    // The example's OWN `#[cls]` submissions, collected from inventory — the SAME
    // (registry, configs) the runner assembles. `App::from_manifest` reads the
    // per-entrypoint config via the same `config_for` path `.remote()` uses.
    let (_registry, configs) = modal_rust::from_inventory_with_configs();
    let app = App::from_manifest(
        configs
            .into_iter()
            .map(|(name, options)| (name.to_string(), options)),
    );

    let manifest = app
        .dry_run(entrypoint, &run_cfg())
        .unwrap_or_else(|e| panic!("dry_run projects the RUN manifest for {entrypoint:?}: {e}"));

    manifest
        .requests
        .iter()
        .find_map(|r| match r {
            PlannedRequest::FunctionCreate {
                gpu, timeout_secs, ..
            } => Some((gpu.clone(), *timeout_secs)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the manifest for {entrypoint:?} plans a FunctionCreate"))
}

#[test]
fn embed_method_entrypoint_rides_with_overridden_gpu() {
    // `embed` is its OWN dotted entrypoint; its `#[method(gpu = "A10G")]` override beats
    // the class default, while `timeout = 600` is inherited from `#[cls]`.
    let (gpu, timeout) = function_create_gpu_timeout("Embedder.embed");
    assert_eq!(
        gpu.as_deref(),
        Some("A10G"),
        "embed's #[method(gpu=\"A10G\")] override rode into FunctionCreate"
    );
    assert_eq!(
        timeout, 600,
        "embed inherits timeout=600 from the #[cls] class default"
    );
}

#[test]
fn dim_method_entrypoint_rides_with_inherited_class_config() {
    // `dim` is a bare `#[method]`, so it inherits BOTH gpu=T4 and timeout=600 from the
    // class — and rides into its own dotted entrypoint just like a free fn.
    let (gpu, timeout) = function_create_gpu_timeout("Embedder.dim");
    assert_eq!(
        gpu.as_deref(),
        Some("T4"),
        "dim inherits gpu=T4 from the #[cls] class default"
    );
    assert_eq!(
        timeout, 600,
        "dim inherits timeout=600 from the #[cls] class default"
    );
}
