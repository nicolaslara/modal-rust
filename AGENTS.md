# AGENTS.md

Repository for **modal-rust**, a Rust-on-Modal function runtime (CLI +
runtime/client crates). Progress persists in files and git, not conversation
context.

## Source Of Truth

| File | Role |
| --- | --- |
| `TASKS.md` | User-edited workpad queue: which phase to work in |
| `project.md` | Product goal, architecture stance, runtime contract, phases, backlog |
| `WORKING.md` | Agent loop, gates, verification, review thresholds |
| `workpads/WORKPADS.md` | Per-workpad load lists and objectives |
| `workpads/architecture/boundaries.md` | Crate layout, runner protocol, run-vs-deploy boundary, shim/CLI contracts |
| `workpads/{workpad}/tasks.md` | Executable tasks with acceptance criteria + evidence |
| `workpads/{workpad}/knowledge.md` | Decisions, findings, open questions |
| `workpads/{workpad}/references.md` | External/local research with dates |
| `workpads/prototype/spec.md` | POC scope: the `add` function e2e, minimum and non-goals |
| `.claude/commands/next.md` (+ `.cursor`/`.opencode`) | `/next` task-execution command |
| `.claude/workflows/refine-plan.js` | Multi-agent workflow: adversarially refine a workpad plan until sound |
| `.claude/workflows/implement.js` | Multi-agent workflow: implement + verify the next milestone task |

## Resolve Active Workpad

1. Read `TASKS.md`; the first unchecked workpad is active unless Notes override it.
2. Confirm status, objective, and load list in `workpads/WORKPADS.md`.
3. Gate check:
   - `architecture` requires the research gate passed (or `TASKS.md` authorizes
     parallel discovery).
   - `prototype` requires the architecture gate passed (or `TASKS.md` authorizes
     a spike).
   - `gpu-compute` requires the prototype gate passed: `add` runs via
     `modal-rust run` **and** the run-vs-deploy build boundary is proven.
   - `ergonomics` requires the prototype gate passed; macros must not change the
     runner protocol.

## Current Phase

**architecture** is active as of 2026-06-03. The multi-agent planning workflow
(`.claude/workflows/plan-research.js`) already produced the doc-level research and
the reviewed design in `workpads/architecture/research-synthesis.md`, so
`research` is checked off at the doc level in `TASKS.md`. The architecture phase
now **ratifies** the contract doc `workpads/architecture/boundaries.md`
section-by-section (A0–A8) and passes the gate.

The critical open question remains empirical: **can a normal Modal Function mount
Rust source and compile it at runtime on the happy path (no Sandbox)?** It is
confirmed *in principle* from primary docs and is *empirically* confirmed in
prototype M2/M4 (the mount-writability probe + the runtime-compile spike), which
run real Modal calls. M4 carries a fallback branch: if the Function-body build is
infeasible, evaluate + record a Sandbox-based build rather than declaring failure
(stance 1). Spikes that make real Modal calls are authorized; keep them small and
record evidence.

The intended sequence is `research -> architecture -> prototype (add e2e) ->
gpu-compute -> ergonomics`, validating one boundary per step.

## Mandatory Workflow

Before task work:

1. `TASKS.md` -> active workpad.
2. `project.md`, `WORKING.md`, `workpads/WORKPADS.md`.
3. Active workpad `tasks.md`, `knowledge.md`, `references.md`.
4. `workpads/architecture/boundaries.md` for architecture, prototype,
   gpu-compute, and ergonomics work.
5. `workpads/prototype/spec.md` for prototype and gpu-compute work.
6. Pick a pending task; mark it `in_progress`.
7. Complete the acceptance criteria with the smallest correct change.
8. Record findings in `knowledge.md` and source links/dates in `references.md`.
9. Review per `WORKING.md`.
10. Mark complete only after evidence is recorded.

## Design Stances

These come from the source prompt and override convenience (the build boundary is
the hard invariant; direct-execution-first with a Sandbox fallback; prefer static
dispatch):

1. **Direct-execution-first; Sandbox is a documented fallback.** Try the core path
   on normal Modal Functions (`@app.function`) FIRST — runtime compile in a
   Function body is the central thing to prove (prototype M4). If runtime compile
   in a Function body proves infeasible for a step, **iterate to a Modal Sandbox**
   for that step and record the decision. Sandboxes are a fallback explicitly on
   the table, not out of scope.
