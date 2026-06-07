//! `examples/cpu-memory` — right-size compute: request CPU cores + RAM on the decorator.
//!
//! Teaching ONE concept: two resource knobs the decorator sets directly,
//! `#[modal_rust::function(cpu = 2.0, memory = 4096)]`.
//!
//! - `cpu = 2.0` — request 2 CPU CORES. `cpu` is a float number of cores; the facade
//!   resolves it to milli-cores the way Modal does (`milli_cpu = int(1000 * cpu)`, so
//!   `2.0` cores -> `2000` milli-cores) and rides it into the `FunctionCreate`
//!   resources. A fractional request (`cpu = 0.5`) is fine too.
//! - `memory = 4096` — request 4096 MEBIBYTES (4 GiB) of RAM. The facade rides it into
//!   the `FunctionCreate` resources as `memory_mb`.
//!
//! The decorator IS the config. The function body never names Modal, CPUs, or memory —
//! it is a plain Rust fn. The knobs are resource metadata the facade reads when
//! CREATING the Modal function; they size the container, they do not change what the
//! function COMPUTES. Both default to the server's default when unset, so a bare
//! `#[function]` is wire-identical to before.
//!
//! `src/bin/modal_runner.rs` is the one-line runner; `tests/manifest.rs` proves
//! OFFLINE (no live Modal) that BOTH knobs ride into the planned `FunctionCreate`
//! manifest — `milli_cpu == 2000` and `memory_mb == 4096`.

use modal_rust::function;
use serde::{Deserialize, Serialize};

/// Input for [`crunch`] — the size of the in-memory batch to process. This is just a
/// stand-in for a memory- and CPU-hungry job; the resource knobs (cpu, memory) are the
/// lesson, not the workload.
#[derive(Debug, Serialize, Deserialize)]
pub struct Batch {
    /// How many records to fold over.
    pub records: u64,
}

/// Output for [`crunch`].
#[derive(Debug, Serialize, Deserialize)]
pub struct Summary {
    /// The number of records actually processed (echoes the input).
    pub records: u64,
    /// An accumulated checksum, so the work cannot be optimized away and the envelope
    /// carries proof the fold ran.
    pub checksum: u64,
}

/// A CPU- and memory-bound batch job — the kind you RIGHT-SIZE with `cpu` cores and a
/// `memory` ceiling so it gets enough compute without over-provisioning. The body is
/// plain Rust: it just folds an accumulating checksum over the batch. The
/// `#[function(cpu = 2.0, memory = 4096)]` decorator requests 2 cores and 4 GiB
/// without touching this computation.
///
/// Run `modal_runner --describe` to see `"milli_cpu":2000` and `"memory_mb":4096` ride
/// on this entrypoint's config.
#[function(cpu = 2.0, memory = 4096)]
pub fn crunch(batch: Batch) -> anyhow::Result<Summary> {
    // A trivial accumulator so the work is observable and not elided. `wrapping_*`
    // keeps it total over u64 regardless of `records`.
    let mut checksum: u64 = 0;
    for i in 0..batch.records {
        checksum = checksum.wrapping_add(i).wrapping_mul(2_654_435_761);
    }
    Ok(Summary {
        records: batch.records,
        checksum,
    })
}
