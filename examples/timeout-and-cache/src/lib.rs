//! `examples/timeout-and-cache` — the operational knobs: function timeout + build cache.
//!
//! Teaching ONE concept: two operational knobs the decorator sets directly,
//! `#[modal_rust::function(timeout = 1800, cache = true)]`.
//!
//! - `timeout = 1800` — the per-function timeout in SECONDS. The facade rides it into
//!   the `FunctionCreate` manifest as `timeout_secs`, so Modal kills a call that runs
//!   past 1800s (30 min) instead of letting it hang forever.
//! - `cache = true` — turn ON the cargo BUILD cache (it is also on by default on the
//!   RUN path). With it on, the facade resolves the shared `modal-rust-cargo-cache`
//!   Volume and mounts it at `/cache`, so a repeat RUN reuses the cargo registry +
//!   `target/` instead of recompiling every dependency from scratch.
//!
//! The decorator IS the config. The function body never names Modal, timeouts, or a
//! cache — it is a plain Rust fn that runs a real deterministic fold (the workload
//! lives in `src/compute.rs`, so this file stays the clean modal surface: the
//! input/output types + the `#[function]`). The knobs are operational metadata the
//! facade reads when CREATING the Modal function; they do not change what the function
//! COMPUTES.
//!
//! The runner is generated automatically by the modal-rust tooling — no bin needed.
//! `tests/manifest.rs` proves OFFLINE (no live Modal) that BOTH knobs ride into the
//! planned `FunctionCreate` manifest — `timeout_secs == 1800` and the `/cache`
//! cargo-cache volume mount; `tests/local.rs` proves the real fold OFFLINE via
//! `.local()`.

use modal_rust::function;
use serde::{Deserialize, Serialize};

mod compute;

/// Input for [`spin`] — how many iterations of the checksum fold to run. A bigger
/// count is a longer CPU-only job; the knobs (timeout, cache) are the lesson, the
/// workload is the real fold in [`compute::checksum`].
#[derive(Debug, Serialize, Deserialize)]
pub struct Job {
    /// Number of iterations of the checksum fold to run.
    pub iterations: u64,
}

/// Output for [`spin`].
#[derive(Debug, Serialize, Deserialize)]
pub struct Done {
    /// The number of iterations actually run (echoes the input).
    pub iterations: u64,
    /// The deterministic checksum the fold accumulated — proof the loop ran, and a
    /// pure function of `iterations`.
    pub checksum: u64,
}

/// A CPU-only job — the kind you give a generous `timeout` so Modal does not cut it
/// off, and a `cache` so its image's deps are not rebuilt every run. The body is plain
/// Rust: it runs the real deterministic fold in [`compute::checksum`] and returns the
/// total. The `#[function(timeout = 1800, cache = true)]` decorator sets the
/// operational knobs without touching what the function COMPUTES.
///
/// Run `modal_runner --describe` to see `"timeout_secs":1800` and `"cache":true` ride
/// on this entrypoint's config.
#[function(timeout = 1800, cache = true)]
pub fn spin(job: Job) -> anyhow::Result<Done> {
    Ok(Done {
        iterations: job.iterations,
        checksum: compute::checksum(job.iterations),
    })
}
