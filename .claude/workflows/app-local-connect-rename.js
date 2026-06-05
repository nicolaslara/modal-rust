export const meta = {
  name: 'app-local-connect-rename',
  description: 'Rename the App constructor surface to a clean intent-named 2x2: App::from_inventory() -> App::local() and App::new(registry) -> App::local_with_registry(registry); App::connect(name) / App::connect_with_registry(name, registry) stay. Kills the leaky `inventory` mechanism name from the PUBLIC API (the macro `discover`/inventory step becomes an internal detail). Clean rename, NO deprecated aliases (the crate is unpublished). MUST update the README + every example + all tests. Pure rename — zero behavior change.',
  phases: [
    { title: 'Rename', detail: 'rename the two facade App constructors + all call sites + docs + README + examples + tests; gates green + grep-zero leftovers (HARD GATE)' },
    { title: 'Review', detail: 'independent grep-zero + gates + read the README/examples to confirm they are updated and read clean' },
  ],
}

const ROOT = '/Users/nicolas/devel/modal-rust'

const SHARED = [
  'Repo root: ' + ROOT + ' (git on main). This is a PURE RENAME of the facade `App` constructor surface — agreed with the',
  'user. Behavior is UNCHANGED; only names + docs change.',
  '',
  '## The rename (facade App public API only)',
  '- `App::from_inventory()`  ->  `App::local()`            (offline app over the `#[modal_rust::function]`-decorated fns)',
  '- `App::new(<registry>)`   ->  `App::local_with_registry(<registry>)`   (offline app from an explicit `Registry`)',
  '- UNCHANGED: `App::connect(name)`, `App::connect_with_registry(name, registry)`, the testkit-gated `connect_at*`',
  '  constructors, `App::deploy`/`deploy_with`, `App::call`, and EVERYTHING else.',
  '',
  '## Why (so the docs read right)',
  'The old `from_inventory()` leaked an implementation detail — `inventory` is the third-party crate the macro uses to',
  'collect decorated functions at compile time. The new surface names the INTENT / where-it-runs: `App::local()` (in-process,',
  'no Modal) vs `App::connect(name)` (talks to Modal). The collection step is an internal detail; user-facing names/docs must',
  'NOT say "inventory" or "from_inventory". (The lower-level runtime helper `modal_rust_runtime::from_inventory_with_configs()`',
  'is INTERNAL plumbing — KEEP its name; only the FACADE App public methods + their user-facing docs change.)',
  '',
  '## CRITICAL precision',
  '- Rename ONLY the facade `App::new` and `App::from_inventory`. Do NOT touch other `::new`s — `Function::new`,',
  '  `TypedCall::new`, `Registry::new`, `RemoteConfig::*`, etc. are unrelated. Distinguish by receiver type (`App::new(`',
  '  and bare `App::new(` constructions of the facade App) — verify each hit is the facade App.',
  '- Update doc-comments, `//!` module docs, and `///` rustdoc that mention `from_inventory`/`App::new`/"inventory-collected"',
  '  to the new names + framing. Internal CODE comments may still explain the inventory mechanism, but the public method',
  '  names + their doc summaries must read as `local()` / `local_with_registry()`.',
  '',
  '## Where to look (grep the whole tree; these are the known consumers)',
  '- crates/modal-rust/src/app.rs (the two method definitions + their docs + any internal callers + the in-file #[cfg(test)]',
  '  tests `from_inventory_captures_*`).',
  '- crates/modal-rust/src/lib.rs + crates/modal-rust/src/*.rs (any re-export / doc / internal use).',
  '- README.md — THE EMPHASIS (user said "very important"): the macro tutorial (`App::from_inventory()` in the `app.add(2,3)`',
  '  examples), the manual section (`App::new(modal_registry())`), the Examples table, the quickstart snippet, and the',
  '  "alternative ways to get an App" framing if present. Make the offline examples use `App::local()` and the manual',
  '  offline ones `App::local_with_registry(...)`; the remote ones already use `App::connect`.',
  '- examples/orchestrate/src/main.rs (`App::new(modal_registry())`, `App::from_inventory()`, and its #[cfg(test)] tests).',
  '- examples/add , examples/add-macro (mostly lib crates — but update any doc-comment that names from_inventory/App::new).',
  '- crates/modal-rust/tests/*.rs and crates/modal-rust-sdk/tests/*.rs (mock_remote.rs, mock_table.rs, local.rs, the live_*',
  '  tests — any `App::from_inventory()`/`App::new(`). The mock tests mostly use `connect_at*`; update only real hits.',
  '- workpads/**.md are PLANNING docs — OUT OF SCOPE, do not edit.',
  '',
  '## FROZEN',
  'Pure rename: no signature changes beyond the name, no behavior change, runner protocol / wire / error kinds / decorator',
  'semantics untouched. Do NOT add deprecated aliases (unpublished crate — clean break). Do NOT touch the runtime helper',
  'name, the SDK, the macro crate logic, or the user\'s uncommitted docs/testing-strategy.md.',
  '',
  '## Verification (offline = HARD gate)',
  '- default-members + testkit: cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test —',
  '  all green. ALSO run the testkit/facade test targets that use the mock (cargo test -p modal-rust -p modal-rust-sdk).',
  '- GREP-ZERO (paste the commands + empty output): `grep -rn "from_inventory(" crates/modal-rust examples README.md` returns',
  '  NOTHING (the runtime `from_inventory_with_configs` is a different identifier and lives in modal-rust-runtime — exclude',
  '  it; if it appears via the facade calling it internally that is fine, but no `App::from_inventory(` anywhere). And',
  '  `grep -rnE "App::new\\(" crates examples README.md` returns NOTHING. Confirm `App::local(`, `App::local_with_registry(`,',
  '  `App::connect(` are the surface.',
  '- QUOTE the updated README macro-tutorial snippet + the orchestrate App construction lines so the new names are visible.',
  '',
  '## How to return',
  'End with: "RESULT: <STATUS> — <one-line>". STATUS in {BUILD_GREEN, BUILD_FAILED} + exact cargo output + the grep-zero',
  'proof + the quoted README/orchestrate snippets.',
].join('\n')

