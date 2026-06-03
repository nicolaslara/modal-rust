# Working Practices

Living project-wide agreement for how agents work on **modal-rust**. Complements
`project.md` and the workpads. Update when the workflow changes.

## Purpose

Build a Rust-on-Modal function runtime by validating one boundary at a time.
Agents execute small tasks, prove each boundary with evidence, invite critique
when confidence is not high, and ask the user on product-sensitive decisions
(cost, public deploys, API shape).

## General

Whenever a task is too complex or a boundary is risky, spawn the strongest
available analysis/review agents to compare options and produce reviewable
findings. Use faster agents for well-defined file creation or mechanical
expansion once the target structure is clear. The two saved workflows
(`.claude/workflows/refine-plan.js`, `.claude/workflows/implement.js`) encode the
preferred multi-agent shapes.

## Validate One Boundary At A Time

This project's whole method is incremental boundary validation. Each milestone
proves exactly one new assumption and nothing more:

- Do not build the next milestone before the current boundary is proven.
- Do not add ergonomics (macros, PyO3) before the manual subprocess path works
  end to end.
- Do not optimize (caching) before the correctness path it accelerates works.
- If a milestone's evidence is weak, record the uncertainty rather than moving on.

## LLM-Friendly File Boundaries

Prefer files with one conceptual responsibility, understandable in one pass. Aim
for Rust modules around 300-500 LOC; treat 800-1,000+ LOC as a refactor-soon
warning. Keep active workpad `tasks.md` cockpit-like; move accumulated background
and canonical decisions into `knowledge.md` and `boundaries.md`.

## Workarounds

Prefer the right fix. Workarounds are acceptable only to unblock progress,
time-box a spike, or isolate unknowns. When using one:

1. Notify the user in the same turn: what was done, why, and the proper fix.
2. State confidence.
3. Add a follow-up task in the workpad's `tasks.md` (or `project.md` backlog if
   cross-cutting).
4. Explore with review subagents for non-trivial tradeoffs.

Do not silently ship or leave workarounds undocumented.

## Core Loop

Use this loop for `/next` and similar task execution. The command prompt lives in
`.claude/commands/next.md` (mirrored in `.cursor/` and `.opencode/`).

1. Read `TASKS.md` and resolve the active workpad.
2. Load `AGENTS.md`, `project.md`, `WORKING.md`, `workpads/WORKPADS.md`.
3. Load the active workpad's `tasks.md`, `knowledge.md`, `references.md`.
4. For architecture, prototype, gpu-compute, ergonomics: also load
   `workpads/architecture/boundaries.md`.
5. For prototype and gpu-compute: also load `workpads/prototype/spec.md`.
6. Select a task by dependencies, risk, and testability. Prefer the task that
   proves the next un-proven boundary.
7. Mark it `in_progress`.
8. Complete acceptance criteria with the smallest correct change.
9. Verify per the task's evidence standard.
10. Record findings, decisions, and open questions in `knowledge.md`; sources +
    dates in `references.md`.
11. Assess confidence; use review subagents per thresholds below.
12. Incorporate review feedback, record rejections, or ask the user when
    product-sensitive (cost, public deploy, API shape).
13. Mark `completed` only when acceptance criteria and review requirements are met.
14. Before another `/next` pass: explicit commit decision — commit, or record why
    not.

## Verification

Every task needs evidence before completion. Match depth to scope:

| Change touches | Minimum verification |
| --- | --- |
| Research only | Primary/source links, dated notes, and — where the question is empirical — a small Modal spike with the exact command and result |
| Architecture docs | Boundary review, failure modes, explicit assumptions, user-sensitive decisions called out |
| Rust code | `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test` (once a Cargo workspace exists) |
| Generated Python shim | The shim runs via `modal run` / `modal deploy`; record the command and observed output |
| `run` milestone | Remote build happened at execution time (in the function body on the happy path, or a recorded Sandbox fallback if a Function-body build is infeasible); a local source edit changed the next run's output |
| `deploy` milestone | `cargo build` appears in deploy/build logs and is **absent** from call logs; deployed result is stable until redeploy |
| GPU milestone | `nvidia-smi`/CUDA evidence; verified compute result; cost noted |

