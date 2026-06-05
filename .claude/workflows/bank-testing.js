export const meta = {
  name: 'bank-testing',
  description: 'Bank the offline-testing investment now that the mock backend exists: (1) extract the pure request-builders so the whole manifest is unit-testable with NO server, (2) broaden mock-backed integration coverage across the full invoke surface (deploy/call/map/spawn/secrets/volumes/cache/image/error-taxonomy), and (3) expose a dump/dry-run that renders the manifest a run/deploy WOULD send without sending it. Pure-additive + behavior-preserving: same bytes on the wire, runner protocol + facade public API frozen.',
  phases: [
    { title: 'Design', detail: 'read the SDK ops + facade remote/deploy + the testkit + existing mock_*.rs + testing-strategy.md; spec the builder-extraction, the mock test matrix, and the dry-run/dump' },
    { title: 'Builders+Dump', detail: 'extract pure build_*_request fns (byte-identical) + unit tests + a programmatic dry-run/dump over them; gates green (HARD GATE)' },
    { title: 'Coverage', detail: 'broaden mock-backed integration tests across deploy/call/map/spawn/secrets/volumes/cache/image/error-taxonomy; gates green (HARD GATE)' },
    { title: 'Review', detail: 'parallel: (1) coverage breadth + dump correctness; (2) gates green + behavior-preserving (no wire/behaviour drift)' },
  ],
}

const ROOT = '/Users/nicolas/devel/modal-rust'

const SHARED = [
  'Repo root: ' + ROOT + ' (git on main). GOAL: "bank the testing investment" — turn the just-landed in-process mock',
  'backend (crates/modal-rust-testkit) into real offline coverage of the gRPC/manifest + invoke surface, add the cheap',
  'no-server unit layer the testing doc recommends, and surface a dry-run/dump. This is HARDENING, not new user features.',
  '',
  '## The three layers to build (smallest-first, per docs/testing-strategy.md)',
  '1. **Pure request-builders (no server).** Today several top-level requests are BUILT-AND-SENT in one expression in the',
  '   SDK ops / facade, so nothing asserts them offline (the doc names FunctionCreateRequest, AppPublishRequest, etc. and',
  '   notes app.rs/blob.rs had 0 tests). Extract the construction of each outbound request into a PURE function',
  '   `build_<x>_request(...) -> <X>Request` (no I/O), call it from the existing send path (so the WIRE BYTES ARE',
  '   IDENTICAL), and UNIT-TEST the builders directly — no server, no mock. This is the cheapest, fastest layer.',
  '2. **Mock-backed integration coverage.** Using the existing testkit, broaden end-to-end offline tests across the FULL',
  '   invoke surface the mock can drive: deploy+call (AppPublish + FunctionCreate manifest + call decode), .map (FunctionMap',
  '   + FunctionPutInputs + FunctionGetOutputs over N inputs, results in input order), .spawn (handle -> get), secrets +',
  '   volumes (SecretGetOrCreate/VolumeGetOrCreate ids ride into FunctionCreate), the P6 cache volume mount (cache on vs',
  '   off), image build (ImageGetOrCreate + ImageJoinStreaming), and the 5-kind error taxonomy (envelope -> RunnerError).',
  '   Mirror the existing crates/modal-rust/tests/mock_remote.rs + mock_table.rs + crates/modal-rust-sdk/tests/mock_ops.rs',
  '   pattern (App::connect_at(mock.url(), ..) behind the testkit feature; assert mock.requests::<T>()).',
  '3. **Dry-run / dump.** A PROGRAMMATIC way to render the manifest a run (and a deploy) WOULD send, built ON the pure',
  '   builders, with NO network — the deferred P8 dump tool. e.g. an additive facade method like',
  '   `app.dry_run(&RemoteConfig) -> Manifest`/`app.dump_manifest(..)` that returns/renders the assembled requests',
  '   (FunctionCreate + image + mounts + secrets/volumes) as structured data + a readable text form. Keep it additive',
  '   (new method/struct); do NOT change the real run/deploy path beyond routing it through the same extracted builders.',
  '',
  '## FROZEN — additive + behavior-preserving',
  '- Extracting builders must be BYTE-IDENTICAL on the wire: the requests the real client sends are unchanged (prove it —',
  '  the mock records the SAME FunctionCreate/AppPublish as before the refactor). The runner CLI protocol, the 5 error',
  '  kinds, the FILE-mode wire (empty function_serialized, module+function name, add_python + client mounts), `typed!`,',
  '  Registry dispatch, and the facade PUBLIC API (local/local_with_registry/connect/connect_with_registry/deploy/call —',
  '  do not change their signatures/semantics; the dry-run is a NEW additive method) — UNCHANGED. Do NOT modify the macro',
  '  crate or modal-rust-runtime. Do NOT touch the user\'s uncommitted docs/testing-strategy.md (you may READ it).',
  '- The testkit (crates/modal-rust-testkit) stays a dev/test concern (dev-dependency only, not in default-members). New',
  '  integration tests are offline: loopback only, NO Modal creds, NO Python, deterministic (no Date/random), not #[ignore].',
  '',
  '## Ground-truth refs (READ)',
  '- crates/modal-rust-sdk/src/ops/* (where each outbound request is built — function/app/image/mount/secret/volume/blob),',
  '  crates/modal-rust-sdk/src/{client.rs, channel.rs}.',
  '- crates/modal-rust/src/{remote.rs (run manifest assembly), deploy.rs (deploy manifest), scope.rs, app.rs, function.rs}.',
  '- crates/modal-rust-testkit/src/* (MockModal API: start()/builder(), url(), modal_config(), requests::<T>()/last/took,',
  '  function_result_value/function_body/on_<rpc>) + the connect_at* facade seam.',
  '- The EXISTING mock tests to extend: crates/modal-rust/tests/{mock_remote.rs, mock_table.rs},',
  '  crates/modal-rust-sdk/tests/mock_ops.rs ; and crates/modal-rust/tests/local.rs (offline patterns).',
  '- docs/testing-strategy.md (the layered plan: extract build_*_request fns; RecordingSink == the dump tool; insta snapshots).',
  '',
  '## Verification (offline = HARD gate; NO Modal/Python)',
  '- default-members + testkit: cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test —',
  '  all green. Run the facade+sdk mock targets explicitly (cargo test -p modal-rust -p modal-rust-sdk). Paste exact output',
  '  + the NEW test names. The builder refactor must keep every EXISTING test green (esp. the live tests still COMPILE and',
  '  the mock_* tests still record the same requests).',
  '',
  '## How to return',
  'End with: "RESULT: <STATUS> — <one-line>". Build phases STATUS in {BUILD_GREEN, BUILD_FAILED} + exact cargo output + the',
  'new test count/names + (for the dump) a quoted sample of the rendered manifest.',
].join('\n')

