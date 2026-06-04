export const meta = {
  name: 'spawn-map',
  description: 'Implement Function::spawn (fire-and-forget -> FunctionCall handle -> .get()) and Function::map (fan-out: N inputs -> N ordered outputs) on the facade, reusing the proven FunctionMap/FunctionPutInputs/FunctionGetOutputs invoke path. Replace the NotImplemented stubs; prove live.',
  phases: [
    { title: 'Design', detail: 'spawn/map/get semantics + SDK invoke reuse + ordering -> one spec' },
    { title: 'Implement', detail: 'SDK map/spawn/get + facade Function::spawn/map + FunctionCall::get; gates green (HARD GATE)' },
    { title: 'Live', detail: 'map([...]) fan-out -> N ordered results; spawn()->get()->result (CPU add, ephemeral)' },
    { title: 'Review', detail: 'parallel: semantics match Modal + ordering correct + frozen invariants; hygiene' },
  ],
}

const ROOT = '/Users/nicolas/devel/modal-rust'

const SHARED = [
  'You are implementing Function::spawn / Function::map on modal-rust (repo root: ' + ROOT + '; git on main).',
  '',
  '## Where we are',
  'The full product is proven live via our own SDK (no modal-rs) + facade: .local()/.remote()/deploy/call work for',
  'CPU and GPU; the CLI is programmatic; `#[modal_rust::function(gpu=…,timeout=…,cache=…)]` config flows into',
  'FunctionCreate (P4). `.remote()` enqueues ONE input and polls one output via the SDK invoke path. What is missing:',
  '`Function::spawn` and `Function::map` (and `FunctionCall::get`) — currently `Error::NotImplemented` stubs',
  '(crates/modal-rust/src/function.rs).',
  '',
  '## What this builds (Modal semantics)',
  '- `Function::spawn(input) -> FunctionCall`: FIRE-AND-FORGET — enqueue the input, return a handle carrying the',
  '  function_call_id IMMEDIATELY (do not wait). `FunctionCall::get() -> Out` later polls FunctionGetOutputs for that',
  '  call and decodes the result.',
  '- `Function::map(inputs: I) -> Vec<Out>`: FAN-OUT — enqueue N inputs, collect N outputs **in input order**',
  '  (Modal returns outputs tagged with their input index; reassemble by index). Runs across containers in parallel.',
  'Reuse the PROVEN invoke RPCs — do not invent a new path. The SDK already has FunctionMap (pipelined_inputs),',
  'FunctionPutInputs (the fallback), and FunctionGetOutputs (the poll) in crates/modal-rust-sdk/src/ops/invoke.rs;',
  '`.remote()` is the single-input case. Extend that to N inputs (map) + a spawn (enqueue, return call_id) + get-by-call.',
  '',
  '## Ground-truth references (READ)',
  '- crates/modal-rust/src/function.rs (the spawn/map/FunctionCall::get NotImplemented stubs + the signatures to fill)',
  '  + crates/modal-rust/src/app.rs (remote_invoke / remote_envelope / ensure_function reuse).',
  '- crates/modal-rust-sdk/src/ops/invoke.rs (FunctionMap, FunctionPutInputs, FunctionGetOutputs, the poll loop,',
  '  invoke_cbor / invoke with deadline) + the CBOR codec.',
  '- crates/modal-rust-sdk/proto/api.proto (FunctionMapRequest, FunctionPutInputsRequest, FunctionGetOutputsRequest,',
  '  FunctionInput, FunctionCallType, the output item idx/index field for ordering).',
  '- references/modal-client/py/modal/_functions.py (the real .map/.starmap/.spawn semantics: function_call_type,',
  '  input numbering/ordering, how outputs are matched to inputs).',
  '- workpads/shim-backend/knowledge.md, TASKS.md.',
  '',
  '## FROZEN invariants — do NOT change',
  '- The runner protocol / HandlerFn / typed! / Registry dispatch; the run-vs-deploy build boundary; retry_transient',
  '  on all RPCs; ephemeral-run vs persistent-deploy; the add_python / CUDA image paths; FunctionConfig/decorator.',
  '- Do NOT rewrite the working .remote()/deploy/call logic; ADD spawn/map/get, reusing ensure_function + the invoke RPCs.',
  '- Do NOT touch README.md (the maintainer owns it; the orchestrator updates docs separately) or examples/orchestrate',
  '  beyond what is strictly needed for a test.',
  '',
  '## Verification rules (WORKING.md)',
  '- Gates on default-members: cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test',
  '  — all green. Live tests behind #[ignore] + the live feature. retry_transient on all RPCs. Modal flakiness => RETRY.',
  '- DRIVE the live proof to a terminal result. Use the CPU `add` function (fast build), small N for map (e.g. 3-5),',
  '  ephemeral run app (no lingering deploy), cheap.',
  '',
  '## How to return',
  'End with: "RESULT: <STATUS> — <one-line>". Build phases STATUS in {BUILD_GREEN, BUILD_FAILED} + exact cargo output.',
  'Live: the map fan-out outputs (in order) + the spawn->get result, or the precise error after retries.',
].join('\n')

