//! `examples/pip-apt-image` — add arbitrary system / Python dependencies to the build
//! image with real image-builder steps.
//!
//! Teaching ONE concept: a Rust binary often needs deps that are NOT Rust crates — a
//! system shared library it dynamically links (`libpng`, `libssl`, …), a CLI tool a
//! build step shells out to, or even a Python package a sidecar uses. Modal Python
//! adds these with `Image.apt_install(...)` / `.pip_install(...)` / `.run_commands(...)`.
//! `modal-rust` mirrors that with image-builder STEPS on the BUILD config:
//!
//! - [`ImageStep::apt`] — system packages (`apt-get install`), e.g. `["libpng-dev"]`.
//! - [`ImageStep::pip`] — Python packages (`pip install`), e.g. `["numpy"]`.
//! - [`ImageStep::run`] — arbitrary build-time shell commands (`RUN <cmd>`).
//!
//! You chain them on [`RemoteConfig::image_steps`] (RUN) / `DeployConfig::image_steps`
//! (DEPLOY); the facade renders them into the image dockerfile IN CHAIN ORDER, AFTER
//! the Python/Rust provisioning and BEFORE the build — so the deps exist when the
//! in-body `cargo build` runs and when the binary runs.
//!
//! Why a BUILD knob and not a decorator field: the image deps are a property of HOW the
//! crate is built (one image per app), not of WHAT one entrypoint computes — so they
//! ride on the build config, not on `#[function(...)]`. The decorator stays config for
//! the function (gpu/timeout/cache/secrets/volumes); the image is config for the build.
//!
//! The function below is a plain Rust fn that never names Modal, an image, or a
//! dependency. It exists only to give the image something to build. The lesson is the
//! IMAGE the facade renders, proven OFFLINE by `tests/manifest.rs` (and printed by the
//! `pip_apt_image` driver): the dry-run RUN manifest's image layer carries the
//! `apt-get install`, `pip install`, and `RUN` lines, in the order you chained them.

use modal_rust::function;
use serde::{Deserialize, Serialize};

/// Input for [`render`] — a value to echo back. The workload is irrelevant; the image
/// dependencies (chosen via the build config, not this signature) are the lesson.
#[derive(Debug, Serialize, Deserialize)]
pub struct Job {
    /// An arbitrary value echoed back so the envelope carries proof the body ran.
    pub value: u64,
}

/// Output for [`render`].
#[derive(Debug, Serialize, Deserialize)]
pub struct Output {
    /// Echoes the input value.
    pub value: u64,
}

/// A plain Rust fn — no system lib, no Modal, no image. It compiles on any base; the
/// point is that the IMAGE it builds in can carry whatever system/Python deps a real
/// workload needs, requested through the build config (`RemoteConfig::image_steps`),
/// not by editing this body.
///
/// Run the `pip_apt_image` driver to see the rendered image dockerfile (the
/// `apt-get install` / `pip install` / `RUN` lines) the image-builder steps produce.
#[function]
pub fn render(input: Job) -> anyhow::Result<Output> {
    Ok(Output { value: input.value })
}
