# modal-rust — Architecture Review

An honest, evidence-backed assessment of the `modal-rust` codebase as of 2026-06-05.
Every file:line and LOC claim below was verified by reading the source (`wc -l`,
direct reads). This is a documentation-only review — no source was changed.

Scope reviewed: all five crates (`modal-rust-runtime`, `modal-rust-macros`,
`modal-rust-sdk`, `modal-rust`, `modal-rust-cli`), the five examples, and the test
suite. Production (non-test) source is ~11k LOC across `crates/*/src` (10,990 total
incl. inline `#[cfg(test)]` modules), 1,619 LOC of live integration tests, and 1,251
LOC of examples.

---

## Overall verdict

This is a **strong, unusually disciplined codebase** for its size and ambition. It
implements a genuinely hard thing — a first-party Rust gRPC client to Modal plus a
run-vs-deploy build boundary that compiles user Rust either in a function body or at
image-build time — and it does so with a clean layered architecture, a deliberately
frozen core, and exceptional documentation density. The two hard invariants (the
runner CLI protocol and the run/deploy build boundary) are genuinely well-protected:
they live in the right crate, are unit-tested, and are re-asserted at every layer.

The milestone-by-milestone adversarial process shows clearly in two opposite ways:

- **The good:** every non-obvious decision carries a dated, file-cited rationale in a
  comment; "wire-identical to before" is asserted at each additive seam; the frozen
  protocol has not drifted; transient-vs-terminal error handling is principled.
- **The accreted cost:** the run-path Python wrapper has grown into a ~165-line
  embedded Python string with its own cache subsystem; `RemoteConfig` and
  `DeployConfig` still share source/build defaults; and new decorator knobs still
  require touching the macro plus the SDK application point. The former
  downstream-dependency leak and five-shape additive-config copy chain are now fixed.

None of these are architectural faults — they are the visible sediment of an
additive-only, freeze-the-seam discipline. The prioritized refactors at the end are
all non-breaking cleanups, not redesigns.

Headline numbers (verified):

| File | LOC | of which tests | Notes |
| --- | --- | --- | --- |
| `runtime/src/lib.rs` | 1113 | ~294 (from L819) | The frozen core; ~819 real |
| `modal-rust/src/remote.rs` | 980 | ~277 (from L704) | Embedded Python wrapper lives here |
| `sdk/src/ops/image.rs` | 935 | ~334 (from L602) | Dockerfile rendering |
| `sdk/src/ops/local_dir.rs` | 837 | ~256 (from L581) | Mount upload |
| `sdk/src/ops/invoke.rs` | 693 | ~66 (from L628) | Map/spawn/get |
| `sdk/src/ops/function.rs` | 620 | ~184 (from L437) | FunctionCreate |
| `modal-rust/src/app.rs` | 612 | ~114 (from L498) | App handle |
| `modal-rust/src/deploy.rs` | 582 | ~134 (from L449) | Deploy path |
| `modal-rust/src/scope.rs` | 522 | ~199 (from L324) | cargo-metadata scoping |

A useful caveat: every "big" file is roughly half tests. `runtime/lib.rs` is 1113
lines but only 819 before its `#[cfg(test)]` module (L819); `image.rs` is 601 real
lines; `app.rs` 497. The raw LOC overstates the size of the units a reader must hold
in their head.

---

## 1. Code readability

### Good

- **Naming is consistent and intention-revealing across crate boundaries.** The same
  vocabulary (`ensure_function`, `parse_envelope`, `mount_workspace_closure`,
  `from_inventory_with_configs`, `resolve_function`) is used the same way everywhere.
  `HandlerFn`, `Registry`, `RunnerError`, `FunctionConfig`, `FunctionSpec`,
  `ImageSpec`, `RemoteConfig`/`DeployConfig` all say what they are.
- **Control flow in the core runner is linear and obvious.**
  `run_cli_dispatch` (`runtime/src/lib.rs:646`) reads top-to-bottom exactly as the
  frozen precedence reads: parse args → read input → JSON parse → entrypoint lookup →
  run handler → emit. The "frozen precedence" comment at L663 maps 1:1 to the code.
