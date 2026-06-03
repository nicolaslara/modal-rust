# Next: Do Next Task

Follow the modal-rust workpads methodology to complete the next task.

## Step 1: Read State Files

Read these first:

1. `TASKS.md` — active workpad queue and notes
2. `AGENTS.md`
3. `project.md`
4. `WORKING.md`
5. `workpads/WORKPADS.md`
6. `workpads/{active-workpad}/tasks.md`
7. `workpads/{active-workpad}/knowledge.md`
8. `workpads/{active-workpad}/references.md`
9. `workpads/architecture/boundaries.md` — when active workpad is `architecture`,
   `prototype`, `gpu-compute`, or `ergonomics`
10. `workpads/prototype/spec.md` — when active workpad is `prototype` or
    `gpu-compute`

## Step 2: Resolve Active Workpad

- The active workpad is the first unchecked item in `TASKS.md`, unless Notes
  override it.
- Confirm its objective in `workpads/WORKPADS.md` and its `tasks.md`.
- Do not skip gates just because a later task looks more concrete.

## Step 3: Gate Check

- `architecture` requires the research gate passed (or `TASKS.md` authorizes
  parallel discovery).
- `prototype` requires the architecture gate passed (or `TASKS.md` authorizes a
  spike).
- `gpu-compute` requires the prototype gate passed: `add` runs via `modal-rust
  run` and the run-vs-deploy build boundary is proven.
- `ergonomics` requires the prototype gate passed; macros must not change the
  runner protocol.

## Step 4: Select A Task

Choose a pending/unblocked task by dependencies, current state, risk, and
testability. **Prefer the task that proves the next un-proven boundary.** This
project validates one boundary at a time — do not jump ahead.

## Step 5: Execute

1. Mark the task `in_progress`.
2. Complete the acceptance criteria with the smallest correct change.
3. Update `references.md` with sources, local paths, and dates (and, for an
   empirical research question, the exact spike command + result).
4. Update `knowledge.md` with decisions, findings, confidence, rejected options,
   and open questions.
5. Update `tasks.md` with follow-ups discovered during the work.
6. Assess confidence per `WORKING.md`.
7. Spawn focused review subagents when the work is substantial, boundary-defining,
   cost-incurring, or confidence is below high. (Or run the `refine-plan` /
   `implement` workflow in `.claude/workflows/`.)
8. Apply review feedback, record rejected feedback, or ask the user when
   product-sensitive (cost, public deploy, API shape).
9. Mark `completed` only when acceptance criteria and review requirements are met.
10. Make an explicit commit decision before another `/next` pass.

## Rules

- The product prompt captured in `project.md` is the source of truth when docs
  conflict.
- Honor the design stances: **direct-execution-first** — prove the core path on a
  normal `@app.function` first; a Modal Sandbox is a documented fallback (not
  banned) if a Function-body build proves infeasible. The **build boundary is the
  hard invariant** (holds either way): `run` builds at function-execution time,
  `deploy` builds at image-build time and the deployed runtime never runs `cargo`.
  **Prefer static dispatch:** favor `enum`/generics/`impl Trait` over `dyn Trait`;
  reach for `dyn` only for genuinely open sets.
- Do not break the runner protocol; keep the registry macro-compatible and
  static-dispatch.
- Do not log or commit Modal tokens, `~/.modal.toml`, or API keys.
- Real Modal spikes/deploys cost money and run remotely — keep them small, and
  confirm before any persistent or public deploy.
- Do not commit without explicit user confirmation.
- If evidence is weak, record uncertainty instead of guessing.

Start now.