phase('Rename')
const impl = await agent(SHARED + '\n\n' + [
  'YOUR TASK (do the rename — HARD GATE). Grep the whole tree for `App::from_inventory(` and facade `App::new(` (verify each',
  'is the facade App, not Function/Registry/TypedCall::new). Rename per the mapping: from_inventory -> local,',
  'App::new(registry) -> App::local_with_registry(registry). Update the method DEFINITIONS in crates/modal-rust/src/app.rs',
  '(+ their rustdoc summaries to the local()/local_with_registry() framing, no "inventory" in the public name/summary), ALL',
  'call sites, the in-file tests, the README (macro tutorial + manual section + Examples table + quickstart), examples/',
  'orchestrate, and the mock/other tests. Keep connect/connect_with_registry/connect_at*/deploy/call unchanged. Keep the',
  'runtime `from_inventory_with_configs` helper name. No deprecated aliases.',
  'VERIFY (offline HARD, paste exact output): cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ;',
  'cargo test (default-members) ; cargo test -p modal-rust -p modal-rust-sdk — all green. Then the GREP-ZERO proof:',
  '`grep -rn "App::from_inventory(" crates examples README.md` and `grep -rnE "App::new\\(" crates examples README.md` both',
  'EMPTY. QUOTE the updated README macro snippet + the orchestrate App lines.',
  'RESULT: BUILD_GREEN — App surface is local()/local_with_registry()/connect()/connect_with_registry(); gates green; zero from_inventory/App::new refs   (or BUILD_FAILED — <reason>)',
].join('\n'), { phase: 'Rename', label: 'rename:app-surface' })

const implGreen = /RESULT:\s*BUILD_GREEN/i.test(impl || '')
if (!implGreen) log('Rename HARD GATE not green — see Review for the blocker.')

phase('Review')
const reviews = await parallel([
  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / completeness + docs). Independently verify against the tree: `grep -rn "App::from_inventory("',
    'crates examples README.md` and `grep -rnE "App::new\\(" crates examples README.md` are BOTH empty (paste them). Confirm',
    'the facade App now exposes local()/local_with_registry()/connect()/connect_with_registry() (quote the four signatures',
    'from app.rs). READ the README macro tutorial + Examples table + the manual section and confirm they use App::local() /',
    'App::local_with_registry() / App::connect() correctly and read clean (no leftover "from_inventory"/"inventory-collected"',
    'in user-facing prose). Confirm examples/orchestrate constructs the App with the new names. Confirm behavior is unchanged',
    '(pure rename; no signature/semantic drift; connect/deploy/call untouched). RESULT: PASS — rename complete, docs+examples updated  (or FAIL — <what is wrong>)',
  ].join('\n'), { phase: 'Review', label: 'review:rename-complete' }),

  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / gates — RUN them). From ' + ROOT + ' report exact output + exit status: cargo fmt --check ; cargo',
    'clippy --all-targets -- -D warnings ; cargo build ; cargo test (default-members) ; cargo test -p modal-rust',
    '-p modal-rust-sdk (the mock tests). All must be green. Confirm the testkit still builds + its mock tests pass after the',
    'rename. Report any failure verbatim. RESULT: PASS — gates green incl. mock tests  (or FAIL — <exact failing cmd+output>)',
  ].join('\n'), { phase: 'Review', label: 'review:gates' }),
])

return { impl_green: implGreen, impl, reviews }
