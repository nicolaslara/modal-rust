//! Live, best-effort `.remote()` proof — the payoff for the RUN path.
//!
//! Drives the facade end-to-end against REAL Modal: `App::connect` →
//! `app.function("add").remote(add::Input{a:40,b:2})` → `42`, built
//! from the user's REAL Rust `add` (NOT an echo) via `cargo build` IN THE FUNCTION
//! BODY (the run boundary). NO modal CLI, NO per-project `.py`.
//!
//! Gated behind BOTH the `live` cargo feature AND `#[ignore]` so the no-CUDA CI box
//! never runs it. Run locally with:
//!
//! ```text
//! cargo test -p modal-rust --features live --test live_remote \
//!     -- --ignored --nocapture
//! ```
//!
//! Modal flakiness ("socket connection closed unexpectedly", build capacity blips)
//! is transient — retry, never block. The hard gates are the offline compiles.

#![cfg(feature = "live")]

use std::time::Duration;

use example_add_macro::add;
use modal_rust::{App, Error};

const APP_NAME: &str = "modal-rust-live-remote";
/// The cargo package the facade uploads and builds in the function body. `add` is
/// registered by `example-add-macro`; the facade's default package ships no
/// `modal_runner`, so the package must be named.
const PACKAGE: &str = "example-add-macro";

/// Treat transport blips and known transient gRPC messages as retryable. Delegates
/// to the SDK's own classifier (which inspects the tonic `Code` + message) so the
/// test and the SDK agree on what is transient.
fn is_transient(err: &Error) -> bool {
    match err {
        Error::Sdk(sdk_err) => sdk_err.is_transient(),
        _ => false,
    }
}

#[tokio::test]
#[ignore = "live Modal .remote() round-trip; run with --features live -- --ignored"]
async fn remote_real_add_returns_42() {
    let attempts = 4u32;
    let mut last: Option<Error> = None;

    for attempt in 1..=attempts {
        match round_trip().await {
            Ok(out) => {
                println!("LIVE OK: add(40,2).remote() = {out:?}");
                assert_eq!(out, 42, "real Rust add must compute 42, not echo");
                return;
            }
            Err(err) => {
                eprintln!("[remote] attempt {attempt}/{attempts} failed: {err}");
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
        "live .remote() failed after {attempts} attempts: {}",
        last.expect("an error was recorded")
    );
}

async fn round_trip() -> Result<i64, Error> {
    std::env::set_var("MODAL_RUST_PACKAGE", PACKAGE);
    let app = App::connect(APP_NAME).await?;
    app.function("add").remote(add::Input { a: 40, b: 2 }).await
}
