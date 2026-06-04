//! The `.local()` proof for the facade.
//!
//! Uses the `example-add` dev-dependency (acyclic: example-add → modal-rust-runtime
//! only, never → modal-rust) as the real registered handler set.

use example_add::{modal_registry, AddInput, AddOutput};
use modal_rust::{App, Error};

#[test]
fn local_runs_real_add() {
    let app = App::new(modal_registry());
    let out: AddOutput = app.function("add").local(AddInput { a: 40, b: 2 }).unwrap();
    assert_eq!(out.sum, 42); // the M0 proof
}

#[test]
fn local_unknown_entrypoint_errors() {
    let app = App::new(modal_registry());
    let err = app
        .function("nope")
        .local::<_, AddOutput>(AddInput { a: 1, b: 2 })
        .unwrap_err();
    match err {
        Error::UnknownEntrypoint { name, known } => {
            assert_eq!(name, "nope");
            assert!(known.iter().any(|n| n == "add")); // known names listed
        }
        other => panic!("expected UnknownEntrypoint, got {other:?}"),
    }
}

#[test]
fn local_surfaces_function_error() {
    // example_add::fail -> anyhow -> RunnerError::Function -> Error::Runner
    let app = App::new(modal_registry());
    let err = app
        .function("fail")
        .local::<_, AddOutput>(AddInput { a: 1, b: 2 })
        .unwrap_err();
    assert!(matches!(
        err,
        Error::Runner(modal_rust::RunnerError::Function { .. })
    ));
}

#[test]
fn local_surfaces_decode_error_on_wrong_output_shape() {
    // `add` returns {sum:i64}; asking for a String output cannot deserialize.
    let app = App::new(modal_registry());
    let err = app
        .function("add")
        .local::<_, String>(AddInput { a: 1, b: 2 })
        .unwrap_err();
    assert!(matches!(err, Error::Decode(_)), "got {err:?}");
}

#[tokio::test]
async fn remote_on_offline_app_is_not_connected() {
    // `.remote()` needs App::connect; an offline App (App::new) has no control-plane
    // handle, so it errors with NotConnected BEFORE any network call. The
    // #[tokio::test] runtime drives the immediately-ready future.
    let app = App::new(modal_registry());
    let res = app
        .function("add")
        .remote::<_, AddOutput>(AddInput { a: 1, b: 2 })
        .await;
    assert!(matches!(res, Err(Error::NotConnected(_))), "got {res:?}");
}

#[tokio::test]
async fn spawn_and_map_on_offline_app_are_not_connected() {
    // `.spawn()`/`.map()` are implemented now and drive the SAME RUN path as
    // `.remote()`; on an offline App (no `connect`) they hit the same NotConnected
    // guard BEFORE any network call. This also pins the implemented surface shapes:
    // `spawn` returns a `FunctionCall<'_>` handle, `map` returns `Vec<Out>`.
    let app = App::new(modal_registry());
    let f = app.function("add");
    let spawned = f.spawn(AddInput { a: 1, b: 2 }).await;
    assert!(
        matches!(spawned, Err(Error::NotConnected(_))),
        "{spawned:?}"
    );
    let mapped = f
        .map::<_, AddOutput, _>(vec![AddInput { a: 1, b: 2 }])
        .await;
    assert!(matches!(mapped, Err(Error::NotConnected(_))), "{mapped:?}");
}
