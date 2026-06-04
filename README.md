# modal-rust

A **Rust-on-Modal function runtime**. Write ordinary Rust library functions, run
them in-process, run them remotely on [Modal](https://modal.com) during
development, and deploy them as persistent Modal Functions — all through a
first-party Rust client, with no `modal` CLI and no per-project Python.

Burn (Rust ML) is one downstream consumer, not the core abstraction. The core
problem is **Rust function discovery + remote dispatch + project packaging**, with
a clean split between **build-at-run-time** (dev) and **build-at-deploy-time**
(prod).

> **Status:** the programmatic backend is live. Our own first-party Modal gRPC
> client (`crates/modal-rust-sdk`, no `modal` CLI, no `modal-rs`) powers the
> user-facing facade (`crates/modal-rust`, lib `modal_rust`). The CPU `add` path
> is **proven live** for all three flows: `.local()` (offline, in-process),
> `.remote()` (build in the function body on an ephemeral app), and
> `deploy` + `call` (build once at image-build time into a persistent app) —
> `add(40, 2) -> {sum: 42}` in every case. The image/upload was hardened to match
> the official client (hosted python-standalone mount + worker-injected client
> deps; cargo-scoped uploads). GPU *compute* is proven via the runner
> (`examples/cuda-vector-add`, `examples/burn-add` on a T4); GPU *through the
> facade decorator* is not done yet (see [What's next](#what-works-today--whats-next)).

## Try it

**Prerequisites:** a Rust toolchain ([`rustup`](https://rustup.rs)). The offline
`.local()` path needs nothing else. For the live `.remote()` / deploy / call
paths you need Modal credentials — the client reads `~/.modal.toml` or the
`MODAL_TOKEN_ID` / `MODAL_TOKEN_SECRET` environment variables **directly**. There
is **no dependency on the `modal` CLI** for any of the product flows.

Run everything from the repo root.

**Offline — the `.local()` path (zero Modal, zero network):**

```bash
cargo run -p example-orchestrate --bin orchestrate
# -> local: add(40, 2) -> {sum: 42}
# -> (skipping live .remote()/deploy/call — set RUN_REMOTE=1 ...)
```

```bash
cargo test                                # default-members (skips the CUDA-only burn example)
```

**Live — `.remote()` + deploy/call (needs Modal credentials):**

```bash
RUN_REMOTE=1 cargo run -p example-orchestrate --bin orchestrate
# -> local:  add(40, 2) -> {sum: 42}     (in-process)
# -> remote: add(40, 2) -> {sum: 42}     (cargo build IN the function body)
# -> deployed app 'modal-rust-orchestrate-demo' (image im-...)
# -> call:   add(40, 2) -> {sum: 42}     (no rebuild; execs the prebuilt binary)
```

The live round-trips are also covered by the (ignored, feature-gated) integration
tests `crates/modal-rust/tests/live_remote.rs` and `live_deploy.rs`.

## Usage (the facade API)

A user writes ordinary Rust functions in a library crate. Two ways to register
them:

```rust
// Option A — manual registry (examples/add):
use modal_rust_runtime::{typed, Registry};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct AddInput { pub a: i64, pub b: i64 }
#[derive(Serialize, Deserialize)]
pub struct AddOutput { pub sum: i64 }

pub fn add(input: AddInput) -> anyhow::Result<AddOutput> {
    Ok(AddOutput { sum: input.a + input.b })
}

pub fn modal_registry() -> Registry {
    Registry::new().function("add", typed!(add))
}
```

```rust
// Option B — the #[modal_rust::function] macro (examples/add-macro). NOTE the
// macro expands to absolute paths, so the downstream crate must also depend on
// `modal-rust-runtime` and `inventory` (see crates/modal-rust/src/lib.rs):
#[modal_rust::function]
pub fn add(input: AddInput) -> anyhow::Result<AddOutput> {
    Ok(AddOutput { sum: input.a + input.b })
}
// The runner then calls `App::from_inventory()` to collect every registered fn.
```

Then orchestrate them through `App` / `Function`:

```rust
use modal_rust::{App, DeployConfig};

// 1. OFFLINE: in-process dispatch through the same frozen registry the runner uses.
//    Zero Modal, zero network. `App::from_inventory()` for the macro path.
let app = App::new(modal_registry());
let out: AddOutput = app.function("add").local(AddInput { a: 40, b: 2 })?;
assert_eq!(out.sum, 42);

// 2. REMOTE (run path): the crate is uploaded and `cargo build`-ed IN the function
//    body at invoke time; the real Rust `add` runs on Modal and returns typed.
//    `App::connect("my-app")` uses the inventory registry (the macro path); with a
//    manual registry use `App::connect_with_registry("my-app", modal_registry())`.
let app = App::connect_with_registry("my-app", modal_registry()).await?; // reads ~/.modal.toml / MODAL_TOKEN_*
let out: AddOutput = app.function("add").remote(AddInput { a: 40, b: 2 }).await?;

// 3. DEPLOY + CALL: build once at image-build time into a persistent app, then
//    invoke with no rebuild (the deployed body execs only the prebuilt runner).
let app = App::connect_with_registry("my-app", modal_registry()).await?;
let deployed = app.deploy_with(DeployConfig::for_app("my-deploy")).await?;
let out: AddOutput = app.call("my-deploy", "add", AddInput { a: 40, b: 2 }).await?;
```

The full, runnable tour lives in
[`examples/orchestrate`](./examples/orchestrate/src/main.rs): `.local()` runs in
`cargo run` / a test and prints `{sum: 42}`; `.remote()` and deploy/call are
gated behind `RUN_REMOTE=1`. (`.spawn()` / `.map()` exist on `Function` but
currently return `NotImplemented`.)

The runner contract everything compiles down to:

```text
/app/modal_runner --entrypoint add --input-json '{"a":40,"b":2}'
-> {"ok":true,"value":{"sum":42}}
```

You can drive it directly, offline, against any example crate:

```bash
cargo run -p example-add --bin modal_runner -- --entrypoint add --input-json '{"a":40,"b":2}'
# -> {"ok":true,"value":{"sum":42}}
```

Failure returns a structured envelope (five frozen error kinds). `function_error`
means the user's error is **wrapped** on the top-level `RunnerError` enum:
`message` = the Display/anyhow chain, optional additive `details` = the serialized
user error when its error type is `Serialize` (else `null`):

```text
-> {"ok":false,"error":{"kind":"function_error","message":"...","details":null,"backtrace":"..."}}
```

The five frozen error kinds are
`decode_error | unknown_entrypoint | function_error | encode_error | panic`.
stdout carries **exactly one** JSON envelope; cargo/rustc/user diagnostics go to
stderr; the exit code mirrors `ok`.

## GPU

GPU *compute* is proven via the runner on a T4 — see the examples:

- [`examples/cuda-vector-add`](./examples/cuda-vector-add/src/lib.rs) — a `cudarc`
  vector-add through the CUDA Driver API (Tier 0: driver-only, `dynamic-loading`,
  a precompiled PTX kernel JIT'd by the driver).
- [`examples/burn-add`](./examples/burn-add/src/lib.rs) — a Burn (CubeCL/CUDA)
  tensor add (Tier 1: NVRTC + CUDA runtime). This crate is CUDA-only, so it is a
  workspace member but is excluded from `default-members` (it builds on a CUDA
  host / on Modal, not on a typical dev box).

> **GPU through the facade decorator is not done yet.** A `gpu="A100"` spec on
> `#[modal_rust::function]` flowing into the remote `FunctionCreate` resources is
> upcoming (see [What's next](#what-works-today--whats-next)). Today the GPU
> examples prove the *compute* path via the runner; you cannot yet `.remote()`
> them with a GPU spec through `App`/`Function`.

## What works today / What's next

**Works today (proven live, CPU `add`):**

- `crates/modal-rust-sdk` — a first-party Modal gRPC client (auth, channel, CBOR,
  mounts/blobs/images/functions), no `modal` CLI, no `modal-rs`.
- `.local()` — in-process dispatch through the frozen `Registry` (offline).
- `.remote()` — run path: upload + `cargo build` in the function body on an
  **ephemeral** app, GC'd on disconnect.
- `deploy` + `call` — deploy path: `cargo build` **at image-build time** into a
  **persistent** app; `call` execs only the prebuilt `/app/modal_runner` (no
  cargo, no source mount at call time).
- Hardened image/upload: hosted python-standalone mount (`add_python`) +
  worker-injected client deps (matches the official client), cargo-metadata-scoped
  uploads pruned by `.modalignore` > `.gitignore` > built-in defaults.
- `#[modal_rust::function]` macro registration (byte-identical to the manual
  `typed!` registry).
- GPU compute via the runner (cudarc + Burn on a T4).

**Next:**

- **GPU via the decorator (P4):** dynamic config from the registry so a
  `gpu="..."` spec flows into `FunctionCreate.resources` (this also drops the
  legacy `--gpu` CLI flag).
- **CLI migration (P9):** migrate the `modal-rust` CLI off Python codegen onto the
  SDK (see the legacy-CLI note below).
- **`.spawn()` / `.map()`** (fire-and-forget + fan-out) and a faster run-time
  compilation cache.

## Legacy / transitional CLI (pending migration)

A `modal-rust` CLI exists (`crates/modal-rust-cli`) but it still uses the **old**
mechanism: it generates Python shims and shells out to the official `modal` CLI.
It is **not** the new programmatic flow and is pending migration onto the SDK
(P9). Use the `App`/`Function` API above for the current programmatic path. The
CLI is kept only for transitional parity until the migration lands.

## Design stances

1. **Direct-execution-first; Sandbox is a documented fallback.** Try the core path
   on normal Modal Functions (`@app.function`) FIRST — runtime compile in a
   Function body is the central thing to prove. If a normal Function-body build
   proves infeasible for a step, iterate to a Modal Sandbox for that step and
   record the decision. Sandboxes are a fallback explicitly on the table, not a ban.
2. **The build boundary is the product** *(the hard, non-negotiable invariant).*
   `run` compiles Rust *in the function body* at execution time; `deploy` compiles
   *at image-build time* and the deployed runtime executes only the prebuilt
   binary — never `cargo`. This holds whether the build runs in a Function body or
   a Sandbox.
3. **Prefer static dispatch.** Favor compile-time polymorphism (`enum`, generics /
   `impl Trait`, marker types, `cfg`) over `dyn Trait`; reach for `dyn` only for
   genuinely open/unbounded sets. (The handler registry is the one open set; it
   erases user functions to `fn` pointers, not `Box<dyn>`.)

## Examples

| Example | Shows |
| --- | --- |
| [`examples/orchestrate`](./examples/orchestrate/) | The facade flow: `.local()` (offline) + `.remote()` + deploy/call |
| [`examples/add`](./examples/add/) | Defining functions via a manual `modal_registry()` (+ every error kind) |
| [`examples/add-macro`](./examples/add-macro/) | Defining functions via `#[modal_rust::function]` + inventory |
| [`examples/cuda-vector-add`](./examples/cuda-vector-add/) | GPU compute via cudarc (Tier 0, driver-only) |
| [`examples/burn-add`](./examples/burn-add/) | GPU compute via Burn/CubeCL (Tier 1, CUDA-only) |

## How this repo is organized

| File | Role |
| --- | --- |
| [`project.md`](./project.md) | Product goal, architecture stance, runtime contract, phases |
| [`TASKS.md`](./TASKS.md) | User-editable workpad queue (the active phase) |
| [`AGENTS.md`](./AGENTS.md) | Orchestration rules for agents |
| [`WORKING.md`](./WORKING.md) | Execution loop, gates, verification, review policy |
| [`workpads/`](./workpads/) | Per-phase tasks, knowledge, references |
| [`workpads/architecture/boundaries.md`](./workpads/architecture/boundaries.md) | Crate layout, runner protocol, run-vs-deploy boundary |

Phases: `research -> architecture -> prototype (add e2e) -> gpu-compute ->
ergonomics -> shim-backend (programmatic SDK)`. Each validates one boundary.

## Working on it

Run the next task with the `/next` command (see `.claude/commands/next.md`). Two
multi-agent workflows live in `.claude/workflows/`:

- `refine-plan.js` — adversarially refine a workpad plan until it is sound.
- `implement.js` — implement and adversarially verify the next milestone task.
