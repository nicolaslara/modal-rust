//! Offline proof (zero Modal, zero network) that the four invocation shapes hang
//! off the SAME typed `app.square(n)` method. Only `.local()` is exercised here —
//! it is the one shape that needs no Modal; the live `.remote()` / `.spawn()` /
//! `.map()` shapes are compiled by the binary and proven against real Modal in the
//! credential-gated tour (`RUN_REMOTE=1 cargo run -p example-ways-to-call --bin
//! ways_to_call`).

use example_ways_to_call::SquareCall;
use modal_rust::App;

#[test]
fn typed_local_square_returns_36() {
    // The headline ergonomic call: a typed positional method, no I/O type named,
    // brought into scope with one `use example_ways_to_call::SquareCall;`.
    let app = App::local();
    let squared: i64 = app.square(6).local().unwrap();
    assert_eq!(squared, 36);
}

#[test]
fn glob_import_also_brings_the_typed_method() {
    // The README-clean alternative: a single glob brings every `<Pascal>Call` trait
    // into scope at once.
    use example_ways_to_call::*;
    let app = App::local();
    let squared: i64 = app.square(9).local().unwrap();
    assert_eq!(squared, 81);
}

#[test]
fn plain_fn_is_directly_callable() {
    // The macro emits the user fn verbatim, so it stays a plain Rust fn.
    assert_eq!(example_ways_to_call::square(12).unwrap(), 144);
}
