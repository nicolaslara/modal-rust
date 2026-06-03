# GPU Compute Tasks

Milestones M10–M13: GPU observation → real Rust GPU compute → Burn smoke.
Cockpit for the **gpu-compute** workpad (Phase 4). Background and decisions live
in `knowledge.md`; sources in `references.md`; contracts in
`../architecture/boundaries.md` and `../architecture/research-synthesis.md`;
the `add` walking-skeleton scope (the build path these milestones reuse) in
`../prototype/spec.md`.

## Objective

Prove genuine Rust GPU compute on Modal **Burn-free first**, validating one
boundary per task: `nvidia-smi` from the Python shim (M10) → `nvidia-smi` from a
Rust function (M11) → a verified cudarc vector-add via precompiled PTX, driver
only (M12) → a Burn tensor smoke on a CUDA-runtime image (M13). The ONLY new
variable over the prototype's M4/M7 build path is `gpu=` placement (M10/M11)
then the CUDA tier of the image (M12/M13); do not build a later task before the
current boundary is proven with evidence.

> **Cost caveat (cost-sensitive — confirm before running).** Every milestone in
> this workpad attaches a GPU and therefore **costs real money** on Modal (and
> deploys create persistent apps). Per the synthesis §4 Q1, require an explicit
> `--yes` confirmation for `modal-rust run --gpu` (and any persistent deploy),
> surface a per-run cost note, and record the GPU type + observed cost in each
> milestone's evidence. Confirm with the user before any GPU spend.

> **Burn-free-first ordering (do not reorder).** The first GPU compute proof
> (M12) is deliberately Burn-free: cudarc + precompiled PTX through the Driver
> API on a Tier-0 (driver-only) image, isolating the leanest possible CUDA
> stack. Burn (M13) — which drags in cubecl + runtime NVRTC and a Tier-1 image —
> comes ONLY after a Rust GPU result is verified. This is the project's
> "keep the first GPU proof Burn-free" stance (project.md, §2.8).

## Gate

The GPU gate passes when `knowledge.md` records, with evidence: (1) a **verified
Rust GPU compute result** — the M12 cudarc vector-add `c[i]=a[i]+b[i]` on a real
Modal GPU, checked element-wise against a CPU reference, on a Tier-0 driver-only
image with a precompiled-PTX kernel (no runtime NVRTC, no nvcc); **then** (2) a
**Burn tensor smoke** — the M13 minimal Burn CUDA-backend tensor add on a Tier-1
CUDA-runtime image, verified correct, with `libnvrtc`/`libcudart` confirmed
present and all `burn`/`burn-cuda`/`cubecl`/`cudarc` versions pinned together.
M10/M11 (`nvidia-smi` from Python, then from Rust) are the prerequisite
observation steps. GPU cost is recorded for every milestone.

## M10 - GPU `nvidia-smi` from the Python shim (Tier 0 sanity)

Status: completed (2026-06-03) — `gpu="T4"` placed on a real Tesla T4 (15360 MiB);
`nvidia-smi` from the Python shim reports Driver 580.95.05 / CUDA Driver API 13.0;
no CUDA toolkit installed (Tier 0); `gpu=` passthrough verbatim. Evidence in
knowledge.md "GPU validation (2026-06-03) — M10".

Acceptance:
- `@app.function(gpu='T4')` body runs `subprocess.run(['nvidia-smi'])` and
  returns its stdout; the output shows a GPU + driver version + CUDA Driver API
  version. Can run in PARALLEL with the prototype chain (depends only on M1).
- `gpu=` passthrough is exercised: `'T4'` parses and places; a bad GPU type
  surfaces Modal's error (the drifting catalog is NOT re-implemented — strings
  pass through verbatim).
- NO CUDA toolkit is installed — Tier 0, driver-only (`libcuda` + `nvidia-smi`
  preinstalled; `libcudart`/`nvcc`/`libnvrtc` absent).
- GPU type + observed cost recorded (cost-sensitive — confirm before running).

Evidence:
- `modal run /Users/nicolas/devel/modal-rust/workpads/gpu-compute/gpu_app.py::smi_py`
  console output (GPU + driver + CUDA Driver API version).
