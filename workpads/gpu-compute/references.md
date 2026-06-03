# GPU Compute References

Sources backing the GPU milestones (M10–M13). Seeded from the Verified Facts of
[`../architecture/research-synthesis.md`](../architecture/research-synthesis.md)
(§1.4 GPU + CUDA for Rust, plus the cross-cutting Function/image facts the GPU
build path reuses). Append spike commands + outputs (with dates) as milestones
produce evidence. GPU/driver/catalog facts are point-in-time and **drift** —
re-verify before pinning.

## Objective

Anchor every GPU decision and milestone to a primary source or an authored
project document, with observation dates, so the GPU tiering (driver-only vs
CUDA-runtime), the cudarc precompiled-PTX path, and the Burn/cubecl Tier-1
requirement can be re-verified. Driver versions and the GPU catalog change —
re-verify point-in-time facts before pinning.

| Resource | URL or path | Date observed | Notes |
| --- | --- | --- | --- |
| Research & Architecture Synthesis | `workpads/architecture/research-synthesis.md` | 2026-06-03 | Authoritative locked decisions, verified facts (§1.4 GPU/CUDA), corrected M0–M13 plan; GPU tiering (§2.8) |
| Architecture boundaries | `workpads/architecture/boundaries.md` | 2026-06-03 | Crate layout, runner protocol, run-vs-deploy boundary, `--gpu` mapping (architecture-gate artifact) |
| Product goal & stances | `project.md` | 2026-06-03 | Goal, design stances (build boundary is the hard invariant; direct-execution-first with a Sandbox fallback; prefer static dispatch), GPU capability, "keep the first GPU proof Burn-free" loose end, phases |
| Agent rules | `AGENTS.md` | 2026-06-03 | gpu-compute gate (verified Rust GPU compute, Burn-free first; then Burn smoke); cheap/CPU before GPU; cost rules |
| Working practices | `WORKING.md` | 2026-06-03 | GPU milestone verification (nvidia-smi/CUDA evidence, verified compute result, cost noted); GPU gate |
| Prototype spec | `workpads/prototype/spec.md` | 2026-06-03 | The `add` walking skeleton + M4/M7 build path the GPU milestones reuse unchanged; GPU/Burn deferred to this workpad |
| Modal CUDA guide | https://modal.com/docs/guide/cuda | 2026-06-03 | GPU machines preinstall driver + CUDA Driver API (`libcuda`) + `nvidia-smi` (Tier 0); toolkit (`libcudart`/`nvcc`/`libnvrtc`) NOT preinstalled; observed driver 580.95.05 / Driver API 13.0 (drifts); normal Function can `subprocess.run(['nvidia-smi'])` |
| Modal GPU acceleration | https://modal.com/docs/guide/gpu | 2026-06-03 | `gpu=` string with count suffix (`"H100:8"`) + fallback list; families T4/L4/A10/L40S/A100(40/80GB)/H100/H200/B200/RTX-PRO-6000 (catalog + max counts drift) |
| Modal existing images | https://modal.com/docs/guide/existing-images | 2026-06-03 | `nvidia/cuda:*` base usable via `from_registry(..., add_python=...)`; Function image needs python+pip on `$PATH`, linux/amd64; ENTRYPOINT must `exec "$@"` (Tier 1/2 images) |
| Modal Images (reference) | https://modal.com/docs/reference/modal.Image | 2026-06-03 | `from_registry(tag, add_python=, setup_dockerfile_commands=, force_build=)`; `run_commands`; `.entrypoint([])` (basis for Tier 1/2 image construction) |
| cudarc (repo) | https://github.com/coreylowman/cudarc | 2026-06-03 | 0.19.x defaults to `dynamic-loading` (links with no CUDA at build time, dlopens at runtime); supports CUDA 11.4–13.0 via features or `cuda-version-from-build-system` |
| cudarc (docs.rs) | https://docs.rs/cudarc | 2026-06-03 | Driver-API path loading precompiled PTX/cubin needs only `libcuda` (Tier 0); runtime NVRTC (`nvrtc::compile_ptx`) needs `libnvrtc.so` (Tier 1) — the M12-Tier-0 / M13-Tier-1 split |
| burn-cuda (lib.rs) | https://lib.rs/crates/burn-cuda | 2026-06-03 | burn-cuda → cubecl → cudarc; CubeCL JIT-compiles via NVRTC at runtime → needs `libnvrtc`+`libcudart` (Tier 1); README "Requires CUDA 12.x on PATH" |
| Burn (repo) | https://github.com/tracel-ai/burn | 2026-06-03 | Pre-1.0 with frequent breaking releases (high churn) — pin `burn`/`burn-cuda`/`cubecl`/`cudarc` together (M13) |
| Rust-CUDA / rustc_codegen_nvvm | https://rust-gpu.github.io | 2026-06-03 | Rust-authored kernels rebooted-but-experimental; pins exact nightlies + LLVM 7 — OUT OF SCOPE for v0 (M12 uses precompiled PTX) |
| NVIDIA CUDA runtime wheel (cu12) | https://pypi.org/project/nvidia-cuda-runtime-cu12/ | 2026-06-03 | pip-installable `libcudart` for the Tier-0+pip Tier-1 recipe (M13 alternative to a CUDA-runtime base image) |
| NVIDIA CUDA NVRTC wheel (cu12) | https://pypi.org/project/nvidia-cuda-nvrtc-cu12/ | 2026-06-03 | pip-installable `libnvrtc` for the Tier-0+pip Tier-1 recipe (M13) — required by CubeCL runtime JIT |
| Modal pricing / GPU cost | https://modal.com/pricing | 2026-06-03 | GPU runs cost real money; deploys create persistent apps — cost-sensitive, require `--yes`, record GPU type + per-run cost (§4 Q1) |
