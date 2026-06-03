# Ergonomics Knowledge

## Objective

Capture the decisions, facts, and open questions for the ergonomics phase —
proc-macro registry (`#[modal_rust::function]` via `inventory`), generated local
Rust remote-call stubs (`app.add(20, 2).await?`), and an optional PyO3/maturin
bridge — so they can be added as PURE SUGAR without touching the frozen runner
seam. The canonical contracts live in
`../architecture/boundaries.md` and `../architecture/research-synthesis.md`; this
file records the reasoning, confidence, and what remains user-sensitive. The
design stances bind: the build boundary is the hard, non-negotiable invariant
(`run` builds at function-execution time, `deploy` builds at image-build time and
the deployed runtime never runs `cargo`); plus direct-execution-first with a
documented Sandbox fallback, and prefer static dispatch. This workpad starts only
AFTER the prototype gate passes.

## Gate Status

Not passed yet.

The ergonomics gate passes when this file records, with evidence: (1) the
`#[modal_rust::function]` macro produces the validated runner shape — the same
`Registry`/`HandlerFn` (monomorphized `fn`-pointer) dispatch and the UNCHANGED
runner CLI protocol, with `add` still returning
`{"ok":true,"value":{"sum":42}}`; (2) generated local Rust stubs let user code call
the deployed function as `app.add(20, 2).await?`, with private transport structs
and the real output type returned; and (3) PyO3 is proven OPTIONAL — a
maturin-built wheel dispatches through the same `Registry` in a Modal image while
the subprocess `run`/`deploy`/`call` path still passes with PyO3 absent from the
default dependency tree (per `WORKING.md` Workpad Gates and `AGENTS.md`: macros
must not change the runner protocol). The macro-compatibility invariant in
`../architecture/boundaries.md` is the contract this gate protects.

## Decisions

Seeded from `research-synthesis.md` §2.3 (macro-compatible registry) and the
`project.md` / `AGENTS.md` runtime contract. Items marked **[CHANGED]** folded in
a HIGH-severity review `must_fix` already committed in the frozen trait so the
macro path is additive, not seam-breaking.

