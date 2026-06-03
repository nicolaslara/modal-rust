# Workpads Index

Per-workpad load lists and objectives for **modal-rust**. Resolve the active
workpad from [`../TASKS.md`](../TASKS.md) (first unchecked item unless Notes
override), confirm its objective and load list here, then follow the core loop in
[`../WORKING.md`](../WORKING.md). The authoritative architecture facts and the
M0–M13 milestone plan live in
[`architecture/research-synthesis.md`](./architecture/research-synthesis.md);
its locked decisions and the design stances bind every workpad.

The design stances bind all workpads (the **build boundary is the hard
invariant**): **(1) direct-execution-first; Sandbox is a documented fallback** —
try the core path on a normal `@app.function` first (runtime compile in a Function
body, M4); if that proves infeasible for a step, iterate to a Modal Sandbox for
that step and record it; **(2)** the build boundary is the product — `run` builds
at function-execution time, `deploy` builds at image-build time, and the deployed
runtime executes only the prebuilt `/app/modal_runner` and never invokes `cargo`
(this holds whether the build runs in a Function body or a Sandbox); **(3) prefer
static dispatch** — favor `enum`/generics/`impl Trait` over `dyn Trait`, reaching
for `dyn` only for genuinely open sets.

## Current Focus

| Workpad | Status | Description |
| --- | --- | --- |
| `research` | Complete (doc-level) | Done via the planning workflow — synthesis §1 + `research/knowledge.md`. Live runtime-compile + mount-writability spikes carried into prototype M2/M4 |
| `architecture` | **Active** | Ratify the run-vs-deploy boundary, runner protocol + registry API, generated shim + CLI design in `boundaries.md` (A0–A8) against the research findings, then pass the gate |
| `prototype` | Planned | The `add` function end to end (M0–M9): local dispatch → remote runtime compile → deploy-time build → deployed call with no runtime compile → `modal-rust` CLI wrapper |
| `gpu-compute` | Planned | The GPU path (M10–M13): nvidia-smi from Python → from Rust → real Rust CUDA vector add → Burn tensor smoke |
| `ergonomics` | Planned | Proc-macro registry (`inventory`, `#[modal_rust::function]`) compiling to the same `Registry` shape, plus an optional PyO3/maturin bridge |

## research

Status: Complete (doc-level), via the planning workflow. The live runtime-compile
and mount-writability spikes are carried into prototype M2/M4. Re-open only if you
want empirical confirmation before ratifying the architecture.

Objective: Validate the riskiest assumptions before any later phase depends on
them, with primary sources and small authorized Modal spikes. The central
question is empirical and gates everything in `run` mode: **can a normal
`@app.function` (not a Sandbox) mount Rust source via `add_local_dir(copy=False)`
and `cargo build` it at runtime?** Also resolve whether `copy=False` mounts are
writable in place (the single biggest unverified assumption), whether a
Cargo-cache Volume makes warm rebuilds materially faster (null result allowed),
how much deploy/invoke lifecycle `modal-rs` exposes vs needing generated Python,
and the GPU/CUDA tiering facts. Separate proven facts from assumptions; record
the exact spike command and result. (Grounds the Verified Facts §1 and Residual
Risks §5 of `research-synthesis.md`.)

**Load:**

```text
../TASKS.md
../AGENTS.md
../project.md
../WORKING.md
WORKPADS.md
research/tasks.md
research/knowledge.md
research/references.md
```

Quick nav:

- Authoritative facts + risks: [`architecture/research-synthesis.md`](./architecture/research-synthesis.md) (§1 Verified Facts, §5 Residual Risks)
- Validation-first philosophy + riskiest loose ends: [`../project.md`](../project.md)
- Research verification standard: [`../WORKING.md`](../WORKING.md) (Verification table, Research gate)

Rules:

- Prefer primary sources (Modal docs, modal-rs docs.rs/repo, CUDA-crate docs);
  record observation dates — Modal's API and pricing drift.
- Spikes that make real Modal calls are authorized **only** here; keep them
  small, opt-in, and record the exact command + result. Real runs cost money.
- Separate proven facts (spike or primary doc) from assumptions and
  recommendations; never log or commit Modal tokens / `~/.modal.toml`.
- Do not contradict `research-synthesis.md` or the design stances.

## architecture

Status: **Active**. Research is doc-complete via the planning workflow; this phase
ratifies `boundaries.md` (A0–A8) against the synthesis and passes the gate.

