export const meta = {
  name: 'modal-rust-plan-research',
  description: 'Ground the modal-rust plan in verified Modal/modal-rs facts, design the architecture, adversarially review it, and synthesize a sound, sequenced plan',
  whenToUse: 'Run once at project start (or when the plan needs re-grounding) to validate the riskiest assumptions before writing workpad tasks',
  phases: [
    { title: 'Research', detail: 'parallel primary-source research over Modal images/functions/volumes/gpu/invoke, modal-rs surface, Rust GPU + PyO3' },
    { title: 'Design', detail: 'architecture + milestone-plan proposals informed by research' },
    { title: 'Review', detail: 'adversarial lenses: red-team, sequencing, modal-correctness, rust-quality' },
    { title: 'Synthesize', detail: 'consolidate into one authoritative synthesis with locked decisions + user questions' },
  ],
}

// args: { date: "YYYY-MM-DD" }  (Date.now() is unavailable in workflow scripts)
const DATE = (args && args.date) || 'unknown-date'

const RESEARCH_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['dimension', 'verified_facts', 'open_questions', 'implications_for_design', 'risks'],
  properties: {
    dimension: { type: 'string' },
    verified_facts: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['claim', 'source', 'confidence'],
        properties: {
          claim: { type: 'string' },
          source: { type: 'string', description: 'URL or doc reference; "training-knowledge" if not web-verified' },
          confidence: { type: 'string', enum: ['high', 'medium', 'low'] },
        },
      },
    },
    open_questions: { type: 'array', items: { type: 'string' } },
    implications_for_design: { type: 'array', items: { type: 'string' } },
    risks: { type: 'array', items: { type: 'string' } },
  },
}

const DESIGN_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['component', 'decisions', 'code_sketches', 'open_questions_for_user', 'risks'],
  properties: {
    component: { type: 'string' },
    decisions: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['decision', 'rationale'],
        properties: {
          decision: { type: 'string' },
          rationale: { type: 'string' },
          alternatives_rejected: { type: 'array', items: { type: 'string' } },
        },
      },
    },
    code_sketches: { type: 'array', items: { type: 'string' } },
    open_questions_for_user: { type: 'array', items: { type: 'string' } },
    risks: { type: 'array', items: { type: 'string' } },
  },
}

const MILESTONE_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['milestones', 'sequencing_rationale', 'open_questions'],
  properties: {
    milestones: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['id', 'name', 'validates', 'acceptance', 'evidence', 'depends_on'],
        properties: {
          id: { type: 'string', description: 'e.g. M0..M13' },
          name: { type: 'string' },
          validates: { type: 'string', description: 'the single boundary this milestone proves' },
          acceptance: { type: 'array', items: { type: 'string' } },
          evidence: { type: 'array', items: { type: 'string' } },
          spike_commands: { type: 'array', items: { type: 'string' } },
          depends_on: { type: 'array', items: { type: 'string' } },
          risk: { type: 'string' },
        },
      },
    },
    sequencing_rationale: { type: 'string' },
    open_questions: { type: 'array', items: { type: 'string' } },
  },
}

const REVIEW_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['lens', 'verdict', 'must_fix', 'nice_to_have'],
  properties: {
    lens: { type: 'string' },
    verdict: { type: 'string', enum: ['sound', 'sound-with-changes', 'unsound'] },
    must_fix: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['issue', 'why', 'suggested_change', 'severity'],
        properties: {
          issue: { type: 'string' },
          why: { type: 'string' },
          suggested_change: { type: 'string' },
          severity: { type: 'string', enum: ['high', 'medium', 'low'] },
        },
      },
    },
    nice_to_have: { type: 'array', items: { type: 'string' } },
    praise: { type: 'array', items: { type: 'string' } },
  },
}

