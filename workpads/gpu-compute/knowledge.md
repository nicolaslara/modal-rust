# GPU Compute Knowledge

Decisions, findings, and open questions for the GPU milestones (M10–M13).
Seeded from the locked decisions in
[`../architecture/research-synthesis.md`](../architecture/research-synthesis.md)
(date 2026-06-03). Append as milestones produce evidence; do not contradict the
synthesis or the design stances (build boundary is the hard invariant;
direct-execution-first with a Sandbox fallback; prefer static dispatch).

## Objective

Prove genuine Rust GPU compute on Modal **Burn-free first**: observe a real GPU
from Python (M10) then from Rust (M11), run a verified cudarc vector-add via
precompiled PTX on a driver-only image (M12), and only then run a Burn tensor
smoke on a CUDA-runtime image (M13). The build path is the prototype's
CPU-proven M4/M7 recipe; the only new variables are `gpu=` placement and the
CUDA tier of the image. Build slowly, one boundary per milestone, recording
evidence (and GPU cost) before depending on it.

## Gate Status

**Not passed yet.** No milestones executed. The gate passes only when this file
records, with evidence: (1) a **verified Rust GPU compute result** — the M12
cudarc vector-add `c[i]=a[i]+b[i]` on a real Modal GPU, checked element-wise
against a CPU reference, on a Tier-0 driver-only image with a precompiled-PTX
kernel; **then** (2) a **Burn tensor smoke** — the M13 minimal Burn CUDA-backend
tensor add on a Tier-1 CUDA-runtime image, verified correct, with
`libnvrtc`/`libcudart` confirmed present and `burn`/`burn-cuda`/`cubecl`/`cudarc`
pinned together. M10/M11 are the prerequisite observation steps; GPU cost is
recorded for every milestone.

## Decisions

Carried from the synthesis (authoritative). Record any gpu-compute-local
refinements below as they are made.

- **Design stances (build boundary is the hard invariant).** (1)
  **Direct-execution-first; Sandbox is a documented fallback** — every GPU step is
  validated on a normal `@app.function` first; if direct Function execution proves
  infeasible for a step, iterate to a Modal Sandbox for that step and record it
  (a fallback on the table, not a ban). (2) **The build boundary is the product**
  (the hard, non-negotiable invariant): `run` builds at function-execution time,
  `deploy` builds at image-build time and the deployed runtime execs only the
  prebuilt binary, never `cargo` — and this holds whether the build runs in a
  Function body or a Sandbox. (3) **Prefer static dispatch.** The GPU milestones
  reuse the prototype's build path unchanged — `gpu=` and the image tier are the
  only new variables.
- **Burn-free-first ordering (do not reorder).** Keep the first GPU proof
  Burn-free: nvidia-smi (Python → Rust) → cudarc precompiled-PTX vector add →
  Burn smoke, last. The lean cudarc/driver-only stack is verified before the
  heavier Burn/cubecl/NVRTC/Tier-1 stack (project.md "Riskiest Loose Ends"; §2.8).
- **`gpu=` is passed through verbatim** — including a count suffix (`"H100:8"`)
  and a fallback list (`gpu=["H100","A100-40GB:2"]`). modal-rust does NOT validate
  the drifting catalog; it surfaces Modal's error. Families seen: T4, L4, A10,
  L40S, A100(40/80GB), H100, H200, B200, RTX-PRO-6000 (catalog drifts) (§2.8).
- **GPU tiering (the load-bearing GPU decision):**
  - **Tier 0** — default `rust:slim`, driver only (`libcuda` + `nvidia-smi`
    preinstalled). Enough for `nvidia-smi` from Rust (M10/M11) and cudarc
    Driver-API execution of **precompiled PTX** (M12).
  - **Tier 1** — Tier 0 + pip `nvidia-cuda-nvrtc-cu12` + `nvidia-cuda-runtime-cu12`
    (or `nvidia/cuda:*-runtime-*` + `add_python`). Adds `libnvrtc`/`libcudart` for
    runtime NVRTC / Burn / cubecl (M13).
  - **Tier 2** — `nvidia/cuda:*-devel-*` + `add_python`. Only when `nvcc` is
    needed (e.g. to generate PTX at build time); not on any runtime path here.
