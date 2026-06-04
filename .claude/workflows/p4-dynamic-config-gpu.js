export const meta = {
  name: 'p4-dynamic-config-gpu',
  description: 'P4: per-function config from the decorator — #[modal_rust::function(gpu="T4", timeout=…, cache=…)] flows into FunctionCreate.resources.gpu_config + timeout at runtime (the decorator IS the config, like Modal Python); drop the legacy --gpu CLI flag; prove a real GPU function via the facade .remote() on a T4.',
  phases: [
    { title: 'Design', detail: 'macro args + additive Registration config + SDK gpu_config/Resources + facade wiring -> one spec' },
    { title: 'Implement', detail: 'macro gpu/timeout/cache + Registration FunctionConfig + SDK gpu_config + facade reads config; drop CLI --gpu; gates green (HARD GATE)' },
    { title: 'Live', detail: '#[modal_rust::function(gpu="T4")] GPU fn via App.remote() on a T4 -> real result; FunctionCreate.resources.gpu_config verified' },
    { title: 'Review', detail: 'parallel: config flow (decorator->registration->resources, bare macro unchanged) + GPU/proto correctness + hygiene' },
  ],
}

const ROOT = '/Users/nicolas/devel/modal-rust'

const SHARED = [
  'You are implementing P4 of modal-rust (repo root: ' + ROOT + '; git on main).',
  '',
  '## Where we are',
  'The run/deploy/call triad is proven live via our own first-party SDK (crates/modal-rust-sdk, no modal-rs) + the',
  'facade (crates/modal-rust). `.local()`/`.remote()`/`App::deploy`/`App::call` all return {sum:42} for CPU. The',
  'image uses add_python (matches the official client) and the upload is cargo-scoped. What is MISSING: per-function',
  'config (gpu/timeout/cache) is NOT yet sourced from the code — there is no way to say "this function needs a GPU".',
  '',
  '## What P4 builds: the decorator IS the config (like Modal Python)',
  '`#[modal_rust::function(gpu="T4", timeout=1800, cache=false)]` -> that config flows, at runtime, into the',
  '`FunctionCreate` request (`resources.gpu_config`, `timeout_secs`, …) when the facade creates the function. No',
  'static pre-parse, no CLI flag — the Rust registry is the source of truth. Concretely:',
  '  1. **Macro** (crates/modal-rust-macros): accept OPTIONAL args `gpu="…"`, `timeout=<int secs>`, `cache=<bool>` on',
  '     `#[modal_rust::function(...)]`. The bare `#[modal_rust::function]` form MUST stay byte-identical (backward-',
  '     compatible). The macro emits the parsed config into the inventory registration.',
  '  2. **Registration** (crates/modal-rust-runtime): extend the inventory `Registration` ADDITIVELY with an optional',
  '     `FunctionConfig { gpu: Option<String>, timeout_secs: Option<u32>, cache: Option<bool> }` (default all None).',
  '     This is METADATA only — the runner CLI protocol, the `HandlerFn` signature, `typed!()`, and the runner\'s',
  '     `Registry::from_inventory()` dispatch are FROZEN and must be byte-identical. The runner IGNORES config; only',
  '     the control-plane (facade) reads it. (This additive extension was explicitly anticipated — knowledge.md.)',
  '  3. **SDK** (crates/modal-rust-sdk): `FunctionResources` / `FunctionSpec.to_proto` must populate',
  '     `Resources.gpu_config` (proto `GPUConfig { count, gpu_type }`) from a parsed GPU spec, plus `timeout_secs`.',
  '     A GPU spec string like "T4", "A100", "A100-80GB", "H100:4" (`:N` = count) maps the way Modal does. A GPU',
  '     LIST (fallback ranking) routing through `FunctionData.ranked_functions` is an OPTIONAL stretch — single-GPU',
  '     is the required path.',
  '  4. **Facade** (crates/modal-rust): `App::from_inventory()` must capture per-name `FunctionConfig` (alongside the',
  '     handler `Registry`); when `.remote()` / `deploy` create the function, read that config and set',
  '     `FunctionSpec.with_resources(<gpu>)` + `.with_timeout(<secs>)`. The manual `App::new(registry)` path has no',
  '     decorator config (defaults apply) — the decorator/from_inventory path is where config lives.',
  '  5. **CLI** (crates/modal-rust-cli): DROP the legacy `--gpu` flag + any static gpu parse (config is dynamic from',
  '     the decorator now). Secondary/cheap; the CLI is the legacy path anyway.',
  '',
  '## Ground-truth references (READ; never depend on — references/ is gitignored)',
  '- crates/modal-rust-macros/src/lib.rs — the current `#[modal_rust::function]` proc-macro (attr parsing + the',
  '  `inventory::submit!` it emits).',
  '- crates/modal-rust-runtime/src/lib.rs — `Registration`, the `inventory` wiring, `Registry`, `Registry::from_inventory`,',
  '  `typed!`, `HandlerFn`, `run_cli` (the FROZEN runner protocol — do NOT change its behavior).',
  '- crates/modal-rust/src/{app.rs (from_inventory + how the registry is built), function.rs, remote.rs (ensure_function',
  '  builds the FunctionSpec), deploy.rs (deploy FunctionSpec)}.',
  '- crates/modal-rust-sdk/src/ops/function.rs — `FunctionResources`, `FunctionSpec`, `to_proto` (resources), the',
  '  existing `with_resources`/`with_timeout`; crates/modal-rust-sdk/proto/api.proto — `Resources`, `GPUConfig`,',
  '  `FunctionData.ranked_functions`.',
  '- references/modal-client/py/modal/_functions.py — `convert_fn_config_to_resources_config` (gpu/cpu/memory ->',
  '  Resources) + how a GPU LIST routes through `ranked_functions`; and the GPU-spec parsing (`parse_gpu_config` /',
  '  the "TYPE[:count]" + "-MEM" format). Mirror the gpu_type/count mapping faithfully.',
  '- examples/cuda-vector-add/{src/lib.rs,Cargo.toml} — the GPU live target (cudarc dynamic-loading: builds WITHOUT a',
  '  CUDA toolkit, runs on a T4 via the driver + a precompiled PTX; already in default-members).',
  '- workpads/shim-backend/knowledge.md, TASKS.md (status + the P4 line).',
  '',
  '## FROZEN invariants — do NOT change',
  '- The runner CLI protocol (`modal_runner --entrypoint … --input-…`, one JSON envelope, five error kinds), the',
  '  `HandlerFn` signature, `typed!()`, and the runner\'s `Registry::from_inventory()` DISPATCH must be byte-identical.',
  '  You ADD optional config METADATA to `Registration` (which the runner ignores) — nothing about how functions run',
  '  changes. The bare `#[modal_rust::function]` must expand the same as today.',
  '- The run-vs-deploy build boundary; retry_transient on all RPCs; the add_python image + cargo-scoped upload.',
  '- Do NOT touch README.md or examples/orchestrate (recently landed). examples/add is fine to read but avoid churn.',
  '',
  '## Verification rules (WORKING.md)',
  '- Gates on default-members: cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo',
  '  test. Keep no-CUDA CI green (cuda-vector-add uses dynamic-loading -> builds without CUDA; keep it that way).',
  '  Live tests behind #[ignore] + the live feature. Reuse the existing live-test patterns + retry_transient.',
  '- Modal flakiness => RETRY. DRIVE the live GPU proof to a terminal result (do NOT punt to a background monitor).',
  '  Use a CHEAP T4 only. Use an ephemeral app (run path) so it does not leave a persistent deploy.',
  '',
  '## How to return',
  'End with: "RESULT: <STATUS> — <one-line>". Build phases STATUS in {BUILD_GREEN, BUILD_FAILED} + exact cargo output.',
  'Live: the decoded GPU result + proof gpu_config was set (request fields / server side), or the precise error after retries.',
].join('\n')

