//! Offline proof of the quickstart's `.local()` path (zero Modal, zero network) —
//! the exact ergonomic behavior the README promises. This crate is a PURE LIBRARY —
//! the `modal-rust` CLI generates the runner automatically; the runner protocol itself
//! (`--describe` / `--entrypoint`) is exercised via the CLI (see the verification
//! commands and `examples/add-macro`). This test stays on the clean public surface:
//! no `__private`.

use modal_rust::App;
use quickstart::AddCall;

#[test]
fn typed_local_add_returns_5() {
    // The headline ergonomic call: a typed positional method, no I/O type named,
    // brought into scope with one `use quickstart::AddCall;`.
    let app = App::local();
    let sum: i64 = app.add(2, 3).local().unwrap();
    assert_eq!(sum, 5);
}

#[test]
fn glob_import_also_brings_the_typed_method() {
    // The README-clean alternative: a single glob brings every `<Pascal>Call` trait
    // into scope at once.
    use quickstart::*;
    let app = App::local();
    let sum: i64 = app.add(2, 3).local().unwrap();
    assert_eq!(sum, 5);
}

#[test]
fn plain_fn_is_directly_callable() {
    // The macro emits the user fn verbatim, so it stays a plain Rust fn.
    assert_eq!(quickstart::add(40, 2).unwrap(), 42);
}

#[test]
fn explicit_named_input_path_local() {
    // The generated `add::Input` is also usable via the string-keyed path — no
    // `__private`, just the public `App::function`.
    let app = App::local();
    let sum: i64 = app
        .function("add")
        .local(quickstart::add::Input { a: 2, b: 3 })
        .unwrap();
    assert_eq!(sum, 5);
}