- **Ergonomics is the LAST phase and starts only after the prototype gate.** No
  proc-macros and no PyO3 until the manual subprocess path works end to end
  (`WORKING.md`: "Do not add ergonomics (macros, PyO3) before the manual
  subprocess path works end to end"). (project.md, WORKING.md)
- **Macro-compatibility invariant (the contract this phase protects).** Every
  entry — manual or macro-registered — reduces to the SAME monomorphized
  `HandlerFn` (bare `fn` pointer, no `dyn`/vtable/`Box`); `typed!()` owns all
  decode/encode; `run_cli` and `Registry` never change shape regardless of
  registration path. The manual registry and the future
  `inventory`/`#[modal_rust::function]` path converge on ONE dispatch code path:
  `name -> typed! wrapper (fn pointer) -> JSON bytes in -> JSON bytes out`. (§2.3,
  project.md, `../architecture/boundaries.md`)
- **`#[modal_rust::function]` expands to `inventory::submit!`.** The macro
  registers each annotated fn via `inventory` (submitting the monomorphized `fn`
  pointer); `Registry::from_inventory()` collects the submitted entries into the
  same `BTreeMap<&'static str, HandlerFn>` the manual
  `Registry::new().function(...)` builder produces. The future proc-macro may
  alternatively emit a fully static `match` dispatch table. The runner template
  stays identical between the manual and macro worlds. (§2.3, project.md "v2 macro
  must expand to the same registry shape")
- **[CHANGED — Rust #4] Macro async detection is already reserved.**
  `typed_async!` is reserved with the same `fn`-pointer shape in the runtime now
  (may be unimplemented in v0). The macro detects `async fn` and expands to
  `typed_async!(..)` vs `typed!(..)`, keeping async additive with no change to the
  `HandlerFn` shape. (§2.3)
- **[CHANGED — Rust MED] Frozen argument shape drives multi-arg expansion.** The
  runner input is always a single JSON object. Single-arg handlers take `In`
  directly; a multi-arg macro generates a private `#[derive(Deserialize)]` named-
  field args struct (field names = parameter names) + a shim fn that
  destructures and calls `f(a, b)`, registered via `typed!(shim)`. Arguments are a
  named object, never a positional array. (§2.3)
- **[CHANGED — Rust MED] Duplicate-name rejection at startup.**
  `Registry::function()` and `Registry::from_inventory()` reject duplicate names
  with a hard error at runner startup. This matters MORE for the macro/inventory
  world, where a duplicate `#[modal_rust::function(name = "x")]` is easy to write;
  silent last-write-wins is a footgun. (§2.3)
- **The runner protocol is the frozen seam and the macro must not touch it.** Five
  error kinds (`decode_error|unknown_entrypoint|function_error|encode_error|
  panic`), where `function_error` is the user error WRAPPED on the top-level
  `RunnerError` enum — `message` = Display/anyhow chain, with an optional additive
  `details` field = the serialized user error when its type is `Serialize` (else
  `null`); stdout-only single-envelope, exit-code mirrors `ok`, precedence
  (parse → lookup → decode → call → encode). Any change must be additive-only and
  reviewed against the manual-registry runner. (§2.2, AGENTS.md, WORKING.md)
- **User-facing remote ergonomics should hide transport wrapper types.** The
  low-level client may still have `RemoteFunction<In, Out>` internally, but the
  macro/codegen target is `app.add(20, 2).await?`: generated private named input
  structs preserve the wire format, and the method returns the handler's real
  output type directly. This avoids making users write or think about `AddInput`
  / `AddOutput` for the common path. (E2)
- **`app.add(...)` is the preferred default syntax to evaluate first.** It scopes
  calls to a deployment/app handle and matches the planned macro-generated stubs
  better than a free `remote!(add, ...)` macro. Keep `remote!(add, 20, 2)` and
  `remote!(add(a: 20, b: 2))` as alternatives to compare before locking the API,
  but do not let either expose generated transport structs. (E2 open design)
- **[CHANGED — Rust #5] Codec-neutral `fn`-pointer shape makes future formats
  additive.** `type HandlerFn = fn(&[u8]) -> Result<Vec<u8>, RunnerError>` (static
  dispatch, no trait object). The macro emits `typed!(..)`/`typed_async!(..)`
  wrappers which own the JSON `Codec`; a future CBOR path adds only a `Codec` impl.
  The macro never needs to know the wire format. (§2.3)
- **PyO3/maturin is a LATER, OPTIONAL bridge — not a v0 dependency.** It replaces
  the subprocess boundary with an in-process call, validated only AFTER the
  subprocess POC works. It must be provably optional: the default build carries no
  `pyo3`/`maturin` dependency, and the subprocess `run`/`deploy`/`call` path keeps
  passing. (project.md "Stack Direction", "PyO3/maturin and proc-macros are later
  optimizations, not v0 dependencies")
- **PyO3 path changes only the shim↔runner boundary, NOT the build boundary.** The
  design stances still hold: the build boundary is the hard invariant — `run` still
  builds at function-execution time and `deploy` still builds at image-build time
  with the deployed runtime never invoking `cargo` (and that invariant holds
  whether the build runs in a Function body or a Sandbox fallback). The PyO3 bridge
  is a wheel installed in the image; the build-placement boundary is unchanged.
  (project.md design stances, AGENTS.md)
- **Concurrency caveat carried forward from v0 (recorded for any in-process
  host).** The v0 panic-capture uses a process-global slot and the process exits
  after one call, so it is correct for v0. A future concurrent in-process host
  (PyO3 "Mode B") MUST revisit per-call panic routing and the panic-then-reuse
  hazard before enabling concurrency. (§2.3 concurrency caveat)
- **`FunctionCreate` always needs a Python `function_serialized` + `image_id`.**
  Even with PyO3, the deployed Modal unit is still a Python-defined function;
  modal-rs does NOT remove the Python shim. The PyO3 bridge replaces the
  subprocess call inside the body, not the Python authoring surface. (§1.5,
  research-synthesis residual risk #5)

## Findings

Seeded from `research-synthesis.md` §1 / §2.3 and `project.md`. Confidence as
noted; PyO3/maturin specifics are to be VERIFIED during E3 (no spike run yet).

- The frozen seam already reserves exactly what the macro phase needs:
  codec-neutral `fn`-pointer shape `type HandlerFn = fn(&[u8]) -> Result<Vec<u8>,
  RunnerError>` (static dispatch, no `dyn`) (§2.3, Rust #5), reserved `typed_async!`
  with the same `fn`-pointer shape for `async fn` (§2.3, Rust #4), frozen named-
  object argument shape for multi-arg expansion (§2.3, Rust MED), and duplicate-
  name rejection (§2.3, Rust MED). These were folded in NOW so the GPU/PyO3/macro
  phases are additive rather than seam-breaking. (high) (§2.3, residual risk #7)
- `inventory` collects distributed registrations at startup; pairing it with
  `Registry::from_inventory()` is the documented path to a macro registry that
  assembles the same `BTreeMap<&'static str, HandlerFn>` as the manual builder
  (or, alternatively, a fully static `match` dispatch table).
  (medium — to confirm against `inventory` docs in E1) (§2.3, project.md)
- A deployed Modal Function image must still host Modal's Python runtime
  (`add_python` required) even when the workload is native; a PyO3 extension is an
  in-image wheel the Python entrypoint imports. So PyO3 does not eliminate the
  Python layer — it changes how the body reaches Rust (in-process import vs
  subprocess exec). (high) (§1.1, §2.5, §1.5)
- PyO3/maturin specifics (abi3 vs version-specific wheels, manylinux/linux-amd64
  wheel compatibility with the `rust:slim` + `add_python='3.12'` image, whether
  `maturin develop` is usable inside an image build vs only `maturin build` + `pip
  install`) are UNVERIFIED here — E3 must establish them with a spike and record
  pins. (open) (project.md PyO3/maturin references)

## Open Questions

Product- and design-sensitive decisions for this phase. None are gate blockers on
their own, but E2's remote-call syntax and E3's "optional" requirement / wire
format affect API shape — surface to the user before locking. The four §4
synthesis questions remain in force; the ones below are ergonomics-specific.

- **Macro surface — `name` inference vs explicit.** Default: support explicit
  `#[modal_rust::function(name = "add")]`; infer the entrypoint name from the fn
  name when `name` is omitted. Option: require explicit names to avoid collisions
  in large crates. (E1 design; not a blocker)
- **Remote-call syntax.** Default candidate: generated app methods,
  `app.add(20, 2).await?`, because the app handle scopes deployment/auth and the
  macro can hide both input and output wrapper types. Alternatives to compare:
  `remote!(add, 20, 2).await?` and `remote!(add(a: 20, b: 2)).await?`. The
  selected syntax must not expose `AddInput`/`AddOutput` in the common path. (E2)
- **PyO3 packaging — feature flag vs separate crate.** Default: keep the PyO3
  bridge behind a non-default cargo feature and/or in a separate crate so the
  default build's `cargo tree` shows no `pyo3`/`maturin` (the "proven optional"
  requirement). Option: a dedicated `modal-rust-pyo3` crate. (E3 design)
- **Whether PyO3 ever becomes the default `call`/dispatch boundary.** Default: NO
  for v0 — subprocess stays the validated control path; PyO3 is an opt-in
  optimization, promoted only after a proven non-scalar round-trip and an
  equivalence check of the five-kind envelope across the boundary. (E3; mirrors
  the §4.3 default `call` mode stance)
- **Wire format across the PyO3 boundary.** Default: JSON envelope as a Python
  `str` (same as the subprocess `--input-json`/stdout text), since the `Handler`
  trait is codec-neutral on `&[u8]`. Option: pass `bytes` for a future
  CBOR/msgpack `Codec`. (mirrors §4.4 wire-format default)

## E1 proc-macro registry (2026-06-03)

E1 is DONE: build + verify both pass with captured evidence. The
`#[modal_rust::function]` macro lands as pure additive sugar — the macro path is
byte-identical in behaviour to the manual `Registry::new().function("add",
typed!(add))` builder, and the frozen runner seam is untouched.

**What the macro generates.** `#[modal_rust::function]` (optionally
`#[modal_rust::function(name = "add")]`) is an attribute macro at
`crates/modal-rust-macros/src/lib.rs`. Applied to a handler such as `pub fn
add(input: AddInput) -> anyhow::Result<AddOutput>`, it emits two things:
(1) the **original function verbatim**, unchanged; and (2) an
`inventory::submit!` of a `modal_rust_runtime::Registration { name, handler }`
whose `handler` is the SAME monomorphized `modal_rust_runtime::typed!(add)`
wrapper `fn` pointer the manual builder produces. The entrypoint name defaults to
the fn name and is overridable via `name = "..."`. The macro never invents a wire
format — `typed!` still owns all decode/encode, so the registration reduces to the
one dispatch path `name -> typed! wrapper (fn pointer) -> JSON bytes in -> JSON
bytes out`. Verified: runtime diff is **+42/−0** (only a new `Registration` struct
+ `inventory::collect!` + `from_inventory()`, reusing the existing `.function()`);
`HandlerFn` stays a bare `fn` pointer (no `Box`/`dyn`/vtable).

**The `Registry::from_inventory()` addition.** A new associated constructor on the
existing (unchanged-shape) `Registry` in
`crates/modal-rust-runtime/src/lib.rs`. It iterates the `inventory`-collected
`Registration` submissions and assembles the SAME `BTreeMap<&'static str,
HandlerFn>` as the manual builder, routing each through the existing
`.function()` so duplicate names hit the same hard "duplicate entrypoint" error
(no silent last-write-wins). The runner binary body collapses to exactly
`modal_rust_runtime::run_cli(modal_rust_runtime::Registry::from_inventory())`.

**The example.** `examples/add-macro` mirrors `examples/add`:
`#[modal_rust::function] pub fn add(AddInput) -> anyhow::Result<AddOutput>` in
`src/lib.rs`, the one-line `from_inventory()` runner in
`src/bin/modal_runner.rs`. It carries lib tests (byte-identical envelope,
`from_inventory` lookup, `unknown_entrypoint`) plus a separate-binary integration
test `tests/duplicate_rejected.rs` proving two registrations of the same name make
`from_inventory()` fail with the frozen "duplicate entrypoint" error. Workspace
`members` + `default-members` updated for both new crates
(`modal-rust-macros`, `add-macro`).

**async / `typed_async!`: DEFERRED, not implemented.** The macro DETECTS `async
fn` but does NOT emit `typed_async!` — `typed_async!` is reserved in the runtime
(boundaries.md §3) but not yet implemented, so emitting it would not compile.
Instead the macro rejects `async fn` with a clear `compile_error!` ("use a
synchronous handler that may `block_on` internally for now"), keeping the original
fn so the rest of the crate still type-checks. Multi-arg handlers are likewise
DEFERRED with a `compile_error!` (v0 supports exactly one `In` argument, matching
`typed!(add)`); the reserved private-args-struct + shim expansion is not built yet.
Both rejections are diagnostics, never silent mis-registration. When `typed_async!`
lands, the async arm switches from diagnostic to emitting `typed_async!(..)` with
the identical `HandlerFn` shape — no seam change required.

**Manual path + frozen runner protocol UNCHANGED (confirmed).** `HandlerFn`,
`Registry` (shape), `typed!`, `run_cli`, and the five-kind `RunnerError`
(`decode_error|unknown_entrypoint|function_error|encode_error|panic`) are all
unmodified. Verified by running the macro-built runner: `--entrypoint add
--input-json '{"a":40,"b":2}'` → exactly `{"ok":true,"value":{"sum":42}}`, exit 0,
`cmp`-clean against the manual `examples/add` capture; `unknown_entrypoint` → exit
1 with `details:null` and known-list `["add"]`; `decode_error` (malformed JSON) →
byte-identical macro-vs-manual (`expected ident at line 1 column 2`, exit 1);
precedence (bad JSON + unknown entrypoint → `decode_error`) → byte-identical, exit
1. stdout-only single-envelope and exit-code mapping behave identically.
Default-members `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
and `cargo test` all exit 0. No git mutations.

Caveats: verification ran clippy/test on `default-members` (not
`--workspace`/`--all-features` literally), but covered both new crates. One
pre-existing flaky runtime unit test (`panic_captured_with_backtrace`, the
documented process-global panic-hook race per boundaries §3) flaked once mid-run
then passed 5/5 subsequent full runs; unrelated to this change (no edits to
`run_handler`, the panic hook, or the global slot).

Evidence files: `crates/modal-rust-macros/src/lib.rs`,
`crates/modal-rust-runtime/src/lib.rs`, `examples/add-macro/src/lib.rs`,
`examples/add-macro/src/bin/modal_runner.rs`,
`examples/add-macro/tests/duplicate_rejected.rs`.

**Next (optional):** E2 (generated local Rust remote-call stubs,
`app.add(20, 2).await?`) and then E3 (PyO3/maturin bridge) remain the optional
next ergonomics steps; neither is started. The frozen seam already reserves what
they need.
