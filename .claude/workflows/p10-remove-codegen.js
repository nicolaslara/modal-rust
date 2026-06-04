export const meta = {
  name: 'p10-remove-codegen',
  description: 'P10: delete the legacy Python-shim codegen from the CLI now that the programmatic path is proven everywhere. Remove templates.rs + templates/*.tmpl + the --use-shim flag + cmd_*_shim + the doctor modal-CLI requirement. The CLI becomes purely programmatic. Keep the programmatic run/deploy/call working (offline gates + a light live re-confirm).',
  phases: [
    { title: 'Design', detail: 'inventory what to delete (templates, --use-shim, cmd_*_shim, doctor modal check, dead tests) + confirm no other refs -> spec' },
    { title: 'Implement', detail: 'delete codegen + simplify CLI to programmatic-only + doctor; gates green (HARD GATE)' },
    { title: 'Live', detail: 'modal-rust run/deploy/call add -> {sum:42} (programmatic, no --use-shim); light re-confirm' },
    { title: 'Review', detail: 'parallel: no dangling refs to deleted code + programmatic path intact + frozen invariants; hygiene' },
  ],
}

const ROOT = '/Users/nicolas/devel/modal-rust'

const SHARED = [
  'You are doing P10 (final cleanup) on modal-rust (repo root: ' + ROOT + '; git on main).',
  '',
  '## Where we are',
  'The programmatic path is PROVEN LIVE everywhere: `.local()`/`.remote()`/`deploy`/`call` (CPU + GPU), the CLI',
  '(P9, programmatic by default), `.spawn()`/`.map()`, the cargo cache (P6), and secrets/volumes. In P9 the legacy',
  'Python-shim codegen was kept behind a `--use-shim` flag as a safety net. It is no longer needed. P10 DELETES it.',
  '',
  '## What to delete (the CLI codegen / shim path)',
  '- crates/modal-rust-cli/src/templates.rs and crates/modal-rust-cli/src/templates/*.tmpl (the Python shim renderer +',
  '  the dev/deploy/call .py templates).',
  '- The `--use-shim` flag on run/deploy/call (and doctor), and the `cmd_*_shim` functions in main.rs (the path that',
  '  rendered Python + shelled out to the official `modal` CLI). The CLI becomes PURELY programmatic (the P9',
  '  `programmatic.rs` path is the only path).',
  '- The doctor `modal`-CLI requirement / the shim-specific doctor branch — doctor keeps ONLY the programmatic preflight',
  '  (auth: ~/.modal.toml / MODAL_TOKEN_*, plus the `--rust` cargo/rustc checks).',
  '- The now-dead shim tests in main.rs (the byte-equivalence `dev_shim`/`deploy_shim` tests, the `*_never_emits_gpu_kwarg`',
  '  tests, etc.) — delete them (they test deleted code).',
  '',
  '## What to KEEP (do NOT delete)',
  '- crates/modal-rust-cli/src/programmatic.rs (the programmatic run/deploy/call — this IS the path now).',
  '- The runner / facade / SDK / examples — untouched.',
  '- workpads/prototype/*.py and workpads/gpu-compute/*.py — these are HISTORICAL PROTOTYPE REFERENCE (the proven',
  '  recipes, now encoded in the SDK). Leave them as reference; do NOT delete workpad files. (P10 removes the',
  '  per-project codegen in the SHIPPING CLI, not the project\'s historical notes.)',
  '- README.md — do NOT edit it (the orchestrator updates docs separately; it will drop the --use-shim mention).',
  '',
  '## FROZEN invariants — do NOT change',
  '- The runner protocol / HandlerFn / typed! / dispatch; the run-vs-deploy build boundary; retry_transient;',
  '  ephemeral-run vs persistent-deploy; the add_python/CUDA image paths; cargo-scoped upload; cache; secrets/volumes;',
  '  spawn/map; the decorator config (gpu/timeout/cache/secrets/volumes). P10 ONLY removes the dead shim codegen +',
  '  simplifies the CLI to programmatic-only. The programmatic run/deploy/call behavior is UNCHANGED.',
  '',
  '## Ground-truth references (READ)',
  '- crates/modal-rust-cli/src/{main.rs (the clap commands, the --use-shim branch + cmd_*_shim, the shim tests),',
  '  templates.rs, templates/*.tmpl, doctor.rs, programmatic.rs (KEEP)}.',
  '- crates/modal-rust-cli/Cargo.toml (drop any deps that only the shim path used, e.g. a template engine, IF unused',
  '  after deletion — verify with cargo).',
  '- workpads/shim-backend/knowledge.md (P9 section), TASKS.md (the P10 line).',
  '',
  '## Verification rules (WORKING.md)',
  '- Gates on default-members: cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test',
  '  — all green AFTER deletion (no dangling references; dead tests removed). Live test gated. Modal flakiness => RETRY.',
  '- DRIVE the light live re-confirm to a terminal result (the programmatic path is unchanged, so this should just work).',
  '  CPU, ephemeral run app, cheap.',
  '',
  '## How to return',
  'End with: "RESULT: <STATUS> — <one-line>". Build phases STATUS in {BUILD_GREEN, BUILD_FAILED} + exact cargo output.',
  'Live: the run/deploy/call outputs ({sum:42}) confirming the programmatic path works with NO --use-shim, or the error.',
].join('\n')

