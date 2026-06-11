//! OFFLINE proof of the shared-state concept (zero Modal, zero network): the
//! SAME [`example_dict_kv::write_scores`] core the `#[function]` body runs in a
//! container writes through one `Dict` handle, and a SECOND handle — resolved
//! independently from the SAME name, like the caller — reads every score back
//! typed. Backed by the in-process mock's stateful Dict store
//! (`modal-rust-testkit`), end-to-end through the real gRPC transport on
//! loopback. The live path is the credential-gated tour
//! (`RUN_REMOTE=1 cargo run -p example-dict-kv --bin dict_kv`).

use example_dict_kv::{scoring::scrabble_score, write_scores, SCORES_DICT};
use modal_rust::Dict;
use modal_rust_testkit::prelude::*;

#[tokio::test]
async fn function_writes_then_caller_reads_through_the_shared_name() {
    let mock = MockModal::start().await.expect("mock up");
    let words: Vec<String> = ["jazz", "quartz", "modal", "rust"]
        .into_iter()
        .map(String::from)
        .collect();

    // WRITER side — what the #[function] body does (minus credentials): resolve
    // the named Dict and run the shared write core.
    let writer = Dict::from_name_at(SCORES_DICT, mock.url())
        .await
        .expect("resolve writer");
    let written = write_scores(&writer, &words).await.expect("write scores");
    assert_eq!(written, 4);

    // READER side — the caller: a SEPARATE handle resolved from the same name
    // (CREATE_IF_MISSING is idempotent, so it lands on the same object) …
    let reader = Dict::from_name_at(SCORES_DICT, mock.url())
        .await
        .expect("resolve reader");
    assert_eq!(reader.dict_id(), writer.dict_id());

    // … reads every score back typed, plus the absent-key contract.
    for w in &words {
        assert_eq!(
            reader.get::<i64>(w).await.expect("get"),
            Some(scrabble_score(w)),
            "the caller must read exactly what the function wrote for {w:?}"
        );
    }
    assert_eq!(reader.get::<i64>("absent").await.expect("miss"), None);
    assert_eq!(reader.len().await.expect("len"), 4);
}

#[test]
fn the_function_is_registered_in_the_inventory() {
    // The #[modal_rust::function] registration the live `.remote()` path rides.
    let registry = modal_rust::registry_from_inventory();
    assert!(
        registry.get("record_scores").is_some(),
        "record_scores must be a registered entrypoint"
    );
}
