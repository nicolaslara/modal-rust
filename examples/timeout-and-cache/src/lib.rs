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
//! cache — it is a plain Rust fn. The knobs are operational metadata the facade reads
//! when CREATING the Modal function; they do not change what the function COMPUTES.
//!
//! `src/bin/modal_runner.rs` is the one-line runner; `tests/manifest.rs` proves
//! OFFLINE (no live Modal) that BOTH knobs ride into the planned `FunctionCreate`
//! manifest — `timeout_secs == 1800` and the `/cache` cargo-cache volume mount.

use modal_rust::function;
use serde::{Deserialize, Serialize};

/// Input for [`spin`] — how many iterations of busy work to do. This is just a
/// stand-in for a long-running job; the knobs (timeout, cache) are the lesson, not
/// the workload.
#[derive(Debug, Serialize, Deserialize)]
pub struct Job {
    /// Number of iterations of the busy loop to run.
    pub iterations: u64,
}

/// Output for [`spin`].
#[derive(Debug, Serialize, Deserialize)]
pub struct Done {
    /// The number of iterations actually run (echoes the input).
    pub iterations: u64,
    /// An accumulated checksum, so the work cannot be optimized away and the
    /// envelope carries proof the loop ran.
    pub checksum: u64,
}

/// A deliberately long-ish, CPU-only job — the kind you give a generous `timeout` so
/// Modal does not cut it off, and a `cache` so its image's deps are not rebuilt every
/// run. The body is plain Rust: it just spins an accumulating loop and returns the
/// total. The `#[function(timeout = 1800, cache = true)]` decorator sets the
/// operational knobs without touching this computation.
///
/// Run `modal_runner --describe` to see `"timeout_secs":1800` and `"cache":true` ride
/// on this entrypoint's config.
#[function(timeout = 1800, cache = true)]
pub fn spin(job: Job) -> anyhow::Result<Done> {
    // A trivial accumulator so the work is observable and not elided. `wrapping_*`
    // keeps it total over u64 regardless of `iterations`.
    let mut checksum: u64 = 0;
    for i in 0..job.iterations {
        checksum = checksum.wrapping_add(i).wrapping_mul(2_654_435_761);
    }
    Ok(Done {
        iterations: job.iterations,
        checksum,
    })
}
