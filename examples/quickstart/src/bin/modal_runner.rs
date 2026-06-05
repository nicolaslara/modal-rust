//! The runner binary — the whole thing, one line. `modal_runner!(<lib>)` expands to
//! the runner `main()`: it links the lib crate's `#[function]` registrations,
//! assembles the registry + configs, and runs the FROZEN runner CLI protocol (one
//! JSON envelope to stdout, `ok` mirrored in the exit code). No `main()` to write, no
//! `__private` to name.

modal_rust::modal_runner!(quickstart);
