# Project Task Queue

**You edit this file.** It tells agents which workpad to load for `/next` and
similar commands.

Read top to bottom. The **first unchecked** item is the active workpad unless
Notes override it. Check items off when a phase is finished, not when pausing
mid-phase.

## Active Now

**gpu-compute** — The GPU path (M10–M13), Burn-free first: `nvidia-smi` from the
Python shim (M10) → `nvidia-smi` from a Rust function via the runner (M11) → real
Rust CUDA vector-add with cudarc precompiled-PTX (M12) → Burn tensor smoke on a
Tier-1 image (M13). Cheapest `T4` GPU; gated so the expensive CUDA-toolkit/Burn
tiers only run if the cheap nvidia-smi steps pass.

**Prototype complete (M0–M9) 2026-06-03.** `add` runs end-to-end via the
`modal-rust` CLI (run/deploy/call/doctor); the build boundary is proven both ways.
Non-blocking follow-ups tracked: **M0-R** (panic-capture review) and **M6b** (a
smarter run-compilation cache — M6's volume cache was a null result).

The plan deliberately validates **one boundary at a time**. GPU runs incur a
little cost (T4, ~cents); the loop is incremental + gated.

> **Architecture gate passed (design-complete) 2026-06-03.** `boundaries.md` is
> complete, internally consistent, and derived from the adversarially-reviewed
> synthesis (§2). The A0–A8 ratification is satisfied at the doc level; the two
> genuinely *empirical* confirmations (mount writability, runtime compile) are the
> job of prototype **M2/M4**, which run real Modal calls. We move to code now to
> validate beliefs ASAP.

> **Doc-research already done.** The multi-agent planning workflow
> (`.claude/workflows/plan-research.js`) performed the doc-level research
> (`workpads/architecture/research-synthesis.md` §1, grounded in primary Modal/
> modal-rs docs) and the architecture design + adversarial review (§2). So
> **research** is checked off at the doc level below. The two genuinely *empirical*
> confirmations — does `add_local_dir(copy=False)` mount writably, and does a normal
> `@app.function` actually `cargo build` at runtime — are intentionally carried into
> **prototype** M2/M4 (which run real Modal calls anyway), per the validation-first
> method. If you'd rather confirm runtime-compile empirically *before* ratifying the
> architecture, re-open `research` and run the R2 live spike first (it's recorded in
> `workpads/research/tasks.md` R2).

## Workpad Queue

- [x] **research** — Doc-level research complete via the planning workflow:
  Modal image/copy semantics, runtime-compile feasibility (in principle),
  `copy=False` mount, Cargo-cache, the `modal-rs` capability matrix, and GPU/CUDA
  facts are recorded in `research-synthesis.md` §1 + `research/knowledge.md`. Live
  runtime-compile + mount-writability spikes are carried into prototype M2/M4.
- [x] **architecture** — Gate passed (design-complete) 2026-06-03: `boundaries.md`
  records the crate layout, runner protocol + registry API, the **run-vs-deploy
  build boundary**, the generated shim design (dev/deploy/call), CLI surface,
  Cargo-cache design, and ignore rules — internally consistent and derived from the
  reviewed synthesis. Empirical confirmation deferred to prototype M2/M4.
- [x] **prototype** — Complete (M0–M9) 2026-06-03: `add` runs via `modal-rust
  run/deploy/call`, build boundary proven both ways. Follow-ups M0-R (panic review)
  + M6b (smart run cache) tracked in `workpads/prototype/tasks.md`. Original scope:
  The `add` function end to end (M0-M9): local dispatcher ->
  generated Function control path -> source mount -> remote runtime compile ->
  source-edit reactivity -> Cargo cache -> deploy-time build -> deployed call
  with no runtime compile -> `modal-rust` CLI wrapping the shims. Gate: `add`
  runs via `modal-rust run` and `modal-rust call` with the build boundary proven.
- [ ] **gpu-compute** — GPU path (M10-M13): nvidia-smi from the Python shim ->
  nvidia-smi from a Rust function -> real Rust CUDA vector add -> Burn tensor
  smoke. Gate: a real Rust GPU compute returns a verified result, Burn-free
  first, then a Burn smoke.
- [ ] **ergonomics** — Proc-macro registry (`inventory`, `#[modal_rust::function]`)
  that compiles to the same `Registry` shape, plus an optional PyO3/maturin
  bridge to replace the subprocess boundary. Gate: macros produce the validated
  runner shape; PyO3 path proven as optional, not required.
- [ ] **shim-backend** — Exploratory follow-up: compare generated templates vs
  static/data-driven Python shims, env/path config, hidden cache/module
  materialization, image-baked shims, and deeper Modal authoring backends. Gate: a
  decision-ready design matrix + spike plan; does not change the active GPU path.

## Notes

- Source-of-truth product prompt lives in `project.md`. The design stances:
  **(1)** direct-execution-first — try normal Modal Functions; a Sandbox is a
  documented fallback (not banned) if a Function-body build is infeasible;
  **(2)** the build boundary is the hard invariant — `run`
  builds at function-execution time, `deploy` builds at image-build time and the
  deployed runtime never runs `cargo`.
- CLI name is `modal-rust`. Internal crates may have other names. `modal-rs` is
  the existing unofficial SDK we may consume.
- Skip proc-macros and PyO3 until the manual-registry subprocess path works end
  to end. Design the v0 API so macros can be added without changing the runner
  protocol.
- Keep the first GPU proof independent of Burn. Order: nvidia-smi (python) ->
  nvidia-smi (Rust) -> CUDA kernel -> Burn.
- Shim-backend exploration is intentionally queued after the planned validation
  phases unless Notes override it; do not let it distract from proving the GPU
  path.
- Research and architecture may overlap only when task boundaries are independent
  and findings are recorded before the dependent architecture decision is made.
- Spikes (running real Modal calls) are authorized inside the `research` workpad
  because the central open question — "can we compile at runtime on a normal
  Function?" — can only be answered empirically. Keep spikes small and record
  evidence. Larger implementation waits for the architecture gate.

- **AFK autonomous run (2026-06-03 night).** User is away and authorized
  *commit-and-continue without waiting*: proceed through the remaining plan
  (gpu-compute → ergonomics → shim-backend, plus the M0-R panic-capture and M6b
  smart-cache follow-ups), committing at each milestone with a clear message, and
  report a summary when done. This **overrides the default "confirm before commit"**
  for this run. Per-milestone commits + knowledge-file notes are the durable record
  if context falls off — a fresh context should read this note, the latest commits,
  and the active-workpad knowledge.md, then continue (still: never log/commit Modal
  tokens; GPU stays on cheap T4; Modal flakiness → retry, never block).
