export const meta = {
  name: 'macro-hygiene',
  description: 'Eliminate the proc-macro dependency wart: re-export modal_rust_runtime + inventory from the modal-rust facade under __private, route ALL macro-emitted paths through ::<facade>::__private::… (via proc-macro-crate so a renamed `modal-rust` still resolves), so a crate using #[modal_rust::function] needs ONLY `modal-rust`. Drop the direct modal-rust-runtime/inventory deps from macro-using examples + the README "Dependency note". MUST run AFTER macro-auto-io is committed (both edit the macro).',
  phases: [
    { title: 'Design', detail: 'read the POST-auto-io macro + facade; spec the __private re-export + proc-macro-crate path routing + inventory::submit! feasibility' },
    { title: 'Implement', detail: 'facade __private re-exports + macro emits facade-routed paths + drop example deps + README dep-note; gates green + a macro example builds with ONLY modal-rust (HARD GATE)' },
    { title: 'Review', detail: 'parallel: wart eliminated (macro user needs only modal-rust; rename-safe; runner protocol frozen) + hygiene' },
  ],
}

const ROOT = '/Users/nicolas/devel/modal-rust'

const SHARED = [
  'You are removing the proc-macro DEPENDENCY WART in modal-rust (repo root: ' + ROOT + '; git on main).',
  '',
  '## The problem',
  '`#[modal_rust::function(...)]` (crate modal-rust-macros) expands to BARE absolute paths — `::modal_rust_runtime::…`',
  '(Registration, FunctionConfig, typed!, HandlerFn, …) and `::inventory::submit!` — which only resolve in the USER',
  'crate\'s extern prelude. So a crate using the macro must add `modal-rust-runtime` AND `inventory` as DIRECT deps',
  'alongside `modal-rust` (the README has a "Dependency note" about exactly this; examples/add-macro carries both deps).',
  'The standard fix (serde/clap/pyo3/tracing pattern): route those paths THROUGH the facade crate so the user needs ONLY',
  '`modal-rust`.',
  '',
  '## The fix to implement',
  '1. **Re-export the macro deps from the `modal-rust` facade** under a hidden module, e.g. in crates/modal-rust/src/',
  '   lib.rs: `#[doc(hidden)] pub mod __private { pub use ::inventory; pub use ::modal_rust_runtime as runtime; }`',
  '   (+ re-export `typed!` if the macro emits it: `pub use ::modal_rust_runtime::typed;`). Keep the existing public',
  '   re-exports too.',
  '2. **Make the macro emit facade-routed paths**: every `::modal_rust_runtime::X` -> `::<facade>::__private::runtime::X`',
  '   and `::inventory::submit!{…}` -> `::<facade>::__private::inventory::submit!{…}`. COVER EVERY EMIT SITE — including',
  '   the NEW auto-I/O codegen + the typed `App` extension-method codegen added by the just-landed macro-auto-io work',
  '   (read the CURRENT macro source; do not assume the old emit set).',
  '3. **Resolve the facade crate name with `proc-macro-crate`** (add it as a build/normal dep of modal-rust-macros):',
  '   look up the import name of `modal-rust` in the user\'s Cargo.toml at expansion time (default ident `modal_rust`,',
  '   but honor a rename / the `crate` self-reference case for in-workspace use). Emit the resolved path.',
  '4. **Drop the now-unnecessary direct deps** (`modal-rust-runtime`, `inventory`) from macro-using example Cargo.tomls',
  '   (examples/add-macro + any auto-io example), so they depend on ONLY `modal-rust` (+ serde/anyhow). This is the PROOF',
  '   the wart is gone: those examples must still compile + their macro expansion must still work.',
  '5. **Update the README**: remove the "Dependency note" block and collapse the macro-path Install snippet to the same',
  '   minimal `modal-rust`-only set as the manual path (by the time this runs, the README formatter + any auto-io README',
  '   update are already committed, so no contention).',
  '',
  '## Feasibility to verify (do NOT hand-wave)',
  '- `inventory::submit!` invoked THROUGH a re-export (`::modal_rust::__private::inventory::submit!`): confirm it works',
  '  (edition 2018+ macro-path resolution + the re-exported `Registration` type). It SHOULD. If it genuinely does not,',
  '  FALLBACK: still route `modal_rust_runtime` through the facade (kills the bigger half of the wart) and keep',
  '  `inventory` direct, documenting precisely why — but try the full fix first.',
  '- `typed!` (a macro_rules in the runtime) through the re-export likewise.',
  '- proc-macro-crate `FoundCrate::Itself` (when the macro is used INSIDE the modal-rust workspace, e.g. examples) vs',
  '  `FoundCrate::Name(name)` — handle both so workspace examples AND external crates both resolve.',
  '',
  '## FROZEN invariants — do NOT change',
  '- The runner CLI protocol / HandlerFn / typed! BEHAVIOR / Registry::from_inventory dispatch / the 5 error kinds / the',
  '  FILE-mode wire format — UNCHANGED. This is a pure PATH-ROUTING + re-export change; the generated code is semantically',
  '  identical, only the paths it names change. The bare `#[modal_rust::function]`, all decorator args (gpu/timeout/cache/',
  '  secrets/volumes), the auto-I/O + typed-method codegen, and `macro_path_byte_identical_to_manual` (the macro==manual',
  '  equivalence test) must all still hold. Do NOT touch the SDK/runtime invoke logic or the facade beyond adding __private.',
  '',
  '## Ground-truth references (READ — the CURRENT, post-auto-io sources)',
  '- crates/modal-rust-macros/src/lib.rs (EVERY `::modal_rust_runtime` / `::inventory` emit site, incl. auto-I/O + the',
  '  generated App methods) + crates/modal-rust-macros/Cargo.toml.',
  '- crates/modal-rust/src/lib.rs (where to add `__private`) + crates/modal-rust-runtime/src/lib.rs (the items re-exported).',
  '- examples/add-macro/{Cargo.toml,src/lib.rs,src/bin/modal_runner.rs} (+ any auto-io example): drop the direct deps.',
  '- README.md (the "Dependency note" + the macro-path Install block) — read before editing. references/modal-client',
  '  is irrelevant here; this is a Rust-macro-hygiene fix (model it on serde_derive / clap_derive path routing).',
  '',
  '## Verification rules (WORKING.md)',
  '- Gates on default-members: cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test',
  '  — all green. NO live Modal needed (compile-time change). The KEY proof: a macro-using example compiles + its tests',
  '  pass with ONLY `modal-rust` as the modal dep (no modal-rust-runtime / inventory in its Cargo.toml).',
  '',
  '## How to return',
  'End with: "RESULT: <STATUS> — <one-line>". Build phases STATUS in {BUILD_GREEN, BUILD_FAILED} + exact cargo output +',
  'confirmation the example Cargo.toml no longer lists modal-rust-runtime/inventory and still builds + macro-expands.',
].join('\n')

