//! Custom base image, proven OFFLINE.
//!
//! The `#[modal_rust::function] fn probe` from this crate's `lib.rs` is a plain Rust
//! fn. The lesson is the IMAGE it would be built in — which you choose through the
//! BUILD config, not the decorator:
//!
//! - `RemoteConfig.base_image` (env `MODAL_RUST_BASE_IMAGE`) — the registry tag the
//!   run image is `FROM`. Here: a CUDA-devel base that ships NO Rust.
//! - `RemoteConfig.install_rust` (env `MODAL_RUST_INSTALL_RUST`) — ask the facade to
//!   rustup-install a toolchain into that base so the in-body `cargo build` has one.
//!
//! `App::dry_run` projects EXACTLY the request sequence `.remote()` would send, with
//! NO Modal and NO network — so we read the rendered image dockerfile straight off the
//! planned manifest and print it. The same projection is asserted by
//! `tests/manifest.rs`.
//!
//! Run: `cargo run -p example-custom-base --bin custom_base` (offline; no credentials).

use modal_rust::{App, PlannedRequest, RemoteConfig};

/// The CUDA-devel base used in the live burn-add deploy — it carries the CUDA toolkit
/// + headers but NO Rust toolchain, so it is the textbook case for `install_rust`.
const CUDA_DEVEL_BASE: &str = "nvidia/cuda:12.6.3-devel-ubuntu22.04";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The build config — the EXPOSED knobs, set explicitly here (the env vars
    // MODAL_RUST_BASE_IMAGE / MODAL_RUST_INSTALL_RUST do the same thing without code).
    // `use_cargo_scoping: false` keeps the projection from shelling out to
    // `cargo metadata`, so this stays a pure, offline render.
    let cfg = RemoteConfig {
        package: "example-custom-base".to_string(),
        use_cargo_scoping: false,
        base_image: CUDA_DEVEL_BASE.to_string(),
        install_rust: true,
        ..RemoteConfig::default()
    };

    // `App::local()` builds the app from the `#[modal_rust::function]` inventory with
    // ZERO Modal. `dry_run` projects the RUN manifest `.remote()` WOULD send.
    let app = App::local();
    let manifest = app.dry_run("probe", &cfg)?;

    // The base/run image is layer 0. Its `dockerfile_commands` are exactly what the
    // wire carries — so the `FROM <base>` line and the rustup install RUN are visible.
    let dockerfile = manifest
        .requests
        .iter()
        .find_map(|r| match r {
            PlannedRequest::ImageGetOrCreate {
                dockerfile_commands,
                layer: 0,
            } => Some(dockerfile_commands.clone()),
            _ => None,
        })
        .expect("the RUN manifest plans a base image (layer 0)");

    let from = dockerfile
        .first()
        .expect("the dockerfile opens with a FROM line");
    let rustup = dockerfile
        .iter()
        .find(|c| c.contains("rustup.rs"))
        .expect("install_rust renders the rustup install RUN");

    println!("base:   {from}");
    println!("rustup: {rustup}");

    // Document the contract this example guarantees: the CUDA-devel base, and a Rust
    // toolchain installed into it via the exposed knob.
    assert_eq!(
        from,
        &format!("FROM {CUDA_DEVEL_BASE}"),
        "base_image rides into the image dockerfile FROM line"
    );
    assert!(
        rustup.contains("--default-toolchain stable"),
        "install_rust renders the rustup toolchain install"
    );

    Ok(())
}
