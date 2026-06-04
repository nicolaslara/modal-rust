//! Live, best-effort P6 CARGO BUILD CACHE proof — cold-vs-warm on the RUN path.
//!
//! Drives the facade end-to-end against REAL Modal to prove the archive-as-single-
//! object cargo cache (knowledge.md §C):
//!
//!   1. COLD: a `.remote()` run with cache ON resolves+attaches the V2 volume at
//!      `/cache`, finds NO archive (`[cache] COLD (no archive)`), builds in-body,
//!      then PACKS `/cache/cache.tar.zst` (`[cache] packed`) — committed via
//!      `allow_background_commits` (NO hot-path reload).
//!   2. WARM: a SECOND fresh ephemeral app (=> a fresh container, so warm-container
//!      `_MARKER` reuse can't mask the test) finds + UNPACKS the archive
//!      (`[cache] WARM`), so cargo sees a warm CARGO_HOME and the build is faster.
//!   3. Both runs return the CORRECT result (`{sum:42}`) — a cache miss only costs
//!      time, NEVER changes the result.
//!
//! Client-side wall-clock of each `.remote()` round-trip is the headline timing
//! signal (it includes the in-body build). The `[cache]` markers + the written
//! archive object are inspected out-of-band via `modal volume ls` / `modal app
//! logs` by the driver harness (not asserted here, since the wrapper's stderr is the
//! REMOTE container's, surfaced in Modal logs).
//!
//! Reset the cache volume for a true cold run BEFORE this test:
//!   `modal volume rm -r modal-rust-cargo-cache` (then the SDK recreates it).
//!
//! Gated behind BOTH the `live` cargo feature AND `#[ignore]`. Run with:
//!
//! ```text
//! MODAL_RUST_CACHE_TARGET=1 cargo test -p modal-rust --features live \
//!     --test live_cache -- --ignored --nocapture
//! ```
//!
//! Modal flakiness is transient — retry, never block.

#![cfg(feature = "live")]

use std::time::{Duration, Instant};

use example_add::{modal_registry, AddInput, AddOutput};
use modal_rust::{App, Error};

/// COLD app: first run after a volume reset (no archive => packs one).
const COLD_APP: &str = "modal-rust-live-cache-cold";
/// WARM app: a SEPARATE ephemeral app => a fresh container that unpacks the archive
/// the COLD run packed (so warm-container `_MARKER` reuse can't mask the win).
const WARM_APP: &str = "modal-rust-live-cache-warm";

fn is_transient(err: &Error) -> bool {
    match err {
        Error::Sdk(sdk_err) => sdk_err.is_transient(),
        _ => false,
    }
}

/// One timed `.remote()` round-trip on `app_name`, retrying only transient errors.
async fn timed_remote(label: &str, app_name: &str) -> (AddOutput, Duration) {
    let attempts = 4u32;
    let mut last: Option<Error> = None;
    for attempt in 1..=attempts {
        let started = Instant::now();
        let res = async {
            let app = App::connect_with_registry(app_name, modal_registry()).await?;
            app.function("add").remote(AddInput { a: 40, b: 2 }).await
        }
        .await;
        match res {
            Ok(out) => {
                let elapsed = started.elapsed();
                println!(
                    "[{label}] remote add(40,2) = {out:?} in {:.1}s",
                    elapsed.as_secs_f64()
                );
                return (out, elapsed);
            }
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
        "[{label}] live .remote() failed after {attempts} attempts: {}",
        last.expect("an error was recorded")
    );
}

/// COLD run (cache ON, empty volume): build in-body, pack the archive, return 42.
/// The driver resets the volume BEFORE running this so it is a true cold build.
#[tokio::test]
#[ignore = "live P6 cache COLD run; reset the volume first; run with --features live -- --ignored"]
async fn cache_cold_run_builds_and_packs() {
    let (out, elapsed) = timed_remote("COLD", COLD_APP).await;
    assert_eq!(
        out.sum, 42,
        "cold run must compute the correct result (42), not depend on cache"
    );
    println!("COLD_SECONDS={:.1}", elapsed.as_secs_f64());
}

/// WARM run (cache ON, archive present): a fresh container unpacks the archive,
/// cargo sees a warm CARGO_HOME, build is faster, still returns 42. Run AFTER the
/// cold test (which packed the archive).
#[tokio::test]
#[ignore = "live P6 cache WARM run; run AFTER the cold test; run with --features live -- --ignored"]
async fn cache_warm_run_reuses_archive() {
    let (out, elapsed) = timed_remote("WARM", WARM_APP).await;
    assert_eq!(out.sum, 42, "warm run must compute the correct result (42)");
    println!("WARM_SECONDS={:.1}", elapsed.as_secs_f64());
}

/// OPT-OUT run (`MODAL_RUST_NO_CACHE=1` set by the driver): NO volume is resolved or
/// attached, the wrapper renders `CACHE_ON = False`, and the result is still correct.
/// Proves the opt-out path is miss-safe and attaches no volume.
#[tokio::test]
#[ignore = "live P6 cache OPT-OUT run (MODAL_RUST_NO_CACHE=1); run with --features live -- --ignored"]
async fn cache_opt_out_attaches_no_volume() {
    assert_eq!(
        std::env::var("MODAL_RUST_NO_CACHE").ok().as_deref(),
        Some("1"),
        "this test must run with MODAL_RUST_NO_CACHE=1 so cache is OFF"
    );
    let (out, elapsed) = timed_remote("OPTOUT", "modal-rust-live-cache-optout").await;
    assert_eq!(
        out.sum, 42,
        "opt-out run must still compute the correct result (42)"
    );
    println!("OPTOUT_SECONDS={:.1}", elapsed.as_secs_f64());
}
