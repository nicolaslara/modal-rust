//! How a failure crosses the boundary — and what the caller does with it.
//!
//! Two `#[modal_rust::function]`s from this crate's `lib.rs` fail on purpose:
//!
//! - `withdraw`         returns `anyhow::Result<_>`        → OPAQUE error, `details: null`.
//! - `withdraw_checked` returns `Result<_, WithdrawError>` → STRUCTURED error, `details: {…}`.
//!
//! Both come back as the SAME frozen failure kind (`function_error`); the only
//! difference is whether `details` carries the typed error. This tour runs both
//! OFFLINE through `.local()` (zero Modal, zero network), prints the exact wire
//! envelope each produces, and shows the caller BRANCHING on the structured one.
//!
//! `cargo run -p example-error-handling --bin error_handling`

use example_error_handling::{Receipt, WithdrawCall, WithdrawCheckedCall, WithdrawError};
use modal_rust::{App, Error, RunnerError};

fn main() {
    let app = App::local();

    // ----- 1. Plain anyhow error: opaque, `details: null` ------------------------
    //
    // `withdraw` returns `anyhow::Result`, so a failure is OPAQUE to the caller: a
    // human `message` and nothing machine-readable.
    let err = app
        .withdraw(150, 100)
        .local()
        .expect_err("over-withdrawing must fail");
    println!("anyhow:     {}", caller_view(&err));
    println!("  envelope: {}", wire_envelope(&err));

    // ----- 2. Structured Serialize error: `details` carries the typed error ------
    //
    // `withdraw_checked` returns `Result<_, WithdrawError>` and `WithdrawError`
    // derives `Serialize`, so the SAME failure now carries a machine-readable
    // `details` object alongside the `message`.
    let err = app
        .withdraw_checked(150, 100)
        .local()
        .expect_err("over-withdrawing must fail");
    println!("structured: {}", caller_view(&err));
    println!("  envelope: {}", wire_envelope(&err));

    // ----- 3. Branch on the structured error -------------------------------------
    //
    // The point of `details`: the caller deserializes it back into the typed error
    // and matches on it — no string-scraping. Here we recover the exact shortfall
    // and decide what to do.
    match classify(&err) {
        Some(WithdrawError::InsufficientFunds { shortfall }) => {
            println!("branch:     short by {shortfall} cents -> prompt a top-up");
        }
        Some(WithdrawError::NonPositive { amount }) => {
            println!("branch:     bad amount {amount} -> reject the request");
        }
        None => println!("branch:     opaque error -> log the message and bail"),
    }

    // The happy path is unchanged: a success decodes straight to the typed `Out`.
    let ok: Receipt = app
        .withdraw(40, 100)
        .local()
        .expect("a valid withdrawal succeeds");
    println!(
        "ok:         withdrew {} cents, {} remaining",
        ok.withdrawn, ok.remaining
    );
}

/// What the caller sees: the failure kind + the human-readable message, and whether
/// a machine-readable `details` rode along.
fn caller_view(err: &Error) -> String {
    match err {
        Error::Runner(r) => {
            let has_details = r.details().is_some();
            format!(
                "kind={}, message={:?}, details={}",
                r.kind(),
                r.message(),
                if has_details { "present" } else { "null" }
            )
        }
        other => format!("non-handler error: {other}"),
    }
}

/// The exact JSON envelope the runner emits for this failure — proving the wire
/// shape `.local()` and `.remote()` share.
fn wire_envelope(err: &Error) -> String {
    match err {
        Error::Runner(r) => r.to_envelope().to_string(),
        other => format!("{{\"non_handler_error\":{:?}}}", other.to_string()),
    }
}

/// Recover the typed error from a structured failure by deserializing `details`.
/// Returns `None` for an opaque (anyhow) error — there is nothing machine-readable
/// to branch on.
fn classify(err: &Error) -> Option<WithdrawError> {
    match err {
        Error::Runner(RunnerError::Function {
            details: Some(value),
            ..
        }) => serde_json::from_value(value.clone()).ok(),
        _ => None,
    }
}