- **cudarc pinned with default `dynamic-loading`** — links with NO CUDA present
  at build time and dlopens the libs at runtime. Keep the container toolkit major
  ≤ the host driver's supported major (12.x/13.x guaranteed compatible). Because
  dynamic-loading hides a missing lib until first use, a **startup self-check must
  dlopen the required libs and fail loudly** (§2.8).
- **M12 kernel ships as precompiled PTX**, not runtime-NVRTC and not a fixed-arch
  cubin: PTX is driver-JIT and forward-compatible, needs only `libcuda` (Tier 0),
  and is checked in or generated at deploy/image-build (Tier 2 builder) — never
  NVRTC-compiled at runtime. This is what lets M12 stay Tier 0.
- **M13 is the first milestone that requires Tier 1.** CubeCL (under burn-cuda)
  JIT-compiles kernels via NVRTC at runtime, so `libnvrtc.so` + `libcudart.so`
  MUST be on the loader path; the burn-cuda README states "Requires CUDA 12.x on
  PATH". A hard `dlopen` self-check guards against an accidentally Tier-0 image.
- **Rust-CUDA / `rustc_codegen_nvvm`** (Rust-authored kernels, rebooted but
  experimental, pins exact nightlies + LLVM 7) is **out of scope for v0** — M12
  uses precompiled PTX, not Rust-authored device code (§1.4, §2.8).
- **Cost confirmation (cost-sensitive — confirm with the user).** GPU runs cost
  real money and deploys create persistent apps. Recommended default (§4 Q1):
  require an explicit `--yes` for `modal-rust run --gpu` and for any persistent
  deploy, with a per-run cost note; record GPU type + observed cost per milestone.
