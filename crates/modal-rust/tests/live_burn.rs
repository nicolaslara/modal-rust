//! Live, best-effort BURN/GPU CAPSTONE proof — a REAL Burn/CubeCL tensor op on a
//! CUDA GPU on Modal, through the facade.
//!
//! This is the HEAVIEST live proof in the project. Unlike the cudarc `vector_add`
//! (Tier 0, driver-only, a precompiled PTX kernel through the Driver API), `burn_add`
//! drives **CubeCL**, which JIT-compiles its kernels via NVRTC at runtime — making it
//! the project's first **Tier 1** workload (boundaries.md §9): `libnvrtc` + `libcudart`
//! MUST be on the loader path, and the CUDA **headers** must be on disk (CubeCL passes
//! `--include-path=$CUDA_PATH/include` to NVRTC). The runtime image is therefore a
//! `nvidia/cuda:<ver>-devel` base + the Rust toolchain (rustup) + python + the wrapper.
//!
//! ## What this drives end-to-end
//!   `#[modal_rust::function(gpu = "T4", name = "burn_add")]` (the DECORATED STUB in
//!   THIS test binary) → `App::connect` captures `FunctionConfig { gpu: Some("T4"), .. }`
//!   under the entrypoint name `burn_add` → `app.deploy_with(cfg)` with the CUDA-devel
//!   base + `install_rust = true` → the deploy BASE layer bakes rust + the CUDA env,
//!   the TOP layer COPYs the `example-burn-add` source and runs `cargo build --release
//!   -p example-burn-add --bin modal_runner` AT IMAGE-BUILD time → Modal schedules the
//!   deployed wrapper on a **T4** (the decorator gpu rode into `Resources.gpu_config`)
//!   → `app.call(..)` execs the prebuilt `/app/modal_runner`, which registers the REAL
//!   `burn_add` via its own `modal_registry()` and runs a CubeCL CUDA kernel.
//!
//! The LOCAL `burn_add` stub below is NEVER executed remotely — `call` dispatches by
//! entrypoint NAME against the uploaded burn crate's runner (which registers the REAL
//! `burn_add`). The stub exists only so the decorator records `gpu="T4"` and the name
//! matches the uploaded entrypoint.
//!
//! ## Build-cost strategy: DEPLOY (build once)
//! The CUDA image pull + rustup + cubecl/burn compile is MANY minutes. The DEPLOY
//! path builds ONCE at image-build time (Modal caches the image by hash, so subsequent
//! calls are fast); the STABLE app name re-deploys in place; a cheap T4 is used. (The
//! RUN path rebuilds in-body on every cold container — slower; deploy is primary.)
//!
//! Gated behind BOTH the `live` cargo feature AND `#[ignore]` so the no-CUDA CI box
//! never compiles or runs it. Run locally with:
//!
//! ```text
//! cargo test -p modal-rust --features live --test live_burn \
//!     -- --ignored --nocapture
//! ```
//!
//! Modal flakiness (transport blips, build/GPU capacity) is transient — retry, drive
//! to a terminal result, be PATIENT (the CUDA+burn build takes many minutes). The hard
//! gates are the offline compiles.

#![cfg(feature = "live")]

use std::time::Duration;

// The facade re-exports the proc-macro as `modal_rust::function`, so the attribute is
// spelled exactly as a user would. The macro's emitted code references
// `::modal_rust_runtime` / `::inventory` by their real crate names — both are in this
// test binary's extern prelude (a normal dep + a dev-dep of `modal-rust`).
use modal_rust::{App, DeployConfig, Error};
use serde::{Deserialize, Serialize};

/// STABLE deploy app name: re-deploys REPLACE this app in place (no accumulation), so
/// the heavy CUDA image is built once and cached by hash.
const DEPLOY_APP: &str = "modal-rust-burn-deploy";
/// A separate ephemeral connect name for the client (the deploy publishes under
/// `DEPLOY_APP` regardless; this is just the connection's throwaway app).
const CONNECT_APP: &str = "modal-rust-live-burn-driver";
/// The burn crate to upload + build remotely (its runner registers the REAL `burn_add`).
const PACKAGE: &str = "example-burn-add";
/// The CUDA-devel base (headers needed for CubeCL's runtime NVRTC). Proven in M13 /
/// `gpu_app.py`. Escalation to a `13.x-devel-ubuntu22.04` tag is a one-line change if a
/// future cubecl/cudarc bump introduces a CUDA-13-only symbol (NOT expected for the
/// current pin set: cudarc 0.19 dynamic-loading links with NO CUDA at build time).
const CUDA_BASE: &str = "nvidia/cuda:12.6.3-devel-ubuntu22.04";

/// The single named-JSON-object input for `burn_add` (mirrors
/// `example_burn_add::BurnAddInput`). Derives both serde traits so the `typed!` wrapper
/// the macro emits type-checks (`In: DeserializeOwned`) as well as the outbound
/// serialize path used by `call`.
#[derive(Debug, Serialize, Deserialize)]
struct BurnAddIn {
    /// Number of elements in each tensor.
    n: usize,
}

