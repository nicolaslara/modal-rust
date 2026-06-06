# Architecture References

## Objective

Record the primary sources behind the locked architecture decisions, so each
contract in `boundaries.md` and each finding in `knowledge.md` traces to a dated,
re-verifiable source. Seeded from the Verified Facts (§1.1-§1.5) of
`research-synthesis.md`. Point-in-time facts (driver versions, GPU catalog,
modal-rs version) WILL drift — re-verify before pinning.

| Resource | URL or path | Date observed | Notes |
| --- | --- | --- | --- |
| Research & Architecture Synthesis (authoritative) | workpads/architecture/research-synthesis.md | 2026-06-03 | Single source of truth: locked decisions §2, verified facts §1, M0-M13 plan §3, open questions §4 |
| Product goal + runtime contract | project.md | 2026-06-03 | Goal, design stances (build boundary is the hard invariant; direct-execution-first with a Sandbox fallback; prefer static dispatch), runtime contract, crate layout, phases |
| Agent rules + runner protocol | AGENTS.md | 2026-06-03 | Design stances (build boundary is the hard invariant; direct-execution-first with a Sandbox fallback; prefer static dispatch), runner protocol, git/Modal/secrets rules, verification |
| Working practices + gates | WORKING.md | 2026-06-03 | Core loop, verification depth table, workpad gates, confidence/review |
| Runtime/control-plane registration split | crates/modal-rust-runtime/src/lib.rs; crates/modal-rust/src/registration.rs; crates/modal-rust-macros/src/lib.rs | 2026-06-06 | Runtime registration is dispatch-only; facade registration atomically pairs handler + FunctionConfig + package for macro inventory |
| Modal Images reference | https://modal.com/docs/reference/modal.Image | 2026-06-03 | `add_local_dir` copy=False (startup) vs copy=True (image layer); `run_commands`; `ignore=`; `from_registry`/`add_python`; `entrypoint([])` (§1.1) |
| Modal Images guide | https://modal.com/docs/guide/images | 2026-06-03 | copy=False adds files at startup, no later build steps; per-layer cache cascade (§1.1) |
| Modal custom container guide | https://modal.com/docs/guide/custom-container | 2026-06-03 | `run_commands` build-time shell + network egress (git clone, apt) (§1.1) |
| Modal existing images guide | https://modal.com/docs/guide/existing-images | 2026-06-03 | Non-Python base usable via `add_python`; Function image must be linux/amd64 + python/pip on PATH; ENTRYPOINT must exec "$@" (§1.1-§1.2) |
| Modal 1.0 migration guide | https://modal.com/docs/guide/modal-1-0-migration | 2026-06-03 | `copy_local_*` deprecated for `add_local_*`; default now mount-at-runtime (§1.1) |
| Modal changelog | https://modal.com/docs/reference/changelog | 2026-06-03 | `add_python` exact version set undocumented beyond 3.11/3.12 (medium) (§1.1) |
| Using CUDA on Modal | https://modal.com/docs/guide/cuda | 2026-06-03 | Subprocess `nvidia-smi` example; GPU driver + Driver API preinstalled (Tier 0); Toolkit not preinstalled; host driver version drifts (§1.2, §1.4) |
| Modal resources guide | https://modal.com/docs/guide/resources | 2026-06-03 | Writable filesystem; default 512 GiB disk, up to 3 TiB via `ephemeral_disk`; `/tmp` writable (§1.2) |
| Modal dataset ingestion example | https://modal.com/docs/guide/dataset-ingestion | 2026-06-03 | Confirms writable container disk / `/tmp` (§1.2) |
| Modal timeouts guide | https://modal.com/docs/guide/timeouts | 2026-06-03 | Timeout default 300 s, range 1 s-24 h, per-attempt (§1.2) |
| Modal local-data guide | https://modal.com/docs/guide/local-data | 2026-06-03 | cloudpickle-serializable args/results; plain str in/out works (§1.2) |
| Modal troubleshooting guide | https://modal.com/docs/guide/troubleshooting | 2026-06-03 | ~100 MB gRPC payload limit (413 overflow); 16 KiB parametrized args; 4 GiB web bodies (medium) (§1.2) |
| Modal Volume reference | https://modal.com/docs/reference/modal.Volume | 2026-06-03 | `Volume.from_name(create_if_missing=)`; `vol.reload()` fails "volume busy" if files open (§1.3) |
| Modal Volumes guide | https://modal.com/docs/guide/volumes | 2026-06-03 | Automatic background commits + final commit; v1 last-write-wins, avoid >5 concurrent commits; v2 distinct-file concurrency (§1.3) |
| Modal examples (cache patterns) | https://modal.com/docs/examples/ | 2026-06-03 | `HF_HUB_CACHE`/`HF_HOME` on a Volume — pattern for `CARGO_HOME`/`CARGO_TARGET_DIR` (§1.3) |
| Cargo reference (env + caching) | https://doc.rust-lang.org/cargo/ | 2026-06-03 | `CARGO_HOME` vs `CARGO_TARGET_DIR`; incremental reuse needs stable mount path + toolchain + rustflags; single-writer per target dir (§1.3) |
| Modal GPU acceleration guide | https://modal.com/docs/guide/gpu | 2026-06-03 | `gpu=` string + count suffix + fallback list; families T4/L4/A10/L40S/A100/H100/H200/B200/RTX-PRO-6000; catalog drifts (§1.4) |
| cudarc repo | https://github.com/coreylowman/cudarc | 2026-06-03 | 0.19.x default `dynamic-loading`; links with no CUDA at build time; CUDA 11.4-13.0 (§1.4) |
| cudarc docs.rs | https://docs.rs/cudarc | 2026-06-03 | Driver-API PTX/cubin needs only `libcuda`; runtime NVRTC needs `libnvrtc.so` (§1.4) |
| burn-cuda crate | https://lib.rs/crates/burn-cuda | 2026-06-03 | burn-cuda -> cubecl -> cudarc; NVRTC JIT at runtime needs `libnvrtc`+`libcudart` (Tier 1); pre-1.0 churn (§1.4) |
| Burn repo | https://github.com/tracel-ai/burn | 2026-06-03 | "Requires CUDA 12.x on PATH"; frequent breaking releases (§1.4) |
| Rust-CUDA project | https://rust-gpu.github.io | 2026-06-03 | `rustc_codegen_nvvm` rebooted-but-experimental, pins nightlies + LLVM 7; out of scope v0 (§1.4) |
| Modal managing deployments | https://modal.com/docs/guide/managing-deployments | 2026-06-03 | `modal run` ephemeral vs `modal deploy` persistent + versioned; rollback Team/Enterprise-only (§1.5) |
| Modal trigger deployed functions | https://modal.com/docs/guide/trigger-deployed-functions | 2026-06-03 | `Function.from_name(app, fn).remote()/.spawn()/.map()`; auth via `~/.modal.toml`/`MODAL_TOKEN_*` (§1.5) |
| Modal apps guide | https://modal.com/docs/guide/apps | 2026-06-03 | `modal run ::fn --flag` auto-binds flags ONLY for `@app.local_entrypoint()` (§1.5) |
| Modal webhooks guide | https://modal.com/docs/guide/webhooks | 2026-06-03 | `fastapi_endpoint`/`asgi_app`/`wsgi_app`/`web_server`; public unless proxy-auth (§1.5) |
| Modal webhook URLs guide | https://modal.com/docs/guide/webhook-urls | 2026-06-03 | URL shape `https://<workspace>--<label>.modal.run`; `Modal-Key`/`Modal-Secret` (§1.5) |
| Modal scale guide | https://modal.com/docs/guide/scale | 2026-06-03 | `min_containers`/`max_containers`/`scaledown_window`/`buffer_containers`; scale-to-zero default (§1.5) |
| Modal concurrent inputs guide | https://modal.com/docs/guide/concurrent-inputs | 2026-06-03 | `@modal.concurrent(max_inputs=, target_inputs=)` (§1.5) |
| modal-rs crate metadata | https://crates.io/api/v1/crates/modal-rs | 2026-06-03 | Unofficial 0.1.3 (2026-03-09), single-maintainer `thehumanworks`, pre-1.0, ~200 downloads (§1.5) |
| modal-rs docs.rs | https://docs.rs/modal-rs | 2026-06-03 | gRPC/tonic over TLS; vendors `api.proto`; `inner_mut()` escape hatch; reads `~/.modal.toml`/`MODAL_*` (§1.5) |
| modal-rs 0.1.3 source (extracted) | extracted 0.1.3 source: function_authoring.rs, function.rs, pickle.rs | 2026-06-03 | `FunctionCreate` needs Python `function_serialized` + `image_id`; CBOR-or-Pickle wire; serde-pickle protocol 2/3 vs cloudpickle 4 (§1.5) |
| PyO3 | https://pyo3.rs/ | 2026-06-03 | Native Python modules from Rust; later tighter bridge, not v0 (project.md) |
| maturin | https://docs.rs/maturin | 2026-06-03 | Build/package PyO3 wheels; later, not v0 (project.md) |
