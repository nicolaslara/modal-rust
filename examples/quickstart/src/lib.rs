//! The README quickstart — the whole modal-rust authoring surface, as a real crate.
//!
//! The code between the `quickstart:begin` / `quickstart:end` markers is the EXACT
//! authoring code the README's ```rust quickstart``` block shows; a drift-guard test
//! (`tests/readme_drift.rs`) reads README.md, extracts that block, and asserts it
//! matches this span byte-for-byte — so a stale README is a TEST FAILURE. Because
//! this crate compiles and its `.local()` test passes, "README == crate source"
//! implies "the README authoring code compiles and runs".
//!
//! The companion `src/bin/modal_runner.rs` is the one-line runner; `tests/local.rs`
//! proves `App::local()` + `app.add(2, 3).local()? == 5`.

// quickstart:begin
use modal_rust::function;

/// Add two integers — the whole function. `#[function]` generates the JSON
/// input/output plumbing, registers the entrypoint via `inventory`, and adds a
/// typed `app.add(2, 3)` method to `App` (brought into scope with one `use`:
/// `use quickstart::AddCall;`, or `use quickstart::*;`).
#[function]
pub fn add(a: i64, b: i64) -> anyhow::Result<i64> {
    Ok(a + b)
}
// quickstart:end
