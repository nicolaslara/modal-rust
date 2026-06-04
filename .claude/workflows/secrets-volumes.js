export const meta = {
  name: 'secrets-volumes',
  description: 'User-facing secrets + volumes: attach a modal.Secret (env vars) and a user modal.Volume (persistent storage at a mount path) to a function via the decorator/config, flowing into FunctionCreate (secret_ids + volume_mounts). Prove live: a function reads a secret env var and read/writes a persisted volume file.',
  phases: [
    { title: 'Design', detail: 'macro/FunctionConfig for secrets+volumes + SDK SecretGetOrCreate + facade wiring -> one spec' },
    { title: 'Implement', detail: 'macro secrets/volumes + ops/secret.rs + FunctionSpec.secret_ids + facade resolve/attach; gates green (HARD GATE)' },
    { title: 'Live', detail: 'a fn with a secret (env var injected) + a user volume (write then read-back persists); ephemeral' },
    { title: 'Review', detail: 'parallel: secret/volume semantics match Modal + frozen invariants (cache volume separate); hygiene' },
  ],
}

const ROOT = '/Users/nicolas/devel/modal-rust'

const SHARED = [
  'You are adding user-facing SECRETS + VOLUMES to modal-rust (repo root: ' + ROOT + '; git on main).',
  '',
  '## Where we are',
  'Full product proven live (CPU+GPU, run/deploy/call, CLI+facade, spawn/map, cargo cache). Per-function config flows',
  'from the decorator: `#[modal_rust::function(gpu=…, timeout=…, cache=…)]` -> FunctionConfig -> FunctionCreate (P4).',
  'The SDK already has `volume_get_or_create` (V2) + `FunctionVolumeMount` + `FunctionSpec.volume_mounts` (added in P6',
  'for the cargo cache). What is missing: USER-facing secrets + volumes — a way to attach a `modal.Secret` (env vars)',
  'and a user `modal.Volume` (persistent storage at a mount path) to a function. This is the "extras attach via',
  'volumes; credentials via secrets" path real apps need.',
  '',
  '## What this builds',
  '1. **Secrets**: `#[modal_rust::function(secrets = ["my-secret", "other"])]` -> the named Modal secrets are resolved',
  '   to secret_ids and attached to `FunctionCreate.secret_ids`; their key/values are injected as ENV VARS in the',
  '   container, readable by the user fn (std::env) and the runner. SDK: a new `ops/secret.rs`',
  '   `secret_get_or_create(name) -> secret_id` (lookup by name; + a from_dict create path for tests/ephemeral) +',
  '   `FunctionSpec.secret_ids` -> `Function.secret_ids`.',
  '2. **User volumes**: `#[modal_rust::function(volumes = ["/data=my-vol"])]` (path=name pairs) -> each name resolved',
  '   via the existing `volume_get_or_create` and attached via `FunctionVolumeMount{mount_path:"/data"}` so the fn can',
  '   read/write persistent storage. REUSE the P6 volume plumbing; this is a SEPARATE, user-specified mount (NOT the',
  '   cargo-cache volume — keep them independent; both can coexist on a function).',
  '',
  '## Config ergonomics',
  'Follow the P4 decorator-is-config pattern: extend the macro + `FunctionConfig` (additive) with `secrets: Vec<...>`',
  'and `volumes: Vec<(mount_path, name)>` (default empty). The macro must parse a string-list for secrets and',
  'path=name pairs for volumes (a list of "MOUNT=NAME" strings is the simplest parseable form — design decides; map',
  'syntax is harder in attribute parsing). The bare macro + the existing gpu/timeout/cache args stay byte-identical.',
  'The facade `App::from_inventory` config map carries them into RemoteConfig/DeployConfig; `ensure_function`/',
  '`deploy_function` resolve secret_ids + volume_ids and attach them (run AND deploy). Also expose a config/builder',
  'override so non-macro users can set them.',
  '',
  '## Ground-truth references (READ)',
  '- crates/modal-rust-sdk/proto/api.proto (SecretGetOrCreate / SecretGetOrCreateRequest, Function.secret_ids field;',
  '  VolumeMount / Function.volume_mounts already used). crates/modal-rust-sdk/src/ops/{secret? (none yet — create it),',
  '  volume.rs (the GetOrCreate + FunctionVolumeMount pattern to mirror), function.rs (FunctionSpec — add secret_ids;',
  '  volume_mounts exists), mount.rs}.',
  '- references/modal-client/py/modal/secret.py (Secret.from_name / from_dict; SecretGetOrCreate object_creation_type;',
  '  how env vars are injected) + volume.py (from_name).',
  '- crates/modal-rust-macros/src/lib.rs (the gpu/timeout/cache arg parsing to extend) + crates/modal-rust-runtime/',
  '  src/lib.rs (FunctionConfig) + crates/modal-rust/src/{app.rs (from_inventory config map + resolve_function),',
  '  remote.rs (ensure_function + RemoteConfig), deploy.rs (deploy_function + DeployConfig)}.',
  '- workpads/shim-backend/knowledge.md (P4/P6 config-flow + volume patterns), TASKS.md.',
  '',
  '## FROZEN invariants — do NOT change',
  '- The runner protocol / HandlerFn / typed! / dispatch; the run-vs-deploy build boundary; retry_transient;',
  '  ephemeral-run vs persistent-deploy; the add_python/CUDA image paths; the cargo-scoped upload; spawn/map; the P6',
  '  cargo-cache volume (the user volume is a SEPARATE mount — do not disturb the cache logic). The bare macro + the',
  '  existing decorator args stay byte-identical.',
  '- Do NOT rewrite working create/invoke logic; ADD secrets + user-volume wiring. Do NOT touch README.md.',
  '',
  '## Verification rules (WORKING.md)',
  '- Gates on default-members: cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test',
  '  — all green. Live tests behind #[ignore] + the live feature. retry_transient on all RPCs. Modal flakiness => RETRY.',
  '- DRIVE the live proof to a terminal result. For the test, CREATE the secret + volume programmatically (SecretGetOrCreate',
  '  from_dict / volume_get_or_create) so no manual setup is needed; clean up / use ephemeral where possible. CPU, cheap.',
  '',
  '## How to return',
  'End with: "RESULT: <STATUS> — <one-line>". Build phases STATUS in {BUILD_GREEN, BUILD_FAILED} + exact cargo output.',
  'Live: proof the secret env var was readable in the fn + the volume file persisted across calls, or the precise error.',
].join('\n')

