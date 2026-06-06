//! `examples/deploy-and-call` — the production model: deploy ONCE, call many times.
//!
//! Teaching ONE concept: the **run-vs-deploy build boundary**, the heart of the
//! production deployment model.
//!
//! - `.remote()` (the RUN path) uploads your source and runs `cargo build` IN the
//!   function body, on every cold container — great for iterating, but the build is
//!   on the invoke's critical path.
//! - `deploy` + `call` (the DEPLOY path) is the production model: `cargo build
//!   --release` runs ONCE at IMAGE-BUILD time and bakes the binary into the image;
//!   each subsequent `call` resolves the already-published function by name and
//!   invokes the prebuilt `/app/modal_runner` with NO upload, NO image, NO rebuild.
//!   The deployed runtime never runs `cargo`.
//!
//! The function below is an ordinary Rust fn behind `#[modal_rust::function]`; it is
//! the SAME function whether you `.remote()` it or `deploy`+`call` it. What changes
//! is WHEN the binary is built — and that is the whole lesson.
//!
//! `src/bin/modal_runner.rs` is the one-line runner. `src/bin/deploy_and_call.rs` is
//! the OFFLINE driver: it projects BOTH manifests (no Modal, no network) and prints
//! the contrast. `tests/manifest.rs` drives a REAL `deploy` + `call` against the
//! in-process mock and asserts the deploy manifest (image cargo-build layer,
//! client-only mount, deployed publish) and that `call` resolves the function with no
//! rebuild.

use modal_rust::function;

/// A small, deterministic workload that stands in for "real work the deployed
/// binary does". The point is not the arithmetic — it is that this exact compiled
/// function is what a `call` invokes, with the build already done at deploy time.
///
/// `#[function]` keeps the body a plain Rust fn (callable in-process and in tests),
/// generates the JSON I/O plumbing, registers the entrypoint via `inventory`, and
/// adds a typed `app.fib(n)` method to `App`.
#[function]
pub fn fib(n: u32) -> anyhow::Result<u64> {
    let (mut a, mut b) = (0u64, 1u64);
    for _ in 0..n {
        (a, b) = (
            b,
            a.checked_add(b)
                .ok_or_else(|| anyhow::anyhow!("overflow"))?,
        );
    }
    Ok(a)
}
