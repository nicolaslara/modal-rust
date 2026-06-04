export const meta = {
  name: 'remote-live-resilience',
  description: 'Make real .remote() live-green: exclude references/ (+junk) from the source upload, add retry_transient_errors to ALL control-plane unary RPCs (Modal SDK pattern), optionally add_python to shrink the image build; then prove app.function("add").remote()=={sum:42} live',
  phases: [
    { title: 'Design', detail: 'read grpc_utils retry_transient_errors + modal-rs + our call sites -> retry+ignore spec' },
    { title: 'Implement', detail: 'retry_transient helper on all unary RPCs + fix upload ignore (references/) + optional add_python; gates green (HARD GATE)' },
    { title: 'Live', detail: 'app.function("add").remote(AddInput{40,2}).await == {sum:42} live (retry past Modal flakiness)' },
    { title: 'Review', detail: 'parallel: retry masks only transient (not real errors) + run build-boundary intact; hygiene' },
  ],
}

const ROOT = '/Users/nicolas/devel/modal-rust'

const SHARED = [
  'You are fixing the live .remote() path of modal-rust (repo root: ' + ROOT + '; git on main).',
  '',
  '## Context: real .remote() is CODE-COMPLETE + offline-green, but the LIVE run keeps failing',
  'The run-path .remote() (crates/modal-rust/src/remote.rs + crates/modal-rust-sdk) compiles and passes all',
  'offline gates, and the upload primitive + FILE-mode create were each proven live in isolation. But the full',
  'live app.function("add").remote(AddInput{a:40,b:2}) -> {sum:42} has NOT succeeded: every attempt dies on a',
  'transient transport reset (hyper ConnectionReset / "h2 protocol error") and the test\'s outer 4x retry restarts',
  'the WHOLE sequence (re-upload + rebuild) so it never converges. DIAGNOSIS (verified) — TWO real bugs:',
  '',
  '  BUG 1 — the source upload includes references/. ensure_function (crates/modal-rust/src/remote.rs ~line 206)',
  '  calls client.mount_local_dir(&config.local_root, &config.remote_src, &ignore, None) where config.local_root',
  '  is the WORKSPACE ROOT and the default ignore is [target, .git, .modal-rust, **/*.rlib]. That ignore does NOT',
  '  exclude references/ — which holds TWO full clones (references/modal-rs + references/modal-client, many MB of',
  '  Go/JS/Python). So every .remote() uploads the reference clones too: a huge, slow, reset-prone upload. FIX:',
  '  exclude references/ (and any other non-source junk) from the upload ignore list. The container only needs the',
  '  real workspace source to run `cargo build -p example-add --bin modal_runner`: the workspace Cargo.toml +',
  '  Cargo.lock + crates/* + examples/* — NOT references/, target/, .git/, .modal-rust/, *.rlib. Keep all workspace',
  '  member crates (cargo needs every [workspace].members path to exist) but drop references/.',
  '',
  '  BUG 2 — no transient-retry on the control-plane RPCs. ensure_function issues ~7 unary RPCs with bare .await?:',
  '  client_mount_id, mount_local_dir (which itself does many MountPutFile + blob PUT calls), image_get_or_create,',
  '  function_precreate, function_create, app_publish, function_from_name (then invoke). is_transient exists',
  '  (crates/modal-rust-sdk/src/error.rs:63) but is used ONLY in the image-build join-poll reconnect',
  '  (crates/modal-rust-sdk/src/ops/image.rs:270). A reset on ANY other RPC fails the whole .remote(). Modal\'s own',
  '  SDKs wrap EVERY unary RPC in retry_transient_errors (exponential backoff on transient gRPC/transport errors).',
  '  FIX: add an equivalent retry helper and apply it to all control-plane unary RPCs + the per-file upload calls,',
  '  so a single transient reset retries just that RPC instead of discarding all progress. These RPCs are idempotent',
  '  for our use (Modal dedups images by hash; MountPutFile dedups by sha; from_name/get are reads; create with the',
  '  same precreate_id/definition is safe to re-send after a dropped response) — mirror the Python SDK\'s assumptions.',
  '',
  '## OPTIONAL secondary win — shrink the image build (add_python instead of apt+pip)',
  'The run image is rust:slim + apt-get install python3/pip + pip install --break-system-packages modal — a',
  'MINUTES-long build (a long ImageJoinStreaming stream = a wide reset window). The prototype (workpads/prototype/',
  'dev_app.py) used from_registry("rust:1-slim", add_python="3.12"): add_python attaches Modal\'s hosted',
  'python-standalone MOUNT (resolved by name, like the client mount — NO build step), so the image needs no slow',
  'RUN commands. If clean, implement add_python (resolve python-standalone-mount-{version} GLOBAL -> mount_id ->',
  'attach) to shrink/remove the build. This is SECONDARY — the retry + ignore fixes are the primary, must-do work.',
  'If add_python\'s client-deps story is unclear, keep the apt+pip image and rely on the retry fix + Modal layer',
  'caching (once one build completes it caches, so later runs are fast). Do NOT block on add_python.',
  '',
  '## Ground-truth references (READ; never depend on — references/ is gitignored)',
  '- references/modal-client/py/modal/_utils/grpc_utils.py — retry_transient_errors (the canonical pattern: which',
  '  status codes + transport errors are transient, exponential backoff, max attempts, idempotency).',
  '- references/modal-rs/crates/modal-rs/src/client.rs — the Rust precedent for unary-RPC retry, if any.',
  '- references/modal-client/py/modal/image.py + mount.py — add_python / python_standalone mount (for the optional win).',
  '- crates/modal-rust-sdk/src/error.rs (is_transient:63), src/client.rs, src/ops/{image.rs:270 reconnect, local_dir.rs,',
  '  blob.rs, mount.rs, function.rs, app.rs, invoke.rs}; crates/modal-rust/src/remote.rs (ensure_function/RemoteConfig).',
  '- workpads/shim-backend/knowledge.md (status + the PEP-668 finding).',
  '',
  '## FROZEN invariants — do NOT change',
  '- The runner CLI protocol, the Registry/macros, and the RUN build boundary (cargo builds IN THE FUNCTION BODY at',
  '  execution time; source is MOUNTED; NEVER cargo at image-build time). Do not rewrite working .local() or the',
  '  proven ops; ADD the retry wrapper + fix the ignore list.',
  '',
  '## Verification rules (WORKING.md)',
  '- Gates on default-members: cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo',
  '  test. Keep no-CUDA CI green. Live tests behind #[ignore] + the live feature.',
  '- Modal flakiness => RETRY. The retry helper must ONLY retry TRANSIENT errors (transport reset / UNAVAILABLE /',
  '  DEADLINE-style); real errors (auth, invalid arg, build failure, function_error) must surface IMMEDIATELY, never',
  '  be masked or retried into a timeout.',
  '',
  '## How to return',
  'End with: "RESULT: <STATUS> — <one-line summary>". Build phases STATUS in {BUILD_GREEN, BUILD_FAILED} + exact',
  'cargo output. Live phase: the decoded result or the precise error after retries. Be concrete; cite paths.',
].join('\n')