const SYNTH_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['synthesis_path', 'locked_decisions', 'user_questions', 'residual_risks', 'verdict'],
  properties: {
    synthesis_path: { type: 'string' },
    locked_decisions: { type: 'array', items: { type: 'string' } },
    user_questions: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['question', 'recommendation'],
        properties: {
          question: { type: 'string' },
          options: { type: 'array', items: { type: 'string' } },
          recommendation: { type: 'string' },
        },
      },
    },
    residual_risks: { type: 'array', items: { type: 'string' } },
    verdict: { type: 'string', enum: ['plan-is-sound', 'plan-needs-user-input'] },
  },
}

const GROUND = `You are researching to ground the design of "modal-rust", a Rust-on-Modal function runtime.
Use primary sources where reachable: prefer WebFetch/WebSearch against modal.com/docs and docs.rs, and the context7 MCP (resolve-library-id then query-docs) for library docs. If web access fails, answer from training knowledge but mark source as "training-knowledge" and confidence accordingly. Record observation date ${DATE}.
Design stances constrain everything: (1) the build boundary is the HARD, non-negotiable invariant — "run" builds Rust at function-execution time (source mounted, cargo build in the function body) while "deploy" builds at image-build time and the deployed runtime executes only a prebuilt binary (never cargo); this run-vs-deploy split holds whether the build runs in a Function body or a Sandbox; (2) direct-execution-first — prove the core path on a normal @app.function (@app.function) first, and if runtime compile in a Function body proves infeasible for a step, iterate to a Modal Sandbox as a documented fallback and record it (Sandboxes are a fallback, not banned); (3) prefer static dispatch.
Return verified facts (each with a source + confidence), open questions, concrete implications for the design, and risks. Be specific and skeptical; flag anything you cannot confirm.`