phase('Design')
const spec = await agent(SHARED + '\n\n' + [
  'YOUR TASK (Design the hygiene fix). Read the CURRENT crates/modal-rust-macros/src/lib.rs (enumerate EVERY',
  '`::modal_rust_runtime::…` and `::inventory::…` emit site — there are MORE now after macro-auto-io: the auto-generated',
  'input struct, the spread-call wrapper, the typed App extension methods), crates/modal-rust/src/lib.rs, crates/',
  'modal-rust-runtime/src/lib.rs, examples/add-macro/{Cargo.toml,src/*}, and README.md (the Dependency note + Install).',
  'WRITE a build-ready spec to ' + ROOT + '/workpads/shim-backend/macro-hygiene-spec.md covering: the facade `__private`',
  're-export module (exact items: inventory, runtime, typed!); the proc-macro-crate resolution (dep to add; FoundCrate::',
  'Itself vs Name handling for workspace examples vs external crates); the COMPLETE list of macro emit sites to reroute',
  '(quote each, with its new facade-routed form); the inventory::submit!/typed!-through-re-export feasibility check + the',
  'documented fallback if submit! cannot route; the example Cargo.toml dep removals; and the README edits (remove the',
  'Dependency note; collapse the macro Install block). Cite file:line. RESULT: SPEC_DONE — wrote macro-hygiene-spec.md',
].join('\n'), { phase: 'Design', label: 'design:hygiene' })