- **Recommended pins (defaults, confirm before locking):** GPU type `T4` (cheapest
  family) for all smoke milestones; `add_python='3.12'`; for M13 a single Tier-1
  recipe (CUDA-runtime tag OR the two pip wheels) with `burn`/`burn-cuda`/`cubecl`/
  `cudarc` versions pinned together and recorded (§4 Q5; Residual Risk #6).

## Findings

Seeded verified facts most load-bearing for the GPU milestones (full table +
confidence in the synthesis §1.4). Append empirical spike results per milestone.

- **GPU machines preinstall the NVIDIA driver + CUDA Driver API (`libcuda.so`) +
  `nvidia-smi`** — "Tier 0" (high). Observed point-in-time: driver 580.95.05 /
  Driver API 13.0 — **will drift; do NOT hardcode**. This is what makes M10/M11
  (nvidia-smi) and M12 (Driver-API PTX execution) possible with no toolkit.
- **The CUDA Toolkit (`libcudart`, `nvcc`, `libnvrtc`) is NOT preinstalled**
  (high). Add it via `nvidia/cuda:*-runtime-*`/`*-devel-*` images (with
  `add_python`) or pip `nvidia-cuda-runtime-cu12`/`nvidia-cuda-nvrtc-cu12`. Keep
  toolkit major ≤ host (12.x/13.x guaranteed compatible). This is the M13 Tier-1
  requirement.
- **A normal `@app.function` can `subprocess.run(['nvidia-smi'])`** (high) — this
  is Modal's own documented example and the direct basis for M10 (Python) and, by
  shelling out from the runner, M11 (Rust). No Sandbox, no toolkit needed.
- **`gpu=` takes a string with optional count suffix and a fallback list** (high);
  the family catalog and per-type max counts **drift** (medium) — pass strings
  through and surface Modal's error rather than re-implementing the catalog.
- **cudarc (0.19.x) defaults to `dynamic-loading`** (high): links with no CUDA at
  build time, dlopens at runtime; supports CUDA 11.4–13.0 via features or
  `cuda-version-from-build-system`. The driver-API path loading **precompiled
  PTX/cubin needs only `libcuda`** (Tier 0); runtime NVRTC
  (`nvrtc::compile_ptx`) needs `libnvrtc.so` (Tier 1). This split is exactly why
  M12 is Tier 0 and M13 is Tier 1.
- **Burn (`burn-cuda`) → cubecl → cudarc; CubeCL JIT-compiles kernels via NVRTC at
  runtime** → needs `libnvrtc` + `libcudart` (Tier 1) (high). burn-cuda README:
  "Requires CUDA 12.x on PATH". Burn is **pre-1.0 with frequent breaking
  releases** (high churn) → pin all versions together (M13).
- **Rust-CUDA / `rustc_codegen_nvvm`** is rebooted-but-experimental, pins exact
  nightlies + LLVM 7 (medium) → **out of scope for v0**; M12 uses precompiled PTX
  instead of Rust-authored device code.
- **The runner build path is CPU-proven** (prototype M4/M7): the GPU milestones
  add only `gpu=` placement (M10/M11) and the image tier (M12/M13). M11's
  acceptance states this explicitly so exactly one boundary is under test.

### M13 — Burn tensor smoke (Tier 1, CUDA image) — PASSED (2026-06-03)

- **Result (verified correct on T4):**
  `cargo build --release -p example-burn-add --bin modal_runner` then
  `modal_runner --entrypoint burn_add --input-file …` returned, via the M0 JSON
  envelope: `{"ok":true,"value":{"backend":"burn-cuda (CubeCL CUDA / cudarc)",`
  `"libcudart":"libcudart.so","libnvrtc":"libnvrtc.so","n":256,`
  `"samples":[[0,0.0,0.0],[128,384.0,384.0],[255,765.0,765.0]],"valid":true}}`,
  exit 0. The GPU tensor add `c=a+b` (a[i]=i, b[i]=2i ⇒ c[i]=3i) matches the CPU
  reference element-wise (`valid:true`; samples 128→384, 255→765). Run on
  `gpu="T4"` (cheapest family). Cold run: CUDA-devel image build + rustup +
  cold Burn compile (~1m34s cargo build in the Function body) + GPU exec.
- **Pinned version set (pinned TOGETHER; from Cargo.lock):**
  `burn 0.21.0` + `burn-cuda 0.21.0` → `cubecl 0.10.0` (cuda) → `cubecl-cuda 0.10.0`
  → `cudarc 0.19.7` (features `driver, nvrtc, std, fallback-dynamic-loading,
  fallback-latest, cuda-version-from-build-system`). burn-cuda pulls cudarc with
  dynamic loading, so the crate builds with **no CUDA toolkit at build time** (it
  built fine on the CPU-only Mac host and in the Modal Function body); CubeCL
  dlopens cudarc/NVRTC at runtime. CUDA container major **12.x ≤ host** (observed
  Driver API 13.0; drifts — not hardcoded).
- **Tier 1 recipe (recorded):** `nvidia/cuda:12.6.3-devel-ubuntu22.04` +
  `add_python="3.12"` + rustup-installed stable Rust toolchain, `gpu="T4"`,
  `CUDA_PATH=/usr/local/cuda`. Build path otherwise IDENTICAL to the M4/M7/M11/M12
  recipe (mount `copy=False`, `cargo build` in the Function body to a
  known-writable `/tmp/target`); the image tier is the only new variable.
- **EMPIRICAL FINDING — `-runtime-` is NOT sufficient for Burn/CubeCL; need the
  CUDA headers (`-devel-`).** First attempt on `nvidia/cuda:12.6.3-runtime-ubuntu22.04`
  built + dlopen-self-checked fine (libnvrtc/libcudart present) but the Burn op
  FAILED at the first kernel launch with NVRTC error `catastrophic error: cannot
  open source file "cuda_runtime.h"`. Root cause: CubeCL JIT-compiles its CUDA C
  kernels via NVRTC and the generated source `#include <cuda_runtime.h>`;
  `cubecl-cuda` passes `--include-path=$CUDA_PATH/include` (default
  `/usr/local/cuda/include`) to NVRTC, so the CUDA **headers** must be on disk —
  not just the runtime shared libs. The `*-runtime-*` image ships `libcudart` +
  `libnvrtc` but NOT the headers; the `*-devel-*` image ships them at
  `/usr/local/cuda/include`. This is what "burn-cuda requires CUDA 12.x on PATH"
  means in practice. We still do NOT invoke `nvcc` ourselves — NVRTC (the Tier-1
  runtime mechanism) does the compiling — but it needs the toolkit headers the
  devel image provides. **Refinement to the §2.8 tiering note for the Burn path:
  pip `nvidia-cuda-runtime-cu12`/`-nvrtc-cu12` or a `*-runtime-*` tag alone are
  insufficient for CubeCL; use a `*-devel-*` image (headers) or otherwise place
  `cuda_runtime.h` under `$CUDA_PATH/include`.**
- **Hard Tier-1 self-check verified as a real gate.** The runner's
  `tier1_self_check()` dlopens `libnvrtc`+`libcudart` before touching Burn. On the
  CPU-only Mac host (and any accidentally Tier-0 image) it returns the loud
  `function_error` envelope (`"Tier-1 self-check FAILED: could not dlopen
  libnvrtc…"`, exit 1) — proven locally; on T4 it passes and reports the opened
  sonames (`libnvrtc.so`, `libcudart.so`) in the result.
- **`gpu="T4"` passthrough** exercised again — string passed verbatim, placed on a
  real NVIDIA T4. Cost-sensitive: the run attaches a T4 GPU (Modal on-demand T4 is
  the cheapest family; on the order of ~$0.60/hr, billed per-second for the ~mins
  of cold build + exec).

## GPU validation (2026-06-03)

Full M10→M13 GPU chain run end-to-end on Modal; all four milestones produced
verified results on a real NVIDIA **Tesla T4**. The chain did NOT stop — the GPU
gate passes. Point-in-time host values observed across the whole chain: driver
**580.95.05** / CUDA Driver API **13.0** (these drift — not hardcoded; re-verify
in `nvidia-smi`). GPU type `T4` (cheapest family) for every milestone; per-run
cost is on the order of Modal's on-demand T4 (~$0.60/hr, billed per-second for
the few minutes of cold build + exec). No Modal incidents/flakiness — bugs hit
along the way were real (M12 PTX) and were fixed, not retried-around.

