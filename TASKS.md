# Project Task Queue

**You edit this file.** It tells agents which workpad to load for `/next` and
similar commands.

Read top to bottom. The **first unchecked** item is the active workpad unless
Notes override it. Check items off when a phase is finished, not when pausing
mid-phase.

## Active Now

**[2026-06-04] `crates/modal-rust-sdk` landed + proven live.** The control-plane
client decision is resolved to **(b)**: our own lean first-party Modal gRPC client,
**no `modal-rs` dependency** (both `modal-rs` and the official `modal-client` cloned
into gitignored `references/` for inspiration). The crate vendors the canonical Modal
proto, re-implements auth/channel/CBOR + the proven FILE-mode ops natively (with the 3
spike fixes + Modal-native client-mount injection), and **proved end-to-end live**: our
client created + invoked a Modal function →
`{"ok":true,"source":"rust_sdk_live_wrapper.handler","echoed":{"n":42,"hi":1}}` with no
`modal` CLI and no per-project `.py`. Offline gates green (fmt/clippy `-D warnings`/build/
test on default-members; 23 sdk unit tests; live tests `#[ignore]`+`live`-feature gated).
3/3 adversarial reviews PASS. Built via the `build-modal-rust-sdk` workflow. This is **P1
done + a large slice of P3** (see `workpads/shim-backend/{knowledge.md,tasks.md}`).

> **Next (programmatic backend, `shim-backend` workpad):** wire the SDK into the
> `App`/`Function` `.remote()`/`.local()` ergonomics and migrate `modal-rust run/deploy/
> call` off Python codegen (P3→P9), then deploy path (P5), dynamic config from the registry
> (P4, drops the `--gpu` flag), cache-on-by-default (P6), local orchestration (P7). One
> live-verified caveat folded into the design: the client mount carries modal *source* only,
> so a bare base also needs the client's pip dep closure (`with_pip_install_modal()`).

<details><summary>Superseded: paused-for-input note (2026-06-03 night)</summary>

**Paused for user input (2026-06-03 night).** The validated core is DONE and
committed: prototype **M0–M9**, GPU **M10–M13** (T4: nvidia-smi → cudarc vector-add
→ Burn), ergonomics **E1** (`#[modal_rust::function]` proc-macro), the **M0-R**
panic-capture hardening, the CLI `--gpu` + multi-bin `-p` fix, and the **M6b**
sccache cache experiment (null payoff on the toy graph → opt-in `--cache` deferred
as `M6b-wire`). `cargo test`/`clippy`/`fmt` green on default-members; README "Try
it" is current.

The remaining items each need a decision or are optional/exploratory, so the
autonomous overnight run paused here rather than churn fragile work:

- **ergonomics E2** (remote-call stubs `app.add(20,2).await?`) — needs the §4-Q3
  invoke-mode decision: a *real* in-process Rust client wants the unofficial,
  pre-1.0 `modal-rs` (pickle-protocol caveats); a no-`modal-rs` version is just a
  thin wrapper over the proven `modal-rust call` subprocess. **Your call.**
- **ergonomics E3** (PyO3/maturin bridge) — explicitly optional.
- **shim-backend** — your exploratory design workpad; left for you to drive.
- **M6b-wire** — implement `--cache` (OFF by default) + the local-SCCACHE_DIR +
  Volume-snapshot-sync strategy; low value until a dependency-heavy example exists.

> Note: E2's "invoke-mode decision" is now resolved — the `modal-rust-sdk` (option b,
> own client, no `modal-rs`) is the in-process invoke path; E2 builds on it.

</details>

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
- [x] **gpu-compute** — Complete (M10–M13) 2026-06-03: verified on T4 — nvidia-smi
  (Python + Rust), cudarc precompiled-PTX vector-add (Tier 0), and a Burn tensor
  add (Tier-1 `nvidia/cuda:12.6.3-devel`). `--gpu <spec>` wired into the CLI.
  CUDA-only `example-burn-add` excluded from `default-members`.
- [~] **ergonomics** — E1 DONE (`#[modal_rust::function]` proc-macro + `inventory`
  + `Registry::from_inventory()`, byte-identical to manual `typed!`). E2 (remote-call
  stubs) + E3 (PyO3/maturin, optional) remain — E2 needs the invoke-mode decision
  (see Active Now). Gate: macros produce the validated runner shape (met for E1).
- [~] **shim-backend** — Pivoted from "compare shim backends" to **building a
  programmatic Modal control plane in Rust** (decision (b), own client). Done:
  design matrix + spike (FILE-mode create+invoke proven), control-plane decision
  locked, and **`crates/modal-rust-sdk` landed + proven live** (P1 + a slice of P3).
  Remaining staged tasks P3→P10 in `workpads/shim-backend/tasks.md`: App/Function
  ergonomics, CLI off-codegen, deploy, dynamic config, cache, local orchestration.

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
