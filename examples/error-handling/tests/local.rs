//! Offline proof (zero Modal, zero network) of how a failure crosses the boundary:
//! a plain `anyhow` error is OPAQUE (`details = null`); a `Serialize` error is
//! STRUCTURED (`details` carries the typed error the caller branches on). Both land
//! on the SAME frozen `function_error` kind.

use example_error_handling::{Receipt, WithdrawCall, WithdrawCheckedCall, WithdrawError};
use modal_rust::{App, Error, RunnerError};

#[test]
fn anyhow_error_is_opaque_with_null_details() {
    let app = App::local();
    let err = app
        .withdraw(150, 100)
        .local()
        .expect_err("over-withdrawing must fail");

    match err {
        Error::Runner(r @ RunnerError::Function { .. }) => {
            assert_eq!(r.kind(), "function_error");
            assert!(r.message().contains("insufficient funds"));
            // The anyhow path is opaque: nothing machine-readable.
            assert!(r.details().is_none(), "anyhow error must have null details");
        }
        other => panic!("expected a function_error, got {other:?}"),
    }
}

#[test]
fn serialize_error_carries_structured_details() {
    let app = App::local();
    let err = app
        .withdraw_checked(150, 100)
        .local()
        .expect_err("over-withdrawing must fail");

    let details = match &err {
        Error::Runner(RunnerError::Function {
            details: Some(value),
            ..
        }) => value.clone(),
        other => panic!("expected a function_error with details, got {other:?}"),
    };

    // The caller branches on the typed error recovered from `details` — no
    // string-scraping.
    let recovered: WithdrawError =
        serde_json::from_value(details).expect("details deserializes back to WithdrawError");
    match recovered {
        WithdrawError::InsufficientFunds { shortfall } => assert_eq!(shortfall, 50),
        other => panic!("expected InsufficientFunds, got {other:?}"),
    }
}

#[test]
fn both_failures_share_the_function_error_kind() {
    let app = App::local();
    let opaque = app
        .withdraw(-1, 100)
        .local()
        .expect_err("a negative amount must fail");
    let structured = app
        .withdraw_checked(-1, 100)
        .local()
        .expect_err("a negative amount must fail");

    for err in [&opaque, &structured] {
        match err {
            Error::Runner(r) => assert_eq!(r.kind(), "function_error"),
            other => panic!("expected a function_error, got {other:?}"),
        }
    }
}

#[test]
fn happy_path_decodes_to_the_typed_receipt() {
    let app = App::local();
    let ok: Receipt = app
        .withdraw(40, 100)
        .local()
        .expect("a valid withdrawal succeeds");
    assert_eq!(ok.withdrawn, 40);
    assert_eq!(ok.remaining, 60);
}
