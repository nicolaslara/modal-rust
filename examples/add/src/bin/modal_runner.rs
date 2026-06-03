//! The runner binary (boundaries.md §1.4). The user does NOT own `main()`: the CLI
//! owns this ~15-line template whose fixed body is
//! `modal_rust_runtime::run_cli(user_crate::modal_registry())`.
//!
//! It prints exactly one JSON envelope to stdout and mirrors `ok` in the exit code
//! (boundaries.md §2): `0` success, `1` failure.

fn main() -> std::process::ExitCode {
    let code = modal_rust_runtime::run_cli(example_add::modal_registry());
    std::process::ExitCode::from(code as u8)
}