const RESEARCH = [
  {
    key: 'modal-images',
    prompt: `${GROUND}\n\nDIMENSION: Modal Images — local file inclusion and build steps.\nNail down: add_local_dir / add_local_file semantics; copy=False (available at container startup, NOT bakeable by later build steps) vs copy=True (copied into an image layer so later run_commands can use them); how .run_commands build steps chain; ignore patterns; image layer caching/rebuild triggers; from_registry + add_python for non-Python base images; whether a Rust toolchain is best installed via rustup in run_commands or via a rust: base image with add_python. This dimension decides the run(copy=False)/deploy(copy=True) split.`,
  },
  {
    key: 'modal-functions-runtime',
    prompt: `${GROUND}\n\nDIMENSION: Modal Functions at runtime — can a normal @app.function compile Rust in its body?\nNail down: do Modal Functions require Python in the image (and does add_python satisfy that for a rust base image)?; can the function body run subprocess (e.g. cargo build, then exec a binary)?; is the container filesystem writable (where does add_local_dir(copy=False) land, and can cargo write target/ there or must it target a Volume?); function timeout limits for long builds; passing input as a string arg and returning a string; this is THE central feasibility question (runtime compile in a normal Function body on the happy path — and, if that proves infeasible, whether a Modal Sandbox is the documented fallback build environment).`,
  },
  {
    key: 'modal-volumes-cache',
    prompt: `${GROUND}\n\nDIMENSION: Modal Volumes for a Cargo cache across Function invocations.\nNail down: Volume.from_name(create_if_missing=True); mounting a Volume on a Function; whether writes persist across invocations and how commit/reload works; setting CARGO_HOME and CARGO_TARGET_DIR onto a mounted volume path; concurrency/locking caveats with parallel containers writing the same volume; whether incremental cargo builds actually benefit. This decides the dev-iteration caching design (added only AFTER the uncached path works).`,
  },
  {
    key: 'modal-gpu-cuda',
    prompt: `${GROUND}\n\nDIMENSION: GPU + CUDA on Modal for Rust.\nNail down: the gpu= parameter and currently supported GPU families (T4/L4/A10/L40S/A100/H100/H200/B200) and the string syntax; whether the NVIDIA driver + CUDA Driver API are preinstalled on GPU functions (so nvidia-smi works) vs needing the CUDA toolkit/nvcc for COMPILING kernels; what a Rust crate (cudarc vs cust) needs at runtime (driver API via libcuda) vs build time; what Burn's CUDA backend (cubecl) requires. Order the GPU proof: nvidia-smi (python) -> nvidia-smi (rust) -> cudarc vector add -> Burn.`,
  },
  {
    key: 'modal-invoke-deploy',
    prompt: `${GROUND}\n\nDIMENSION: Deploying and invoking Modal Functions.\nNail down: modal deploy vs modal run vs ephemeral apps; how a deployed Function is looked up and called from OUTSIDE (Python client Function.from_name + .remote(); HTTPS/web endpoints for non-Python callers; @modal.web_server for a long-lived Rust HTTP server); autoscaling knobs (min_containers/max_containers/scaledown_window) and how they map to a modal-rust deploy. Compare "Mode A: generated Python function shim that subprocess-execs the Rust runner" vs "Mode B: Rust HTTP server behind web_server" — recommend Mode A for the add POC.`,
  },
  {
    key: 'modal-rs-surface',
    prompt: `${GROUND}\n\nDIMENSION: the modal-rs Rust SDK surface area (docs.rs/modal-rs + its GitHub repo).\nNail down precisely WHAT IT EXPOSES today: app create/lookup; sandboxes; images; Volumes; FUNCTION creation/deploy; FUNCTION invocation of deployed functions; credential loading from ~/.modal.toml / env; gRPC vs REST; version, last release date, maintainer, maturity/stability. The key product question: can modal-rust use modal-rs to DEPLOY and INVOKE Functions, or only sandboxes — i.e. how much must fall back to generated Python? Give a capability matrix.`,
  },
  {
    key: 'rust-gpu-and-pyo3',
    prompt: `${GROUND}\n\nDIMENSION: Rust GPU crates + the later PyO3/maturin bridge.\nNail down: cudarc vs cust current maturity, what a minimal vector-add needs (PTX from nvcc? or driver-API-only?), and runtime libs; Burn + CUDA backend (cubecl) feature flags and deps; separately, PyO3 (extension-module) + maturin (develop vs build, wheel install into a Modal image) for a future tighter Python<->Rust bridge that replaces the subprocess. Confirm: subprocess-first for v0, PyO3 later. Flag native-dependency drift risks.`,
  },
]

phase('Research')
const research = (
  await parallel(
    RESEARCH.map((r) => () =>
      agent(r.prompt, { label: `research:${r.key}`, phase: 'Research', schema: RESEARCH_SCHEMA }),
    ),
  )
).filter(Boolean)

const researchDigest = JSON.stringify(research, null, 2)
log(`Research complete: ${research.length}/${RESEARCH.length} dimensions returned`)

