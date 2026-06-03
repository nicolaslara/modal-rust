# Prototype Spec — the `add` function end to end

POC scope for the **prototype** workpad (Phase 3, milestones M0–M9). Grounds the
`add` walking skeleton in the locked decisions of
[`../architecture/research-synthesis.md`](../architecture/research-synthesis.md).
When this doc and the synthesis disagree, the synthesis wins.

## Objective

Prove the whole `modal-rust` core path on the smallest possible function — `add`
— by validating one boundary at a time (M0→M9). The deliverable is a working
walking skeleton, not a complete product: `add` written as an ordinary Rust
library function, run remotely with a **runtime build**, deployed with a
**build-time build**, and called on the deployed Function returning
`{"sum":42}` — with the run-vs-deploy build boundary observably proven. The
design stances apply: the build boundary is the hard invariant;
direct-execution-first (try a normal `@app.function`, with a Modal Sandbox as a
documented fallback if a Function-body build proves infeasible); prefer static
dispatch.

## Vision

A user writes plain Rust:

```rust
// examples/add/src/lib.rs
pub fn add(input: AddInput) -> anyhow::Result<AddOutput> {
    Ok(AddOutput { sum: input.a + input.b })
}

pub fn modal_registry() -> Registry {
    Registry::new().function("add", typed!(add))   // typed! yields a bare fn pointer (static dispatch)
}
```

They never own `main()` and never write Modal Python. The CLI owns a fixed
~15-line `src/bin/modal_runner.rs` template whose `main()` is
`modal_rust_runtime::run_cli(user_crate::modal_registry())`. The runner exposes
the frozen seam:

```text
modal_runner --entrypoint add ( --input-json '<json>' | --input-file <path> | --input-stdin )
ok:    {"ok":true,"value":{"sum":42}}
error: {"ok":false,"error":{"kind":"<kind>","message":"...","details":<json|null>,"backtrace":"..."}}
```

`details` is an **optional, additive** field carrying the **wrapped user error**:
for `function_error` (the user's `Err` wrapped on the top-level `RunnerError`
enum), `message` is the Display/anyhow chain and `details` is the serialized user
error when its type is `Serialize` (else `null`). It is `null`/absent for the
framework kinds.

stdout carries **exactly one** JSON envelope; all cargo/rustc/user diagnostics go
to stderr. The same runner binary serves both paths — only **where it is built**
differs:

```text
modal-rust run add      -> add_local_dir(copy=False) + cargo build IN THE FUNCTION BODY (runtime)
modal-rust deploy add   -> add_local_dir(copy=True)  + run_commands(cargo build) AT IMAGE-BUILD (deploy)
modal-rust call add     -> deployed runtime EXECs /app/modal_runner only — never cargo
```

v0 authoring uses **generated Python shims + the official `modal` CLI** as the
known-good control path (per synthesis §2.7); `modal-rs` is confined to the
later `call` path behind a validated flag and is out of scope for this gate.

## Prototype Minimum (the `add` function)

The irreducible spine that makes the gate pass. All four steps operate on the
**single** `add` function returning `{"sum":42}` for input `{"a":40,"b":2}`.

1. **Write a lib fn.** `examples/add/src/lib.rs` defines `add(AddInput) ->
   anyhow::Result<AddOutput>` and `modal_registry()` registering it as `"add"`
   via `Registry::new().function("add", typed!(add))`. The runtime crate
   (`crates/modal-rust-runtime`) provides `Registry`
   (`BTreeMap<&'static str, HandlerFn>`), the `typed!` macro, `HandlerFn`
   (`fn(&[u8]) -> Result<Vec<u8>, RunnerError>`, static dispatch — no
   `Box<dyn>`), the codec, the runner protocol, and `run_cli`. *(M0)*
2. **Run it remotely with a runtime build.** `modal-rust run add --input
   '{"a":40,"b":2}'` generates `dev_app.py`, which mounts the source with
   `add_local_dir(copy=False)` on a `rust:<pin>-slim` + `add_python='3.12'`
   image, then **in the function body** runs `cargo build --release --bin
   modal_runner` and execs it. One `modal run` invocation shows BOTH the cargo
   output AND `{"ok":true,"value":{"sum":42}}`. *(M2→M4→M5)*
3. **Deploy it with a build-time build.** `modal-rust deploy add` generates
   `deploy_app.py`, which copies the source into an image **layer**
   (`add_local_dir(copy=True)`), runs `cargo build --release` via `run_commands`
   at **image-build** time, and bakes `/app/modal_runner` into the image. The
   `cargo build` lines appear in the deploy/build log. *(M7)*
4. **Call the deployed fn and get `{"sum":42}`.** `modal-rust call add --input
   '{"a":40,"b":2}'` invokes the deployed Function; its body execs **only**
   `/app/modal_runner --entrypoint add --input-file /tmp/in.json` and returns
   `{"sum":42}`. The CALL log contains **no** cargo/compilation lines. *(M8)*

Wrapping all four behind one `modal-rust` binary (`run`/`deploy`/`call`/`doctor`),
as a pure shim generator, is the final minimum step. *(M9)*

## MVP additions

In-scope for the prototype workpad but beyond the irreducible spine:

- **All five error kinds** (`decode_error`, `unknown_entrypoint`,
  `function_error`, `encode_error`, `panic`) with frozen schema, correct exit
  codes, frozen precedence (JSON-parse → entrypoint → decode → call → encode),
  and panic capture via hook + `catch_unwind`. `function_error` is the user's
  error **wrapped** on the top-level `RunnerError` enum (`message` = Display/anyhow
  chain; optional `details` = the serialized user error when `Serialize`). Build
  asserted NOT `panic = "abort"`. *(M0; §2.2, §2.6)*
- **CLI arg routing proven** end to end: a CLI-passed value reaches the function
  body via a `@app.local_entrypoint()` (a bare `@app.function` does not auto-bind
  flags). *(M1; §2.4)*
- **Mount write-probe** resolving whether `add_local_dir(copy=False)` mounts
  read-only or writable — this gates M4's build location. *(M2; §2.4)*
- **Toolchain + Python coexistence** on one image: `cargo`, `rustc`, `python`
  versions plus `which -a python python3`, and the bare-image negative control.
  *(M3; §2.4)*
- **Source-edit reactivity:** edit `add` to `a+b+1`, re-run → 43, revert → 42,
  with no redeploy. *(M5)*
- **Cargo-cache Volume** as a best-effort dev-iteration speedup (`CARGO_HOME` on
  a Volume; `CARGO_TARGET_DIR` stays `/tmp/target` unless benchmarked
  net-positive). Best-effort, **not** a gate dependency; a null result is an
  acceptable deliverable. *(M6; §2.4, §2.6)*
- **`modal-rust doctor`** preflight (creds, `modal` CLI, pinned versions,
  `panic=abort` detection) reusing the M0 structured error model. *(M9; §2.7)*

## Deferred

Out of scope for the prototype gate; tracked elsewhere:

- **GPU / CUDA / Burn** (M10–M13) — the `gpu-compute` workpad.
- **Proc-macros** (`#[modal_rust::function]`, `inventory` registry) and the
  **PyO3/maturin** bridge — the `ergonomics` workpad. The future proc-macro
  generates the same monomorphized wrapper + an `inventory` registration of the
  `fn` pointer (or a static `match` dispatch table). `HandlerFn` is already frozen
  codec-neutral (`&[u8]`) with a reserved `typed_async!` (same `fn`-pointer shape)
  so these stay additive.
- **`modal-rs` as the default `call` invoke mode** — wired only behind a
  validated `--use-modal-rs` flag after a non-scalar round-trip smoke (§4 Q3).
- **CBOR/msgpack wire format** — the `Codec` seam is already neutral; a
  `--input-format` is additive later (§4 Q4).
- **Cargo cache promotion to `CARGO_TARGET_DIR` on a Volume**, multi-developer
  shared caches, and Volume v2 (§4 Q6).
- **Deploy hardening:** dependency-prebuild caching layer and `--vendor`
  (`cargo vendor`) hermetic builds — documented, applied only if egress fails
  (§2.5).
- **Web endpoints** (any public/authenticated HTTP surface) — none in v0; the
  deployed `add` is callable only via `Function.from_name().remote()` / the
  generated `call` shim (§4 Q2).
- **Large/binary payloads** beyond scalar JSON envelopes (the ~100 MB gRPC
  ceiling); route via Volume/object storage later (§2.5, Residual Risk #8).

## Prototype Gate

The gate **passes** when, recorded with evidence in `knowledge.md`:

1. `add` runs via **`modal-rust run add --input '{"a":40,"b":2}'`** and a single
   `modal run` invocation shows the **runtime build** (cargo output) AND
   `{"ok":true,"value":{"sum":42}}` from the freshly built `modal_runner`.
2. `add` is callable via **`modal-rust call add --input '{"a":40,"b":2}'`**
   against a deployed Function, returning `{"sum":42}`.
3. **The build boundary is proven both ways:** `cargo build` appears in the
   `run` function-body log and in the `deploy`/image-build log, and is **absent**
   from the `call` log; the deployed result is stable until an explicit redeploy
   (edit local source → call still returns the old value; redeploy → new value,
   with the new cargo build only in the new deploy's build log).

(Best-effort items — the M6 cache speedup — do **not** block the gate; a null
cache result is acceptable per WORKING.md.)

## Non-Goals

Hard exclusions for this POC (echoing the design stances — the build boundary is
the hard, non-negotiable invariant):

- **Direct-execution-first; Sandbox is a documented fallback.** Try every step on
  a normal `@app.function` first; the happy path uses no Sandbox. A Modal Sandbox
  is a documented fallback (not a ban): if a Function-body build proves infeasible
  for a step, iterate to a Sandbox-based build and record it (see M4's fallback
  branch). The build boundary holds whether the build runs in a Function body or a
  Sandbox.
- **No proc-macros yet.** The manual registry
  (`Registry::new().function("add", typed!(add))`) must work end to end first;
  macros are added later and must compile to the same registry shape (same
  monomorphized `fn`-pointer wrapper) without changing the runner protocol.
- **No local binary upload.** The runner is built remotely — in the function
  body for `run`, in the image layer for `deploy`. The prototype never
  cross-compiles or uploads a host-built binary.
- **The deployed runtime never compiles.** The deployed `add` Function execs only
  the prebuilt `/app/modal_runner`; it never invokes `cargo`, mounts no source,
  and mounts no cargo-cache Volume. `cargo build` in deploy/build logs, never in
  call logs.
- **No GPU, no Burn, no PyO3, no public web endpoint** in this workpad (see
  Deferred).