2. **The build boundary is the product** *(the hard, non-negotiable invariant).*
   `run` builds Rust at function-execution time (`add_local_dir(copy=False)` +
   `cargo build` in the function body — or a Sandbox if that proves necessary).
   `deploy` builds at image-build time (`add_local_dir(copy=True)` +
   `run_commands(cargo build)`) and the deployed runtime executes only the
   prebuilt `/app/modal_runner` — it must never run `cargo`. This run-vs-deploy
   split holds whether the build runs in a Function body or a Sandbox. Every deploy
   task must prove `cargo build` appears in deploy/build logs and **not** in call
   logs.
3. **Prefer static dispatch.** Favor compile-time polymorphism — `enum`
   (closed-world), generics (`T: Trait`) / `impl Trait` (monomorphization),
   marker/type-state, `cfg` features — over `dyn Trait`. Reach for `dyn` only when
   the set of implementations is genuinely open/unbounded. (The handler registry is
   the one open set; it erases user functions to `fn` pointers, not `Box<dyn>`.)

## Runner Protocol (do not break)

```text
/app/modal_runner --entrypoint <name> ( --input-json <json> | --input-file <path> | --input-stdin )
ok:    {"ok":true,"value":{...}}
error: {"ok":false,"error":{"kind":"decode_error|unknown_entrypoint|function_error|encode_error|panic","message":"...","details":<serialized user error|null>,"backtrace":"..."}}
```

stdout carries **exactly one** JSON envelope; cargo/rustc/user diagnostics go to
stderr; exit code mirrors `ok`. The error enum is frozen at **five** kinds
(`encode_error` keeps an output-serialization failure from masquerading as a
`panic`). `function_error` is the **user error wrapped** on the top-level
`RunnerError` enum: `message` = Display/anyhow chain, optional additive `details`
= the serialized user error when the handler's error type is `Serialize` (else
`null`). Manual registry now (`Registry::new().function("add", typed!(add))`),
where `type HandlerFn = fn(&[u8]) -> Result<Vec<u8>, RunnerError>` and `Registry =
BTreeMap<&'static str, HandlerFn>`; `typed!(f)` is a `macro_rules!` yielding a
monomorphized wrapper `fn` pointer (no `dyn`, no vtable, no `Box`). Proc-macros
later must compile to the same wrapper (+ an `inventory` registration or a static
`match` table). The codec is neutral on bytes and `typed_async!` is reserved with
the same `fn`-pointer shape, so CBOR/async/macros stay additive. The runner
contract is the stable seam — guard it across every change. See
`workpads/architecture/boundaries.md` §2–§3 for the frozen detail.

## Git Rules

- Do not commit or push without explicit user confirmation.
- If asked to commit, show files and message first.
- No destructive git commands unless explicitly requested.
- Keep generated shims, research clones, and scratch artifacts under gitignored
  paths (`.modal-rust/`, `target/`, `tmp/`).

## Modal / Secrets Rules

- Never log or commit Modal tokens, `~/.modal.toml` contents, API keys, or
  workspace credentials.
- Treat the generated Python shims under `.modal-rust/generated/` as
  disposable, regenerable artifacts; do not hand-edit them as a source of truth.
- Real Modal spikes cost money and run remotely (outward-facing). Keep them
  small, opt-in, and record what was run. Confirm before any deploy that creates
  persistent apps.
- Prefer cheap/CPU validation before GPU; GPU runs incur higher cost.

## Research Rules

- Prefer primary sources: Modal docs, modal-rs docs.rs/repo, PyO3/maturin docs,
  CUDA-crate docs. Modal's API and pricing change; record observation dates.
- Separate proven facts (verified by a spike or primary doc) from assumptions and
  recommendations.
- When a spike answers an open question, record the exact command and the result.

## Verification

**Research:** cited URLs/local paths, dated notes, spike command + result,
recommendation confidence, open questions.

**Architecture:** boundary/contract definitions, failure modes, acceptance
criteria, user-sensitive decisions called out.

**Implementation (once a Cargo workspace exists):** `cargo fmt --check`,
`cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, plus
the milestone's manual smoke (local dispatch, remote run, deploy/call, or GPU as
applicable). Record skipped verification with a reason.