phase('Design')
const design = await parallel([
  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Design / macro + FunctionConfig for secrets+volumes + facade read). Read crates/modal-rust-macros/src/',
    'lib.rs (the gpu/timeout/cache arg parsing), crates/modal-rust-runtime/src/lib.rs (FunctionConfig), crates/modal-rust/',
    'src/app.rs (from_inventory config map + resolve_function). Produce a PRECISE spec for: extending `#[function(...)]`',
    'to parse `secrets = [".."]` (string list) and `volumes = ["/mount=name", ..]` (path=name pairs — simplest',
    'attribute-parseable form), backward-compatible (bare + gpu/timeout/cache unchanged); the additive FunctionConfig',
    'fields secrets (a list of static str names) + volumes (a list of (mount_path, name) static-str pairs), default',
    'empty; how the facade config map + RemoteConfig/DeployConfig carry them; and a non-macro config/builder override.',
    'RESULT: SPEC_DONE — macro+config secrets/volumes spec',
  ].join('\n'), { phase: 'Design', label: 'design:macro-config' }),

  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Design / SDK secret ops + FunctionSpec wiring). Read the proto SecretGetOrCreate + Function.secret_ids,',
    'crates/modal-rust-sdk/src/ops/{volume.rs (the GetOrCreate + FunctionVolumeMount pattern), function.rs (FunctionSpec)},',
    'and references/modal-client/py/modal/secret.py. Produce a PRECISE spec for: a new `ops/secret.rs`',
    '`secret_get_or_create(name) -> secret_id` (lookup by name) + `secret_from_dict(name, env) -> secret_id` (create,',
    'for tests/ephemeral; object_creation_type create), retry_unary; adding `FunctionSpec.secret_ids: Vec<String>` ->',
    '`Function.secret_ids` (additive, default empty). For USER volumes: reuse `volume_get_or_create` + `FunctionVolumeMount`',
    '(mount at the user path, writable) — confirm it composes with the P6 cache volume_mount (both in the list, distinct',
    'mount paths). How `ensure_function`/`deploy_function` resolve secret_ids + user-volume_ids from the config and attach',
    'them on run AND deploy. Cite proto fields + file:line. RESULT: SPEC_DONE — SDK secret/volume wiring spec',
  ].join('\n'), { phase: 'Design', label: 'design:sdk-wiring' }),
])

const spec = await agent(SHARED + '\n\n' + [
  'YOUR TASK (Synthesize). Merge the two notes into ONE build-ready spec at',
  ROOT + '/workpads/shim-backend/secrets-volumes-spec.md (overwrite): the macro/FunctionConfig secrets+volumes parsing,',
  'the SDK secret_get_or_create/secret_from_dict + FunctionSpec.secret_ids, the user-volume reuse of P6 plumbing',
  '(distinct mount, coexists with the cache volume), and the facade run+deploy wiring. Note which files change. Preserve',
  'the frozen invariants (esp. the P6 cache volume untouched; bare macro byte-identical). Resolve contradictions. Keep tight.',
  '',
  '=== MACRO + CONFIG NOTE ===',
  (design[0] || '(missing)'),
  '',
  '=== SDK SECRET/VOLUME WIRING NOTE ===',
  (design[1] || '(missing)'),
  '',
  'RESULT: SPEC_DONE — wrote secrets-volumes-spec.md',
].join('\n'), { phase: 'Design', label: 'design:synthesize' })

