//! Fan-out over N inputs with `.map()`.
//!
//! The single `#[modal_rust::function] fn analyze(doc)` from this crate's `lib.rs`
//! is run over a BATCH of documents. Because each call depends only on its own
//! input, the batch is embarrassingly parallel — the canonical `.map()` workload:
//!
//! - OFFLINE (default): the local fan-out. `app.function("analyze").local(doc)?` runs
//!   the real handler in-process for each input, IN INPUT ORDER — zero Modal, zero
//!   network.
//! - LIVE (`RUN_REMOTE=1` + Modal credentials):
//!   `app.function("analyze").map(docs).await?` enqueues all N inputs under one map
//!   call and runs them across containers in parallel, returning `Vec<Reading>` in
//!   the SAME input order.
//!
//! Both shapes return results in input order, so result item `k` is always the
//! analysis of input `k` — the offline run proves that ordering locally and the live
//! run preserves it across the fan-out.
//!
//! Because the per-record input is one of your own structs (`Document`), the call
//! site names the entrypoint and hands it that struct directly — the same string-keyed
//! `app.function("analyze")` handle drives every shape.

use example_fan_out_map::{Document, Reading};
use modal_rust::App;

/// The batch to fan out over — three independent documents. Order matters only in
/// that the result comes back aligned to it.
fn corpus() -> Vec<Document> {
    [
        ("intro", "modal rust maps one function over many inputs"),
        (
            "scale",
            "each input runs in its own container so the batch is parallel",
        ),
        ("recap", "results come back in input order"),
    ]
    .into_iter()
    .map(|(title, body)| Document {
        title: title.to_string(),
        body: body.to_string(),
    })
    .collect()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let docs = corpus();

    // ----- OFFLINE: the local fan-out — map the real handler in-process ------------
    //
    // `App::local()` builds an in-process app from the `#[modal_rust::function]`
    // inventory. Running `app.function("analyze").local(doc)?` for each input, in
    // order, is the local mirror of `.map(..)`: same handler, same per-record result,
    // results in input order — but zero Modal, zero network, nothing to install.
    let app = App::local();
    let local: Vec<Reading> = docs
        .iter()
        .map(|doc| app.function("analyze").local(doc.clone()))
        .collect::<Result<_, _>>()?;

    println!(
        "local fan-out over {} docs (results in input order):",
        local.len()
    );
    for r in &local {
        println!("  {} -> {} words, {} min", r.title, r.words, r.minutes);
    }
    assert_eq!(
        local.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
        ["intro", "scale", "recap"],
        "the local fan-out must return results in input order",
    );

    // ----- LIVE: `.map([..])` — fan out across containers (credential-gated) -------
    //
    // This hits real Modal, so it only runs when explicitly enabled. The code is
    // always compiled (it is the genuine API), it is just not executed by default.
    if std::env::var("RUN_REMOTE").as_deref() == Ok("1") {
        run_live_map(docs, &local).await?;
    } else {
        println!(
            "(skipping live .map([..]) — set RUN_REMOTE=1 with Modal credentials to \
             fan out across containers)"
        );
    }

    Ok(())
}

/// The live fan-out against a connected App. `App::connect("name").await` builds a
/// live control-plane client (reading `~/.modal.toml` / `MODAL_TOKEN_*`) and uses
/// the inventory registry, so the SAME `app.function("analyze")` handle drives the
/// `.map(..)` shape. We assert the remote results equal the local ones — identical
/// values, identical input order.
async fn run_live_map(
    docs: Vec<Document>,
    expected: &[Reading],
) -> Result<(), Box<dyn std::error::Error>> {
    let app = App::connect("modal-rust-fan-out-map").await?;

    // `.map(docs)` enqueues all N inputs under one map call and returns `Vec<Reading>`
    // in input order. Each item is the function's wire input — here, your own
    // `Document` struct.
    let remote: Vec<Reading> = app.function("analyze").map(docs).await?;

    println!(
        "remote .map([..]) over {} docs (results in input order):",
        remote.len()
    );
    for r in &remote {
        println!("  {} -> {} words, {} min", r.title, r.words, r.minutes);
    }
    assert_eq!(
        remote, expected,
        "remote .map results must match the local fan-out"
    );

    Ok(())
}
