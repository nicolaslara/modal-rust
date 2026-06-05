# modal-rust benchmarks

A reproducible **A/B benchmark suite** that measures `modal-rust` against the
official **Modal Python** SDK on the same workloads, against the same live Modal
account. Every scenario has a `modal-rust` side and a hand-written Modal-Python
side so the numbers are a true apples-to-apples comparison, not a one-sided demo.

> [!NOTE]
> **This directory is a PLAN + scaffold.** Only this README, a `scenarios/`
> layout, and a `results/` directory exist today. The runnable bench harness and
> the per-scenario crates/scripts are a tracked follow-up — see
> [`../workpads/benchmarks/tasks.md`](../workpads/benchmarks/tasks.md). Nothing
> here is a compiling crate or a Cargo workspace member yet; the main build stays
> green.

## Why a separate `benchmarks/` directory

`examples/` is documentation: small, clean, easy to read end-to-end. Benchmarks
are the opposite — they carry timing harnesses, repetition loops, a Python
counterpart per scenario, and result artifacts. Keeping them out of `examples/`
keeps the examples pristine.

Where a benchmark scenario needs the *exact* Rust workload an example already
defines (e.g. `add`, `cuda-vector-add`, `burn-add`), it **reuses or symlinks** the
example crate rather than copying it, so the benchmark and the documented example
never drift. The Python counterpart is written once per scenario and lives beside
it under `scenarios/<name>/python/`.

## What this measures (and what it cannot)

This is a **systems / developer-velocity** benchmark, not a raw-compute
benchmark. Both sides ultimately run on Modal's infrastructure, so we are *not*
claiming Rust-vs-Python execution speed in general. We measure the parts the two
SDKs actually own and where they differ:

1. **Build / iteration model** — `modal-rust`'s defining trait is that `.remote()`
   compiles the user's Rust *in the function body* at invoke time, with a build
   cache on by default; Python has no compile step. The honest comparison is
   "cold first remote call" and "warm subsequent remote call" for `modal-rust`
   versus Python's deploy/start, plus how each behaves as the loop repeats.
2. **Control-plane round-trip** — how long the SDK itself takes to create a
   function, enqueue an input, and decode a result, holding the remote work near
   zero. This isolates each client's gRPC/auth/serialization overhead.
3. **Upload size & time** — what each SDK ships to Modal to make a function
   runnable (Rust source closure vs Python module + mounts).
4. **Fan-out throughput** — `.map()` / `.spawn()` vs `Function.map()` /
   `.spawn()` at N inputs.
5. **Cold-start latency** — time from invoke to first user-code execution on a
   freshly scheduled container, once the image is built/warm.

Each is reported as a paired A/B with the methodology fixed below.

## The honest framing of the headline number

The most important, and most easily abused, comparison is **build/iteration**.
State it carefully:

- A `modal-rust` **cold** `.remote()` pays a Rust compile in the function body
  (already measured live at ~6.45 s of `cargo build` for the trivial `add`
  crate, dominated by image build on a truly cold image; a heavy crate like
  `burn-add` compiled in ~1m16s at image-build time during the capstone deploy).
- A `modal-rust` **warm** `.remote()` hits the cache and the build collapses to a
  `Fresh` no-op (measured live at **0.06 s** of `cargo build`; end-to-end
  ~32.9 s → ~9.3 s, ≈3.5× on the trivial `add`).
- Modal **Python** has no compile, so its "cold" is image pull + container start
  + import, and its "warm" is just container start + import.

So the fair claims are: (a) Python wins the very first cold call on a brand-new
image because Rust must compile; (b) the modal-rust **warm** loop is competitive
because the cache turns the rebuild into a no-op; (c) modal-rust then delivers a
*compiled, typed* function for the steady state. We report all three columns and
never collapse them into a single "Rust is faster/slower" line.

## A/B scenarios

Each scenario is `scenarios/<name>/` with a `rust/` side, a `python/` side, and a
`scenario.md` describing the exact metric, the variable under test, and what is
held constant. The Rust workload is reused/symlinked from `examples/` wherever
one already exists.

