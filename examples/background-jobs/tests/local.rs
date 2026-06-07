//! Offline proof (zero Modal, zero network) of the job this example fires off with
//! `.spawn()`. Only the in-process `.local()` shape is exercised here — it runs the
//! real `run_job` handler and yields the deterministic `JobResult` that a live
//! `.spawn()` + `.get(timeout)` round-trip converges to. The live fire-and-forget
//! shape is compiled by the binary and proven against real Modal in the
//! credential-gated tour (`RUN_REMOTE=1 cargo run -p example-background-jobs --bin
//! background_jobs`).

use example_background_jobs::work::digest;
use example_background_jobs::{Job, JobResult};
use modal_rust::App;

/// The digest the deterministic fold produces for the example's job — the exact value a
/// spawned background run would later `.get()`. Pinned so a change to the handler is a
/// test failure, and so the offline run and the live spawn agree.
const EXPECTED_DIGEST: u64 = 17267777379177717202;

#[test]
fn run_job_local_yields_the_deterministic_result() {
    let app = App::local();
    // Your input struct in; your output struct back — the result a .spawn() handle's
    // .get() would later return, computed in-process with no Modal.
    let result: JobResult = app
        .function("run_job")
        .local(Job {
            label: "nightly-report".to_string(),
            rounds: 250_000,
        })
        .expect("the offline .local() path should run the job in-process");

    assert_eq!(result.label, "nightly-report");
    assert_eq!(result.rounds, 250_000);
    assert_eq!(result.digest, EXPECTED_DIGEST);
    // The handler is real glue over the extracted computation: its digest IS the fold
    // over the job's rounds, not a fixed constant baked into the function.
    assert_eq!(result.digest, digest(result.rounds));
}

#[test]
fn the_digest_is_real_work_over_rounds() {
    // The result is genuinely computed from `rounds`, not echoed or fixed: a job with
    // more rounds folds to a different digest, and zero rounds is the untouched seed.
    let app = App::local();
    let one_round: JobResult = app
        .function("run_job")
        .local(Job {
            label: "report".to_string(),
            rounds: 1,
        })
        .unwrap();
    let many_rounds: JobResult = app
        .function("run_job")
        .local(Job {
            label: "report".to_string(),
            rounds: 250_000,
        })
        .unwrap();
    assert_ne!(one_round.digest, many_rounds.digest);
    assert_eq!(one_round.digest, digest(1));
    assert_eq!(digest(0), 0x9e37_79b9_7f4a_7c15); // zero rounds -> the untouched seed
}

#[test]
fn the_job_is_deterministic() {
    // The same job always yields the same digest — that determinism is what lets the
    // offline run stand in for the result you would .get() from a live spawn.
    let app = App::local();
    let job = Job {
        label: "report".to_string(),
        rounds: 1_000,
    };
    let first: JobResult = app.function("run_job").local(job.clone()).unwrap();
    let second: JobResult = app.function("run_job").local(job).unwrap();
    assert_eq!(first, second);
}

#[test]
fn plain_fn_is_directly_callable() {
    // The macro emits the user fn verbatim, so it stays a plain Rust fn over your
    // structs.
    let result = example_background_jobs::run_job(Job {
        label: "report".to_string(),
        rounds: 250_000,
    })
    .unwrap();
    assert_eq!(result.digest, EXPECTED_DIGEST);
}
