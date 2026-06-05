export const meta = {
  name: 'readme-examples-validation',
  description: 'Make the README examples self-validating: each curated README example shows the EXACT run command (cd examples/<x> && cargo run …), the shown code corresponds to a real example crate (unrelated computation elided with // …, but ALL modal-rust code shown in full + matching the crate), and a test harness RUNS the offline run commands and asserts the documented output — so a stale README/example is a TEST FAILURE. Runs AFTER ergonomics-hardening (both touch README/examples).',
  phases: [
    { title: 'Design', detail: 'read the post-ergonomics README + the real example crates; pick the curated set, the exact run command for each, the elision boundary (project vs computation), and the run+drift test mechanism' },
    { title: 'Implement', detail: 'add the exact run bash to each shown example; align the shown code to the real crate (elide only unrelated computation); build the harness that runs the offline commands + a drift guard; gates green (HARD GATE)' },
    { title: 'Review', detail: 'parallel: (1) the run commands actually execute + match documented output, the shown code matches the crate (all project code present), curation sensible; (2) gates green incl. the new harness' },
  ],
}

const ROOT = '/Users/nicolas/devel/modal-rust'

const SHARED = [
  'Repo root: ' + ROOT + ' (git on main). GOAL (user convention, now in AGENTS.md "Examples & README Rules"): the README',
  'examples must be SELF-VALIDATING. By the time this runs, the ergonomics-hardening workflow has already (a) made the user',
  'path clean (modal_runner!() macro, package auto-detect, no facade rename, no manual AddCall import), (b) created real',
  'minimal example crate(s), and (c) made the README snippets match the crates with a drift guard. THIS workflow adds the',
  'run-command layer + the elision convention + actually EXECUTING the commands.',
  '',
  '## The convention to enforce (AGENTS.md)',
  '1. **Exact run command shown + tested.** For each example the README features, show the EXACT bash to run it (e.g.',
  '   `cd examples/<x> && cargo run --bin modal_runner -- --entrypoint add --input-json \'{"a":2,"b":3}\'` or',
  '   `cargo run -p example-<x> -- …`), immediately followed by the expected output. A test HARNESS must RUN the',
  '   offline-runnable commands and assert their stdout matches the documented output. A stale command/output => TEST FAILURE.',
  '2. **Shown code == real crate, with principled elision.** The code block shown for an example is the REAL crate\'s code.',
  '   You MAY elide implementation UNRELATED to modal-rust (the actual computation — e.g. a GPU kernel body, ML math) with a',
  '   `// …` marker, BUT every line of modal-rust integration a user must write (the single `modal-rust` dep, the',
  '   `#[modal_rust::function]` attribute + signature, `modal_runner!()`, the `App`/config/`.local()`/`.remote()` calls)',
  '   MUST be shown in full and MUST match the crate verbatim. A drift guard asserts the shown non-elided lines appear in the',
  '   real crate.',
  '3. **Curate.** Do NOT show every example — a few for clarity (e.g. the basic add quickstart, and ONE that shows config',
  '   like gpu where the computation is elided). The shown example is clearly the one the run command executes.',
  '',
  '## The run/test harness (the key new piece)',
  '- Build a test that PARSES README.md for the example run commands + their documented expected output, EXECUTES the',
  '  OFFLINE-runnable ones (the `--describe` / `--entrypoint …`/`.local()` style commands — no Modal, loopback at most), and',
  '  asserts stdout matches. Commands that REQUIRE Modal (`.remote()`/deploy/live) must NOT run in the offline gate — assert',
  '  those crates COMPILE and mark the command as creds-required (a clearly-labeled skip), so the gate stays offline + green.',
  '- AND a DRIFT GUARD test: for each README example block, the shown modal-rust lines (after stripping `// …` elisions)',
  '  must be found in the corresponding real crate file. Stale README => failing test.',
  '- Pick a clean home for the harness (a dev-only test target, e.g. crates/modal-rust/tests/readme_examples.rs or a small',
  '  xtask-style test) that runs under bare `cargo test`. Deterministic, offline, no creds.',
  '',
  '## FROZEN — additive',
  'Do NOT change the runner protocol / wire / the macro or App semantics / the ergonomics work just landed. This is docs +',
  'a test harness + (at most) light edits to the example crates to make elision clean. Keep examples/add (manual reference)',
  'and the live_* tests intact. Do NOT touch the user\'s uncommitted docs/{testing-strategy,ergonomics-and-docs-review}.md.',
  '',
  '## Ground-truth refs (READ — the POST-ergonomics state)',
  '- README.md (the authoring + run sections produced by ergonomics-hardening), workpads/shim-backend/ergonomics-hardening-spec.md.',
  '- The real example crates (the quickstart + add-macro + the GPU examples) — their Cargo.toml + lib.rs + how each is run.',
  '- AGENTS.md "Examples & README Rules" (the convention), docs/getting-started.md (if created).',
  '- Any drift-guard test ergonomics-hardening already added (extend it, do not duplicate).',
  '',
  '## Verification (offline = HARD gate; NO Modal/Python)',
  '- default-members + testkit: cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test —',
  '  all green, INCLUDING the new run-harness + drift-guard tests. Demonstrate the harness actually runs a README command',
  '  (paste its executed output matching the README). Prove a deliberately-wrong README command/output would fail (describe',
  '  or show the assertion). Confirm the offline gate runs NO Modal command.',
  '',
  '## How to return',
  'End with "RESULT: <STATUS> — <one-line>". STATUS in {BUILD_GREEN, BUILD_FAILED} + exact cargo output + the executed',
  'README-command output + the quoted run-harness/drift-guard test + the list of curated examples and their run commands.',
].join('\n')

