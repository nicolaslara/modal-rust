export const meta = {
  name: 'modal-rust-materialize-workpads',
  description: 'Turn the validated research-synthesis into the canonical workpad files (boundaries, spec, WORKPADS index, and per-workpad tasks/knowledge/references), then consistency-check them',
  whenToUse: 'After plan-research has written workpads/architecture/research-synthesis.md, to materialize the durable workpad scaffold',
  phases: [
    { title: 'Contracts', detail: 'write boundaries.md + prototype/spec.md from the synthesis' },
    { title: 'Workpads', detail: 'write per-workpad tasks/knowledge/references + the WORKPADS index in parallel' },
    { title: 'Consistency', detail: 'critic verifies cross-references, milestone IDs, and gates' },
  ],
}

const SYN = 'workpads/architecture/research-synthesis.md'

const FORMAT = `Match the workpads style exactly.
- tasks.md: starts with "# <Title> Tasks", then "## Objective", then "## Gate" (what makes this workpad's gate pass), then one "## <ID> - <name>" section per task with "Status: pending|in_progress|blocked|completed|deferred", "Acceptance:" (bullet list of objectively checkable criteria), and "Evidence:" (bullet list of exact commands / observable outputs / file paths). Keep tasks small — one boundary each.
- knowledge.md: "# <Title> Knowledge", "## Objective", "## Gate Status" (Not passed yet), "## Decisions", "## Findings", "## Open Questions". Seed Decisions/Findings from the synthesis; leave room to append.
- references.md: "# <Title> References", "## Objective", then a markdown table | Resource | URL or path | Date observed | Notes | seeded from the synthesis's Verified Facts sources (date 2026-06-03).
Read ${SYN} for the authoritative locked decisions (including §0 Amendments), verified facts, and corrected M0..M13 milestone plan. Read project.md, AGENTS.md, WORKING.md for the rules. Do NOT contradict the synthesis or the design stances: (1) direct-execution-first — try a normal @app.function first; a Modal Sandbox is a documented fallback (not banned) if a Function-body build is infeasible; (2) the build boundary is the HARD invariant — run builds at function-exec time, deploy builds at image-build time and the deployed runtime never runs cargo (holds whether the build runs in a Function body or a Sandbox); (3) prefer static dispatch. Use the Write tool to create each file.`

const RESULT_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['files_written', 'notes'],
  properties: {
    files_written: { type: 'array', items: { type: 'string' } },
    notes: { type: 'string' },
  },
}

const CRITIC_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['consistent', 'issues'],
  properties: {
    consistent: { type: 'boolean' },
    issues: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['file', 'issue', 'severity', 'fix'],
        properties: {
          file: { type: 'string' },
          issue: { type: 'string' },
          severity: { type: 'string', enum: ['high', 'medium', 'low'] },
          fix: { type: 'string' },
        },
      },
    },
  },
}

phase('Contracts')
await parallel([
  () =>
    agent(
      `${FORMAT}\n\nWrite workpads/architecture/boundaries.md — the canonical contract doc. Include, with headings: the cargo workspace + crate layout; the runner CLI protocol (modal_runner --entrypoint/--input-json -> {"ok":true,"value":..}/{"ok":false,"error":{kind,message,details,backtrace}} with FIVE kinds decode_error|unknown_entrypoint|function_error|encode_error|panic, where function_error wraps the user error on the top-level RunnerError enum and details = serialized user error when Serialize else null); the static-dispatch Registry + typed!() API (type HandlerFn = fn(&[u8]) -> Result<Vec<u8>, RunnerError>; Registry = BTreeMap<&'static str, HandlerFn>; typed!(f) yields a monomorphized wrapper fn pointer, no Box<dyn>) with the macro-compatibility invariant ("name -> typed! wrapper -> bytes in -> bytes out"); the run-vs-deploy build boundary table; the generated Python shim design (dev_app copy=False+cargo-in-body+cache-volume, deploy_app copy=True+run_commands+baked binary, call_app Function.from_name+.remote()); the modal-rust CLI surface; and the .modalrustignore/.gitignore mounting rules. Pull every locked decision from ${SYN}.`,
      { label: 'write:boundaries', phase: 'Contracts', schema: RESULT_SCHEMA },
    ),
  () =>
    agent(
      `${FORMAT}\n\nWrite workpads/prototype/spec.md — the POC scope doc (spec.md style): Objective, Vision, "Prototype Minimum" (the add function: write a lib fn, run it remotely with a runtime build, deploy it with a build-time build, call the deployed fn and get {"sum":42}), MVP additions, Deferred, the Prototype Gate (add works via modal-rust run AND modal-rust call with the build boundary proven), and Non-Goals (no Sandboxes; no proc-macros yet; no local binary upload; deployed runtime never compiles). Ground it in ${SYN}.`,
      { label: 'write:spec', phase: 'Contracts', schema: RESULT_SCHEMA },
    ),
])
log('Contracts written: boundaries.md + prototype/spec.md')

