//! A producer/consumer pipeline through a named Queue — the driver tour.
//!
//! - OFFLINE (default): the honest computation runs locally — each job's
//!   Collatz stopping time via [`example_queue_pipeline::collatz`], printed in
//!   the order the producer would enqueue them. Zero Modal, zero credentials.
//! - LIVE (`RUN_REMOTE=1` + Modal credentials): the real pipeline — THIS
//!   process `put_many`s the jobs into the named Queue, then
//!   `app.drain_jobs(2000).remote()` runs the consumer IN a Modal container,
//!   which blocking-`get`s jobs until the queue stays empty for 2 s and returns
//!   the typed `DrainSummary`. Cleans up with `Queue::delete`.
//!
//! The offline produce→drain round-trip itself is proven against the
//! in-process mock backend in `tests/mock_queue.rs`.

use example_queue_pipeline::{collatz::collatz_steps, DrainJobsCall, JOBS_QUEUE};
use modal_rust::{App, Queue};

/// The job batch both paths process: numbers whose Collatz stopping times we
/// compute. 27 is the classic (111 steps).
const JOBS: [u64; 4] = [27, 6, 97, 9];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ----- OFFLINE: the computation, locally (what the consumer computes) -------
    println!("local stopping times (what drain_jobs computes per job):");
    let mut total = 0u64;
    for job in JOBS {
        let steps = collatz_steps(job);
        total += steps;
        println!("  local: {job} -> {steps} steps");
    }
    println!("local: {} jobs, {total} total steps", JOBS.len());
    assert_eq!(collatz_steps(27), 111, "the classic Collatz stopping time");

    // ----- LIVE: produce here, drain in a container (credential-gated) ----------
    //
    // This hits real Modal, so it only runs when explicitly enabled. The code is
    // always compiled (it is the genuine API), it is just not executed by default.
    if std::env::var("RUN_REMOTE").as_deref() == Ok("1") {
        run_live(total).await?;
    } else {
        println!(
            "(skipping live produce + remote drain — set RUN_REMOTE=1 with Modal \
             credentials to run it)"
        );
    }

    Ok(())
}

/// The live pipeline: the PRODUCER (this process) enqueues; the CONSUMER (the
/// function, in a container) drains with blocking get and returns the summary.
async fn run_live(expected_total: u64) -> Result<(), Box<dyn std::error::Error>> {
    // PRODUCE side: this process shares NOTHING with the container but the name.
    let q = Queue::from_name(JOBS_QUEUE).await?;
    example_queue_pipeline::produce(&q, &JOBS).await?;
    println!("produced {} jobs into {JOBS_QUEUE:?}", JOBS.len());

    // CONSUME side: drain remotely with a 2 s idle timeout (stop once the queue
    // stays empty that long — the whole finite batch gets consumed).
    let app = App::connect("modal-rust-queue-pipeline-demo").await?;
    let summary = app.drain_jobs(2000).remote().await?;
    println!(
        "remote drain: {} jobs, {} total steps (max {})",
        summary.jobs, summary.total_steps, summary.max_steps
    );
    assert_eq!(summary.jobs, JOBS.len() as u64);
    assert_eq!(summary.total_steps, expected_total);

    // Clean up the demo object entirely (irreversible, like Python's delete).
    Queue::delete(JOBS_QUEUE).await?;
    println!("cleaned up: Queue::delete({JOBS_QUEUE:?})");
    Ok(())
}
