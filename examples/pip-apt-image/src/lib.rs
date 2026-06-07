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
//! The function below exists only to give the image something to build — it never names
//! Modal, an image, or a dependency. So that it is not a pure echo, its body does a
//! trivial-but-real deterministic transform of its input (a multiplicative-hash mix);
//! that keeps the envelope honest while staying out of the way of the actual lesson.
//! The lesson is the IMAGE the facade renders, proven OFFLINE by `tests/manifest.rs`
//! (and printed by the `pip_apt_image` driver): the dry-run RUN manifest's image layer
//! carries the `apt-get install`, `pip install`, and `RUN` lines, in the order you
//! chained them.

use modal_rust::function;
use serde::{Deserialize, Serialize};

/// Input for [`render`] — an arbitrary value. The workload is intentionally trivial; the
/// image dependencies (chosen via the build config, not this signature) are the lesson.
#[derive(Debug, Serialize, Deserialize)]
pub struct Job {
    /// An arbitrary value the body mixes into a deterministic digest so the envelope
    /// carries real proof the body ran (rather than echoing the input back unchanged).
    pub value: u64,
}

/// Output for [`render`].
#[derive(Debug, Serialize, Deserialize)]
pub struct Output {
    /// A deterministic digest of the input value — `mix(value)`, not a copy of it.
    pub digest: u64,
}

/// Mix `value` into a deterministic digest via a multiplicative-hash step (wrapping add
/// of a large odd constant, then wrapping multiply by Knuth's multiplicative-hash
/// constant `2_654_435_761`). It is small, CPU-only, and fully deterministic, and — being
/// a real arithmetic transform — it cannot be elided to a copy of the input.
///
/// # Examples
///
/// ```
/// use example_pip_apt_image::mix;
/// assert_eq!(mix(0), mix(0)); //                  deterministic
/// assert_ne!(mix(1), 1); //                        not a pure echo of the input
/// assert_ne!(mix(1), mix(2)); //                   distinct inputs -> distinct digests
/// ```
pub fn mix(value: u64) -> u64 {
    value
        .wrapping_add(0x9E37_79B9_7F4A_7C15)
        .wrapping_mul(2_654_435_761)
}

/// A plain Rust fn — no system lib, no Modal, no image. It compiles on any base; the
/// point is that the IMAGE it builds in can carry whatever system/Python deps a real
/// workload needs, requested through the build config (`RemoteConfig::image_steps`),
/// not by editing this body. The body itself just runs a trivial real transform of the
/// input ([`mix`]) so the result is genuinely computed, not echoed.
///
/// Run the `pip_apt_image` driver to see the rendered image dockerfile (the
/// `apt-get install` / `pip install` / `RUN` lines) the image-builder steps produce.
#[function]
pub fn render(input: Job) -> anyhow::Result<Output> {
    Ok(Output {
        digest: mix(input.value),
    })
}
