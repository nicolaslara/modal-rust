//! `examples/dict-kv` — shared state through a named `modal.Dict`.
//!
//! Teaching ONE concept: two parties that never talk to each other share state
//! through a **named Dict** — a `#[modal_rust::function]` computes word scores
//! and writes them into `Dict::from_name("dict-kv-scores")`; the CALLER (a
//! different process, possibly a different machine) opens the SAME name and
//! reads the scores back typed:
//!
//! ```text
//! caller ──.remote()──▶ record_scores() ──put──▶ Dict "dict-kv-scores"
//! caller ◀────────────────────────────────get──┘   (read back typed)
//! ```
//!
//! The Python interop boundary (by design): Dict keys are `&str`, and values
//! round-trip with Python for PLAIN DATA (str/int/float/bool/bytes/lists/dicts/
//! structs-as-dicts) — a Python `modal.Dict` reader sees these entries as
//! ordinary `str -> int`. Pickled Python custom classes/functions do NOT
//! interop: reading one from Rust fails with a typed codec error, never a
//! panic or a silent `None`.
//!
//! The actual scoring math lives in [`scoring`] so this file stays the clean
//! modal-rust surface; [`write_scores`] is the Dict-writing core, shared by the
//! `#[function]` body (against real Modal) and the offline mock test
//! (`tests/mock_dict.rs`, against the in-process testkit backend).

use modal_rust::{function, Dict};

pub mod scoring;

/// The shared Dict's deployment name — the ONLY coupling between the function
/// (writer) and the caller (reader).
pub const SCORES_DICT: &str = "dict-kv-scores";

/// Score every word and `put` it into the Dict (`word -> score`, overwriting).
/// Returns how many entries were written. Takes the handle as a parameter so
/// the SAME code runs against real Modal (from [`record_scores`]) and against
/// the in-process mock (from the offline test).
pub async fn write_scores(d: &Dict, words: &[String]) -> anyhow::Result<u64> {
    for w in words {
        d.put(w.as_str(), &scoring::scrabble_score(w)).await?;
    }
    Ok(words.len() as u64)
}

/// The Modal function: scores `words` and writes `word -> score` entries into
/// the named Dict, returning the entry count. Handlers are sync by contract,
/// and Dict methods are async — so the body drives them with its own runtime
/// (fine in the container: the runner serve loop is sync, no ambient runtime).
#[function]
pub fn record_scores(words: Vec<String>) -> anyhow::Result<u64> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let d = Dict::from_name(SCORES_DICT).await?;
        write_scores(&d, &words).await
    })
}
