# Benchmarks Tasks — staged plan (A/B vs Modal Python)

Build a reproducible **A/B benchmark suite** comparing `modal-rust` to the
official **Modal Python** SDK on the same workloads against the same live Modal
account. The plan + scaffold live in [`../../benchmarks/README.md`](../../benchmarks/README.md);
proven baselines and methodology decisions are in [`knowledge.md`](./knowledge.md).

This workpad opens **after** the project's core is complete (see `../../TASKS.md`
"Project complete" and `../shim-backend/knowledge.md`). It adds *measurement* on
top of proven capability — it changes no product code and must not regress the
build.

## Objective

Produce, for each scenario in `benchmarks/README.md` (B1–B8), a paired
modal-rust-vs-Modal-Python measurement with a fixed methodology (warmup,
repetitions, held-constant settings, T4 for GPU, recorded cost), captured as a
machine-readable + human-readable result under `benchmarks/results/<name>/`. The
headline (B1 cold-vs-warm build/iteration) must be reported as three honest
columns — cold, warm, Δ — never collapsed into a single "faster/slower" claim.
The deliverable is a suite a reader can re-run and trust, not a one-sided demo.

## Frozen invariants (must NOT change)

Benchmarking is **read-only with respect to the product.** It must not touch:

- The **runner CLI protocol** (`modal_runner --entrypoint … --input-file …`, one
  JSON envelope, five error kinds).
- The **inventory `Registry`** + `typed!()` / `#[modal_rust::function(...)]`
  macros, and the decorator-is-the-config model.
- The **run-vs-deploy build boundary** (`run` builds at function-execution time;
  `deploy` builds at image-build time; the deployed runtime never runs `cargo`).
- The main build staying **green on `default-members`**: nothing in `benchmarks/`
  becomes a Cargo workspace member until B0 deliberately adds it, and the
  GPU/CUDA `example-burn-add` stays out of `default-members`.

Benchmark scenarios **reuse or symlink** the relevant `examples/*` crate rather
than copying it, so the measured Rust workload never drifts from the documented
example.

## Method

**Validate one measurement boundary per task.** Each task adds exactly one
scenario (or one piece of shared harness), produces captured evidence (a result
file with its environment header), and crosses a single new comparison boundary.
Stages are ordered: shared harness (B0) → the cheap CPU scenarios → the GPU
scenario last. Do not start the GPU scenario until the CPU suite is green and the
methodology is settled. CPU scenarios are pre-authorized (cheap, ephemeral apps);
the GPU scenario (B8) needs the standard GPU heads-up before running.

## Gate

This workpad passes when:

1. `benchmarks/` contains the shared harness (B0) and at least the CPU scenarios
   B1–B7, each with a `scenario.md` and a captured paired result in
   `benchmarks/results/<name>/` (modal-rust + Modal Python, same methodology).
2. B1 (cold-vs-warm build) reports the three-column honest framing and its
   numbers are within the same order of magnitude as the seeded live baselines
   (P6 cold 6.45 s → warm 0.06 s `Fresh`; end-to-end ≈3.5×).
3. The methodology (warmup, reps, held-constant list, cost notes) is applied
   uniformly and recorded in each result's environment header.
4. The main build stays green: `cargo fmt --check`, `cargo clippy --all-targets
   -- -D warnings`, `cargo test` on `default-members` are unaffected; any bench
   crate added in B0 is excluded from `default-members`.
5. `benchmarks/README.md` is updated from "plan" to "results available" with a
   link to the captured runs.

The GPU scenario (B8) is a tracked follow-up inside the gate, not a blocker for
the CPU-suite gate (it costs money and needs the heads-up).

---

## B0 — Shared bench harness + scaffold (no product change)

Status: pending

risk: low. depends_on: []

- **Boundary crossed:** a runnable harness exists that drives a scenario's
  `rust/` and `python/` sides, applies warmup + repetitions, captures the
  environment header, and writes a paired result — without becoming a
  `default-members` workspace member.
- **acceptance:**
  - A small driver (Rust bin under `benchmarks/<crate>` OR a script) that, given a
    scenario name, runs both sides, times the same logical span on each, and emits
    `results/<name>/<utc>.json` + `<utc>.md`.
  - The environment header captures: date/time, Modal account class, `modal`
    Python version, `modal-rust` git commit, host OS/arch, and the held-constant
    settings (region/GPU class, CPU/mem request, concurrency, `image_builder_version`).
  - If the driver is a Rust crate, it is added to `members` but **excluded from
    `default-members`** (like `example-burn-add`), so the main build/CI stays
    green; a comment records why.
  - The Python side has a pinned `modal` version recorded in the result.
