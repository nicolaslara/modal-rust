//! The runner binary — the whole thing, one line. This is the ONLY binary this
//! crate ships: there is no hand-written driver. The `modal-rust` CLI builds this
//! `--bin modal_runner` to read the `--describe` manifest and to bake into the
//! deploy image; `modal-rust run`/`deploy`/`call` are the operations surface.
//!
//! `modal_runner!(<lib>)` expands to the runner `main()`: it links the lib crate's
//! `#[function]` registrations, assembles the registry + decorator configs, and runs
//! the FROZEN runner CLI protocol (one JSON envelope to stdout, `ok` mirrored in the
//! exit code). No `main()` to write, no `__private` to name.

modal_rust::modal_runner!(example_cli_workflow);
