# Architecture Tasks

Tasks A0–A8: **ratify and finalize** the canonical contract doc `boundaries.md`,
one boundary per task. `boundaries.md` is **drafted from the locked synthesis**;
the architecture phase confirms each section is present, correct, consistent with
`research-synthesis.md`, and (for `[spike: Rx]`-tagged decisions) confirmed
against the empirical research spike named — then passes the gate. Cockpit for
the architecture workpad. Reasoning and confidence live in `knowledge.md`;
sources in `references.md`; the authoritative long-form decisions in
`research-synthesis.md`.

## Objective

Ratify the single reviewable contract every later phase builds against —
`workpads/architecture/boundaries.md` (drafted from the synthesis) — one task per
boundary: workspace + crate layout; the frozen runner protocol; the
`Registry`/`typed!()`/`HandlerFn` static-dispatch API (macro-compatible); the run-vs-deploy build
boundary; the generated shim design (`dev_app`/`deploy_app`/`call_app`); the
`modal-rust` CLI surface; the Cargo-cache design; the
`.modalrustignore`/`.gitignore` ignore rules; then a gate review. Confirm each
section records its contract and failure modes, resolve any `[spike: Rx]`
dependency against the research findings, and surface the user-sensitive
decisions. Do not contradict `research-synthesis.md` or the design stances (the
build boundary is the hard invariant: `run` builds at function-execution time,
`deploy` builds at image-build time and the deployed runtime never runs `cargo`;
direct-execution-first with a Sandbox fallback; prefer static dispatch).

## Gate

The architecture gate passes when `workpads/architecture/boundaries.md` records,
with no contradiction of `research-synthesis.md` or the design stances (the build
boundary being the hard invariant): (1) the
cargo workspace + crate layout and its acyclic dependency edges; (2) the runner
CLI protocol — the frozen five-kind error taxonomy, exit-code mapping,
stdout-only-envelope rule, and frozen precedence; (3) the `Registry`/`typed!()`/
`HandlerFn` static-dispatch API including the codec-neutral fn-pointer
`type HandlerFn = fn(&[u8]) -> Result<Vec<u8>, RunnerError>` (no `Box<dyn>`/vtable),
the macro-compatibility invariant, reserved `typed_async!`, and
duplicate-name rejection; (4) the run-vs-deploy build boundary as an explicit
table making clear the deployed runtime never invokes `cargo`; (5) the three
generated shims (`dev_app` copy=False + cargo-in-body; `deploy_app` copy=True +
`run_commands` + baked binary; `call_app` `Function.from_name` + `.remote()`);
(6) the `modal-rust` CLI surface (`doctor`/`run`/`deploy`/`call`); and (7) the
Cargo-cache design and `.modalrustignore`/`.gitignore` ignore rules. The
user-sensitive decisions (GPU/cost confirmation, public deploys, default `call`
mode, wire format, toolchain pin, cache sharing) are explicitly surfaced with the
synthesis's recommended defaults. `knowledge.md` and `references.md` are seeded
from the synthesis.

## A0 - Cargo workspace + crate layout

Status: pending

Acceptance:
- `boundaries.md` has a "Workspace & crate layout" section recording a single
  virtual cargo workspace at repo root with `crates/modal-rust-runtime`,
  `crates/modal-rust-cli`, `crates/modal-rust-client`, `crates/modal-rust-macros`
  (empty placeholder), and `examples/add`.
- The acyclic dependency edges are stated explicitly: `macros -> runtime`;
  `client -> runtime`; `cli -> client + runtime`; per-user runner binary `->
  runtime + the user's own lib crate`. No cycle is implied.
- It records that `modal-rust-runtime` has ZERO Modal/network/Python deps (serde,
  serde_json, anyhow, a tiny hand-rolled arg parser only), with the rationale
  (recompiled every dev run, baked into the deploy image — keep it minimal), and
  that `clap` is CLI-only (the clap-in-runtime review conflict is resolved).
- It records edition 2021 for published crates (2024 only in user/example
  crates), and that the user does not own `main()`: the CLI owns the ~15-line
  `src/bin/modal_runner.rs` template whose fixed body is
  `modal_rust_runtime::run_cli(user_crate::modal_registry())`; the user authors
  only `lib.rs` and `pub fn modal_registry() -> Registry`.

