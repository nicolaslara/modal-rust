export const meta = {
  name: 'p6-cargo-cache',
  description: 'P6: cargo build cache for the run path — a V2 Volume holding ONE compressed archive (cache.tar.zst) of CARGO_HOME(+target), unpacked on container start / repacked on exit (background commit). ON by default; opt-out via #[modal_rust::function(cache=false)]. Benchmark cold-vs-warm on the heavy burn-add build.',
  phases: [
    { title: 'Design', detail: 'SDK Volume support + FunctionSpec.volume_mounts + wrapper archive pack/unpack + cache-on-by-default wiring -> spec' },
    { title: 'Implement', detail: 'ops/volume.rs (VolumeGetOrCreate v2) + FunctionSpec.volume_mounts + wrapper cache logic + cache config; gates green (HARD GATE)' },
    { title: 'Live', detail: 'mechanism proof (volume mounts, archive unpack/repack, warm reuse) + cold-vs-warm benchmark on burn-add; cache=false opt-out' },
    { title: 'Review', detail: 'parallel: cache correctness (a miss never changes results) + build boundary + frozen invariants; hygiene' },
  ],
}

const ROOT = '/Users/nicolas/devel/modal-rust'

const SHARED = [
  'You are implementing P6 (the cargo build cache) on modal-rust (repo root: ' + ROOT + '; git on main).',
  '',
  '## Where we are',
  'Full product proven live (CPU+GPU, run/deploy/call, CLI+facade, spawn/map). The `run` path builds the user crate',
  'IN THE FUNCTION BODY at execution time — so a fresh container recompiles from scratch (slow for heavy crates like',
  'burn-add). What is missing: a build CACHE so warm runs skip re-fetching/re-compiling. The decorator already carries',
  '`cache: Option<bool>` (P4 FunctionConfig) — it is plumbed but unused. P6 makes it real, ON by default.',
  '',
  '## The CHOSEN mechanism (knowledge.md §C — do NOT deviate to the naive approach)',
  'Modal volumes DEGRADE past ~50k files (latency scales with file count), so putting CARGO_HOME/target DIRECTLY on a',
  'mounted Volume (as the old prototype dev_app.py M6 did) is the WORST case. The chosen design is',
  '**archive-as-single-object on a V2 Volume**:',
  '  1. Build on FAST LOCAL DISK: CARGO_HOME=/tmp/cargo, CARGO_TARGET_DIR=/tmp/target (NEVER on the mounted volume).',
  '  2. Persist the cache as ONE compressed archive `cache.tar.zst` on a V2 Volume mounted at a STABLE path (e.g.',
  '     /cache). On container START: if the archive exists, unpack it to /tmp (warm); else cold. On EXIT: repack the',
  '     changed dirs into the single archive + rely on AUTOMATIC background commit (`allow_background_commits=true`) —',
  '     NO `vol.reload()` on the hot path (cargo holds locks).',
  '  3. Scope: CARGO_HOME (registry index + downloaded crates — high value, mostly-read) + optionally target/.',
  '  4. V2 volume (concurrent writes). Wire dynamically: VolumeGetOrCreate("modal-rust-cargo-cache", v2) ->',
  '     Function.volume_mounts[{volume_id, mount_path:"/cache", allow_background_commits:true}].',
  '  5. DEFAULT ON; opt-out via `#[modal_rust::function(cache=false)]` (the decorator config) — and `MODAL_RUST_NO_CACHE`',
  '     / a RemoteConfig knob. A cache miss must ONLY cost time, NEVER change the result (correctness rule).',
  'This is a RUN-path optimization (build-in-body). DEPLOY builds once at image-build time (Modal caches the image),',
  'so the cargo cache primarily helps `.remote()`/`run`. Apply it there.',
  '',
  '## Ground-truth references (READ)',
  '- workpads/shim-backend/knowledge.md section C ("Native Volume bulk-copy answer + chosen cache-on-by-default',
  '  mechanism") — the authoritative design. workpads/prototype/dev_app.py (the M6 `run_entrypoint_cached` recipe —',
  '  but use the ARCHIVE approach from §C, not CARGO_HOME-directly-on-volume).',
  '- crates/modal-rust-sdk/proto/api.proto (VolumeGetOrCreate / VolumeGetOrCreateRequest, VolumeMount{volume_id,',
  '  mount_path, allow_background_commits, read_only}, Function.volume_mounts field).',
  '- crates/modal-rust-sdk/src/ops/{function.rs (FunctionSpec — add volume_mounts; to_proto), mount.rs (the',
  '  GetOrCreate pattern to mirror for VolumeGetOrCreate), image.rs}; crates/modal-rust/src/remote.rs (the run-path',
  '  FILE-mode WRAPPER — add the cache unpack/repack; RemoteConfig + the cache config from the decorator).',
  '- references/modal-client/py/modal/volume.py (Volume.from_name version=2, create_if_missing, allow_background_commits;',
  '  VolumeGetOrCreate fields) for parity.',
  '- crates/modal-rust-macros + modal-rust-runtime FunctionConfig.cache (already exists from P4); crates/modal-rust/',
  '  src/app.rs (from_inventory config map -> RemoteConfig).',
  '- TASKS.md, workpads/shim-backend/knowledge.md (P4/spawn-map sections for the config-flow pattern).',
  '',
  '## FROZEN invariants — do NOT change',
  '- The runner protocol / HandlerFn / typed! / dispatch; the run-vs-deploy build boundary (cache is a run-path build',
  '  ACCELERATOR — the build still happens in the function body; correctness never depends on cache state); retry_transient;',
  '  ephemeral-run vs persistent-deploy; the add_python / CUDA image paths; the cargo-scoped upload; spawn/map.',
  '- Do NOT rewrite the working create/invoke logic; ADD volume support + the wrapper cache step. Do NOT touch README.md.',
  '',
  '## Verification rules (WORKING.md)',
  '- Gates on default-members: cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test',
  '  — all green. Live tests behind #[ignore] + the live feature. retry_transient on all RPCs. Modal flakiness => RETRY.',
  '- DRIVE the live proof to a terminal result. The cold-vs-warm benchmark on burn-add is HEAVY+slow (CUDA build) — be',
  '  patient; if it is too flaky, a mechanism proof (archive written+reused, warm CARGO_HOME present) on a heavy-ish',
  '  crate + a recorded cold-vs-warm delta is acceptable. Use ephemeral run apps; reset the cache volume between cold runs.',
  '',
  '## How to return',
  'End with: "RESULT: <STATUS> — <one-line>". Build phases STATUS in {BUILD_GREEN, BUILD_FAILED} + exact cargo output.',
  'Live: the cold-vs-warm timing (or mechanism evidence: archive written/reused, warm cache present) + cache=false opt-out,',
  'or the precise error after retries.',
].join('\n')

