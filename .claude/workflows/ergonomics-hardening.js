export const meta = {
  name: 'ergonomics-hardening',
  description: 'Fix the newcomer P0s so the minimal user path is clean and the README examples are REAL: (1) a modal_runner!() macro that kills the hand-written runner bin + the __private leak, (2) .remote() package auto-detection (no MODAL_RUST_PACKAGE footgun), (3) resolve the modal_rust import collision so `use modal_rust::function` works with NO facade rename, (4) make the typed app.add(2,3) method auto-available (no manual AddCall import). Then turn the README examples into real, compiled, tested crates that read exactly as a user would write them, and rewrite the README + a Getting Started page to match (with a drift guard). VERIFY each claimed bug against the code before fixing.',
  phases: [
    { title: 'Design', detail: 'VERIFY each P0 against the code (remote.rs package hardcode, __private leak, the modal_rust rename reason, the AddCall import); design modal_runner!(), package auto-detect (env! CARGO_PKG_NAME), the import-collision fix, AddCall auto-availability, the real-example-crate layout + README drift guard' },
    { title: 'Ergonomics', detail: 'implement modal_runner!() + package auto-detect + import-collision fix + AddCall prelude; gates green; a clean test crate (no runner bin/rename/env var) builds + --describe + .local() works (HARD GATE)' },
    { title: 'Examples', detail: 'make the README examples REAL minimal crates (no boilerplate), compiled + tested in the workspace; migrate add-macro to the clean form; keep the others green (HARD GATE)' },
    { title: 'Docs', detail: 'rewrite README install/authoring to match the real clean crate + fix the snippet; add a Getting Started page (token setup, zero->local->remote->deploy, concepts, Py->Rust cheat sheet, troubleshooting); add a drift guard so README snippets == real crate' },
    { title: 'Review', detail: 'parallel: (1) a newcomer can reproduce from the README; ergonomics clean (no boilerplate/__private/env-var/rename); (2) gates green + frozen (runner protocol/wire/existing live tests intact)' },
  ],
}

const ROOT = '/Users/nicolas/devel/modal-rust'

