//! The runner binary for the M12 cuda-vector-add example — the whole thing, one
//! line.
//!
//! `modal_runner!(<lib>)` expands to the runner `main()`. It links the lib crate's
//! `#[modal_rust::function]` registrations, assembles the registry plus decorator
//! configs (the gpu rides into `--describe`), and runs the FROZEN runner CLI
//! protocol — one JSON envelope to stdout, `ok` mirrored in the exit code. There is
//! no hand-written `main()` and no `__private` in user code.
//!
//! Spelled through the facade alias (`modal_rust_facade`) this crate uses; the lib
//! crate name is passed because a `[[bin]]` does not auto-link its package's lib.

modal_rust_facade::modal_runner!(example_cuda_vector_add);