| # | Scenario (`scenarios/<name>`) | Metric | modal-rust side | Modal Python equivalent | Reuses |
| --- | --- | --- | --- | --- | --- |
| B1 | `cold-vs-warm-build` | In-body build time (cold vs warm) + end-to-end `.remote()` | `add.remote()` cold (cache empty) then warm (cache hit) | `@app.function` `.remote()` cold (image build) then warm | `examples/add` |
| B2 | `deploy-image-build` | Image-build / deploy wall-clock | `App::deploy_with` (cargo at image-build time) | `modal deploy` / `app.deploy()` with `run_commands` | `examples/add` |
| B3 | `invoke-roundtrip` | Per-call round-trip latency, trivial body | `add.remote()` on a warm deployed app | deployed `@app.function` `.remote()` | `examples/add` |
| B4 | `map-fanout` | Throughput (items/s) + makespan at N | `function("add").map(inputs)` | `add.map(inputs)` | `examples/add` |
| B5 | `spawn-fanout` | Time to enqueue N + time to collect N | `.spawn()` + `FunctionCall::get()` | `.spawn()` + `FunctionCall.get()` | `examples/add` |
| B6 | `cold-start` | Container cold-start latency (image warm) | deployed `add`, force fresh container | deployed Python fn, force fresh container | `examples/add` |
| B7 | `upload-size` | Bytes + files shipped, upload time | `mount_local_dir` cargo-scoped closure | `add_local_python_source` / `Mount` for the Python pkg | `examples/add` |
| B8 | `heavy-build` (GPU, opt-in) | Cold/warm build on a realistic heavy crate | `burn-add` `.remote()`/deploy on T4 | Python+CUDA equivalent (Torch tensor add) on T4 | `examples/burn-add` |

B8 is the realistic-workload counterpart to B1: a build cache only earns its keep
on a heavy crate (the `knowledge.md` "use a realistic HEAVY build" steer), so the
warm-cache win is measured where it actually matters. B8 is GPU/cost-gated and
opt-in.

### Per-scenario A/B detail

- **B1 cold-vs-warm-build** — the headline. *Variable:* the in-body Rust build and
  the modal-rust build cache. *Rust:* `add.remote()` with the cargo-cache Volume
  reset (cold), then again (warm `Fresh`). *Python:* a `@app.function` whose body
  is `a+b`, `.remote()` on a never-built image (cold = image build + start), then
  warm. *Held constant:* same Modal account, same region/zone class, CPU only,
  same input, same warmup discipline. *Report:* three columns — cold, warm, and
  Δ(warm−cold) — for each side, plus the cargo `Fresh`/`Compiling` evidence line.

- **B2 deploy-image-build** — *Variable:* where the build happens. *Rust:* `deploy`
  builds `cargo build --release` at image-build time and bakes `/app/modal_runner`.
  *Python:* `app.deploy()` whose image has `run_commands` doing comparable build
  work (or none — note the asymmetry explicitly). *Report:* image-build wall-clock,
  and confirm via logs that `cargo` is in the build log and absent from the call
  log (the run-vs-deploy boundary, asserted as part of the bench).

- **B3 invoke-roundtrip** — *Variable:* SDK control-plane + wire overhead. *Both:*
  a warm, already-deployed trivial function; measure invoke→decoded-result with the
  body near-zero. *Report:* p50/p90/p99 over many reps; this isolates gRPC + CBOR
  (Rust) vs gRPC + pickle/CBOR (Python).

- **B4 map-fanout** — *Variable:* fan-out path. *Both:* `map` over N ∈ {1, 10, 100,
  1000} trivial inputs on a warm deployed function. *Report:* makespan and
  items/sec at each N; assert input-order results on the Rust side (a proven
  invariant). Hold container limits / concurrency settings identical on both sides.

- **B5 spawn-fanout** — *Variable:* fire-and-forget enqueue + collect. *Both:*
  spawn N, then gather N results. *Report:* enqueue time and collect time
  separately.

- **B6 cold-start** — *Variable:* container cold-start only (image already built).
  *Both:* a deployed function, forced onto a fresh container (idle-out or a fresh
  app), measure invoke→first-user-code. *Report:* cold-start latency distribution;
  note that Rust executes a prebuilt native binary while Python imports a module.

- **B7 upload-size** — *Variable:* what each SDK ships. *Rust:* the cargo-metadata
  closure-scoped upload (measured live at **7 files / 187 KB** for `examples/add`,
  down from 14 MB before scoping). *Python:* the auto-mounted local source / the
  client mount for an equivalent package. *Report:* file count, total bytes, and
  upload wall-clock for each.

- **B8 heavy-build (GPU, opt-in)** — *Variable:* cold/warm build on a genuinely
  heavy graph. *Rust:* `burn-add` (burn + cubecl + cudarc) `.remote()` (cold then
  warm cache) and the `deploy` build (measured live: base image ~116 s, top layer
  `cargo build` ~84 s incl. release in 1m16s; 214 s end-to-end the first time).
  *Python:* a comparable GPU tensor-add (e.g. Torch `c = a + b`) on the same T4.
  *Held constant:* T4 GPU, same CUDA-class base where comparable. *Cost:* GPU,
  opt-in only.