phase('Design')
const design = await parallel([
  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Design / macro args + additive Registration config + facade read). Read crates/modal-rust-macros/src/',
    'lib.rs, crates/modal-rust-runtime/src/lib.rs (Registration/inventory/Registry/from_inventory), and crates/',
    'modal-rust/src/app.rs (from_inventory). Produce a PRECISE spec for:',
    '- Extending `#[modal_rust::function]` to parse OPTIONAL `gpu="…"`, `timeout=<int>`, `cache=<bool>` args (syn',
    '  attribute parsing), backward-compatible: bare `#[modal_rust::function]` and `#[modal_rust::function(name="…")]`',
    '  unchanged. What the macro emits into `inventory::submit!` so the config travels with the registration.',
    '- The ADDITIVE `FunctionConfig { gpu: Option<String>, timeout_secs: Option<u32>, cache: Option<bool> }` on the',
    '  runtime `Registration` (default None). PROVE the runner side is untouched: `Registry::from_inventory()` still',
    '  builds name->HandlerFn and dispatch is byte-identical; the config is ignored by the runner.',
    '- How the facade `App::from_inventory()` captures per-name `FunctionConfig` (a name->config map on `App`, built',
    '  from the same inventory iteration that builds the Registry) so `.remote()`/`deploy` can read it. What happens on',
    '  the manual `App::new(registry)` path (no config -> defaults).',
    'Cite file:line. Keep the runner FROZEN. RESULT: SPEC_DONE — macro + Registration config + facade-read spec',
  ].join('\n'), { phase: 'Design', label: 'design:macro-config' }),

  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Design / SDK gpu_config + Resources + facade wiring). Read crates/modal-rust-sdk/src/ops/function.rs',
    '(FunctionResources/FunctionSpec/to_proto/with_resources/with_timeout), crates/modal-rust-sdk/proto/api.proto',
    '(Resources, GPUConfig, FunctionData.ranked_functions), references/modal-client/py/modal/_functions.py',
    '(convert_fn_config_to_resources_config + gpu-list/ranked_functions + the GPU spec parsing), and crates/modal-rust/',
    'src/{remote.rs,deploy.rs} (where FunctionSpec is built). Produce a PRECISE spec for:',
    '- Adding GPU support to `FunctionResources` -> `Resources.gpu_config` (proto `GPUConfig { count, gpu_type }`):',
    '  parse a GPU spec string "TYPE", "TYPE:count", "TYPE-MEM" the way Modal does (gpu_type uppercased? count default',
    '  1?) — mirror `convert_fn_config_to_resources_config`/`parse_gpu_config`. Plus `timeout_secs` (already on',
    '  FunctionSpec). A GPU LIST -> `ranked_functions` is an OPTIONAL stretch; single-GPU is required.',
    '- How `.remote()` (remote.rs ensure_function) and `deploy` (deploy.rs) read the per-function `FunctionConfig`',
    '  (from the facade map, other design task) and set `FunctionSpec.with_resources(<from gpu>)` + `.with_timeout(<secs>)`.',
    '- Confirm the exact proto field path (`Function.resources` field number, `Resources.gpu_config`, `GPUConfig`',
    '  fields) and that resources stays ALWAYS-set (the fix-#1 invariant).',
    'Cite file:line + proto fields + the Python mapping. RESULT: SPEC_DONE — SDK gpu_config + facade-wiring spec',
  ].join('\n'), { phase: 'Design', label: 'design:sdk-gpu' }),
])

