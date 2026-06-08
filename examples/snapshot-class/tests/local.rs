//! Offline proof (zero Modal, zero network) of the load-once snapshot class:
//!
//!   1. `#[enter]` (the index build) runs EXACTLY ONCE across many
//!      `app.concordance().search(..).local()?` / `.vocabulary().local()?` calls — the
//!      load-once win every `#[cls]` has, snapshot or not. (The memory-snapshot extension
//!      of load-once across COLD starts is a DEPLOY-time Modal behavior, not something a
//!      `.local()` test can exercise; it is proven to ride onto the wire by
//!      `tests/manifest.rs`.)
//!   2. the concordance the class serves is REAL: a prefix search returns the matching
//!      words with their corpus counts, sorted, and is deterministic across calls.
//!
//! CPU-only, in-process, runs in the normal `cargo test` and in
//! `scripts/check-examples.sh`.

use modal_rust::App;
// The `#[cls]` macro emits the `ConcordanceCls` extension trait (which carries
// `app.concordance()`) at the lib's scope, so a glob import brings it in — exactly the one
// glob an external user writes (`use snapshot_class::*;`).
use snapshot_class::*;

#[test]
fn enter_runs_once_across_many_local_calls() {
    let app = App::local();

    // Several method calls — same and different methods — through the generated handle.
    let vocab: usize = app
        .concordance()
        .vocabulary()
        .local()
        .expect("vocabulary().local()");
    // EXACT — this is the number the README documents as the `vocabulary` output; pinning
    // it keeps the README honest (a corpus edit that drifts it is a test failure).
    assert_eq!(
        vocab, 99,
        "the embedded corpus has exactly 99 distinct words (the README's documented value)"
    );

    // "the" is the most common word in the corpus; a prefix search for it returns at
    // least that entry, with a count > 1 (real occurrence counting, not an echo).
    let the_hits: Vec<Entry> = app
        .concordance()
        .search("the".into())
        .local()
        .expect("search().local()");
    let the = the_hits
        .iter()
        .find(|e| e.word == "the")
        .expect("the word \"the\" is in the corpus");
    assert!(
        the.count > 1,
        "\"the\" occurs more than once in the corpus, got {}",
        the.count
    );

    // The search is a real prefix match over the sorted index: every hit starts with the
    // prefix and the run is sorted ascending by word.
    let wa_hits: Vec<Entry> = app
        .concordance()
        .search("wa".into())
        .local()
        .expect("search().local() prefix");
    assert!(
        wa_hits.iter().all(|e| e.word.starts_with("wa")),
        "every prefix hit starts with the prefix: {wa_hits:?}"
    );
    assert!(
        wa_hits.windows(2).all(|w| w[0].word <= w[1].word),
        "prefix hits come back sorted: {wa_hits:?}"
    );
    // EXACT — this is the `Concordance.search` output the README documents for prefix
    // "wa"; pinning it keeps the README JSON honest against a corpus edit.
    assert_eq!(
        wa_hits,
        vec![
            Entry {
                word: "want".to_string(),
                count: 3,
            },
            Entry {
                word: "was".to_string(),
                count: 1,
            },
        ],
        "the \"wa\" prefix search returns exactly want(3), was(1) — the README's value"
    );

    // Deterministic: the same query again yields the identical result.
    let the_again: Vec<Entry> = app
        .concordance()
        .search("the".into())
        .local()
        .expect("search().local() again");
    assert_eq!(
        the_hits, the_again,
        "the concordance search is deterministic across calls"
    );

    // The load-once win: `#[enter]` (the OnceLock init) ran EXACTLY ONCE across every
    // `.local()` call above, regardless of which method or how many. The counter is
    // process-global (so is the singleton), so this is the global truth: the index is
    // built a single time no matter how many calls — or tests — drive it.
    assert_eq!(
        snapshot_class::build_count(),
        1,
        "#[enter] (the index build) ran exactly once across all .local() calls"
    );
}
