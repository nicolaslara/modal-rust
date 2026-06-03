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