phase('Implement')
const impl = await agent(SHARED + '\n\n' + [
  'The spec is at ' + ROOT + '/workpads/shim-backend/macro-hygiene-spec.md — READ IT FIRST.',
  '',
  'YOUR TASK (Implement the hygiene fix — HARD GATE). Per the spec: add the facade `#[doc(hidden)] pub mod __private`',
  're-exporting inventory + modal_rust_runtime (+ typed!); add `proc-macro-crate` to modal-rust-macros and resolve the',
  'facade name; reroute EVERY macro-emitted `::modal_rust_runtime::…` / `::inventory::…` path through',
  '`::<facade>::__private::…` (cover ALL emit sites incl. the auto-I/O + typed-method codegen); drop the direct',
  '`modal-rust-runtime` + `inventory` deps from macro-using example Cargo.toml(s); and update README.md (remove the',
  'Dependency note + collapse the macro Install snippet to modal-rust-only). If inventory::submit! genuinely cannot route',
  'through the re-export, apply the documented fallback (keep inventory direct, still route runtime) and note it.',
  'Do NOT change the runner protocol / typed! behavior / invoke logic / decorator semantics — paths only.',
  'VERIFY (offline hard): cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test',
  '(all default-members) — all green, INCLUDING macro_path_byte_identical_to_manual. Then PROVE the wart is gone: confirm',
  'the macro-using example Cargo.toml lists neither modal-rust-runtime nor inventory, and `cargo build -p <that example>`',
  '+ its tests still pass + the macro expands (run its modal_runner --describe offline). Paste exact output.',
  'RESULT: BUILD_GREEN — macro routes via facade __private; macro example needs only modal-rust; gates green   (or BUILD_FAILED — <reason>)',
].join('\n'), { phase: 'Implement', label: 'hygiene-impl' })

const implGreen = /RESULT:\s*BUILD_GREEN/i.test(impl || '')
if (!implGreen) log('Implement HARD GATE not green — hygiene fix did not compile / broke the macro. Review documents the blocker.')

phase('Review')
const reviews = await parallel([
  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / wart eliminated + frozen). Verify against the code:',
    '- The facade exposes `__private` (inventory + runtime + typed!). EVERY macro emit site routes through',
    '  `::<facade>::__private::…` (no bare `::modal_rust_runtime::` / `::inventory::` left in the macro output — grep the',
    '  macro source). proc-macro-crate handles both the in-workspace (FoundCrate::Itself) and external (Name) cases.',
    '- THE PROOF: the macro-using example Cargo.toml lists NEITHER modal-rust-runtime NOR inventory (only modal-rust +',
    '  serde/anyhow) and still builds + expands. Quote the trimmed Cargo.toml.',
    '- Runner protocol / typed! behavior / dispatch / decorator semantics UNCHANGED (paths only); the macro==manual',
    '  equivalence test passes. README "Dependency note" is gone + the Install macro block is minimal.',
    '- If the inventory-direct fallback was used, it is documented + justified.',
    'RESULT: PASS — wart eliminated, semantics frozen  (or FAIL — <what is wrong>)',
  ].join('\n'), { phase: 'Review', label: 'review:wart-gone' }),

  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / hygiene — RUN the gates). From ' + ROOT + ' report exact output + exit status:',
    '- cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test  (default-members).',
    'Confirm example-burn-add still excluded from default-members; examples/add (manual form) untouched + passing; the',
    'new proc-macro-crate dep is minimal; no hand-written file grossly exceeds ~500 LOC. Report failures verbatim.',
    'RESULT: PASS — gates green  (or FAIL — <exact failing command + output>)',
  ].join('\n'), { phase: 'Review', label: 'review:hygiene' }),
])

return { impl_green: implGreen, impl, reviews }
