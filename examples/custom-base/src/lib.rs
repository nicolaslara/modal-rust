//! `examples/custom-base` — pick the RUN base image + install the Rust toolchain
//! through the EXPOSED config knobs.
//!
//! Teaching ONE concept: the base image your code builds against is NOT decorator
//! config — it is a RUN/DEPLOY-path knob you set on [`RemoteConfig`] /
//! [`DeployConfig`], or via two env vars, with NO change to the function body:
//!
//! - `base_image` (env: `MODAL_RUST_BASE_IMAGE`) — the registry tag the run image is
//!   built `FROM`. The default is `rust:<ver>-slim` (Rust already on PATH). Point it
//!   at a CUDA-devel base (`nvidia/cuda:12.6.3-devel-ubuntu22.04`) when your build
//!   needs the CUDA toolkit/headers.
//! - `install_rust` (env: `MODAL_RUST_INSTALL_RUST`) — a CUDA-devel base ships NO
//!   Rust, so the in-body `cargo build` would have no toolchain. Set this and the
//!   facade renders the proven `rustup` install RUN into the image dockerfile (plus
//!   the cargo + CUDA `PATH`/`CUDA_PATH` ENV) so `cargo` is on PATH at build time.
//!
//! Why a knob and not a decorator field: the base image is a property of HOW the crate
//! is built (one image per app), not of WHAT one entrypoint computes — so it rides on
//! the build config, not on `#[function(...)]`. The decorator stays config for the
//! function (gpu/timeout/cache/secrets/volumes); the image is config for the build.
//!
//! The function below is a plain Rust fn that never names Modal, an image, or CUDA. It
//! exists only to give the run image something to build. The lesson is the IMAGE the
//! facade renders, proven OFFLINE by `tests/manifest.rs` (and printed by the
//! `custom_base` driver): the dry-run RUN manifest's image layer starts
//! `FROM nvidia/cuda:12.6.3-devel-ubuntu22.04` and carries the rustup install RUN.

use modal_rust::function;
use serde::{Deserialize, Serialize};

/// Input for [`probe`] — a value to echo back. The workload is irrelevant; the base
/// image (chosen via the build config, not this signature) is the lesson.
#[derive(Debug, Serialize, Deserialize)]
pub struct Probe {
    /// An arbitrary value echoed back so the envelope carries proof the body ran.
    pub value: u64,
}

/// Output for [`probe`].
#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    /// Echoes the input value.
    pub value: u64,
}

/// A plain Rust fn — no GPU, no Modal, no image. It compiles on ANY base that has a
/// Rust toolchain, which is exactly the point: you pick the base image and ask for a
/// toolchain through the build config (`RemoteConfig`/env), not by editing this body.
///
/// Run the `custom_base` driver to see the rendered image dockerfile (`FROM
/// nvidia/cuda:...` + the rustup install RUN) the CUDA-base build config produces.
#[function]
pub fn probe(input: Probe) -> anyhow::Result<Report> {
    Ok(Report { value: input.value })
}
