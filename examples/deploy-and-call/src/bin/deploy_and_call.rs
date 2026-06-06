//! The run-vs-deploy build boundary, proven OFFLINE.
//!
//! `App::dry_run` and `App::dump_deploy_manifest` project EXACTLY the control-plane
//! request sequence `.remote()` (resp. `deploy`) WOULD send — with NO Modal and NO
//! network. So we read the two manifests side by side and print where the build
//! happens in each:
//!
//! - RUN  (`.remote()`): the image carries NO `cargo build`; the source is mounted
//!   and `cargo build` runs IN the function body, on every cold container.
//! - DEPLOY (`deploy`):  the image's TOP layer runs `cargo build --release` ONCE at
//!   image-build time and bakes the binary; the FunctionCreate attaches the CLIENT
//!   mount ONLY (no source mount) and the app is PUBLISHED persistently. A later
//!   `call` resolves that published function by name and invokes the prebuilt
//!   binary with no rebuild (proven against the mock in `tests/manifest.rs`).
//!
//! Run: `cargo run -p example-deploy-and-call --bin deploy_and_call` (offline; no
//! credentials).

use modal_rust::{App, DeployConfig, PlannedRequest, RemoteConfig};

/// The persistent app name a real deploy would publish under (and a later `call`
/// would resolve by). Only its NAME is used here — the dump never connects.
const DEPLOY_APP: &str = "modal-rust-deploy-and-call-demo";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // `App::local()` builds the app from this crate's `#[modal_rust::function]`
    // inventory with ZERO Modal. `use_cargo_scoping: false` keeps the projection from
    // shelling out to `cargo metadata`, so both dumps stay pure + offline.
    let app = App::local();

    // ---- RUN: where does .remote() build? IN THE BODY (image carries no cargo) ----
    let run_cfg = RemoteConfig {
        package: "example-deploy-and-call".to_string(),
        use_cargo_scoping: false,
        ..RemoteConfig::default()
    };
    let run = app.dry_run("fib", &run_cfg)?;
    let run_image = image_dockerfile(&run, 0);
    let run_builds_in_image = run_image.iter().any(|c| c.contains("cargo build"));
    println!(
        "run:    image builds the binary? {}  (=> .remote() runs cargo build IN the body)",
        run_builds_in_image
    );

    // ---- DEPLOY: where does `deploy` build? AT IMAGE-BUILD TIME (baked once) -------
    let deploy = app.dump_deploy_manifest(&DeployConfig::for_app(DEPLOY_APP))?;
    let deploy_top = image_dockerfile(&deploy, 1);
    let deploy_builds_in_image = deploy_top
        .iter()
        .any(|c| c.contains("cargo build --release"));
    println!(
        "deploy: image builds the binary? {}   (=> the binary is baked ONCE at image-build time)",
        deploy_builds_in_image
    );

    // The DEPLOY FunctionCreate attaches the CLIENT mount ONLY (the prebuilt
    // /app/modal_runner is in the image — no runtime source mount, no cargo at call).
    let (deploy_mounts, deploy_publish) = function_create_and_publish(&deploy);
    println!(
        "deploy: FunctionCreate mounts = {} (client only), published = {:?}",
        deploy_mounts, deploy_publish
    );

    // The headline line check-examples.sh asserts: the boundary in one sentence.
    println!("boundary: deploy builds ONCE at image-build, call invokes with no rebuild");

    // Document the contract this example guarantees.
    assert!(
        !run_builds_in_image,
        "RUN image must NOT carry cargo build (it builds in the function body)"
    );
    assert!(
        deploy_builds_in_image,
        "DEPLOY top layer must carry `cargo build --release` (build at image-build time)"
    );
    assert_eq!(
        deploy_mounts, 1,
        "DEPLOY FunctionCreate attaches the CLIENT mount ONLY"
    );
    assert_eq!(
        deploy_publish.as_deref(),
        Some("deployed"),
        "deploy publishes persistently"
    );

    Ok(())
}

/// The rendered `dockerfile_commands` of the image layer `layer` in `manifest`
/// (exactly what the wire carries).
fn image_dockerfile(manifest: &modal_rust::Manifest, layer: u8) -> Vec<String> {
    manifest
        .requests
        .iter()
        .find_map(|r| match r {
            PlannedRequest::ImageGetOrCreate {
                dockerfile_commands,
                layer: l,
            } if *l == layer => Some(dockerfile_commands.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the manifest plans an image layer {layer}"))
}

/// The DEPLOY FunctionCreate mount count and the AppPublish state.
fn function_create_and_publish(manifest: &modal_rust::Manifest) -> (usize, Option<String>) {
    let mounts = manifest
        .requests
        .iter()
        .find_map(|r| match r {
            PlannedRequest::FunctionCreate {
                mount_ids_count, ..
            } => Some(*mount_ids_count),
            _ => None,
        })
        .expect("the manifest plans a FunctionCreate");
    let publish = manifest.requests.iter().find_map(|r| match r {
        PlannedRequest::AppPublish { app_state } => Some(app_state.to_string()),
        _ => None,
    });
    (mounts, publish)
}