phase('Design')
const spec = await agent(SHARED + '\n\n' + [
  'YOUR TASK (Design). READ the post-ergonomics README + the real example crates + AGENTS.md "Examples & README Rules".',
  'Decide: the CURATED set of README examples (e.g. the add quickstart + one config/gpu example with elided computation);',
  'for EACH, the exact run command(s) + the documented expected output, split into OFFLINE-runnable vs Modal-required; the',
  'elision boundary per example (which lines are project code that MUST show vs unrelated computation that may be `// …`);',
  'and the harness mechanism (how to parse README commands + expected output, run the offline ones, and the drift-guard that',
  'shown lines appear in the crate) + where it lives. Write it to ' + ROOT + '/workpads/shim-backend/readme-validation-spec.md.',
  'Flag any example whose run command cannot be made offline-testable. RESULT: SPEC_DONE — wrote readme-validation-spec.md',
].join('\n'), { phase: 'Design', label: 'design:readme-validation' })

phase('Implement')
const impl = await agent(SHARED + '\n\n' + [
  'The spec is at ' + ROOT + '/workpads/shim-backend/readme-validation-spec.md — READ IT FIRST.',
  '',
  'YOUR TASK (Implement — HARD GATE). Per the spec: in README.md, ensure each curated example shows the EXACT run',
  'command(s) + expected output, and the shown code matches the real crate (elide ONLY unrelated computation with `// …`;',
  'show ALL modal-rust code in full). Make small edits to the example crates only if needed for clean elision. Build the',
  'run-harness test (parses the README run commands + expected output, EXECUTES the offline ones and asserts stdout; skips',
  'Modal-required ones with a clear label but compiles their crates) and the drift-guard test (shown non-elided lines exist',
  'in the crate). Keep everything additive + offline + deterministic.',
  'VERIFY (offline HARD, paste exact output): cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ;',
  'cargo test — all green incl. the new tests. Paste the harness ACTUALLY RUNNING a README command (its stdout matching the',
  'README). QUOTE the run-harness + drift-guard tests + the curated examples with their run commands.',
  'RESULT: BUILD_GREEN — README examples self-validating (run commands executed + drift-guarded); gates green   (or BUILD_FAILED — <reason>)',
].join('\n'), { phase: 'Implement', label: 'build:readme-validation' })

const implGreen = /RESULT:\s*BUILD_GREEN/i.test(impl || '')
if (!implGreen) log('Implement HARD GATE not green — Review documents the blocker.')

phase('Review')
const reviews = await parallel([
  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / convention satisfied). Verify against README.md + the crates + the tests: every curated example',
    'shows an EXACT run command + expected output; the offline ones are actually EXECUTED by the harness and asserted (show',
    'one running); the drift guard ties the shown code to the real crate (and would fail if the README drifted — show why);',
    'elision is principled (ALL modal-rust code shown, only unrelated computation trimmed with `// …`); curation is sensible.',
    'Confirm a newcomer could copy-paste the dep + the shown code + the run command and it works. RESULT: PASS — README examples self-validating per the convention  (or FAIL — <what is wrong>)',
  ].join('\n'), { phase: 'Review', label: 'review:convention' }),

  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / gates — RUN them). From ' + ROOT + ': cargo fmt --check ; cargo clippy --all-targets -- -D',
    'warnings ; cargo build ; cargo test — report exact output + exit status; all green INCLUDING the new run-harness +',
    'drift-guard tests. Confirm the offline gate runs NO Modal command (the harness skips creds-required commands), it is',
    'deterministic, and nothing in the runner protocol / macro / App / live_* tests regressed. Report failures verbatim.',
    'RESULT: PASS — gates green, harness offline+deterministic  (or FAIL — <exact failing cmd+output>)',
  ].join('\n'), { phase: 'Review', label: 'review:gates' }),
])

return { impl_green: implGreen, impl, reviews }