const SHARED = [
  'Repo root: ' + ROOT + ' (git on main). GOAL: make the NEWCOMER path clean and make the README examples REAL. The user',
  'LIKES the simple README snippets (simpler than the current examples) and wants them to be actual crates that compile +',
  'are tested + read exactly as a real user would write them. "The cleaner the better." The fixes below are what MAKE the',
  'user code that clean. Findings come from docs/ergonomics-and-docs-review.md (READ it) — but VERIFY each against the code',
  'before changing anything (the reviewer was sharp but confirm every claim with file:line).',
  '',
  '## The P0s to fix',
  '1. **`modal_runner!()` macro — kill the boilerplate + the __private leak.** Every crate hand-writes',
  '   `src/bin/modal_runner.rs` (+ a `[[bin]]` stanza), and the macro-path version writes',
  '   `modal_rust_facade::__private::runtime::{from_inventory_with_configs, run_cli_with_configs}` — i.e. USER-WRITTEN code',
  '   touching `__private`, which crates/modal-rust/src/lib.rs (~:81) marks "NOT a stable public API." Ship a public',
  '   `modal_rust::modal_runner!()` (or an attribute) that EXPANDS to the runner main, so the user writes ONE line and the',
  '   __private usage lives only in GENERATED code (like serde_derive). The user must NOT need a separate runner bin file at',
  '   all if avoidable, or at most `// src/bin/modal_runner.rs` = `modal_rust::modal_runner!();`. Decide the cleanest shape.',
  '2. **`.remote()` builds the wrong package (footgun).** crates/modal-rust/src/remote.rs hardcodes `package = "example-add"`',
  '   in `RemoteConfig::default()` (VERIFY the exact line) unless `MODAL_RUST_PACKAGE` is set. The CLI auto-detects via',
  '   `--project`, but the documented library path `App::connect(...).remote()` would try to build `example-add` for ANY',
  '   crate. FIX: auto-detect the package. The clean mechanism: the macro (`#[modal_rust::function]` and/or `modal_runner!()`)',
  '   expands in the USER crate, so `env!("CARGO_PKG_NAME")` THERE is the user\'s package — capture it and thread it into the',
  '   config the facade reads (so `.remote()` builds the right `-p <pkg>` automatically; MODAL_RUST_PACKAGE stays as an',
  '   override). VERIFY the package name flows: registration/inventory -> RemoteConfig -> the `cargo build -p <pkg> --bin',
  '   modal_runner` in the run wrapper.',
  '3. **`use modal_rust::function;` must work with NO facade rename.** The README shows `modal-rust = {..}` +',
  '   `use modal_rust::function;` + `#[modal_rust::function]`, but every example uses `modal_rust_facade = { package =',
  '   "modal-rust" }` + `extern crate modal_rust_facade as modal_rust;`. FIND OUT WHY the rename is needed (the comment',
  '   blames an `extern crate modal_rust_macros as modal_rust` shadow) and FIX THE ROOT CAUSE so a fresh crate that just',
  '   depends on `modal-rust` and writes `use modal_rust::function;` compiles + the macro expands. If the rename is NOT',
  '   actually needed anymore, prove it and drop it from the examples.',
  '4. **The typed `app.add(2,3)` method must be usable without a manual `AddCall` import.** Today the macro generates a',
  '   per-fn `AddCall` extension trait that must be in scope. Make it auto-available — e.g. emit the trait into a',
  '   crate-level prelude/glob, or attach the method without a user-visible trait import — so the README snippet',
  '   `let sum = app.add(2,3).local()?;` compiles with only the natural imports. (If a `use mycrate::*` or a documented',
  '   one-liner is unavoidable, pick the least-surprising option and document it.)',
  '',
  '## Then: REAL, tested README examples',
  '- Once #1-#4 land, a user crate is ~just: a Cargo.toml with the single `modal-rust` dep (+ serde/anyhow), a few-line',
  '  `#[modal_rust::function] fn add(a,b)->anyhow::Result<i64>`, and `modal_runner!()` — NO __private, NO rename, NO env',
  '  var, NO hand-written bin boilerplate. Create the clean minimal example crate(s) that the README shows (a "quickstart"),',
  '  as their OWN workspace crate(s), and BUILD + TEST them (offline: --describe + .local()). Migrate examples/add-macro to',
  '  this clean form (drop the runner bin + the facade rename); keep examples/add (manual) + the GPU examples green (migrate',
  '  to modal_runner!() where it simplifies them, but do not break live tests / the frozen manual reference).',
  '- The README snippets MUST equal real, compiling code. Add a DRIFT GUARD: e.g. the README quickstart block is the',
  '  quickstart crate\'s actual source, asserted by a test (read README.md, extract the tagged rust block, assert it matches',
  '  the crate file) OR the crate file is `include_str!`-embedded — pick a mechanism that makes a stale README a TEST',
  '  FAILURE. The point: "generate and test the README examples so they actually work."',
  '',
  '## Docs (Getting Started)',
  '- Rewrite the README Install + authoring sections to the clean form (single `modal-rust` dep, `use modal_rust::function`,',
  '  `modal_runner!()`, `app.add(2,3)`), matching the real quickstart crate. Fix the headline snippet so it compiles.',
  '- Add a real GETTING STARTED walkthrough (a new docs/getting-started.md or a top README section): prerequisites (Modal',
  '  account + token setup — `modal token`/env), then zero -> write a function -> `.local()` -> `.remote()` -> `deploy`+',
  '  `call`, each step runnable; a short Core Concepts (App / Function / the run-vs-deploy boundary); a Python->Rust cheat',
  '  sheet (@app.function vs #[function], .remote()/.map() parity); and a troubleshooting section. Keep it newcomer-depth',
  '  (architecture.html stays the maintainer doc).',
  '',
  '## FROZEN — additive + behavior-preserving',
  '- The runner CLI protocol, the 5 error kinds, the FILE-mode wire, `typed!`/Registry dispatch, the auto-I/O + decorator',
  '  semantics, and the App surface (local/local_with_registry/connect/connect_with_registry/deploy/call) — UNCHANGED. The',
  '  modal_runner!() macro + package auto-detect are ADDITIVE (a hand-written runner that still uses the old path keeps',
  '  working; MODAL_RUST_PACKAGE still overrides). Do not break the live_* tests (they must still compile under --features',
  '  live) or examples/add (the manual reference). Do not change the testkit or the SDK wire. Do NOT touch the user\'s',
  '  uncommitted docs/{testing-strategy.md, ergonomics-and-docs-review.md} except to READ them.',
  '',
  '## Verification (offline = HARD gate; NO Modal/Python)',
  '- default-members + testkit: cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test —',
  '  all green. The NEW clean example crate: `cargo build -p <quickstart>` + its `modal_runner --describe` lists the fn +',
  '  `--entrypoint add --input-json {"a":2,"b":3}` => `{"ok":true,"value":5}` + a .local() test. The README drift-guard test',
  '  passes. Prove the clean form needs NO `__private`/rename/env-var/hand-bin (grep the quickstart crate). live_* still',
  '  compile under --features live. Package auto-detect: prove (offline) the captured CARGO_PKG_NAME flows into the',
  '  RemoteConfig the run path uses (a unit/integration assertion or the dry-run/dump manifest showing the right package).',
  '',
  '## How to return',
  'End with "RESULT: <STATUS> — <one-line>". Build phases STATUS in {BUILD_GREEN, BUILD_FAILED} + exact cargo output + the',
  'quoted clean quickstart crate (Cargo.toml + lib.rs) + the README drift-guard proof.',
].join('\n')

