//! OFFLINE proof of the pipeline concept (zero Modal, zero network): the SAME
//! [`example_queue_pipeline::produce`] / [`drain`] cores the binary and the
//! `#[function]` body run live do a real produce→drain round-trip against the
//! in-process mock's stateful Queue store (`modal-rust-testkit`), end-to-end
//! through the real gRPC transport on loopback — FIFO order, blocking-get idle
//! timeout, typed summary. The live path is the credential-gated tour
//! (`RUN_REMOTE=1 cargo run -p example-queue-pipeline --bin queue_pipeline`).

use std::time::Duration;

use example_queue_pipeline::{collatz::collatz_steps, drain, produce, JOBS_QUEUE};
use modal_rust::Queue;
use modal_rust_testkit::prelude::*;

/// A short idle timeout keeps the drain's final empty poll fast in tests.
const IDLE: Duration = Duration::from_millis(200);

#[tokio::test]
async fn produce_then_drain_consumes_the_whole_batch() {
    let mock = MockModal::start().await.expect("mock up");
    let jobs: [u64; 4] = [27, 6, 97, 9];

    // PRODUCER side — a handle resolved from the shared name.
    let producer = Queue::from_name_at(JOBS_QUEUE, mock.url())
        .await
        .expect("resolve producer");
    produce(&producer, &jobs).await.expect("produce");
    assert_eq!(producer.len().await.expect("len"), 4);

    // CONSUMER side — what the #[function] body does (minus credentials): an
    // INDEPENDENT handle on the same name, drained with blocking get(idle).
    let consumer = Queue::from_name_at(JOBS_QUEUE, mock.url())
        .await
        .expect("resolve consumer");
    assert_eq!(consumer.queue_id(), producer.queue_id());
    let summary = drain(&consumer, IDLE).await.expect("drain");

    assert_eq!(summary.jobs, 4, "the whole batch is consumed");
    assert_eq!(
        summary.total_steps,
        jobs.iter().map(|&j| collatz_steps(j)).sum::<u64>()
    );
    assert_eq!(summary.max_steps, 118, "97 has the longest stopping time");
    assert_eq!(
        producer.len().await.expect("len after"),
        0,
        "queue is drained"
    );
}

#[tokio::test]
async fn queue_is_fifo_and_drain_of_empty_is_zero() {
    let mock = MockModal::start().await.expect("mock up");
    let q = Queue::from_name_at("fifo-check", mock.url())
        .await
        .expect("resolve");

    // FIFO: items come back in put order (get(Some(ZERO)) = non-blocking poll).
    produce(&q, &[1, 2, 3]).await.expect("produce");
    for expect in 1..=3u64 {
        let got: Option<u64> = q.get(Some(Duration::ZERO)).await.expect("get");
        assert_eq!(got, Some(expect), "queue must be FIFO");
    }

    // Draining an EMPTY queue blocks only for the idle timeout, then summarizes
    // zero work — the consumer's clean-exit contract.
    let summary = drain(&q, IDLE).await.expect("drain empty");
    assert_eq!(
        (summary.jobs, summary.total_steps, summary.max_steps),
        (0, 0, 0)
    );
}

#[test]
fn the_function_is_registered_in_the_inventory() {
    // The #[modal_rust::function] registration the live `.remote()` path rides.
    let registry = modal_rust::registry_from_inventory();
    assert!(
        registry.get("drain_jobs").is_some(),
        "drain_jobs must be a registered entrypoint"
    );
}
