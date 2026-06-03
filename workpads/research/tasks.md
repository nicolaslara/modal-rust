# Research Tasks

## Objective

Validate the riskiest assumptions in `project.md` with sourced findings plus tiny,
authorized Modal spikes — enough to commit to the architecture. The research
dimensions are: (R1) Modal image semantics (`copy=False` vs `copy=True` +
`run_commands`), (R2) THE central empirical question — can a normal
`@app.function` (no Sandbox) compile Rust at runtime, (R3) `copy=False` mount
speed/reliability for dev iteration, (R4) Cargo-cache persistence across
invocations via a Volume, (R5) the `modal-rs` capability matrix (does it deploy/
invoke Functions, or only sandboxes?), (R6) GPU/CUDA facts (driver, `nvidia-smi`,
gpu types, toolkit-vs-driver split), and (R7) a PyO3/maturin assessment (defer to
ergonomics). The design stances: the build boundary is the hard, non-negotiable
invariant (`run` builds at function-execution time, `deploy` builds at image-build
time and the deployed runtime never runs `cargo` — whether the build runs in a
Function body or a Sandbox); direct-execution-first with a Sandbox fallback (try a
normal `@app.function` first, iterate to a Modal Sandbox and record it if a
Function-body build proves infeasible for a step — Sandboxes are a documented
fallback, not banned); and prefer static dispatch. Spikes that make real Modal
calls are
authorized here (per `AGENTS.md` Current Phase) — keep them small and record the
exact command + result.

## Gate

The research gate passes when `workpads/research/knowledge.md` records, with
evidence (a primary Modal/crate source or a small spike command + result), enough
verified findings to commit to the architecture per `WORKING.md` Workpad Gates #1:
whether runtime compile works on a normal Function (R2 — the central claim),
whether `copy=False` mount (R3) + a Cargo cache (R4) make dev iteration tolerable,
how much deploy/invoke surface `modal-rs` exposes vs needing generated Python (R5),
and the GPU/CUDA facts (R6). Each finding separates proven fact from assumption,
cites a dated source, and — where the question is empirical — names the exact spike
command and its result, with open questions flagged for the architecture phase.
The synthesis in `workpads/architecture/research-synthesis.md` is the authoritative
consolidation of this gate's output; this workpad records the per-dimension
research and the spike contracts that feed it.

## R0 - Capture source prompt as the research baseline

Status: completed

Acceptance:
- The design discussion from the initial user prompt (2026-06-03) is captured as
  the project baseline: goal, the design stances (build boundary as the hard
  invariant; direct-execution-first with a Sandbox fallback; prefer static
  dispatch), the runtime contract, the intended crate layout, the phase plan, and
  the riskiest loose ends.
- The captured baseline names the research dimensions to validate (runtime compile,
  `copy=False` mount, Cargo cache, `modal-rs` surface, GPU/CUDA, PyO3/maturin defer).
- No statement contradicts the build-boundary invariant (the hard, non-negotiable
  stance).

Evidence:
- File path: `/Users/nicolas/devel/modal-rust/project.md` (Goal, Source Of Truth,
  Product Thesis, Validation-First Philosophy, Riskiest Loose Ends).
- `project.md` "Validation-First Philosophy" enumerates the five risky assumptions;
  `project.md` "Phases" maps Research -> `workpads/research/`.

## R1 - Modal image semantics: copy=False / copy=True + run_commands

Status: completed

Acceptance:
- It is recorded, with a primary Modal doc, that `add_local_dir(local, remote, *,
  copy=False, ignore=[])` defaults `copy=False`; that `copy=False` adds files at
  container startup (NOT an image layer, no later build steps), and `copy=True`
  bakes a build-time layer (required for a later `run_commands` to see the files).
- It is recorded that `run_commands(*cmds, ...)` runs shell at image-build time,
  each call a separate layer, with build-time network egress (docs show
  `git clone` / `apt`), and that per-layer caching cascades (frequently-changing
  layers go last).
- It is recorded that a non-Python base (`rust:slim`) is a valid Function image
  only via `add_python=` (3.11/3.12 documented by example), must be linux/amd64
  with python+pip on `$PATH`, and that a base ENTRYPOINT must exec its args
  (`Image.entrypoint([])` neutralizes it).
- The run-vs-deploy mapping is explicit: `copy=False` is the `run` (dev) mount;
  `copy=True` + `run_commands(cargo build)` is the `deploy` (image-build) path.

Evidence:
- `knowledge.md` Findings cite modal.com/docs/reference/modal.Image,
  modal.com/docs/guide/images, modal.com/docs/guide/custom-container,
  modal.com/docs/guide/existing-images, modal.com/docs/guide/modal-1-0-migration
  (all dated 2026-06-03 in `references.md`).
