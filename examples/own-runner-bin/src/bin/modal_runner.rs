//! The runner binary — the WHOLE thing, one line. This is the file the rest of the
//! crate exists to demonstrate: a HAND-WRITTEN `modal_runner` target you own and keep
//! in your tree, instead of the one the CLI generates for a pure-library crate.
//!
//! `modal_rust::modal_runner!(<lib>)` expands to the runner `main()`: it links the
//! library crate's `#[modal_rust::function]` `inventory` submissions (a `[[bin]]` does
//! not auto-link its package's lib, so the macro emits `use own_runner_bin as _;` to
//! force the registrations in), assembles the registry + decorator configs, and runs
//! the FROZEN runner CLI protocol (exactly one JSON envelope to stdout, `ok` mirrored
//! in the exit code).
//!
//! WHY keep your own copy instead of the generated one? Because it is YOUR `main()`:
//! the CLI auto-detects this `modal_runner` bin and uses it as-is, so you can wrap the
//! macro with extra startup logic — e.g. `tracing_subscriber::fmt().init();` before the
//! line below, read a config file, or set a process-wide env var — and every run/deploy
//! goes through it. The lib name (`own_runner_bin`) is passed because the bin does not
//! auto-link its package's lib.

modal_rust::modal_runner!(own_runner_bin);
