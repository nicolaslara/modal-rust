export const meta = {
  name: 'modal-rust-refine-plan',
  description: 'Adversarially stress-test and refine a workpad tasks.md until the plan is sound, across multiple lenses, looping until reviewers stop finding material issues',
  whenToUse: 'When a workpad plan (tasks.md) needs hardening before implementation: pass the workpad name as args (e.g. "prototype")',
  phases: [
    { title: 'Load', detail: 'read the workpad plan + project context' },
    { title: 'Critique', detail: 'parallel adversarial lenses find material issues' },
    { title: 'Revise', detail: 'apply accepted fixes and re-critique until dry' },
    { title: 'Finalize', detail: 'write the refined tasks.md and a changelog' },
  ],
}

// args: "prototype"  OR  { workpad: "prototype", maxRounds: 3 }
const WORKPAD = typeof args === 'string' ? args : (args && args.workpad) || 'prototype'
const MAX_ROUNDS = (args && args.maxRounds) || 3
const TASKS_PATH = `workpads/${WORKPAD}/tasks.md`

const CRITIQUE_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['lens', 'verdict', 'issues'],
  properties: {
    lens: { type: 'string' },
    verdict: { type: 'string', enum: ['sound', 'sound-with-changes', 'unsound'] },
    issues: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['issue', 'why', 'fix', 'severity'],
        properties: {
          issue: { type: 'string' },
          why: { type: 'string' },
          fix: { type: 'string' },
          severity: { type: 'string', enum: ['high', 'medium', 'low'] },
        },
      },
    },
  },
}

const REVISE_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['applied', 'rejected', 'summary'],
  properties: {
    applied: { type: 'array', items: { type: 'string' } },
    rejected: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['issue', 'reason'],
        properties: { issue: { type: 'string' }, reason: { type: 'string' } },
      },
    },
    summary: { type: 'string' },
  },
}

const CONTEXT = `Project: modal-rust, a Rust-on-Modal function runtime. Read project.md, WORKING.md, and workpads/architecture/boundaries.md (and workpads/architecture/research-synthesis.md if present) for the rules. Design stances: (1) the build boundary is the HARD, non-negotiable invariant — "run" builds Rust at function-execution time, "deploy" builds at image-build time and the deployed runtime never runs cargo (this holds whether the build runs in a Function body or a Sandbox); (2) direct-execution-first — try a normal @app.function first, and if runtime compile in a Function body is infeasible for a step, iterate to a Modal Sandbox as a documented fallback and record it (Sandboxes are a fallback, not banned); (3) prefer static dispatch. The method is to validate ONE boundary per task. The runner protocol (modal_runner --entrypoint/--input-json -> {"ok":true,"value":..} | {"ok":false,"error":{kind,message,details,backtrace}}; function_error wraps the user error on the top-level RunnerError enum, details = serialized user error when Serialize else null) and the macro-compatible Registry (BTreeMap<&'static str, HandlerFn> with HandlerFn = fn(&[u8]) -> Result<Vec<u8>, RunnerError> built via typed!(f), no Box<dyn>) must not be broken.`

const LENSES = [
  {
    key: 'boundary-isolation',
    prompt: `Read ${TASKS_PATH}. Does each task isolate exactly one new boundary/assumption? Find tasks that fuse two validations, skip a boundary, or depend on something not yet proven. Check the run-vs-deploy build placement is correct (build-at-exec for run, build-at-image-time for deploy, deployed runtime never runs cargo). The core path is direct-execution-first on a normal @app.function; a Modal Sandbox is a documented fallback (not banned) if a Function-body build proves infeasible — flag tasks that declare failure instead of recording a Sandbox fallback.`,
  },
  {
    key: 'acceptance-evidence',
    prompt: `Read ${TASKS_PATH}. Is every task's acceptance criteria objectively checkable, and does each have concrete evidence (exact commands / observable outputs)? Flag vague acceptance, missing evidence, or evidence that wouldn't actually prove the boundary (e.g. a deploy task that doesn't check cargo is absent from call logs).`,
  },
  {
    key: 'red-team',
    prompt: `Read ${TASKS_PATH}. Red-team it: what will fail on real Modal? Hidden assumptions about filesystem writability, copy semantics, volume caching, deployed invocation via modal-rs, GPU native deps, cost. What ordering will cause a late expensive surprise?`,
  },
  {
    key: 'dependency-order',
    prompt: `Read ${TASKS_PATH}. Check task IDs, dependencies, and ordering. Is anything out of order, missing a dependency, or blocking work that should come first? Is the cheapest/riskiest validation done before expensive (GPU/deploy) work?`,
  },
]

phase('Load')
log(`Refining ${TASKS_PATH} (up to ${MAX_ROUNDS} rounds)`)

let round = 0
let dry = false
const history = []

while (round < MAX_ROUNDS && !dry) {
  round++
  phase(`Critique r${round}`)
  const critiques = (
    await parallel(
      LENSES.map((l) => () =>
        agent(`${CONTEXT}\n\n${l.prompt}\n\nReturn your verdict and a list of material issues (with a concrete fix and severity each). Only report issues that genuinely matter; do not invent nits.`, {
          label: `critique:${l.key}:r${round}`,
          phase: `Critique r${round}`,
          schema: CRITIQUE_SCHEMA,
        }),
      ),
    )
  ).filter(Boolean)

  const material = critiques.flatMap((c) => c.issues.filter((i) => i.severity !== 'low'))
  log(`Round ${round}: ${material.length} material issue(s) across ${critiques.length} lenses`)
  history.push({ round, critiques })

  if (material.length === 0) {
    dry = true
    break
  }

  phase(`Revise r${round}`)
  const revision = await agent(
    `${CONTEXT}\n\nRevise ${TASKS_PATH} IN PLACE using the Edit/Write tools to address these reviewer issues. Apply the clearly-correct fixes; reject any that conflict with the project's hard stances or the runner protocol (record why). Preserve the file's existing structure, task-ID scheme, and formatting (Objective / Gate / per-task Status + Acceptance + Evidence). Issues:\n${JSON.stringify(material, null, 2)}`,
    { label: `revise:r${round}`, phase: `Revise r${round}`, schema: REVISE_SCHEMA },
  )
  log(`Round ${round} revision: ${revision.summary}`)
  history.push({ round, revision })
}

phase('Finalize')
const final = await agent(
  `${CONTEXT}\n\nDo a final read of ${TASKS_PATH} and confirm it is internally consistent (task IDs, dependencies, gate). Then write/append a short "Plan Refinement Log" section at the BOTTOM of ${TASKS_PATH} summarizing this refinement pass (rounds run, key changes applied, anything rejected). Keep it concise. Confirm the plan is now sound or list what remains.`,
  { label: 'finalize', phase: 'Finalize' },
)

return { workpad: WORKPAD, rounds: round, converged: dry, finalNote: final, history }
