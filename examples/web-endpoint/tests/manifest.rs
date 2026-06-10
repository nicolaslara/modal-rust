//! Offline proof (zero Modal, zero network) that `#[endpoint(method = "POST")]` takes
//! effect on the DEPLOY boundary — and ONLY there (D5: the URL is deploy-only in v0).
//!
//!   - DEPLOY: the entrypoint's planned `FunctionCreate` carries
//!     `webhook_method = Some("POST")` — the dump projects it off the SAME
//!     `FunctionSpec.webhook` the live path sends, which is ALSO the field that makes
//!     the SDK's request builder attach `webhook_config { type: FUNCTION, method }`
//!     (proto field 15) AND swap the advertised data formats to the ASGI pair
//!     (`supported_input_formats = [ASGI]`, `supported_output_formats = [ASGI,
//!     GENERATOR_DONE]`) — the format swap is keyed on that one field and unit-proven
//!     at the SDK seam (`ops/function.rs::with_webhook_rides_config_and_swaps_formats_
//!     to_asgi`). So `webhook_method = Some` here ⇒ the webhook config + ASGI formats
//!     ride this FunctionCreate on the wire.
//!   - RUN: the SAME decorated entrypoint plans `webhook_method = None` — the facade
//!     suppresses the webhook on the RUN boundary, so the wire (config AND formats)
//!     stays byte-identical to a plain `#[function]`, exactly like
//!     `enable_memory_snapshot`.
//!
//! The facade's public, network-free `App::dump_deploy_manifest` / `App::dry_run`
//! project exactly the request sequence `deploy` / `.remote()` WOULD send, so we read
//! the webhook off those manifests. No live Modal, no credentials.

use modal_rust::{App, DeployConfig, Manifest, PlannedRequest, RemoteConfig};
// Bring the generated `SummarizeCall` trait into scope AND force the example lib (with
// its `#[endpoint]` inventory submission) to be linked into this test binary — an
// integration test only links a dependency rlib when it references a symbol from it.
use web_endpoint::*;

/// Reference a symbol from the example lib so its inventory submission links into this
/// test binary (rlibs are pulled in lazily, only when referenced). The typed
/// `app.summarize(..)` extension lives alongside the `#[endpoint]` `inventory::submit!`,
/// so exercising it guarantees `from_inventory_with_configs` sees this entrypoint.
fn force_link_inventory() {
    let app = App::local();
    let _ = app
        .summarize("Linkage proof. Offline.".to_string(), 1)
        .local()
        .expect("summarize().local()");
}

/// Build an `App` from THIS crate's own `#[endpoint]` inventory submission — the SAME
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
        package: "web-endpoint".to_string(),
        use_cargo_scoping: false,
        cache: false,
        ..RemoteConfig::default()
    }
}

fn deploy_cfg() -> DeployConfig {
    DeployConfig {
        package: "web-endpoint".to_string(),
        base_image: "rust:1-slim".to_string(),
        use_cargo_scoping: false,
        ..DeployConfig::for_app("modal-rust-web-endpoint")
    }
}

/// The `webhook_method` of EVERY FunctionCreate in a manifest's request sequence
/// (`FunctionCreate` does not carry the dotted entrypoint name — `function` is the
/// object tag — so we collect the field across all of them).
fn webhook_methods(manifest: &Manifest) -> Vec<Option<String>> {
    manifest
        .requests
        .iter()
        .filter_map(|r| match r {
            PlannedRequest::FunctionCreate { webhook_method, .. } => Some(webhook_method.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn webhook_method_rides_into_deploy_function_create() {
    // DEPLOY: `#[endpoint(method = "POST")]` rides into the planned FunctionCreate —
    // the wire request that carries `webhook_config{type: FUNCTION, method: "POST"}`
    // and advertises the ASGI formats (both keyed on the same `FunctionSpec.webhook`;
    // see the module docs for the SDK-seam proof of the format swap).
    let manifest = app_from_inventory()
        .dump_deploy_manifest(&deploy_cfg())
        .expect("dump_deploy_manifest projects the DEPLOY manifest");
    let methods = webhook_methods(&manifest);
    assert!(
        !methods.is_empty(),
        "the DEPLOY manifest plans at least one FunctionCreate"
    );
    assert!(
        methods.iter().all(|m| m.as_deref() == Some("POST")),
        "DEPLOY: the endpoint method rides into every FunctionCreate, got {methods:?}"
    );
    assert!(
        manifest.render().contains("webhook_method=Some(\"POST\")"),
        "the DEPLOY render shows the webhook method"
    );

    // The object TAG stays the entrypoint name ("summarize") — `modal-rust call`
    // and the typed `.remote()` path resolve the SAME function the URL fronts; the
    // per-endpoint web adapter swap rides `implementation_name`, not the tag.
    let tags: Vec<String> = manifest
        .requests
        .iter()
        .filter_map(|r| match r {
            PlannedRequest::FunctionCreate { function, .. } => Some(function.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(tags, vec!["summarize"], "object TAG stays the entrypoint");
}

#[test]
fn run_manifest_suppresses_the_webhook() {
    // RUN: the decorator does NOT take effect — the URL is deploy-only (D5), so the
    // planned FunctionCreate stays wire-identical to a plain `#[function]`: no webhook
    // config, and therefore the PICKLE/CBOR formats (not ASGI). `dry_run` projects ONE
    // entrypoint at a time.
    let manifest = app_from_inventory()
        .dry_run("summarize", &run_cfg())
        .expect("dry_run projects the RUN manifest");
    let methods = webhook_methods(&manifest);
    assert!(
        !methods.is_empty(),
        "the RUN manifest plans a FunctionCreate for summarize"
    );
    assert!(
        methods.iter().all(|m| m.is_none()),
        "RUN suppresses the webhook — wire-identical to a plain #[function], \
         got {methods:?}"
    );
    assert!(
        manifest.render().contains("webhook_method=None"),
        "the RUN render shows the webhook off"
    );
}
