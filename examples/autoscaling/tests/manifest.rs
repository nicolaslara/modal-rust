//! Offline proof (zero Modal, zero network) that the autoscaler knobs on the decorator
//! ride into the planned `FunctionCreate` manifest.
//!
//! `#[modal_rust::function(min_containers = 1, max_containers = 10, buffer_containers =
//! 2, scaledown_window = 120)]` records those knobs on the entrypoint's config. The
//! facade's public, network-free `App::dry_run` projects exactly the request sequence
//! `.remote()` WOULD send — so we assert that for the decorated `embed` entrypoint the
//! planned `FunctionCreate` carries each autoscaler knob. No live Modal, no credentials.

use modal_rust::{App, PlannedRequest, RemoteConfig};

/// A deterministic RUN config for the projection. No cargo scoping so the projection
/// never shells out to `cargo metadata`; cache off so the manifest stays minimal.
fn run_cfg() -> RemoteConfig {
    RemoteConfig {
        package: "example-autoscaling".to_string(),
        use_cargo_scoping: false,
        cache: false,
        ..RemoteConfig::default()
    }
}

#[test]
fn autoscaler_knobs_ride_into_function_create() {
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
        .dry_run("embed", &run_cfg())
        .expect("dry_run projects the RUN manifest");

    // The decorator's autoscaler knobs rode into FunctionCreate's autoscaler_settings.
    let knobs = manifest
        .requests
        .iter()
        .find_map(|r| match r {
            PlannedRequest::FunctionCreate {
                min_containers,
                max_containers,
                buffer_containers,
                scaledown_window,
                ..
            } => Some((
                *min_containers,
                *max_containers,
                *buffer_containers,
                *scaledown_window,
            )),
            _ => None,
        })
        .expect("the manifest plans a FunctionCreate");
    assert_eq!(
        knobs,
        (Some(1), Some(10), Some(2), Some(120)),
        "the decorator's autoscaler knobs rode into FunctionCreate's autoscaler_settings"
    );
}

#[test]
fn body_is_a_plain_rust_fn() {
    // The macro emits the user fn verbatim, so it stays a plain Rust fn with no scaling
    // logic — autoscaling is metadata, not behavior. The body just maps input to output.
    let out = example_autoscaling::embed(example_autoscaling::Document {
        text: "the quick brown fox".to_string(),
    })
    .expect("embed succeeds");
    assert_eq!(out.text, "the quick brown fox");
    assert_eq!(out.dimensions, example_autoscaling::EMBED_DIMENSIONS);
}