const spec = await agent(SHARED + '\n\n' + [
  'YOUR TASK (Synthesize). Merge the two notes into ONE build-ready spec and WRITE it to',
  ROOT + '/workpads/shim-backend/p4-build-spec.md (overwrite if present): the macro args, the additive Registration',
  'FunctionConfig (runner frozen), the facade per-name config capture + read, the SDK gpu_config/Resources mapping',
  '(GPU spec parsing mirroring Modal), the run+deploy FunctionSpec wiring, and dropping the CLI --gpu flag. Note which',
  'files change. Resolve contradictions (prefer what Modal Python actually does for gpu_config). Keep it tight.',
  '',
  '=== MACRO + REGISTRATION + FACADE-READ NOTE ===',
  (design[0] || '(missing)'),
  '',
  '=== SDK GPU_CONFIG + FACADE-WIRING NOTE ===',
  (design[1] || '(missing)'),
  '',
  'RESULT: SPEC_DONE — wrote p4-build-spec.md',
].join('\n'), { phase: 'Design', label: 'design:synthesize' })

phase('Implement')
const impl = await agent(SHARED + '\n\n' + [
  'RESUME NOTE: a prior run died on a TRANSIENT infra socket error before any work was done — the Design phase + the',
  'spec are complete; implement them now. (If the spawn blips again, that is infra, not a real failure.)',
  'The spec is at ' + ROOT + '/workpads/shim-backend/p4-build-spec.md — READ IT FIRST.',
  '',
  'YOUR TASK (Implement P4 — HARD GATE on offline gates). Per the spec:',
  '1. Macro: add optional `gpu`/`timeout`/`cache` args to `#[modal_rust::function(...)]`, backward-compatible; emit the',
  '   config into the inventory registration. The bare form must expand identically (add a test asserting it).',
  '2. Runtime: add the additive `FunctionConfig` to `Registration` (default None). The runner `Registry::from_inventory`',
  '   dispatch + `HandlerFn` + `typed!` + the runner protocol MUST be byte-identical (verify a runner test still passes).',
  '3. SDK: `FunctionResources`/`to_proto` populate `Resources.gpu_config` from a parsed GPU spec (mirror Modal) +',
  '   `timeout_secs`; resources stays always-set.',
  '4. Facade: `App::from_inventory()` captures per-name `FunctionConfig`; `.remote()` (remote.rs) + `deploy` (deploy.rs)',
  '   set `FunctionSpec.with_resources(...)` + `.with_timeout(...)` from it.',
  '5. CLI: remove the `--gpu` flag + static gpu parse from crates/modal-rust-cli.',
  'Do NOT change the runner protocol/HandlerFn/typed!() or the build boundary. Do NOT touch README.md / examples/orchestrate.',
  'VERIFY (offline hard): cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test',
  '(all default-members) — all green. Paste exact output. Include a test proving bare #[modal_rust::function] is unchanged',
  'AND a test that `gpu="T4"` produces a GPUConfig with gpu_type T4 (count 1).',
  'RESULT: BUILD_GREEN — decorator gpu/timeout/cache -> FunctionCreate.resources; --gpu flag dropped; bare macro unchanged',
  '   (or BUILD_FAILED — <reason>)',
].join('\n'), { phase: 'Implement', label: 'p4-impl' })

