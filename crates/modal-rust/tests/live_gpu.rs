//! Live, best-effort GPU proof — the payoff for P4 ("the decorator IS the config").
//!
//! Drives the facade end-to-end against REAL Modal so the decorator-sourced GPU
//! config rides into `FunctionCreate`:
//!   `#[modal_rust::function(gpu = "T4", name = "vector_add")]` (in THIS test binary)
//!   → `App::from_inventory()` captures `FunctionConfig { gpu: Some("T4"), .. }`
//!   → `app.function("vector_add").remote(GpuIn { n })`
//!   → the facade sets `FunctionSpec::with_gpu(Some("T4"))` → `Resources.gpu_config`
//!     (gpu_type "T4", count 1) on the outbound `FunctionCreate`
//!   → Modal schedules the wrapper on a **T4**, builds the `example-cuda-vector-add`
//!     crate IN THE FUNCTION BODY (RUN boundary), and execs `vector_add`, which runs
//!     a real cudarc kernel via the CUDA Driver API.
//!
//! The decoded `GpuOut.gpu_name` (e.g. "Tesla T4") + `valid == true` IS the
//! server-side proof that `gpu_config` was honored: a CPU container has no GPU, so a
//! T4 name can only appear if the decorator's gpu spec rode into the create request.
//!
//! The LOCAL `vector_add` stub below is never executed remotely — `.remote()`
//! dispatches by entrypoint NAME against the uploaded cuda crate's runner (which
//! registers the REAL `vector_add` via its own `#[modal_rust::function(gpu = "T4",
//! name = "vector_add")]` decorator → inventory). The stub exists only so the decorator
//! records the config and the name matches.
//!
//! Gated behind BOTH the `live` cargo feature AND `#[ignore]` so the no-CUDA CI box
//! never runs it. Run locally with:
//!
//! ```text
//! MODAL_RUST_PACKAGE=example-cuda-vector-add \
//!   cargo test -p modal-rust --features live --test live_gpu \
//!   -- --ignored --nocapture
//! ```
//!
//! Uses a CHEAP T4 and an EPHEMERAL app (the run path), so it leaves no persistent
//! deploy. Modal flakiness ("socket connection closed unexpectedly", build/GPU
//! capacity blips) is transient — retry, never block. The hard gates are the offline
//! compiles.

#![cfg(feature = "live")]

use std::time::Duration;

// The facade re-exports the proc-macro as `modal_rust::function`, so the attribute
// is spelled exactly as a user would. The macro's emitted code references
// `::modal_rust_runtime` / `::inventory` by their real crate names — both are in this
// test binary's extern prelude (a normal dep + a dev-dep of `modal-rust`).
use modal_rust::{App, Error};
use serde::{Deserialize, Serialize};

/// Ephemeral connect app name (the RUN path is GC'd on disconnect).
const APP_NAME: &str = "modal-rust-live-gpu";
/// The cuda crate to upload + build remotely (its runner registers `vector_add`).
const PACKAGE: &str = "example-cuda-vector-add";

/// The single named-JSON-object input for `vector_add` (mirrors
/// `example_cuda_vector_add::VectorAddInput`). Derives both serde traits so the
/// `typed!` wrapper the macro emits type-checks (`In: DeserializeOwned`) as well as
/// the outbound serialize path used by `.remote()`.
#[derive(Debug, Serialize, Deserialize)]
struct GpuIn {
    /// Number of elements in each vector.
    n: usize,
}

/// The decoded output of the REAL remote `vector_add` (mirrors
/// `example_cuda_vector_add::VectorAddOutput`). Only the fields the proof asserts on
/// are required. Derives both serde traits for the same reason as [`GpuIn`].
#[derive(Debug, Serialize, Deserialize)]
struct GpuOut {
    /// `true` iff the GPU result matched the CPU reference element-wise.
    valid: bool,
    /// Elements computed on the GPU.
    n: usize,
    /// GPU model reported by the driver (e.g. "Tesla T4") — the server-side proof.
    gpu_name: String,
    /// CUDA Driver API version (evidence; drifts, never asserted against a constant).
    driver_version: i32,
}

/// The DECORATED stub: its only job is to record `FunctionConfig { gpu: Some("T4") }`
/// under the entrypoint name `vector_add` into this test binary's inventory. It is
/// never executed remotely (the uploaded cuda crate runs the real kernel), so its
/// body just errors. The `String` error is `Display + Serialize`, satisfying the
/// `typed!` wrapper without pulling in `anyhow`.
///
/// The converted `example-cuda-vector-add` crate now carries its OWN
/// `#[modal_rust::function(gpu = "T4", name = "vector_add")]` decorator (consumed
/// REMOTELY by the uploaded runner). This test binary deliberately does NOT depend on
/// that crate, so its inventory is not linked here and there is no duplicate-name panic
/// (`from_inventory_with_configs` panics on duplicate names); this local stub remains
/// the test binary's create-time source of the same `gpu = "T4"` config.
#[modal_rust::function(gpu = "T4", name = "vector_add")]
fn vector_add(_input: GpuIn) -> Result<GpuOut, String> {
    Err("local stub: vector_add runs on Modal (T4), not in-process".to_string())
}

/// Treat transport blips and known transient gRPC messages as retryable. Delegates
/// to the SDK's own classifier so the test and the SDK agree on what is transient.
fn is_transient(err: &Error) -> bool {
    match err {
        Error::Sdk(sdk_err) => sdk_err.is_transient(),
        _ => false,
    }
}

#[tokio::test]
#[ignore = "live Modal GPU .remote() round-trip on a T4; run with --features live -- --ignored"]
async fn remote_gpu_runs_on_t4_via_decorator_config() {
    // The facade uploads/builds the package named by MODAL_RUST_PACKAGE; point it at
    // the cuda crate so the remote runner registers the real `vector_add`.
    std::env::set_var("MODAL_RUST_PACKAGE", PACKAGE);

    let attempts = 4u32;
    let mut last: Option<Error> = None;

    for attempt in 1..=attempts {
        match round_trip().await {
            Ok(out) => {
                println!(
                    "LIVE GPU OK: vector_add(n={}).remote() valid={} gpu_name={:?} driver={} \
                     (decorator gpu=\"T4\" -> Resources.gpu_config -> T4)",
                    out.n, out.valid, out.gpu_name, out.driver_version
                );
                assert!(out.valid, "GPU result must match the CPU reference");
                assert!(
                    out.gpu_name.to_uppercase().contains("T4"),
                    "must run on a T4 (decorator gpu=\"T4\" drove gpu_config); got gpu_name={:?}",
                    out.gpu_name
                );
                return;
            }
            Err(err) => {
                eprintln!("[gpu] attempt {attempt}/{attempts} failed: {err}");
                let transient = is_transient(&err);
                last = Some(err);
                if !transient || attempt == attempts {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(3 * attempt as u64)).await;
            }
        }
    }
    panic!(
        "live GPU .remote() failed after {attempts} attempts: {}",
        last.expect("an error was recorded")
    );
}

async fn round_trip() -> Result<GpuOut, Error> {
    // `App::connect` builds from THIS binary's inventory, capturing the decorator
    // config: the `vector_add` entrypoint carries `gpu = Some("T4")`, which the
    // facade threads into `FunctionSpec::with_gpu` -> `Resources.gpu_config` on the
    // outbound `FunctionCreate` (and into the EPHEMERAL run app).
    let app = App::connect(APP_NAME).await?;
    app.function("vector_add").remote(GpuIn { n: 1024 }).await
}
