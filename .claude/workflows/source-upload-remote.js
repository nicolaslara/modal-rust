export const meta = {
  name: 'source-upload-remote',
  description: 'Add local-source UPLOAD to modal-rust-sdk (MountPutFile/BlobCreate/reqwest + dir walk/hash) and wire real .remote(): port the proven dev_app.py run recipe into a FILE-mode wrapper so app.function("add").remote() builds+runs modal_runner live -> {sum:42}',
  phases: [
    { title: 'Design', detail: 'read upload protocol (mount.py/blob_utils + modal-rs) + dev_app.py run recipe + facade -> one spec' },
    { title: 'Upload', detail: 'SDK add_local_dir-equivalent: MountGetOrCreate(create)+MountPutFile+BlobCreate+reqwest; gates green (HARD GATE)' },
    { title: 'Remote', detail: 'FILE-mode run wrapper (port dev_app.py) + Function::remote wiring; gates green (HARD GATE)' },
    { title: 'Live', detail: 'app.function("add").remote(AddInput{40,2}).await == {sum:42} on Modal, real runner built in-body (best-effort, retry)' },
    { title: 'Review', detail: 'parallel: independence, run-build-boundary correctness, hygiene' },
  ],
}

const ROOT = '/Users/nicolas/devel/modal-rust'

const SHARED = [
  'You are building part of modal-rust (repo root: ' + ROOT + '; git repo on branch main).',
  '',
  '## What we are building (this workflow): local-source UPLOAD + real .remote()',
  'Today crates/modal-rust-sdk can resolve the HOSTED client mount (lookup-only) and bake a tiny wrapper module into',
  'an image via a Dockerfile RUN, but it CANNOT upload the user\'s local crate to Modal. So .remote() in the facade',
  '(crates/modal-rust) is a stub. This workflow closes that gap and makes .remote() REAL for the run path:',
  '  (1) Add a local-directory UPLOAD capability to the SDK (the add_local_dir(copy=False) equivalent): create a Mount',
  '      from a local dir — walk files, sha256 each, MountGetOrCreate(create) + MountPutFile (inline small files) +',
  '      BlobCreate + HTTP PUT (reqwest) for large files — returning a mount_id to attach via Function.mount_ids.',
  '  (2) Port the PROVEN run recipe (workpads/prototype/dev_app.py run_entrypoint) into a FILE-mode Python wrapper',
  '      baked into the image: receive the CBOR input, write /tmp/in.json, cargo build the mounted crate IN THE',
  '      FUNCTION BODY at execution time, exec the freshly built modal_runner via the frozen runner protocol, return',
  '      the JSON envelope.',
  '  (3) Wire Function::remote() in the facade: ensure the function exists on Modal (app + run-image + source mount +',
  '      client mount + FunctionCreate FILE mode), invoke via CBOR, parse the runner envelope, return Result<Out, Error>',
  '      with the SAME semantics as .local() (ok -> Out; error kinds -> facade Error).',
  'GOAL: app.function("add").remote(AddInput{a:40,b:2}).await? == AddOutput{sum:42}, run live on Modal, with the user\'s',
  'REAL Rust add (NOT an echo), built in the function body, with NO modal CLI and NO per-project .py.',
  '',
  '## THE BUILD BOUNDARY (hard, non-negotiable invariant)',
  'This is the RUN path: source is MOUNTED (copy=False equivalent) and cargo build runs IN THE FUNCTION BODY at',
  'execution time — NEVER at image-build time. (Deploy = build-at-image-time is a LATER milestone; do not do it here.)',
  'The live proof MUST show cargo running in the function/runtime logs, not in the image build.',
  '',
  '## Ground-truth references (READ; never depend on — references/ is gitignored)',
  '- Upload protocol (the new capability):',
  '  - references/modal-client/py/modal/mount.py  — how a local-dir Mount is built (file walk, MountGetOrCreate with',
  '    files, MountPutFile, sha256, ignore filters).',
  '  - references/modal-client/py/modal/_utils/blob_utils.py  — blob upload (blob_create, the inline-vs-blob size',
  '    threshold, multipart/HTTP PUT, hashing).',
  '  - references/modal-rs/crates/modal-rs/src/{mount.rs,blob_transfer.rs}  — the Rust precedent for both.',
  '  - proto (crates/modal-rust-sdk/proto/api.proto): MountGetOrCreateRequest (~2596), MountPutFileRequest (~2614),',
  '    MountFile (~2589), BlobCreateRequest (~815); rpcs MountGetOrCreate (~4269), MountPutFile (~4270),',
  '    BlobCreate (~4163), BlobGet (~4164). Read the actual fields.',
  '- The proven run recipe to port: workpads/prototype/dev_app.py (the run_entrypoint @app.function: base image',
  '  rust:1-slim + add_python="3.12" + entrypoint([]) + RUST_BACKTRACE=1; add_local_dir(copy=False, ignore=[target,',
  '  .git, .modal-rust, **/*.rlib]); body: CARGO_HOME=/tmp/cargo, CARGO_TARGET_DIR=/tmp/target, build in /src if',
  '  writable else cp -a to /tmp/build, cargo build --release -p <PACKAGE> --bin modal_runner, write /tmp/in.json,',
  '  exec /tmp/target/release/modal_runner --entrypoint <name> --input-file /tmp/in.json, return stdout envelope).',
  '  NOTE: rust:1-slim has NO python; Modal containers need python for modal._container_entrypoint. Decide how to',
  '  provision it (the hosted python-standalone mount like add_python, OR apt-get install python3 via run_commands) —',
  '  the simplest self-contained choice that works. The client mount still supplies the modal client SOURCE; on a',
  '  rust base its pip deps may also be needed (with_pip_install_modal) — verify, mirror the SDK\'s earlier finding.',
  '- Current SDK + facade to extend (do not rewrite working code):',
  '  - crates/modal-rust-sdk/src/ops/{mount.rs,image.rs,function.rs,invoke.rs}, src/client.rs, src/error.rs.',
  '  - crates/modal-rust/src/{app.rs,function.rs,error.rs} (the facade; Function::remote is the stub to implement).',
  '  - examples/add (the target: add(AddInput{a,b}) -> AddOutput{sum}; PACKAGE = example-add; bin modal_runner).',
  '- workpads/shim-backend/knowledge.md (FILE-mode facts, the 3 fixes, the client-mount finding, status).',
  '',
  '## FROZEN invariants — do NOT change',
  '- modal-rust-runtime (Registry/HandlerFn/typed!/run_cli/RunnerError), the runner CLI protocol, the macros.',
  '- The run-vs-deploy build boundary (above). Do not rewrite the proven SDK ops or the facade .local(); ADD to them.',
  '',
  '## Verification rules (WORKING.md)',
  '- Gates on default-members (NOT --workspace/--all-features). Hard gates: cargo fmt --check ; cargo clippy',
  '  --all-targets -- -D warnings ; cargo build ; cargo test (all default-members). Keep the no-CUDA CI green.',
  '- New deps (e.g. reqwest with rustls, no default features; walkdir; sha2 is already present) must keep CI green.',
  '- Modal flakiness is TRANSIENT — RETRY, never block. Live round-trips are best-effort; the HARD gates are the',
  '  offline compiles. Live tests behind #[ignore] + the existing live feature so CI never runs them.',
  '- Files ~300-500 LOC each.',
  '',
  '## How to return',
  'End with: "RESULT: <STATUS> — <one-line summary>". Build phases STATUS in {BUILD_GREEN, BUILD_FAILED} with exact',
  'cargo output. Live phase: report the decoded result or the precise transient error after retries. Be concrete; cite paths.',
].join('\n')

