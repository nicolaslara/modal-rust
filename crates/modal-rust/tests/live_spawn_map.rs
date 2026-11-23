//! Live, best-effort `.spawn()`/`.get()` + `.map()` proof — fire-and-forget and
//! fan-out over REAL Modal, built from the user's REAL Rust `add` (NOT an echo) via
//! `cargo build` IN THE FUNCTION BODY (the run boundary), reusing the SAME wrapper
//! the `.remote()` path proves.
//!
//! - `spawn` enqueues ONE input and returns a `FunctionCall` handle IMMEDIATELY;
//!   `get` polls that call's single output later → `42`.
//! - `map` fans out N inputs under one map call and collects the N outputs in INPUT
//!   ORDER (the idx reassembly), running across containers in parallel.
//!
//! Gated behind BOTH the `live` cargo feature AND `#[ignore]` so the no-CUDA CI box
//! never runs it. Run locally with:
//!
//! ```text
//! cargo test -p modal-rust --features live --test live_spawn_map \
//!     -- --ignored --nocapture
//! ```
//!
//! Modal flakiness ("socket connection closed unexpectedly", build capacity blips)
//! is transient — retry, never block. The hard gates are the offline compiles.

#![cfg(feature = "live")]

use std::time::Duration;

use example_add_macro::add;
use modal_rust::{App, Error};

const SPAWN_APP: &str = "modal-rust-live-spawn";
const MAP_APP: &str = "modal-rust-live-map";
/// The cargo package the facade uploads and builds in the function body. `add` is
/// registered by `example-add-macro`; the facade's default package ships no
/// `modal_runner`, so the package must be named.
const PACKAGE: &str = "example-add-macro";

/// Treat transport blips and known transient gRPC messages as retryable. Delegates
/// to the SDK's own classifier so the test and the SDK agree on what is transient.
fn is_transient(err: &Error) -> bool {
    match err {
        Error::Sdk(sdk_err) => sdk_err.is_transient(),
        _ => false,
    }
}

/// Run `attempt_fn` up to `attempts` times, retrying only on transient errors.
async fn retry<T, F, Fut>(label: &str, attempts: u32, mut attempt_fn: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, Error>>,
{
    let mut last: Option<Error> = None;
    for attempt in 1..=attempts {
        match attempt_fn().await {
            Ok(v) => return v,
            Err(err) => {
                eprintln!("[{label}] attempt {attempt}/{attempts} failed: {err}");
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
        "live {label} failed after {attempts} attempts: {}",
        last.expect("an error was recorded")
    );
}

#[tokio::test]
#[ignore = "live Modal .spawn()/.get() round-trip; run with --features live -- --ignored"]
async fn spawn_then_get_returns_42() {
    let out: i64 = retry("spawn->get", 4, spawn_get_round_trip).await;
    println!("LIVE OK: add(40,2).spawn().get() = {out:?}");
    assert_eq!(out, 42, "real Rust add must compute 42, not echo");
}

async fn spawn_get_round_trip() -> Result<i64, Error> {
    std::env::set_var("MODAL_RUST_PACKAGE", PACKAGE);
    let app = App::connect(SPAWN_APP).await?;
    let fc = app
        .function("add")
        .spawn(add::Input { a: 40, b: 2 })
        .await?;
    println!("[spawn->get] function_call_id = {}", fc.function_call_id());
    // The spawned input pays the cold in-body `cargo build`; the default `get`
    // deadline (wrapper timeout + buffer) covers it.
    fc.get(None).await
}

#[tokio::test]
#[ignore = "live Modal .map() fan-out (ordered); run with --features live -- --ignored"]
async fn map_returns_outputs_in_input_order() {
    let sums: Vec<i64> = retry("map", 4, map_round_trip).await;
    println!("LIVE OK: add.map([1+1, 2+2, 3+3, 40+2]) = {sums:?}");
    // Proves INPUT ORDER (not completion order) — the idx reassembly. The last
    // input's much larger sum (42) is a distinct value, so a misordered collect
    // would not coincidentally match.
    assert_eq!(sums, vec![2, 4, 6, 42], "map must preserve input order");
}

async fn map_round_trip() -> Result<Vec<i64>, Error> {
    std::env::set_var("MODAL_RUST_PACKAGE", PACKAGE);
    let app = App::connect(MAP_APP).await?;
    let inputs = vec![
        add::Input { a: 1, b: 1 },
        add::Input { a: 2, b: 2 },
        add::Input { a: 3, b: 3 },
        add::Input { a: 40, b: 2 },
    ];
    app.function("add").map(inputs).await
}
