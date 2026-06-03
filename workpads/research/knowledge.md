# Research Knowledge

## Objective

Record the validated answers to the riskiest assumptions in `project.md`, with the
fact-vs-assumption split and confidence the architecture gate needs: whether a
normal Modal Function can compile Rust at runtime (the central claim), whether
`copy=False` mount + a Cargo cache make dev iteration tolerable, how much deploy/
invoke surface `modal-rs` exposes vs needing generated Python, and the GPU/CUDA
facts. The consolidated, adversarially-reviewed output of this research lives in
`workpads/architecture/research-synthesis.md` (the authoritative single source for
the architecture gate); this file holds the per-dimension findings and the spike
contracts that feed it. Design stances: the build boundary is the hard,
non-negotiable invariant — `run` builds at function-execution time, `deploy` builds
at image-build time and the deployed runtime never runs `cargo`, whether the build
runs in a Function body or a Sandbox; direct-execution-first with a Sandbox fallback
(prove the core path on a normal `@app.function`, iterate to a Modal Sandbox and
record it if a Function-body build proves infeasible for a step — Sandboxes are a
documented fallback, not banned); and prefer static dispatch.

## Gate Status

Not passed yet.

The research gate passes when this file records, with evidence (a primary source or
a small spike command + result), enough verified findings to commit to the
architecture per `WORKING.md` Workpad Gates #1: runtime compile on a normal Function
(R2), `copy=False` mount (R3) + Cargo cache (R4) dev-iteration tolerability,
`modal-rs` deploy/invoke surface (R5), and the GPU/CUDA facts (R6). The findings
below are seeded from `research-synthesis.md` §1 (Verified Facts) at high confidence
on the documented points; the empirical points (mount writability, warm-rebuild
speedup) remain OPEN and are deferred to live prototype spikes (M2, M4, M6) that
this workpad has specified but not yet executed. R0 (source-prompt capture) is
complete; R1/R6 are documented; R2-R5 are documented with their authorized spike
contracts named; R7 (PyO3/maturin) is deferred to the `ergonomics` phase and does
not gate research.

## Decisions

Seeded from `research-synthesis.md` §2 (locked decisions) and the source prompt.
These are the research-phase conclusions that shape the architecture; items marked
**[CHANGED]** folded in a HIGH-severity review `must_fix`.

- **Direct-execution-first; a Sandbox is a documented fallback.** Validate the
  central claim on a NORMAL `@app.function` first; the whole `run`/`deploy`/`call`
  path is proven on ordinary Modal Functions on the happy path. If a Function-body
  build proves infeasible for a step, iterate to a Modal Sandbox-based build for that
  step and record the decision — Sandboxes are on the table, not banned. (project.md
  design stance 1, R2)
- **The build boundary is the product (the hard, non-negotiable invariant).** `run` =
  `add_local_dir(copy=False)` + `cargo build` in the function body (build at
  function-execution time); `deploy` = `add_local_dir(copy=True)` +
  `run_commands(cargo build)` at image-build time, and the deployed runtime execs
  ONLY a prebuilt binary, never `cargo`. This holds whether the build runs in a
  Function body or a Sandbox. (design stance 2, R1)
- **Prefer static dispatch.** The handler registry uses monomorphized `fn`-pointer
  wrappers, not `Box<dyn>` trait objects: `type HandlerFn = fn(&[u8]) -> Result<Vec<u8>,
  RunnerError>;`, `Registry = BTreeMap<&'static str, HandlerFn>`, `typed!(f)` a
  `macro_rules!` yielding a monomorphized wrapper fn pointer (no dyn, no vtable, no
  Box); the future proc-macro emits the same wrapper + an `inventory` registration
  (or a static `match` table); `typed_async!` reserved with the same fn-pointer
  shape. (project.md design stance 3, §0.3 / boundaries.md §3)
- **v0 authoring/build uses generated Python + the official `modal` CLI** (the
  known-good control path), NOT modal-rs. modal-rs is unofficial/pre-1.0 and
  `FunctionCreate` still needs a serialized Python callable, so it does NOT remove
  the Python shim; it is confined to the `call` invocation behind a validated
  `--use-modal-rs` flag. (R5, §2.7)