phase('Design')
const design = await parallel([
  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Design / SDK Volume + FunctionSpec.volume_mounts). Read the proto VolumeGetOrCreate/VolumeMount/',
    'Function.volume_mounts, crates/modal-rust-sdk/src/ops/{function.rs,mount.rs}, and references/modal-client/py/modal/',
    'volume.py. Produce a PRECISE spec for: a new `ops/volume.rs` `volume_get_or_create("name", v2, create_if_missing)',
    '-> volume_id` (mirror the mount GetOrCreate pattern, retry_transient, V2/version field); extending `FunctionSpec`',
    'with `volume_mounts: Vec<VolumeMount{volume_id, mount_path, allow_background_commits}>` wired into `to_proto`',
    '(Function.volume_mounts) — additive, default empty (so existing functions are unchanged). Cite proto fields.',
    'RESULT: SPEC_DONE — SDK volume + volume_mounts spec',
  ].join('\n'), { phase: 'Design', label: 'design:sdk-volume' }),

  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Design / wrapper cache archive + cache-on-by-default wiring). Read knowledge.md §C, workpads/prototype/',
    'dev_app.py (M6 cached recipe), crates/modal-rust/src/remote.rs (the run wrapper + RemoteConfig + the from_inventory',
    'config map), and the FunctionConfig.cache field. Produce a PRECISE spec for:',
    '- The wrapper cache logic (the run-path FILE-mode wrapper): on start, if cache=on and /cache/cache.tar.zst exists,',
    '  unpack to /tmp (CARGO_HOME=/tmp/cargo, CARGO_TARGET_DIR=/tmp/target); build there; on exit, repack CARGO_HOME',
    '  (+ optionally target) into /cache/cache.tar.zst (single object) and rely on background commit (NO vol.reload).',
    '  Exact tar+zstd commands; what to scope (CARGO_HOME registry/index at minimum). Build NEVER on the mounted volume.',
    '- The cache-on-by-default wiring: cache defaults ON; `#[modal_rust::function(cache=false)]` (FunctionConfig.cache) +',
    '  a RemoteConfig knob / MODAL_RUST_NO_CACHE opt out. When ON, the facade resolves the V2 volume (volume_get_or_create)',
    '  and attaches it via FunctionSpec.volume_mounts on the RUN path only. The correctness rule: a cache miss only costs',
    '  time, never changes the result.',
    '- The cold-vs-warm benchmark plan on burn-add (run path, CUDA image) + a cheaper mechanism check.',
    'Cite file:line. RESULT: SPEC_DONE — wrapper cache + cache-on-by-default spec',
  ].join('\n'), { phase: 'Design', label: 'design:wrapper-cache' }),
])