- **Idiomatic Rust throughout.** Builder methods consume `self` and return `Self`
  (`ImageSpec`, `FunctionSpec`); `Option`/`Result` are used over sentinels; iterator
  chains are readable (e.g. `closure_from_metadata` in `scope.rs:257`); the
  fallible-vs-infallible boundary is explicit (`with_gpu` validates at set time so
  `to_proto` is infallible — `function.rs:214` + `:68`).
- **The hand-rolled arg parser** (`parse_args`, `runtime/src/lib.rs:509`) is a simple
  index loop with clear per-flag duplicate checks — appropriate given the deliberate
  no-`clap`-in-runtime rule.

### Bad / weak

- **`remote.rs` mixes three abstraction levels in one file:** a 165-line Python
  program (the `WRAPPER_SRC` string literal, `remote.rs:60–225`), the 13-field
  `RemoteConfig` struct (`:266`), six env-discovery helpers (`discover_*`,
  `:344–423`), and the multi-step control-plane `ensure_function` (`:446`). A reader
  fixing the cache logic must scroll past Rust config plumbing to reach embedded
  Python. (See §6.)
- **Inline `std::collections::` and `std::env::var` full-path spellings** recur where a
  `use` would read better — e.g. `app.rs:215` `std::collections::HashMap::new()` twice
  on adjacent lines; `app.rs:18` even introduces a `MapInput` alias specifically to
  tame `clippy::type_complexity`, which is the right instinct but signals the
  underlying CBOR tuple type is awkward.
- **`resolve_function` (`app.rs:244`) does a lot:** it copies five decorator fields
  into a per-call `RemoteConfig` clone inside a `get_or_try_init` closure, then
  computes a deadline. It is correct and commented, but it is the densest method in
  the facade and the "config bound at first call" caveat (`:234`) is a real
  foot-gun the comment honestly flags.

### Could improve

- Pull the `WRAPPER_SRC` Python out of `remote.rs` into its own module or a
  `include_str!` of a real `.py` file (which would also get Python-syntax tooling).
- A few `let cfg_x = ...; let cfg_y = ...;` ladders in `resolve_function` could be a
  single small struct-update.

---

## 2. Abstractions

### Good — the Registry / HandlerFn static-dispatch choice

This is the standout design decision and it is **the right one**. `HandlerFn = fn(&[u8])
-> Result<Vec<u8>, RunnerError>` (`runtime/src/lib.rs:33`) erases every user function
to a bare monomorphized `fn` pointer — no `Box<dyn>`, no vtable — and `Registry =
BTreeMap<&'static str, HandlerFn>` (`:341`). The `typed!` macro (`:189`) generates a
per-handler `__wrap` that inlines decode/call/encode for the concrete `In`/`Out`/`Err`.
Both the manual builder (`Registry::new().function(...)`) and the macro/inventory path
(`Registry::from_inventory`, `:366`) converge on the *same* map shape. This honors the
"prefer static dispatch" stance precisely, keeps the recompiled-on-every-run core
minimal, and leaves the codec and async paths additive. The autoref/inherent-priority
specialization in `__macro_support` (`:221–254`) to pick `Serialize`-vs-opaque error
wrapping at compile time is genuinely elegant and well-explained.

The error model is also right: `RunnerError` (`:39`) *wraps* the user error structurally
(`details: Option<serde_json::Value>`) instead of stringifying early, and `encode_error`
is a distinct kind so an output-serialization bug can't masquerade as a `panic` (frozen
at five kinds, `:69`).

### Good — the ops layer

`sdk/src/ops/` is cleanly factored: each RPC family is its own submodule implemented as
`impl ModalClient` blocks (`ops/mod.rs:23–31`), so the public surface is just methods on
one client. The shared `result_status` / `ResultState` / `describe_failure` helpers
(`ops/mod.rs:50–89`) deduplicate the poll-terminal logic across image build, invoke,
and map. `ImageSpec`/`FunctionSpec` are declarative builders that render to proto in one
`to_proto()` — a clean separation of "describe" from "send."

### Leaky / questionable abstractions