- **evidence:** one end-to-end dry run of the harness on a trivial scenario
  producing a result file; `cargo build`/`clippy`/`test` on `default-members`
  unchanged; `cargo metadata` `workspace_default_members` confirms no new default
  member.
- fallback: if a Rust driver adds friction, the driver is a shell/Python script
  under `benchmarks/` (no workspace member at all) — the result-file contract is
  identical.

## B1 — Cold-vs-warm build / iteration (the headline)

Status: pending

risk: medium. depends_on: [B0]

- **Boundary crossed:** the defining modal-rust trait — in-body Rust build + a
  build cache on by default — measured against Python's no-compile model, with
  the cache reset/warm states controlled.
- **acceptance:** `scenarios/cold-vs-warm-build/` with a `rust/` side (reuses
  `examples/add`) and a `python/` side (`@app.function` body `a+b`). Measure, for
  each side: a **cold** `.remote()` (cargo cache reset via `modal volume rm
  modal-rust-cargo-cache` / `MODAL_RUST_NO_CACHE` baseline; Python = never-built
  image) and a **warm** `.remote()` (cache hit; Python = warm image). Report three
  columns — cold, warm, Δ — plus the cargo `Fresh`/`Compiling` evidence line on
  the Rust side. ≥ 3 reps per state.
- **evidence:** the paired result file with the three-column table; the captured
  cargo build line showing `Fresh` (≈0.06 s) on the warm Rust run and `Compiling`
  on the cold run; the cache-reset command.
- fallback: if cache reset is flaky, measure warm-after-cold within one session
  and note the reset method used.

## B2 — Deploy / image-build time + boundary assertion

Status: pending

risk: medium. depends_on: [B0]

- **Boundary crossed:** the deploy boundary's wall-clock — Rust builds at
  image-build time and bakes the binary; Python deploys with comparable (or no)
  build work.
- **acceptance:** `scenarios/deploy-image-build/` (reuses `examples/add`). Measure
  image-build / deploy wall-clock for `App::deploy_with` vs `app.deploy()`. Assert
  the run-vs-deploy boundary as part of the bench: `cargo` appears in the deploy
  build log and is **absent** from the subsequent call log. Note the Rust-vs-Python
  asymmetry (Rust compiles at build time; the Python body has no compile) as a
  labeled column, not a hidden one. ≥ 3 reps.
- **evidence:** the result file; the build-log excerpt with `cargo` present at
  deploy and absent at call.
- fallback: if redeploy churn is costly, measure one cold deploy + one warm
  redeploy and report both.

## B3 — Invoke round-trip latency (trivial body)

Status: pending

risk: low. depends_on: [B0]

- **Boundary crossed:** SDK control-plane + wire overhead in isolation (body near
  zero, warm deployed function on both sides).
- **acceptance:** `scenarios/invoke-roundtrip/` (reuses `examples/add`). On a warm
  deployed function each side, measure invoke→decoded-result over ≥ 20 reps after
  warmup; report p50/p90/p99 and min for each SDK.
- **evidence:** the latency distribution in the result file; the warm-deployed
  fixture used; the discarded warmup iteration noted.
- fallback: none needed (cheap, deterministic).

## B4 — `.map()` fan-out throughput at N

Status: pending

risk: medium. depends_on: [B3]

- **Boundary crossed:** the fan-out path at scale — `function("add").map(inputs)`
  vs `add.map(inputs)`.
- **acceptance:** `scenarios/map-fanout/` (reuses `examples/add`). For N ∈ {1, 10,
  100, 1000} trivial inputs on a warm deployed function, measure makespan and
  items/sec for each side; ≥ 3 reps per N. Hold container concurrency /
  `max_containers` identical on both sides. Assert input-order results on the Rust
  side.
- **evidence:** the throughput-vs-N table; the concurrency settings recorded as
  held-constant; the input-order assertion passing.
- fallback: cap N at the largest cheap value if 1000 is too costly; record the cap.

## B5 — `.spawn()` fan-out (enqueue + collect)

Status: pending

risk: low. depends_on: [B3]

