//! `examples/custom-types` — a Modal function over YOUR OWN structs.
//!
//! Teaching ONE concept: a `#[modal_rust::function]` takes and returns your own
//! `#[derive(Serialize, Deserialize)]` types. You name the input/output structs in
//! the signature; the macro infers the wire I/O from it — the JSON object on the
//! wire IS your input struct, the success value IS your output struct. No I/O type
//! is named at the call site beyond your own structs.
//!
//! `tests/local.rs` proves the offline `.local()` round-trip through your structs.
//! The runner is generated automatically by the modal-rust tooling — no bin needed.
//!
//! The actual scoring math lives in [`scoring`] so this file stays a clean surface:
//! your input/output types plus the `#[function]` that hands its fields to the module.

use modal_rust::function;
use serde::{Deserialize, Serialize};

pub mod scoring;

/// The function INPUT — a plain user struct you own.
#[derive(Debug, Serialize, Deserialize)]
pub struct Player {
    /// Display name, echoed back on the result.
    pub name: String,
    /// Hits landed this match.
    pub hits: u32,
    /// Shots taken this match (the denominator for accuracy).
    pub shots: u32,
}

/// The function OUTPUT — another plain user struct you own.
#[derive(Debug, Serialize, Deserialize)]
pub struct Scored {
    /// The player this score belongs to.
    pub name: String,
    /// Hits × 100, the headline score.
    pub points: u32,
    /// `hits / shots` as a percentage, rounded to a whole number.
    pub accuracy_pct: u32,
}

/// Score a player — the whole function. Because the single parameter is one of your
/// own structs, the macro uses `Player` AS the wire input and `Scored` AS the wire
/// output; the call site only ever names your structs:
/// `app.function("score").local(player)?`.
///
/// The body is just glue: it forwards the input fields to [`scoring::score_player`]
/// and packs the `(points, accuracy_pct)` it returns back into your output struct.
#[function]
pub fn score(p: Player) -> anyhow::Result<Scored> {
    let (points, accuracy_pct) = scoring::score_player(p.hits, p.shots);
    Ok(Scored {
        name: p.name,
        points,
        accuracy_pct,
    })
}
