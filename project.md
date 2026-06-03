# modal-rust

## Goal

Build **modal-rust**, a general **Rust-on-Modal function runtime**: a CLI plus
runtime crates that let a user write ordinary Rust library functions, run them on
Modal during development, deploy them as persistent Modal Functions, and invoke
them remotely — including on GPUs.

Burn (Rust ML) is **one downstream consumer**, not the core abstraction. The
hard design problem is **Rust function discovery + remote dispatch + project
packaging**, with a clean split between **build-at-run-time** (dev) and
**build-at-deploy-time** (prod).

## Name

- **CLI binary:** `modal-rust` (the user-facing command). `modal-rs` already
  exists as an unofficial Rust SDK for Modal; we may consume it internally.
- **Crate names** are implementation detail (`modal-rust-runtime`,
  `modal-rust-cli`, `modal-rust-client`, `modal-rust-macros`). Users should not
  need to care which crate does what.

## Source Of Truth

This project is set up from the design discussion captured in the initial user
prompt (2026-06-03). When generated docs conflict with that prompt, preserve the
prompt's intent and update the docs.

The plan was then grounded in primary Modal/modal-rs docs and adversarially
reviewed by a multi-agent workflow; the authoritative consolidation (verified
facts, locked architecture decisions, the M0–M13 milestone plan, open questions,
and residual risks) lives in
[`workpads/architecture/research-synthesis.md`](./workpads/architecture/research-synthesis.md),
distilled into the contract doc
[`workpads/architecture/boundaries.md`](./workpads/architecture/boundaries.md).
Those two files are authoritative for the architecture gate.

The design stances:

1. **Direct-execution-first; Sandbox is a documented fallback** *(a hypothesis to
   validate, not a permanent ban).* Try the core path on **normal Modal Functions**
   (`@app.function`) FIRST — runtime compile in a Function body is the central
   thing to prove (prototype M4). If direct Function execution proves infeasible
   for a step, **iterate to a Modal Sandbox** for that step and record the
   decision. Sandboxes are a fallback explicitly on the table, not the default.
2. **The build boundary is the product** *(the hard, non-negotiable invariant).*
   `run` builds Rust *at function-execution time* (source mounted, `cargo build`
   in the function body — or a Sandbox if that proves necessary). `deploy` builds
   Rust *at image-build time* and the deployed runtime executes only a prebuilt
   binary — never `cargo`.
3. **Prefer static dispatch.** Favor compile-time polymorphism — `enum`
   (closed-world), generics (`T: Trait`) / `impl Trait` (monomorphization),
   marker/type-state, `cfg` features — over `dyn Trait`; reach for `dyn` only for
   genuinely open/unbounded sets. (The handler registry is the one open set; it
   erases user functions to `fn` pointers, not `Box<dyn>`.)

## Product Thesis

Once the unit is "Rust functions on Modal", the architecture is:

```text
user crate (library)        # registered Rust functions, normal lib.rs
        |
generated runner binary     # links the user crate, dispatches by name
        |
remote Modal container      # runs the runner, calls the function by name
```

Key decision: **the user does not own `main()`.** This is a **library crate +
generated/owned runner binary** model, not a "user writes the Modal entrypoint"
model. Macros are sugar added *later*; the v0 API is a manual registry that
macros must compile down to.

```text
Python Modal:  module has decorated functions; Modal imports & finds them.
modal-rust:    crate has registered functions; a runner links the crate & dispatches them.
```

## Core Architecture Stance

The portability boundary is **source for dev, image/binary for deploy.**

```text
modal-rust run <entrypoint>
  local Rust project
    -> mount source into a Modal Function (add_local_dir copy=False)
    -> cargo build inside the function body (Linux/CUDA env)
    -> execute the selected entrypoint, return result

modal-rust deploy <entrypoint>
  local Rust project
    -> copy source into a Modal image layer (add_local_dir copy=True)
    -> cargo build during image build (run_commands)
    -> bake /app/modal_runner into the image
    -> deployed runtime executes the prebuilt binary only (no cargo)
```