/// The decoded output of the REAL remote `burn_add` (mirrors
/// `example_burn_add::BurnAddOutput`). Derives both serde traits: `Serialize` for the
/// outbound `typed!` stub, `Deserialize` to decode the real output.
#[derive(Debug, Serialize, Deserialize)]
struct BurnAddOut {
    /// `true` iff the GPU-computed `c` matched the CPU reference `a + b` element-wise.
    valid: bool,
    /// Elements computed on the GPU.
    n: usize,
    /// Which Burn backend executed the op (proof it ran on CUDA, not CPU).
    backend: String,
    /// Resolved loader path of `libnvrtc` (Tier-1 proof — NVRTC was on the loader path).
    libnvrtc: String,
    /// Resolved loader path of `libcudart` (Tier-1 proof).
    libcudart: String,
    /// A few spot-checked GPU results `(index, gpu_value, cpu_reference)`.
    samples: Vec<(usize, f32, f32)>,
}

/// The DECORATED stub: its only job is to record `FunctionConfig { gpu: Some("T4") }`
/// under the entrypoint name `burn_add` into this test binary's inventory. It is NEVER
/// executed remotely (the uploaded burn crate runs the real kernel), so its body just
/// errors. The `String` error is `Display + Serialize`, satisfying the `typed!` wrapper
/// without pulling in `anyhow`. `name = "burn_add"` MUST match the entrypoint the
/// uploaded runner registers.
#[modal_rust::function(gpu = "T4", name = "burn_add")]
fn burn_add(_input: BurnAddIn) -> Result<BurnAddOut, String> {
    Err("local stub: burn_add runs on Modal (T4), not in-process".to_string())
}

/// Treat transport blips and known transient gRPC messages as retryable. Delegates to
/// the SDK's own classifier so the test and the SDK agree on what is transient.
fn is_transient(err: &Error) -> bool {
    match err {
        Error::Sdk(sdk_err) => sdk_err.is_transient(),
        _ => false,
    }
}

#[tokio::test]
#[ignore = "live Modal CUDA Burn deploy + call on a T4; run with --features live -- --ignored"]
async fn deploy_then_call_burn_add_on_t4_returns_valid() {
    let attempts = 4u32;
    let mut last: Option<Error> = None;

    for attempt in 1..=attempts {
        match deploy_and_call().await {
            Ok(out) => {
                println!(
                    "LIVE BURN OK: burn_add(n={}) valid={} backend={:?} \
                     libnvrtc={:?} libcudart={:?} samples={:?} \
                     (CUDA base {CUDA_BASE}; decorator gpu=\"T4\" -> Resources.gpu_config -> T4)",
                    out.n, out.valid, out.backend, out.libnvrtc, out.libcudart, out.samples
                );
                assert!(
                    out.valid,
                    "GPU Burn result must match the CPU reference element-wise"
                );
                assert!(
                    out.backend.contains("burn-cuda"),
                    "must run on the Burn CUDA backend (not CPU); got backend={:?}",
                    out.backend
                );
                assert!(
                    !out.libnvrtc.is_empty(),
                    "Tier-1 proof: libnvrtc must resolve on the loader path"
                );
                assert!(
                    !out.libcudart.is_empty(),
                    "Tier-1 proof: libcudart must resolve on the loader path"
                );
                return;
            }
            Err(err) => {
                eprintln!("[burn] attempt {attempt}/{attempts} failed: {err}");
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
        "live burn deploy/call failed after {attempts} attempts: {}",
        last.expect("an error was recorded")
    );
}

async fn deploy_and_call() -> Result<BurnAddOut, Error> {
    // `App::connect` builds from THIS binary's inventory, capturing the decorator
    // config: the `burn_add` entrypoint carries `gpu = Some("T4")`, which
    // `deploy_with` threads into `Resources.gpu_config` on the deployed function.
    let app = App::connect(CONNECT_APP).await?;

    // DEPLOY (persistent, build-once): the CUDA-devel base + install_rust ride the
    // DeployConfig (independent of the decorator). The base layer bakes rust + the
    // CUDA env; the top layer runs `cargo build --release -p example-burn-add --bin
    // modal_runner` AT image-build time. The decorated gpu="T4" rides into the
    // deployed function's resources.
    let mut cfg = DeployConfig::for_app(DEPLOY_APP);
    cfg.package = PACKAGE.to_string();
    cfg.base_image = CUDA_BASE.to_string();
    cfg.install_rust = true;
    let deployed = app.deploy_with(cfg).await?;
    println!(
        "deployed '{}' fn={} image={} url={:?}",
        deployed.name, deployed.function_id, deployed.image_id, deployed.url
    );
    assert_eq!(deployed.name, DEPLOY_APP);

    // CALL (no upload, no build): resolve from_name + invoke. The deployed runtime
    // execs the prebuilt /app/modal_runner, which runs the REAL CubeCL CUDA kernel.
    app.call::<_, BurnAddOut>(DEPLOY_APP, "burn_add", BurnAddIn { n: 256 })
        .await
}
