//! `examples/background-jobs` — fire-and-forget a longer job with `.spawn()`.
//!
//! Teaching ONE concept: a job you do NOT want to block on. `.spawn(input)` enqueues
//! the work and hands back a handle IMMEDIATELY (no waiting for the result); you go do
//! other things, then call `handle.get(timeout)` to collect the result later:
//!
//! ```text
//! let handle = app.function("run_job").spawn(job).await?;  // returns at once — work runs in the background
//! //  … other work here …
//! let result: JobResult = handle.get(Some(timeout)).await?; // poll the handle for the result
//! ```
//!
//! `.get(timeout)` bounds how long to wait for the background result; `None` uses the
//! function's own deadline (with a buffer for the cold in-body `cargo build` the
//! spawned input may still be running). This is the difference from `.remote()`, which
//! blocks until the result is ready — `.spawn()` is for work you start and check on.
//!
//! The companion `src/bin/background_jobs.rs` is the runnable tour: the OFFLINE default
//! runs the same job in-process with `.local()` (the result you would later `.get()`,
//! computed with zero Modal, zero network); the live `.spawn()` + `.get(timeout)` shape
//! compiles always and runs only with Modal credentials. `src/bin/modal_runner.rs` is
//! the one-line runner.

use modal_rust::function;
use serde::{Deserialize, Serialize};

/// The job to run — a longer unit of work, sized by `rounds`. A plain user struct you
/// own; the macro uses `Job` AS the wire input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    /// A label echoed back on the result so a finished job is traceable to its input.
    pub label: String,
    /// How many rounds of work to grind through (the "longer" knob).
    pub rounds: u64,
}

/// The finished job's result — what you collect later with `.get(timeout)`. Another
/// plain user struct you own; the macro uses `JobResult` AS the wire output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobResult {
    /// The job this result belongs to (carried through from the input).
    pub label: String,
    /// How many rounds were completed.
    pub rounds: u64,
    /// A deterministic digest of the work, so the same job always yields the same
    /// result — the offline `.local()` run and the live `.spawn()` run agree.
    pub digest: u64,
}

/// Run ONE job — the whole longer-running task. It folds a deterministic digest over
/// `rounds` iterations; the point is only that it takes real work to finish, which is
/// exactly why you `.spawn()` it and poll later instead of blocking on `.remote()`.
///
/// Because the single parameter is one of your own structs, the macro uses `Job` AS
/// the wire input and `JobResult` AS the wire output; the call site names the
/// entrypoint and hands it your struct directly:
/// `app.function("run_job").spawn(job).await?` then `handle.get(timeout).await?`.
#[function]
pub fn run_job(job: Job) -> anyhow::Result<JobResult> {
    // A deterministic fold (a small xorshift mix per round) — same input, same digest.
    let mut digest: u64 = 0x9e37_79b9_7f4a_7c15;
    for round in 0..job.rounds {
        digest ^= round.wrapping_mul(0x2545_f491_4f6c_dd1d);
        digest = digest.rotate_left(13).wrapping_add(round);
    }
    Ok(JobResult {
        label: job.label,
        rounds: job.rounds,
        digest,
    })
}