phase('Design')
const spec = await agent(SHARED + '\n\n' + [
  'YOUR TASK (Design — VERIFY then plan). For EACH P0, confirm or refute against the code with file:line: the remote.rs',
  'package hardcode (quote it), the __private usage in the example runner bins + the lib.rs not-stable marker, the REASON',
  'the examples rename the facade (reproduce the collision or show it is unnecessary), and the AddCall import requirement.',
  'Then design, concretely: (a) the modal_runner!() macro (crate it lives in, exact expansion, how it routes through a',
  'PUBLIC entry so user code never names __private, and whether it can also capture env!("CARGO_PKG_NAME")); (b) the package',
  'auto-detect data flow (where CARGO_PKG_NAME is captured -> inventory/Registration -> RemoteConfig -> cargo build -p);',
  '(c) the import-collision root-cause fix so `use modal_rust::function` works with no rename; (d) AddCall auto-availability;',
  '(e) the real quickstart example-crate layout + which existing examples migrate; (f) the README drift-guard mechanism;',
  '(g) the Getting Started outline. Write it to ' + ROOT + '/workpads/shim-backend/ergonomics-hardening-spec.md. Flag risks',
  '(esp. anything that can\'t be done additively). RESULT: SPEC_DONE — verified the P0s; wrote ergonomics-hardening-spec.md',
].join('\n'), { phase: 'Design', label: 'design:ergonomics' })

phase('Ergonomics')
const ergo = await agent(SHARED + '\n\n' + [
  'The spec is at ' + ROOT + '/workpads/shim-backend/ergonomics-hardening-spec.md — READ IT FIRST.',
  '',
  'YOUR TASK (Ergonomics core — HARD GATE). Implement per the spec: the modal_runner!() macro (public, no __private in user',
  'code), package auto-detect (capture CARGO_PKG_NAME in the user crate -> config -> the run build), the import-collision',
  'fix (`use modal_rust::function` works with NO rename), and AddCall auto-availability. Keep it ADDITIVE + frozen',
  '(old hand-written runners + MODAL_RUST_PACKAGE override still work; live_* still compile; examples/add intact).',
  'VERIFY (offline HARD, paste exact output): cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ;',
  'cargo test. Stand up a TEMP clean crate (single modal-rust dep, a 3-line #[function] add, `modal_runner!()`, no rename,',
  'no env var, no hand bin) and prove: it builds, `modal_runner --describe` lists `add`, `--entrypoint add --input-json',
  '{"a":2,"b":3}` => `{"ok":true,"value":5}`, and the captured package name flows into the run RemoteConfig (show via the',
  'dry-run/dump manifest or a unit assertion). RESULT: BUILD_GREEN — modal_runner!()/auto-detect/no-rename/AddCall-prelude; clean crate works; gates green   (or BUILD_FAILED — <reason>)',
].join('\n'), { phase: 'Ergonomics', label: 'build:ergonomics' })

phase('Examples')
const examples = await agent(SHARED + '\n\n' + [
  'The spec + the ergonomics changes are in place (read ergonomics-hardening-spec.md + the new macro/config code).',
  '',
  'YOUR TASK (Real example crates — HARD GATE). Make the README examples REAL: create the clean minimal quickstart',
  'crate(s) that read exactly as a user would write them (single modal-rust dep, few-line #[function], modal_runner!(), NO',
  'boilerplate/__private/rename/env-var), as workspace member(s), and BUILD + TEST them offline (--describe + .local()).',
  'Migrate examples/add-macro to this clean form (drop the runner-bin file + the facade rename). Keep examples/add (manual',
  'reference) + the GPU examples green (migrate to modal_runner!() only where it simplifies without breaking live tests).',
  'VERIFY (offline HARD, paste output): the four gates green ; `cargo build -p <quickstart>` + its --describe + entrypoint',
  'run + a .local() test pass ; grep the quickstart crate to PROVE no `__private` / no `extern crate ... as modal_rust` /',
  'no MODAL_RUST_PACKAGE / no hand-written modal_runner bin. QUOTE the full quickstart Cargo.toml + lib.rs.',
  'RESULT: BUILD_GREEN — real clean quickstart crate(s) compiled+tested; add-macro migrated; gates green   (or BUILD_FAILED — <reason>)',
].join('\n'), { phase: 'Examples', label: 'build:examples' })

