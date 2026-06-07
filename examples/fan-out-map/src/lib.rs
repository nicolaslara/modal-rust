//! `examples/fan-out-map` — embarrassingly-parallel scale-out with `.map()`.
//!
//! Teaching ONE concept: a SINGLE `#[modal_rust::function]` mapped over N
//! independent inputs. The per-record work here is reading-time analysis of a
//! document — each record is self-contained, so the inputs share nothing and can
//! run in any order across many containers. That is the textbook
//! "embarrassingly parallel" shape, and `.map([..])` is how you scale it out:
//!
//! ```text
//! app.analyze(..).map([doc0, doc1, doc2, ..]).await   // N inputs, N containers
//!     -> vec![out0, out1, out2, ..]                    // results in INPUT ORDER
//! ```
//!
//! `.map(..)` runs the supplied inputs and returns `Vec<Out>` in input order (Modal
//! tags each output with its input ordinal and the SDK reassembles by ordinal), so
//! item `k` of the result is always the analysis of input `k`.
//!
//! The companion `src/bin/fan_out_map.rs` is the runnable tour: the OFFLINE default
//! maps the real handler in-process over the inputs (the local fan-out, results in
//! input order); the live `.map([..])` shape compiles always and runs only with
//! Modal credentials. `src/bin/modal_runner.rs` is the one-line runner.

use modal_rust::function;
use serde::{Deserialize, Serialize};

/// The real reading-time analysis, kept off this modal surface (see the module docs).
pub mod reading;

/// One record to process — a document with its title and body text. Plain user
/// structs you own; the macro uses them AS the wire input/output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    /// Identifies the document; echoed back so a result is traceable to its input.
    pub title: String,
    /// The full body text to analyze.
    pub body: String,
}

/// The per-record result — the document's reading-time analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reading {
    /// The document this analysis belongs to (carried through from the input).
    pub title: String,
    /// Number of whitespace-separated words in the body.
    pub words: u32,
    /// Estimated reading time in whole minutes at 200 words/minute (min 1).
    pub minutes: u32,
}

/// Analyze ONE document — the whole per-record task. It depends on nothing but its
/// own input, which is exactly what makes a batch of these embarrassingly parallel:
/// `.map([..])` fans the SAME function out over N documents at once.
///
/// Because the single parameter is one of your own structs, the macro uses
/// `Document` AS the wire input and `Reading` AS the wire output; the call site names
/// the entrypoint and hands it your struct directly:
/// `app.function("analyze").local(doc)?` / `.map(docs).await?`.
///
/// The body is just glue: it forwards the document body to [`reading::analyze_body`]
/// and labels the result with the document's title.
#[function]
pub fn analyze(doc: Document) -> anyhow::Result<Reading> {
    let (words, minutes) = reading::analyze_body(&doc.body);
    Ok(Reading {
        title: doc.title,
        words,
        minutes,
    })
}
