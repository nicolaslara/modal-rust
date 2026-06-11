//! Shared state through a named Dict — the driver tour.
//!
//! - OFFLINE (default): the honest computation runs locally — score each word
//!   with [`example_dict_kv::scoring::scrabble_score`] and print the entries
//!   the live path would write. Zero Modal, zero network, zero credentials.
//! - LIVE (`RUN_REMOTE=1` + Modal credentials): the real shared-state
//!   round-trip — `app.record_scores(words).remote()` runs IN a Modal container
//!   and writes the scores into the named Dict; then THIS process opens
//!   `Dict::from_name("dict-kv-scores")` and reads every score back typed,
//!   proving the two sides only share a NAME. Cleans up with `Dict::delete`.
//!
//! The offline write→read round-trip itself is proven against the in-process
//! mock backend in `tests/mock_dict.rs`.

use example_dict_kv::{scoring::scrabble_score, RecordScoresCall, SCORES_DICT};
use modal_rust::{App, Dict};

/// The words both paths score. Order is irrelevant — a Dict is keyed, not
/// ordered.
fn words() -> Vec<String> {
    ["jazz", "quartz", "modal", "rust"]
        .into_iter()
        .map(String::from)
        .collect()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let words = words();

    // ----- OFFLINE: the computation, locally (what the function would write) ----
    println!("local scores (what record_scores writes to the Dict):");
    for w in &words {
        println!("  local: {} -> {}", w, scrabble_score(w));
    }
    assert_eq!(scrabble_score("jazz"), 29, "j8 a1 z10 z10");

    // ----- LIVE: function writes, caller reads (credential-gated) ---------------
    //
    // This hits real Modal, so it only runs when explicitly enabled. The code is
    // always compiled (it is the genuine API), it is just not executed by default.
    if std::env::var("RUN_REMOTE").as_deref() == Ok("1") {
        run_live(&words).await?;
    } else {
        println!(
            "(skipping live function-writes/caller-reads — set RUN_REMOTE=1 with \
             Modal credentials to run it)"
        );
    }

    Ok(())
}

/// The live round-trip: the FUNCTION (in a container) writes the scores; the
/// CALLER (this process) reads them back through the same Dict name.
async fn run_live(words: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    // WRITE side: run record_scores remotely — it opens Dict::from_name and puts.
    let app = App::connect("modal-rust-dict-kv-demo").await?;
    let written: u64 = app.record_scores(words.to_vec()).remote().await?;
    println!("remote: record_scores wrote {written} entries");

    // READ side: this process shares NOTHING with the container except the name.
    let d = Dict::from_name(SCORES_DICT).await?;
    for w in words {
        let score: Option<i64> = d.get(w).await?;
        println!("dict: {} -> {:?}", w, score);
        assert_eq!(
            score,
            Some(scrabble_score(w)),
            "caller must read what the function wrote"
        );
    }

    // Clean up the demo object entirely (irreversible, like Python's delete).
    // DICT_KV_KEEP=1 skips it so you can inspect the entries from Modal's own
    // tooling — `modal dict items dict-kv-scores` shows them as plain str -> int,
    // the cross-language interop proof made visible.
    if std::env::var("DICT_KV_KEEP").ok().as_deref() == Some("1") {
        println!("kept: inspect with `modal dict items {SCORES_DICT}` (delete with `modal dict delete {SCORES_DICT}`)");
        return Ok(());
    }
    Dict::delete(SCORES_DICT).await?;
    println!("cleaned up: Dict::delete({SCORES_DICT:?})");
    Ok(())
}
