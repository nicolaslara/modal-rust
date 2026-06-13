//! Manual producer for the queue-pipeline demo.
//!
//! `put_many`s a fixed batch of jobs into the named Queue and then EXITS,
//! leaving the queue populated — so you can drain it from a SEPARATE process
//! with the consumer function:
//!
//! ```bash
//! cargo run -p example-queue-pipeline --bin produce       # enqueue the batch
//! modal-rust run drain_jobs --input '{"idle_ms":2000}'    # then drain it
//! ```
//!
//! This is the standalone producer half of the pipeline. The `queue_pipeline`
//! driver instead produces AND drains in one process (and cleans up); use this
//! one when you want to populate the Queue and run `modal-rust run drain_jobs`
//! by hand. It hits real Modal (Queue handles open a gRPC client), so it needs
//! Modal credentials configured.

use example_queue_pipeline::{produce, JOBS_QUEUE};
use modal_rust::Queue;

/// The job batch to enqueue — the same numbers the driver uses (27 is the
/// classic Collatz case, 111 steps).
const JOBS: [u64; 4] = [27, 6, 97, 9];

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let q = Queue::from_name(JOBS_QUEUE).await?;
    produce(&q, &JOBS).await?;
    println!(
        "produced {} jobs into {JOBS_QUEUE:?} — now drain them with:\n  \
         modal-rust run drain_jobs --input '{{\"idle_ms\":2000}}'",
        JOBS.len()
    );
    Ok(())
}