const spec = await agent(SHARED + '\n\n' + [
  'YOUR TASK (Synthesize). Merge the two notes into ONE build-ready spec at',
  ROOT + '/workpads/shim-backend/p6-cache-spec.md (overwrite): the SDK volume_get_or_create + FunctionSpec.volume_mounts,',
  'the wrapper archive unpack/repack (build on /tmp, archive on a V2 volume, background commit), and the',
  'cache-ON-by-default wiring (decorator cache=false opt-out; run-path only). Note which files change. Preserve the',
  'frozen invariants + the §C archive design (NOT CARGO_HOME-directly-on-volume) + the cache-miss-never-wrong rule.',
  'Resolve contradictions. Keep tight.',
  '',
  '=== SDK VOLUME NOTE ===',
  (design[0] || '(missing)'),
  '',
  '=== WRAPPER CACHE + WIRING NOTE ===',
  (design[1] || '(missing)'),
  '',
  'RESULT: SPEC_DONE — wrote p6-cache-spec.md',
].join('\n'), { phase: 'Design', label: 'design:synthesize' })

phase('Implement')
const impl = await agent(SHARED + '\n\n' + [
  'The spec is at ' + ROOT + '/workpads/shim-backend/p6-cache-spec.md — READ IT FIRST.',
  '',
  'YOUR TASK (Implement P6 — HARD GATE on offline gates). Per the spec: add `ops/volume.rs`',
  '(volume_get_or_create, V2, retry_transient) + `FunctionSpec.volume_mounts` (additive, to_proto -> Function.volume_mounts);',
  'add the run-path WRAPPER cache logic (unpack /cache/cache.tar.zst -> /tmp on start; build on /tmp; repack on exit;',
  'background commit, no reload); wire cache ON by default with `#[function(cache=false)]` + RemoteConfig/MODAL_RUST_NO_CACHE',
  'opt-out, attaching the V2 volume on the RUN path only. A cache miss must never change results. Do NOT rewrite the',
  'working create/invoke logic; do NOT touch README.md; do NOT alter the deploy build-at-image-time path.',
  'VERIFY (offline hard): cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test',
  '(all default-members) — all green. Add unit tests (volume_mounts in to_proto; cache default-on; cache=false omits the',
  'volume). Paste exact output.',
  'RESULT: BUILD_GREEN — volume support + wrapper cache + cache-on-by-default; gates green   (or BUILD_FAILED — <reason>)',
].join('\n'), { phase: 'Implement', label: 'p6-impl' })

