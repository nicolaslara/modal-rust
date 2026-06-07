//! `examples/cli-workflow` — drive your crate from the generic `modal-rust` CLI.
//!
//! Teaching ONE concept: you write a plain `#[function]` (and the one-line
//! `modal_runner`), and the **`modal-rust` CLI is your whole operations surface** —
//! no driver binary to write. The four verbs:
//!
//! - `modal-rust doctor --project examples/cli-workflow` — OFFLINE preflight: is
//!   this environment ready (credentials; with `--rust`, cargo/rustc and a sane
//!   release panic profile)? This is the only one that runs with no Modal.
//! - `modal-rust run summarize  --project examples/cli-workflow --input '{"text":"..."}'`
//!   — ephemeral run: builds your crate IN the function body and invokes once.
//! - `modal-rust deploy summarize --project examples/cli-workflow --app <name>` —
//!   persistent deploy: builds the binary ONCE at image-build time and publishes it.
//! - `modal-rust call summarize --app <name> --input '{"text":"..."}'` — invoke the
//!   deployed function by name with NO rebuild.
//!
//! Every command resolves THIS crate by `--project`: the CLI builds `--bin
//! modal_runner`, reads its `--describe` manifest, and drives the SAME `App`
//! orchestration the facade uses. The decorator IS the config — `name =
//! "summarize"` is the entrypoint the CLI verbs address. The crate carries no Modal
//! glue; the CLI does.

use modal_rust::function;
use serde::{Deserialize, Serialize};

/// Input for `summarize`: the runner argument is always a single named JSON object
/// (the `--input '{"text":"..."}'` you pass on the CLI decodes into this).
#[derive(Debug, Deserialize)]
pub struct Doc {
    /// The text to summarize.
    pub text: String,
}

/// Output for `summarize`: a tiny, deterministic digest of the input text.
#[derive(Debug, Serialize)]
pub struct Summary {
    /// Number of whitespace-separated words.
    pub words: usize,
    /// Number of characters (including whitespace).
    pub chars: usize,
    /// An estimated read time in minutes (rounded up; 200 wpm), so the envelope
    /// itself carries a believable result you can eyeball from `run`/`call` output.
    pub read_minutes: usize,
}

/// Summarize a document — a small, deterministic stand-in for "real work the
/// deployed binary does". The point is not the arithmetic; it is that this exact
/// function is what `modal-rust run` / `deploy` + `call` drive, with NO driver
/// binary in this crate.
///
/// `#[function(name = "summarize")]` keeps the body a plain Rust fn (callable
/// in-process and in tests), generates the JSON I/O plumbing, and registers the
/// `summarize` entrypoint via `inventory` — which is the name the CLI's `run` /
/// `deploy` / `call` verbs address.
#[function(name = "summarize")]
pub fn summarize(doc: Doc) -> anyhow::Result<Summary> {
    let words = doc.text.split_whitespace().count();
    let chars = doc.text.chars().count();
    let read_minutes = words.div_ceil(200).max(1);
    Ok(Summary {
        words,
        chars,
        read_minutes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_counts_words_chars_and_read_time() {
        let s = summarize(Doc {
            text: "the quick brown fox".to_string(),
        })
        .unwrap();
        assert_eq!(s.words, 4);
        assert_eq!(s.chars, 19);
        assert_eq!(s.read_minutes, 1);
    }

    #[test]
    fn input_decodes_from_named_object() {
        let d: Doc = serde_json::from_str(r#"{"text":"hello world"}"#).unwrap();
        assert_eq!(d.text, "hello world");
    }

    #[test]
    fn registry_has_summarize() {
        // The `#[function(name = "summarize")]` decorator submits the entrypoint to
        // inventory under that name — the SAME name the CLI's run/deploy/call verbs
        // address. `registry_from_inventory()` collects it into the runner's lookup.
        let reg = modal_rust::registry_from_inventory();
        assert!(reg.get("summarize").is_some());
        assert!(reg.get("nope").is_none());
    }
}
