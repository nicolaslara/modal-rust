# modal-rust

A **Rust-on-Modal function runtime**. Write ordinary Rust library functions, run
them on [Modal](https://modal.com) during development, deploy them as persistent
Modal Functions, and invoke them remotely — including on GPUs.

Burn (Rust ML) is one downstream consumer, not the core abstraction. The core
problem is **Rust function discovery + remote dispatch + project packaging**,
with a clean split between **build-at-run-time** (dev) and **build-at-deploy-time**
(prod).

> Status: **early prototype.** The walking skeleton runs: `examples/add` compiles
> and executes on Modal, and the central belief — *a normal Modal Function can
> `cargo build` mounted Rust source at runtime* — is validated (prototype M0–M4).
> Built one boundary at a time via a file-backed workpads workflow; see
> `project.md`, `TASKS.md`, and `workpads/prototype/`.

## The idea

```text
user crate (library)        # registered Rust functions, normal lib.rs — no required main()
        |
generated runner binary     # links the user crate, dispatches by name
        |
remote Modal container      # runs the runner, calls the function by name
```

The intended user experience:

```bash
modal-rust run add    --project examples/add --input-json '{"a":2,"b":40}'
modal-rust deploy add --project examples/add --app modal-rust-add-poc
modal-rust call   add --app modal-rust-add-poc --input-json '{"a":2,"b":40}'
```

The runner contract everything compiles down to:

```text
/app/modal_runner --entrypoint add --input-json '{"a":2,"b":40}'
-> {"ok":true,"value":{"sum":42}}
```

Failure returns a structured envelope (five frozen error kinds). `function_error`
means the user's error is **wrapped** on the top-level `RunnerError` enum:
`message` = the Display/anyhow chain, optional additive `details` = the serialized
user error when its error type is `Serialize` (else `null`):

```text
-> {"ok":false,"error":{"kind":"function_error","message":"...","details":null,"backtrace":"..."}}
```

## Try it

**Prerequisites:** a Rust toolchain ([`rustup`](https://rustup.rs)) and the Modal
CLI authenticated — `pip install modal && modal token new` (or an existing
`~/.modal.toml`). The Modal CLI reads your credentials itself; nothing to copy.

**Test it locally** (offline — no Modal, no cost):

```bash
cargo test --workspace
# run the function through the runner, locally:
cargo run --bin modal_runner -- --entrypoint add --input-json '{"a":40,"b":2}'
# -> {"ok":true,"value":{"sum":42}}
```

**Run it on Modal** (the real thing — your source is mounted and compiled *inside*
the container, then executed):

```bash
modal run workpads/prototype/dev_app.py::main
# builds examples/add in the Modal container, prints {"ok":true,"value":{"sum":42}}
```

It defaults to `add` with `{"a":40,"b":2}`; override either:

```bash
modal run workpads/prototype/dev_app.py::main --entrypoint add --input-json '{"a":2,"b":3}'
```

> These are the raw generated shims under `workpads/prototype/`. The
> `modal-rust run/deploy/call` CLI (prototype M9) will wrap them so you won't call
> `modal` directly. Deploy/call commands are added here once M7–M8 land.

## Design stances

1. **Direct-execution-first; Sandbox is a documented fallback.** Try the core
   path on normal Modal Functions (`@app.function`) FIRST — runtime compile in a
   Function body is the central thing to prove. If a normal Function-body build
   proves infeasible for a step, **iterate to a Modal Sandbox** for that step and
   record the decision. Sandboxes are a fallback explicitly on the table, not a
   ban.
2. **The build boundary is the product** *(the hard, non-negotiable invariant).*
   `run` compiles Rust *in the function body* at execution time; `deploy`
   compiles *at image-build time* and the deployed runtime executes only the
   prebuilt binary — never `cargo`. This holds whether the build runs in a
   Function body or a Sandbox.
3. **Prefer static dispatch.** Favor compile-time polymorphism (`enum`, generics /
   `impl Trait`, marker types, `cfg`) over `dyn Trait`; reach for `dyn` only for
   genuinely open/unbounded sets.

v0 uses generated Python Modal shims (a private implementation detail) shelling
out to a Rust subprocess runner. Proc-macros and PyO3/maturin are later
ergonomics, validated only after the manual path works.

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
ergonomics`. Each validates one boundary.

## Working on it

Run the next task with the `/next` command (see `.claude/commands/next.md`). Two
multi-agent workflows live in `.claude/workflows/`:

- `refine-plan.js` — adversarially refine a workpad plan until it is sound.
- `implement.js` — implement and adversarially verify the next milestone task.