## Metric & methodology (held constant across the suite)

These rules apply to every scenario unless its `scenario.md` overrides them with a
stated reason.

- **Warmup.** Discard the first measured iteration of every warm scenario (it pays
  one-time JIT/connection/auth costs). Cold scenarios measure exactly the first
  iteration and say so. The build cache is explicitly reset before any *cold*
  measurement (e.g. `modal volume rm modal-rust-cargo-cache` or `MODAL_RUST_NO_CACHE`
  baseline) and left warm for *warm* measurements.
- **Repetitions.** Latency scenarios: ≥ 20 reps after warmup, report p50 / p90 /
  p99 and min. Build/deploy scenarios: ≥ 3 reps (they are slow and costly), report
  median and the spread. Fan-out scenarios: ≥ 3 reps per N.
- **What is held constant.** Same Modal workspace and credentials; same wall-clock
  window where feasible (Modal scheduler load drifts over the day — record the
  timestamp); same region/GPU class; same CPU/memory request; identical input
  payloads; identical container concurrency / `max_containers` settings on both
  sides; the same Modal `image_builder_version`. Record every one of these in the
  result file so a run is reproducible.
- **Cold vs warm is a first-class axis, not noise.** Always report both. Never
  average a cold run into a warm distribution.
- **GPU.** All GPU scenarios use **T4** (the project's cheap-GPU standard) unless a
  scenario explicitly needs more. GPU scenarios are opt-in (an env flag), print a
  cost heads-up, and record the GPU type in the result.
- **Cost notes.** Every scenario's `scenario.md` states an order-of-magnitude cost
  (container-seconds, GPU-seconds, egress) so a reader knows what a full run
  spends. CPU scenarios are cheap; GPU (B8) is the only material cost. Prefer the
  cheapest faithful configuration; never run GPU to measure something CPU can show.
- **Apples-to-apples discipline.** The Python side must use idiomatic, current
  Modal SDK code (pin the `modal` version in the result) and the *same logical
  workload* — not a deliberately slow strawman. Where the two SDKs are genuinely
  asymmetric (Rust compiles, Python does not), the asymmetry is reported as a
  labeled column, never hidden.
- **Fairness of the timing boundary.** Time the same logical span on both sides:
  from "ask the SDK to run this input" to "typed result decoded in the client
  process". Exclude one-time process startup of the bench harness itself; include
  the SDK's own connect/auth on cold scenarios (and say so).
- **Environment capture.** Each result records: date/time, Modal account class,
  `modal` Python version, `modal-rust` git commit, host OS/arch, network locale,
  and the held-constant settings above.

## Directory layout

```text
benchmarks/
  README.md                      # this plan
  scenarios/
    <name>/                      # one dir per A/B scenario (B1..B8)
      scenario.md                # metric, variable-under-test, held-constant, cost
      rust/                      # the modal-rust side (reuses/symlinks examples/* where possible)
      python/                    # the Modal Python equivalent (pinned `modal` version)
  results/
    <name>/                      # captured runs: raw timings + an environment header
      <utc-timestamp>.json       # machine-readable result + held-constant settings
      <utc-timestamp>.md         # human summary table (cold | warm | Δ, p50/p90/p99)
```

A future `runner` (a small Rust bin or a script, tracked in the workpad) drives a
scenario's `rust/` and `python/` sides, applies the warmup/repetition rules, and
writes the paired result into `results/<name>/`. It is **not** a Cargo workspace
member until the workpad's scaffolding stage adds it deliberately (so the main
build stays green in the meantime).

## Running (once the harness lands)

Planned UX (not yet implemented — see the workpad):

```bash
# CPU suite (cheap), one scenario:
benchmarks/run.sh cold-vs-warm-build

# whole CPU suite:
benchmarks/run.sh --cpu-only

# include the GPU scenario (T4, costs money, opt-in):
BENCH_GPU=1 benchmarks/run.sh heavy-build
```

Until then, the proven live numbers seeded from `workpads/shim-backend/knowledge.md`
(P6 cold 6.45 s → warm 0.06 s; upload 7 files / 187 KB; burn capstone 214 s
end-to-end; deploy boundary) stand in as the baseline the harness must reproduce.

## Status

Plan + scaffold only. Implementation is staged in
[`../workpads/benchmarks/tasks.md`](../workpads/benchmarks/tasks.md); proven
baselines and methodology decisions are in
[`../workpads/benchmarks/knowledge.md`](../workpads/benchmarks/knowledge.md).
