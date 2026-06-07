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
fn body_is_a_plain_rust_fn_that_computes_a_real_embedding() {
    // The macro emits the user fn verbatim, so it stays a plain Rust fn with no scaling
    // logic — autoscaling is metadata, not behavior. The body runs the real embedding
    // model: a fixed-width, unit-length feature vector, computed deterministically.
    let text = "the quick brown fox";
    let out = example_autoscaling::embed(example_autoscaling::Document {
        text: text.to_string(),
    })
    .expect("embed succeeds");

    // The input is carried through; the vector is the model's real output.
    assert_eq!(out.text, text);

    // Fixed width: the vector is exactly EMBED_DIMENSIONS long, and `dimensions`
    // reports its real length.
    assert_eq!(out.vector.len(), example_autoscaling::EMBED_DIMENSIONS);
    assert_eq!(out.dimensions, example_autoscaling::EMBED_DIMENSIONS);

    // Real compute, not an echo or a constant: a multi-word input produces a non-zero
    // vector.
    assert!(
        out.vector.iter().any(|&x| x != 0.0),
        "a non-empty text embeds to a non-zero vector"
    );

    // Unit length: for non-empty text the embedding is L2-normalized (sum of squares
    // ~= 1.0).
    let norm_sq: f32 = out.vector.iter().map(|x| x * x).sum();
    assert!(
        (norm_sq - 1.0).abs() < 1e-5,
        "embedding is L2-normalized to unit length (sum of squares = {norm_sq})"
    );

    // Deterministic: embedding the same text again yields the identical vector.
    let again = example_autoscaling::embed(example_autoscaling::Document {
        text: text.to_string(),
    })
    .expect("embed succeeds");
    assert_eq!(
        out.vector, again.vector,
        "embedding is deterministic across calls"
    );
}
