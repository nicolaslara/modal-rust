# Prototype References

## Objective

Anchor every prototype decision and milestone (M0–M9) to a primary source or an
authored project document, with observation dates, so the run-vs-deploy build
boundary and the runner contract can be re-verified. Seeded from the Verified
Facts (§1.1–§1.5) of `../architecture/research-synthesis.md` (date 2026-06-03).
Append spike commands + outputs (with dates) as milestones produce evidence.
Modal's API/pricing and the GPU catalog / modal-rs version drift — re-verify
point-in-time facts before pinning.

| Resource | URL or path | Date observed | Notes |
| --- | --- | --- | --- |
| Research & Architecture Synthesis (authoritative) | workpads/architecture/research-synthesis.md | 2026-06-03 | Single source of truth: locked decisions §2, verified facts §1, corrected M0–M13 plan §3, open questions §4 |
| Architecture boundaries | workpads/architecture/boundaries.md | 2026-06-03 | Crate layout, runner protocol, run-vs-deploy boundary, shim/CLI contracts (architecture-gate artifact) |
| Product goal & stances | project.md | 2026-06-03 | Goal, design stances, runtime contract, crate layout, phases, backlog |
| Agent rules | AGENTS.md | 2026-06-03 | Source-of-truth map, design stances, runner protocol, git/Modal/secrets rules, verification |
| Working practices | WORKING.md | 2026-06-03 | Core loop, verification depth table, workpad gates, confidence/review thresholds |
| Prototype spec | workpads/prototype/spec.md | 2026-06-03 | POC scope: prototype minimum, MVP additions, deferred, gate, non-goals |
| Modal Images (reference) | https://modal.com/docs/reference/modal.Image | 2026-06-03 | `add_local_file/dir(copy=False)`; `copy=True` bakes a layer; `run_commands`; `from_registry(add_python=)`; `entrypoint([])`; `ignore=` (§1.1) |
| Modal Images (guide) | https://modal.com/docs/guide/images | 2026-06-03 | `copy=False` adds files at startup (no later build steps); per-layer cache cascade — put fast-changing layers last (§1.1) |
| Modal existing images | https://modal.com/docs/guide/existing-images | 2026-06-03 | Function image needs python+pip on `$PATH` (or `add_python`), linux/amd64; non-Python base via `add_python='3.11'/'3.12'`; ENTRYPOINT must `exec "$@"` (§1.1–§1.2) |
| Modal custom container | https://modal.com/docs/guide/custom-container | 2026-06-03 | `run_commands` build-time shell with network access (`git clone`, `apt`) (§1.1) |
| Modal 1.0 migration | https://modal.com/docs/guide/modal-1-0-migration | 2026-06-03 | `copy_local_*` deprecated for `add_local_*`; new default is mount-at-runtime (`copy=False`) (§1.1) |
| Modal changelog | https://modal.com/docs/reference/changelog | 2026-06-03 | `add_python` version set undocumented beyond 3.11/3.12 by example (medium) (§1.1) |
| Modal CUDA guide | https://modal.com/docs/guide/cuda | 2026-06-03 | Normal Function can `subprocess.run` (e.g. `nvidia-smi`); GPU driver/Driver API preinstalled; toolkit not (basis for runtime compile; GPU milestones M10+) (§1.2, §1.4) |
| Modal resources | https://modal.com/docs/guide/resources | 2026-06-03 | Writable container FS; default 512 GiB disk, up to 3 TiB via `ephemeral_disk` (§1.2) |
| Modal dataset ingestion | https://modal.com/docs/guide/dataset-ingestion | 2026-06-03 | `/tmp` guaranteed writable (the build-into-local-path basis) (§1.2) |
| Modal timeouts | https://modal.com/docs/guide/timeouts | 2026-06-03 | Default 300 s, settable 1 s–24 h; per-attempt — basis for `timeout=1800` on the run path (§1.2) |
| Modal local data | https://modal.com/docs/guide/local-data | 2026-06-03 | Functions accept/return cloudpickle-serializable args; plain `str` in/out works for `.remote()` (§1.2) |
| Modal troubleshooting | https://modal.com/docs/guide/troubleshooting | 2026-06-03 | `.remote()` ~100 MB gRPC payload limit (413 on overflow); parametrized args 16 KiB; web bodies up to 4 GiB (medium) (§1.2) |
| Modal apps / entrypoints | https://modal.com/docs/guide/apps | 2026-06-03 | `modal run app.py::fn --flag` auto-binds flags ONLY for `@app.local_entrypoint()`, not a bare `@app.function` (§1.5) |
| Modal Volume (reference) | https://modal.com/docs/reference/modal.Volume | 2026-06-03 | `Volume.from_name(create_if_missing=True)`; `vol.reload()` fails "volume busy" if files open (§1.3) |
| Modal Volumes (guide) | https://modal.com/docs/guide/volumes | 2026-06-03 | Background commits "every few seconds" + final commit; v1 last-write-wins, avoid >~5 concurrent commits; v2 concurrent distinct-file writes (§1.3) |
| Modal examples (caching) | https://modal.com/docs/examples | 2026-06-03 | Tool-cache env var → Volume path is the first-class caching pattern (`HF_HOME`/`HF_HUB_CACHE`); analogous to `CARGO_HOME`/`CARGO_TARGET_DIR` (§1.3) |
| Cargo reference | https://doc.rust-lang.org/cargo/ | 2026-06-03 | `CARGO_HOME` = index+downloads, `CARGO_TARGET_DIR` = artifacts; incremental reuse needs stable path + toolchain + rustflags; single-writer per target dir (medium) (§1.3) |
| Managing deployments | https://modal.com/docs/guide/managing-deployments | 2026-06-03 | `modal run` ephemeral vs `modal deploy` persistent, version-incremented, zero-downtime; rollback Team/Enterprise-only (§1.5) |
| Trigger deployed functions | https://modal.com/docs/guide/trigger-deployed-functions | 2026-06-03 | `modal.Function.from_name(app, fn).remote()/.spawn()/.map()`; auth via `~/.modal.toml` / `MODAL_TOKEN_*` — the `call` path (§1.5) |
| Modal scaling | https://modal.com/docs/guide/scale | 2026-06-03 | `min_containers`/`max_containers`/`scaledown_window`/`buffer_containers`; scale-to-zero default (deploy lifecycle context) (§1.5) |
| Modal concurrent inputs | https://modal.com/docs/guide/concurrent-inputs | 2026-06-03 | `@modal.concurrent(max_inputs=, target_inputs=)` (deploy/concurrency context; v0 single-call) (§1.5) |
| Modal webhooks | https://modal.com/docs/guide/webhooks | 2026-06-03 | Web endpoints public unless proxy-auth — NO web endpoint in v0 (deferred, §4 Q2) (§1.5) |
| Modal webhook URLs | https://modal.com/docs/guide/webhook-urls | 2026-06-03 | URL shape `https://<workspace>--<label>.modal.run`; `Modal-Key`/`Modal-Secret` proxy-auth (§1.5) |
| modal-rs crate metadata | https://crates.io/api/v1/crates/modal-rs | 2026-06-03 | 0.1.3 (2026-03-09), UNOFFICIAL, single-maintainer `thehumanworks`, pre-1.0, ~200 downloads — `call` only, behind a flag (§1.5) |
| modal-rs SDK docs | https://docs.rs/modal-rs | 2026-06-03 | gRPC/tonic over TLS; vendors `api.proto`; `inner_mut()` escape hatch; reads `~/.modal.toml`/`MODAL_*` (§1.5) |
| modal-rs 0.1.3 source (extracted) | extracted 0.1.3 source: function_authoring.rs, function.rs, pickle.rs | 2026-06-03 | `FunctionCreate` needs Python `function_serialized` + `image_id` (does NOT remove the Python shim); CBOR-or-Pickle wire; serde-pickle protocol 2/3 vs cloudpickle 4 (§1.5) |
| Modal GPU acceleration | https://modal.com/docs/guide/gpu | 2026-06-03 | `gpu=` string + count suffix + fallback list; families T4/L4/A10/L40S/A100/H100/H200/B200/RTX-PRO-6000; catalog drifts — pass strings through (GPU milestones M10+) (§1.4) |
| cudarc repo | https://github.com/coreylowman/cudarc | 2026-06-03 | 0.19.x default `dynamic-loading` (links with no CUDA at build time); CUDA 11.4–13.0 — out of scope for the prototype gate (M12) (§1.4) |
| cudarc docs.rs | https://docs.rs/cudarc | 2026-06-03 | Driver-API PTX/cubin needs only `libcuda`; runtime NVRTC needs `libnvrtc.so` (Tier 1) — GPU milestones (§1.4) |
| burn-cuda crate | https://lib.rs/crates/burn-cuda | 2026-06-03 | burn-cuda → cubecl → cudarc; NVRTC JIT at runtime needs `libnvrtc`+`libcudart` (Tier 1); pre-1.0 churn — GPU milestone M13 (§1.4) |
| Burn repo | https://github.com/tracel-ai/burn | 2026-06-03 | "Requires CUDA 12.x on PATH"; frequent breaking releases — GPU milestone M13 (§1.4) |
| Rust-CUDA project | https://rust-gpu.github.io | 2026-06-03 | `rustc_codegen_nvvm` rebooted-but-experimental, pins nightlies + LLVM 7; out of scope for v0 (§1.4) |
| PyO3 | https://pyo3.rs/ | 2026-06-03 | Native Python modules from Rust — deferred (ergonomics phase) (project.md) |
| maturin | https://docs.rs/maturin | 2026-06-03 | Build/package PyO3 wheels — deferred (ergonomics phase) (project.md) |
