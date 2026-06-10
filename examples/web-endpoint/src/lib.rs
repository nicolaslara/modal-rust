//! `examples/web-endpoint` — expose a plain function over HTTP with ONE attribute.
//!
//! Teaching ONE concept: `#[modal_rust::endpoint(method = "POST")]` — the web-endpoint
//! variant of `#[function]` (the `@modal.fastapi_endpoint` analogue). The handler stays
//! an ordinary Rust fn — same auto-IO, same decorator vocabulary, same typed
//! `app.summarize(..)` call surface — and on `modal-rust deploy` it ALSO gets an HTTP
//! URL: Modal wraps the in-container callable in a FastAPI app
//! (`WEBHOOK_TYPE_FUNCTION`), so a `curl -X POST` with the function's input JSON as the
//! body returns the function's output JSON. No web-framework dependency in this crate —
//! that is the whole point of `#[endpoint]` v0.
//!
//! The HTTP contract is the auto-IO contract: the request body IS the function's input
//! JSON (the same shape `--input` takes, here `{"text":"..","max_sentences":2}`), and
//! the response body is the output JSON (a [`Summary`]). A handler error returns
//! `{"kind","message"}` JSON with status 500 (input that fails to decode: 422).
//!
//! TWO things to know:
//!
//! - **The URL is DEPLOY-only (v0).** `modal-rust run summarize --input '{..}'` still
//!   works as a normal one-shot typed call, but the webhook config is SUPPRESSED on the
//!   RUN boundary — the wire stays byte-identical to a plain `#[function]`, and no URL
//!   exists. Deploy to get the URL. Proven offline by `tests/manifest.rs`: the webhook
//!   method rides the DEPLOY `FunctionCreate` and not the RUN one.
//! - **The deployed endpoint is HTTP-ONLY (v0).** The fn remains a normal function for
//!   `.local()` (proven by `tests/local.rs`) and for `modal-rust run` (webhook
//!   suppressed), but Modal's worker wraps a webhook function's in-container callable
//!   in an ASGI app, so the typed envelope path (`.remote()` / `modal-rust call`)
//!   against the DEPLOYED app is rejected (live-verified: an `asgi_app_wrapper`
//!   TypeError). Want both surfaces on one deploy? Use Modal's own idiom: a plain
//!   `#[function]` for compute plus a thin `#[endpoint]` fn that calls it.
//!
//! Exposure: a deployed endpoint is **public by default** (matching Modal). Opt into
//! Modal proxy-auth with `#[endpoint(method = "POST", requires_proxy_auth = true)]` —
//! Modal then rejects requests lacking the `Modal-Key`/`Modal-Secret` header pair
//! before they reach the container.
//!
//! The real computation — a frequency-based extractive summarizer (split sentences,
//! count content words, score each sentence by mean word frequency, keep the top
//! scorers in original order) — lives in [`extractive`], so this file stays the clean
//! modal-rust surface: the output type and the `#[endpoint]` fn that calls the module.

mod extractive;

use modal_rust::endpoint;
use serde::{Deserialize, Serialize};

// README drift-guard: the region between the begin/end markers below is kept
// byte-identical to the ```rust endpoint blocks in this crate's README.md AND the root
// README's web-endpoints section (see tests/readme_drift.rs).
// endpoint:begin
/// The summary a POST returns — the response body, as JSON. Every field is computed
/// by the frequency model in `extractive.rs`, not fixed.
#[derive(Debug, Serialize, Deserialize)]
pub struct Summary {
    /// The selected sentences, joined in their original order.
    pub summary: String,
    /// How many sentences the summary kept.
    pub sentences_kept: usize,
    /// How many sentences the input text held.
    pub sentences_total: usize,
    /// How many words the input text held.
    pub words_total: usize,
}

/// Boil `text` down to its `max_sentences` most representative sentences. A normal
/// handler — IDENTICAL to a `#[function]` — but `#[endpoint]` ALSO exposes it over
/// HTTP on deploy: POST `{"text":"..","max_sentences":2}` (the auto-IO input JSON)
/// to the deployed URL and the response body is the [`Summary`] JSON. The typed call
/// surface keeps working alongside the URL: `app.summarize(text, 2).local()`.
#[endpoint(method = "POST")]
pub fn summarize(text: String, max_sentences: usize) -> anyhow::Result<Summary> {
    anyhow::ensure!(max_sentences > 0, "max_sentences must be at least 1");
    let sentences = extractive::split_sentences(&text);
    anyhow::ensure!(
        !sentences.is_empty(),
        "text holds no sentences to summarize"
    );
    let picked = extractive::pick_top(&sentences, max_sentences);
    Ok(Summary {
        sentences_kept: picked.len(),
        sentences_total: sentences.len(),
        words_total: extractive::word_count(&text),
        summary: picked.join(" "),
    })
}
// endpoint:end