phase('Design')
const design = await parallel([
  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Design / local-dir UPLOAD capability). Read references/modal-client/py/modal/mount.py +',
    'references/modal-client/py/modal/_utils/blob_utils.py + references/modal-rs/crates/modal-rs/src/{mount.rs,',
    'blob_transfer.rs} + the proto messages (MountGetOrCreateRequest, MountPutFileRequest, MountFile, BlobCreateRequest)',
    'and the current crates/modal-rust-sdk/src/ops/mount.rs. Produce a PRECISE spec for an add_local_dir-equivalent on',
    'ModalClient that uploads a local directory as a Mount and returns a mount_id:',
    '- File walk + ignore patterns (target, .git, .modal-rust, **/*.rlib) matching dev_app.py; remote path mapping',
    '  (local dir -> /src style); per-file sha256.',
    '- The inline-vs-blob threshold: small files inline in MountPutFile; large files via BlobCreate -> HTTP PUT (reqwest)',
    '  -> reference the blob_id. Exact request fields + the upload sequence (MountGetOrCreate create -> per-file',
    '  MountPutFile -> finalize). Dedup by sha256 if the protocol supports skipping already-present files.',
    '- The new dep set (reqwest rustls + no-default-features; walkdir or std; sha2 already present) and how it stays',
    '  CI-green. The public API shape (e.g. ModalClient::mount_local_dir(local, remote, ignore) -> Result<String>).',
    'Cite file:line + proto fields. RESULT: SPEC_DONE — upload-capability spec',
  ].join('\n'), { phase: 'Design', label: 'design:upload' }),

  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Design / FILE-mode run wrapper + run image). Read workpads/prototype/dev_app.py (run_entrypoint) +',
    'crates/modal-rust-sdk/src/ops/image.rs (with_wrapper_module, dockerfile_commands, with_pip_install_modal) +',
    'crates/modal-rust-sdk/src/ops/invoke.rs (how args arrive) + the runner protocol in',
    'crates/modal-rust-runtime/src/lib.rs (run_cli, the envelope shape). Produce a PRECISE spec for:',
    '- The FILE-mode Python wrapper module (baked via with_wrapper_module) that ports dev_app.py run_entrypoint: a',
    '  top-level handler(payload) that writes the input JSON to /tmp/in.json, ensures the crate is built (cargo build',
    '  --release -p <PACKAGE> --bin modal_runner with CARGO_HOME=/tmp/cargo, CARGO_TARGET_DIR=/tmp/target, building in',
    '  the mounted /src if writable else cp -a to /tmp/build), execs /tmp/target/release/modal_runner --entrypoint',
    '  <ENTRYPOINT> --input-file /tmp/in.json, and RETURNS the one-line JSON envelope (string). ENTRYPOINT + PACKAGE are',
    '  baked per-function. Build ONCE per container if cheap (guard so repeated invokes do not rebuild). Keep stdout = the',
    '  single envelope; all build logs to stderr.',
    '- The run IMAGE: base rust:<ver>-slim, python provisioning decision (hosted python-standalone mount vs apt-get',
    '  install python3 via run_commands — pick the simplest that boots modal._container_entrypoint), entrypoint([])',
    '  neutralization equivalent, RUST_BACKTRACE=1, and whether with_pip_install_modal is needed on the rust base.',
    '  Note: the SOURCE is NOT baked — it arrives as the uploaded source mount (the other design task). image.rs may need',
    '  a small additive extension (e.g. with_apt/with_env/add_python) — specify the minimal additions.',
    '- How the wrapper maps the runner envelope back: it returns the envelope string; the facade parses it (next task).',
    'Cite file:line. RESULT: SPEC_DONE — run-wrapper + run-image spec',
  ].join('\n'), { phase: 'Design', label: 'design:wrapper-image' }),

  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Design / Function::remote wiring in the facade). Read crates/modal-rust/src/{app.rs,function.rs,error.rs}',
    '(the current .local() + the .remote() stub + App::connect) + crates/modal-rust-sdk/src/ops/{app,image,function,',
    'invoke,mount}.rs + client.rs. Produce a PRECISE spec for implementing Function::remote (run path):',
    '- The end-to-end sequence on first .remote(): App::connect (ModalClient + app_id) -> resolve client mount -> upload',
    '  the user crate as a source mount (the new mount_local_dir) -> build the run image (rust base + python + baked run',
    '  wrapper for THIS entrypoint) -> FunctionPrecreate -> FunctionCreate(FILE, module+function = the wrapper, image_id,',
    '  mount_ids=[client_mount, source_mount], resources, supported_input_formats=[PICKLE,CBOR]) -> AppPublish (or keep',
    '  ephemeral) -> from_name. Then invoke_cbor((input,), {}) -> the wrapper returns the envelope string -> parse it.',
    '- CACHING within a process: do not recreate the function on every .remote() call; memoize the created function id /',
    '  handle on the App (or a per-name cache). Keep it simple + correct.',
    '- RESULT SEMANTICS matching .local(): parse the runner JSON envelope; ok:true -> deserialize value as Out; ok:false',
    '  -> map the five error kinds (decode_error/unknown_entrypoint/function_error/encode_error/panic) into the facade',
    '  Error (reuse the existing Error variants; add only if needed). So .remote() and .local() return the SAME',
    '  Result<Out, Error> shape for the same input.',
    '- Where the local crate root comes from (the dir to upload): a sensible default (the user crate / workspace root)',
    '  with an override; PACKAGE detection for the -p flag (from the crate). Keep config minimal for v0 (add still works).',
    'Cite file:line. RESULT: SPEC_DONE — Function::remote wiring spec',
  ].join('\n'), { phase: 'Design', label: 'design:remote-wiring' }),
])