Evidence:
- File path: `/Users/nicolas/devel/modal-rust/workpads/architecture/boundaries.md`,
  "Workspace & crate layout" section (traces to `research-synthesis.md` §2.1).
- `grep -nE "modal-rust-runtime|modal-rust-cli|modal-rust-client|modal-rust-macros|examples/add" workpads/architecture/boundaries.md`
  lists all four crates + the example.
- `grep -n "run_cli" workpads/architecture/boundaries.md` shows the fixed runner
  `main()` body.
- `grep -nE "zero|clap" workpads/architecture/boundaries.md` shows the
  zero-dep-runtime rule and clap-is-CLI-only resolution.

## A1 - Runner CLI protocol (the frozen seam)

Status: pending

Acceptance:
- `boundaries.md` has a "Runner CLI protocol" section: binary `modal_runner`,
  invocation `--entrypoint <name> ( --input-json <json> | --input-file <path> |
  --input-stdin )`, with `--input-file`/`--input-stdin` justified (argv-length /
  ~100 MB gRPC ceiling).
- The stdout-only-envelope rule is stated: stdout carries EXACTLY ONE JSON
  envelope; all cargo/rustc/user diagnostics go to stderr. Exit code mirrors `ok`
  (0 success / 1 failure); the error kind lives in the JSON, not the exit code.
- The success envelope `{"ok":true,"value":<json>}` and failure envelope
  `{"ok":false,"error":{"kind":"...","message":"...","details":<json|null>,"backtrace":"..."}}`
  are recorded verbatim, including the additive optional `details` field.