phase('Design')
const spec = await agent(SHARED + '\n\n' + [
  'YOUR TASK (Design spawn/map/get). Read crates/modal-rust/src/{function.rs,app.rs}, crates/modal-rust-sdk/src/ops/',
  'invoke.rs, the proto invoke messages, and references/modal-client/py/modal/_functions.py (map/spawn semantics).',
  'Then WRITE a build-ready spec to ' + ROOT + '/workpads/shim-backend/spawn-map-spec.md covering:',
  '- SDK: the new invoke surface — `map`/`spawn`/`get_by_call` (or equivalent) reusing FunctionMap/PutInputs/',
  '  GetOutputs. How N inputs are enqueued (indices) and how outputs are matched back to input order; how spawn',
  '  returns a function_call_id; how get polls outputs for a given call_id + decodes via CBOR. The function_call_type',
  '  (map/spawn) per the Python SDK.',
  '- Facade: `Function::spawn(input) -> Result<FunctionCall>` (ensure_function first, then enqueue, return handle',
  '  immediately), `FunctionCall::get() -> Result<Out>` (poll+decode), `Function::map(inputs) -> Result<Vec<Out>>`',
  '  (fan-out, ordered). What `FunctionCall` carries (client handle + function_call_id). Reuse ensure_function/the',
  '  per-name config so spawn/map respect the decorator gpu/timeout.',
  '- The live test plan (CPU add, ephemeral, small N).',
  'Cite file:line + proto fields. RESULT: SPEC_DONE — wrote spawn-map-spec.md',
].join('\n'), { phase: 'Design', label: 'design:spawn-map' })

phase('Implement')
const impl = await agent(SHARED + '\n\n' + [
  'The spec is at ' + ROOT + '/workpads/shim-backend/spawn-map-spec.md — READ IT FIRST.',
  '',
  'YOUR TASK (Implement spawn/map/get — HARD GATE on offline gates). Per the spec: add the SDK map/spawn/get-by-call',
  'support (reusing FunctionMap/FunctionPutInputs/FunctionGetOutputs + the CBOR codec + retry_transient), then',
  'implement `Function::spawn`/`Function::map`/`FunctionCall::get` in the facade (replace the NotImplemented stubs),',
  'reusing ensure_function + the per-name FunctionConfig. map returns outputs in INPUT ORDER; spawn returns the handle',
  'immediately. Do NOT rewrite the working .remote()/deploy/call logic; do NOT touch README.md.',
  'VERIFY (offline hard): cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test',
  '(all default-members) — all green. Add unit tests (ordering reassembly; the handle shape). Paste exact output.',
  'RESULT: BUILD_GREEN — Function::spawn/map + FunctionCall::get implemented; gates green   (or BUILD_FAILED — <reason>)',
].join('\n'), { phase: 'Implement', label: 'spawn-map-impl' })

const implGreen = /RESULT:\s*BUILD_GREEN/i.test(impl || '')
let live = null
if (!implGreen) {
  log('Implement HARD GATE not green — spawn/map did not compile. Skipping Live; Review documents the blocker.')
} else {
  phase('Live')
  live = await agent(SHARED + '\n\n' + [
    'spawn/map/get are implemented and compile. Run the LIVE proof and DRIVE IT TO A TERMINAL RESULT yourself.',
    '',
    'YOUR TASK (LIVE — fan-out + fire-and-forget against REAL Modal). Using the CPU `add` function (ephemeral app):',
    '  1. `Function::map` over a small list of inputs (e.g. [{a:1,b:1},{a:2,b:2},{a:3,b:3},{a:40,b:2}]) -> the N',
    '     outputs IN ORDER ([2,4,6,42]). Prove ordering is correct (not just the set).',
    '  2. `Function::spawn(input)` -> a FunctionCall handle returned immediately, then `.get().await?` -> the result.',
    'Behind #[ignore] + the live feature. Modal flakiness => RETRY (retry_transient + image caching help). If a real',
    'bug surfaces, make the MINIMAL fix + re-verify offline gates. Cheap CPU, ephemeral (no lingering deploy).',
    'Capture: the ordered map outputs + the spawn->get result.',
    'RESULT: BUILD_GREEN — live map (ordered) + spawn->get == expected   (or BUILD_FAILED/INFRA_BLOCKED — <detail>)',
  ].join('\n'), { phase: 'Live', label: 'live-spawn-map' })
}

phase('Review')
const reviews = await parallel([
  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / semantics + ordering + frozen). Verify against the code + references/modal-client _functions.py:',
    '- spawn returns the handle WITHOUT waiting (fire-and-forget); FunctionCall::get polls + decodes for that call_id.',
    '- map enqueues N inputs and returns outputs IN INPUT ORDER (reassembled by index, not arrival order) — quote the',
    '  reordering code + the unit test. Errors in any input surface correctly.',
    '- Reuses the proven FunctionMap/PutInputs/GetOutputs + CBOR + retry_transient + ensure_function (decorator config',
    '  respected); the run/deploy/.remote() logic + runner protocol are unchanged. README.md untouched.',
    'RESULT: PASS — spawn/map semantics + ordering correct, invariants intact  (or FAIL — <what is wrong>)',
  ].join('\n'), { phase: 'Review', label: 'review:semantics' }),

  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / hygiene — RUN the gates). From ' + ROOT + ' report exact output + exit status:',
    '- cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test  (default-members).',
    'Confirm example-burn-add still excluded from default-members; live tests #[ignore]+live gated; README.md untouched;',
    'no hand-written file grossly exceeds ~500 LOC. Report failures verbatim.',
    'RESULT: PASS — gates green  (or FAIL — <exact failing command + output>)',
  ].join('\n'), { phase: 'Review', label: 'review:hygiene' }),
])

return { impl_green: implGreen, impl, live, reviews }