- **`RemoteConfig` vs `DeployConfig` still share build/source defaults.** The
  per-function Modal options are no longer copied through these structs: the facade now
  converts static `FunctionConfig` into owned `FunctionOptions`, and `App`,
  `RemoteConfig`, `DeployConfig`, deploy plans, dry-run, and the CLI manifest all carry
  that one type. The remaining duplication is the build/source-path core
  (`local_root`, `package`, `use_cargo_scoping`, `modalignore_name`, `base_image`,
  `timeout_secs`, `install_rust`, plus deploy's `app_name` and run's `remote_src` /
  cache default). A future shared `BuildPathConfig` core could remove that smaller
  doc/field duplication, but the high-risk gpu/timeout/cache/secrets/volumes copy chain
  is gone.
- **The embedded-Python-wrapper-as-Rust-string is a deliberate but leaky abstraction.**
  `WRAPPER_SRC` (`remote.rs:60`) is a full Python program — including an entire cache
  pack/unpack subsystem (`_unpack_cache`, `_pack_cache`, `_pack_one`, zstd/gzip
  fallback) — living as a `&'static str` with `{{PACKAGE}}`/`{{CACHE}}`/`{{ARCHIVE_*}}`
  template holes filled by `run_wrapper_src` (`:237`). It is base64-baked into the
  Dockerfile (`image.rs:447`), so there is no shell-quoting risk, and there is a test
  asserting the placeholders are substituted and the archive path matches the Rust
  constants (`remote.rs:709`, `:724`). But: this Python has no type checking, no
  linting, no unit tests of its *own* logic (only that the string substitutes), and it
  has grown the cache state machine that arguably wants to be real code. The
  deploy-side twin (`DEPLOY_WRAPPER_SRC`, `deploy.rs:65`) is mercifully small and has a
  good negative-assertion test that it contains no `cargo`/`/src`/`CARGO_` (`:453`).
- **[FIXED 2026-06-06] The additive-config hand-threading was collapsed.** A
  decorator value now travels through one static boundary and one owned domain type:
  `#[function(gpu=..)]` → static facade `FunctionConfig` (`registration.rs`) →
  owned `FunctionOptions` → `FunctionSpec::with_gpu` / timeout / secret / volume
  application. The `--describe` manifest serializes `FunctionOptions` directly, and
  the CLI deserializes it directly, so the previous `DescribeConfig` /
  `FunctionConfigView` / `Box::leak` path is gone. Adding a new per-function knob still
  touches the macro and the SDK application point, but no longer requires five parallel
  struct declarations plus string/slice conversion sites.

### Under-abstracted

- The `discover_*` env-var helpers in `remote.rs` (`:344–423`) are six near-identical
  "read env, lowercase, match truthy" functions. `discover_install_rust`,
  `discover_cache`, `discover_cache_target` differ only in var name and default. A
  single `env_bool(name, default)` helper would collapse three of them.

---

## 3. Domain separation & separation of concerns

### Good

- **Crate boundaries are clean and acyclic, exactly as `boundaries.md §1` mandates.**
  `runtime` has zero Modal/network/Python deps (only serde/serde_json/anyhow +
  inventory — Cargo.toml confirms). `clap`/`tokio` live only in the CLI. The SDK has no
  facade dependency; the facade depends on SDK + runtime + macros; the CLI depends only
  on the facade (transitively pulling the SDK). The dev-dep cycle is broken correctly:
  `example-add` depends only on `runtime`, never on `modal-rust`, so `modal-rust`'s
  dev-dep on `example-add` is acyclic (commented at `modal-rust/Cargo.toml`).
- **The runner protocol lives entirely in `runtime`** and nothing above it can change
  the envelope shape — the facade only *parses* it (`parse_envelope`, `remote.rs:657`)
  and *reconstructs* `RunnerError` from JSON (`reconstruct_runner_error`, `:674`),
  mirroring `.local()` byte-for-byte. That mirroring is asserted by tests
  (`remote.rs:843–931`).
- **Run vs deploy is split by file, not by flag:** `remote.rs` (ephemeral, build in
  body) and `deploy.rs` (persistent, build at image time) are separate modules with
  separate wrapper constants and separate publish semantics. The deploy module's doc
  header (`deploy.rs:1–22`) states the invariant and the code enforces it (client mount
  only, no source mount — `:341`).

### Leaks / mislocations

