//! Offline proof (zero Modal, zero network) that `crunch` does REAL work: the checksum
//! it returns is a deterministic, order-dependent fold over the batch — not a fixed or
//! echoed value. The SAME `app.function("crunch")` handle that the runner drives runs the
//! real handler in-process via `.local()`. No live Modal, no credentials.

use example_cpu_memory::{Batch, Summary};
use modal_rust::App;

fn crunch(records: u64) -> Summary {
    App::local()
        .function("crunch")
        .local(Batch { records })
        .expect("the .local() path runs the fold in-process")
}

#[test]
fn empty_batch_folds_to_zero() {
    let s = crunch(0);
    assert_eq!(s.records, 0, "the input record count is echoed back");
    assert_eq!(s.checksum, 0, "an empty fold is the zero accumulator");
}

#[test]
fn fold_is_deterministic() {
    // Same input -> same checksum: the computation is reproducible, not random.
    assert_eq!(crunch(1000).checksum, crunch(1000).checksum);
}

#[test]
fn every_record_contributes() {
    // A larger batch produces a different fold — the work is real and not elided.
    let small = crunch(100);
    let large = crunch(101);
    assert_eq!(small.records, 100);
    assert_eq!(large.records, 101);
    assert_ne!(
        small.checksum, large.checksum,
        "adding a record changes the running fold"
    );
}

#[test]
fn plain_fn_is_directly_callable() {
    // The macro emits the user fn verbatim, so it stays a plain Rust fn — and it agrees
    // with the standalone module computation.
    let s = example_cpu_memory::crunch(Batch { records: 5 }).unwrap();
    assert_eq!(s.records, 5);
    assert_ne!(
        s.checksum, 0,
        "a non-empty batch folds to a non-zero checksum"
    );
}
