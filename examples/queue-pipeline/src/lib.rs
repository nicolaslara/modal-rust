//! `examples/queue-pipeline` — a producer/consumer pipeline through a named
//! `modal.Queue`.
//!
//! Teaching ONE concept: decouple work production from work consumption with a
//! **named Queue** — the producer (the caller) `put_many`s jobs; a
//! `#[modal_rust::function]` consumer drains them with **blocking
//! `get(timeout)`** and computes each job's Collatz stopping time:
//!
//! ```text
//! producer ──put_many──▶ Queue "queue-pipeline-jobs" ──get(timeout)──▶ drain_jobs()
//! producer ◀──────────────────── DrainSummary ─────────────.remote()──┘
//! ```
//!
//! The timeout convention (Python's `get(block=True, timeout=..)` without the
//! boolean): `None` blocks forever; `Some(d)` waits ~`d` then yields `None`;
//! `Some(Duration::ZERO)` is one non-blocking poll. The consumer uses `Some(d)`
//! as an IDLE timeout — "stop when the queue stays empty for `d`" — the
//! standard way to drain a finite batch.
//!
//! The Python interop boundary (by design): queue items round-trip with Python
//! for PLAIN DATA (here each job is a `u64`; a Python producer would just
//! `q.put(27)`). Pickled Python custom classes/functions do NOT interop:
//! reading one from Rust fails with a typed codec error, never a panic.
//!
//! The actual math lives in [`collatz`] so this file stays the clean modal-rust
//! surface; [`produce`] / [`drain`] are the Queue core, shared by the
//! `#[function]` body (against real Modal) and the offline mock test
//! (`tests/mock_queue.rs`, against the in-process testkit backend).

use std::time::Duration;

use modal_rust::{function, Queue};
use serde::{Deserialize, Serialize};

pub mod collatz;

/// The shared Queue's deployment name — the ONLY coupling between the producer
/// (caller) and the consumer (function).
pub const JOBS_QUEUE: &str = "queue-pipeline-jobs";

/// What one drain run accomplished — the consumer's typed result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrainSummary {
    /// Jobs consumed before the queue stayed empty past the idle timeout.
    pub jobs: u64,
    /// Sum of all computed Collatz stopping times.
    pub total_steps: u64,
    /// The largest single stopping time seen (0 when no jobs ran).
    pub max_steps: u64,
}

/// Producer side: enqueue the whole batch in order (ONE `QueuePut` — `values`
/// is repeated on the wire).
pub async fn produce(q: &Queue, jobs: &[u64]) -> anyhow::Result<()> {
    q.put_many(jobs).await?;
    Ok(())
}

/// Consumer side: blocking-`get` jobs until the queue stays empty for `idle`,
/// computing each job's Collatz stopping time. Takes the handle as a parameter
/// so the SAME code runs against real Modal (from [`drain_jobs`]) and against
/// the in-process mock (from the offline test).
pub async fn drain(q: &Queue, idle: Duration) -> anyhow::Result<DrainSummary> {
    let mut summary = DrainSummary {
        jobs: 0,
        total_steps: 0,
        max_steps: 0,
    };
    // `get(Some(idle))` BLOCKS up to `idle` waiting for an item, then yields
    // `Ok(None)` — which is exactly the "batch is done" signal here.
    while let Some(job) = q.get::<u64>(Some(idle)).await? {
        let steps = collatz::collatz_steps(job);
        summary.jobs += 1;
        summary.total_steps += steps;
        summary.max_steps = summary.max_steps.max(steps);
    }
    Ok(summary)
}

/// The Modal function: drain the named Queue with an `idle_ms` idle timeout and
/// return the summary. Handlers are sync by contract, and Queue methods are
/// async — so the body drives them with its own runtime (fine in the container:
/// the runner serve loop is sync, no ambient runtime).
#[function]
pub fn drain_jobs(idle_ms: u64) -> anyhow::Result<DrainSummary> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let q = Queue::from_name(JOBS_QUEUE).await?;
        drain(&q, Duration::from_millis(idle_ms)).await
    })
}
