//! The runner binary for the timeout-and-cache example — the WHOLE thing, one line.
//!
//! `modal_rust::modal_runner!(<lib>)` expands to the runner `main()`: it links the
//! library crate's `#[modal_rust::function]` `inventory` submissions, assembles the
//! registry + decorator configs, and runs the FROZEN runner CLI protocol (prints
//! exactly one JSON envelope to stdout, mirrors `ok` in the exit code). `--describe`
//! shows the `timeout_secs` + `cache` knobs riding on the entrypoint's config. The
//! user never writes `main()`.

modal_rust::modal_runner!(example_timeout_and_cache);
