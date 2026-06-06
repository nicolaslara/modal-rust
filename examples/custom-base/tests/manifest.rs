//! Offline proof (zero Modal, zero network) that the EXPOSED base-image knobs select
//! the run image and render its dockerfile commands.
//!
//! The base image is a BUILD-path knob, not decorator config — you set it on
//! [`RemoteConfig`] (env: `MODAL_RUST_BASE_IMAGE`) and ask for a Rust toolchain via
//! `install_rust` (env: `MODAL_RUST_INSTALL_RUST`). `App::dry_run` projects exactly the
//! request sequence `.remote()` WOULD send, so we read the rendered image dockerfile
//! straight off the planned `ImageGetOrCreate` (layer 0) and assert:
//!
//! 1. it opens `FROM nvidia/cuda:12.6.3-devel-ubuntu22.04` — `base_image` rode in; and
//! 2. it carries the rustup install RUN + a `/root/.cargo/bin` PATH ENV — `install_rust`
//!    rendered the toolchain so the in-body `cargo build` finds `cargo` on a base that
//!    ships none.
//!
//! The knobs are set on an explicit `RemoteConfig` here (deterministic, no process-env
//! mutation); the equivalent `MODAL_RUST_BASE_IMAGE` / `MODAL_RUST_INSTALL_RUST` env
//! path is covered by the facade's own unit tests. No live Modal, no credentials.

use modal_rust::{App, PlannedRequest, RemoteConfig};

const CUDA_DEVEL_BASE: &str = "nvidia/cuda:12.6.3-devel-ubuntu22.04";

/// Pull the base/run image layer's (`layer == 0`) rendered dockerfile commands off a
/// dry-run RUN manifest for `probe` built with `cfg`.
fn base_image_dockerfile(cfg: &RemoteConfig) -> Vec<String> {
    let app = App::local();
    let manifest = app
        .dry_run("probe", cfg)
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

/// A deterministic RUN base config pointed at the CUDA-devel base with the Rust
/// toolchain requested. `use_cargo_scoping: false` keeps the projection from shelling
/// out to `cargo metadata`, so the test stays pure + offline.
fn cuda_cfg() -> RemoteConfig {
    RemoteConfig {
        package: "example-custom-base".to_string(),
        use_cargo_scoping: false,
        base_image: CUDA_DEVEL_BASE.to_string(),
        install_rust: true,
        ..RemoteConfig::default()
    }
}

#[test]
fn base_image_and_install_rust_ride_into_the_image_dockerfile() {
    let cmds = base_image_dockerfile(&cuda_cfg());

    // 1. `base_image` rides into the image's opening FROM line.
    assert_eq!(
        cmds.first().map(String::as_str),
        Some(format!("FROM {CUDA_DEVEL_BASE}").as_str()),
        "base_image selects the dockerfile FROM (got {cmds:?})"
    );

    // 2. `install_rust` renders the rustup install RUN...
    assert!(
        cmds.iter()
            .any(|c| c.contains("rustup.rs") && c.contains("--default-toolchain stable")),
        "install_rust renders the rustup toolchain install (got {cmds:?})"
    );
    // ...and bakes cargo onto PATH so the in-body `cargo build` finds it.
    assert!(
        cmds.iter()
            .any(|c| c.starts_with("ENV PATH=") && c.contains("/root/.cargo/bin")),
        "install_rust bakes /root/.cargo/bin onto PATH (got {cmds:?})"
    );
}

#[test]
fn default_base_has_no_rustup_layer() {
    // The DEFAULT base (`rust:1-slim`) already ships Rust, so leaving the knobs unset
    // renders NO rustup layer — proving the rustup RUN above came from the knob, and
    // that the feature is purely additive (byte-identical default path).
    let cfg = RemoteConfig {
        package: "example-custom-base".to_string(),
        use_cargo_scoping: false,
        ..RemoteConfig::default()
    };
    let cmds = base_image_dockerfile(&cfg);

    assert!(
        cmds.first()
            .map(|c| c.starts_with("FROM rust:"))
            .unwrap_or(false),
        "the default base is a rust:* image (got {cmds:?})"
    );
    assert!(
        !cmds.iter().any(|c| c.contains("rustup.rs")),
        "the default rust base renders NO rustup layer (got {cmds:?})"
    );
}

#[test]
fn body_is_a_plain_rust_fn() {
    // The macro emits the user fn verbatim, so it stays a plain Rust fn callable with
    // no Modal in the loop — the base image is build config, not behavior.
    let report = example_custom_base::probe(example_custom_base::Probe { value: 7 })
        .expect("probe runs locally");
    assert_eq!(report.value, 7);
}