- **Boundary crossed:** fire-and-forget enqueue and later collection.
- **acceptance:** `scenarios/spawn-fanout/` (reuses `examples/add`). Spawn N, then
  collect N via `FunctionCall::get()` / `FunctionCall.get()`. Report enqueue time
  and collect time **separately** for each side; ≥ 3 reps.
- **evidence:** the enqueue/collect split in the result file.
- fallback: reuse B4's N set; record any cap.

## B6 — Cold-start latency (image warm)

Status: pending

risk: medium. depends_on: [B3]

- **Boundary crossed:** container cold-start only, with the image already built —
  Rust execs a prebuilt native binary; Python imports a module.
- **acceptance:** `scenarios/cold-start/` (reuses `examples/add`). Force a fresh
  container (idle-out or fresh app) on each side, measure invoke→first-user-code
  over several cold containers; report the cold-start distribution. Note the
  native-binary-vs-import asymmetry.
- **evidence:** the cold-start distribution; how a fresh container was forced.
- fallback: if forcing cold starts is unreliable, report the method and a
  best-effort sample with the caveat recorded.

## B7 — Upload size & time

Status: pending

risk: low. depends_on: [B0]

- **Boundary crossed:** what each SDK ships to make a function runnable.
- **acceptance:** `scenarios/upload-size/` (reuses `examples/add`). Measure file
  count, total bytes, and upload wall-clock for the modal-rust cargo-scoped
  closure upload vs the Python local-source/client-mount equivalent. The Rust side
  should reproduce the seeded baseline (**7 files / 187 KB** for `examples/add`).
- **evidence:** the size/time table; the file list on the Rust side matching the
  cargo-metadata closure.
- fallback: none needed (cheap, mostly deterministic).

## B8 — Heavy build on a GPU (realistic workload, opt-in)

Status: pending

risk: medium-high (cost). depends_on: [B1, B2]

- **Boundary crossed:** the cold/warm build comparison on a genuinely heavy graph
  (the realistic-workload counterpart to B1), where the build cache earns its keep.
- **acceptance:** `scenarios/heavy-build/` (reuses `examples/burn-add`). Measure
  `burn-add` `.remote()` cold (cache reset) then warm (`Fresh`), plus the `deploy`
  build wall-clock, on a **T4**; compare against a Python+CUDA tensor-add (e.g.
  Torch `c = a + b`) on the same T4. Held constant: T4 GPU, comparable CUDA-class
  base where feasible. Opt-in (`BENCH_GPU=1`), with a cost heads-up and the GPU
  type recorded. Seeded deploy baseline: base image ~116 s, top `cargo build`
  ~84 s, 214 s end-to-end the first time.
- **evidence:** the GPU result file with cold/warm build columns, the GPU type and
  cost note; the deploy build-log boundary assertion (cargo at build, absent at
  call). Confirm before running (GPU spend).
- fallback: a cheaper pure-CPU heavy crate as the build-cache stressor if T4 cost
  is undesirable (per the knowledge.md "optional cheaper proxy" note); record the
  substitution.

## B9 — Suite summary + README flip

Status: pending

risk: low. depends_on: [B1, B2, B3, B4, B5, B6, B7]

- **Boundary crossed:** the suite is presentable — a single summary table across
  scenarios and the README updated from "plan" to "results available".
- **acceptance:** a top-level `benchmarks/results/SUMMARY.md` aggregating the
  paired results (one row per scenario, modal-rust vs Modal Python, with the
  held-constant caveats); `benchmarks/README.md` updated to link the captured runs
  and drop the "plan + scaffold only" status. The honest three-column framing of
  B1 is preserved in the summary.
- **evidence:** the summary table; the updated README; `default-members` build
  still green.
- fallback: ship the summary with whatever CPU scenarios are green; mark GPU (B8)
  as pending if it was deferred.

---

## DAG

```text
B0 ─┬─ B1 ─┐
    │       ├─ B8 (GPU, opt-in)
    ├─ B2 ──┘
    ├─ B3 ─┬─ B4
    │      ├─ B5
    │      └─ B6
    └─ B7
{B1,B2,B3,B4,B5,B6,B7} → B9
```

B8 (GPU) and B9 (summary) are the trailing leaves. The CPU suite (B1–B7) is the
gate; B8 is a cost-gated follow-up inside the gate, B9 packages whatever is green.
