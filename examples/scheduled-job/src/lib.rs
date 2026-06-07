//! `examples/scheduled-job` — a DEPLOYED function that runs on a cron schedule.
//!
//! Teaching ONE concept: one operational knob the decorator sets directly,
//! `#[modal_rust::function(schedule = Cron("0 9 * * 1"))]` — a DEPLOYED function that
//! Modal invokes automatically on a cadence, with NO caller.
//!
//! Some work is not request/response: a nightly cleanup, a weekly report, an hourly
//! health probe. You do not want a client calling it — you want the platform to run it
//! on a timer. `schedule = Cron("0 9 * * 1")` does exactly that: the facade
//! canonicalizes the `Cron`/`Period` form to a spec, the SDK parses it into Modal's
//! `Schedule` (a `Cron`/`Period` oneof), and it rides onto the DEPLOY `FunctionCreate`
//! manifest's `schedule` (proto field 72). Once `deploy`ed, Modal triggers the function
//! on that cadence — here every Monday at 09:00 UTC.
//!
//! The decorator IS the config. The body is a plain Rust fn that does its work and
//! returns a summary; it contains NO scheduling logic, NO timer, NO Modal. The
//! cadence is operational metadata the facade reads when CREATING the Modal function;
//! it does not change what the function COMPUTES, only WHEN Modal runs it. `schedule`
//! defaults to unset (no schedule), so a bare `#[function]` is wire-identical to before
//! and is invoked only by callers.
//!
//! Unlike the other examples there is NO caller here (`.remote()`/`.call()`): a
//! scheduled job is invoked by Modal itself. The way you ship it is `modal-rust deploy`;
//! after that, nothing calls it — the schedule does. `src/bin/modal_runner.rs` is the
//! one-line runner; `tests/manifest.rs` proves OFFLINE (no live Modal) that the cron
//! schedule rides into the planned `FunctionCreate` manifest.

use modal_rust::function;
use serde::{Deserialize, Serialize};

/// Input for [`weekly_report`]. A scheduled function takes no caller arguments — Modal
/// triggers it on the cadence — so this is a tiny placeholder the runner decodes. The
/// `dataset` lets the offline demo call the body deterministically; in production the
/// schedule supplies the (empty) input automatically.
#[derive(Debug, Serialize, Deserialize)]
pub struct Tick {
    /// The dataset to summarise on this run.
    pub dataset: String,
}

/// The summary a single scheduled run produces.
#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    /// The dataset that was summarised.
    pub dataset: String,
    /// How many rows the run processed (toy fixed value for the demo).
    pub rows: u64,
    /// A human-readable note describing what ran.
    pub note: String,
}

/// The fixed row count the toy report "processes" each run — stands in for whatever a
/// real scheduled job would do (scan a table, roll up metrics, send a digest).
pub const ROWS_PER_RUN: u64 = 1000;

/// Compile a weekly report for `dataset`. Modal runs this automatically every Monday at
/// 09:00 UTC — `schedule = Cron("0 9 * * 1")` — with NO caller. The body is plain Rust:
/// it does its work and returns a [`Report`]. The cron cadence is metadata the
/// `#[function(schedule = ..)]` decorator sets; it rides onto the DEPLOY FunctionCreate
/// so the platform, not a client, triggers the function.
///
/// Run `modal_runner --describe` to see `"schedule":"cron:UTC:0 9 * * 1"` ride on this
/// entrypoint's config.
#[function(schedule = Cron("0 9 * * 1"))]
pub fn weekly_report(tick: Tick) -> anyhow::Result<Report> {
    Ok(Report {
        note: format!(
            "weekly report for {:?} ({} rows)",
            tick.dataset, ROWS_PER_RUN
        ),
        dataset: tick.dataset,
        rows: ROWS_PER_RUN,
    })
}