phase('Design')
const [architecture, milestonePlan] = await parallel([
  () =>
    agent(
      `Design the modal-rust ARCHITECTURE, grounded in this research digest:\n${researchDigest}\n\n` +
        `Produce concrete decisions (with rationale + rejected alternatives) for: (1) the cargo workspace + crate layout (modal-rust-runtime / modal-rust-cli / modal-rust-client / modal-rust-macros placeholder; examples/add); (2) the runner CLI protocol — modal_runner --entrypoint <name> --input-json <json> -> {"ok":true,"value":..} or {"ok":false,"error":{kind,message,details,backtrace}} with kinds decode_error|unknown_entrypoint|function_error|encode_error|panic, where function_error WRAPS the user error on the top-level RunnerError enum (message = Display/anyhow chain, details = the serialized user error when the handler's error type is Serialize, else null) and details is an additive optional field; (3) the static-dispatch Registry + typed!() API: type HandlerFn = fn(&[u8]) -> Result<Vec<u8>, RunnerError>; Registry = BTreeMap<&'static str, HandlerFn>; typed!(f) is a macro_rules! that yields a monomorphized wrapper fn pointer (no Box<dyn>, no vtable) and owns decode/call/encode; built via Registry::new().function("add", typed!(add)); designed so a future #[modal_rust::function] proc-macro generates the SAME wrapper + an inventory registration (or a static match table) without changing the protocol; typed_async! reserved with the same fn-pointer shape; (4) the generated Python shim design for dev_app (add_local_dir copy=False + cargo build in function body + exec runner + Cargo-cache Volume), deploy_app (add_local_dir copy=True + run_commands cargo build + bake /app/modal_runner + runtime execs only the binary), and call_app (Function.from_name + .remote()); (5) the modal-rust CLI surface (doctor [--rust] / run / deploy / call) and how it generates+invokes the shims via modal run / modal deploy, using modal-rs where the research says it suffices. Include short code sketches. Surface genuinely product-sensitive open questions for the user (cost, public deploys, default invoke mode, JSON vs msgpack).`,
      { label: 'design:architecture', phase: 'Design', schema: DESIGN_SCHEMA },
    ),
  () =>
    agent(
      `Design the modal-rust MILESTONE PLAN (M0..M13), grounded in this research digest:\n${researchDigest}\n\n` +
        `The method is: validate ONE boundary per milestone, direct-execution-first (normal @app.function on the happy path; a Modal Sandbox is a documented fallback if a Function-body build is infeasible), with the HARD build boundary of build-at-run-time for dev vs build-at-deploy-time for prod (deployed runtime never runs cargo). Use this skeleton and sharpen each with precise acceptance criteria, evidence, and the exact spike commands: ` +
        `M0 local dispatcher (add returns 42 locally; unknown entrypoint -> structured error); ` +
        `M1 generated Modal Function runs a shell command (uname) — control path, no Rust; ` +
        `M2 source mount copy=False (find /workspace; local==remote Cargo.toml hash); ` +
        `M3 rust toolchain image (cargo --version in the function); ` +
        `M4 RUNTIME COMPILE in a normal Function body (cargo build in function body, exec runner, add->42) — the key validation; on the happy path no Sandbox is used, but add a fallback branch: if the Function-body build is infeasible, evaluate + record a Sandbox-based build rather than declaring failure; ` +
        `M5 source-edit reactivity (42 -> edit -> 43 -> revert -> 42); ` +
        `M6 Cargo-cache Volume (full build, then incremental); ` +
        `M7 deploy-time build (copy=True + run_commands cargo build, bake /app/modal_runner); ` +
        `M8 deployed runtime does NOT compile (cargo build in deploy logs, absent from call logs; 42 stable until redeploy then 43); ` +
        `M9 modal-rust CLI wraps the shims (run/deploy/call); ` +
        `M10 GPU nvidia-smi from the python shim; ` +
        `M11 nvidia-smi from a Rust function; ` +
        `M12 real Rust CUDA vector add (cudarc); ` +
        `M13 Burn tensor smoke. ` +
        `For each milestone give depends_on and the single boundary it validates. Call out any sequencing flaw you find in this skeleton and propose the fix.`,
      { label: 'design:milestones', phase: 'Design', schema: MILESTONE_SCHEMA },
    ),
])

const designDigest = JSON.stringify({ architecture, milestonePlan }, null, 2)
log('Design complete: architecture + milestone plan drafted')