- Confirmation no CUDA toolkit was added (Tier 0); the `gpu=` passthrough note
  (good type places, bad type surfaces Modal's error).
- Recorded GPU type + per-run cost in `knowledge.md`.

## M11 - `nvidia-smi` from a Rust function (Tier 0, no CUDA crate)

Status: completed (2026-06-03) — `gpu_info` runner (built in-body via the M4
recipe; sole new variable `gpu="T4"`) returned `{"ok":true,...}` with `nvidia-smi`
output produced BY RUST (Tesla T4 / driver 580.95.05 / CUDA Driver API 13.0);
`cargo tree -p example-add` shows NO cudarc/cuda/cubecl/burn (still Tier 0).
Evidence in knowledge.md "GPU validation (2026-06-03) — M11".

Acceptance:
- A `gpu_info` entrypoint runs `std::process::Command::new("nvidia-smi")` and
  returns its output via the M0 JSON envelope (`{"ok":true,"value":{...}}`).
- On a `gpu='T4'` Function (driven via the M9 `modal-rust` CLI), the result
  matches M10 — now produced BY RUST.
- No CUDA crate in the tree: `cargo tree`/`Cargo.lock` show no `cudarc`; the
  image is still Tier 0 (driver-only).
- Acceptance states explicitly: the build path is IDENTICAL to the prototype's
  M4/M7 recipe (CPU-proven); the SOLE new variable is `gpu=` placement.
- GPU type + observed cost recorded.

Evidence:
- `cargo run -p modal-rust-cli -- run gpu_info --gpu T4 --input '{}'` — the
  JSON-enveloped Rust result carrying `nvidia-smi` output (matches M10).
- `cargo tree` / `Cargo.lock` excerpt showing no CUDA dependency.
- The build/run command + recorded GPU type + per-run cost; explicit note that
  the only delta vs M4/M7 is `gpu=`.

## M12 - Real Rust CUDA vector add (cudarc, precompiled PTX, driver-only)

Status: completed (2026-06-03) — cudarc 0.19.7 (dynamic-loading) vector-add via
the Driver API + precompiled PTX on Tesla T4 (driver 580.95.05 / CUDA 13.0),
`valid:true` element-wise vs CPU ref at n=1024 and n=4096. Tier 0 confirmed
(`rust:1-slim`+add_python only; libcuda-only at runtime, no NVRTC/nvcc/libcudart;
forward-compatible `.target sm_52` → T4 sm_75). Self-check fails loudly without a
GPU. Evidence in knowledge.md "GPU validation (2026-06-03) — M12".

Acceptance:
- cudarc added with `-F dynamic-loading` (links with NO CUDA present at build
  time; dlopens libs at runtime); `cargo tree` shows the dynamic-loading feature.
- The kernel is shipped as **precompiled PTX** (checked-in, or generated at
  deploy/image-build time in a Tier-2 builder) — NOT NVRTC-compiled at runtime;
  PTX (driver-JIT, forward-compatible), not a fixed-arch cubin.
- `c[i]=a[i]+b[i]` on `gpu='T4'` verified element-wise against a CPU reference
  (equal).
- A startup self-check loads `libcuda` and fails LOUDLY on misconfig
  (dynamic-loading otherwise hides a missing lib until first use).
- The runtime image is Tier 0 (driver-only): only `libcuda` needed — no nvcc, no
  runtime NVRTC, no `libcudart`.
- The cuda-feature pin (or `fallback-latest`) vs Modal's host driver is recorded;
  the point-in-time driver version is NOT hardcoded (it drifts).
- GPU type + observed cost recorded.

Evidence:
- `cargo run -p modal-rust-cli -- run cuda_vector_add --gpu T4 --input '{"n":1024}'`
  — computed vector vs CPU-reference, equal.
- Confirmation the runtime image is Tier 0 and the kernel was precompiled PTX
  (plus where the PTX was generated).
- `cargo tree` showing cudarc with `dynamic-loading`; recorded cuda feature pin +
  GPU type + per-run cost.

## M13 - Burn tensor smoke (Tier 1, CUDA-runtime image)

Status: completed (2026-06-03) — verified Burn CUDA-backend tensor add on T4
(`valid:true`; samples 128→384, 255→765). Pinned set: burn 0.21.0 / burn-cuda
0.21.0 / cubecl 0.10.0 / cubecl-cuda 0.10.0 / cudarc 0.19.7. Tier-1 recipe:
`nvidia/cuda:12.6.3-devel-ubuntu22.04` + add_python=3.12 + rustup,
`CUDA_PATH=/usr/local/cuda`, gpu="T4". Empirical: CubeCL's runtime NVRTC needs
the CUDA **headers** (`cuda_runtime.h`) — a `*-runtime-*` image (libs only)
fails; `*-devel-*` (or headers under `$CUDA_PATH/include`) is required. Hard
`dlopen` self-check of libnvrtc+libcudart passes on T4, fails loudly on Tier 0.
See knowledge.md "M13 — Burn tensor smoke … PASSED" for full evidence.

Acceptance:
- Tier 1 image: `nvidia/cuda:<12.x|13.x>-runtime-<os>` + `add_python='3.12'`, OR
  Tier 0 + pip `nvidia-cuda-nvrtc-cu12` + `nvidia-cuda-runtime-cu12` so that
  `libnvrtc.so` and `libcudart.so` are on the loader path (CubeCL JIT-compiles
  kernels via NVRTC at runtime).
- A minimal Burn CUDA-backend tensor add on `gpu='T4'` verified correct.
- `burn`, `burn-cuda`, `cubecl`, `cudarc` versions are pinned TOGETHER and
  recorded (Burn is pre-1.0 with frequent breaking releases).
- Container CUDA toolkit major ≤ host driver's supported major (12.x/13.x
  guaranteed compatible).
- A startup self-check `dlopen`s `libnvrtc` + `libcudart` as a HARD gate, failing
  loudly if the image is accidentally Tier 0.
- GPU type + observed cost recorded.

Evidence:
- `cargo run -p modal-rust-cli -- run burn_add --gpu T4 --input '{"n":256}'` —
  verified Burn tensor-add result.
- The Tier 1 recipe (CUDA-runtime tag OR exact pip wheels) + confirmation
  `libnvrtc`/`libcudart` are present (self-check passes).
- Pinned `burn`/`burn-cuda`/`cubecl`/`cudarc` versions + CUDA major; recorded GPU
  type + per-run cost.