- **The run-path Python wrapper logic lives in the facade (`remote.rs`), not the SDK.**
  This is arguably the most defensible "leak": the wrapper is the *contract* between
  the facade's build-boundary intent and the container, so co-locating it with
  `ensure_function` is reasonable. But it means the facade crate owns ~165 lines of
  Python and a cache file-format, while the SDK (which owns *all other* container/image
  concerns) does not. A reader looking for "how does caching work" must look in the
  facade, while "how is the image built" is in the SDK. The seam is slightly in the
  wrong place — the wrapper text is image/container infrastructure.
- **`scope.rs` (cargo-metadata scoping + workspace-manifest rewrite) lives in the
  facade**, but it shells out to `cargo metadata` and does TOML rewriting — pure
  build-tooling concerns with no facade-state dependency. It is cleanly separated as
  its own module and is pure/testable (good), but it could equally live in the SDK or a
  small `build-scope` crate. Minor.
- **`DEFAULT_DEPLOY_APP` is defined in two places with different values:**
  `deploy.rs:46` (`"modal-rust-add-deploy"`) and `cli/src/main.rs:28`
  (`"modal-rust-add-poc"`). They are independent defaults for different layers, but the
  same constant name holding different strings is a readability trap.

---

## 4. APIs & entrypoints

### Good

- **The facade public surface is small and coherent.** `lib.rs` re-exports exactly
  `App`, `Function`/`FunctionCall`, `DeployConfig`/`DeployedApp`, `RemoteConfig`,
  `Error`/`Result`, plus the runtime essentials and `sdk` namespace
  (`modal-rust/src/lib.rs:51–79`). The lifecycle reads naturally:
  `App::new(registry)` / `App::from_inventory()` for offline `.local()`,
  `App::connect(name)` for remote, then `app.function("add").local(..)/.remote(..)
  /.spawn(..)/.map(..)`. This deliberately mirrors Modal Python (`Function.local()`,
  `.remote()`, `.spawn()`, `.map()`), which is the right north star.
- **The SDK surface is method-calls-on-one-client**, discoverable from the
  `ModalClient` impl blocks and documented end-to-end in `sdk/src/lib.rs:18–28`. The
  `inner_mut()` escape hatch (`client.rs:157`) is an honest pressure valve.
- **The CLI is a thin, coherent four-verb surface** (`doctor`/`run`/`deploy`/`call`,
  `cli/src/main.rs:42`) that drives the *same* facade methods, with no second control
  path — `programmatic.rs` builds the user crate, runs `--describe`, and calls
  `App::connect_from_manifest` + `remote_envelope`/`deploy_with`/`call_envelope`.
- **Error UX is principled.** `Error` (`modal-rust/src/error.rs:14`) wraps
  `RunnerError` verbatim and adds the facade-only modes (`UnknownEntrypoint` with the
  known-names list, the two distinct serde boundaries `Encode`/`Decode`, `Sdk`,
  `NotConnected`, `Config`). The deliberate *absence* of a blanket
  `From<serde_json::Error>` (`error.rs:121`) — because the same serde type covers both
  the encode and decode boundary and they must map to distinct variants — is exactly
  the kind of subtle correctness call that's easy to get wrong and here is documented.

### Bad — the macro-hygiene wart

