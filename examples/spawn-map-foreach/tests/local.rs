//! Offline proof (zero Modal, zero network) of the side-effect fan-out: the SAME
//! `app.function("notify")` handle that drives the live `.for_each([..])` /
//! `.spawn_map([..])` shapes also runs the real handler in-process per input. Running
//! `.local()` over a batch is the local mirror of `.for_each(..)` — every side effect
//! happens, the receipts are normally discarded. Here we DO inspect the receipts to
//! prove the per-send work is real: each `receipt_id` is computed from `(name,
//! channel)`, so it is deterministic (same input -> same id) and distinct across
//! recipients. The live fan-out across containers is proven against real Modal in the
//! credential-gated tour (`RUN_REMOTE=1 cargo run -p example-spawn-map-foreach --bin
//! spawn_map_foreach`).

use example_spawn_map_foreach::{Receipt, Recipient};
use modal_rust::App;

fn recipients() -> Vec<Recipient> {
    [("ada", "email"), ("babbage", "sms"), ("turing", "push")]
        .into_iter()
        .map(|(name, channel)| Recipient {
            name: name.to_string(),
            channel: channel.to_string(),
        })
        .collect()
}

#[test]
fn local_for_each_runs_the_side_effect_for_every_recipient() {
    // The local mirror of `.for_each(..)`: run `.local()` per input, in order. No
    // Modal, no network, no credentials.
    let app = App::local();
    let receipts: Vec<Receipt> = recipients()
        .into_iter()
        .map(|r| app.function("notify").local(r))
        .collect::<Result<_, _>>()
        .expect("the .local() path should run in-process for every recipient");

    // The side effect ran for ALL inputs (this is what `.for_each` guarantees).
    assert_eq!(receipts.len(), 3, "every recipient must be notified");

    // Each receipt carries its recipient through (input order preserved).
    assert_eq!(
        receipts.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        ["ada", "babbage", "turing"],
    );
    assert_eq!(
        receipts
            .iter()
            .map(|r| r.channel.as_str())
            .collect::<Vec<_>>(),
        ["email", "sms", "push"],
    );
}

#[test]
fn receipt_id_is_deterministic_for_the_same_input() {
    // Real computation, not an echo: the same recipient on the same channel always
    // yields the SAME receipt id across independent calls.
    let app = App::local();
    let r = Recipient {
        name: "ada".to_string(),
        channel: "email".to_string(),
    };
    let first: Receipt = app.function("notify").local(r.clone()).unwrap();
    let second: Receipt = app.function("notify").local(r).unwrap();
    assert_eq!(first.receipt_id, second.receipt_id);
    // And it is a fixed-width hex id, not a fixed constant.
    assert_eq!(first.receipt_id.len(), 16);
    assert!(first.receipt_id.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn receipt_ids_are_distinct_across_recipients() {
    // Different recipients (and different channels) produce different ids — proof the
    // id is derived from the input, not a shared constant.
    let app = App::local();
    let ids: Vec<String> = recipients()
        .into_iter()
        .map(|r| {
            app.function("notify")
                .local::<_, Receipt>(r)
                .unwrap()
                .receipt_id
        })
        .collect();
    let unique: std::collections::HashSet<&String> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        ids.len(),
        "every recipient gets a distinct id"
    );
}

#[test]
fn plain_fn_is_directly_callable() {
    // The macro emits the user fn verbatim, so it stays a plain Rust fn over your
    // structs — and it genuinely computes a receipt.
    let receipt = example_spawn_map_foreach::notify(Recipient {
        name: "lovelace".to_string(),
        channel: "email".to_string(),
    })
    .unwrap();
    assert_eq!(receipt.name, "lovelace");
    assert_eq!(receipt.channel, "email");
    assert_eq!(receipt.receipt_id.len(), 16);
}