const spec = await agent(SHARED + '\n\n' + [
  'YOUR TASK (Synthesize the build spec). Merge the three notes (upload capability; run wrapper + image; Function::remote',
  'wiring) into ONE authoritative build-ready spec and WRITE it to ' + ROOT + '/workpads/shim-backend/remote-build-spec.md',
  '(overwrite if present). Concrete enough to implement without re-deriving: the SDK upload API + request sequence + new',
  'deps; the FILE-mode run wrapper source + the run image (incl. python provisioning); the Function::remote sequence +',
  'caching + envelope->Result mapping; and which files change. Preserve the RUN build boundary (build in the function',
  'body, source mounted, never at image-build time). Resolve contradictions (prefer the proven dev_app.py recipe + the',
  'verified FILE-mode facts). Keep it tight.',
  '',
  '=== UPLOAD NOTE ===',
  (design[0] || '(missing)'),
  '',
  '=== WRAPPER + IMAGE NOTE ===',
  (design[1] || '(missing)'),
  '',
  '=== REMOTE WIRING NOTE ===',
  (design[2] || '(missing)'),
  '',
  'RESULT: SPEC_DONE — wrote remote-build-spec.md',
].join('\n'), { phase: 'Design', label: 'design:synthesize' })

phase('Upload')
const upload = await agent(SHARED + '\n\n' + [
  'The build spec is at ' + ROOT + '/workpads/shim-backend/remote-build-spec.md — READ IT FIRST.',
  '',
  'YOUR TASK (SDK local-dir UPLOAD capability — HARD GATE). Implement the add_local_dir-equivalent on ModalClient per',
  'the spec: walk a local dir (apply ignore patterns), sha256 each file, MountGetOrCreate(create) + MountPutFile for',
  'small files inline + BlobCreate -> HTTP PUT (reqwest) for large files, returning a mount_id. Add the new deps',
  '(reqwest rustls no-default-features; walkdir if used) to crates/modal-rust-sdk/Cargo.toml. Put it in a focused module',
  '(e.g. extend ops/mount.rs or a new ops/upload.rs, ~300-500 LOC). Do NOT break the existing lookup-only client-mount',
  'resolution.',
  'VERIFY:',
  '- OFFLINE (hard): cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test  (all',
  '  default-members) — all green.',
  '- LIVE (best-effort, behind #[ignore]+live feature): upload a SMALL dir (e.g. a temp dir with 1-2 files, or the',
  '  examples/add crate with ignores) -> assert a non-empty mount_id. Retry on Modal blips; record exact transient errors.',
  'Paste cargo output + the live mount_id (or the precise transient error after retries).',
  'RESULT: BUILD_GREEN — SDK upload capability green (mount_id <id/transient-blip>)   (or BUILD_FAILED — <reason>)',
].join('\n'), { phase: 'Upload', label: 'sdk-upload' })