Objective: Turn the locked synthesis into the canonical, reviewable contracts
every later phase builds against, recorded in
[`architecture/boundaries.md`](./architecture/boundaries.md): the cargo workspace
+ crate layout and its acyclic dependency edges (§2.1); the runner CLI protocol —
the frozen seam — with the five-kind error taxonomy
(`decode_error|unknown_entrypoint|function_error|encode_error|panic`), the failure
envelope's optional additive `details` field carrying the wrapped user error, exit
codes, the stdout-only-envelope rule, and the frozen precedence (§2.2); the
static-dispatch `Registry`/`typed!()`/`HandlerFn` API with the
macro-compatibility invariant — `type HandlerFn = fn(&[u8]) -> Result<Vec<u8>,
RunnerError>`, `Registry = BTreeMap<&'static str, HandlerFn>` (no `Box<dyn>`, no
vtable) — and the reserved `typed_async!` (§2.3); the run-vs-deploy build boundary as an explicit
table; the three generated shims (`dev_app`/`deploy_app`/`call_app`); the
`modal-rust` CLI surface (`doctor`/`run`/`deploy`/`call`); and the cache /
ignore-rules design. Surface the four user-sensitive decisions with the
synthesis's recommended defaults (§4).

**Load:**

```text
../TASKS.md
../AGENTS.md
../project.md
../WORKING.md
WORKPADS.md
architecture/tasks.md
architecture/knowledge.md
architecture/references.md
architecture/boundaries.md
architecture/research-synthesis.md
```

Quick nav:

- Locked decisions: [`architecture/research-synthesis.md`](./architecture/research-synthesis.md) (§2.1–§2.8), open questions (§4)
- Canonical contracts: [`architecture/boundaries.md`](./architecture/boundaries.md)
- Runtime contract + boundary model: [`../project.md`](../project.md)
- Architecture gate: [`../WORKING.md`](../WORKING.md) (Workpad Gates #2)

Rules:

- Every contract must trace to a locked decision in `research-synthesis.md` (§2)
  and contradict neither the synthesis nor the design stances.
- The five error kinds + stdout-only-envelope are the cross-version seam; any
  change must be additive-only (the optional `details` field that wraps the user
  error on the top-level enum follows this rule; codec-neutral `&[u8]`, reserved
  `typed_async!`, named-object argument shape, reserved optional `meta`/`version`).
- Define boundaries and failure modes before broad implementation; call out the
  four user-sensitive decisions (GPU/cost, public deploys, default `call` mode,
  wire format) with the recommended defaults.
- `clap` is CLI-only; the runner uses a hand-rolled parser. v0 authoring/build
  uses generated Python + the official `modal` CLI; modal-rs is `call`-only.

## prototype

Status: Planned (gate prerequisite: architecture gate passed, or `TASKS.md`
authorizes a spike).

Objective: Prove the whole `modal-rust` core path on the smallest possible
function — `add` → `{"sum":42}` for `{"a":40,"b":2}` — validating one boundary
per milestone (M0–M9): local dispatcher + runner contract (no Modal) → generated
Function control path → `copy=False` source mount → **runtime compile in the
function body** (the key validation, M4; on the happy path no Sandbox, but if the
Function-body build is infeasible, evaluate + record a Modal Sandbox build rather
than declaring failure) → source-edit reactivity → best-effort
Cargo-cache Volume → **deploy-time build** (`copy=True` + `run_commands`, baked
`/app/modal_runner`, M7) → deployed call that **never compiles** (M8) → the
`modal-rust` CLI wrapping the shims (M9). Deliver a working walking skeleton, not
a complete product. (Grounds §3 M0–M9 of `research-synthesis.md` and
[`prototype/spec.md`](./prototype/spec.md).)

**Load:**

```text
../TASKS.md
../AGENTS.md
../project.md
../WORKING.md
WORKPADS.md
prototype/tasks.md
prototype/knowledge.md
prototype/references.md
prototype/spec.md
architecture/boundaries.md
architecture/research-synthesis.md
```

Quick nav:

- POC scope, minimum, non-goals: [`prototype/spec.md`](./prototype/spec.md)
- Milestone acceptance + evidence (M0–M9): [`architecture/research-synthesis.md`](./architecture/research-synthesis.md) (§3)
- Contracts the milestones build against: [`architecture/boundaries.md`](./architecture/boundaries.md)
- Prototype gate: [`../WORKING.md`](../WORKING.md) (Workpad Gates #3)

Rules:

- Do not build the next milestone before the current boundary is proven with
  evidence; record uncertainty rather than moving on.
- Direct-execution-first: prove the happy path with no Sandbox (build in the
  Function body for `run`); if a Function-body build proves infeasible for a step,
  the documented fallback is a Modal Sandbox build for that step — evaluate and
  record it rather than declaring failure (M4). No proc-macros yet; no local binary
  upload — the runner is built remotely (function body for `run`, image layer for
  `deploy`).
- The deployed runtime never compiles: prove `cargo build` appears in
  deploy/build logs and is **absent** from call logs; deployed result stable
  until explicit redeploy.
- The M6 cache speedup is best-effort and does NOT block the gate; a null result
  is an acceptable deliverable. Confirm before any deploy that creates a
  persistent app.

## gpu-compute

Status: Planned (gate prerequisite: prototype gate passed — `add` runs via
`modal-rust run` and the run-vs-deploy boundary is proven; M10 may run in
parallel off M1).

Objective: Prove the GPU path one boundary at a time (M10–M13), keeping the first
GPU proof Burn-free: `nvidia-smi` from the Python shim (Tier 0 sanity, M10) →
`nvidia-smi` from a Rust function where the only new variable vs M4/M7 is `gpu=`
placement (M11) → a real Rust CUDA vector add via cudarc + `dynamic-loading`
running **precompiled PTX** through the Driver API on Tier 0 (M12) → a Burn
(burn-cuda/cubecl) tensor smoke on a **Tier 1** image whose loader path carries
`libnvrtc`/`libcudart` (M13). `gpu=` is passed through verbatim — the drifting
catalog is not re-implemented. (Grounds §3 M10–M13 and §2.8 of
`research-synthesis.md`.)

**Load:**

```text
../TASKS.md
../AGENTS.md
../project.md
../WORKING.md
WORKPADS.md
gpu-compute/tasks.md
gpu-compute/knowledge.md
gpu-compute/references.md
prototype/spec.md
architecture/boundaries.md
architecture/research-synthesis.md
```

Quick nav:

- GPU tiering + `gpu=` passthrough: [`architecture/research-synthesis.md`](./architecture/research-synthesis.md) (§2.8), milestones (§3 M10–M13)
- GPU/CUDA verified facts + drift risk: [`architecture/research-synthesis.md`](./architecture/research-synthesis.md) (§1.4, §5.6)
- Build recipe reused for GPU: [`architecture/boundaries.md`](./architecture/boundaries.md), [`prototype/spec.md`](./prototype/spec.md)
- GPU gate: [`../WORKING.md`](../WORKING.md) (Workpad Gates #4)

Rules:

- Keep the first GPU proof independent of Burn: order is nvidia-smi (Python) →
  nvidia-smi (Rust) → CUDA kernel → Burn.
- The build path is the exact M4/M7 recipe (CPU-proven); the sole new variable is
  `gpu=` — isolate one boundary at a time.
- cudarc uses `dynamic-loading` (links with no CUDA at build time); a startup
  self-check dlopens the required libs and fails loudly. Burn/cubecl on a
  driver-only image fails at runtime — M13 requires a Tier 1 image.
- Never hardcode the point-in-time driver/CUDA version; keep container toolkit
  major ≤ host. GPU runs cost more — record the GPU type + cost; confirm before
  deploying a persistent GPU app.

## ergonomics

Status: Planned (gate prerequisite: prototype gate passed; macros must not change
the runner protocol).

Objective: Add the deferred ergonomics on top of the proven manual path — a
proc-macro registry (`#[modal_rust::function]` + `inventory`) that compiles to the
**same** static-dispatch `Registry` shape and runner protocol, and an optional
PyO3/maturin bridge that may replace the subprocess boundary. The v0 handler is
already frozen as a codec-neutral bare `fn` pointer
(`type HandlerFn = fn(&[u8]) -> Result<Vec<u8>, RunnerError>`, no `Box<dyn>`) with
a reserved `typed_async!` and a frozen named-object argument shape, so both stay
strictly additive — the proc-macro generates the same monomorphized wrapper + an
`inventory` registration (or a static `match` table). (Grounds
the macro-compatibility invariant in `research-synthesis.md` §2.3 and the §2.3
concurrency caveat for a future PyO3 in-process host.)

**Load:**

```text
../TASKS.md
../AGENTS.md
../project.md
../WORKING.md
WORKPADS.md
ergonomics/tasks.md
ergonomics/knowledge.md
ergonomics/references.md
architecture/boundaries.md
architecture/research-synthesis.md
```

Quick nav:

- Macro-compatibility invariant + reserved async: [`architecture/research-synthesis.md`](./architecture/research-synthesis.md) (§2.3)
- Concurrency caveat for a future PyO3 host: [`architecture/research-synthesis.md`](./architecture/research-synthesis.md) (§2.3 concurrency note, §5.7)
- Frozen contracts macros must match: [`architecture/boundaries.md`](./architecture/boundaries.md)
- Stack direction (PyO3/maturin as a later bridge): [`../project.md`](../project.md)

Rules:

- No proc-macros or PyO3 until the manual-registry subprocess path works end to
  end; macros must compile to the validated runner shape without changing the
  protocol.
- The macro detects `async fn` and expands to `typed_async!` vs `typed!` (yielding
  the same bare `fn`-pointer `HandlerFn` shape, no `dyn`/`Box`); duplicate-name
  rejection and the named-object argument shape are preserved.
- A future concurrent PyO3 host (in-process Mode B) must revisit per-call panic
  routing and the panic-then-reuse hazard before enabling concurrency (the v0
  panic-capture uses a process-global slot).
- The PyO3 path is proven as optional, not required — the subprocess runner stays
  the known-good control path. Do not contradict the design stances.
