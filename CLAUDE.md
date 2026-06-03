# CLAUDE.md

Claude should use `AGENTS.md` as the main entrypoint for this repository.

## Required Startup

1. Read `AGENTS.md`.
2. Resolve the active workpad from `TASKS.md`.
3. Load the required files listed in `AGENTS.md` and `workpads/WORKPADS.md`.
4. Follow the mandatory workflow, the two hard design stances, the runner
   protocol, git/Modal/secrets rules, and verification rules from `AGENTS.md`.

## Source Of Truth

- `AGENTS.md` is the orchestration brain and primary instruction surface.
- `TASKS.md` determines the active workpad.
- `WORKING.md` defines the execution loop and evidence expectations.
- `workpads/WORKPADS.md` defines per-workpad context.
- `workpads/architecture/boundaries.md` defines the runner protocol, crate
  layout, and the run-vs-deploy build boundary.
- Active workpad files define the task acceptance criteria and evidence.

Do not invent a separate Claude-specific workflow. If this file conflicts with
`AGENTS.md`, follow `AGENTS.md` and update this file later only if needed.

## Multi-Agent Workflows

Two saved workflows live in `.claude/workflows/` (run with the `Workflow` tool;
they require explicit user opt-in because they spawn many agents):

- `refine-plan.js` — adversarially stress-test and refine a workpad's `tasks.md`
  across several lenses until the plan is sound.
- `implement.js` — pick the next milestone task, implement the smallest correct
  change, and adversarially verify it before marking complete.