const uploadGreen = /RESULT:\s*BUILD_GREEN/i.test(upload || '')
let remote = null, live = null
if (!uploadGreen) {
  log('Upload HARD GATE not green — source upload did not compile. Skipping Remote+Live; going to Review to document the blocker.')
} else {
  phase('Remote')
  remote = await agent(SHARED + '\n\n' + [
    'The build spec is at ' + ROOT + '/workpads/shim-backend/remote-build-spec.md. The SDK upload capability now exists.',
    'First run cargo build and read the current SDK + facade files to orient.',
    '',
    'YOUR TASK (FILE-mode run wrapper + run image + Function::remote — HARD GATE on offline build). Implement per the spec:',
    '- The FILE-mode run wrapper (ported from dev_app.py run_entrypoint): baked via image with_wrapper_module; writes',
    '  /tmp/in.json, builds the mounted crate in-body (cargo build --release -p <PACKAGE> --bin modal_runner, CARGO_HOME=',
    '  /tmp/cargo, CARGO_TARGET_DIR=/tmp/target, build in /src if writable else cp -a /tmp/build), execs modal_runner,',
    '  returns the JSON envelope string. Build-once guard per container.',
    '- The run IMAGE additions to ops/image.rs as needed (rust base + python provisioning + RUST_BACKTRACE + the wrapper).',
    '- Function::remote in crates/modal-rust/src/function.rs (replace the NotImplemented stub for the RUN path): the',
    '  ensure-created sequence (app, client mount, uploaded source mount, run image, FunctionCreate FILE, from_name),',
    '  per-name caching on the App, invoke_cbor, parse the runner envelope, map ok/err to Result<Out, Error> exactly like',
    '  .local(). Keep .spawn()/.map() as honest stubs unless trivial.',
    'Do NOT change the run-vs-deploy boundary (build stays in the function body). Do NOT rewrite .local() or the proven SDK ops.',
    'VERIFY (offline hard): cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test',
    '(all default-members) — all green. (Live proof is the NEXT phase; do not block on it here, but you may smoke-compile',
    'the live test behind the live feature.)',
    'Paste cargo output. RESULT: BUILD_GREEN — run wrapper + image + Function::remote compile green   (or BUILD_FAILED — <reason>)',
  ].join('\n'), { phase: 'Remote', label: 'wrapper+remote' })

  const remoteGreen = /RESULT:\s*BUILD_GREEN/i.test(remote || '')
  if (!remoteGreen) {
    log('Remote HARD GATE not green — wrapper/remote did not compile. Skipping Live; Review will document the blocker.')
  } else {
    phase('Live')
    live = await agent(SHARED + '\n\n' + [
      'The SDK upload + the run wrapper + Function::remote are implemented and compile. The build spec is at',
      ROOT + '/workpads/shim-backend/remote-build-spec.md.',
      '',
      'YOUR TASK (LIVE PROOF — the payoff). Prove real .remote() end-to-end against REAL Modal: a live test/example',
      '(behind #[ignore] + the live feature so CI never runs it) that does:',
      '  App::connect(...) ; app.function("add").remote(AddInput{a:40,b:2}).await?  ==  AddOutput{sum:42}',
      'where the function runs the user\'s REAL Rust add (NOT an echo) — the wrapper cargo-builds the mounted crate IN',
      'THE FUNCTION BODY and execs modal_runner. Modal flakiness => RETRY (a few attempts, brief waits) — never block.',
      'Capture: the decoded {sum:42}; PROOF that cargo ran in the function/runtime logs (the RUN build boundary, not',
      'image-build); the FunctionCreate fields (FILE mode, mount_ids include the uploaded source mount + client mount);',
      'and confirmation no modal CLI / no per-project .py was written. If a real bug surfaces, make the MINIMAL fix and',
      're-verify offline gates stay green. If it blips after retries, record the EXACT error + that the code is complete',
      'and offline-green (note for a later retry — NOT a block).',
      'Paste the live result + cargo gate status.',
      'RESULT: BUILD_GREEN — live .remote() <succeeded: {sum:42} / transient-blip: err>   (or BUILD_FAILED — <reason>)',
    ].join('\n'), { phase: 'Live', label: 'live-remote' })
  }
}