phase('Docs')
const docs = await agent(SHARED + '\n\n' + [
  'The ergonomics changes + the real quickstart crate(s) are in place (read them — the README must MATCH them).',
  '',
  'YOUR TASK (Docs — HARD GATE). Rewrite README.md Install + authoring to the clean form that matches the real quickstart',
  'crate (single modal-rust dep, `use modal_rust::function`, `modal_runner!()`, `app.add(2,3).local()/.remote()`), and fix',
  'the headline snippet so it compiles as shown. Add a GETTING STARTED walkthrough (new docs/getting-started.md + a README',
  'link/section): Modal token/account prerequisite, then zero -> #[function] -> .local() -> .remote() -> deploy+call (each',
  'runnable), a short Core Concepts, a Python->Rust cheat sheet, and troubleshooting. ADD THE DRIFT GUARD: a test that the',
  'README quickstart block equals the real quickstart crate source (stale README => test failure). Keep architecture.html',
  'as the maintainer doc. Do not touch docs/{testing-strategy,ergonomics-and-docs-review}.md.',
  'VERIFY (offline HARD, paste output): the four gates green INCLUDING the new drift-guard test ; confirm every README',
  'authoring snippet matches a real compiling crate (no rename/__private/env-var in the shown user code). QUOTE the new',
  'README authoring snippet + the drift-guard test. RESULT: BUILD_GREEN — README+getting-started match the real crates; drift-guard green   (or BUILD_FAILED — <reason>)',
].join('\n'), { phase: 'Docs', label: 'build:docs' })

const allGreen = [ergo, examples, docs].every(r => /RESULT:\s*BUILD_GREEN/i.test(r || ''))
if (!allGreen) log('A build phase HARD GATE was not green — Review documents the blocker.')

phase('Review')
const reviews = await parallel([
  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / newcomer + ergonomics). Put yourself in a newcomer\'s shoes: following ONLY the README + Getting',
    'Started, can they add the dep, write the function, run `.local()`, `.remote()`, and deploy? Confirm the shown user code',
    'is CLEAN — grep the quickstart crate + the README snippets for `__private`, `extern crate ... as modal_rust`,',
    '`MODAL_RUST_PACKAGE`, and a hand-written runner bin: NONE should appear in user-facing code. Confirm `modal_runner!()`',
    'exists + works, `use modal_rust::function` needs no rename, `app.add(2,3)` needs no manual trait import, and `.remote()`',
    'auto-detects the package (show the captured CARGO_PKG_NAME in the dump manifest). Confirm the README drift-guard makes',
    'a stale snippet fail. RESULT: PASS — newcomer-reproducible, ergonomics clean, examples real+tested  (or FAIL — <what is wrong>)',
  ].join('\n'), { phase: 'Review', label: 'review:newcomer' }),

  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / gates + frozen — RUN them). From ' + ROOT + ': cargo fmt --check ; cargo clippy --all-targets --',
    '-D warnings ; cargo build ; cargo test (incl. the drift-guard + the quickstart tests) — report exact output + exit',
    'status; all green. Confirm FROZEN: the runner protocol / 5 error kinds / FILE-mode wire / auto-I/O + decorator',
    'semantics / App surface are unchanged; the modal_runner!()/auto-detect are additive (a hand-written runner still works,',
    'MODAL_RUST_PACKAGE still overrides); examples/add (manual reference) intact; live_* still compile under --features live;',
    'the testkit + SDK wire untouched. Report any failure verbatim. RESULT: PASS — gates green, additive + frozen  (or FAIL — <exact failing cmd+output>)',
  ].join('\n'), { phase: 'Review', label: 'review:gates-frozen' }),
])

return { ergo_green: /RESULT:\s*BUILD_GREEN/i.test(ergo||''), examples_green: /RESULT:\s*BUILD_GREEN/i.test(examples||''), docs_green: /RESULT:\s*BUILD_GREEN/i.test(docs||''), reviews }
