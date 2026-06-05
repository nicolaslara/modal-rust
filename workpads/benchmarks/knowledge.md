# Benchmarks Knowledge

## Objective

Capture the design, baselines, and methodology decisions for the `modal-rust`
vs Modal-Python A/B benchmark suite. This is a *measurement* workpad layered on a
**complete, live-proven** product (see `../../TASKS.md` "Project complete" and
`../shim-backend/knowledge.md`): it adds benchmarks, not capability, and must not
regress the build or touch the frozen runner/registry/boundary invariants.

The plan + scaffold are in [`../../benchmarks/README.md`](../../benchmarks/README.md);
the staged build is in [`tasks.md`](./tasks.md).

## Gate Status

Not passed yet. This file currently records the seeded baselines (proven live in
prior milestones) and the methodology the harness must implement. It passes when
the CPU suite (B1–B7) has captured paired results applying the recorded
methodology, B1 reproduces the cold-vs-warm baselines within an order of
magnitude, and the main `default-members` build stays green.

## Why benchmark at all (and the honest framing)

Both sides run on Modal, so this is a **systems / developer-velocity** benchmark,
not a raw-compute one. We are not claiming Rust-vs-Python execution speed in
general; we measure the parts the two SDKs own and differ on (build/iteration
model, control-plane round-trip, upload, fan-out, cold-start).

The one comparison that is easy to abuse is **build/iteration**, because
`modal-rust` compiles the user's Rust in the function body while Python has no
compile step. The discipline (locked):

- Python wins the very first **cold** call on a brand-new image (Rust must
  compile); report it honestly.
- The modal-rust **warm** loop is competitive because the build cache (on by
  default) turns the rebuild into a `Fresh` no-op.
- Steady state, modal-rust delivers a compiled, typed function.

So B1 is always reported as **three columns — cold, warm, Δ** — per side, never
collapsed into a single "faster/slower" headline. This framing is the most
important single decision in this workpad.

## Seeded baselines (proven LIVE in prior milestones)

These are the numbers the harness must reproduce within an order of magnitude.
Sources are the dated sections of `../shim-backend/knowledge.md` and `../../TASKS.md`.

| Metric | Workload | Proven live value | Source milestone |
| --- | --- | --- | --- |
| In-body cargo build, cold | `examples/add` (16-crate trivial graph) | 6.45 s | P6 cargo cache |
| In-body cargo build, warm (cache hit) | `examples/add` | **0.06 s** (`Fresh`, no `Compiling`/`Downloaded`) | P6 cargo cache |
| End-to-end `.remote()`, cold → warm | `examples/add` | 32.9 s → 9.3 s (≈3.5×) | P6 cargo cache |
| First-ever `.remote()` end-to-end (cold image + build) | `examples/add` | 63.59 s (cold image build ~30 s, then warm) | real `.remote()` PROVEN LIVE |
| Upload size (cargo-metadata closure-scoped) | `examples/add` | **7 files / 187 KB** (was 14 MB+ before scoping) | P-harden |
| Deploy image-build, base layer | `examples/burn-add` (heavy: burn+cubecl+cudarc) | ~116 s (add_python + rustup) | Burn capstone |
| Deploy image-build, top layer (`cargo build --release`) | `examples/burn-add` | ~84 s (release in 1m16s) | Burn capstone |
| Burn deploy+call end-to-end (first time) | `examples/burn-add` on T4 | 214 s | Burn capstone |
| GPU correctness on T4 | `cuda-vector-add`, `burn-add` | `gpu_name="Tesla T4"`; GPU result matches CPU ref | P4 / capstone |
| `.map()` fan-out | `add`, N=4 | `[2,4,6,42]` in input order | spawn/map |

**Key takeaway for B1:** the trivial `add` graph is the *worst* case for a build
cache (sync overhead vs a tiny recompile). The realistic heavy workload
(`burn-add`) is where the warm-cache win dominates — hence the dedicated GPU
scenario B8. This mirrors the recorded steer in `../shim-backend/knowledge.md`
("use a realistic HEAVY build, not `add`"); `add` is kept as the cheap,
deterministic CPU baseline, `burn-add` as the realistic stressor.

## Methodology decisions (locked unless a scenario overrides with a reason)

- **Cold vs warm is a first-class axis, not noise.** Always report both; never
  average a cold run into a warm distribution. Reset the cache before cold
  measurements (`modal volume rm modal-rust-cargo-cache` or a `MODAL_RUST_NO_CACHE`
  baseline); leave it warm for warm measurements.