phase('Design')
const spec = await agent(SHARED + '\n\n' + [
  'YOUR TASK (Design the resilience + ignore fix). Read references/modal-client/py/modal/_utils/grpc_utils.py',
  '(retry_transient_errors), references/modal-rs/crates/modal-rs/src/client.rs, our crates/modal-rust-sdk/src/',
  '{error.rs,client.rs,ops/*.rs}, and crates/modal-rust/src/remote.rs. Then WRITE a build-ready spec to',
  ROOT + '/workpads/shim-backend/remote-resilience-spec.md covering:',
  '- The retry_transient helper: signature (wrap an async unary RPC closure), the transient predicate (reuse/extend',
  '  Error::is_transient — transport reset/h2/UNAVAILABLE/etc), exponential backoff + jitter + max attempts/total',
  '  deadline, and the idempotency note per RPC. Where to apply it: ideally centralize in the SDK (a helper used by',
  '  each ops call), applied to client_mount_id, mount_local_dir per-file (MountPutFile + blob PUT), image_get_or_create',
  '  initial call, function_precreate, function_create, app_publish, function_from_name, and the invoke calls. The',
  '  existing image join-poll reconnect stays.',
  '- The upload IGNORE fix: the corrected default ignore list (add references/ + confirm target/.git/.modal-rust/',
  '  *.rlib) in crates/modal-rust/src/remote.rs RemoteConfig (and/or the call site), so references/ is never uploaded.',
  '  Confirm the kept set still lets `cargo build -p example-add --bin modal_runner` work on Modal (workspace',
  '  Cargo.toml + Cargo.lock + all crates/* members + examples/*).',
  '- OPTIONAL: the add_python (python-standalone hosted mount) plan to shrink the image build, with a clear note that',
  '  it is secondary and skippable if its client-deps story is unclear.',
  'Cite file:line + the grpc_utils transient codes. RESULT: SPEC_DONE — wrote remote-resilience-spec.md',
].join('\n'), { phase: 'Design', label: 'design:resilience' })

