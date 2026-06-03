# Ergonomics References

## Objective

Record the primary sources behind the ergonomics-phase decisions (proc-macro
registry via `inventory`; optional PyO3/maturin bridge), so each task in
`tasks.md` and each finding in `knowledge.md` traces to a dated, re-verifiable
source. Seeded from the Verified Facts (§1.1-§1.5) and macro-compatibility
decisions (§2.3) of `research-synthesis.md`, plus the `project.md` PyO3/maturin
references. PyO3/maturin specifics are UNVERIFIED until the E2 spike — re-verify
before pinning.

| Resource | URL or path | Date observed | Notes |
| --- | --- | --- | --- |
| Research & Architecture Synthesis (authoritative) | workpads/architecture/research-synthesis.md | 2026-06-03 | Single source of truth: §0 design amendments (Sandbox fallback; user errors wrapped + `details`; static dispatch); macro-compatible `Registry`/`typed!()`/`HandlerFn` (§2.3); five-kind protocol (§2.2); design stances (build boundary is the hard invariant); M0-M13 plan §3 |
| Architecture contracts (boundaries) | workpads/architecture/boundaries.md | 2026-06-03 | Macro-compatibility invariant the ergonomics gate protects; runner protocol; run-vs-deploy boundary; registry API |
| Product goal + runtime contract | project.md | 2026-06-03 | Design stances (build boundary is the hard invariant; direct-execution-first with a Sandbox fallback; prefer static dispatch); runtime contract; v2 macro must expand to the same registry shape; PyO3/maturin are later optimizations not v0 deps; PyO3/maturin reference links |
| Agent rules + runner protocol | AGENTS.md | 2026-06-03 | Design stances (build boundary is the hard invariant; Sandbox fallback; static dispatch); runner protocol "do not break"; macros must compile to the same shape; ergonomics gate prerequisite (prototype gate) |
| Working practices + gates | WORKING.md | 2026-06-03 | "Do not add ergonomics (macros, PyO3) before the manual subprocess path works end to end"; workpad gates; verification depth |
| Prototype contracts (the shape macros must reproduce) | workpads/prototype/tasks.md | 2026-06-03 | M0 manual `Registry::new().function("add", typed!(add))` + five error kinds (`function_error` wraps the user error on the top-level enum, optional `details`) + `{"ok":true,"value":{"sum":42}}` — the validated runner shape E1 must match |
| inventory crate (distributed registration) | https://docs.rs/inventory | 2026-06-03 | Collects `inventory::submit!` registrations at startup; basis for `Registry::from_inventory()` (medium — confirm in E1) (§2.3) |
| syn (proc-macro parsing) | https://docs.rs/syn | 2026-06-03 | Parse the annotated fn (detect `async fn`, parameter names) for `#[modal_rust::function]` expansion (E1) |
| quote (proc-macro codegen) | https://docs.rs/quote | 2026-06-03 | Generate the `inventory::submit!` + `typed!(..)`/`typed_async!(..)` (monomorphized `fn`-pointer wrapper) expansion (E1) |
| cargo-expand (macro verification) | https://github.com/dtolnay/cargo-expand | 2026-06-03 | Evidence tool: show `#[modal_rust::function]` expanding to the manual-path shape (E1) |
| trybuild (macro snapshot tests) | https://docs.rs/trybuild | 2026-06-03 | Optional: assert macro expansion / compile-fail behaviour (E1) |
| PyO3 | https://pyo3.rs/ | 2026-06-03 | Native Python modules from Rust; the optional in-process bridge replacing the subprocess boundary (E2); abi3/version-specific wheels to verify |
| maturin | https://docs.rs/maturin | 2026-06-03 | Build/package PyO3 wheels; `maturin build` / `maturin develop`; manylinux/linux-amd64 wheel compatibility to verify in E2 |
| Modal existing images guide | https://modal.com/docs/guide/existing-images | 2026-06-03 | Function image needs python+pip on PATH / `add_python`; a PyO3 wheel is `pip install`ed into the image (§1.1) — PyO3 does not remove the Python layer |
| Modal Images guide | https://modal.com/docs/guide/images | 2026-06-03 | `run_commands` / pip install in an image layer to install the maturin-built wheel (E2) (§1.1) |
| Modal local-data guide | https://modal.com/docs/guide/local-data | 2026-06-03 | Plain `str` in/out across the Modal boundary — same envelope text as the subprocess path crosses the PyO3 boundary (§1.2) |
| modal-rs 0.1.3 source (extracted) | extracted 0.1.3 source: function_authoring.rs | 2026-06-03 | `FunctionCreate` needs a Python `function_serialized` + `image_id`; PyO3 does NOT remove the Python shim (§1.5, residual risk #5) |
