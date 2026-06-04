//! Live, best-effort DEPLOY-path proof — the payoff for the deploy boundary.
//!
//! Drives the facade end-to-end against REAL Modal:
//!   `App::connect` → `app.deploy()` (source COPYed into an image LAYER; `cargo
//!   build --release` runs AT IMAGE-BUILD time) → `app.call("modal-rust-add-deploy",
//!   "add", AddInput{40,2})` → `AddOutput{sum:42}` — the deployed runtime execs ONLY
//!   the prebuilt `/app/modal_runner` (NO cargo, NO source mount, NO build at call
//!   time). Built from the user's REAL Rust `add` (NOT an echo).
//!
//! Uses a STABLE app name so re-runs REPLACE the deploy (no accumulation) and the
//! image is the FIXED one (no crash-loop). NO modal CLI, NO per-project `.py`.
//!
//! Gated behind BOTH the `live` cargo feature AND `#[ignore]` so the no-CUDA CI box
//! never runs it. Run locally with:
//!
//! ```text
//! cargo test -p modal-rust --features live --test live_deploy \
//!     -- --ignored --nocapture
//! ```
//!
//! Modal flakiness ("socket connection closed unexpectedly", build capacity blips)
//! is transient — retry, never block. The hard gates are the offline compiles.

#![cfg(feature = "live")]

use std::time::Duration;

use example_add::{modal_registry, AddInput, AddOutput};
use modal_rust::{App, DeployConfig, Error};

/// STABLE deploy app name: re-runs REPLACE this deploy (no accumulation), and the
/// image is the FIXED one (no crash-loop). Must match `DEFAULT_DEPLOY_APP`.
const DEPLOY_APP: &str = "modal-rust-add-deploy";
/// A separate ephemeral connect name for the client (the deploy publishes under
/// `DEPLOY_APP` regardless; this is just the connection's throwaway app).
const CONNECT_APP: &str = "modal-rust-live-deploy-driver";

/// Treat transport blips and known transient gRPC messages as retryable.
fn is_transient(err: &Error) -> bool {
    match err {
        Error::Sdk(sdk_err) => sdk_err.is_transient(),
        _ => false,
    }
}

#[tokio::test]
#[ignore = "live Modal deploy + call round-trip; run with --features live -- --ignored"]
async fn deploy_then_call_real_add_returns_42() {
    let attempts = 4u32;
    let mut last: Option<Error> = None;

    for attempt in 1..=attempts {
        match deploy_and_call().await {
            Ok(out) => {
                println!("LIVE OK: deploy + call('add', 40, 2) = {out:?}");
                assert_eq!(out.sum, 42, "real Rust add must compute 42, not echo");
                return;
            }
            Err(err) => {
                eprintln!("[deploy] attempt {attempt}/{attempts} failed: {err}");
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
        "live deploy/call failed after {attempts} attempts: {}",
        last.expect("an error was recorded")
    );
}

async fn deploy_and_call() -> Result<AddOutput, Error> {
    let app = App::connect_with_registry(CONNECT_APP, modal_registry()).await?;

    // 1. DEPLOY (persistent): build the deploy image (cargo build AT image-build
    //    time — the build logs stream the `Compiling`/`cargo build --release`
    //    lines), create the FILE-mode function with the client mount ONLY, and
    //    publish persistently under the STABLE name.
    let deployed = app.deploy_with(DeployConfig::for_app(DEPLOY_APP)).await?;
    println!(
        "deployed '{}' fn={} image={} url={:?}",
        deployed.name, deployed.function_id, deployed.image_id, deployed.url
    );
    assert_eq!(deployed.name, DEPLOY_APP);

    // 2. CALL (no upload, no build): resolve from_name + invoke. The deployed
    //    runtime execs the prebuilt /app/modal_runner; cargo never runs here.
    app.call::<_, AddOutput>(DEPLOY_APP, "add", AddInput { a: 40, b: 2 })
        .await
}