phase('Workpads')
const WORKPADS = [
  {
    key: 'research',
    prompt: `Write workpads/research/{tasks,knowledge,references}.md. Tasks R0..R7 mirror the research dimensions and the empirical spikes from ${SYN}: R0 capture source prompt (completed — project.md); R1 Modal images copy=False/copy=True + run_commands; R2 the KEY runtime-compile feasibility spike (small live Modal Function that runs cargo build in its body — authorized spike, record exact command + result); R3 copy=False mount speed/reliability for dev iteration; R4 Cargo-cache persistence across invocations via a Volume; R5 modal-rs surface-area capability matrix (can it deploy/invoke Functions or only sandboxes?); R6 GPU/CUDA facts (driver present, nvidia-smi, gpu types, toolkit-vs-driver); R7 PyO3/maturin assessment (defer to ergonomics). Gate: enough verified findings + spikes to commit to the architecture.`,
  },
  {
    key: 'architecture',
    prompt: `Write workpads/architecture/{tasks,knowledge,references}.md (do NOT overwrite boundaries.md or research-synthesis.md). Tasks A0..A8: A0 cargo workspace + crate layout; A1 runner protocol; A2 Registry/typed() API (macro-compatible); A3 the run-vs-deploy build boundary; A4 generated shim design (dev/deploy/call); A5 CLI surface (doctor/run/deploy/call); A6 Cargo-cache design; A7 ignore rules (.modalrustignore/.gitignore); A8 architecture gate review. Each task's evidence points at the relevant section of boundaries.md. Gate: boundaries.md records the layout, protocol, build boundary, shim + CLI design, and cache design, with user-sensitive decisions called out.`,
  },
  {
    key: 'prototype',
    prompt: `Write workpads/prototype/{tasks,knowledge,references}.md (do NOT overwrite spec.md). Transcribe the corrected M0..M9 milestone plan from ${SYN} into tasks (use the milestone IDs M0..M9 as task IDs; P-style scaffold can be M0). Each task: Status pending, Acceptance, Evidence (include the exact spike commands and, for deploy tasks, the check that cargo build is in deploy logs and ABSENT from call logs). Gate: add runs via modal-rust run and modal-rust call with the build boundary proven.`,
  },
  {
    key: 'gpu-compute',
    prompt: `Write workpads/gpu-compute/{tasks,knowledge,references}.md. Transcribe M10..M13 from ${SYN}: M10 nvidia-smi from the python shim; M11 nvidia-smi from a Rust function; M12 real Rust CUDA vector add (cudarc); M13 Burn tensor smoke. Each with Acceptance + Evidence + exact --gpu spike commands. Note the cost caveat (GPU runs cost money; confirm before running) and the Burn-free-first ordering. Gate: a verified Rust GPU compute result, then a Burn smoke.`,
  },
  {
    key: 'ergonomics',
    prompt: `Write workpads/ergonomics/{tasks,knowledge,references}.md. Tasks: E1 proc-macro registry — #[modal_rust::function] expanding to inventory::submit! that compiles to the SAME Registry shape without changing the runner protocol (reference the macro-compatibility invariant in boundaries.md); E2 optional PyO3/maturin bridge to replace the subprocess boundary (generated extension crate; maturin build/develop; wheel install in image), validated as optional not required. Gate: macros produce the validated runner shape; PyO3 proven optional. Note this starts only after the prototype gate.`,
  },
]

await parallel([
  ...WORKPADS.map((w) => () =>
    agent(`${FORMAT}\n\n${w.prompt}`, { label: `write:${w.key}`, phase: 'Workpads', schema: RESULT_SCHEMA }),
  ),
  () =>
    agent(
      `${FORMAT}\n\nWrite workpads/WORKPADS.md — the workpad index. Start with a "## Current Focus" table (Workpad | Status | Description) for research/architecture/prototype/gpu-compute/ergonomics (research = Active, rest = Planned). Then one "## <workpad>" section each with Status, Objective, a "**Load:**" fenced list of the files to load (../TASKS.md, ../project.md, ../WORKING.md, the workpad's tasks/knowledge/references, plus workpads/architecture/boundaries.md for architecture/prototype/gpu-compute/ergonomics and workpads/prototype/spec.md for prototype/gpu-compute), a Quick nav, and Rules. Ground objectives in ${SYN}.`,
      { label: 'write:WORKPADS', phase: 'Workpads', schema: RESULT_SCHEMA },
    ),
])
log('Workpad files + WORKPADS index written')

phase('Consistency')
const critic = await agent(
  `You are a consistency critic for the modal-rust workpad scaffold. Read ${SYN}, project.md, AGENTS.md, WORKING.md, workpads/WORKPADS.md, workpads/architecture/boundaries.md, workpads/prototype/spec.md, and every workpads/*/tasks.md|knowledge.md|references.md. ` +
    `Check: (a) milestone IDs are consistent (M0..M13 referenced identically in prototype + gpu-compute + boundaries + WORKPADS); (b) the run-vs-deploy build boundary (the hard invariant) and the direct-execution-first / Sandbox-is-a-documented-fallback stance are stated consistently everywhere (no surviving "no Sandboxes" ban); (c) the runner protocol string is identical everywhere it appears; (d) every workpad load-list in WORKPADS.md matches the files that actually exist; (e) gates referenced in AGENTS.md/WORKING.md match the workpad gates; (f) no file contradicts the synthesis. Report each inconsistency with file, severity, and a concrete fix. Set consistent=true only if no high/medium issues remain.`,
  { label: 'consistency-critic', phase: 'Consistency', schema: CRITIC_SCHEMA },
)

log(`Consistency: ${critic.consistent ? 'clean' : `${critic.issues.length} issue(s)`}`)
return { critic }
