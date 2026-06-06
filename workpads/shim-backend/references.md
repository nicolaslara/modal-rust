# Shim Backend References

## Objective

Record local and external sources relevant to the shim-backend design space:
current generated templates, architecture contracts, Modal CLI/module behavior,
and prior research about why v0 uses generated Python plus the official `modal`
CLI.

| Resource | URL or path | Date observed | Notes |
| --- | --- | --- | --- |
| Current CLI wrapper | crates/modal-rust-cli/src/main.rs | 2026-06-03 | Renders shims to `.modal-rust/generated/` and invokes `modal run` / `modal deploy`; current path is a pure wrapper over official Modal CLI |
| Current template renderer | crates/modal-rust-cli/src/templates.rs | 2026-06-03 | Uses `include_str!` and simple placeholder replacement for app names, Rust version, and local source path |
| Run shim template | crates/modal-rust-cli/src/templates/dev_app.py.tmpl | 2026-06-03 | Current `run` control plane: `copy=False`, runtime `cargo build` in Function body, local_entrypoint receives `entrypoint`/`input_json` |
| Deploy shim template | crates/modal-rust-cli/src/templates/deploy_app.py.tmpl | 2026-06-03 | Current `deploy` control plane: `copy=True`, `run_commands(cargo build)`, deployed body runs only baked `/app/modal_runner` |
| Call shim template | crates/modal-rust-cli/src/templates/call_app.py.tmpl | 2026-06-03 | Current `call` control plane: local entrypoint invokes deployed `call_entrypoint` with `Function.from_name(...).remote(...)` |
| Architecture contracts | workpads/architecture/boundaries.md | 2026-06-03 | Source of truth for generated shim design, run-vs-deploy boundary, CLI surface, ignore rules, and modal-rs caveat |
| Architecture synthesis | workpads/architecture/research-synthesis.md | 2026-06-03 | Reviewed rationale for v0 authoring/build as generated Python + official Modal CLI; modal-rs limited to call path unless proven otherwise |
| Prototype tasks | workpads/prototype/tasks.md | 2026-06-03 | M9a byte-equivalence guard and evidence for current generated CLI shims |
| Project goal | project.md | 2026-06-03 | Product stance: generated Python shims are private implementation detail; build boundary is the product |
| Agent rules | AGENTS.md | 2026-06-03 | Requires generated shims under gitignored paths, no hand-editing generated shims as source of truth, and preservation of runner protocol |
| Modal CLI `run` reference | https://modal.com/docs/reference/cli/run | 2026-06-03 | CLI accepts a file/module ref; relevant to whether static shims must be materialized or importable |
| Modal apps guide | https://modal.com/docs/guide/apps | 2026-06-03 | `@app.local_entrypoint()` flag binding and app/function authoring model |
| Modal images reference | https://modal.com/docs/reference/modal.Image | 2026-06-03 | `add_local_dir`, `copy`, `ignore`, `run_commands`; config fields needed at module import time |
| Facade RUN/deploy config memoization | crates/modal-rust/src/app.rs | 2026-06-05 | `RemoteHandle.function_ids` is keyed by effective RUN config; deploy rejects divergent deploy-time configs |
| Per-entrypoint config regression | crates/modal-rust/tests/mock_table.rs | 2026-06-05 | Offline testkit repro for CPU-first then GPU-second order dependence against captured `FunctionCreate` |
