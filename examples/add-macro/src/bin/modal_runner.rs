//! The runner binary for the macro example — the WHOLE thing, one line.
//!
//! `modal_rust::modal_runner!(<lib>)` expands to the runner `main()`: it links the
//! library crate's `#[modal_rust::function]` `inventory` submissions, assembles the
//! registry + decorator configs, and runs the FROZEN runner CLI protocol (prints
//! exactly one JSON envelope to stdout, mirrors `ok` in the exit code). The user
//! never writes `main()` and never names the hidden `__private` runtime re-exports
//! — that all lives in GENERATED code (the serde_derive pattern).
//!
//! The lib crate name (`example_add_macro`) is passed because a `[[bin]]` does not
//! auto-link its package's lib in Cargo; the macro emits the `use <crate> as _;`
//! link from it. (A single-binary crate with the functions in this file would write
//! the bare `modal_rust::modal_runner!();`.)

modal_rust::modal_runner!(example_add_macro);
