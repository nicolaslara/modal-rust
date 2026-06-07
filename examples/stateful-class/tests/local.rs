//! Offline proof (zero Modal, zero network) of the load-once stateful class:
//!
//!   1. `#[enter]` (the model load) runs EXACTLY ONCE across many
//!      `app.embedder().embed(..).local()?` / `.dim().local()?` calls — the load-once
//!      win, the whole point of `#[cls]`.
//!   2. the embedding the class serves is REAL: fixed width (== `dim()`), deterministic
//!      across calls, and L2-normalized to unit length for non-empty text.
//!   3. `.remote()` on an offline (unconnected) `App` errors WITHOUT any network call —
//!      the client-gated surface, identical to a free `#[function]`.
//!
//! CPU-only, in-process, runs in the normal `cargo test` and in
//! `scripts/check-examples.sh`.

use modal_rust::App;
// The `#[cls]` macro emits the `EmbedderCls` extension trait (which carries
// `app.embedder()`) at the lib's scope, so a glob import brings it in — exactly the one
// glob an external user writes (`use stateful_class::*;`).
use stateful_class::*;

#[test]
fn enter_runs_once_across_many_local_calls() {
    let app = App::local();

    // Several method calls — same and different methods — through the generated handle.
    let d: usize = app.embedder().dim().local().expect("dim().local()");
    assert_eq!(d, stateful_class::EMBED_DIMENSIONS);

    let v1: Vec<f32> = app
        .embedder()
        .embed("the quick brown fox".into())
        .local()
        .expect("embed().local()");
    let _v2: Vec<f32> = app
        .embedder()
        .embed("jumps over the lazy dog".into())
        .local()
        .expect("embed().local()");
    let _d2: usize = app.embedder().dim().local().expect("dim().local() again");

    // The real embedding properties (proven on a live `.local()` result).
    assert_eq!(
        v1.len(),
        d,
        "the vector width equals the model's reported dim"
    );
    assert!(
        v1.iter().any(|&x| x != 0.0),
        "a multi-word input embeds to a non-zero vector (real compute, not an echo)"
    );
    let norm_sq: f32 = v1.iter().map(|x| x * x).sum();
    assert!(
        (norm_sq - 1.0).abs() < 1e-5,
        "embedding is L2-normalized to unit length (sum of squares = {norm_sq})"
    );

    // Deterministic: embedding the same text again yields the identical vector.
    let v1_again: Vec<f32> = app
        .embedder()
        .embed("the quick brown fox".into())
        .local()
        .expect("embed().local() again");
    assert_eq!(v1, v1_again, "embedding is deterministic across calls");

    // The load-once win: `#[enter]` (the OnceLock init) ran EXACTLY ONCE across every
    // `.local()` call above, regardless of which method or how many. The counter is
    // process-global (so is the singleton), so this is the global truth: the model is
    // loaded a single time no matter how many calls — or tests — drive it.
    assert_eq!(
        stateful_class::load_count(),
        1,
        "#[enter] (the model load) ran exactly once across all .local() calls"
    );
}

#[tokio::test]
async fn remote_on_offline_app_is_not_connected() {
    // `.remote()` on a method needs a connected App, exactly like a free fn: an offline
    // App errors WITHOUT any network call (the client-gated surface). This also keeps
    // the `.remote()` surface compile-tested in the dev-only `client`-feature build.
    let app = App::local();
    let err = app
        .embedder()
        .dim()
        .remote()
        .await
        .expect_err("remote on offline app must error");
    let msg = err.to_string();
    assert!(
        msg.contains("client") || msg.contains("connect"),
        "unexpected error: {msg}"
    );
}
