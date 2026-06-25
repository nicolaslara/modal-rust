# burn-lm-bench — `#[modal_rust::web_server]` + a real benchmark of burn-lm batching

Wraps the [burn-lm-http](https://github.com/nicolaslara/burn-lm) inference server
(an OpenAI-compatible `POST /v1/chat/completions` API) as a long-running, GPU-backed
Modal **web server** with one attribute, then load-tests the deployed URL and charts
how decode throughput **scales with concurrency** across models.

It is two things at once:

1. The live dogfood of `#[modal_rust::web_server]` — a function that *launches a server
   bound to a port and blocks forever*. On `modal-rust deploy` Modal assigns a public URL
   and proxies ALL traffic to that port, so multi-route apps and SSE streaming work
   unchanged. (`#[web_server]` is DEPLOY-only in v0.)
2. The server-GPU benchmark of **continuous batching**,
   [tracel-ai/burn-lm#57](https://github.com/tracel-ai/burn-lm/pull/57). The pinned rev is
   that PR's `nicolas/batching` head, so this runs the exact code under test. The PR's
   development numbers are on Metal; this is the CUDA measurement the PR says it still
   needs.

Two crates:

- **`example-burn-lm-bench`** (this crate) — the server. A `#[web_server]` `serve()`
  that runs `burn_lm_http::App`. GPU/CUDA-only and its OWN cargo workspace (it consumes
  `modal-rust` as a third-party git dep — see the note below), so bare `cargo`/CI in the
  modal-rust repo never touch it.
- **`burn-lm-bench-client`** (`../burn-lm-bench-client`) — the load tester + plotter.
  Pure HTTP (no burn/modal deps), so it builds in ~2s and runs anywhere.

```rust
use modal_rust::web_server;

#[web_server(
    port = 3000,
    gpu = "T4",
    memory = 16384,
    // CUDA-devel base gives CubeCL the cudart/nvrtc libs at runtime; install_rust adds
    // the toolchain for the in-image cargo build; `apt = ["curl"]` is REQUIRED because
    // burn-lm-http's utoipa-swagger-ui build script downloads Swagger UI with curl and
    // the CUDA base image ships without it.
    image = Image(base = "nvidia/cuda:12.4.1-devel-ubuntu22.04", install_rust = true, apt = ["curl"]),
    startup_timeout = 600,
    // Per-container input concurrency: let ONE replica receive up to 32 inputs at once
    // (target 8, matching the batched KV slab) so burn-lm's continuous batching actually
    // fills. Without this Modal hands the container one input at a time and the effective
    // batch is always 1 (see Notes). `min_containers = 1` keeps the GPU warm;
    // `max_containers = 1` pins to one GPU container so the batching measurement isn't
    // diluted by scale-OUT.
    max_concurrent_inputs = 32,
    target_concurrent_inputs = 8,
    min_containers = 1,
    max_containers = 1
)]
async fn serve(port: u16) -> anyhow::Result<()> {
    // Bind 0.0.0.0 (Modal's proxy reaches the port from outside loopback) and size the
    // batched KV slab: max_slots = 8 lanes, max_seq_len = 2048. (Abridged — the full body,
    // incl. the RUST_LOG batching-observability default, is in src/lib.rs.)
    let config = burn_lm_http::AppConfig { max_slots: Some(8), max_seq_len: Some(2048) };
    burn_lm_http::App::new_with_config(
        std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
        port,
        config,
    )
    .serve()
    .await
    .map_err(|e| anyhow::anyhow!(e.to_string()))
}
```

> **The server must bind `0.0.0.0`, not `127.0.0.1`.** Modal's `@web_server` proxy
> reaches the port from outside the container's loopback, so a `127.0.0.1` bind is
> invisible to it (the container is SIGKILLed after `startup_timeout` with
> "initializing for too long"). This fork of burn-lm-http binds `0.0.0.0`.

> **These two crates are STANDALONE** — each is its own cargo workspace and depends on
> `modal-rust` as a third-party **git** dependency (it is not on crates.io), exactly as an
> outside user would. They are NOT members of the modal-rust repo's workspace, so build/run
> them from their own directories (paths below are from the example dir, not the repo root).

## 0. Install the `modal-rust` CLI

```bash
cargo install --git https://github.com/nicolaslara/modal-rust --package modal-rust-cli
# => `modal-rust` on your PATH. The example's Cargo.toml tracks the same repo (branch=main),
#    so the facade you compile against matches the CLI you deploy with.
```

## 1. Deploy the server

`burn-lm-http` pulls the full `burn` stack, which is too heavy to compile on a typical
laptop (and `--remote-describe`'s in-body describe build OOMs on it). So this example
deploys with a **hand-written describe manifest** (`--manifest`), which skips the local
build and lets Modal compile the crate at **image-build time** (full resources):

```bash
cd examples/burn-lm-bench   # the example is its own workspace; deploy from here

# manifest.json — the per-entrypoint config the macro would otherwise emit:
cat > /tmp/serve-llama.manifest.json <<'JSON'
{"schema":"modal-rust/describe@1","entrypoints":[{"name":"serve","config":{
  "gpu":"T4","memory_mb":16384,"min_containers":1,"max_containers":1,
  "max_concurrent_inputs":32,"target_concurrent_inputs":8,
  "web_server_port":3000,"web_server_startup_timeout":600,
  "image":"{\"base\":\"nvidia/cuda:12.4.1-devel-ubuntu22.04\",\"install_rust\":true,\"apt\":[\"curl\"]}"
}}]}
JSON

modal-rust deploy serve --project . \
  --manifest /tmp/serve-llama.manifest.json --app burn-lm-bench
# => prints the web URL, e.g. https://<workspace>--burn-lm-bench-serve.modal.run
```

Notes:

- **Weights are public.** The Llama-3.x `*-burn` weights are public Hugging Face
  mirrors (`tracel-ai/...`), so **no `hf-token` secret is needed**. (For a gated model,
  add `secrets = ["hf-token"]` to the decorator and `modal secret create hf-token
  HF_TOKEN=hf_...`.)
- **`min_containers = 1`** keeps one GPU container warm so the loaded model stays
  resident across requests — essential for a clean sweep (otherwise the container
  scales down after ~60s idle and the next request pays a full model reload).
  **Stop the app (`modal app stop burn-lm-bench`) when done** to stop GPU billing.
- **`max_concurrent_inputs = 32` / `target_concurrent_inputs = 8` — per-container input
  concurrency, NOT scale-out.** Modal defaults a container to **1** concurrent input
  (`Function.max_concurrent_inputs` unset ⇒ `or 1` at runtime), so even with 32 requests
  in flight the load generator gets them served one at a time — that serialized the load
  test and HID burn-lm's continuous batching (the effective batch was always 1). Setting
  `max_concurrent_inputs` lets a single replica receive many inputs at once so the batched
  forward pass actually fills; `target_concurrent_inputs = 8` matches the batched KV slab.
  This is distinct from `max_containers` (scale-OUT: number of replicas) — `max_containers
  = 1` pins everything to one GPU so concurrent requests share one batched forward pass,
  which is exactly what we measure. modal-rust accepts both keys on the `#[web_server]`
  attr AND in the `--manifest` config object (the keys above).
- **Aggregate throughput caps at `max_slots`.** burn-lm-http's *own* server default is
  `max_slots = 4`. This example raises it to **8** (via `App::new_with_config` in `lib.rs`,
  matching `target_concurrent_inputs = 8`), so the server batches up to 8 sequences in one
  fused decode; aggregate throughput scales up to ~`max_slots` and then flattens. Going
  beyond 8 needs more VRAM (a T4 holds ~8 f32 KV lanes) — that, or making `max_slots`
  deployment-tunable, is a burn-lm SERVER concern (`BURN_LM_MAX_SLOTS` env overrides it
  without a rebuild), separate from this modal-rust change.
- For the 8B model, deploy with `"gpu":"A100-80GB"` (it does not fit a T4 in f32 — see
  *GPU sizing* below).
- `/v1/models` only lists models whose weights are already downloaded; the others are
  still reachable — `POST /v1/chat/completions` downloads on demand on first use.
- Model ids are the human names, e.g. `Llama 3.2 (1B Instruct)`, `Llama 3.2 (3B
  Instruct)`, `Llama 3.1 (8B Instruct)` (matched case-insensitively).

## 2. Load-test it

The load tester is the separate, standalone `burn-lm-bench-client` crate (pure HTTP, no
burn/modal deps — builds in ~2s). Run it from its own directory:

```bash
cd examples/burn-lm-bench-client   # its own workspace
cargo run --bin bench -- \
  --url https://<workspace>--burn-lm-bench-serve.modal.run \
  --model "Llama 3.2 (1B Instruct)" \
  --concurrency 8 --requests 32 --max-tokens 128
# => === burn-lm-bench summary === total/ok/failed, requests/s, tokens/s,
#    p50/p95 e2e + ttft, and per-request decode tok/s
```

## 3. Sweep + scaling charts (pure Rust, no Python/bash)

`--sweep` runs a model × concurrency sweep with the same binary, writes `results.csv`,
and renders **interactive HTML** charts with [`plotly`](https://crates.io/crates/plotly)
(self-contained — open in any browser to zoom/hover/toggle series):

- `throughput.html` — **aggregate system tokens/s** vs concurrency (the scaling curve; the headline).
- `speedup.html` — speedup vs the lowest level, plus a dashed `ideal (linear)` line.
- `latency.html` — p95 end-to-end latency (ms) vs concurrency (where the knee is).
- `ttft.html` — p95 time-to-first-token vs concurrency (where the prefill tail shows up).
- `decode.html` — p50 per-request decode tok/s (excludes TTFT; stays flat if decode is healthy).

```bash
# from examples/burn-lm-bench-client (see above):
cargo run --bin bench -- --sweep \
  --url https://<workspace>--burn-lm-bench-serve.modal.run \
  --models "Llama 3.2 (1B Instruct),Llama 3.2 (3B Instruct)" \
  --levels "1,2,4,8" \
  --rounds 4 --max-tokens 128 \
  --out-dir .modal-rust/burn-lm-bench-runs/t4
```

- `--out-dir` defaults to `.modal-rust/burn-lm-bench-runs/`; the client crate's
  `.gitignore` excludes `.modal-rust/`, so run outputs (`results.csv`, `results.jsonl`,
  the HTML charts) never land in a committed path. Point it at a per-GPU subdir (e.g.
  `.../t4`, `.../a100`) to keep sweeps side by side. (Keep large chart HTML out of the
  *server* crate dir — it would bloat that crate's `modal-rust deploy` source upload.)
- Each point sends `(concurrency * --rounds).max(2 * --rounds)` requests. Lower
  `--rounds` for big/slow models so each point finishes in reasonable wall time.
- Longer `--max-tokens` emphasises decode **batching** over prefill.
- Each model is **warmed up** (one untimed streaming request) before its timed points,
  and each concurrency level pays its one-time batch-kernel JIT compile in an untimed
  `warm_level` pass — so neither the model-load nor the kernel-compile cost lands in the
  measured numbers.

**Crash-resilient & resumable.** Every completed point is appended+flushed to
`results.csv` immediately, and every request is salvaged to `results.jsonl`. Re-running
the same command **skips points already in `results.csv`** and retries any point that
had zero successes — so a crash mid-sweep never loses prior work. Re-chart partial
results without re-running via `--plot-only --out-dir ./bench-out`.

`results.csv` columns:
`model,concurrency,tokens_per_s,speedup,requests_per_s,e2e_ms_p95,ttft_ms_p95,failure_rate,total,ok,failed,wall_s,aborted,ttft_ms_p50,decode_tok_s_p50`
(`speedup` is left blank and computed at render time from the lowest level, so it stays
correct across resumed runs.)

## Reading the numbers (continuous batching has three different "tok/s")

Continuous batching advances all in-flight sequences together through one batched
forward per *round*, but it admits **at most one prompt prefill per round**
(`PrefillBudget`). Interleaving a new prompt's prefill into the shared decode round adds
a one-round latency bump to the lanes already decoding — so under bursty closed-loop
load, where several requests complete and several new ones are admitted around the same
time, you get a **straggler tail**: a few requests wait through preceding admissions and
report a high TTFT (and, if you measured it end-to-end, a misleadingly low tok/s). This
is intrinsic to one-prefill-per-round scheduling; the admission policy bounds *where* the
cost lands but cannot remove it. The real fix — **chunked / mixed-batch prefill** — is a
deliberate follow-up in PR #57. None of it is a `modal-rust` / `#[web_server]` issue.

So report three numbers, and don't conflate them:

1. **System throughput** = total output tokens ÷ wall-clock (`tokens_per_s`). A request
   stuck waiting for its prefill round produces no tokens, but the GPU is busy with
   *others'* decode during that wait, so this stays honest. **This is the headline scaling
   metric** (`throughput.html` / `speedup.html`) and the number comparable to PR #57's
   batch-1/2/4/8 table.
2. **Per-request decode rate** = (out_tokens − 1) ÷ (e2e − ttft) (`decode_tok_s_p50`).
   Excludes the queue + prefill wait, so it isolates decode health from the *scheduling*
   tail. It stays roughly flat when the GPU has compute headroom; on a **compute/bandwidth-
   bound T4 it declines** as the batch grows (15 → 6 here) — that decline is the device
   saturating under the fused decode (roofline), which is *expected*, not the scheduling
   tail. A bigger GPU keeps it flatter (`decode.html`).
3. **TTFT** (p50/p95, `ttft.html`) is where the straggler tail is visible and honest: p95
   climbs with concurrency. Report it explicitly rather than letting it silently drag a
   per-request end-to-end rate. **Never headline `out_tokens ÷ e2e`** as "throughput" — if
   shown at all, label it "user-perceived rate" next to TTFT.

Always use **percentiles + the aggregate**, never a per-request *mean* tok/s (the tail
would distort the mean).

## Results (CUDA)

> Numbers are aggregate system throughput (`tokens_per_s`) per concurrency level; each
> model is warmed. Tokens/s, with decode-rate (p50) and TTFT-p95 alongside.

**T4 (16 GB), f32, `--rounds 6 --max-tokens 128`:**

| model | c=1 | c=2 | c=4 | c=8 | speedup@8 | decode tok/s p50 (c1→c8) | ttft p95 @8 |
|---|---:|---:|---:|---:|---:|---:|---:|
| Llama 3.2 (1B Instruct) | 14.7 | 26.1 | 38.7 | 47.1 | **3.2×** | 15.1 → 6.0 | 0.51 s |
| Llama 3.2 (3B Instruct) | — | — | — | — | — | — | OOMs on T4 (8-lane f32 KV slab); needs A100-80GB |

All 1B points ran at **0% failures**. Aggregate tok/s scales ~3.2× through c=8 (the
`max_slots = 8` batching ceiling); per-request decode rate **declines** (15 → 6) because a
T4 is compute/bandwidth-bound once the fused decode batches (expected — see *Reading the
numbers*). The lever is real: with Modal's **default `max_concurrent_inputs = 1`** the same
sweep is FLAT — `9.4 → 11.1 → 13.3 → 14.1` tok/s (~1.5× at c=8) with TTFT p95 ballooning to
~69 s as inputs queue. Raising `max_concurrent_inputs` is what unlocks the 3.2× (see Notes).

**A100-80GB, f32** — _not yet run._ Needed for 3B/8B (8B at f32 ≈ 32 GB doesn't fit a T4);
deploy with `"gpu":"A100-80GB"` in the manifest and re-run the sweep.

## Caveats

- **GPU sizing (f32).** `burn-lm-inference` is built with the `f32` element type, so
  weights are 4 bytes/param. Only **1B (≈ 5 GB weights)** fits a **T4 (16 GB)** alongside
  the 8-lane KV slab. **3B (≈ 12 GB)** plus the 8-lane f32 slab **OOMs on a T4** in
  practice — it (and 8B, ≈ 32 GB) needs an **A100-80GB** (`"gpu":"A100-80GB"` in the
  manifest). The non-quantized Llamas use burn-lm's continuous-batching channel (what makes
  decode throughput scale with concurrency); the Q4 model is single-stream by design and
  not part of the sweep.
- **CUDA-only.** This crate pulls `burn-lm-inference` with the `cuda` backend
  (CubeCL/cudarc dynamic loading): it *compiles* without a CUDA toolkit but only *runs*
  on an NVIDIA GPU. It is excluded from `default-members` so bare `cargo`/CI stay green.
