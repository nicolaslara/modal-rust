# web-endpoint

Expose a plain function over HTTP with **one attribute**:
`#[modal_rust::endpoint(method = "POST")]`.

An endpoint is a normal `#[function]`-shaped handler — same auto-IO, same decorator
vocabulary (`gpu`/`timeout`/`secrets`/…), same typed `app.summarize(..)` call surface —
that **also** gets an HTTP URL when the app is **deployed**. Modal wraps the
in-container callable in a FastAPI app (`WEBHOOK_TYPE_FUNCTION`, the
`@modal.fastapi_endpoint` analogue), so a `curl -X POST` with the function's input JSON
as the body returns the function's output JSON. No web-framework dependency in this
crate — that is the whole point of `#[endpoint]` v0.

Here the work is a real frequency-based extractive summarizer (`src/extractive.rs`):
split the text into sentences, count content words, score each sentence by the mean
whole-text frequency of its words, keep the top scorers in original order. The model
lives in its own module so `src/lib.rs` stays the clean modal-rust surface:

```rust endpoint
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
```

`method` is **required** — one of `"GET" | "POST" | "PUT" | "DELETE" | "PATCH"` (a
missing or invalid method is a compile error, never a live-deploy surprise).

## The HTTP contract

The request body **is** the function's input JSON — the same shape `--input` takes
(here the auto-generated Mode-B input object `{"text":"..","max_sentences":2}`). The
response body is the function's output JSON (the `Summary`). Errors come back as
`{"kind","message"}` JSON: a body that fails to decode is **422**, a handler error
(like `max_sentences = 0` above) is **500**.

## Deploy and curl it

The URL is **deploy-only** (v0): Modal assigns it when the function is created on a
*persistent* app. Deploying auto-installs `fastapi[standard]` into the deploy image
(Modal requires FastAPI in the image for FUNCTION webhooks) — nothing to declare.

```bash
cd examples/web-endpoint
modal-rust deploy summarize --app modal-rust-web-endpoint
```

The assigned URL has Modal's stable shape
`https://<workspace>--<app-name>-<entrypoint>.modal.run` (it is also shown on the
app's Modal dashboard page):

```bash
curl -X POST "https://<workspace>--modal-rust-web-endpoint-summarize.modal.run" \
  -H 'content-type: application/json' \
  -d '{"text":"Rust guarantees memory safety without a garbage collector. The borrow checker proves memory safety at compile time. My cat enjoys long naps. Most memory bugs in large systems are safety violations a compiler can catch.","max_sentences":2}'
```

```json
{"summary":"Rust guarantees memory safety without a garbage collector. The borrow checker proves memory safety at compile time.","sentences_kept":2,"sentences_total":4,"words_total":35}
```

The two kept sentences are the frequency model's top scorers ("memory"/"safety" recur
across the text); the off-topic aside is dropped. Wrong method? The single-route
FastAPI registration enforces `method = "POST"`, so a GET returns *405 Method Not
Allowed*.

## Public by default — opt into proxy-auth

A deployed endpoint is **public** by default (matching Modal): anyone with the URL can
call it. To require auth the way Modal does, opt into proxy-auth on the decorator:

```rust
#[endpoint(method = "POST", requires_proxy_auth = true)]
```

Modal then rejects any request lacking a valid proxy-auth token pair **before** it
reaches the container; callers send the `Modal-Key` / `Modal-Secret` headers:

```bash
curl -X POST "$URL" \
  -H "Modal-Key: $MODAL_PROXY_TOKEN_ID" \
  -H "Modal-Secret: $MODAL_PROXY_TOKEN_SECRET" \
  -H 'content-type: application/json' -d '{"text":"..","max_sentences":1}'
```

## The dual surface (and what `run` does)

An `#[endpoint]` fn **remains a normal function** everywhere except on the deployed
webhook itself:

- `app.summarize(text, 2).local()` works offline (proven by `tests/local.rs`).
- `modal-rust run summarize --input '{..}'` still works as a one-shot typed call: the
  webhook is **suppressed** on the RUN boundary, so the wire stays byte-identical to
  a plain `#[function]` and **no URL exists** — deploy to get the URL (D5).
- The **deployed** endpoint is **HTTP-only (v0)**: Modal's worker wraps a webhook
  function's in-container callable in an ASGI app, so the typed envelope path
  (`.remote()` / `modal-rust call`) against the *deployed* app is not available
  (live-verified: the call reaches `asgi_app_wrapper` and is rejected). Need both
  surfaces on one deploy? Do what Modal's own idiom does: keep the compute fn a plain
  `#[function]` and add a thin `#[endpoint]` fn that calls it.

```bash
modal-rust run summarize --input '{"text":"Rust guarantees memory safety without a garbage collector. The borrow checker proves memory safety at compile time. My cat enjoys long naps. Most memory bugs in large systems are safety violations a compiler can catch.","max_sentences":1}'
```

```json
{"ok":true,"value":{"summary":"Rust guarantees memory safety without a garbage collector.","sentences_kept":1,"sentences_total":4,"words_total":35}}
```

In-container the serve loop already frames one request → one response for both the
typed envelope and an HTTP request, so `#[cls]` load-once and memory snapshots compose
with endpoints for free; the HTTP-only restriction on the deployed function comes from
Modal's worker-side ASGI wrapping, not from this runtime.

`tests/manifest.rs` proves offline that the webhook rides the DEPLOY `FunctionCreate`
plan and not the RUN one. Routing, multiple methods, streaming, and websockets are the
`#[web_server]` follow-up (see `docs/ROADMAP.md`); v0 is one method, one
request/response per fn.

## Prereqs

Modal credentials configured (`modal token new`). Run `modal-rust doctor --rust`
to check your environment first.
