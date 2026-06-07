//! Offline proof (zero Modal, zero network) that the image-builder STEPS render into
//! the build image dockerfile.
//!
//! Image deps are a BUILD-path knob, not decorator config — you chain them on
//! [`RemoteConfig::image_steps`] as [`ImageStep`]s (`apt`/`pip`/`run`, mirroring
//! Modal's `apt_install`/`pip_install`/`run_commands`). `App::dry_run` projects exactly
//! the request sequence `.remote()` WOULD send, so we read the rendered image dockerfile
//! straight off the planned `ImageGetOrCreate` (layer 0) and assert:
//!
//! 1. the apt_install / pip_install / run_commands lines are present with their args; and
//! 2. they render in CHAIN ORDER, AFTER provisioning and BEFORE the (baked) wrapper.
//!
//! No live Modal, no credentials.

use modal_rust::{App, ImageStep, PlannedRequest, RemoteConfig};

/// Pull the base/run image layer's (`layer == 0`) rendered dockerfile commands off a
/// dry-run RUN manifest for `render` built with `cfg`.
fn run_image_dockerfile(cfg: &RemoteConfig) -> Vec<String> {
    let app = App::local();
    let manifest = app
        .dry_run("render", cfg)
        .expect("dry_run projects the RUN manifest");
    manifest
        .requests
        .into_iter()
        .find_map(|r| match r {
            PlannedRequest::ImageGetOrCreate {
                dockerfile_commands,
                layer: 0,
            } => Some(dockerfile_commands),
            _ => None,
        })
        .expect("the RUN manifest plans a base image (layer 0)")
}

/// A deterministic RUN config with the three image-builder steps chained.
/// `use_cargo_scoping: false` keeps the projection from shelling out to `cargo
/// metadata`, so the test stays pure + offline.
fn steps_cfg() -> RemoteConfig {
    RemoteConfig {
        package: "example-pip-apt-image".to_string(),
        use_cargo_scoping: false,
        image_steps: vec![
            ImageStep::apt(["libpng-dev", "libjpeg-dev"]),
            ImageStep::pip(["numpy", "pillow"]),
            ImageStep::run(["echo built > /opt/marker"]),
        ],
        ..RemoteConfig::default()
    }
}

#[test]
fn image_steps_render_into_the_run_image_dockerfile_in_chain_order() {
    let cmds = run_image_dockerfile(&steps_cfg());

    // 1. apt_install renders the system packages in a single update+install+clean RUN.
    let apt = cmds
        .iter()
        .position(|c| {
            c.contains("apt-get install")
                && c.contains("libpng-dev")
                && c.contains("libjpeg-dev")
                && c.contains("rm -rf /var/lib/apt/lists/*")
        })
        .expect("apt_install rendered (got {cmds:?})");

    // 2. pip_install renders the Python packages via the universal launcher.
    let pip = cmds
        .iter()
        .position(|c| c == "RUN python3 -m pip install --no-cache-dir numpy pillow")
        .expect("pip_install rendered (got {cmds:?})");

    // 3. run_commands renders the shell command verbatim.
    let run = cmds
        .iter()
        .position(|c| c == "RUN echo built > /opt/marker")
        .expect("run_commands rendered (got {cmds:?})");

    // Provisioning (add_python COPY) precedes the steps; the (baked) wrapper follows.
    let copy = cmds
        .iter()
        .position(|c| c == "COPY /python/. /usr/local")
        .expect("add_python provisioning present");
    let bake = cmds
        .iter()
        .position(|c| c.contains("b64decode("))
        .expect("the wrapper bake (last) present");

    // Chain order is preserved across the three kinds.
    assert!(apt < pip, "apt_install precedes pip_install (chain order)");
    assert!(pip < run, "pip_install precedes run_commands (chain order)");
    // Boundaries: provisioning < steps < wrapper bake.
    assert!(copy < apt, "provisioning precedes the image steps");
    assert!(run < bake, "image steps precede the wrapper bake");

    // The RUN image still builds in-body (no cargo build at image-build time).
    assert!(
        !cmds.iter().any(|c| c.contains("cargo build")),
        "RUN image builds in-body, not at image-build time"
    );
}

#[test]
fn no_image_steps_render_no_extra_lines() {
    // Leaving image_steps empty renders NO apt/pip/run lines — proving the steps above
    // came from the knob, and that the feature is purely additive (byte-identical
    // default path).
    let cfg = RemoteConfig {
        package: "example-pip-apt-image".to_string(),
        use_cargo_scoping: false,
        ..RemoteConfig::default()
    };
    let cmds = run_image_dockerfile(&cfg);
    assert!(
        !cmds.iter().any(|c| c.contains("apt-get install")),
        "no apt step without image_steps (got {cmds:?})"
    );
    assert!(
        !cmds.iter().any(|c| c.contains("pip install")),
        "no pip step without image_steps (got {cmds:?})"
    );
}

#[test]
fn body_is_a_plain_rust_fn() {
    // The macro emits the user fn verbatim, so it stays a plain Rust fn callable with
    // no Modal in the loop — the image deps are build config, not behavior.
    let out = example_pip_apt_image::render(example_pip_apt_image::Job { value: 9 })
        .expect("render runs locally");
    assert_eq!(out.value, 9);
}
