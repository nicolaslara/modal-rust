//! `examples/autoscaling` — control warm capacity and scale-to-zero from the decorator.
//!
//! Teaching ONE concept: the autoscaler knobs the decorator sets directly,
//! `#[modal_rust::function(min_containers = .., max_containers = .., buffer_containers
//! = .., scaledown_window = ..)]`.
//!
//! A serverless function scales with demand: Modal spins containers up under load and
//! down when idle. The autoscaler knobs shape that curve:
//!
//! - `min_containers` — the warm FLOOR. Keep this many containers always running so the
//!   first request never pays a cold start. `min_containers = 0` (the default) means
//!   scale all the way to zero when idle.
//! - `max_containers` — the CEILING. Cap concurrent containers to bound cost / protect a
//!   rate-limited downstream.
//! - `buffer_containers` — a warm BUFFER kept beyond current demand, so a burst of new
//!   requests has spare capacity ready instead of waiting for a scale-up.
//! - `scaledown_window` — how long (seconds) an idle container waits before Modal scales
//!   it down. A longer window trades a little idle cost for fewer cold starts.
//!
//! The decorator IS the config. The body is a plain Rust fn that just does its work —
//! it contains NO scaling logic and NO Modal. Autoscaling is operational metadata the
//! facade reads when CREATING the Modal function: it rides into the `FunctionCreate`
//! manifest's `autoscaler_settings` (mirroring Modal's `min_containers` /
//! `max_containers` / `buffer_containers` / `scaledown_window` kwargs). It changes how
//! many containers Modal keeps warm, never what the function COMPUTES. Every knob
//! defaults to unset, so a bare `#[function]` is wire-identical to before.
//!
//! `src/bin/modal_runner.rs` is the one-line runner; `tests/manifest.rs` proves
//! OFFLINE (no live Modal) that the knobs ride into the planned `FunctionCreate`
//! manifest — `min_containers == 1`, `max_containers == 10`, `buffer_containers == 2`,
//! `scaledown_window == 120`.

use modal_rust::function;
use serde::{Deserialize, Serialize};

/// Input for [`embed`] — the text to turn into an embedding. The embedding model is
/// expensive to spin up cold, which is exactly why this function keeps a warm floor.
#[derive(Debug, Serialize, Deserialize)]
pub struct Document {
    /// The text to embed.
    pub text: String,
}

/// The embedding a successful call returns.
#[derive(Debug, Serialize, Deserialize)]
pub struct Embedding {
    /// The text that was embedded (echoed back).
    pub text: String,
    /// The number of dimensions in the produced vector.
    pub dimensions: usize,
}

/// The fixed embedding dimensionality this stand-in model produces.
pub const EMBED_DIMENSIONS: usize = 8;

/// Embed `text` into a vector. On Modal this would load a model and run inference — an
/// expensive-to-cold-start workload, so we keep a warm floor and a small buffer.
///
/// The body is plain Rust: it just maps the input to an [`Embedding`]. The
/// `#[function(min_containers = 1, max_containers = 10, buffer_containers = 2,
/// scaledown_window = 120)]` decorator tells Modal to keep ONE container always warm
/// (no cold start for the first request), allow up to TEN under load, keep TWO extra
/// warm to absorb bursts, and wait 120s of idle before scaling a container down.
///
/// Run `modal_runner --describe` to see the autoscaler knobs ride on this entrypoint's
/// config (`"min_containers":1,"max_containers":10,"buffer_containers":2,
/// "scaledown_window":120`).
#[function(
    min_containers = 1,
    max_containers = 10,
    buffer_containers = 2,
    scaledown_window = 120
)]
pub fn embed(doc: Document) -> anyhow::Result<Embedding> {
    Ok(Embedding {
        text: doc.text,
        dimensions: EMBED_DIMENSIONS,
    })
}
