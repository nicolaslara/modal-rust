//! The runner binary for the deploy-and-call example — the WHOLE thing, one line.
//!
//! `modal_rust::modal_runner!(<lib>)` expands to the runner `main()`: it links the
//! library crate's `#[modal_rust::function]` `inventory` submissions, assembles the
//! registry + decorator configs, and runs the FROZEN runner CLI protocol (prints
//! exactly one JSON envelope to stdout, mirrors `ok` in the exit code). This is the
//! SAME `/app/modal_runner` that the DEPLOY image bakes once at image-build time and
//! that every `call` execs with no rebuild. The user never writes `main()`.

modal_rust::modal_runner!(example_deploy_and_call);
