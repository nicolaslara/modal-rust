# Ergonomics Tasks

Tasks E1–E3: proc-macro registry, generated local Rust remote-call stubs, and an
optional PyO3/maturin bridge. This workpad starts only AFTER the prototype gate
passes (`add` runs via `modal-rust run` and is callable via `modal-rust call`,
with the run-vs-deploy build boundary proven). Background and decisions live in
`knowledge.md`; sources in
`references.md`; contracts in `../architecture/boundaries.md` and
`../architecture/research-synthesis.md`.

## Objective

Add ergonomic sugar WITHOUT changing the frozen seam: a
`#[modal_rust::function]` proc-macro that registers functions via `inventory`,
generated local Rust client stubs so user code can call `app.add(20, 2).await?`
without writing `AddInput`/`AddOutput` wrapper structs, and an OPTIONAL
PyO3/maturin bridge that can replace the subprocess boundary. All are additive —
the macro must compile down to the SAME `Registry` / `HandlerFn` (monomorphized
`fn`-pointer) shape the manual path produces, the remote-call stubs must compile
down to the same `modal-rust-client` transport/envelope path, and the PyO3 path
must be provably optional (the subprocess control path keeps working unchanged).
Do not contradict the synthesis or the design stances (build boundary is the hard
invariant — `run` builds at function-execution time, `deploy` builds at
image-build time and the deployed runtime never runs `cargo`; direct-execution-
first with a Sandbox fallback; prefer static dispatch).

## Gate

The ergonomics gate passes when `knowledge.md` records, with evidence: (1)
`#[modal_rust::function]` produces the validated runner shape — a crate using
the macro builds a `Registry` byte-for-byte equivalent in behaviour to the
manual `Registry::new().function("add", typed!(add))`, the runner CLI protocol
(five frozen error kinds, stdout-only envelope, exit codes, precedence) is
UNCHANGED, and `add` still returns `{"ok":true,"value":{"sum":42}}`; (2)
local Rust code can call the deployed function as `app.add(20, 2).await?`,
with generated private transport types and the real output type returned to the
caller; and (3) PyO3 is proven OPTIONAL — the generated extension crate builds via
`maturin build`/`develop`, a wheel installs in a Modal image and dispatches
through the same `Registry`, AND the existing subprocess `run`/`deploy`/`call`
path still passes with PyO3 absent from the dependency tree. The macro-
compatibility invariant in `../architecture/boundaries.md` is the contract this
gate protects: macros must not change the runner protocol.

## E1 - Proc-macro registry (`#[modal_rust::function]` → `inventory::submit!`)

Status: pending

Acceptance:
- `crates/modal-rust-macros` provides `#[modal_rust::function]` (optionally with
  `name = "..."`) that expands to an `inventory::submit!` registration whose
  collected entries `Registry::from_inventory()` assembles into the SAME
  `BTreeMap<&'static str, HandlerFn>` shape (monomorphized `fn` pointers, no
  `dyn`/vtable/`Box`) as the manual builder (the macro-compatibility invariant in
  `../architecture/boundaries.md`).
- The macro detects `async fn` and expands to `typed_async!(..)`, otherwise
  `typed!(..)`; multi-arg functions expand to a private named-field
  `#[derive(Deserialize)]` args struct + a shim registered via `typed!(shim)`
  (arguments stay a single named JSON object, never a positional array).
- An `examples/add`-equivalent annotated with `#[modal_rust::function(name =
  "add")]` produces a `Registry` that, driven by the UNCHANGED `run_cli`, makes
  `modal_runner --entrypoint add --input-json '{"a":40,"b":2}'` print exactly
  `{"ok":true,"value":{"sum":42}}` and exit 0 — identical to the M0 manual-path
  output.
- The runner CLI protocol is unchanged: all five frozen error kinds
  (`decode_error|unknown_entrypoint|function_error|encode_error|panic`) — where
  `function_error` is the user error WRAPPED on the top-level `RunnerError` enum
  (`message` = Display/anyhow chain, optional additive `details` = the serialized
  user error when its type is `Serialize`, else `null`) — the stdout-only single-
  envelope rule, exit-code mapping, and the frozen precedence (parse → lookup →
  decode → call → encode) behave identically to M0.
- `Registry::from_inventory()` rejects duplicate names with a hard error at
  runner startup (no silent last-write-wins).
- `HandlerFn`, `Registry`, `run_cli`, and `typed!()` signatures in
  `modal-rust-runtime` are NOT modified (the macro is purely additive sugar).
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D
  warnings`, and `cargo test --workspace` all pass.

Evidence:
- `cargo expand -p <macro-example-crate>` (or a `trybuild`/snapshot test) showing
  `#[modal_rust::function(name = "add")]` expanding to `inventory::submit!` plus
  a `typed!(..)`/`typed_async!(..)` wrapper.
- Captured stdout + exit code: macro-built `modal_runner --entrypoint add
  --input-json '{"a":40,"b":2}'` → `{"ok":true,"value":{"sum":42}}`, exit 0,
  byte-identical to the M0 manual-path capture.