phase('Review')
const LENSES = [
  {
    key: 'red-team',
    prompt: `RED-TEAM the modal-rust design + milestone plan. What will actually FAIL on Modal? Find hidden assumptions, especially around: runtime cargo build in a normal @app.function body on the happy path (filesystem writability, where source lands, timeouts) and whether the documented Modal Sandbox fallback is viable if a Function-body build is infeasible, copy=False vs copy=True correctness, the Cargo-cache volume across parallel containers, deployed-call invocation from Rust (modal-rs gaps), and GPU native-dep drift. Confirm the HARD build boundary (run=build-at-exec, deploy=build-at-image-time, deployed runtime never runs cargo) holds regardless of whether the build runs in a Function body or a Sandbox. Default to skepticism. List concrete must_fix items with suggested changes.`,
  },
  {
    key: 'sequencing',
    prompt: `Review the MILESTONE SEQUENCING. Does each milestone isolate exactly one new boundary? Is anything mis-ordered, fused, or skippable? Is the run-vs-deploy boundary proven before GPU? Is caching correctly placed AFTER the uncached correctness path? Are M0..M13 dependencies right? Propose a corrected ordering if needed.`,
  },
  {
    key: 'modal-correctness',
    prompt: `Check the design against the VERIFIED MODAL FACTS in this research digest:\n${researchDigest}\n\nFlag any place the architecture or milestones contradict or over-assume Modal semantics (images, functions, volumes, gpu, deploy/invoke). Quote the relevant fact. List must_fix where the design is wrong or unverified-but-assumed.`,
  },
  {
    key: 'rust-quality',
    prompt: `Review the RUST runtime contract + static-dispatch Registry/typed!() API for idiomatic quality and, critically, MACRO-COMPATIBILITY: will a future #[modal_rust::function] (inventory-based) proc-macro compile to this exact registry shape (HandlerFn = fn(&[u8]) -> Result<Vec<u8>, RunnerError>; Registry = BTreeMap<&'static str, HandlerFn>; typed!(f) yielding a monomorphized wrapper fn pointer, no Box<dyn>/vtable) WITHOUT changing the runner protocol? Check error/panic capture (catch_unwind across the FFI-free boundary) and the function_error wrapping of the user error on the top-level RunnerError enum (message from Display/anyhow chain, additive optional details = serialized user error when Serialize else null), serde codec choice, async support via the reserved typed_async! (same fn-pointer shape), and multi-arg vs single-input-struct. List must_fix.`,
  },
]

const reviews = (
  await parallel(
    LENSES.map((l) => () =>
      agent(`${l.prompt}\n\nDESIGN UNDER REVIEW:\n${designDigest}`, {
        label: `review:${l.key}`,
        phase: 'Review',
        schema: REVIEW_SCHEMA,
      }),
    ),
  )
).filter(Boolean)

const reviewDigest = JSON.stringify(reviews, null, 2)
const verdicts = reviews.map((r) => `${r.lens}: ${r.verdict}`).join('; ')
log(`Review complete: ${verdicts}`)

phase('Synthesize')
const synthesis = await agent(
  `You are the lead synthesizer for the modal-rust plan. Consolidate everything into ONE authoritative document and WRITE it to workpads/architecture/research-synthesis.md using the Write tool.\n\n` +
    `RESEARCH:\n${researchDigest}\n\nDESIGN:\n${designDigest}\n\nREVIEWS:\n${reviewDigest}\n\n` +
    `The synthesis document must contain, with clear headings: (1) a Verified Facts table (claim | source | confidence) covering images/functions/volumes/gpu/invoke/modal-rs; (2) Locked Architecture Decisions (crate layout, runner protocol, registry API, run-vs-deploy shim design, CLI surface) — fold in every high-severity must_fix from the reviews and note what changed; (3) the corrected Milestone Plan M0..M13 with per-milestone validates/acceptance/evidence/spike_commands/depends_on; (4) Open Questions For The User with a recommended default for each; (5) Residual Risks. ` +
    `Resolve review conflicts explicitly. Mark the plan "plan-is-sound" only if no high-severity must_fix remains unaddressed; otherwise "plan-needs-user-input". Return the structured summary (synthesis_path = workpads/architecture/research-synthesis.md, locked_decisions, user_questions, residual_risks, verdict).`,
  { label: 'synthesize', phase: 'Synthesize', schema: SYNTH_SCHEMA },
)

log(`Synthesis verdict: ${synthesis.verdict}; written to ${synthesis.synthesis_path}`)
return { research, architecture, milestonePlan, reviews, synthesis }
