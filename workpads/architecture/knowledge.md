# Architecture Knowledge

Decisions, findings, and open questions for the architecture phase (tasks
A0–A8). Seeded from the locked decisions in
[`research-synthesis.md`](./research-synthesis.md) (date 2026-06-03). The
canonical contracts themselves live in `boundaries.md`; this file records the
reasoning, confidence, and what remains user-sensitive. Append as tasks land
contracts; do not contradict the synthesis or the design stances.

## Objective

Capture the locked architectural decisions, the facts they rest on, and the open
product questions, so the prototype and GPU phases can build against stable
contracts. The design stances: the build boundary is the hard, non-negotiable
invariant (`run` builds at function-execution time, `deploy` builds at image-build
time and the deployed runtime never runs `cargo`, whether the build runs in a
Function body or a Sandbox); direct-execution-first with a Sandbox as a documented
fallback (try a normal `@app.function` first; iterate to a Modal Sandbox and record
it only if a step's Function-body build proves infeasible); and prefer static
dispatch.

## Gate Status

**Not passed yet.** The architecture gate passes when `boundaries.md` records the
crate layout (A0), the runner protocol + registry API (A1, A2), the run-vs-deploy
build boundary (A3), the generated shim design (A4), the CLI surface (A5), and
the cache + ignore design (A6, A7) — with the user-sensitive decisions called out
(A8, per `WORKING.md` Workpad Gates #2 and `research-synthesis.md` §4). The locked
decisions below are authoritative inputs; the gate work is writing them into
`boundaries.md` as reviewable contracts (one boundary per task) and seeding this
file + `references.md` from the synthesis.

## Decisions

Seeded from `research-synthesis.md` §2 (locked decisions). Items marked
**[CHANGED]** folded in a HIGH-severity review `must_fix`.

- **Single virtual cargo workspace** at repo root: `crates/modal-rust-runtime`,
  `crates/modal-rust-cli`, `crates/modal-rust-client`, `crates/modal-rust-macros`
  (empty placeholder), plus `examples/add`. Edition 2021 for published crates
  (2024 allowed only in user/example crates). (A0, §2.1)
- **Acyclic deps:** `macros -> runtime`; `client -> runtime`; `cli -> client +
  runtime`; per-user runner binary `-> runtime + the user's lib crate`.
  `modal-rust-runtime` has zero Modal/network/Python deps (serde, serde_json,
  anyhow, tiny arg parser only) because it is recompiled every dev run and baked
  into the deploy image. (A0, §2.1)
- **The user does not own `main()`.** The CLI owns a ~15-line
  `src/bin/modal_runner.rs` template; the user authors only `lib.rs` and
  `pub fn modal_registry() -> Registry`. Runner `main()` is fixed:
  `modal_rust_runtime::run_cli(user_crate::modal_registry())`. (A0, §2.1)
- **Runner uses a hand-rolled 3-flag arg parser; `clap` is CLI-only**, never a
  runtime dependency (resolves the clap-in-runtime review conflict). (A0, §2.1)
- **Runner CLI protocol is the frozen seam.** Binary `modal_runner`; invocation
  `--entrypoint <name> ( --input-json <json> | --input-file <path> |
  --input-stdin )`. stdout carries EXACTLY ONE JSON envelope; all diagnostics go
  to stderr. Exit code mirrors `ok` (0 success / 1 failure); the kind lives in the
  JSON, not the exit code. `--input-file`/`--input-stdin` exist to avoid argv
  limits and the ~100 MB gRPC ceiling. (A1, §2.2)
- **[CHANGED — Rust #3] Five frozen error kinds** (was four):
  `decode_error | unknown_entrypoint | function_error | encode_error | panic`.
  `encode_error` was added so an `Out`-serialization failure is never reported as
  a `panic`. (A1, §2.2)
- **[CHANGED — §0.2] User errors are WRAPPED on the top-level `RunnerError` enum.**
  The five kinds are unchanged; the failure envelope gains an additive optional
  `details` field:
  `{"ok":false,"error":{"kind":"function_error","message":"<Display/anyhow chain>","details":<serialized user error|null>,"backtrace":"..."}}`.
  `function_error` = the user error wrapped on the top-level enum — `message` from
  Display/the anyhow chain, `details = serde_json::to_value(&e).ok()` when the
  handler's error type is `Serialize` (else `null`). (A1, §2.2, §0.2)
- **Frozen precedence:** top-level JSON parse -> entrypoint lookup -> decode `In`
  -> call -> encode `Out`. Malformed JSON + bad entrypoint yields `decode_error`.
  Panic capture: panic hook + `std::backtrace::Backtrace` + `catch_unwind`; the
  shim sets `RUST_BACKTRACE=1`. Future envelope additions must be additive optional
  fields (`details` follows this rule; `meta`/`version` reserved); unknown fields
  are ignored. (A1, §2.2)
- **[CHANGED — §0.3 / Rust #5] Prefer static dispatch; codec-neutral `HandlerFn`
  fn pointer.** `type HandlerFn = fn(&[u8]) -> Result<Vec<u8>, RunnerError>` — a
  bare `fn` pointer, NO `Box<dyn Handler>`, no vtable. `typed!(f)` is a
  `macro_rules!` that generates a monomorphized wrapper `fn` and yields its pointer
  (decode/call/encode inlined for `f`'s concrete `In`/`Out`/`Err`), owning
  decode/encode via a `Codec` (JSON for v0); a future CBOR path only adds a `Codec`
  impl, never touching `HandlerFn` or `Registry`. (A2, §2.3, §0.3)
- **[CHANGED — §0.3 / Rust #4] Async path reserved now with the same fn-pointer
  shape.** `HandlerFn` stays sync; `typed_async!` is committed (may be unimplemented
  in v0) and `block_on`s a runtime-owned Tokio executor inside the same
  `fn(&[u8]) -> Result<Vec<u8>, _>` wrapper. The future macro detects `async fn`
  and expands to `typed_async!`. (A2, §2.3, §0.3)
- **[CHANGED — Rust MED] Duplicate-name rejection** at runner startup
  (`Registry::function()` / `from_inventory()`), not silent last-write-wins. (A2,
  §2.3)
- **[CHANGED — Rust MED] Argument shape frozen as a single JSON object.** Single-
  arg handlers take `In` directly; a future multi-arg macro generates a named-field
  `#[derive(Deserialize)]` args struct. Arguments are a named object, never a
  positional array. (A2, §2.3)
- **Static-dispatch macro-compatibility invariant:** every entry is a monomorphized
  `typed!` wrapper reduced to a bare `fn` pointer (no `Box<dyn>`, no vtable);
  `run_cli`/`Registry` never change shape regardless of registration path.
  `name -> monomorphized typed! wrapper (fn pointer) -> bytes in -> bytes out`. The
  manual registry and the future `inventory`/`#[modal_rust::function]` path converge
  on one dispatch code path (the proc-macro generates the same wrapper + an
  inventory registration, or a static `match` table). `Registry` is a
  `BTreeMap<&'static str, HandlerFn>` (static-str keys, fn-pointer values — no
  allocation, no `dyn`); builder `Registry::new().function("add", typed!(add))`.
  (A2, §2.3, §0.3, project.md)
- **The build boundary is the product (the hard, non-negotiable invariant —
  stance 2).** `run` builds at function-execution time (`add_local_dir(copy=False)`
  + `cargo build` in the function body); `deploy` builds at image-build time
  (`add_local_dir(copy=True)` + `run_commands(cargo build)`, binary baked to
  `/app/modal_runner`); the deployed runtime execs ONLY the binary and NEVER
  invokes cargo. Proof obligation: `cargo build` in deploy/build logs, ABSENT from
  call logs. This run-vs-deploy split holds whether the build runs in a Function
  body or a Sandbox. Per stance 1 (direct-execution-first), the happy path uses a
  normal `@app.function`; a Modal Sandbox is a documented fallback if a
  Function-body build proves infeasible for a step. (A3, §2.4–§2.5, §0.1, AGENTS.md)
- **[CHANGED — §0.1] Direct-execution-first; Sandbox is a documented fallback
  (stance 1).** "No Sandboxes" is NOT a ban. Prove the core path on a normal
  `@app.function` (runtime compile in the Function body, M4) FIRST; if that proves
  infeasible for a step, iterate to a Modal Sandbox for that step and record the
  decision. M4 acceptance gains a fallback branch: if the Function-body build is
  infeasible, evaluate + record a Sandbox-based build rather than declaring failure
  (the run-vs-deploy build boundary is unchanged either way). (A3/A4, §0.1, project.md)
  (`CARGO_TARGET_DIR=/tmp/target`), NOT a Volume. `CARGO_HOME` MAY sit on a Volume
  earlier (lower risk). Promoting `CARGO_TARGET_DIR` to a Volume requires M6 to
  benchmark it net-positive and lock-safe. (A3/A6, §2.4)
- **[CHANGED — Modal MED] Read-only mount recipe:** if the M2 write-probe shows
  `/src` read-only, mount read-only -> `cp -a /src /tmp/build` -> build with
  `CARGO_TARGET_DIR` on a writable path. Sidesteps the unverified mount-writability
  assumption. (A3, §2.4)
- **`dev_app` (run) shim:** `from_registry("rust:{VER}-slim", add_python="3.12")
  .entrypoint([])` + `.env({"RUST_BACKTRACE":"1"})` +
  `.add_local_dir(LOCAL_SRC, "/src", copy=False, ignore=[...])`. CLI flags routed
  via `@app.local_entrypoint()` — a bare `@app.function` does NOT auto-bind `modal
  run` flags (Modal #1). `timeout=1800` (300 s default too low for cold compile).
  Never `vol.reload()` mid-build. (A4, §2.4)
- **`deploy_app` shim (build at IMAGE-BUILD time):** `from_registry(...)
  .entrypoint([]).env(...).add_local_dir(MANIFEST_DIR, "/app", copy=True, ...)
  .run_commands("cd /app && cargo build --release --bin modal_runner")
  .run_commands("cp .../release/modal_runner /app/modal_runner && chmod +x ...")`.
  Deployed body execs ONLY `/app/modal_runner ...`, mounts no source, mounts no
  cache Volume. `add_python` still required (the image must host Modal's Python
  runtime even though the workload is a native binary). (A4, §2.5)
- **[CHANGED — Modal #6] `call_app` routing via `@app.local_entrypoint()`**
  (module-scope `print(fn.remote(...))` would `NameError`); default `call` uses
  generated Python + the official `modal` CLI via `Function.from_name(APP, fn)
  .remote(...)`. modal-rs `Function::from_name().remote()` is confined to `call`
  behind a validated `--use-modal-rs` flag. (A4/A5, §2.5, §2.7)
- **No web endpoint in v0.** The deployed `add` is callable only via
  `Function.from_name().remote()`. Invoked arg + return are plain `str` (the JSON
  envelope text), well under the ~100 MB gRPC limit; large I/O routes via a
  Volume/object storage (out of scope for the `add` POC). (A4, §2.5, §4 Q2)
- **[CHANGED — Rust MED] Build profile must be `panic = "unwind"`** for
  `catch_unwind` to upgrade panics into envelopes. `modal-rust doctor --rust`
  detects `panic = "abort"` and/or the runner is built under a forced-unwind
  profile. M0 asserts the build is not `panic = "abort"`. (A5, §2.6)
- **CLI surface:** `doctor [--rust]`, `run <entrypoint> [--input <json|@file>]
  [--gpu] [--timeout]`, `deploy <entrypoint> [--gpu] [--app-name]`,
  `call <entrypoint> [--input <json|@file>] [--app-name] [--use-modal-rs]`. The
  CLI is a pure wrapper introducing NO new Modal capability. All generated shims
  are disposable artifacts under gitignored `.modal-rust/generated/` and must be
  byte-equivalent (modulo injected params) to the prototype shims. (A5, §2.7)
- **Cargo cache (run-path only, best-effort):**
  `Volume.from_name("modal-rust-cargo-cache", create_if_missing=True)` at a STABLE
  path; `CARGO_HOME` MAY sit on the Volume; `CARGO_TARGET_DIR` stays `/tmp/target`
  by default. Relies on automatic background/shutdown commits; never `vol.reload()`
  on the hot path. A cache miss only costs time, never a wrong result. NOT a
  dependency of deploy (deploy mounts no cache Volume). Single-writer /
  low-concurrency; parallel shared-cache writes out of scope for v0. (A6, §2.4
  cache, §1.3)
- **Ignore rules:** mount/copy `ignore=` excludes `target`, `.git`, `.modal-rust`,
  build artifacts (`**/*.rlib`) on both `copy=False` (run) and `copy=True`
  (deploy), so uploads are minimal/reactive; `.gitignore` keeps `.modal-rust/`
  (generated shims + runner), `target/`, and scratch out of git as disposable
  artifacts. `ignore=` is a predicate `Path->bool` OR dockerignore-syntax patterns
  (a `FilePatternMatcher`). A future `.modalrustignore` is the user-facing override
  for the mount/copy set (mirrors `.dockerignore`), distinct from `.gitignore`.
  (A7, §2.4 ignore, §1.1, AGENTS.md)
- **GPU `gpu=` passed through verbatim** (incl. `"H100:8"` and fallback lists); the
  drifting catalog is NOT re-implemented. Tier 0 (driver-only) -> Tier 1 (+ NVRTC/
  runtime via pip or `nvidia/cuda:*-runtime-*`) -> Tier 2 (`*-devel-*`, only for
  `nvcc`). cudarc pinned with `dynamic-loading`; startup self-check dlopens the
  required libs. Rust-CUDA/`rustc_codegen_nvvm` out of scope for v0. (§2.8)
- **v0 authoring/build uses generated Python + the official `modal` CLI** (known-
  good control path); modal-rs is unofficial/pre-1.0 and `FunctionCreate` still
  needs a Python `function_serialized`, so it does NOT remove the Python shim. (A5,
  §2.7)

## Findings

Seeded from `research-synthesis.md` §1 (verified facts). Confidence as noted there.

- **2026-06-06 — runtime/control-plane registration boundary repaired.**
  `modal-rust-runtime::Registration` is dispatch-only again (`name`, `handler`).
  Macro discovery now submits one facade-owned `modal_rust::Registration` that
  atomically pairs `handler` with `FunctionConfig` and `package`; the facade splits
  that single record into a runtime `Registry` plus control-plane configs. This
  avoids the two-submit footgun while removing Modal vocabulary from the runtime
  crate. Evidence: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  and `cargo test` passed on 2026-06-06.

- **2026-06-06 — per-function option duplication collapsed.** `FunctionConfig`
  remains only as the const-friendly inventory initializer; the facade converts it
  once into owned `FunctionOptions`. `App.configs`, run `RemoteConfig.options`,
  deploy `DeployConfig.options`, `DeployEntrypoint.options`, dry-run, and the CLI
  `--describe` parser now carry that one owned shape. This removes the previous
  `DescribeConfig` / `FunctionConfigView` / `Box::leak` path and stops
  gpu/timeout/cache/secrets/volumes from being re-declared across run and deploy.

- `add_local_dir(local, remote, *, copy=False, ignore=[])` defaults `copy=False`;
  `copy=False` mounts files at container startup (not an image layer, no later
  build steps), `copy=True` bakes a build-time layer (required for later
  `run_commands`). (high) (§1.1)
- `run_commands(*cmds, ...)` runs shell at image-build time, each call a separate
  layer; build-time network egress verified by docs (`git clone`, `apt`). Layer
  caching cascades — frequently-changing layers go last. (high) (§1.1)
- A non-Python base (`rust:slim`) is a valid Function image only via
  `add_python=...` (3.11/3.12 documented by example); the image must be
  linux/amd64 with python+pip on `$PATH` and an ENTRYPOINT that ends by exec-ing
  its args. `Image.entrypoint([])` neutralizes a base ENTRYPOINT. (high) (§1.1)
- **Feasibility of runtime compile CONFIRMED in principle:** a normal
  `@app.function` body can run subprocesses, the Python requirement is satisfiable
  via `add_python`, the filesystem is writable, and timeouts go to 24 h. The single
  biggest UNVERIFIED assumption is whether `copy=False` mounts are writable in
  place (M2 write-probe gates M4). (high / open) (§1.2)
- Function timeout defaults to 300 s, settable 1 s-24 h. `.remote()` args/results
  have a ~100 MB gRPC limit; plain `str` in/out works. (high / medium) (§1.2)
- `Volume.from_name(name, create_if_missing=True)`; writes durable only after a
  commit, with automatic background commits "every few seconds" + a final commit;
  `vol.reload()` fails "volume busy" if files are open (avoid on the build path).
  Pointing a cache env var (`CARGO_HOME`/`CARGO_TARGET_DIR`) at a Volume is the
  first-class Modal caching pattern; Cargo assumes a single writer per target dir.
  Whether a network-FS target dir actually speeds warm rebuilds is unverified
  (M6 benchmarks it; a null result is acceptable and does not block deploy). (§1.3)
- GPU machines preinstall the NVIDIA driver + CUDA Driver API (`libcuda`) +
  `nvidia-smi` (Tier 0); the CUDA Toolkit (`libcudart`, `nvcc`, `libnvrtc`) is NOT
  preinstalled. cudarc 0.19.x `dynamic-loading` links with no CUDA at build time
  and dlopens at runtime; Burn/cubecl JIT-compile via NVRTC at runtime (Tier 1).
  Driver/catalog versions drift — pass `gpu=` strings through, never hardcode. (§1.4)
- `modal run app.py::fn --flag val` auto-binds CLI flags ONLY for
  `@app.local_entrypoint()`, not a bare `@app.function`. Deployed functions are
  invoked via `modal.Function.from_name(app, fn).remote(*args)`. modal-rs (crates.io
  0.1.3) is unofficial/single-maintainer/pre-1.0; `FunctionCreate` needs a
  serialized Python callable; serde-pickle protocol 2/3 vs cloudpickle 4. (§1.5)

## Open Questions

The user-sensitive product decisions from `research-synthesis.md` §4, each with
the synthesis's recommended default. These must be called out in `boundaries.md`
(A8). None block M0-M3.

- **GPU / cost confirmation.** Default: require an explicit `--yes` flag for
  `modal-rust run --gpu` and for `modal-rust deploy`, with a per-run cost note.
  Override: budget ceiling / disable confirmation. (§4.1)
- **Public deploys / auth.** Default: NO web endpoint in v0 — callable only via
  `Function.from_name().remote()`. Options: opt-in authenticated endpoint; public
  (not recommended). (§4.2)
- **Default `call` invoke mode.** Default: generated `call_app.py` via `modal run`
  for v0; wire modal-rs `Function::from_name().remote()` behind `--use-modal-rs`,
  promoted to default only after a non-scalar round-trip smoke. (§4.3)
- **Wire format.** Default: JSON for v0 (the `typed!` wrapper / `HandlerFn` are
  already codec-neutral on `&[u8]`, so CBOR/msgpack + `--input-format` is additive). (§4.4)
- **Rust toolchain pin.** Default: `from_registry("rust:1.83-slim",
  add_python="3.12")` as the single image backing run and deploy (3.12 is the only
  doc-by-example value). (§4.5)
- **Cache sharing / concurrency.** Default: a single shared
  `modal-rust-cargo-cache` (fine for one developer; matches "avoid >5 concurrent
  commits"). Option: sharded names / Volume v2 for multiple developers. (§4.6)
