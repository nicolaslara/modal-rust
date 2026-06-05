//! The runner binary for the macro example (boundaries.md §1.4, ergonomics E1).
//!
//! Identical in shape to `examples/add`'s runner, except the registry is assembled
//! from the macro's `inventory` submissions via `Registry::from_inventory()`
//! instead of a hand-written `modal_registry()` builder. Both converge on the SAME
//! `Registry` and the UNCHANGED `run_cli`, so the runner protocol is identical.
//!
//! It prints exactly one JSON envelope to stdout and mirrors `ok` in the exit code
//! (boundaries.md §2): `0` success, `1` failure.

// The macro's `inventory::submit!` lives in the library crate; pull it in so its
// registration is linked and visible to `Registry::from_inventory()`.
use example_add_macro as _;

// The frozen runner protocol lives in `modal-rust-runtime`, reached through the
// facade's hidden `__private::runtime` re-export — so this crate depends only on the
// `modal-rust` facade (no direct `modal-rust-runtime`).
use modal_rust_facade::__private::runtime;

fn main() -> std::process::ExitCode {
    // Config-carrying entry (P9 §A.4): `from_inventory_with_configs` threads the
    // decorator gpu/timeout/cache into the additive `--describe` manifest. The
    // FROZEN `--entrypoint` dispatch ignores the configs, so this is behavior-
    // identical to `run_cli(Registry::from_inventory())` for the runner protocol.
    let (registry, configs) = runtime::from_inventory_with_configs();
    let code = runtime::run_cli_with_configs(registry, &configs);
    std::process::ExitCode::from(code as u8)
}