const implGreen = /RESULT:\s*BUILD_GREEN/i.test(impl || '')
let live = null
if (!implGreen) {
  log('Implement HARD GATE not green — P6 cache did not compile. Skipping Live; Review documents the blocker.')
} else {
  phase('Live')
  live = await agent(SHARED + '\n\n' + [
    'P6 is implemented and compiles. Run the LIVE proof and DRIVE IT TO A TERMINAL RESULT yourself (be PATIENT — the',
    'burn-add build is heavy).',
    '',
    'YOUR TASK (LIVE — cache mechanism + cold-vs-warm). Against REAL Modal (ephemeral run apps):',
    '  1. MECHANISM: a `.remote()` run with cache ON attaches the V2 volume, unpacks (cold: none) / repacks',
    '     /cache/cache.tar.zst on exit (confirm the archive object is written + committed; a second run finds + unpacks it,',
    '     warm CARGO_HOME present). cache=false (or MODAL_RUST_NO_CACHE) attaches NO volume.',
    '  2. BENCHMARK on burn-add (run path, CUDA image): COLD run (reset/empty cache volume) build time vs WARM run',
    '     (populated cache) build time — show the warm run is measurably faster (registry/index + crate downloads, and',
    '     ideally target, reused). If the full burn cold-vs-warm is too slow/flaky after honest retries, record the',
    '     mechanism proof + whatever cold-vs-warm delta you obtained (even on a lighter heavy-deps crate) and say so.',
    'Modal flakiness => RETRY. If a real bug surfaces, make the MINIMAL fix + re-verify offline gates. CRITICAL: confirm',
    'a cache miss still produces the CORRECT result (run with an empty cache -> same output).',
    'Capture: the archive written/reused evidence; the cold-vs-warm timings; cache=false omits the volume; correctness held.',
    'RESULT: BUILD_GREEN — cache works (archive persisted+reused), warm < cold on the heavy build, miss-safe, opt-out works',
    '   (or BUILD_FAILED — <real bug>   or INFRA_BLOCKED — <detail + the mechanism evidence obtained>)',
  ].join('\n'), { phase: 'Live', label: 'live-cache' })
}

phase('Review')
const reviews = await parallel([
  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / cache correctness + boundary). Verify against the code + knowledge.md §C:',
    '- The cache is the §C ARCHIVE approach (single cache.tar.zst on a V2 volume; build on /tmp, NOT on the mounted',
    '  volume; background commit, no vol.reload on the hot path) — NOT CARGO_HOME-directly-on-volume.',
    '- A cache MISS only costs time, never changes the result (the build inputs are the mounted source + the runner;',
    '  correctness does not depend on cache state). Quote the wrapper logic.',
    '- cache is ON by default; `#[function(cache=false)]` / the opt-out attaches NO volume. The volume is attached on the',
    '  RUN path only (deploy build-at-image-time is unchanged). FunctionSpec.volume_mounts is additive (default empty ->',
    '  existing functions unchanged). retry_transient on VolumeGetOrCreate.',
    'RESULT: PASS — cache archive design + miss-safe + run-path-only + opt-out correct  (or FAIL — <what is wrong>)',
  ].join('\n'), { phase: 'Review', label: 'review:cache-correctness' }),

  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / hygiene — RUN the gates). From ' + ROOT + ' report exact output + exit status:',
    '- cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test  (default-members).',
    'Confirm example-burn-add still excluded from default-members; cuda-vector-add builds without CUDA; live tests',
    '#[ignore]+live gated; README.md untouched; no hand-written file grossly exceeds ~500 LOC. Report failures verbatim.',
    'RESULT: PASS — gates green  (or FAIL — <exact failing command + output>)',
  ].join('\n'), { phase: 'Review', label: 'review:hygiene' }),
])

return { impl_green: implGreen, impl, live, reviews }
