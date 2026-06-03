export const meta = {
  name: 'modal-rust-implement',
  description: 'Implement the next pending milestone task in the active workpad with the smallest correct change, then adversarially verify it before marking complete',
  whenToUse: 'To execute the next task in a workpad end-to-end: pass the workpad name (and optionally a task id) as args',
  phases: [
    { title: 'Select', detail: 'resolve active workpad + pick the next pending task' },
    { title: 'Implement', detail: 'smallest correct change satisfying acceptance criteria' },
    { title: 'Verify', detail: 'run verification + adversarial check that the boundary is truly proven' },
    { title: 'Record', detail: 'update knowledge/references/tasks and report' },
  ],
}

// args: "prototype"  OR  { workpad: "prototype", task: "M4" }
const WORKPAD = typeof args === 'string' ? args : (args && args.workpad) || null
const TASK_HINT = (args && args.task) || null

const SELECT_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['workpad', 'task_id', 'task_title', 'boundary', 'acceptance', 'evidence', 'plan'],
  properties: {
    workpad: { type: 'string' },
    task_id: { type: 'string' },
    task_title: { type: 'string' },
    boundary: { type: 'string', description: 'the single boundary this task validates' },
    acceptance: { type: 'array', items: { type: 'string' } },
    evidence: { type: 'array', items: { type: 'string' } },
    plan: { type: 'array', items: { type: 'string' }, description: 'ordered implementation steps' },
    blocked_reason: { type: 'string' },
  },
}

const IMPL_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['files_changed', 'commands_run', 'acceptance_status', 'notes'],
  properties: {
    files_changed: { type: 'array', items: { type: 'string' } },
    commands_run: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['command', 'ok'],
        properties: { command: { type: 'string' }, ok: { type: 'boolean' }, output: { type: 'string' } },
      },
    },
    acceptance_status: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['criterion', 'met'],
        properties: { criterion: { type: 'string' }, met: { type: 'boolean' }, note: { type: 'string' } },
      },
    },
    notes: { type: 'string' },
  },
}

const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['boundary_proven', 'findings', 'verdict'],
  properties: {
    boundary_proven: { type: 'boolean' },
    findings: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['finding', 'severity'],
        properties: { finding: { type: 'string' }, severity: { type: 'string', enum: ['high', 'medium', 'low'] } },
      },
    },
    verdict: { type: 'string', enum: ['complete', 'needs-rework'] },
  },
}

const CONTEXT = `Project: modal-rust, a Rust-on-Modal function runtime. Honor project.md, AGENTS.md, WORKING.md, and workpads/architecture/boundaries.md. Design stances: (1) the build boundary is the HARD, non-negotiable invariant — "run" builds at function-execution time, "deploy" builds at image-build time (deployed runtime never runs cargo), and this holds whether the build runs in a Function body or a Sandbox; (2) direct-execution-first — prove the core path on a normal @app.function first, and if runtime compile in a Function body is infeasible for a step, iterate to a Modal Sandbox as a documented, recorded fallback (Sandboxes are a fallback, not banned); (3) prefer static dispatch. Keep the runner protocol intact (modal_runner --entrypoint/--input-json -> {"ok":true,"value":..} | {"ok":false,"error":{kind,message,details,backtrace}}; function_error wraps the user error on the top-level RunnerError enum, details = serialized user error when Serialize else null) and the macro-compatible Registry intact (BTreeMap<&'static str, HandlerFn> with HandlerFn = fn(&[u8]) -> Result<Vec<u8>, RunnerError> built via typed!(f), no Box<dyn>). Make the SMALLEST correct change that proves the one boundary. Verification once a Cargo workspace exists: cargo fmt --check; cargo clippy --all-targets --all-features -- -D warnings; cargo test. Real Modal spikes cost money and run remotely — if a task requires a live Modal/GPU/deploy call, do the local/CPU parts, then STOP and report exactly the command for the user to run rather than spending or deploying without confirmation.`

