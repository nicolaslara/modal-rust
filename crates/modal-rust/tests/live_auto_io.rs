//! Live, best-effort auto-I/O `.remote()` proof — the payoff for the
//! plain-signature ergonomics (macro auto-I/O + typed `app.<fn>(..)` methods).
//!
//! Drives the facade end-to-end against REAL Modal for the PLAIN-SIGNATURE handler
//! `#[modal_rust::function] fn add(a: i64, b: i64) -> anyhow::Result<i64>`
//! (defined in `examples/add-macro`, a dev-dependency). It proves BOTH call shapes:
//!
//! 1. the typed positional method `app.add(2, 3).remote().await? == 5` — the
//!    user names NO type; the macro built `add::Input` from the args and
//!    decoded the return type;
//! 2. the explicit generated-input path
//!    `app.function("add").remote(add::Input { a: 2, b: 3 }) == 5`.
//!
//! Both ride the SAME frozen wire (`{"a":2,"b":3}` in, `{"ok":true,"value":5}` out),
//! built from the user's REAL Rust via `cargo build` IN THE FUNCTION BODY.
//!
//! Gated behind BOTH the `live` cargo feature AND `#[ignore]` so the no-CUDA CI box
//! never runs it. Run locally with:
//!
//! ```text
//! cargo test -p modal-rust --features live --test live_auto_io \
//!     -- --ignored --nocapture
//! ```
//!
//! Modal flakiness ("socket connection closed unexpectedly", build capacity blips)
//! is transient — retry, never block. The hard gates are the offline compiles.

#![cfg(feature = "live")]

use std::time::Duration;

// Link example-add-macro's inventory submissions (incl. the plain-signature
// `add`) AND bring its generated typed-call trait into scope, so
// `App::connect(..)` surfaces the handler and `app.add(..)` resolves.
use example_add_macro::add;
use example_add_macro::AddCall;
use modal_rust::{App, Error};

const APP_NAME: &str = "modal-rust-live-auto-io";
/// The cargo package the facade must upload + `cargo build -p <pkg>` in the function
/// body: the crate that defines the plain-signature `add` handler (its
/// `modal_runner` bin assembles the registry via `Registry::from_inventory()`, so the
/// remote runner registers `add`). `App::connect` reads this via
/// `RemoteConfig::default()` (`MODAL_RUST_PACKAGE`); without it the facade defaults to
/// `example-add`, whose runner does NOT know this plain-signature `add` (→ struct form).
const PACKAGE: &str = "example-add-macro";

/// Treat transport blips and known transient gRPC messages as retryable. Delegates
/// to the SDK's own classifier so the test and the SDK agree on what is transient.
fn is_transient(err: &Error) -> bool {
    match err {
        Error::Sdk(sdk_err) => sdk_err.is_transient(),
        _ => false,
    }
}

#[tokio::test]
#[ignore = "live Modal auto-I/O .remote() round-trip; run with --features live -- --ignored"]
async fn remote_auto_io_add_returns_5() {
    let attempts = 4u32;
    let mut last: Option<Error> = None;

    for attempt in 1..=attempts {
        match round_trip().await {
            Ok((typed_sum, explicit_sum)) => {
                println!(
                    "LIVE OK: app.add(2,3).remote() = {typed_sum}; \
                     app.function(\"add\").remote(Input) = {explicit_sum}"
                );
                assert_eq!(typed_sum, 5, "typed app.add(2,3) must compute 5");
                assert_eq!(explicit_sum, 5, "explicit add::Input must compute 5");
                return;
            }
            Err(err) => {
                eprintln!("[auto-io] attempt {attempt}/{attempts} failed: {err}");
                let transient = is_transient(&err);
                last = Some(err);
                if !transient || attempt == attempts {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(3 * attempt as u64)).await;
            }
        }
    }
    panic!(
        "live auto-I/O .remote() failed after {attempts} attempts: {}",
        last.expect("an error was recorded")
    );
}

/// One full round-trip: connect, then exercise BOTH the typed positional method and
/// the explicit generated-input path. Uses `from_inventory` so the macro's
/// `add` registration + decorator config flow into the connected App.
async fn round_trip() -> Result<(i64, i64), Error> {
    // The facade uploads/builds the package named by MODAL_RUST_PACKAGE; point it at
    // the macro crate so the remote runner registers the plain-signature `add`
    // (its generated spread shim). Without this the default `example-add` is built,
    // whose runner has only the struct-form `add` entrypoint. Mirrors `live_gpu.rs`.
    std::env::set_var("MODAL_RUST_PACKAGE", PACKAGE);

    let app = App::connect(APP_NAME).await?;
    // (1) Typed positional sugar: no type named by the caller.
    let typed_sum: i64 = app.add(2, 3).remote().await?;
    // (2) Explicit generated-input path through the string-keyed API.
    let explicit_sum: i64 = app
        .function("add")
        .remote(add::Input { a: 2, b: 3 })
        .await?;
    Ok((typed_sum, explicit_sum))
}