- **Warmup.** Discard the first measured iteration of every *warm* scenario.
  *Cold* scenarios measure exactly the first iteration and say so.
- **Repetitions.** Latency: ≥ 20 reps after warmup → p50/p90/p99 + min.
  Build/deploy: ≥ 3 reps (slow + costly) → median + spread. Fan-out: ≥ 3 reps per N.
- **Held constant (recorded in every result header):** Modal workspace +
  credentials; timestamp (scheduler load drifts over the day); region/GPU class;
  CPU/mem request; identical input payloads; identical container concurrency /
  `max_containers`; same `image_builder_version`; pinned `modal` Python version;
  `modal-rust` git commit; host OS/arch.
- **Timing boundary (same span on both sides):** from "ask the SDK to run this
  input" to "typed result decoded in the client process". Exclude the bench
  harness's own process startup; include the SDK's connect/auth on cold scenarios
  (and say so).
- **GPU = T4** (the project's cheap-GPU standard) unless a scenario explicitly
  needs more. GPU scenarios are opt-in (`BENCH_GPU=1`), print a cost heads-up, and
  record the GPU type.
- **Cost notes** per scenario: order-of-magnitude container-seconds / GPU-seconds /
  egress in each `scenario.md`. CPU scenarios are cheap; B8 (GPU) is the only
  material cost. Prefer the cheapest faithful config; never run GPU to measure
  what CPU can show.
- **Apples-to-apples discipline.** The Python side uses idiomatic, current Modal
  SDK code (pinned version) and the *same logical workload* — not a strawman.
  Genuine asymmetries (Rust compiles, Python does not) are reported as labeled
  columns, never hidden.

## Reuse-not-copy (so the bench never drifts from the example)

Benchmark scenarios **reuse or symlink** the relevant `examples/*` crate rather
than copying the workload:

- B1/B2/B3/B4/B5/B6/B7 reuse `examples/add` (the canonical trivial CPU function).
- B8 reuses `examples/burn-add` (the heavy CUDA/Burn workload; CUDA-only, stays
  out of `default-members`).

A copied workload would silently diverge from the documented example over time; a
symlink/reuse keeps the measured Rust identical to what users read in `examples/`.
The Modal Python counterpart is the one piece written fresh per scenario, under
`scenarios/<name>/python/` with its `modal` version pinned in the result.

## Build-hygiene constraints (do not regress)

- Nothing under `benchmarks/` becomes a Cargo workspace `default-members` entry.
  If B0 adds a Rust driver crate it goes in `members` but is **excluded from
  `default-members`** (same pattern as `example-burn-add`), with a comment
  explaining why, so `cargo {build,clippy,test}` on `default-members` and CI stay
  green.
- The fallback for B0 is a plain script (shell/Python) under `benchmarks/` — no
  workspace member at all — with an identical result-file contract.
- This workpad changes **no product code** and must not touch the runner
  protocol, the inventory registry, the macros, or the run-vs-deploy boundary.
  Build-boundary assertions in B2/B8 *observe* the boundary (cargo in the deploy
  log, absent at call); they do not modify it.

## Open questions (resolve during the build, record the answer here)

- **Python equivalent for B8.** Torch `c = a + b` on T4 is the obvious CUDA
  tensor-add analogue, but it imports a large wheel — is its cold "build/pull" a
  fair counterpart to a Rust compile, or should the Python side be timed only on
  the warm path with the asymmetry labeled? (Lean: report both, label the
  asymmetry.)
- **Forcing cold starts (B6).** Idle-out vs fresh-app-per-rep — which gives a
  cleaner, cheaper cold-start sample on each SDK? Record the chosen method.
- **Cache reset granularity (B1).** `modal volume rm` (true cold) vs
  `MODAL_RUST_NO_CACHE` (no-cache baseline) measure subtly different things — the
  README/tasks call for the volume-rm true-cold; confirm it's not too flaky and
  record the fallback if so.
- **Fan-out N cap (B4/B5).** Is N=1000 cheap enough on both SDKs, or should the
  cap be lower? Record the chosen N set and any cost cap.
- **Driver shape (B0).** Rust bin (excluded from `default-members`) vs a script —
  pick the lower-friction option that keeps the build green; record the choice.
- **Result schema.** Lock the `results/<name>/<utc>.json` schema (environment
  header + per-state samples) in B0 so all scenarios share it.

## Status

Plan + scaffold + baselines recorded. Implementation staged in [`tasks.md`](./tasks.md)
(B0 harness → CPU scenarios B1–B7 → GPU B8 → summary B9). No product code touched;
`default-members` build unaffected.