phase('Review')
const reviews = await parallel([
  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / independence + dep hygiene). Verify the new upload capability keeps the crate self-contained:',
    '- No modal-rs/modal-client dependency or path/git dep into references/ crept in; references/ stays gitignored.',
    '- New deps (reqwest etc.) are minimal (rustls, no-default-features) and do not pull a second TLS stack conflicting',
    '  with tonic; the SDK still builds on a no-CUDA box. cargo tree sanity.',
    '- The proto upload messages are used from the VENDORED proto, not references/.',
    'Cite file:line. RESULT: PASS — independent + lean deps  (or FAIL — <what is wrong>)',
  ].join('\n'), { phase: 'Review', label: 'review:independence' }),

  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / RUN build-boundary correctness). This is the critical invariant. Verify against the code +',
    'workpads/prototype/dev_app.py + workpads/architecture/boundaries.md:',
    '- The run wrapper builds the crate IN THE FUNCTION BODY at execution time (cargo invoked at runtime), NOT at',
    '  image-build time. The image does NOT run cargo; the SOURCE is MOUNTED (uploaded source mount, copy=False',
    '  equivalent), not baked into an image layer.',
    '- The runner is invoked via the FROZEN protocol (modal_runner --entrypoint <name> --input-file ...), one JSON',
    '  envelope on stdout; .remote() maps the envelope to Result<Out, Error> with the SAME semantics as .local().',
    '- FILE mode is intact (definition_type=FILE, empty function_serialized, module+function = the wrapper); the 3 fixes',
    '  and client-mount injection are preserved.',
    'Quote offending code if the boundary is violated (e.g. cargo at image-build time). RESULT: PASS — run boundary intact',
    '(or FAIL — <what violates it>)',
  ].join('\n'), { phase: 'Review', label: 'review:build-boundary' }),

  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / hygiene — RUN the gates). From ' + ROOT + ' run and report exact output + exit status:',
    '- cargo fmt --check',
    '- cargo clippy --all-targets -- -D warnings   (default-members)',
    '- cargo build                                  (default-members)',
    '- cargo test                                   (default-members)',
    'Confirm example-burn-add still excluded from default-members (no-CUDA CI green); the live tests are #[ignore]+live-',
    'feature gated (CI runs 0 of them); no hand-written file grossly exceeds ~500 LOC. Report any failure verbatim.',
    'RESULT: PASS — gates green  (or FAIL — <exact failing command + output>)',
  ].join('\n'), { phase: 'Review', label: 'review:hygiene' }),
])

return { upload_green: uploadGreen, upload, remote, live, reviews }