phase('Design')
const spec = await agent(SHARED + '\n\n' + [
  'YOUR TASK (Design). READ the ground-truth refs. Produce a build-ready spec at ' + ROOT + '/workpads/shim-backend/',
  'bank-testing-spec.md covering: (1) EXACTLY which outbound requests to extract into pure `build_*_request` fns (list each',
  'with its current build-and-send site file:line and the proposed pure-fn signature + where it gets called from so the',
  'wire stays identical), and which currently have zero offline assertion; (2) the MOCK TEST MATRIX — one row per flow',
  '(deploy+call, map, spawn, secrets, volumes, cache on/off, image build, each of the 5 error kinds), each naming the RPCs',
  'it exercises, what the mock should be configured to return, and the exact assertion (which recorded request fields /',
  'decoded outputs); (3) the DRY-RUN/DUMP design — the additive facade API (method name, return type/struct, text render),',
  'how it reuses the pure builders, and what it includes for run vs deploy. Cite file:line. Flag anything that cannot be',
  'driven by the current mock (and whether to extend the testkit minimally). RESULT: SPEC_DONE — wrote bank-testing-spec.md',
].join('\n'), { phase: 'Design', label: 'design:bank-testing' })

phase('Builders+Dump')
const seam = await agent(SHARED + '\n\n' + [
  'The spec is at ' + ROOT + '/workpads/shim-backend/bank-testing-spec.md — READ IT FIRST.',
  '',
  'YOUR TASK (Builders + Dump — HARD GATE). Per the spec: extract the outbound-request construction into pure',
  '`build_*_request` functions (call them from the existing send path so the WIRE IS BYTE-IDENTICAL), add UNIT TESTS for',
  'those builders (no server — assert the constructed request fields, incl. the previously-untested top-level requests +',
  'app.rs/blob.rs), and add the additive programmatic DRY-RUN/DUMP (renders the run + deploy manifest over the same',
  'builders, no network). Do NOT change wire bytes / runner protocol / facade public signatures (dry-run is a NEW method).',
  'VERIFY (offline HARD, paste exact output): cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ;',
  'cargo test ; cargo test -p modal-rust -p modal-rust-sdk — all green, INCLUDING every pre-existing test (the mock_* tests',
  'must still record identical requests). QUOTE a sample of the rendered dry-run manifest. RESULT: BUILD_GREEN — pure builders + unit tests + dry-run; wire byte-identical; gates green   (or BUILD_FAILED — <reason>)',
].join('\n'), { phase: 'Builders+Dump', label: 'build:builders-dump' })