phase('Design')
const spec = await agent(SHARED + '\n\n' + [
  'YOUR TASK (Design the deletion). Read crates/modal-rust-cli/src/{main.rs, templates.rs, doctor.rs, programmatic.rs,',
  'Cargo.toml} + ls crates/modal-rust-cli/src/templates/. WRITE a build-ready spec to',
  ROOT + '/workpads/shim-backend/p10-spec.md listing EXACTLY: which files to delete (templates.rs, templates/*.tmpl,',
  'their `mod templates;`), which main.rs items to remove (the `--use-shim` flag on each command, the `cmd_*_shim`',
  'functions, the `if use_shim {…} else {…}` branches collapsed to the programmatic arm, the dead shim/gpu-kwarg tests),',
  'the doctor changes (drop the modal-CLI requirement + shim branch; keep auth + --rust cargo/rustc), and any',
  'Cargo.toml deps to drop if now-unused. Grep the workspace to CONFIRM nothing else references templates.rs / the',
  'deleted symbols (so deletion leaves no dangling refs). Keep programmatic.rs + the runner/facade/SDK/workpads',
  'untouched. Cite file:line. RESULT: SPEC_DONE — wrote p10-spec.md',
].join('\n'), { phase: 'Design', label: 'design:deletion' })

phase('Implement')
const impl = await agent(SHARED + '\n\n' + [
  'The spec is at ' + ROOT + '/workpads/shim-backend/p10-spec.md — READ IT FIRST.',
  '',
  'YOUR TASK (Delete the codegen — HARD GATE on offline gates). Per the spec: delete templates.rs + templates/*.tmpl',
  '(+ the `mod templates;`); remove the `--use-shim` flag, the `cmd_*_shim` functions, and collapse the dispatch to the',
  'programmatic path only; simplify doctor (drop the modal-CLI requirement + shim branch; keep auth + --rust); delete',
  'the dead shim/gpu-kwarg tests; drop any now-unused Cargo.toml deps. KEEP programmatic.rs. Do NOT touch the runner/',
  'facade/SDK/examples/workpads or README.md. The programmatic run/deploy/call behavior must be UNCHANGED.',
  'VERIFY (offline hard): cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test',
  '(all default-members) — all green (no dangling refs; dead tests gone). Paste exact output + confirm templates.rs and',
  'templates/ are gone and `grep -rn "use_shim\\|cmd_run_shim\\|templates::" crates/` is empty.',
  'RESULT: BUILD_GREEN — legacy shim codegen deleted; CLI is programmatic-only; gates green   (or BUILD_FAILED — <reason>)',
].join('\n'), { phase: 'Implement', label: 'p10-impl' })

const implGreen = /RESULT:\s*BUILD_GREEN/i.test(impl || '')
let live = null
if (!implGreen) {
  log('Implement HARD GATE not green — P10 deletion broke the build. Skipping Live; Review documents the blocker.')
} else {
  phase('Live')
  live = await agent(SHARED + '\n\n' + [
    'The shim codegen is deleted and the CLI compiles programmatic-only. Run a LIGHT live re-confirm and DRIVE IT TO A',
    'TERMINAL RESULT yourself (the programmatic path is unchanged from P9, so this should just work).',
    '',
    'YOUR TASK (LIVE re-confirm). Using examples/add, against REAL Modal (ephemeral run app, CPU, cheap):',
    '  - `modal-rust run add --project examples/add --input \'{"a":40,"b":2}\'` -> {"ok":true,"value":{"sum":42}} via the',
    '    programmatic path. Confirm there is NO `--use-shim` flag anymore (the CLI rejects it / it does not exist) and',
    '    no `.modal-rust/generated/*.py` is written and no `modal` subprocess is spawned.',
    '  - (optional, if quick) `deploy` + `call` once for parity.',
    'Modal flakiness => RETRY. If a real bug surfaces, make the MINIMAL fix + re-verify offline gates.',
    'Capture: the run output + confirmation --use-shim is gone and no .py/no modal subprocess.',
    'RESULT: BUILD_GREEN — programmatic CLI still works post-deletion (run add == {sum:42}, no --use-shim, no .py/modal)',
    '   (or BUILD_FAILED/INFRA_BLOCKED — <detail>)',
  ].join('\n'), { phase: 'Live', label: 'live-reconfirm' })
}

phase('Review')
const reviews = await parallel([
  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / clean deletion + intact behavior). Verify against the code:',
    '- templates.rs + templates/*.tmpl are GONE; `grep -rn "templates::|use_shim|cmd_run_shim|cmd_deploy_shim|cmd_call_shim"',
    '  crates/` returns nothing (no dangling refs). The `--use-shim` flag is removed from run/deploy/call/doctor.',
    '- The CLI run/deploy/call now have a SINGLE programmatic path (programmatic.rs) — behavior unchanged from P9.',
    '- doctor no longer requires the `modal` CLI; keeps auth + --rust cargo/rustc.',
    '- KEPT: programmatic.rs, the runner/facade/SDK/examples, and the workpads/*.py historical reference (not deleted).',
    '  Frozen invariants (runner protocol, build boundary, cache, secrets/volumes, spawn/map, decorator config) intact.',
    'RESULT: PASS — codegen cleanly removed, programmatic path intact  (or FAIL — <what is wrong>)',
  ].join('\n'), { phase: 'Review', label: 'review:clean-deletion' }),

  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / hygiene — RUN the gates). From ' + ROOT + ' report exact output + exit status:',
    '- cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test  (default-members).',
    'Confirm example-burn-add still excluded from default-members; live tests #[ignore]+live gated; README.md untouched',
    '(orchestrator updates it); no hand-written file grossly exceeds ~500 LOC. Report failures verbatim.',
    'RESULT: PASS — gates green  (or FAIL — <exact failing command + output>)',
  ].join('\n'), { phase: 'Review', label: 'review:hygiene' }),
])

return { impl_green: implGreen, impl, live, reviews }