This is real and the code is admirably honest about it. `#[modal_rust::function]` is
re-exported from the facade so it's spellable without the `extern crate ... as
modal_rust` alias, **but its expansion emits absolute `::modal_rust_runtime::...` and
`::inventory::submit!` paths**, which resolve against the *downstream* crate's extern
prelude. So any crate using the macro must add **three** direct deps:
`modal-rust`, `modal-rust-runtime`, and `inventory` (documented frankly at
`modal-rust/src/lib.rs:31–48`, and `examples/add-macro/Cargo.toml` proves it). For a
"single-dep facade," needing three deps to use the headline ergonomic feature is a
genuine surface wart. The fix (re-export `inventory` and runtime paths through the
facade, or have the macro emit `$crate`-relative paths via a `modal_rust::__rt` shim)
is non-trivial because it would change the frozen macro expansion and break
`examples/add-macro` — which is exactly why it was left. Worth fixing eventually behind
a new example.

### Could improve

- **`App` has a large method count with several near-duplicate pairs:** `call` vs
  `call_envelope`, `remote_invoke` vs `remote_envelope`, `connect` vs
  `connect_with_registry` vs `connect_from_manifest` vs `from_manifest`. Each exists for
  a real reason (typed vs raw-envelope for the generic CLI; explicit vs default config),
  and they're documented, but the constructor matrix (4 connect-ish entry points) is
  more than a newcomer can hold. A doc table in the `App` rustdoc mapping
  "which constructor for which situation" would help.

---

## 5. Code comments

### Good — this is a genuine strength

- **Density is high and the content is load-bearing, not noise.** Comment lines:
  `runtime/lib.rs` 311, `image.rs` 318, `deploy.rs` 217, `app.rs` 216, `remote.rs` 267.
  Crucially, the comments explain *why*, often with a dated live-observation and a
  proto/file citation: e.g. the `mount_client_dependencies` / builder-version coupling
  ("Sending an empty builder version ... was TERMINATED at boot (live-observed
  2026-06-04)", `client.rs:98`); the ephemeral-vs-deployed publish bug and its symptom
  (`remote.rs:432–445`); the `--break-system-packages` rationale (`image.rs:324`).
- **The frozen seams are explicitly labeled** at every layer — "FROZEN", "wire-identical
  to before", "byte-identical default render" recur with the specific invariant they
  protect. The runtime doc header (`runtime/lib.rs:1–20`) enumerates exactly what the
  crate provides and why it stays minimal.
- **The `typed!` specialization trick is fully explained** (`runtime/lib.rs:200–211`
  and `:221–254`) — the kind of thing that is otherwise inscrutable.
- **TODO/FIXME/HACK debt is essentially nil** — three matches total, and they are
  documentation phrasing ("alias hack" in a doc sentence, one genuine
  `TODO(fallback)` note in `local_dir.rs:207`), not rotting markers.

### Bad / risk

- **Comment-to-code ratio is *so* high in places it risks staleness drift.** `remote.rs`
  is 27% comments; `function.rs` (facade) is 52% comments (105 of 202 lines). When the
  rationale is this dense, a future edit that changes behavior but not the adjacent
  paragraph creates a misleading comment. The "config bound at first RUN-path call"
  caveat (`app.rs:234`) and the "double-enqueue caveat" (`invoke.rs:159`, `:389`) are
  examples of comments encoding *current* limitations that must be kept in sync.
- A handful of comments restate the obvious line below them (e.g. `app.rs:215`
  "two positional args ... no kwargs" immediately above the empty-kwargs map), but this
  is rare.

### Could improve

- Consider promoting the recurring multi-line "wire-identical / additive" rationale
  paragraphs (repeated near-verbatim in `remote.rs`, `deploy.rs`, `function.rs`,
  `runtime/lib.rs` for secrets/volumes) into one referenced doc section to cut the
  copy drift risk.

---

## 6. File sizes & naming

### Files over ~500 LOC (real counts; tests noted)

| File | LOC | Verdict |
| --- | --- | --- |
| `runtime/src/lib.rs` (1113, ~819 real) | The frozen core. **Justifiably one file** — it is THE seam and benefits from being read as a unit. Could optionally split `codec` and `__macro_support` into submodules, but the cohesion argument wins. |
| `modal-rust/src/remote.rs` (980, ~704 real) | **Should be split.** The embedded Python `WRAPPER_SRC` (~165 lines) + cache subsystem wants its own module (or a real `.py` via `include_str!`); the `discover_*` env helpers want a `config_discovery` module; `ensure_function` + `RemoteConfig` + `parse_envelope` are the actual facade logic. |
| `sdk/ops/image.rs` (935, ~602 real) | Borderline. The Dockerfile rendering (`dockerfile_commands`, `to_proto`, `bake_command`) and the build-poll (`poll_image_build`, `drain_build_window`) are two distinct concerns that could split into `image/render.rs` + `image/build.rs`. Tests are ~334 lines — the real unit is moderate. |
| `sdk/ops/local_dir.rs` (837, ~581 real) | Reasonable as-is; it has one job (upload) with a clear pipeline (matcher → collect → hash → upload → finalize). The `cfg(unix)`/`cfg(not unix)` `file_mode` split (`:570`) is clean. |
| `sdk/ops/invoke.rs` (693, ~627 real) | Mostly real code, low test ratio. `invoke`/`spawn`/`map`/`get` share a lot of `FunctionMap`+fix#3 boilerplate (the same ~20-line enqueue block appears in `invoke_raw_with_deadline`, `spawn_raw`, and `map_cbor`). A private `enqueue_one`/`enqueue_n` helper would cut ~40 duplicated lines. |
| `modal-rust/src/app.rs` (612, ~498 real) | Reasonable; it is the App handle and its many methods are cohesive. |
| `modal-rust/src/deploy.rs` (582, ~449 real) | Reasonable; its size is inflated by the `DeployConfig` duplication of `RemoteConfig` (§2). |
| `modal-rust/src/scope.rs` (522, ~324 real) | Well-factored: I/O (`run_cargo_metadata`) is split from the pure algorithm (`closure_from_metadata`) specifically so the latter is fixture-testable. Good. |

### Naming

- **Module names are clear and accurate:** `runtime`, `sdk`, `ops/{app,image,mount,
  function,invoke,volume,secret,blob,local_dir}`, `remote`/`deploy`/`scope`/`app`/
  `function`/`error`. `scope.rs` is slightly under-descriptive (it's specifically
  *source-upload scoping*), but the module doc fixes that immediately.
- **The one naming trap** is the duplicate `DEFAULT_DEPLOY_APP` constant with different
  values across `deploy.rs:46` and `cli/main.rs:28` (noted in §3).
- `MapInput` type alias (`app.rs:18`) is a small concession to a genuinely awkward CBOR
  tuple type — fine, and honestly labeled.

---

## 7. Tests, live gating, CUDA exclusion

### Good

- **The dual-gate live-test pattern is excellent.** Every live test is gated by BOTH a
  crate-level `#![cfg(feature = "live")]` (so it doesn't even compile into the default
  test binary) AND a per-test `#[ignore = "reason"]` with an actionable run command
  (e.g. `live_remote.rs:19` + `:39`). All 10 live test files follow this exactly. The
  no-CUDA CI box therefore never runs (or compiles) a real-Modal test, but a developer
  can run any one with a copy-pasteable command. This is the cleanest possible
  separation of "free offline gates" from "costed live proofs."
