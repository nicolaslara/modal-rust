//! Offline proof (zero Modal, zero network) of the endpoint's DUAL SURFACE: an
//! `#[endpoint]` fn REMAINS a normal `#[function]`, so the typed `app.summarize(..)`
//! call path works in-process via `.local()` — and the computation it runs is real
//! (a frequency-scored extractive summary), not a fixed string.
//!
//! The HTTP leg of the dual surface (the deploy-only URL) cannot be exercised offline;
//! `tests/manifest.rs` proves its wire config rides the DEPLOY plan, and the live
//! deploy + curl is the operator's proof.

use modal_rust::App;
use web_endpoint::*;

/// Four sentences where "memory"/"safety" dominate the content-word frequencies and
/// one sentence is an off-topic aside — so a 2-sentence summary provably keeps the
/// two top-scoring on-topic sentences and drops the aside.
const TEXT: &str = "Rust guarantees memory safety without a garbage collector. \
     The borrow checker proves memory safety at compile time. \
     My cat enjoys long naps. \
     Most memory bugs in large systems are safety violations a compiler can catch.";

#[test]
fn typed_local_call_summarizes_for_real() {
    // The SAME typed call surface a plain `#[function]` gets: the macro generated
    // `summarize::Input`/`Output` and the `SummarizeCall` extension trait (in scope
    // via the glob), and `.local()` dispatches in-process through the frozen registry.
    let app = App::local();
    let s: Summary = app.summarize(TEXT.to_string(), 2).local().unwrap();

    // The frequency model keeps the two sentences whose content words ("memory",
    // "safety") recur across the text — in their ORIGINAL order — and drops the
    // off-topic aside. This is the genuine score-and-rank computation, not an echo.
    assert_eq!(
        s.summary,
        "Rust guarantees memory safety without a garbage collector. \
         The borrow checker proves memory safety at compile time."
    );
    assert!(!s.summary.contains("cat"), "the off-topic aside is dropped");
    assert_eq!(s.sentences_kept, 2);
    assert_eq!(s.sentences_total, 4);
    // Real token count across all four sentences (8 + 9 + 5 + 13).
    assert_eq!(s.words_total, 35);
}

#[test]
fn asking_for_more_sentences_than_exist_keeps_everything() {
    let app = App::local();
    let s: Summary = app.summarize(TEXT.to_string(), 99).local().unwrap();
    assert_eq!(s.sentences_kept, 4, "capped at the available sentences");
    assert!(s.summary.contains("cat"), "nothing is dropped when all fit");
}

#[test]
fn handler_errors_stay_plain_rust_errors() {
    // The macro emits the user fn verbatim, so it is still directly callable — and
    // its validation errors are ordinary `anyhow` errors. Over HTTP the deployed
    // adapter maps this envelope error to a 500 `{"kind","message"}` JSON response
    // (422 for a body that fails to decode); locally it is just an `Err`.
    let err = summarize(TEXT.to_string(), 0).unwrap_err();
    assert!(err.to_string().contains("max_sentences"));

    let err = summarize("   ".to_string(), 3).unwrap_err();
    assert!(err.to_string().contains("no sentences"));
}