phase('Implement')
const impl = await agent(SHARED + '\n\n' + [
  'The spec is at ' + ROOT + '/workpads/shim-backend/secrets-volumes-spec.md — READ IT FIRST.',
  '',
  'YOUR TASK (Implement secrets + user volumes — HARD GATE on offline gates). Per the spec: extend the macro +',
  'FunctionConfig with `secrets`/`volumes` (additive, bare macro + gpu/timeout/cache byte-identical); add `ops/secret.rs`',
  '(secret_get_or_create + secret_from_dict, retry_unary) + `FunctionSpec.secret_ids` -> Function.secret_ids; resolve',
  'secret_ids + user-volume mounts from the config in `ensure_function` + `deploy_function` and attach them (run AND',
  'deploy), reusing the P6 volume plumbing for user volumes (distinct mount path; do NOT disturb the cache volume).',
  'Provide a non-macro config/builder override too. Do NOT touch README.md.',
  'VERIFY (offline hard): cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test',
  '(all default-members) — all green. Add unit tests (macro parses secrets/volumes; secret_ids in to_proto; bare macro',
  'unchanged; user volume + cache volume coexist). Paste exact output.',
  'RESULT: BUILD_GREEN — secrets + user volumes wired (decorator + config) into FunctionCreate; gates green   (or BUILD_FAILED — <reason>)',
].join('\n'), { phase: 'Implement', label: 'secrets-volumes-impl' })

const implGreen = /RESULT:\s*BUILD_GREEN/i.test(impl || '')
let live = null
if (!implGreen) {
  log('Implement HARD GATE not green — secrets/volumes did not compile. Skipping Live; Review documents the blocker.')
} else {
  phase('Live')
  live = await agent(SHARED + '\n\n' + [
    'secrets + user volumes are implemented and compile. Run the LIVE proof and DRIVE IT TO A TERMINAL RESULT yourself.',
    '',
    'YOUR TASK (LIVE — a fn with a secret + a user volume, against REAL Modal). CREATE the test resources',
    'programmatically (no manual setup): a secret via secret_from_dict (e.g. {MODAL_RUST_TEST_SECRET:"hello-secrets"})',
    'and a user volume via volume_get_or_create. Use a CPU function (decorated or config) that:',
    '  (a) reads the secret ENV VAR (std::env::var("MODAL_RUST_TEST_SECRET")) and returns it -> assert "hello-secrets";',
    '  (b) WRITES a file to the mounted user volume (e.g. /data/marker) on the first call, then a SECOND call READS it',
    '      back -> assert the value persisted (proves the volume is real persistent storage, committed across calls).',
    'Behind #[ignore] + the live feature. Modal flakiness => RETRY. If a real bug surfaces, make the MINIMAL fix +',
    're-verify offline gates. Confirm the P6 cargo cache still works alongside (the user volume + cache volume coexist).',
    'Ephemeral apps; clean up. Capture: the secret value read in the fn + the volume read-back proving persistence.',
    'RESULT: BUILD_GREEN — live: secret env injected (read in-fn) + user volume persisted (write->read across calls)',
    '   (or BUILD_FAILED — <real bug>   or INFRA_BLOCKED — <detail>)',
  ].join('\n'), { phase: 'Live', label: 'live-secrets-volumes' })
}

phase('Review')
const reviews = await parallel([
  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / secret+volume semantics + frozen). Verify against the code + references/modal-client',
    'secret.py/volume.py:',
    '- secrets: named secrets resolve to secret_ids attached to Function.secret_ids; env vars are injected (the live fn',
    '  read one). The from_dict create path is correct (object_creation_type). retry_unary on SecretGetOrCreate.',
    '- user volumes: resolved via volume_get_or_create + FunctionVolumeMount at the user mount path, writable, attached',
    '  to Function.volume_mounts — and they COEXIST with the P6 cargo-cache volume (distinct mount paths; the cache',
    '  logic is untouched). Quote the attach code showing both mounts.',
    '- additive: FunctionSpec.secret_ids + the new config fields default empty (existing functions wire-identical);',
    '  the bare macro + gpu/timeout/cache args unchanged; applied on BOTH run and deploy. README.md untouched.',
    'RESULT: PASS — secrets+volumes correct, cache volume intact, invariants held  (or FAIL — <what is wrong>)',
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