- **Offline coverage is broad and lives next to the code:** 23 src files carry
  `#[cfg(test)]` modules. The runtime has all five error kinds + precedence + describe +
  duplicate-rejection tested (`runtime/lib.rs:897–1112`); `scope.rs` tests the closure
  algorithm and the manifest rewrite on fixtures including the *panic=unwind*
  preservation (`scope.rs:446`); `image.rs` asserts Dockerfile *ordering* invariants
  (add_python < rustup < bake) rather than just presence (`:725`); `deploy.rs` asserts
  the deployed wrapper contains no `cargo`/`/src` (`:453`). These are *invariant* tests,
  not smoke tests.
- **`not_panic_abort_profile` (`runtime/lib.rs:1104`)** turns the build-profile
  requirement into an asserted property — and `doctor --rust` (`doctor.rs:144`) checks
  the same thing as a user-facing preflight. The invariant is defended at three layers
  (workspace `Cargo.toml` pins `panic = "unwind"`, the test, the doctor).
- **The CUDA exclusion is the right call and is thoroughly documented.**
  `example-burn-add` pulls `cubecl-cuda`, which needs a CUDA toolkit to compile, so it
  stays a workspace *member* (the Modal shim can `-p example-burn-add` it) but is
  excluded from `default-members` so bare `cargo build`/`test`/`clippy`/CI stay green on
  a non-CUDA host (root `Cargo.toml`, with a ~7-line comment explaining exactly why,
  and why `example-cuda-vector-add` — dynamic-loading cudarc — does *not* need
  excluding). This is precisely the kind of decision that is opaque without the comment
  and is fully explained.

### Bad / gaps

- **The embedded Python wrappers have no behavioral tests.** The Rust tests assert the
  *string* substitutes correctly and the archive paths match the Rust constants
  (`remote.rs:709–766`), but the cache pack/unpack logic, the read-only-mount `cp -a`
  fallback (`_build_dir`), and the warm-container skip are only exercised by the live
  tests (`live_cache.rs`). The most logic-heavy part of the run path is the part with
  the least unit-test reach. (This is the cost of the Python-as-string abstraction —
  §2.)
