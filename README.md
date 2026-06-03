# modal-rust

A **Rust-on-Modal function runtime**. Write ordinary Rust library functions, run
them on [Modal](https://modal.com) during development, deploy them as persistent
Modal Functions, and invoke them remotely — including on GPUs.

Burn (Rust ML) is one downstream consumer, not the core abstraction. The core
problem is **Rust function discovery + remote dispatch + project packaging**,
with a clean split between **build-at-run-time** (dev) and **build-at-deploy-time**
(prod).

> Status: **prototype complete (M0–M9).** `examples/add` runs end-to-end on Modal
> via the `modal-rust` CLI: the dev path compiles the mounted source *in the
> function body* and reflects local edits; the deploy path compiles *once at
> image-build time* with the deployed runtime executing only the prebuilt binary
> (never `cargo`). The build boundary is proven both ways. Remaining: GPU
> (Stage 6, M10–M13). Built one boundary at a time via a file-backed workpads
> workflow; see `workpads/prototype/`.

## Try it

**Prerequisites:** a Rust toolchain ([`rustup`](https://rustup.rs)) and the Modal
CLI authenticated — `pip install modal && modal token new` (or an existing
`~/.modal.toml`). Run everything from the repo root.

**Install the `modal-rust` CLI** (M9 — built and validated):

```bash
cargo install --path crates/modal-rust-cli   # puts `modal-rust` on your PATH
modal-rust doctor                            # offline preflight: modal CLI + credentials (--rust also checks cargo/rustc)
```

**Run / deploy / call** (defaults to the `add` entrypoint in `examples/add`):

```bash
modal-rust run    add --input '{"a":40,"b":2}'   # dev: mounts source, compiles IN the container -> {"ok":true,"value":{"sum":42}}
modal-rust deploy add                            # prod: compiles once at image-build time, bakes the binary (app: modal-rust-add-poc)
modal-rust call   add --input '{"a":40,"b":2}'   # invoke the deployed function -> {"ok":true,"value":{"sum":42}} (no recompile)
```

**Local only** (offline — no Modal):

```bash
cargo test --workspace
cargo run --bin modal_runner -- --entrypoint add --input-json '{"a":40,"b":2}'   # -> {"ok":true,"value":{"sum":42}}
```

<details><summary><b>Under the hood</b> — what the CLI generates</summary>

`modal-rust` writes Python shims under `.modal-rust/generated/` and calls the
official `modal` CLI. The same shims live (committed) under `workpads/prototype/`
and can be driven directly without installing the CLI:

```bash
modal run    workpads/prototype/dev_app.py::main      # == modal-rust run add
modal deploy workpads/prototype/deploy_app.py         # == modal-rust deploy add
modal run    workpads/prototype/call_app.py::main     # == modal-rust call add
```

</details>

## The idea

```text
user crate (library)        # registered Rust functions, normal lib.rs — no required main()
        |
generated runner binary     # links the user crate, dispatches by name
        |
remote Modal container      # runs the runner, calls the function by name
```

The user experience — the `modal-rust` CLI (milestone **M9, built** — see
**Try it** above):

```bash
modal-rust run add    --input '{"a":2,"b":40}'
modal-rust deploy add --app modal-rust-add-poc
modal-rust call   add --app modal-rust-add-poc --input '{"a":2,"b":40}'
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
