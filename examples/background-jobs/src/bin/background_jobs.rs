//! Fire-and-forget a longer job, then poll the handle for its result.
//!
//! The single `#[modal_rust::function] fn run_job(job)` from this crate's `lib.rs` is
//! a job you do NOT want to block on. Two shapes, same handler:
//!
//! - OFFLINE (default): `app.function("run_job").local(job)?` runs the real handler
//!   in-process and returns the `JobResult` you would later `.get()` — zero Modal, zero
//!   network. This is the result the background job converges to.
//! - LIVE (`RUN_REMOTE=1` + Modal credentials): `app.function("run_job").spawn(job)`
//!   enqueues the work and returns a HANDLE immediately (fire-and-forget); you do other
//!   work, then `handle.get(Some(timeout))` polls that handle for the result later.
//!
//! `.spawn()` is the contrast with `.remote()`: `.remote()` blocks until the result is
//! ready, `.spawn()` returns at once and you collect the result on your own schedule.
//! Both run the SAME handler and converge to the SAME deterministic `JobResult`, so the
//! offline run shows exactly the value the live spawned job will produce.
//!
//! Because the job input is one of your own structs (`Job`), the call site names the
//! entrypoint and hands it that struct directly — the same string-keyed
//! `app.function("run_job")` handle drives both shapes.

use std::time::Duration;

use example_background_jobs::{Job, JobResult};
use modal_rust::App;

/// The job to fire off — a longer unit of work, sized by `rounds`.
fn job() -> Job {
    Job {
        label: "nightly-report".to_string(),
        rounds: 250_000,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let job = job();

    // ----- OFFLINE: run the job in-process — the result you would later .get() --------
    //
    // `App::local()` builds an in-process app from the `#[modal_rust::function]`
    // inventory. `app.function("run_job").local(job)?` runs the real handler in-process
    // and returns the `JobResult` — the value a spawned background run converges to,
    // computed with zero Modal, zero network, nothing to install.
    let app = App::local();
    let result: JobResult = app.function("run_job").local(job.clone())?;
    println!(
        "local: job '{}' done -> {} rounds, digest {}",
        result.label, result.rounds, result.digest
    );
    assert_eq!(result.label, "nightly-report");
    assert_eq!(result.rounds, 250_000);

    // ----- LIVE: .spawn() the job, then .get(timeout) the handle (credential-gated) ---
    //
    // This hits real Modal, so it only runs when explicitly enabled. The code is always
    // compiled (it is the genuine API), it is just not executed by default.
    if std::env::var("RUN_REMOTE").as_deref() == Ok("1") {
        run_live_spawn(job, &result).await?;
    } else {
        println!(
            "(skipping live .spawn() + .get(timeout) — set RUN_REMOTE=1 with Modal \
             credentials to fire the background job)"
        );
    }

    Ok(())
}

/// The live fire-and-forget against a connected App. `App::connect("name").await`
/// builds a live control-plane client (reading `~/.modal.toml` / `MODAL_TOKEN_*`) and
/// uses the inventory registry, so the SAME `app.function("run_job")` handle drives the
/// `.spawn()` shape. We assert the spawned result equals the offline one — the same
/// deterministic `JobResult`.
async fn run_live_spawn(job: Job, expected: &JobResult) -> Result<(), Box<dyn std::error::Error>> {
    let app = App::connect("modal-rust-background-jobs").await?;

    // `.spawn(job)` enqueues the work and returns a handle IMMEDIATELY — no waiting for
    // the result. This is the fire-and-forget step.
    let handle = app.function("run_job").spawn(job).await?;
    println!("spawn: job fired -> handle {}", handle.function_call_id());

    // … in a real app you would go do other work here while the job runs …

    // `.get(Some(timeout))` polls the handle for the background result, waiting at most
    // `timeout` (covering the cold in-body `cargo build` the spawned input pays on first
    // run). The result decodes to your own `JobResult` struct.
    let result: JobResult = handle.get(Some(Duration::from_secs(900))).await?;
    println!(
        "get:   job '{}' done -> {} rounds, digest {}",
        result.label, result.rounds, result.digest
    );
    assert_eq!(
        &result, expected,
        "the spawned job's result must match the offline run"
    );

    Ok(())
}