- **`invoke.rs` has the lowest test-to-code ratio of the big files** (~66 test lines on
  627 real) and the highest internal duplication — the enqueue/fix-#3 path is tested
  indirectly through `reassemble_in_order` and the live tests, not the three enqueue
  call sites directly.

### Could improve

- Extract the run wrapper's cache/build logic into a small testable shape (even keeping
  it Python via `include_str!` of a real file would let a `pytest`/doctest reach it; or
  move the decision logic — "is mount writable", "which archive exists" — into Rust and
  keep the Python a thin executor).

---

## Prioritized, non-breaking cleanups (smallest / highest value first)

1. **Fix the duplicate `DEFAULT_DEPLOY_APP` constant** (`deploy.rs:46` =
   `"modal-rust-add-deploy"` vs `cli/main.rs:28` = `"modal-rust-add-poc"`). Either
   rename one (`CLI_DEFAULT_DEPLOY_APP`) or have the CLI default to the facade
   constant. ~5 minutes, removes a real footgun. *(value: high, cost: trivial)*

2. **Collapse the six `discover_*` env helpers** (`remote.rs:344–423`) into one
   `env_bool(name, default)` (and keep `discover_local_root`/`discover_package`/
   `discover_base_image` as the genuinely-different ones). ~30 lines deleted, no
   behavior change. *(value: medium, cost: low)*

3. **Deduplicate the `FunctionMap` + fix-#3 enqueue block** in `invoke.rs` (appears in
   `invoke_raw_with_deadline:140`, `spawn_raw:348`, and `map_cbor:453`) into a private
   `enqueue(function_call_type, invocation_type, items)` helper. ~40 lines, directly
   testable. *(value: medium, cost: low)*

4. **Split `remote.rs`.** Move `WRAPPER_SRC` + `run_wrapper_src` + the cache template
   into `remote/wrapper.rs` (ideally `include_str!("wrapper.py")` so the Python gets
   real tooling), and the `discover_*` helpers into `remote/discover.rs`. Leaves
   `remote.rs` as just `RemoteConfig` + `ensure_function` + `parse_envelope`. Pure
   reorg, no API change. *(value: high, cost: medium)*

5. **Consider a shared build/source config core for `RemoteConfig`/`DeployConfig`.**
   The per-function option core is already factored as `FunctionOptions`; the remaining
   overlap is source/build defaults (`local_root`, `package`, scoping, ignore name,
   base image, timeout default, install-rust). Factor those only if the two structs
   keep drifting. *(value: medium, cost: medium — touches public structs)*

6. **Optionally split `image.rs`** into `image/render.rs` (`dockerfile_commands`,
   `to_proto`, `bake_command`, `python_series_lt_13`) and `image/build.rs`
   (`image_get_or_create`, `poll_image_build`, `drain_build_window`). Two cleanly
   separable concerns. *(value: low–medium, cost: medium)*

7. **Reduce the macro-hygiene wart to a single dep** (longer-term, behind a new
   example so `examples/add-macro` stays a regression guard): re-export `inventory` and
   the runtime registration types through the facade, and change the macro to emit
   facade-relative paths (e.g. `::modal_rust::__rt::...`). This is the only change here
   that touches the frozen macro expansion, so it is last and must be paired with a new
   example proving the single-dep path while keeping the three-dep example green.
   *(value: high for ergonomics, cost: high — frozen-seam-adjacent)*

---

## What the milestone-by-milestone process bought (and cost)

**Bought:** a frozen core that genuinely did not drift; an additive-only seam where
every extension (`--describe`, secrets/volumes, the cargo cache, GPU, CUDA tiering)
provably left the prior wire format byte-identical, with tests asserting it; dated
live-observation comments that turn the codebase into its own design journal; and a
build boundary that is defended by code, tests, *and* negative assertions (the deploy
wrapper provably contains no `cargo`).

**Cost (the accreted complexity to watch):** the run wrapper grew from a thin exec
shim into a Python program with an embedded cache file-format; the additive-config
discipline produced five parallel representations of the same five config fields; and
`RemoteConfig`/`DeployConfig` are near-twins because each was extended in lockstep
rather than refactored to a shared core. All of it is *non-breaking* to clean up — the
seams are frozen, but the plumbing behind them is free to consolidate.
