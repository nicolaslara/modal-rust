//! Offline proof (zero Modal, zero network) of the REAL persistence the example
//! teaches: a `std::fs` append followed by a read-back line count. A live run points
//! this work at the volume mount `/data`; offline we point the SAME extracted core
//! (`visit_log::record`) at a temp directory — the local stand-in for the mount — and
//! assert the real, observable behavior:
//!
//! - the line written by an earlier call is still there for a later call, so the
//!   returned count grows by exactly one per call against the same directory (this is
//!   the persistence the volume provides on a live run);
//! - the labels land in the file once each, in call order;
//! - the first call against a fresh (missing) directory creates it and reports `1`.
//!
//! `tests/manifest.rs` separately proves the named volume rides into the planned
//! `FunctionCreate` manifest at its mount path; together they cover the wire config
//! AND the real computation, both fully offline.

use example_volumes::visit_log::record;

/// A unique scratch directory per test — the offline stand-in for the volume mount.
fn scratch(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("mr-volumes-test-{tag}-{}", std::process::id()))
}

#[test]
fn count_grows_by_one_per_call_proving_persistence() {
    let dir = scratch("persistence");
    let _ = std::fs::remove_dir_all(&dir); // start from a fresh "volume"

    // Each call reads back the lines the previous calls wrote: real persistence.
    assert_eq!(record(&dir, "first").unwrap(), 1); //  fresh log -> 1 visit
    assert_eq!(record(&dir, "second").unwrap(), 2); // first line survived -> 2 visits
    assert_eq!(record(&dir, "third").unwrap(), 3); //  and again -> 3 visits

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn labels_are_appended_once_each_in_order() {
    let dir = scratch("order");
    let _ = std::fs::remove_dir_all(&dir);

    record(&dir, "alpha").unwrap();
    record(&dir, "beta").unwrap();
    record(&dir, "gamma").unwrap();

    let contents = std::fs::read_to_string(dir.join("visits.log")).unwrap();
    assert_eq!(
        contents.lines().collect::<Vec<_>>(),
        ["alpha", "beta", "gamma"],
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn first_call_against_a_fresh_volume_creates_the_dir_and_counts_one() {
    // A freshly attached, empty volume: the mount directory does not exist yet.
    let dir = scratch("fresh");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!dir.exists());

    assert_eq!(record(&dir, "only").unwrap(), 1);
    assert!(dir.join("visits.log").exists());

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn each_run_against_the_same_dir_is_deterministic_for_a_given_prior_state() {
    // Real but deterministic: the count a call returns is fully determined by how many
    // lines the file already held. Two separate "volumes" stepped identically yield
    // identical counts at every step.
    let a = scratch("det-a");
    let b = scratch("det-b");
    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);

    for label in ["x", "y", "z"] {
        assert_eq!(record(&a, label).unwrap(), record(&b, label).unwrap());
    }

    std::fs::remove_dir_all(&a).unwrap();
    std::fs::remove_dir_all(&b).unwrap();
}