v0 implementation uses **generated Python Modal shims** as a known-good control
path (Modal's Function authoring surface is Python-first), shelling out to a Rust
**subprocess** runner. The generated shim is a private implementation detail; the
public UX is the `modal-rust` CLI. PyO3/maturin and proc-macros are later
optimizations, not v0 dependencies.

## Crate / Repo Layout (intended)

```text
modal-rust/
  Cargo.toml                          # workspace
  crates/
    modal-rust-runtime/               # Registry, typed!(), Codec, HandlerFn (static dispatch), runner protocol
    modal-rust-cli/                   # produces the `modal-rust` binary; generates shims
    modal-rust-client/                # control-plane: talk to Modal (modal-rs and/or generated python)
    modal-rust-macros/                # placeholder for v2 #[modal_rust::function]
  examples/
    add/                              # examples/add/src/lib.rs + src/bin/modal_runner.rs
    gpu-info/                         # nvidia-smi from a Rust function
    cuda-vector-add/                  # tiny Rust GPU compute (cudarc/cust)
    burn-add/                         # Burn tensor smoke (last)
```

## Runtime Contract (the invariant to protect)

The runner is brutally simple and stable across the manual-registry and
future-macro worlds:

```text
/app/modal_runner --entrypoint <name> --input-json '<json>'
```

Success:

```json
{ "ok": true, "value": { ... } }
```

Failure:

```json
{ "ok": false, "error": { "kind": "decode_error|unknown_entrypoint|function_error|encode_error|panic", "message": "...", "details": null, "backtrace": "..." } }
```

The closed error-kind enum is **five** kinds (frozen), modeled as a Rust
`RunnerError` enum that **wraps the user's error** rather than stringifying it
early: `decode_error` (bad input JSON or wrong-shape), `unknown_entrypoint`,
`function_error` (handler returned `Err` — the user error wrapped: `message` =
Display/anyhow chain, optional `details` = the serialized user error when it is
`Serialize`), `encode_error` (output failed to serialize — must NOT masquerade as
a panic), and `panic` (handler unwound; captured via panic hook + `catch_unwind`,
which requires `panic = "unwind"`). stdout carries **exactly one** JSON envelope;
all cargo/rustc/user diagnostics go to stderr. Exit code mirrors `ok`.

The invariant every layer protects — static dispatch (monomorphized `fn`-pointer
wrappers, no `Box<dyn>`), codec-neutral on bytes (CBOR/msgpack + async additive):

```text
name -> monomorphized typed! wrapper (fn pointer) -> bytes in -> bytes out   (JSON Codec in v0)
```

v0 manual registry:

```rust
pub fn modal_registry() -> Registry {
    Registry::new().function("add", typed!(add))   // typed! yields a bare fn pointer
}
```

v2 macro (must expand to the same registry shape):

```rust
#[modal_rust::function(name = "add")]
pub fn add(input: AddInput) -> anyhow::Result<AddOutput> { ... }
```

## Desired Capabilities

| Area | Capability |
| --- | --- |
| Authoring | Write normal Rust library functions; no required `main()` |
| Dispatch | Named functions, typed JSON in/out, structured errors, panic capture |
| Dev run | Mount source, compile remotely per run, execute; reflects local edits |
| Deploy | Build once at deploy time; persistent Modal Function; no runtime compile |
| Invoke | Call deployed functions from the CLI (via modal-rs, generated python, or HTTPS) |
| Caching | Cargo registry/git/target cache on a Modal Volume for tolerable dev iteration |
| GPU | Run Rust in GPU-attached containers; CUDA driver access; real GPU compute |
| Ergonomics | Proc-macros (`inventory` registry) and optional PyO3/maturin bridge — later |

## Boundary Model

| Boundary | Responsibility |
| --- | --- |
| User crate | Plain Rust library exposing registered functions |
| `modal-rust-runtime` | Registry, codec, dispatch, runner CLI protocol, error/panic model |
| Runner binary | Links the user crate; `--entrypoint`/`--input-json` -> JSON result |
| `modal-rust-cli` | `doctor`/`run`/`deploy`/`call`; generates Python shims; orchestrates build stage |
| Generated shim | Private Python Modal app (dev_app/deploy_app/call_app); run-vs-deploy build placement |
| `modal-rust-client` | Talk to Modal (modal-rs where it suffices; generated python / HTTPS otherwise) |
| Build stage | **run = build in function body; deploy = build in image layer** (the product boundary) |
| GPU layer | `--gpu` parameter mapping; CUDA-capable image; native dep management |

## Stack Direction

- **Rust** for the runtime, runner, CLI, and client core.
- **Python (generated)** only as the Modal authoring/control surface, kept as a
  private generated artifact behind the CLI.
- **PyO3 / maturin** as a *later* tighter bridge (replace subprocess), validated
  only after the subprocess POC works.
- Use libraries freely (clap, serde, anyhow, tokio where needed). Prefer mature
  crates over hand-rolling. For GPU: `cudarc`/`cust` before Burn.

## Validation-First Philosophy

Build slowly, validate a boundary at a time. The riskiest assumptions are
validated **before** they are depended on:

1. Can a normal Modal Function compile Rust at runtime (no Sandbox)?
2. Is `copy=False` source mount fast/reliable enough for dev iteration?
3. Does a Cargo cache survive across Function invocations (Volume)?
4. How much deployed-Function lifecycle does `modal-rs` expose vs needing python?
5. Keep the first GPU proof independent of Burn (nvidia-smi -> CUDA -> kernel -> Burn).

## Phases

| # | Phase | Workpad | Outcome |
| --- | --- | --- | --- |
| 1 | Research | `workpads/research/` | Validate the risky assumptions above with sourced findings + tiny spikes |
| 2 | Architecture | `workpads/architecture/` | Crate layout, runtime contract, run-vs-deploy boundary, shim & CLI design |
| 3 | Prototype | `workpads/prototype/` | `add` function e2e: local dispatch -> remote run -> deploy -> call (M0-M9) |
| 4 | GPU compute | `workpads/gpu-compute/` | nvidia-smi from python -> from Rust -> CUDA vector add -> Burn smoke (M10-M13) |
| 5 | Ergonomics | `workpads/ergonomics/` | Proc-macro registry (`inventory`) and optional PyO3/maturin bridge |

## Riskiest Loose Ends

- How much deployed-Function creation/invocation `modal-rs` exposes (validate first).
- Image build mechanics: dev `copy=False` vs deploy `copy=True` + `run_commands`.
- GPU native dependency drift — keep the first GPU proof Burn-free.
- Ergonomics hiding too much too soon — no proc-macros until the manual path works end to end.

## Initial References

| Resource | URL | Notes |
| --- | --- | --- |
| Modal Images | https://modal.com/docs/reference/modal.Image | `add_local_dir` copy=False (startup) vs copy=True (image layer) |
| Modal Images guide | https://modal.com/docs/guide/images | `run_commands` build steps |
| Modal existing images | https://modal.com/docs/guide/existing-images | Functions need Python / `add_python` |
| Modal scaling | https://modal.com/docs/guide/scale | Function autoscaling (min/max containers) |
| Invoking deployed fns | https://modal.com/docs/guide/trigger-deployed-functions | Python client + HTTPS routes |
| Managing deployments | https://modal.com/docs/guide/managing-deployments | Persisted apps/functions |
| Using CUDA on Modal | https://modal.com/docs/guide/cuda | NVIDIA driver present; nvidia-smi works on GPU fns |
| GPU acceleration | https://modal.com/docs/guide/gpu | `gpu=` param; T4/L4/A10/L40S/A100/H100/... |
| modal-rs SDK | https://docs.rs/modal-rs | Unofficial Rust SDK; apps/sandboxes/images/creds |
| PyO3 | https://pyo3.rs/ | Native Python modules from Rust |
| maturin | https://docs.rs/maturin | Build/package PyO3 wheels |

## Global Backlog

- Decide modal-rs vs generated-python for each of: create app, build image, deploy function, invoke function.
- Define the runner protocol and registry API (manual now, macro-compatible).
- Prove runtime compile on a normal Modal Function.
- Prove deploy-time build with no runtime compile.
- Add a Cargo cache Volume for dev iteration.
- Prove GPU placement and real Rust GPU compute, Burn-free first.
- Add proc-macros and optional PyO3/maturin bridge once the manual path is solid.