- **[CHANGED — Modal #2] The `run` build location defaults to a known-writable
  LOCAL path** (`CARGO_TARGET_DIR=/tmp/target`), NOT a Volume. The Cargo cache is a
  best-effort dev-iteration speedup (a miss costs time, never a wrong result) and is
  NOT a dependency of deploy; promoting `CARGO_TARGET_DIR` to a Volume requires the
  M6 benchmark to show net-positive + lock-safe. (R4, §2.4, §3 M6)
- **[CHANGED — Modal MED] Read-only-mount recipe** if the M2 write-probe shows
  `/src` is read-only: mount read-only -> `cp -a /src /tmp/build` -> build with
  `CARGO_TARGET_DIR` on a writable path. This sidesteps the single biggest
  unverified assumption (mount writability). (R2/R3, §2.4)
- **Keep the first GPU proof Burn-free.** Sequence: `nvidia-smi` from Python ->
  `nvidia-smi` from Rust -> cudarc precompiled-PTX vector add (Tier 0, `libcuda`
  only) -> Burn tensor smoke last (Tier 1, NVRTC). cudarc is pinned with
  `dynamic-loading` (links with no CUDA at build time); a startup self-check
  dlopens the required libs and fails loudly. Rust-CUDA/`rustc_codegen_nvvm` is out
  of scope for v0. (R6, §2.8)
- **`gpu=` is passed through verbatim** (incl. `"H100:8"` and fallback lists); the
  drifting catalog is NOT re-implemented — Modal's error is surfaced. Driver/CUDA
  versions drift and cap the max toolkit major; never hardcode the point-in-time
  driver version. (R6, §2.8)
- **PyO3 / maturin are deferred to the `ergonomics` phase** (R7) — a later tighter
  bridge that replaces the subprocess runner, not a v0 dependency, and must not
  change the runner protocol. (project.md Stack Direction, WORKING.md)
- **Pinned defaults (recommended, user-confirmable):** single image
  `from_registry("rust:1.83-slim", add_python="3.12")` backing both run and deploy
  (3.12 is the only doc-by-example value); `timeout=1800` on the run path (300 s
  default is too low for a cold full compile). (§2.4, §4.5)

## Findings

Seeded from `research-synthesis.md` §1 (Verified Facts). Confidence as noted there:
`high` = primary Modal doc or extracted crate source; `medium` = doc-by-example or
inference. Point-in-time facts (driver versions, GPU catalog, modal-rs version) WILL
drift — re-verify before pinning.

### R1 — Modal image semantics

- `add_local_file(local, remote, *, copy=False)` and `add_local_dir(local, remote,
  *, copy=False, ignore=[])`; `copy` defaults to `False`. (high)
- `copy=False`: files are added to the container AT STARTUP, not baked into an image
  layer; "you can't run additional build steps after" — enables fast redeploy.
  `copy=True`: files copied into a build-time image LAYER (Docker `COPY`-like),
  required if a later `run_commands` must see them; slows iteration. (high)
- `run_commands(*cmds, env=, secrets=, volumes=, gpu=, force_build=False)` runs
  shell at image-build time; each call is a separate Docker-RUN-like layer.
  Build-time network egress is verified by docs (`git clone`, `apt`). Layer cache
  cascades — breaking one layer rebuilds all later ones, so frequently-changing
  layers go last. (high)
- `ignore=` is a predicate `Path->bool` OR a Sequence of dockerignore-syntax
  patterns. Recommended Rust mount set: `["target",".git",".modal-rust",
  "**/*.rlib"]`. (high)
- A non-Python base (ubuntu, nvidia/cuda, `rust:`) is a valid Function image only
  via `from_registry(..., add_python=...)` (3.11/3.12 documented by example); it
  must be linux/amd64 with python+pip on `$PATH`, and a base ENTRYPOINT must end by
  exec-ing its args (`exec "$@"`) — `Image.entrypoint([])` neutralizes it. Modal 1.0
  deprecated `copy_local_*` for `add_local_*` (new default `copy=False`). (high)

### R2 — Runtime compile in a normal Function (the central claim)

- **CONFIRMED in principle:** a normal `@app.function` body can run subprocesses
  (Modal's own `subprocess.run(['nvidia-smi'])` examples); the Python requirement is
  satisfiable on a Rust base via `add_python`; the container filesystem is writable;
  timeouts reach 24 h — so the body can `cargo build` mounted source and exec the
  freshly built `modal_runner` on the direct-execution happy path (a normal
  Function, no Sandbox). (high)
- **Fallback (not a ban):** if the Function-body build proves infeasible for a step,
  the prototype's M4 iterates to a Modal Sandbox-based build for that step and records
  the decision rather than declaring failure. The build boundary (the hard invariant)
  holds whether the build runs in a Function body or a Sandbox. (decision)
- Container filesystem is writable; default per-container disk 512 GiB, up to 3 TiB
  via `ephemeral_disk`; `/tmp` guaranteed writable. (high)
- Function timeout defaults to 300 s, settable 1 s-24 h via `timeout=`, per-attempt;
  retries reset it. Plan `timeout=1800` for cold compile. (high)
- **OPEN / unverified (the single biggest assumption):** whether
  `add_local_dir(copy=False)` mounts are read-only or writable in place. Gated by
  the M2 write-probe; mitigated by building into a known-writable local path and
  copying `/src` to scratch if read-only. (open)
- **OPEN / unverified:** whether `add_python`'s standalone Python coexists cleanly
  on `$PATH` with a system `python3` in `rust:slim` (M3 asserts `which -a python
  python3` + the resolved path). (open)

### R3 — copy=False mount for dev iteration

- `copy=False` re-uploads current local source at each run (mount-at-startup), so a
  local edit is reflected on the next `run` with no redeploy, no image rebuild, and
  no manual cache busting — the dev-loop reactivity property. (high)
- `ignore=` patterns are applied client-side, so `target/`/`.git` never upload.
  (high)
- **OPEN / early-signal (not a gate):** mount wall-clock + approx uploaded bytes,
  and byte-identity (remote `sha256` == local `sha256`) — measured by the M2 spike,
  recorded as an early signal. (open)

### R4 — Cargo-cache persistence via a Volume

- `Volume.from_name(name, *, create_if_missing=False, ...)`; idiomatic
  `create_if_missing=True`. Mounted via `@app.function(volumes={"/path": vol})` on
  normal Functions. (high)
- Writes are durable only after a commit; Modal does automatic background commits
  "every few seconds" + a final commit on clean shutdown (explicit `vol.commit()`
  often unnecessary). `vol.reload()` fetches latest committed state but FAILS
  "volume busy" if files are open — avoid on the hot build path (Cargo holds lock
  files). (high)
- Pointing a tool's cache env var at a Volume path is the first-class Modal caching
  pattern (Modal's `HF_HUB_CACHE`/`HF_HOME` examples), directly analogous to
  `CARGO_HOME` (registry index + downloads) and `CARGO_TARGET_DIR` (artifacts).
  Cargo incremental reuse needs a stable mount path + stable toolchain + stable
  rustflags; Cargo assumes a SINGLE writer per target dir (local advisory locks).
  (high; single-writer caveat medium)
- Concurrency: v1 = last-write-wins per file, avoid >~5 concurrent commits, no
  distributed file locking; v2 allows concurrent writes to DISTINCT files, same-file
  still last-write-wins. (high)
- **OPEN / unverified on Modal (the M6 benchmark):** whether a network-FS target dir
  actually speeds warm rebuilds (Cargo's many small stat/read ops may erase the
  speedup); atomicity of background commits at partial-file level. A null result is
  acceptable and does NOT block deploy. (open)

### R5 — modal-rs surface-area capability matrix

- `modal-rs` (crates.io 0.1.3, 2026-03-09) is UNOFFICIAL, single-maintainer
  (`thehumanworks`), pre-1.0, ~200 downloads; gRPC/tonic over TLS; vendors Modal's
  `api.proto`; exposes an `inner_mut()` raw escape hatch; reads `~/.modal.toml`/
  `MODAL_*` like the official SDKs. (high)
- It exposes app lifecycle, **FunctionCreate/FunctionGet**, image build, Mount/
  Volume/Secret, and sandboxes (most complete), plus `.remote()`/`.spawn()`/
  `.map()` invocation (webhook fns rejected from `.remote()`). So it CAN deploy and
  invoke Functions, not only sandboxes. (high)
- **CRITICAL:** `FunctionCreate` requires `function_serialized` (a serialized
  **Python** callable) + an `image_id`. There is no Rust-native function-body
  concept — the deployed unit is still a Python-defined function, so a deployed
  Modal Function ALWAYS needs a Python (or Modal-runtime-compatible) entrypoint.
  modal-rs does NOT remove the Python shim. (high)
- Wire format: CBOR (when the function's metadata advertises it) else Pickle.
  modal-rs's `serde-pickle` emits protocol 2/3, but Modal Python uses cloudpickle
  protocol 4 — a compat caveat for non-trivial types (scalar `str` round-trips are
  safe). (high)
- **Architecture-shaping conclusion:** v0 uses generated Python + the official
  `modal` CLI for authoring/build; modal-rs is confined to `call` behind
  `--use-modal-rs`; vendor the proto if adopted deeper. (R5 -> §2.7)

### R6 — GPU / CUDA facts

- `gpu=` takes a string with an optional count suffix (`"H100:8"`) and a fallback
  list (`gpu=["H100","A100-40GB:2"]`). Families: T4, L4, A10, L40S, A100(40/80GB),
  H100, H200, B200, RTX-PRO-6000. Catalog + per-type max counts DRIFT — pass strings
  through, surface Modal's error. (high; catalog medium)
- GPU machines preinstall the NVIDIA driver + CUDA Driver API (`libcuda.so`) +
  `nvidia-smi` (Tier 0). Observed point-in-time: driver 580.95.05 / Driver API 13.0
  — WILL drift, never hardcode. (high; versions point-in-time)
- The CUDA Toolkit (`libcudart`, `nvcc`, `libnvrtc`) is NOT preinstalled. Add via
  `nvidia/cuda:*-runtime-*`/`*-devel-*` (+ `add_python`) or pip
  `nvidia-cuda-runtime-cu12`/`nvidia-cuda-nvrtc-cu12`. Keep toolkit major ≤ host
  (12.x/13.x guaranteed compatible). (high)
- cudarc 0.19.x defaults to `dynamic-loading`: links with NO CUDA at build time;
  dlopens libs at runtime (CUDA 11.4-13.0). Driver-API path loading precompiled
  PTX/cubin needs only `libcuda` (Tier 0); runtime NVRTC (`nvrtc::compile_ptx`)
  needs `libnvrtc.so` (Tier 1). (high)
- Burn (`burn-cuda`) -> cubecl -> cudarc; CubeCL JIT-compiles kernels via NVRTC at
  runtime -> needs `libnvrtc` + `libcudart` (Tier 1). README: "Requires CUDA 12.x on
  PATH"; pre-1.0, frequent breaking releases. Rust-CUDA/`rustc_codegen_nvvm` is
  rebooted-but-experimental (pins exact nightlies + LLVM 7) — OUT OF SCOPE for v0.
  (high; Burn churn high)
- **GPU footgun (flagged):** `dynamic-loading` hides a missing `libnvrtc` until
  runtime, so Burn/cubecl on a driver-only image fails at RUNTIME even though cudarc
  compiles fine — a startup self-check must dlopen the required libs and fail
  loudly. (R6 -> §5 Residual Risk #6)

### R7 — PyO3 / maturin (deferred to ergonomics)

- PyO3 (native Python modules from Rust) and maturin (build/package PyO3 wheels) are
  a LATER tighter bridge that would replace the subprocess runner; they are NOT a v0
  dependency. The v0 control path is generated Python + a Rust subprocess runner,
  validated first. (project.md Stack Direction)
- A future PyO3 in-process host ("Mode B") must revisit the v0 panic-capture, which
  uses a process-global slot and assumes the process exits after one call — concurrent
  in-process reuse needs per-call panic routing before enabling concurrency.
  (§2.3 concurrency caveat)

## Open Questions

The empirical edges this research has SPECIFIED but not yet RESOLVED with a live
spike, plus the user-sensitive product decisions. The empirical ones are resolved in
the prototype/GPU phases (M2/M4/M6/M10); a null result on the cache is acceptable.
The product decisions are surfaced with `research-synthesis.md` §4 recommended
defaults and do NOT block committing to the architecture.

- **Mount writability (empirical, gates M4 build location).** Is
  `add_local_dir(copy=False)` writable in place? Resolved by the M2 write-probe;
  mitigated by building into a local-writable path (copy `/src` to scratch if
  read-only). (R2/R3, §5 Residual Risk #2)
- **`add_python` coexistence (empirical, M3).** Does the standalone `add_python`
  interpreter shadow / get shadowed by a system `python3` in `rust:slim`? M3 asserts
  `which -a python python3` + the resolved path. (R2, §5 Residual Risk #9)
- **Warm-rebuild speedup over a network FS (empirical, M6 benchmark).** Does a
  Volume-backed Cargo cache actually beat a cold build, or do Cargo's many small
  stat/reads erase it? A null result is acceptable and must NOT block deploy. (R4,
  §5 Residual Risk #4)
- **Build-time egress on the target account (empirical, M4/M7).** `run_commands`
  egress is verified-by-docs; re-confirm crates.io reachability on this account.
  Fallback: `cargo vendor` hermetic build. (R1/R2, §5 Residual Risk #1)
- **GPU / cost confirmation (product).** Default: require an explicit `--yes` flag
  for `modal-rust run --gpu` and `modal-rust deploy`, with a per-run cost note.
  Override: budget ceiling / disable confirmation. (§4.1)
- **Public deploys / auth (product).** Default: NO web endpoint in v0 — callable
  only via `Function.from_name().remote()`. Options: opt-in authenticated endpoint;
  public (not recommended). (§4.2)
- **Default `call` invoke mode (product).** Default: generated `call_app.py` via
  `modal run` for v0; wire modal-rs `Function::from_name().remote()` behind
  `--use-modal-rs`, promoted to default only after a non-scalar round-trip smoke.
  (§4.3)
- **Wire format (product).** Default: JSON for v0 (the `typed!` wrapper / `HandlerFn`
  are already codec-neutral on `&[u8]`, so CBOR/msgpack + `--input-format` is additive). (§4.4)
- **Cache sharing / concurrency (product).** Default: a single shared
  `modal-rust-cargo-cache` (fine for one developer; matches "avoid >5 concurrent
  commits"). Option: sharded names / Volume v2 for multiple developers. (§4.6)