const implGreen = /RESULT:\s*BUILD_GREEN/i.test(impl || '')
let live = null
if (!implGreen) {
  log('Implement HARD GATE not green — P4 did not compile. Skipping Live; Review documents the blocker.')
} else {
  phase('Live')
  live = await agent(SHARED + '\n\n' + [
    'P4 is implemented and compiles. Run the LIVE GPU proof and DRIVE IT TO A TERMINAL RESULT yourself (do NOT punt to a',
    'background monitor).',
    '',
    'YOUR TASK (LIVE — a real GPU function via the facade). Prove the decorator GPU config reaches Modal and runs on a',
    'GPU. Approach: make a real GPU function carry `#[modal_rust::function(gpu="T4")]` and invoke it through the facade.',
    'Use examples/cuda-vector-add (cudarc dynamic-loading — builds WITHOUT a CUDA toolkit, runs on a T4 via the driver +',
    'precompiled PTX). Convert/extend its GPU entrypoint to the macro path so `App::from_inventory()` carries the gpu',
    'config (the decorator config flows only via the inventory path). Then a live test (behind #[ignore] + the live',
    'feature): `App::connect(...)` (or connect_with_registry as appropriate) -> `.function("<gpu fn>").remote(input)',
    '.await?` runs on Modal with `gpu="T4"`. Keep cuda-vector-add building locally without CUDA (dynamic-loading).',
    'Use a CHEAP T4 and an EPHEMERAL app (no lingering deploy). Modal flakiness => RETRY (the retry_transient layer +',
    'image caching help). If a real bug surfaces, make the MINIMAL fix + re-verify offline gates.',
    'Capture: the decoded GPU result; PROOF that `FunctionCreate` sent `resources.gpu_config` with gpu_type T4 (the',
    'request fields, or server-side confirmation the container had a GPU, e.g. the function ran cudarc on the device);',
    'and that the bare-CPU `.remote()` still works (no GPU regression).',
    'RESULT: BUILD_GREEN — live GPU .remote() ran on a T4 with gpu_config from the decorator   (or BUILD_FAILED/INFRA_BLOCKED — <detail>)',
  ].join('\n'), { phase: 'Live', label: 'live-gpu' })
}