phase('Coverage')
const cover = await agent(SHARED + '\n\n' + [
  'The spec is at ' + ROOT + '/workpads/shim-backend/bank-testing-spec.md (the mock test matrix). The pure builders +',
  'dry-run from the previous phase are already in place (read them).',
  '',
  'YOUR TASK (Mock coverage — HARD GATE). Implement the mock test matrix: broaden the offline mock-backed integration',
  'tests across the full invoke surface — deploy+call, .map (N inputs, in order), .spawn(handle->get), secrets, volumes,',
  'the P6 cache volume mount (cache on vs off), image build (ImageGetOrCreate + ImageJoinStreaming), and each of the 5',
  'runner error kinds (envelope -> RunnerError). Mirror the existing mock_remote.rs/mock_table.rs/mock_ops.rs pattern',
  '(App::connect_at(mock.url(),..) behind the testkit feature; configure the mock; assert mock.requests::<T>() + decoded',
  'outputs). Offline only (loopback, no creds, deterministic, not #[ignore]). Add a table test where natural.',
  'VERIFY (offline HARD, paste exact output + the NEW test names): cargo fmt --check ; cargo clippy --all-targets -- -D',
  'warnings ; cargo build ; cargo test ; cargo test -p modal-rust -p modal-rust-sdk — all green.',
  'RESULT: BUILD_GREEN — full invoke surface covered offline via the mock; gates green   (or BUILD_FAILED — <reason>)',
].join('\n'), { phase: 'Coverage', label: 'build:mock-coverage' })

const seamGreen = /RESULT:\s*BUILD_GREEN/i.test(seam || '')
const coverGreen = /RESULT:\s*BUILD_GREEN/i.test(cover || '')
if (!seamGreen || !coverGreen) log('A build phase HARD GATE was not green — Review documents the blocker.')

phase('Review')
const reviews = await parallel([
  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / breadth + dump). Verify against the code: the pure `build_*_request` fns exist and have unit tests',
    '(incl. the previously-untested top-level requests + app.rs/blob.rs). The mock test matrix is actually covered — list',
    'each flow (deploy+call, map, spawn, secrets, volumes, cache on/off, image build, the 5 error kinds) and the test that',
    'asserts it (name + the recorded-request/decoded-output assertion); call out any flow from the spec matrix that is',
    'MISSING (no silent gaps). The dry-run/dump renders a correct run + deploy manifest over the same builders (quote a',
    'sample). RESULT: PASS — full surface covered, dump correct  (or FAIL — <what is missing/wrong>)',
  ].join('\n'), { phase: 'Review', label: 'review:breadth-dump' }),

  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / gates + behavior-preserving — RUN them). From ' + ROOT + ': cargo fmt --check ; cargo clippy',
    '--all-targets -- -D warnings ; cargo build ; cargo test ; cargo test -p modal-rust -p modal-rust-sdk — report exact',
    'output + exit status; all green. Confirm the builder extraction is BEHAVIOR-PRESERVING: the wire bytes are unchanged',
    '(the pre-existing mock_* tests still record identical FunctionCreate/AppPublish; the live_* tests still COMPILE under',
    '--features live), the facade public signatures are unchanged (dry-run is purely additive), the runner protocol / 5',
    'error kinds / FILE-mode wire are untouched, and the testkit stays dev-only (cargo tree -e normal shows no shipped',
    'dep). Report any failure verbatim. RESULT: PASS — gates green, wire+API behavior-preserving  (or FAIL — <exact failing cmd+output>)',
  ].join('\n'), { phase: 'Review', label: 'review:gates-frozen' }),
])

return { seam_green: seamGreen, cover_green: coverGreen, seam, cover, reviews }
