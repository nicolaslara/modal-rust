//! Essential offline behavior for the new headline `add` (the macro auto-I/O form).

use example_add_macro::add;
use example_add_macro::AddCall;
use modal_rust::__private::runtime;
use modal_rust::{App, Registry};

#[test]
fn add_is_a_plain_fn() {
    // The macro emits the user fn verbatim, so it stays directly callable.
    assert_eq!(add(2, 3).unwrap(), 5);
}

#[test]
fn add_generates_named_input_and_output() {
    // Mode B generated nameable `add::Input { a, b }` (Serialize + Deserialize) and
    // `add::Output` (= i64); the input serializes to the frozen named JSON object.
    let json = serde_json::to_string(&add::Input { a: 2, b: 3 }).unwrap();
    assert_eq!(json, r#"{"a":2,"b":3}"#);
    let back: add::Input = serde_json::from_str(r#"{"a":40,"b":2}"#).unwrap();
    assert_eq!((back.a, back.b), (40, 2));
    let _out: add::Output = 5i64;
}

#[test]
fn add_registered_via_inventory_runner_envelope() {
    // The generated spread shim is registered under `add` and dispatches through the
    // UNCHANGED runner: wire input `{"a":2,"b":3}` -> envelope `{"ok":true,"value":5}`.
    assert!(Registry::from_inventory().get("add").is_some());
    let argv: Vec<String> = ["--entrypoint", "add", "--input-json", r#"{"a":2,"b":3}"#]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut buf = Vec::new();
    let code = runtime::run_cli_with_args(Registry::from_inventory(), &argv, &mut buf);
    assert_eq!(code, 0);
    assert_eq!(
        String::from_utf8(buf).unwrap(),
        "{\"ok\":true,\"value\":5}\n"
    );
}

#[test]
fn add_typed_app_method_local() {
    // Auto-I/O ergonomics: typed positional method, no input/output type named.
    let app = App::local();
    let sum: i64 = app.add(2, 3).local().unwrap();
    assert_eq!(sum, 5);
}

#[test]
fn add_explicit_input_path_local() {
    // The generated input stays callable explicitly via the string-keyed path.
    let app = App::local();
    let sum: i64 = app
        .function("add")
        .local(add::Input { a: 2, b: 3 })
        .unwrap();
    assert_eq!(sum, 5);
}

#[test]
fn unknown_entrypoint_error_kind() {
    let argv: Vec<String> = ["--entrypoint", "nope", "--input-json", "{}"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut buf = Vec::new();
    let code = runtime::run_cli_with_args(Registry::from_inventory(), &argv, &mut buf);
    assert_eq!(code, 1);
    let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["kind"], "unknown_entrypoint");
}