phase('Implement')
const impl = await agent(SHARED + '\n\n' + [
  'The spec is at ' + ROOT + '/workpads/shim-backend/remote-resilience-spec.md — READ IT FIRST.',
  '',
  'YOUR TASK (Implement the fixes — HARD GATE on offline gates). Per the spec:',
  '1. Add the retry_transient helper to the SDK (e.g. in client.rs or a small util module) and apply it to every',
  '   control-plane unary RPC in the ops layer + the per-file upload calls (MountPutFile + blob PUT). ONLY retry',
  '   transient errors (extend Error::is_transient if needed); real errors surface immediately. Exponential backoff',
  '   + jitter + a sane cap.',
  '2. Fix the upload ignore list in crates/modal-rust/src/remote.rs so references/ (and target/.git/.modal-rust/*.rlib)',
  '   are excluded — references/ must NOT be uploaded.',
  '3. OPTIONAL (only if clean): add_python via the hosted python-standalone mount to shrink the image build. If',
  '   unclear, skip and leave the apt+pip image (the retry fix + Modal layer caching covers it).',
  'Do NOT change the RUN build boundary or rewrite working code beyond these additive fixes.',
  'VERIFY (offline hard): cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test',
  '(all default-members) — all green. Paste exact output.',
  'RESULT: BUILD_GREEN — retry_transient on all unary RPCs + references/ excluded (+ add_python? y/n) compile + test green',
  '   (or BUILD_FAILED — <reason>)',
].join('\n'), { phase: 'Implement', label: 'resilience-impl' })

const implGreen = /RESULT:\s*BUILD_GREEN/i.test(impl || '')
let live = null
if (!implGreen) {
  log('Implement HARD GATE not green — resilience fixes did not compile. Skipping Live; Review documents the blocker.')
} else {
  phase('Live')
  live = await agent(SHARED + '\n\n' + [
    'The resilience + ignore fixes are implemented and compile. Run the LIVE proof and DRIVE IT TO A RESULT — do NOT',
    'leave a background monitor and return; wait for the terminal outcome yourself.',
    '',
    'YOUR TASK (LIVE PROOF). Run the live .remote() test against REAL Modal:',
    '  cargo test -p modal-rust --features live --test live_remote -- --ignored --nocapture',
    'It must prove app.function("add").remote(AddInput{a:40,b:2}).await? == AddOutput{sum:42} with the user\'s REAL',
    'Rust add built IN THE FUNCTION BODY. With references/ excluded the upload is small, and retry_transient should',
    'carry each RPC past transient resets; Modal also caches the image after the first successful build, so a second',
    'run is fast. If a run is slow, be patient (image build can take minutes the first time) and let it finish. If it',
    'fails on a transient reset, RUN IT AGAIN (a few times, brief waits) — never block on Modal flakiness. If a real',
    '(non-transient) bug surfaces, make the MINIMAL fix and re-verify offline gates stay green.',
    'Capture: the decoded {sum:42}; PROOF cargo ran in the function/runtime logs (RUN boundary); the FunctionCreate',
    'fields (FILE mode, mount_ids = client + source). If after several honest retries it STILL only hits transient',
    'transport resets (Modal degraded right now), report that explicitly with the evidence — that is a clean',
    '"infra-blocked, code-correct" outcome, NOT a code failure.',
    'RESULT: BUILD_GREEN — live .remote() == {sum:42}   (or BUILD_FAILED — <real bug>   or INFRA_BLOCKED — <transient resets after N retries>)',
  ].join('\n'), { phase: 'Live', label: 'live-remote' })
}

phase('Review')
const reviews = await parallel([
  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / retry correctness + build boundary). Verify against the code:',
    '- The retry helper retries ONLY transient errors (transport reset / h2 / UNAVAILABLE-class). Real errors — auth,',
    '  invalid argument, image BUILD FAILURE, the runner function_error envelope — surface IMMEDIATELY and are NOT',
    '  retried into a timeout or masked. Quote the transient predicate.',
    '- It is applied to the control-plane unary RPCs + per-file upload (not just the image poll), with bounded backoff.',
    '- references/ is excluded from the source upload; the kept set still supports cargo build -p example-add on Modal.',
    '- The RUN build boundary is intact (cargo in the function body, source mounted; no cargo at image-build time).',
    'RESULT: PASS — retry sound + boundary intact  (or FAIL — <what is wrong>)',
  ].join('\n'), { phase: 'Review', label: 'review:retry-correctness' }),

  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / hygiene — RUN the gates). From ' + ROOT + ' report exact output + exit status:',
    '- cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test  (default-members).',
    'Confirm example-burn-add still excluded from default-members; live tests #[ignore]+live-feature gated; no',
    'hand-written file grossly exceeds ~500 LOC; no new non-transient-masking unwraps. Report failures verbatim.',
    'RESULT: PASS — gates green  (or FAIL — <exact failing command + output>)',
  ].join('\n'), { phase: 'Review', label: 'review:hygiene' }),
])

return { impl_green: implGreen, impl, live, reviews }
