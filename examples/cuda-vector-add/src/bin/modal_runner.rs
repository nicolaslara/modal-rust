//! The runner binary for the M12 cuda-vector-add example (boundaries.md §1.4).
//! The user does NOT own `main()`. The registry is assembled from the macro's
//! `inventory` submissions via `from_inventory_with_configs()` (the decorator gpu
//! rides into `--describe`); both converge on the UNCHANGED `run_cli`, so the runner
//! protocol is identical to the manual path.
//!
//! It prints exactly one JSON envelope to stdout and mirrors `ok` in the exit
//! code (boundaries.md §2): `0` success, `1` failure.

// The macro's `inventory::submit!` lives in the library crate; pull it in so its
// registration is linked and visible to `from_inventory_with_configs()`.
use example_cuda_vector_add as _;

// The frozen runner protocol lives in `modal-rust-runtime`, reached through the
// facade's hidden `__private::runtime` re-export — so this crate depends only on the
// `modal-rust` facade (no direct `modal-rust-runtime`).
use modal_rust_facade::__private::runtime;

fn main() -> std::process::ExitCode {
    let (registry, configs) = runtime::from_inventory_with_configs();
    let code = runtime::run_cli_with_configs(registry, &configs);
    std::process::ExitCode::from(code as u8)
}