phase('Review')
const reviews = await parallel([
  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / config flow + frozen runner). Verify against the code:',
    '- The decorator config flows end-to-end: `#[modal_rust::function(gpu=…,timeout=…)]` -> inventory `Registration.`',
    '  `FunctionConfig` -> facade per-name map -> `FunctionSpec.with_resources/with_timeout` -> `FunctionCreate.resources`',
    '  `.gpu_config`/`timeout_secs`. Quote the chain.',
    '- The runner is FROZEN: `Registry::from_inventory` dispatch, `HandlerFn`, `typed!()`, the runner CLI protocol are',
    '  byte-identical; config is metadata the runner ignores. The bare `#[modal_rust::function]` expands unchanged',
    '  (point to the test). resources stays ALWAYS-set.',
    '- The `--gpu` CLI flag + static parse are removed.',
    'RESULT: PASS — config flows from the decorator, runner unchanged  (or FAIL — <what is wrong>)',
  ].join('\n'), { phase: 'Review', label: 'review:config-flow' }),

  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / GPU + proto correctness). Verify the GPU spec -> proto mapping matches Modal',
    '(references/modal-client/py/modal/_functions.py convert_fn_config_to_resources_config + the gpu-spec parsing):',
    '`Resources.gpu_config` is a `GPUConfig { gpu_type, count }`; "T4" -> gpu_type "T4" count 1; "TYPE:N" -> count N;',
    'memory variants ("A100-80GB") handled or explicitly deferred. A GPU list -> ranked_functions only if implemented',
    '(else single-GPU only, documented). Quote the mapping code + the proto fields. RESULT: PASS — gpu_config matches Modal',
    '(or FAIL — <what is wrong>)',
  ].join('\n'), { phase: 'Review', label: 'review:gpu-proto' }),

  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / hygiene — RUN the gates). From ' + ROOT + ' report exact output + exit status:',
    '- cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test  (default-members).',
    'Confirm cuda-vector-add still builds WITHOUT CUDA (dynamic-loading) and stays in default-members; example-burn-add',
    'still excluded; live tests #[ignore]+live gated; README.md + examples/orchestrate untouched by this work; no',
    'hand-written file grossly exceeds ~500 LOC. Report failures verbatim.',
    'RESULT: PASS — gates green  (or FAIL — <exact failing command + output>)',
  ].join('\n'), { phase: 'Review', label: 'review:hygiene' }),
])

return { impl_green: implGreen, impl, live, reviews }