### M10 — GPU placement (Tier 0, Python shim) — PASS

- `@app.function(gpu="T4")` body running `subprocess.run(["nvidia-smi"])`
  (returned via the `smi_py` `@app.local_entrypoint()`), file
  `/Users/nicolas/devel/modal-rust/workpads/gpu-compute/gpu_app.py`.
- Remote `nvidia-smi` showed a real **Tesla T4** (15360 MiB / ~15 GB), **Driver
  Version 580.95.05**, **CUDA Version (Driver API) 13.0** — matching the seeded
  point-in-time values, confirming Tier 0 (driver + `nvidia-smi` preinstalled).
- NO CUDA toolkit installed: image is `debian_slim()` + `gpu="T4"` as the only
  new variable — no `nvidia-cuda-*` wheels, no `nvidia/cuda:*` base, no
  `nvcc`/`libnvrtc`/`libcudart`.
- `gpu=` passthrough behaves as specified (boundaries.md §9): `"T4"` parses and
  places verbatim; modal-rust does not re-implement the drifting catalog, so a
  bad type would surface Modal's own error.

### M11 — Rust-sees-GPU (Tier 0, no CUDA crate) — PASS

- Target `gpu_info_runner` (`@app.function(gpu="T4", timeout=1800)`) using the
  exact `dev_app.py` M4 mounted-workspace (`copy=False`) build-in-body recipe:
  `cargo build --release -p example-add --bin modal_runner`, then exec
  `--entrypoint gpu_info --input-file`, driven by `gpu_info_rust`
  `@app.local_entrypoint()`. `example-add` was already a workspace member.
