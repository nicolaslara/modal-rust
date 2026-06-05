//! `examples/add-macro` — the macro path, written the way a user would write it.
//!
//! `#[modal_rust::function]` turns a plain Rust function into a Modal function:
//! it generates the JSON input/output plumbing, registers the entrypoint through
//! `inventory` (no `modal_registry()` builder to maintain), and adds a typed
//! `app.add(2, 3)` method — so the call site never names an input/output type.
//! This is the Rust twin of Python's `@app.function()\ndef add(a, b): return a + b`.

// The ONLY modal dependency is `modal-rust` (under its own name — NO rename). The
// macro routes every emitted path through this facade, so this crate needs nothing
// else modal-related. `use modal_rust::function;` makes the attribute spellable as
// `#[function]` (or `#[modal_rust::function]`) — exactly the README form a fresh
// external user would write.
use modal_rust::function;

/// Add two integers — the whole function.
///
/// The macro generates `add::Input { a, b }` / `add::Output` (= `i64`), registers
/// the entrypoint via `inventory`, and adds a typed `app.add(2, 3)` method that
/// chains into `.local()` / `.remote().await` / `.spawn()` / `.map(..)`.
#[function]
pub fn add(a: i64, b: i64) -> anyhow::Result<i64> {
    Ok(a + b)
}

/// Extra entrypoints that keep the decorator-config and live secrets/volumes
/// coverage compiling and registered, kept out of the headline so `add` above
/// reads clean. See `proof.rs`.
pub mod proof;