- `research-synthesis.md` §1.1 (Images table) is the consolidated source.

## R2 - KEY spike: runtime compile in a normal Function body (direct-execution-first; Sandbox is a documented fallback)

Status: completed

Acceptance:
- The central claim is validated in principle and recorded: a normal
  `@app.function` body can run subprocesses (Modal's own `subprocess.run`
  examples), the Python requirement is satisfiable on a Rust base via `add_python`,
  the container filesystem is writable, and timeouts reach 24 h — so a function
  body can `cargo build` mounted source and exec the freshly built binary on the
  direct-execution happy path (a normal Function, no Sandbox).
- The authorized spike is named with its exact command and expected result: a
  small live Modal Function whose body runs `cargo build` (or, as the M0 local
  precursor, builds and execs `modal_runner` locally), proving the
  `name -> build -> exec -> JSON envelope` path. The spike command + result are
  recorded; if the live build is deferred to the prototype's M4, that deferral is
  recorded with the in-principle evidence that justifies it.
- The single biggest UNVERIFIED assumption is flagged: whether
  `add_local_dir(copy=False)` mounts are writable in place (the M2 write-probe
  gates the prototype's M4 build location). The mitigation is recorded: build into
  a known-writable local path (`CARGO_TARGET_DIR=/tmp/target`); `cp -a /src
  /tmp/build` if read-only.
- Direct-execution-first is the happy path (a normal `@app.function`, no Sandbox);
  a Sandbox is a documented fallback, not banned. If the Function-body build proves
  infeasible for a step, the prototype's M4 iterates to a Modal Sandbox-based build
  and records that decision rather than declaring failure. The build boundary
  (the hard invariant) holds whether the build runs in a Function body or a Sandbox.

Evidence:
- `knowledge.md` Findings: "Feasibility of runtime compile CONFIRMED in principle"
  citing modal.com/docs/guide/cuda (subprocess example) and
  modal.com/docs/guide/existing-images (`add_python`).
- Spike contract (the authorized live spike) — exact command and expected output:
  - `modal run /Users/nicolas/devel/modal-rust/workpads/prototype/dev_app.py::run_add --input '{"a":40,"b":2}'`
    -> a SINGLE `modal run` log showing BOTH `cargo` build output AND
    `{"ok":true,"value":{"sum":42}}` (compile at exec time, no Sandbox).
  - Local M0 precursor (no network):
    `…/target/debug/modal_runner --entrypoint add --input-json '{"a":40,"b":2}'`
    -> `{"ok":true,"value":{"sum":42}}`, exit 0.
- `research-synthesis.md` §1.2 (Functions at runtime) + §3 M4 (RUNTIME COMPILE
  without Sandbox) is the consolidated source; §5 Residual Risk #1 (runtime-compile)
  and #2 (mount writability) record the open edges.

## R3 - copy=False mount speed/reliability for dev iteration

Status: completed

Acceptance:
- It is recorded that `copy=False` re-uploads current local source at each run
  (mount-at-startup), so a local source edit is reflected on the next `run` with no
  redeploy, no image rebuild, and no manual cache busting — the dev-loop reactivity
  property.
- The empirical edges are flagged as spike questions: mount wall-clock + approx
  uploaded bytes (recorded as an EARLY SIGNAL, not a gate), `ignore=` patterns
  applied client-side (so `target/`/`.git` are excluded), content byte-identity
  (remote `sha256` == local `sha256`), and mount writability (shared with R2).
- The recommended `ignore=` set for the Rust mount is recorded
  (`["target",".git",".modal-rust","**/*.rlib"]`).

Evidence:
- `knowledge.md` Findings: `copy=False` mounts at startup; `ignore=` is client-side
  (predicate or dockerignore-syntax patterns). Cites
  modal.com/docs/reference/modal.Image, modal.com/docs/guide/images.
- Spike contract — exact commands and expected results:
  - `shasum -a 256 /Users/nicolas/devel/modal-rust/examples/add/Cargo.toml`
    then `modal run …/dev_app.py::mount_probe` -> remote `find /workspace
    -maxdepth 2` lists the tree with `target/`/`.git` ABSENT; remote sha256 ==
    local sha256; write-probe (`touch /workspace/.write_probe`) records writable
    vs EROFS; wall-clock + uploaded bytes recorded as an early signal.
  - Source-edit reactivity: three consecutive runs returning 42 / 43 / 42 after a
    local edit + revert, no `modal deploy` between them.
- `research-synthesis.md` §1.1 (`ignore=`, `copy=False`) + §3 M2/M5 is the
  consolidated source.

## R4 - Cargo-cache persistence across invocations via a Volume

Status: completed

Acceptance:
- It is recorded that `Volume.from_name(name, create_if_missing=True)` mounted via
  `@app.function(volumes={...})` persists data across invocations; that writes are
  durable only after a commit, with automatic background commits "every few
  seconds" + a final commit (explicit `vol.commit()` often unnecessary); and that
  `vol.reload()` fails "volume busy" if files are open, so it must NOT run on the
  hot build path.
- The first-class caching pattern is recorded: point a tool's cache env var at a
  Volume path (Modal's `HF_HUB_CACHE`/`HF_HOME` examples), directly analogous to
  `CARGO_HOME` (registry index + downloads) and `CARGO_TARGET_DIR` (artifacts).
  Cargo's single-writer-per-target-dir assumption and the
  stable-path+toolchain+rustflags requirement for incremental reuse are recorded.
- The OPEN question is flagged as the M6 benchmark: whether a network-FS target dir
  actually speeds warm rebuilds (Cargo's many small stat/read ops may erase it) —
  a null result is acceptable and does NOT block deploy. The default build location
  stays local-writable (`/tmp/target`); `CARGO_HOME` on a Volume is lower risk and
  may sit there earlier; promoting `CARGO_TARGET_DIR` to a Volume requires the
  benchmark to show net-positive + lock-safe.
- Cache is best-effort (a miss costs time, never a wrong result) and is NOT a
  dependency of deploy.

Evidence:
- `knowledge.md` Findings: `Volume.from_name(create_if_missing=True)`; background +
  final commits; `vol.reload()` "volume busy"; `CARGO_HOME`/`CARGO_TARGET_DIR`
  cache pattern; single-writer caveat; warm-rebuild speedup unverified. Cites
  modal.com/docs/reference/modal.Volume, modal.com/docs/guide/volumes,
  modal.com/docs/examples/*, doc.rust-lang.org/cargo/*.
- Spike contract — exact commands and expected results:
  - `modal volume create modal-rust-cargo-cache`
  - `modal run …/dev_app.py::run_add_cached --input '{"a":40,"b":2}'` (×2)
    -> two wall-clocks (cold empty-volume vs warm second-run); warm is meaningfully
    faster OR the null result is recorded as the deliverable; both runs return 42.
  - `modal volume list` + documented reset (`modal volume rm` / new name).
- `research-synthesis.md` §1.3 (Volumes) + §3 M6 + §5 Residual Risk #4 is the
  consolidated source.

## R5 - modal-rs surface-area capability matrix

Status: completed

Acceptance:
- It is recorded, from extracted `modal-rs` 0.1.3 source, which Modal capabilities
  the crate exposes: app lifecycle, `FunctionCreate`/`FunctionGet`, image build,
  Mount/Volume/Secret, sandboxes (most complete), and `.remote()`/`.spawn()`/
  `.map()` invocation (webhook fns rejected from `.remote()`). So it CAN deploy and
  invoke Functions, not only sandboxes.
- The CRITICAL limitation is recorded: `FunctionCreate` requires a serialized
  **Python** callable (`function_serialized`) + an `image_id`; there is no
  Rust-native function-body concept, so a deployed Modal Function ALWAYS needs a
  Python (or Modal-runtime-compatible) entrypoint — modal-rs does NOT remove the
  Python shim. The wire-format caveat is recorded: CBOR (when advertised) else
  Pickle; modal-rs's serde-pickle emits protocol 2/3 vs Modal Python's cloudpickle
  protocol 4.
- The crate's status is recorded: unofficial, single-maintainer (`thehumanworks`),
  pre-1.0 (crates.io 0.1.3, 2026-03-09, ~200 downloads), gRPC/tonic over TLS,
  vendors `api.proto`, reads `~/.modal.toml`/`MODAL_*`, exposes `inner_mut()`.
- The architecture-shaping conclusion is recorded: v0 authoring/build uses
  generated Python + the official `modal` CLI (known-good control path); modal-rs
  is confined to the `call` invocation behind a validated `--use-modal-rs` flag;
  vendor the proto if adopted deeper.

Evidence:
- `knowledge.md` Findings: modal-rs capability matrix + the `FunctionCreate`
  Python-callable limitation + pickle-protocol caveat. Cites
  crates.io/api/v1/crates/modal-rs, docs.rs/modal-rs, and extracted 0.1.3 source
  (`function_authoring.rs`, `function.rs`, `pickle.rs`).
- `research-synthesis.md` §1.5 (modal-rs surface) + §2.7 (resolution) + §5
  Residual Risk #5 is the consolidated source.

## R6 - GPU / CUDA facts (driver, nvidia-smi, gpu types, toolkit-vs-driver)

Status: completed

Acceptance:
- It is recorded that `gpu=` takes a string with an optional count suffix
  (`"H100:8"`) and a fallback list (`gpu=["H100","A100-40GB:2"]`); that the family
  catalog (T4/L4/A10/L40S/A100/H100/H200/B200/RTX-PRO-6000) and per-type max counts
  DRIFT, so `gpu=` strings are passed through verbatim and Modal's error surfaced
  (the catalog is NOT re-implemented).
- The driver-vs-toolkit split is recorded: GPU machines preinstall the NVIDIA
  driver + CUDA Driver API (`libcuda`) + `nvidia-smi` (Tier 0), with observed
  point-in-time driver 580.95.05 / Driver API 13.0 (WILL drift, never hardcode);
  the CUDA Toolkit (`libcudart`, `nvcc`, `libnvrtc`) is NOT preinstalled and is
  added via `nvidia/cuda:*-runtime-*`/`*-devel-*` (+ `add_python`) or pip
  `nvidia-cuda-runtime-cu12`/`nvidia-cuda-nvrtc-cu12`, keeping toolkit major ≤ host.
- The Rust-crate tiering is recorded: cudarc 0.19.x `dynamic-loading` links with NO
  CUDA at build time and dlopens at runtime (driver-API PTX/cubin needs only
  `libcuda` = Tier 0; runtime NVRTC needs `libnvrtc` = Tier 1); Burn (burn-cuda ->
  cubecl -> cudarc) JIT-compiles via NVRTC at runtime = Tier 1; Rust-CUDA/
  `rustc_codegen_nvvm` is experimental and OUT OF SCOPE for v0. The GPU footgun is
  flagged: dynamic-loading hides a missing `libnvrtc` until runtime — a startup
  self-check must dlopen the required libs and fail loudly.
- The first GPU proof is kept Burn-free (nvidia-smi from Python -> from Rust ->
  cudarc precompiled-PTX vector add -> Burn last).

Evidence:
- `knowledge.md` Findings: GPU driver+Driver API+`nvidia-smi` preinstalled (Tier 0);
  Toolkit not preinstalled; cudarc dynamic-loading; Burn/cubecl NVRTC (Tier 1);
  catalog drift. Cites modal.com/docs/guide/gpu, modal.com/docs/guide/cuda,
  github.com/coreylowman/cudarc, docs.rs/cudarc, lib.rs/crates/burn-cuda,
  github.com/tracel-ai/burn, rust-gpu.github.io.
- Spike contract — exact command and expected result (Tier 0 sanity, cheap before
  Rust/CUDA):
  - `modal run /Users/nicolas/devel/modal-rust/workpads/gpu-compute/gpu_app.py::smi_py`
    -> remote `nvidia-smi` output showing a GPU + driver + CUDA Driver API version;
    no toolkit installed; GPU type + cost recorded.
- `research-synthesis.md` §1.4 (GPU+CUDA) + §2.8 (tiering) + §3 M10-M13 + §5
  Residual Risk #6 is the consolidated source.

## R7 - PyO3 / maturin assessment (defer to ergonomics)

Status: deferred

Acceptance:
- It is recorded that PyO3 (native Python modules from Rust) and maturin (build/
  package PyO3 wheels) are a LATER tighter bridge that would replace the subprocess
  runner, and that they are NOT a v0 dependency: the v0 control path is generated
  Python + a Rust subprocess runner, validated first.
- The deferral rationale is recorded: per the source prompt and `WORKING.md`
  ("Do not add ergonomics (macros, PyO3) before the manual subprocess path works
  end to end"), a PyO3/maturin assessment belongs to the `ergonomics` phase and
  must not change the runner protocol.
- The concurrency caveat that a future PyO3 in-process host raises is flagged: the
  v0 panic-capture uses a process-global slot and the process exits after one call,
  so a concurrent in-process host (PyO3 "Mode B") must revisit per-call panic
  routing before enabling concurrency.

Evidence:
- File paths: `project.md` "Stack Direction" (PyO3/maturin as a later bridge) and
  `WORKING.md` "Validate One Boundary At A Time" (no PyO3 before the manual path
  works). `references.md` rows for pyo3.rs and docs.rs/maturin (dated 2026-06-03).
- `research-synthesis.md` §2.3 "Concurrency caveat (recorded)" flags the PyO3
  Mode B panic-routing hazard. This task is intentionally `deferred` to the
  `ergonomics` workpad and does not gate the research gate.
