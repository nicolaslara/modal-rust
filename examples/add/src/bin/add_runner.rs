//! The hand-written runner binary for the MANUAL/internals reference example
//! (boundaries.md §1.4). This bin is deliberately named `add-runner` (NOT
//! `modal_runner`): `example-add` builds its `Registry` BY HAND
//! (`modal_rust_runtime::run_cli(example_add::modal_registry())`, no `inventory`,
//! no `modal-rust` facade), so the tooling cannot generate a `modal_runner` for it.
//! The unique name keeps it from colliding with the generated/own `modal_runner`
//! bins the rest of the workspace uses (cargo#6313 output-filename collision).
//!
//! It prints exactly one JSON envelope to stdout and mirrors `ok` in the exit code
//! (boundaries.md §2): `0` success, `1` failure.

fn main() -> std::process::ExitCode {
    let code = modal_rust_runtime::run_cli(example_add::modal_registry());
    std::process::ExitCode::from(code as u8)
}