Record skipped verification in the task or `knowledge.md` with a reason.

## Workpad Gates

1. **Research gate:** `workpads/research/knowledge.md` records, with evidence,
   whether runtime compile works on a normal Function, whether `copy=False`
   mount + Cargo cache make dev iteration tolerable, how much deploy/invoke
   `modal-rs` exposes, and the GPU/CUDA facts — enough to commit to the
   architecture.
2. **Architecture gate:** `workpads/architecture/boundaries.md` records the crate
   layout, runner protocol + registry API, the run-vs-deploy build boundary, the
   generated shim design, the CLI surface, and the cache design.
3. **Prototype gate:** `workpads/prototype/knowledge.md` records `add` running via
   `modal-rust run` and `modal-rust call`, with the build boundary proven
   (runtime compile for run; no runtime compile for deployed call).
4. **GPU gate:** `workpads/gpu-compute/knowledge.md` records a verified Rust GPU
   compute result (Burn-free first), then a Burn tensor smoke.

Unless `TASKS.md` Notes override, do not start a phase before its gate's
prerequisite gate has passed.

## Confidence Assessment

| Level | Meaning | Expected action |
| --- | --- | --- |
| High | Strong evidence; narrow, verified scope | Proceed; periodic review on important deliverables |
| Medium | Likely correct but assumptions or weak tests | Prefer focused review before completion |
| Low | Unclear requirements, fragile integration, weak sources | Review or user direction before completing |

Consider: acceptance met, smoke/spike evidence, cohesive boundaries, Modal cost,
public-deploy implications, provider lock-in, and the run-vs-deploy invariant.

## Review Subagents

Spawn when work is substantial, boundary-defining, cost-incurring, or confidence
is below high. Useful lenses:

- Boundary fit: the build-boundary invariant holds (run = build-at-exec, deploy =
  build-at-image-time, deployed runtime never runs cargo) whether the build runs
  in a Function body or a Sandbox; direct-execution-first, with a Sandbox used
  only as a recorded fallback (not banned, not the default).
- Protocol stability: runner contract unchanged; registry stays macro-compatible
  and static-dispatch (`fn`-pointer `HandlerFn`, no `Box<dyn>`).
- Modal correctness: image/volume/gpu semantics match the docs; cost is sane.
- Test/smoke adequacy: the smoke proves the intended boundary and fails for the
  right reasons.
- Rust quality: minimal, idiomatic implementation.
- Prior art: how Modal Python and modal-rs solve the same problem.

## Acting On Feedback

- Fix clearly correct issues in scope.
- Record accepted decisions in `knowledge.md`; record rejected feedback when it
  affects future work.
- Ask the user on product tradeoffs, cost, public deploys, or API-shape changes.

## Dependency Policy

Use libraries freely (clap, serde, anyhow, tokio, cudarc/cust, etc.). Prefer
mature crates over hand-rolling. Async standardizes on tokio where needed.
Consume `modal-rs` where it suffices; fall back to generated Python / HTTPS where
it does not. Record intentional version pins in the relevant workpad.

## Research Vs Implementation

- Research: reference + small authorized spikes only (the central question is
  empirical).
- Architecture: boundary and contract definitions before broad implementation.
- Prototype: the smallest useful `add` e2e, not a complete product.
- GPU / ergonomics: start only after their prerequisite gate passes.

## CI

Once a Cargo workspace exists, GitHub Actions enforces the deterministic
unattended subset: `cargo fmt --check`, `cargo clippy --all-targets
--all-features -- -D warnings`, `cargo test --workspace`, and `git diff --check`.
Live Modal runs, deploys, and GPU smokes are opt-in and stay out of unattended CI
(they cost money and need credentials).