- Build path IDENTICAL to the prototype M4/M7 recipe; the SOLE new variable is
  `gpu="T4"`. The single Modal run showed `cargo` updating the crates.io index,
  downloading, and compiling in the Function body (`Finished release profile in
  8.16s`) with source mounted `copy=False`, then returned one JSON envelope.
- Result produced BY RUST (verbatim from the runner's single stdout envelope):
  `{"ok":true,"value":{"exit_code":0,"nvidia_smi":"… NVIDIA-SMI 580.95.05  Driver
  Version: 580.95.05  CUDA Version: 13.0 … Tesla T4 … 15360MiB …","stderr":""}}`
  — matches M10, now from Rust.
- Tier 0 confirmed: `cargo tree -p example-add` shows only
  `anyhow`/`serde`/`serde_json` (no `cudarc`/`cuda`/`cubecl`/`burn`);
  `Cargo.lock` has 0 `cudarc` entries. Tests/clippy/fmt clean; succeeded first
  attempt (no Modal flakiness).

### M12 — cudarc vector-add (Tier 0, precompiled PTX, driver-only) — PASS

- **Result:** cudarc 0.19.7 (dynamic-loading) vector-add `c[i]=a[i]+b[i]` via the
  CUDA **Driver API** + **precompiled PTX** on Tesla T4 (driver 580.95.05 / CUDA
  13.0, `driver_version=13000`), `valid:true` element-wise vs a CPU reference at
  **n=1024** and re-confirmed at **n=4096** (`valid:true`).
- **Tier:** Tier 0 confirmed. Image is `rust:1-slim` + `add_python` only (no CUDA
  toolkit installed). cudarc `dynamic-loading` links with NO CUDA at build time
  (compiles on macOS with no CUDA present); only `libcuda` is dlopened at
  runtime. PTX is precompiled with forward-compatible `.target sm_52` (driver-JIT
  to T4 sm_75) — no nvcc/NVRTC/libcudart on the runtime path. Build is
  package-qualified (`-p example-cuda-vector-add`) since `modal_runner` is shared.
- **Self-check proven to fail loudly with no GPU:** cudarc panics listing all
  `libcuda.so*` names searched → structured `panic` envelope, exit 1.
- **Two REAL bugs found and fixed (not Modal flakiness):** (1) first hand-written
  PTX was rejected with `CUDA_ERROR_INVALID_PTX` — fixed by generating
  authoritative nvcc PTX; (2) the JIT error log (via `cuModuleLoadDataEx` +
  `CU_JIT_ERROR_LOG_BUFFER`) revealed `ptxas fatal: Unexpected non-ASCII
  character on line 10` — an em-dash/§ in the PTX comment header. Fixed by making
  the file ASCII-only and stripping the comment header to `.version` before
  loading (`sanitize_ptx`).
- The `modal-rust` CLI and `examples/add` were not touched.

### M13 — Burn smoke + Tier-1 recipe/pins — PASS

See the detailed "M13 — Burn tensor smoke (Tier 1, CUDA image) — PASSED
(2026-06-03)" block above for full evidence. Summary:

- Burn CUDA-backend tensor add `c=a+b` verified correct on T4 (`valid:true`;
  samples 128→384, 255→765), JSON envelope exit 0, backend `burn-cuda (CubeCL
  CUDA / cudarc)`, `libnvrtc.so`/`libcudart.so` dlopen self-check passed.
- **Pins (together, from Cargo.lock):** `burn 0.21.0` + `burn-cuda 0.21.0` →
  `cubecl 0.10.0` / `cubecl-cuda 0.10.0` → `cudarc 0.19.7` (features `driver,
  nvrtc, std, fallback-dynamic-loading, fallback-latest,
  cuda-version-from-build-system`).
- **Tier-1 recipe:** `nvidia/cuda:12.6.3-devel-ubuntu22.04` + `add_python="3.12"`
  + rustup stable + `CUDA_PATH=/usr/local/cuda`, `gpu="T4"`. Empirical: a
  `*-runtime-*` image (libs only) is INSUFFICIENT — CubeCL's runtime NVRTC needs
  the CUDA **headers** (`cuda_runtime.h`), so `*-devel-*` (or headers under
  `$CUDA_PATH/include`) is required. We never invoke `nvcc` ourselves; NVRTC does
  the compiling at runtime. CUDA container major 12.x ≤ host Driver API 13.0
  (drifts; not hardcoded).

### Gate verdict & follow-up

**GPU gate: PASSED.** Both gate conditions are met with evidence: (1) the M12
cudarc precompiled-PTX vector-add verified element-wise on a Tier-0 driver-only
image; **then** (2) the M13 Burn CUDA-backend tensor add verified correct on a
Tier-1 CUDA image with `libnvrtc`/`libcudart` present and all
`burn`/`burn-cuda`/`cubecl`/`cudarc` versions pinned together. M10/M11
(`nvidia-smi` from Python then Rust) are recorded as the prerequisite
observation steps. GPU type (T4) + per-run cost recorded for every milestone.

**Follow-up — DONE (2026-06-03): `--gpu` is wired into the CLI + package-qualified
shim build.** ~~these validations were driven via direct `modal run` /
per-example `modal_runner` builds and `gpu_app.py`, not yet through the
first-class CLI surface.~~ Both halves of this follow-up have landed (see the
2026-06-03 dated note below): (a) `run`/`deploy` now accept `--gpu <spec>` with
verbatim passthrough (no GPU-catalog re-implementation), and (b) the generated
shim build is package-qualified (`-p <pkg>` derived from `--project`) so
multi-example entrypoints build cleanly. Verified by acceptance + the three
local gates. The M12 PTX-sanitize (ASCII-only, strip-to-`.version`) and the
Tier-1 `*-devel-*`-headers requirement remain as guardrails for the GPU-example
recipes (still tracked here; orthogonal to the CLI wiring, which is generic).

### CLI `--gpu` passthrough + package-qualified shim build (2026-06-03, completed)

The `modal-rust` CLI now wires the GPU path end-to-end, resolving the two
follow-ups above. Decisions + evidence recorded:

- **Package-qualified shim build (`-p <pkg>` from `--project`).** The generated
  dev/deploy shims now build the runner with `cargo build --release -p <pkg>
  --bin modal_runner`, where `<pkg>` is derived from the `--project` path (e.g.
  `--project examples/add` → `PACKAGE = "example-add"`). This fixes the
  **multiple-`modal_runner`-bins regression**: 4 workspace members (`example-add`,
  `example-add-macro`, `example-burn-add`, `example-cuda-vector-add`) all expose a
  `modal_runner` bin, so a bare `--bin modal_runner` was ambiguous and failed.
  The ambiguous bare-`--bin modal_runner` build was removed from both templates
  and prototype refs. Note: `example-burn-add` is excluded from `default-members`
  (CUDA-only) — expected/correct per the workspace config; the
  `-p example-burn-add` shim build still works on a CUDA host / Modal since it
  remains a workspace member.
- **`run`/`deploy` accept `--gpu <spec>` (verbatim passthrough).** The spec string
  passes through unchanged onto the work `@app.function` as a `gpu="<spec>"` kwarg
  — no GPU catalog is re-implemented (a bad type surfaces Modal's own error). With
  no `--gpu`, no `gpu=` kwarg is emitted at all (byte-identical to the no-GPU
  prototype shims). `--gpu T4` run →
  `@app.function(image=mounted_image, timeout=1800, gpu="T4")`; deploy mirrors it
  (`@app.function(image=image, gpu="A100")`). The gpu kwarg lands on the work
  funcs only.
- **Byte-equivalence preserved.** No-GPU `dev_app.py`/`deploy_app.py`/`call_app.py`
  are byte-identical to the prototype refs; the GPU dev shim differs from no-GPU
  ONLY by the injected `gpu="T4"` kwargs. Runner seam unchanged
  (`--input-file /tmp/in.json`, single stdout envelope); deployed body execs only
  `/app/modal_runner`. No unresolved `{{...}}` placeholders.
- **Gates green (default-members, package-qualified — NOT `--workspace`/`--all-features`).**
  `cargo fmt --check` exit 0; `cargo clippy --all-targets -- -D warnings` exit 0;
  `cargo test` exit 0 — 25 CLI unit tests pass incl.
  `dev_shim_injects_package_qualified_build`,
  `deploy_shim_injects_package_qualified_build`,
  `dev_shim_no_gpu_kwarg_when_absent`, `dev_shim_injects_gpu_kwarg_verbatim`,
  `deploy_shim_injects_gpu_kwarg_verbatim`, plus the dev/deploy/call
  byte-equivalence guards; 11 runtime + example tests.
- **Modal acceptance: PASSED (not blocked, not retry-pending).** (a) `run add`
  built `-p example-add` cleanly despite the 4 shared bins → `{"ok":true,"value":
  {"sum":42}}`. (b) `run gpu_info --gpu T4` returned the runner envelope with
  `exit_code:0` and `nvidia-smi` showing a `Tesla T4` (Driver 580.95.05, CUDA
  13.0) — proving `--gpu T4` verbatim passthrough placed the function on a GPU.
  Both completed first try (no Modal flakiness); no git touched.
- **Files:** `crates/modal-rust-cli/src/{templates.rs,workspace.rs,main.rs}` and
  `crates/modal-rust-cli/src/templates/{dev_app,deploy_app,call_app}.py.tmpl`.

## Open Questions

GPU-relevant; each has a recommended default in the synthesis (none block
M10/M11). Record the empirical answer when a milestone resolves it.

- **GPU / deploy cost confirmation** (§4 Q1, gates every milestone here): require
  an explicit `--yes` for `modal-rust run --gpu` and persistent deploys, with a
  per-run cost note? → recommended default yes; **cost-sensitive — confirm with
  the user before any GPU spend**, and record GPU type + cost per milestone.
- **Host driver / CUDA major on this account** (gates M12/M13 toolkit pin): what
  driver and supported CUDA major does Modal's GPU host currently expose? →
  observed point-in-time driver 580.95.05 / Driver API 13.0, but it **drifts** —
  re-verify in M10's `nvidia-smi` output and never hardcode; cap container toolkit
  major ≤ host.
- **Where the M12 PTX is generated** (M12): check in the PTX, or generate it at
  deploy/image-build in a Tier-2 (`*-devel-*` + nvcc) builder? → record the choice
  in M12; either keeps the runtime image Tier 0.
- **Tier-1 recipe for Burn** (M13): `nvidia/cuda:*-runtime-*` + `add_python`, or
  Tier 0 + the two pip wheels (`nvidia-cuda-nvrtc-cu12`, `nvidia-cuda-runtime-cu12`)?
  → pick one in M13, confirm `libnvrtc`/`libcudart` are on the loader path via the
  hard `dlopen` self-check.
- **Burn version compatibility** (M13): which `burn`/`burn-cuda`/`cubecl`/`cudarc`
  set actually builds and runs together given Burn's pre-1.0 churn? → resolved by
  pinning all four together in M13 and recording the working set.
- **cudarc cuda-feature pin vs host** (M12): pin an explicit cuda feature, or use
  `fallback-latest` / `cuda-version-from-build-system`? → record the chosen pin
  against Modal's host driver in M12; do not hardcode the point-in-time version.