phase('Select')
const selection = await agent(
  `${CONTEXT}\n\nResolve the active workpad${WORKPAD ? ` (use "${WORKPAD}")` : ' from TASKS.md (first unchecked, honoring Notes overrides)'} and read its tasks.md. ` +
    `${TASK_HINT ? `Select task "${TASK_HINT}".` : 'Select the next pending/unblocked task that proves the next un-proven boundary.'} ` +
    `Return the task, the single boundary it validates, its acceptance criteria, its evidence requirements, and an ordered implementation plan. If it is blocked, set blocked_reason.`,
  { label: 'select', phase: 'Select', schema: SELECT_SCHEMA },
)

if (selection.blocked_reason) {
  log(`Blocked: ${selection.blocked_reason}`)
  return { blocked: true, selection }
}
log(`Implementing ${selection.task_id}: ${selection.task_title} — proves: ${selection.boundary}`)

phase('Implement')
const impl = await agent(
  `${CONTEXT}\n\nImplement ${selection.workpad} task ${selection.task_id} (${selection.task_title}). Boundary: ${selection.boundary}. ` +
    `Acceptance:\n${selection.acceptance.map((a) => `- ${a}`).join('\n')}\nRequired evidence:\n${selection.evidence.map((e) => `- ${e}`).join('\n')}\n` +
    `Plan:\n${selection.plan.map((p, i) => `${i + 1}. ${p}`).join('\n')}\n\n` +
    `Write the code/files and run the local verification commands you can run without a live Modal spend. Report files changed, commands run (with ok/output), per-criterion acceptance status, and notes. If a live Modal/GPU/deploy step is required to fully satisfy acceptance, implement everything up to it and clearly note the exact command left for the user.`,
  { label: `implement:${selection.task_id}`, phase: 'Implement', schema: IMPL_SCHEMA },
)
log(`Implemented ${selection.task_id}: ${impl.files_changed.length} file(s) changed`)

phase('Verify')
const verify = await agent(
  `${CONTEXT}\n\nAdversarially VERIFY that ${selection.workpad} task ${selection.task_id} actually proves its boundary: "${selection.boundary}". ` +
    `Do NOT trust the implementer's self-report — re-read the changed files (${impl.files_changed.join(', ')}) and re-run/inspect the evidence yourself where possible. ` +
    `Check the boundary is truly isolated and proven, the runner protocol/registry is intact (static-dispatch HandlerFn registry, function_error wrapping with details), and the build boundary holds (run=build-at-exec, deploy=build-at-image-time, deployed runtime never runs cargo). The core path should be direct-execution-first on a normal @app.function; a Modal Sandbox is an acceptable, documented fallback only if a Function-body build was infeasible and the decision was recorded — do not fail a task merely for using the Sandbox fallback. Implementer notes:\n${impl.notes}\nAcceptance self-report:\n${JSON.stringify(impl.acceptance_status, null, 2)}\n` +
    `Return whether the boundary is proven, any findings (with severity), and a verdict.`,
  { label: `verify:${selection.task_id}`, phase: 'Verify', schema: VERIFY_SCHEMA },
)

phase('Record')
const record = await agent(
  `${CONTEXT}\n\nUpdate the workpad records for ${selection.workpad} task ${selection.task_id} using Edit/Write: set the task Status in workpads/${selection.workpad}/tasks.md (completed only if the verifier verdict is "complete"; otherwise leave in_progress with a follow-up note), append decisions/findings/open-questions to workpads/${selection.workpad}/knowledge.md, and add any new sources+dates to workpads/${selection.workpad}/references.md. ` +
    `Verifier verdict: ${verify.verdict}; boundary_proven: ${verify.boundary_proven}. Findings:\n${JSON.stringify(verify.findings, null, 2)}\n` +
    `Return a one-paragraph status summary and an explicit commit recommendation (commit / hold + why).`,
  { label: 'record', phase: 'Record' },
)

return { selection, impl, verify, record }
