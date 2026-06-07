//! Offline proof (zero Modal, zero network) that `spin` runs a REAL deterministic
//! computation — the operational knobs (`timeout`, `cache`) are metadata, but the body
//! genuinely folds `0..iterations` into a checksum. `.local()` round-trips the user
//! structs through the macro-inferred wire I/O without any Modal in the loop.
//!
//! We assert the real output PROPERTIES (not a fixed echoed value): the empty fold is
//! zero, the fold is deterministic (same input -> same output), and a longer run
//! produces a different checksum, so the work is observable and not elided.

use example_timeout_and_cache::{Done, Job};
use modal_rust::App;

#[test]
fn spin_runs_the_real_fold_via_local() {
    let app = App::local();
    let done: Done = app
        .function("spin")
        .local(Job { iterations: 1_000 })
        .unwrap();

    // The iterations echo the input...
    assert_eq!(done.iterations, 1_000);
    // ...and the checksum is the real fold, not a fixed constant or an echo.
    assert_ne!(done.checksum, 0);
    assert_ne!(done.checksum, 1_000);
}

#[test]
fn empty_fold_is_zero() {
    let done = example_timeout_and_cache::spin(Job { iterations: 0 }).unwrap();
    assert_eq!(done.iterations, 0);
    assert_eq!(done.checksum, 0);
}

#[test]
fn fold_is_deterministic() {
    // Same input -> same output: a pure CPU function, safe to retry/cache.
    let a = example_timeout_and_cache::spin(Job { iterations: 5_000 }).unwrap();
    let b = example_timeout_and_cache::spin(Job { iterations: 5_000 }).unwrap();
    assert_eq!(a.checksum, b.checksum);
}

#[test]
fn longer_runs_change_the_checksum() {
    // The loop actually runs every iteration: more work -> a different accumulator.
    let short = example_timeout_and_cache::spin(Job { iterations: 100 }).unwrap();
    let long = example_timeout_and_cache::spin(Job { iterations: 101 }).unwrap();
    assert_ne!(short.checksum, long.checksum);
}