- Captured stdout + exit for each of the five error kinds and the precedence test
  on the macro-built runner, matching M0.
- A test asserting two `#[modal_rust::function(name = "dup")]` registrations make
  `Registry::from_inventory()` fail at startup.
- `git diff` (or grep) showing `HandlerFn`/`Registry`/`run_cli`/`typed!`
  signatures in `crates/modal-rust-runtime` are unchanged.
- `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings
  && cargo test --workspace` (green).

## E2 - Generated local Rust remote-call stubs (`app.add(...).await?`)

Status: pending

Acceptance:
- The macro/codegen layer produces a user-facing app client where a deployed
  function is called as `app.add(20, 2).await?` (or the closest Rust-valid module
  form if method generation requires an explicit generated client type). The
  caller does NOT name `AddInput` or `AddOutput` wrapper structs for the common
  case.
- Multi-arg functions generate a private named-field input struct internally,
  preserving the existing named-object wire format; the user passes normal Rust
  arguments.
- The method returns the handler's real output type directly. For example, if
  `add(a: i32, b: i32) -> anyhow::Result<i32>`, then
  `app.add(20, 2).await?` has type `i32`, not `AddOutput`.
- The generated method lowers to the same `modal-rust-client` transport:
  serialize the private input struct with the active codec, invoke deployed
  `call_entrypoint(entrypoint, input_json)` via the validated generated-Python
  path or validated `modal-rs` backend, parse the runner envelope, and map errors
  to the frozen `RunnerError` shape.
- The generated remote-call stubs do NOT alter `HandlerFn`, `Registry`, `typed!`,
  the runner CLI protocol, or the run-vs-deploy build boundary.
- Compare API alternatives before locking syntax: `app.add(20, 2).await?`,
  `remote!(add, 20, 2).await?`, and `remote!(add(a: 20, b: 2)).await?`. Default
  preference is `app.add(...)` because it hides transport types and naturally
  scopes calls to a Modal app/deployment.
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D
  warnings`, and `cargo test --workspace` all pass.

Evidence:
- A local Rust example/test calling deployed `add` as `app.add(20, 2).await?` and
  asserting the returned value has the real output type.
- A macro expansion or generated-code snapshot showing the private input struct,
  generated method, and lowering to `modal-rust-client`.
- Error-path evidence showing a remote `decode_error`/`function_error`/`panic`
  envelope maps to the same client error shape as `modal-rust call`.
- A short syntax comparison note recording why `app.add(...)` won over
  `remote!(...)` or which syntax remains open.
- `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings
  && cargo test --workspace` (green).

## E3 - Optional PyO3/maturin bridge (replace the subprocess boundary)

Status: pending

Acceptance:
- A generated extension crate exposes the same `Registry` dispatch through PyO3
  (e.g. a `dispatch(entrypoint: str, input_json: str) -> str` returning the same
  JSON envelope as the subprocess runner), built with `maturin build` /
  `maturin develop`.
- A wheel is installed into a Modal image (`pip install <wheel>` in an image
  layer), and a Modal Function imports the extension and dispatches through it,
  returning `{"ok":true,"value":{"sum":42}}` for `add` — same envelope shape as
  the subprocess path.
- PyO3 is proven OPTIONAL, not required: the existing subprocess `run` / `deploy`
  / `call` path (M4/M7/M8 recipe) still passes with PyO3/maturin ABSENT from the
  default dependency tree — the bridge lives behind a feature flag or a separate
  crate, and `cargo tree` for the default build shows no `pyo3`/`maturin`
  dependency.
- The design stances hold: the build boundary remains the hard invariant — `run`
  still builds at function-execution time and `deploy` still builds at image-build
  time with the deployed runtime never invoking `cargo` (the PyO3 path changes only
  the shim↔runner boundary, not the build-placement boundary, and that invariant
  holds whether the build runs in a Function body or a Sandbox fallback).
- The five-kind error taxonomy and the JSON envelope (including the optional
  additive `details` field and `function_error` wrapping the user error on the
  top-level `RunnerError` enum) are preserved across the PyO3 boundary (a Rust
  panic / decode / encode failure still surfaces the same envelope, not a raw
  Python traceback) — OR any divergence is recorded as a gap.
- `cargo fmt --check`, `cargo clippy`, and `cargo test --workspace` pass for the
  default (non-PyO3) build; the PyO3 crate builds under its feature/maturin.

Evidence:
- `maturin build` (or `maturin develop`) output producing a wheel; recorded
  maturin/PyO3/abi3 pins.
- A Modal run/deploy where the Function dispatches via the imported extension and
  returns `{"ok":true,"value":{"sum":42}}` (console output recorded).
- `cargo tree` for the DEFAULT build showing no `pyo3`/`maturin` dependency
  (proves optional), plus a passing subprocess `modal-rust run add --input
  '{"a":40,"b":2}'` → 42 with PyO3 absent.
- A test (or recorded run) showing a panic/decode/encode failure still surfaces
  the frozen envelope across the PyO3 boundary, or a recorded gap note.
- `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings
  && cargo test --workspace` (green) for the default build.
