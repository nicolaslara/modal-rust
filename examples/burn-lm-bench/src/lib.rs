//! `examples/burn-lm-bench` — the live dogfood of `#[modal_rust::web_server]`, and a
//! real server-GPU benchmark of burn-lm **continuous batching**
//! ([tracel-ai/burn-lm#57](https://github.com/tracel-ai/burn-lm/pull/57)).
//!
//! Teaches ONE modal-rust concept: `#[modal_rust::web_server(port = ..)]` wraps a
//! function that LAUNCHES a long-running HTTP server bound to `port` and BLOCKS forever.
//! Unlike `#[endpoint]` (a request/response `fn(In) -> Out` that Modal wraps in FastAPI),
//! `#[web_server]` is a RAW PORT PROXY: on `modal-rust deploy` Modal assigns a public URL
//! and forwards ALL traffic to the bound port — so multi-route apps and SSE streaming work
//! as-is. v0 is DEPLOY-only (the URL is assigned at deploy time).
//!
//! Here we wrap [`burn_lm_http::App`], an OpenAI-compatible inference server
//! (`POST /v1/chat/completions`), and run it on a GPU. PR #57 makes a model serve many
//! concurrent requests in one batched forward pass (continuous batching), so the
//! interesting measurement is how decode throughput scales with concurrency. The
//! separate `burn-lm-bench-client` crate is a standalone HTTP load tester that drives the
//! deployed URL and renders the throughput / TTFT / latency charts.
//!
//! GPU/CUDA-only — see `Cargo.toml`; this crate is excluded from `default-members`.

use modal_rust::web_server;

/// Launch the burn-lm-http inference server on `port` and serve forever.
///
/// `#[web_server]` config:
/// - `gpu = "T4"` — burn-lm runs CUDA inference (the `cuda` backend on
///   `burn-lm-inference`). A T4 fits the f32 1B/3B Llamas; the 8B needs an A100-80GB.
/// - `memory = 16384` — model weights + the batched KV slab need headroom.
/// - `image = Image(base = "nvidia/cuda:12.4.1-devel-ubuntu22.04", install_rust = true,
///   apt = ["curl"])` — a CUDA-devel base gives CubeCL the NVRTC/cudart libs at runtime;
///   `install_rust` adds the toolchain so the in-image `cargo build` (deploy path) has
///   `cargo`; `curl` is required because burn-lm-http's utoipa-swagger-ui build script
///   downloads Swagger UI with it and the CUDA base image ships without it.
/// - `startup_timeout = 600` — the first cold start downloads weights + JIT-compiles CUDA
///   kernels, which can take minutes; give Modal a generous window for the port to come up
///   before it considers the server failed.
/// - `max_concurrent_inputs = 32` / `target_concurrent_inputs = 8` — PER-CONTAINER input
///   concurrency. Modal defaults a container to **1** concurrent input
///   (`Function.max_concurrent_inputs` unset ⇒ `or 1` at runtime), which SERIALIZES the
///   load test and hides burn-lm's continuous batching — the load generator can have 32
///   requests in flight but Modal hands the container one at a time, so the effective
///   batch is always 1. Setting these lets a single replica receive up to 32 inputs at
///   once (target 8, matching the batched KV slab) so the batched forward pass actually
///   fills. This is distinct from `max_containers` (scale-OUT, below).
/// - `max_containers = 1` — pin to ONE GPU container so the batching measurement isn't
///   diluted by scale-out: every concurrent request lands on the same replica and shares
///   one batched forward pass, which is exactly what we want to measure.
///
/// The Llama-3.x `*-burn` weights are public Hugging Face mirrors (`tracel-ai/...`), so no
/// `hf-token` secret is needed. `POST /v1/chat/completions` downloads the requested model
/// on first use. (For a gated model, add `secrets = ["hf-token"]` here and
/// `modal secret create hf-token HF_TOKEN=hf_...`.)
///
/// NOTE on aggregate throughput: burn-lm-http's own server default is `max_slots = 4`.
/// This example raises it to 8 below (via `App::new_with_config`, matching
/// `target_concurrent_inputs = 8`), so the server batches up to 8 sequences in one fused
/// decode — aggregate throughput scales up to ~`max_slots` and then flattens. Going beyond
/// 8 needs more VRAM (a T4 holds ~8 f32 KV lanes); `BURN_LM_MAX_SLOTS` overrides it without
/// a rebuild.
#[web_server(
    port = 3000,
    gpu = "T4",
    memory = 16384,
    image = Image(base = "nvidia/cuda:12.4.1-devel-ubuntu22.04", install_rust = true, apt = ["curl"]),
    startup_timeout = 600,
    max_concurrent_inputs = 32,
    target_concurrent_inputs = 8,
    min_containers = 1,
    max_containers = 1
)]
async fn serve(port: u16) -> anyhow::Result<()> {
    // Turn the batching logs on by default so the deployed container is observable without
    // a redeploy: `effective batching capacity max_slots=N` (once at load), `fused decode
    // parallelizing decode_width=` (info), and per-round `decode_width/decoding_lanes/
    // prefill_candidates` + `admitted/retired in_flight=` (debug). MUST be set BEFORE the
    // App constructor — `trace::init()` runs inside `App::new` via
    // `EnvFilter::from_default_env()` — and only as a default: a Modal env var wins.
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "info,batching=debug");
    }

    // Size the batched KV slab. `new_with_config` only sets these if the env var is unset,
    // so the manifest `env` (BURN_LM_MAX_SLOTS / BURN_LM_MAX_SEQ_LEN) still overrides without
    // a rebuild. max_seq_len=2048 is set EXPLICITLY: the compiled default is 8192, which would
    // make each of the 8 lanes reserve a 4x-larger context.
    let config = burn_lm_http::AppConfig {
        max_slots: Some(8),
        max_seq_len: Some(2048),
    };

    // Bind 0.0.0.0 (not 127.0.0.1) — Modal's web_server proxy reaches the port from outside
    // the container loopback.
    burn_lm_http::App::new_with_config(
        std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
        port,
        config,
    )
    .serve()
    .await
    .map_err(|e| anyhow::anyhow!(e.to_string()))
}
