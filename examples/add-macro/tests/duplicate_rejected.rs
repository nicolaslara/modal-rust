//! Proves `Registry::from_inventory()` rejects duplicate entrypoint names with the
//! SAME hard error as the manual `Registry::function()` builder (boundaries.md §3,
//! ergonomics E1: "no silent last-write-wins").
//!
//! This lives in its own integration-test binary so the two duplicate
//! `inventory::submit!` registrations below do not pollute the library's
//! unit-test inventory set. We submit `Registration` directly (the exact shape the
//! `#[modal_rust::function]` macro emits) twice under the same name and assert
//! `from_inventory()` panics with the frozen "duplicate entrypoint" message.

use modal_rust_runtime::{FunctionConfig, HandlerFn, Registration, Registry, RunnerError};

fn dup_handler(_input: &[u8]) -> Result<Vec<u8>, RunnerError> {
    Ok(b"null".to_vec())
}

inventory::submit! {
    Registration { name: "dup", handler: dup_handler as HandlerFn, config: FunctionConfig::new() }
}

inventory::submit! {
    Registration { name: "dup", handler: dup_handler as HandlerFn, config: FunctionConfig::new() }
}

#[test]
fn from_inventory_rejects_duplicate_names() {
    let err = match std::panic::catch_unwind(Registry::from_inventory) {
        Ok(_) => panic!("from_inventory must reject duplicate names"),
        Err(payload) => payload,
    };
    let msg = err
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| err.downcast_ref::<&str>().copied())
        .unwrap_or("");
    assert!(
        msg.contains("duplicate entrypoint"),
        "expected the frozen duplicate-entrypoint hard error, got: {msg:?}"
    );
}
