//! `examples/own-runner-bin` — bring your OWN `modal_runner` bin.
//!
//! By default a modal-rust crate is a PURE LIBRARY: you write
//! `#[modal_rust::function]` functions and run them with `modal-rust run/deploy/call`,
//! and the tooling GENERATES the runner binary for you (you never see it). This
//! example shows the escape hatch: shipping your own `modal_runner` bin so you can wrap
//! the runner with extra startup logic — initialize a tracing subscriber, read a config
//! file, warm a cache, or set process-wide env before any function runs.
//!
//! The CLI AUTO-DETECTS this: when a crate already exposes a `modal_runner` bin target
//! (seen via `cargo metadata`), the run/deploy/`--describe` paths build and use IT
//! as-is and never materialize a generated shadow runner. So keeping your own bin is a
//! drop-in: the same `modal-rust run own-runner-bin --entrypoint extract_metrics …`
//! command just runs through your binary instead of a generated one. This is the SINGLE
//! crate in the workspace that keeps a bin named `modal_runner` — it exists to exercise
//! that auto-detect path; every other crate relies on the generated runner.
//!
//! The headline function below is a small, realistic log-line aggregator: hand it a
//! batch of raw service log lines and it returns a tally (total lines, error count, and
//! the busiest source). The companion `src/bin/modal_runner.rs` is the one-line runner;
//! `tests/manifest.rs` proves the function is registered (the `--describe`/registry
//! view) and dispatches through the frozen runner CLI — all offline.

use modal_rust::function;
use serde::{Deserialize, Serialize};

/// A batch of raw log lines to crunch — a plain user struct you own; the macro uses
/// `LogBatch` AS the wire input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogBatch {
    /// The raw service log lines, one entry per line. Each is expected to start with a
    /// level token (`INFO`, `WARN`, `ERROR`, …) followed by a `source=<name>` field.
    pub lines: Vec<String>,
}

/// The aggregated tally — what the call site gets back. Another plain user struct you
/// own; the macro uses `Metrics` AS the wire output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metrics {
    /// Total non-empty lines seen.
    pub total: u64,
    /// How many lines were at `ERROR` level.
    pub errors: u64,
    /// The `source=` value that appeared most often (ties broken by first-seen), or
    /// `None` when no line carried a `source=` field.
    pub busiest_source: Option<String>,
}

/// Crunch one batch of log lines into a [`Metrics`] tally — the whole function.
///
/// Because the single parameter is one of your own structs, the macro uses `LogBatch`
/// AS the wire input and `Metrics` AS the wire output; the call site names the
/// entrypoint and hands it your struct directly:
/// `app.function("extract_metrics").remote(batch).await?`.
#[function]
pub fn extract_metrics(batch: LogBatch) -> anyhow::Result<Metrics> {
    let mut total = 0u64;
    let mut errors = 0u64;
    // Ordered tally: first-seen wins ties, so the result is deterministic.
    let mut sources: Vec<(String, u64)> = Vec::new();

    for line in &batch.lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        total += 1;

        if line.split_whitespace().next() == Some("ERROR") {
            errors += 1;
        }

        if let Some(source) = parse_source(line) {
            match sources.iter_mut().find(|(name, _)| name == &source) {
                Some((_, count)) => *count += 1,
                None => sources.push((source, 1)),
            }
        }
    }

    let busiest_source = sources
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(name, _)| name);

    Ok(Metrics {
        total,
        errors,
        busiest_source,
    })
}

/// Pull the value out of the first `source=<value>` token on a line.
fn parse_source(line: &str) -> Option<String> {
    line.split_whitespace()
        .find_map(|tok| tok.strip_prefix("source="))
        .map(|v| v.to_string())
}
