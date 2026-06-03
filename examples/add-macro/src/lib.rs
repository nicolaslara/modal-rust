//! The macro-path twin of `examples/add` (ergonomics E1).
//!
//! This crate proves the macro-compatibility invariant (boundaries.md §3): the
//! `#[modal_rust::function]` attribute is **pure additive sugar** that compiles
//! down to the SAME monomorphized `typed!` wrapper `fn` pointer and the SAME
//! `Registry` / `HandlerFn` shape as the manual `examples/add`. There is no
//! `modal_registry()` builder here — the runner binary calls
//! `Registry::from_inventory()`, which collects the macro's `inventory::submit!`
//! registration into the identical `BTreeMap<&'static str, HandlerFn>`.
//!
//! Driven by the **unchanged** `run_cli`, the macro-registered `add` produces
//! byte-identical output to the manual path:
//! `modal_runner --entrypoint add --input-json '{"a":40,"b":2}'`
//! prints exactly `{"ok":true,"value":{"sum":42}}` and exits 0.

// Alias the proc-macro crate so the attribute is spelled `#[modal_rust::function]`
// exactly as boundaries.md §3 / the ergonomics tasks specify. The macro's
// generated code references the runtime and `inventory` by their real crate names
// (`::modal_rust_runtime`, `::inventory`), independent of this alias.
extern crate modal_rust_macros as modal_rust;

use serde::{Deserialize, Serialize};

/// The single named-JSON-object input for `add` (boundaries.md §3: never a
/// positional array). Mirrors `examples/add::AddInput`.
#[derive(Debug, Deserialize)]
pub struct AddInput {
    /// First addend.
    pub a: i64,
    /// Second addend.
    pub b: i64,
}

/// The output of `add`. Mirrors `examples/add::AddOutput`.
#[derive(Debug, Serialize)]
pub struct AddOutput {
    /// `a + b`.
    pub sum: i64,
}

/// Add two integers — the macro-registered entrypoint.
///
/// `#[modal_rust::function]` expands to this unchanged fn PLUS an
/// `inventory::submit!` of a `Registration { name: "add", handler:
/// modal_rust_runtime::typed!(add) }`. The name defaults to the fn name (`add`);
/// `#[modal_rust::function(name = "...")]` would override it. The handler is the
/// SAME monomorphized `typed!` wrapper the manual `examples/add` registers by
/// hand, so the runner protocol and envelope are identical.
#[modal_rust::function]
pub fn add(input: AddInput) -> anyhow::Result<AddOutput> {
    Ok(AddOutput {
        sum: input.a + input.b,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use modal_rust_runtime::Registry;

    #[test]
    fn add_works() {
        let out = add(AddInput { a: 40, b: 2 }).unwrap();
        assert_eq!(out.sum, 42);
    }

    #[test]
    fn from_inventory_registers_add() {
        // The macro's `inventory::submit!` must surface `add` through
        // `Registry::from_inventory()` — the same lookup the manual builder gives.
        let reg = Registry::from_inventory();
        assert!(reg.get("add").is_some(), "macro did not register `add`");
        assert!(reg.get("nope").is_none());
    }

    #[test]
    fn macro_path_byte_identical_to_manual() {
        // Drive the UNCHANGED run_cli with the macro-built registry and assert the
        // success envelope is byte-for-byte the manual-path output.
        let argv: Vec<String> = ["--entrypoint", "add", "--input-json", r#"{"a":40,"b":2}"#]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut buf = Vec::new();
        let code =
            modal_rust_runtime::run_cli_with_args(Registry::from_inventory(), &argv, &mut buf);
        assert_eq!(code, 0);
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "{\"ok\":true,\"value\":{\"sum\":42}}\n"
        );
    }

    #[test]
    fn unknown_entrypoint_still_works() {
        // An error kind on the macro-built runner behaves identically to manual.
        let argv: Vec<String> = ["--entrypoint", "nope", "--input-json", "{}"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut buf = Vec::new();
        let code =
            modal_rust_runtime::run_cli_with_args(Registry::from_inventory(), &argv, &mut buf);
        assert_eq!(code, 1);
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["kind"], "unknown_entrypoint");
        assert_eq!(v["error"]["details"], serde_json::Value::Null);
    }
}