- All FIVE frozen error kinds are listed with their cause: `decode_error`,
  `unknown_entrypoint`, `function_error`, `encode_error`, `panic`. `encode_error`
  is noted as added (Rust review HIGH #3) so an `Out`-serialization failure is
  never mis-reported as `panic`. `function_error` is recorded as the user error
  WRAPPED on the top-level `RunnerError` enum — `message` = Display/anyhow chain,
  `details` = the serialized user error when the handler's error type is
  `Serialize` (else `null`).
- The frozen precedence is stated: top-level JSON parse -> entrypoint lookup ->
  decode `In` -> call -> encode `Out` (malformed JSON + bad entrypoint yields
  `decode_error`). Panic capture (panic hook + `std::backtrace::Backtrace` +
  `catch_unwind`; shim sets `RUST_BACKTRACE=1`) and the additive-optional-fields
  compatibility rule (`details` follows it; `meta`/`version` reserved) are recorded.

Evidence:
- File path: `boundaries.md`, "Runner CLI protocol" section (traces to
  `research-synthesis.md` §2.2).
- `grep -nE "decode_error|unknown_entrypoint|function_error|encode_error|panic" workpads/architecture/boundaries.md`
  lists all five kinds.
- `grep -nE "EXACTLY ONE|stderr|exit" workpads/architecture/boundaries.md` shows
  the stdout-only-envelope + exit-code rules.
- `grep -nE "precedence|RUST_BACKTRACE|catch_unwind" workpads/architecture/boundaries.md`
  shows precedence + panic capture.
- `grep -nE "details|wrapped|wraps" workpads/architecture/boundaries.md` shows the
  optional `details` field and the user-error-wrapped-on-the-top-level-enum framing.

## A2 - Registry / typed!() / HandlerFn API (macro-compatible)

Status: pending

Acceptance:
- `boundaries.md` has a "Registry / typed!() / HandlerFn" section recording the
  static-dispatch + macro-compatibility invariant: every entry is a monomorphized
  wrapper reduced to a bare `fn` pointer (NO `Box<dyn Handler>`, no vtable);
  `run_cli`/`Registry` never change shape regardless of registration path; the
  manual registry and the future `inventory`/`#[modal_rust::function]` path
  converge on one dispatch code path
  (`name -> monomorphized typed! wrapper (fn pointer) -> bytes in -> bytes out`).
- The handler is recorded as a codec-neutral, synchronous bare `fn` pointer:
  `type HandlerFn = fn(&[u8]) -> Result<Vec<u8>, RunnerError>` (Rust review HIGH
  #5; prefer-static-dispatch stance), with `typed!(f)` — a `macro_rules!` that
  yields a monomorphized wrapper `fn` pointer — owning decode/encode via a `Codec`
  (JSON for v0) so a CBOR `Codec` is additive and never touches `HandlerFn`/`Registry`.
- `typed_async!` is reserved now (Rust review HIGH #4) with the same fn-pointer
  shape — `block_on`s a runtime-owned Tokio executor inside the
  `fn(&[u8]) -> Result<Vec<u8>, _>` wrapper, may be unimplemented in v0, but its
  shape is committed; the future macro detects `async fn` and expands to
  `typed_async!` vs `typed!`.
- `Registry` is recorded as `BTreeMap<&'static str, HandlerFn>` (static-str keys,
  fn-pointer values — no allocation, no `dyn`); builder
  `Registry::new().function("add", typed!(add))`; duplicate names rejected with a
  hard error at startup (not silent last-write-wins).
- The frozen argument shape is recorded: input is always a single named JSON
  object; single-arg handlers take `In` directly; a future multi-arg macro
  generates a named-field `#[derive(Deserialize)]` args struct — never a
  positional array.

Evidence:
- File path: `boundaries.md`, "Registry / typed!() / HandlerFn" section (traces to
  `research-synthesis.md` §2.3).
- `grep -nE "HandlerFn|fn\(&\[u8\]\)|Result<Vec<u8>, RunnerError>" workpads/architecture/boundaries.md`
  shows the codec-neutral fn-pointer handler type (no `Box<dyn>`).
- `grep -nE "typed_async!|Codec|BTreeMap|duplicate" workpads/architecture/boundaries.md`
  shows reserved async, codec, registry type, and duplicate-name rejection.
- `grep -nE "named|positional" workpads/architecture/boundaries.md` shows the
  frozen named-object argument shape.

## A3 - Run-vs-deploy build boundary

Status: pending

Acceptance:
- `boundaries.md` has a "Run-vs-deploy build boundary" section with an explicit
  table contrasting the two paths: `run` = build at function-execution time
  (`add_local_dir(copy=False)` mount + `cargo build` in the function body);
  `deploy` = build at image-build time (`add_local_dir(copy=True)` +
  `run_commands(cargo build)`, binary baked to `/app/modal_runner`).
- The deployed-runtime invariant is stated explicitly: the deployed body execs
  ONLY `/app/modal_runner ...`, mounts no source and no cache Volume, and NEVER
  invokes `cargo`. The proof obligation is recorded: `cargo build` appears in
  deploy/build logs and is ABSENT from call logs.
- The run-path build location is recorded as a known-writable LOCAL path by
  default (`CARGO_TARGET_DIR=/tmp/target`), NOT a Volume (Modal review HIGH #2),
  with the read-only-mount fallback (`cp -a /src /tmp/build`) noted.
- It is recorded that this boundary is the product and overrides convenience, and
  that it is the hard, non-negotiable invariant — holding whether the build runs
  in a Function body or a Sandbox. Per the direct-execution-first stance, the happy
  path uses a normal `@app.function`; a Sandbox is a documented fallback if a
  Function-body build proves infeasible, and the run-vs-deploy split is unchanged
  either way.

Evidence:
- File path: `boundaries.md`, "Run-vs-deploy build boundary" section (traces to
  `research-synthesis.md` §2.4–§2.5, AGENTS.md, project.md).
- `grep -nE "copy=False|copy=True|run_commands" workpads/architecture/boundaries.md`
  shows both build placements.
- `grep -n "never" workpads/architecture/boundaries.md` shows the
  deployed-runtime-never-runs-cargo invariant.
- `grep -nE "ABSENT|absent" workpads/architecture/boundaries.md` shows the
  cargo-in-deploy-logs / absent-from-call-logs proof obligation.

## A4 - Generated shim design (dev / deploy / call)

Status: pending

Acceptance:
- `boundaries.md` has a "Generated shims" section recording all THREE shims as
  recipes:
  - `dev_app.py`: `from_registry("rust:{VER}-slim", add_python="3.12").entrypoint([])`
    + `.env({"RUST_BACKTRACE":"1"})` + `.add_local_dir(LOCAL_SRC,"/src",copy=False,
    ignore=[...])`; body `cargo build --release --bin modal_runner` then exec via
    the runner protocol; CLI flags routed via `@app.local_entrypoint()` (Modal
    review HIGH #1, since a bare `@app.function` does NOT auto-bind `modal run`
    flags); `timeout=1800`; never `vol.reload()` mid-build.
  - `deploy_app.py`: `from_registry(...).entrypoint([]).env(...)
    .add_local_dir(MANIFEST_DIR,"/app",copy=True,ignore=[...])
    .run_commands("cd /app && cargo build --release --bin modal_runner")
    .run_commands("cp .../release/modal_runner /app/modal_runner && chmod +x ...")`;
    deployed body execs ONLY `/app/modal_runner`.
  - `call_app.py`: invocation via `modal.Function.from_name(APP, fn).remote(...)`,
    with the call logic inside a `@app.local_entrypoint()` (Modal review HIGH #6 —
    a module-scope `print(fn.remote(...))` would `NameError`).
- It records that `add_python="3.12"` is mandatory (a bare `rust:` image is an
  invalid Function image) and `.entrypoint([])` neutralizes any base ENTRYPOINT
  so Modal's Python runtime starts.
- It records invoked arg + return are plain `str` (the JSON envelope text), well
  under the ~100 MB gRPC limit; large I/O routes via a Volume/object storage (out
  of scope for the `add` POC); and that generated shims are disposable artifacts
  under gitignored `.modal-rust/generated/`.

Evidence:
- File path: `boundaries.md`, "Generated shims" section (traces to
  `research-synthesis.md` §2.4–§2.5).
- `grep -nE "dev_app|deploy_app|call_app" workpads/architecture/boundaries.md`
  lists all three shims.
- `grep -nE "local_entrypoint|Function.from_name|add_python|entrypoint\(\[\]\)" workpads/architecture/boundaries.md`
  shows arg-routing, the call recipe, and the image preconditions.

## A5 - CLI surface (doctor / run / deploy / call)

Status: pending

Acceptance:
- `boundaries.md` has a "CLI surface" section recording the four subcommands:
  - `modal-rust doctor [--rust]`: preflight `~/.modal.toml`/`MODAL_TOKEN_*`,
    `modal` CLI on `$PATH`, pinned rust/python/image-builder versions; `--rust`
    adds `cargo`/`rustc`/`target` + `panic = "abort"` detection; missing
    prerequisites produce an actionable structured error (reusing the runner error
    model).
  - `modal-rust run <entrypoint> [--input <json|@file>] [--gpu] [--timeout]`:
    generate `dev_app.py`, then `modal run`.
  - `modal-rust deploy <entrypoint> [--gpu] [--app-name]`: generate `deploy_app.py`,
    then `modal deploy`.
  - `modal-rust call <entrypoint> [--input <json|@file>] [--app-name]
    [--use-modal-rs]`: generate/locate `call_app.py` via `modal run` (default), or
    behind a validated `--use-modal-rs` flag use `Function::from_name().remote()`.
- It records the CLI is a pure wrapper introducing NO new Modal capability:
  generated shims must be byte-equivalent (modulo injected params) to the
  prototype shims, and are private/gitignored under `.modal-rust/generated/`.
- It records the `panic = "unwind"` build-profile constraint and the
  doctor `panic = "abort"` check (Rust review MED), and the resolution that v0
  authoring/build uses generated Python + the official `modal` CLI, confining
  modal-rs to `call` behind `--use-modal-rs`.

Evidence:
- File path: `boundaries.md`, "CLI surface" section (traces to
  `research-synthesis.md` §2.7, §2.6).
- `grep -nE "doctor|run <entrypoint>|deploy <entrypoint>|call <entrypoint>" workpads/architecture/boundaries.md`
  lists the four subcommands.
- `grep -nE "use-modal-rs|panic = \"abort\"|byte-equivalent" workpads/architecture/boundaries.md`
  shows the call-mode flag, the abort check, and the pure-wrapper invariant.

## A6 - Cargo-cache design

Status: pending

Acceptance:
- `boundaries.md` has a "Cargo cache" section recording the run-path-only Volume
  cache: `Volume.from_name("modal-rust-cargo-cache", create_if_missing=True)` at
  a STABLE mount path; `CARGO_HOME` (read-mostly index/downloads) MAY sit on the
  Volume; `CARGO_TARGET_DIR` stays `/tmp/target` by default and is promoted to the
  Volume only if it benchmarks net-positive and lock-safe.
- It records the correctness rule: a cache miss only costs time, never a wrong
  result; correctness never depends on cache state.
- It records the Modal Volume semantics that shape the design: automatic
  background commits "every few seconds" + a final commit, so explicit
  `vol.commit()` is often unnecessary; `vol.reload()` fails "volume busy" when
  files are open (Cargo holds locks) — never call it on the hot build path.
- It records that the cache is best-effort and NOT a dependency of deploy (the
  deploy path mounts no cache Volume), the documented cache reset (`modal volume
  rm` / new name), and single-writer / low-concurrency (v1 last-write-wins, avoid
  >~5 concurrent commits; parallel shared-cache writes out of scope for v0).

Evidence:
- File path: `boundaries.md`, "Cargo cache" section (traces to
  `research-synthesis.md` §2.4 cache, §1.3).
- `grep -nE "modal-rust-cargo-cache|CARGO_HOME|CARGO_TARGET_DIR" workpads/architecture/boundaries.md`
  shows the Volume name + the two env vars.
- `grep -nE "reload|background commit|best-effort|not a dependency" workpads/architecture/boundaries.md`
  shows the no-reload-on-hot-path rule and the best-effort / not-a-deploy-dependency
  framing.

## A7 - Ignore rules (.modalrustignore / .gitignore)

Status: pending

Acceptance:
- `boundaries.md` has an "Ignore rules" section recording what is excluded from
  mounts/copies and from git, and why each is excluded:
  - Mount/copy ignore (client-side, dockerignore-syntax or predicate): `target`,
    `.git`, `.modal-rust`, and build artifacts (e.g. `**/*.rlib`) — so the upload
    is minimal and reactive; applied to both `add_local_dir(copy=False)` (run) and
    `add_local_dir(copy=True)` (deploy).
  - `.gitignore`: `.modal-rust/` (generated shims + generated runner),
    `target/`, scratch/`tmp/` — generated shims and scratch are disposable,
    regenerable artifacts and must never be a committed source of truth.
- It records that a future `.modalrustignore` is the user-facing override for the
  mount/copy ignore set (mirroring `.dockerignore`), distinct from `.gitignore`.
- It records that `ignore=` is a predicate `Path->bool` OR a Sequence of
  dockerignore-syntax patterns (converted to a `FilePatternMatcher`).

Evidence:
- File path: `boundaries.md`, "Ignore rules" section (traces to
  `research-synthesis.md` §2.4 ignore, §1.1; AGENTS.md git/secrets rules).
- `grep -nE "\.modalrustignore|\.gitignore|\.modal-rust" workpads/architecture/boundaries.md`
  shows both ignore surfaces.
- `grep -nE "target|\.git|FilePatternMatcher|dockerignore" workpads/architecture/boundaries.md`
  shows the excluded paths and the `ignore=` semantics.

## A8 - Architecture gate review

Status: pending

Acceptance:
- A boundary/contract review confirms `boundaries.md` records, with no
  contradiction of `research-synthesis.md` or the design stances: the workspace
  + crate layout (A0), the runner protocol (A1), the Registry/typed!/HandlerFn
  static-dispatch API (A2), the run-vs-deploy boundary (A3), the three shims (A4),
  the CLI surface
  (A5), the Cargo-cache design (A6), and the ignore rules (A7).
- The user-sensitive decisions are explicitly called out in `boundaries.md` (or
  `knowledge.md` Open Questions) with the synthesis's recommended defaults:
  GPU/cost confirmation, public deploys/auth, default `call` invoke mode, wire
  format, the `rust:1.83-slim` + `add_python="3.12"` toolchain pin, and cache
  sharing/concurrency.
- Failure modes are recorded for each contract (the five error kinds; the
  cascading-rebuild / cold-start / mount-writability / build-time-egress residual
  risks), and `knowledge.md` Gate Status is flipped to reflect the gate decision.
- No statement in the three workpad files contradicts the synthesis or the design
  stances; `knowledge.md` and `references.md` are seeded from the synthesis.

Evidence:
- Boundary review notes recorded in `knowledge.md` (Decisions/Findings), with any
  rejected feedback noted per `WORKING.md`.
- `grep -nE "GPU|public deploy|use-modal-rs|wire format|rust:1.83|cache sharing" workpads/architecture/boundaries.md`
  (or `knowledge.md` Open Questions) shows the user-sensitive decisions called out.
- `grep -n "Not passed yet" workpads/architecture/knowledge.md` (or the updated
  Gate Status line) reflects the gate decision.
- Cross-check: every section heading required by the Gate above is present in
  `boundaries.md`.
