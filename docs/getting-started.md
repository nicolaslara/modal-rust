# Getting Started with modal-rust

Write a Rust function, annotate it with `#[modal_rust::function]`, and run or deploy
it on [Modal](https://modal.com) — no Python per project, no `modal` CLI required.

This is the newcomer walkthrough. For the maintainer-level internals (crate map,
frozen seams, data flows) see `docs/architecture.html`.

One-line mental model for a Modal-Python user:

```text
@app.function()            ->  #[modal_rust::function]
def add(a, b): ...             pub fn add(a: i64, b: i64) -> anyhow::Result<i64> { ... }

add.local(2, 3)            ->  app.add(2, 3).local()?
add.remote(2, 3)           ->  app.add(2, 3).remote().await?
```

---

## 1. Prerequisites

- **A Rust toolchain** (stable). Install via <https://rustup.rs>.
- **A Modal account + API token** — needed only for `.remote()` / `deploy` / `call`.
  `.local()` needs nothing (no account, no network).
  1. Sign up at <https://modal.com>.
  2. Create a token (the Modal dashboard → Settings → API Tokens, or `modal token new`
     if you have the Python `modal` CLI).
  3. Make the token available to modal-rust in **either** form:
     - `~/.modal.toml` (the standard Modal config file), **or**
     - the `MODAL_TOKEN_ID` and `MODAL_TOKEN_SECRET` environment variables.

modal-rust never logs or commits your token.

---

## 2. Zero → a new crate

A modal-rust crate is a **pure library**: a `Cargo.toml` with **one** modal
dependency and a library with your `#[function]`s. There is **no runner binary to
write** — the `modal-rust` CLI generates it for you.

`Cargo.toml`:

```toml
[package]
name = "my-app"
version = "0.0.0"
edition = "2021"

[lib]
name = "my_app"
path = "src/lib.rs"

[dependencies]
modal-rust = { git = "https://github.com/nicolaslara/modal-rust" }
serde = { version = "1", features = ["derive"] }
anyhow = "1"
```

You add only `modal-rust` (plus `serde`/`anyhow` for your handler types). No
`[[bin]]`, no `src/bin/`. The macro routes its generated code through the
`modal-rust` facade — the serde_derive pattern — so there is no
`modal-rust-runtime` or `inventory` to add, and **no dependency rename**.

---

## 3. Write your first function

`src/lib.rs`:

```rust
use modal_rust::function;

#[function]
pub fn add(a: i64, b: i64) -> anyhow::Result<i64> {
    Ok(a + b)
}
```

The macro:

- emits your function unchanged (it stays a plain Rust `fn` you can call directly);
- generates a nameable `add::Input { a, b }` / `add::Output` (= `i64`) pair;
- registers the entrypoint via `inventory` (no `modal_registry()` builder to keep);
- adds a typed `app.add(2, 3)` method to `App` (on a generated `AddCall` trait).

---

## 4. No runner to write

That is the whole crate. There is **no `src/bin/modal_runner.rs`** — the
`modal-rust` CLI builds your library and generates the runner binary for it on the
fly (it mirrors your crate's own `modal-rust` dependency so it resolves identically).
You never write `main()` and never name any internal `__private` path.

**Optional — bring your own runner.** If you want to wrap startup (initialize a
tracing subscriber, read a config file, warm a cache) or run the binary directly
without the CLI, ship a one-line `src/bin/modal_runner.rs`:

```rust
modal_rust::modal_runner!(my_app);
```

Pass your **library crate's** name (`my_app`) so its registered functions are linked
into the runner binary (a `[[bin]]` does not auto-link its package's lib). Add the
matching `[[bin]] name = "modal_runner"` target to `Cargo.toml`. The CLI
**auto-detects** this bin and uses it as-is instead of generating one — so existing
hand-written runners keep working unchanged. See
[`examples/own-runner-bin`](../examples/own-runner-bin) for the full pattern.

---

## 5. Run it locally (`.local()`)

`.local()` runs the handler in-process — **zero Modal, zero network**. Bring the
typed method into scope with one `use`:

```rust
use modal_rust::App;
use my_app::AddCall;            // or `use my_app::*;`

fn demo() -> anyhow::Result<()> {
    let app = App::local();
    let sum: i64 = app.add(2, 3).local()?;
    assert_eq!(sum, 5);
    Ok(())
}
```

No Modal credentials are needed for `.local()`, so you can prove it in a unit test:

```bash
cargo test -p my-app
```

The CLI's `doctor` is an OFFLINE preflight for your project — it checks your Modal
credentials and (with `--rust`) that `cargo`/`rustc` are present and the release
`panic` profile is `unwind` (the strategy the runner needs to capture panics):

```bash
modal-rust doctor --rust --project .
# modal-rust doctor — preflight (OFFLINE) …
```

(If you ship your own runner bin — section 4 — you can also exercise it directly:
`cargo run -p my-app --bin modal_runner -- --entrypoint add --input-json
'{"a":2,"b":3}'` prints `{"ok":true,"value":5}`, exactly what Modal runs remotely.)

---

## 6. Run it on Modal (`.remote()`)

`.remote()` uploads this crate, builds it **in the Modal function body at call
time**, and runs the freshly built runner. The first call to a fresh container
compiles the dependency tree (this can take a few minutes — a cold build); later
calls reuse the warm container and a persistent cargo cache.

> **Enable the `client` feature for orchestration code.** The default `modal-rust`
> dependency is light (no gRPC client) — perfect for the function-only library above
> and for `modal-rust run`/`deploy` (the CLI enables the client for you, section 8).
> The moment *your own code* calls `App::connect`/`.remote()`/`.spawn()`/`.map()`/
> `deploy`/`call`, add the feature on that crate:
>
> ```toml
> modal-rust = { git = "...", features = ["client"] }
> ```
>
> Keeping orchestration in a `[[bin]]` or `tests/` (with `client` on a
> `[dev-dependencies]` edge) lets your `#[function]` library stay light, so the
> in-container runner build stays fast. Without the feature these calls still
> compile — they return a clear "requires the `client` feature" error at runtime.

```rust
use modal_rust::App;
use my_app::AddCall;

async fn demo() -> anyhow::Result<()> {
    let app = App::connect("my-rust-app").await?;   // reads ~/.modal.toml / MODAL_TOKEN_*
    let sum: i64 = app.add(2, 3).remote().await?;
    assert_eq!(sum, 5);
    Ok(())
}
```

**Package auto-detect:** `.remote()` builds `cargo build -p my-app` automatically —
the `#[function]` macro captured your crate's package name (`CARGO_PKG_NAME`) at
compile time. You do not set `MODAL_RUST_PACKAGE`; it remains only as an override.

`.map(...)` fans out across many inputs (results in input order) and `.spawn(...)`
is fire-and-forget (returns a handle to poll later) — both hang off the same typed
method.

---

## 7. Deploy & call

`deploy` builds **once** into a persistent Modal app (the build runs at image-build
time, baked into the image); `call` invokes the prebuilt runner with **no rebuild**.

```rust
use modal_rust::{App, DeployConfig};
use my_app::AddCall;

async fn demo() -> anyhow::Result<()> {
    let app = App::connect("my-rust-app").await?;
    let deployed = app.deploy_with(DeployConfig::for_app("my-rust-app-prod")).await?;
    let sum: i64 = app.call(&deployed.name, "add", my_app::add::Input { a: 2, b: 3 }).await?;
    assert_eq!(sum, 5);
    Ok(())
}
```

This is the **run-vs-deploy build boundary**: `run` builds at call time (source
mounted, `cargo` in the function body); `deploy` builds at image-build time and the
deployed runtime only ever executes the prebuilt `/app/modal_runner` — it never runs
`cargo`.

---

## 8. The CLI alternative

You can drive the same paths from the `modal-rust` CLI without writing the calling
code (the CLI builds your crate, reads `modal_runner --describe`, and invokes over
gRPC):

```bash
cargo install --git https://github.com/nicolaslara/modal-rust --package modal-rust-cli

modal-rust run add    --project . --input '{"a":40,"b":2}'
modal-rust deploy add --project . --app my-rust-app-prod
modal-rust call add   --app my-rust-app-prod --input '{"a":40,"b":2}'
```

---

## 9. Core concepts

- **App** — your handle. `App::local()` is the offline, in-process handle (the only
  one `.local()` needs); `App::connect(name)` is the online handle for
  `.remote()`/`deploy`/`call`.
- **Function** — one registered entrypoint. The macro adds a typed `app.<name>(..)`
  method; you can also resolve one by string with `app.function("<name>")`.
- **Registry / inventory** — how functions are discovered. `#[function]` registers
  each entrypoint via `inventory`; the runner (generated by the CLI, or your own
  `modal_runner!()` bin) collects them into the registry it dispatches against.
- **The run-vs-deploy build boundary** — `.local()` runs in-process; `.remote()`
  builds in the function body at call time (ephemeral); `deploy`+`call` builds once
  into a persistent image and never rebuilds. This boundary is the product invariant.

---

## 10. Python → Rust cheat sheet

| Modal Python | modal-rust |
| --- | --- |
| `@app.function()` | `#[modal_rust::function]` |
| `@app.function(gpu="T4", timeout=1800)` | `#[modal_rust::function(gpu = "T4", timeout = 1800)]` |
| `@app.function(secrets=[Secret.from_name("s")])` | `#[modal_rust::function(secrets = ["s"])]` |
| `@app.function(volumes={"/data": Volume.from_name("v")})` | `#[modal_rust::function(volumes = ["/data=v"])]` |
| `def add(a, b): return a + b` | `pub fn add(a: i64, b: i64) -> anyhow::Result<i64> { Ok(a + b) }` |
| `add.local(2, 3)` | `app.add(2, 3).local()?` |
| `add.remote(2, 3)` | `app.add(2, 3).remote().await?` |
| `add.map([...])` | `app.add(0, 0).map([add::Input { a: 1, b: 1 }, ...]).await?` |
| `add.spawn(2, 3)` | `app.add(2, 3).spawn().await?` |
| `modal run app.py` | `modal-rust run add --project .` |
| `modal deploy app.py` | `modal-rust deploy add --project . --app <name>` |
| `Function.from_name(app, "add").remote(...)` | `app.call("<app>", "add", input).await?` |

See `docs/PARITY.md` for the full feature-by-feature comparison and the gaps still
open.

---

## 11. Troubleshooting

**Platform-dependent dependencies and the cudarc pattern**

The local describe build compiles your crate on your laptop (macOS, Windows, Linux)
to extract the entrypoint manifest. This can fail on any OS where a *compile-time*
dependency (a `-sys` crate, a `build.rs` script probing system headers, or a
`#[cfg(target_os = ...)]` gate on an entrypoint) is unavailable — even though the
remote Modal build on Linux would succeed. **The recommended pattern:** use
compile-everywhere, link-at-runtime crates (the [cudarc](https://github.com/coreylowman/cudarc)
pattern for CUDA — it compiles on any OS and JIT-links CUDA at runtime). This keeps
`.local()` working on all platforms and the describe build offline-capable. Avoid
`#[cfg]`-gating `#[function]` registrations across OS boundaries; if you must, either
embrace the tooling's escape hatches (`--no-local-build`, `--manifest`) or prepare to
use them once. The examples [`cuda-vector-add`](../examples/cuda-vector-add) and
[`burn-add`](../examples/burn-add) follow this pattern.

If the local build does fail, skip it with one of the two escape hatches (with
`--no-local-build` the decorator can't be read, so the inline `--gpu/--timeout/...`
flags supply the config; on a normal build those same flags just override it):

```bash
# Supply config inline and skip the local build:
modal-rust run <entrypoint> --project . \
  --no-local-build --gpu T4 --timeout 600 --input '{...}'

# Or hand-write a describe@1 manifest and pass it:
echo '{"schema":"modal-rust/describe@1","entrypoints":[{"name":"<entrypoint>","config":{"gpu":"T4","timeout_secs":600}}]}' > manifest.json
modal-rust run <entrypoint> --project . --manifest manifest.json --input '{...}'

# Or let Modal build + describe the crate for you (cached locally afterwards):
modal-rust run <entrypoint> --project . --remote-describe --input '{...}'
```

A bare local build failure never auto-runs the remote path — it prints the
diagnostic (which names `--remote-describe`) and exits, so there's no surprise
remote build. `--verify-manifest` builds the manifest both locally and on Modal and
fails if they disagree (catches a `#[cfg]`-gated entrypoint that's invisible to the
local build).

- **`no method named 'add' found for struct App`** — the typed method needs the
  generated trait in scope. Add `use my_app::AddCall;` (named after the function) or
  `use my_app::*;` at the call site.
- **`.remote()` builds the wrong package / "package not found"** — the package is
  auto-detected from the `#[function]` macro, so this should not happen for a
  decorated crate. If you have a manual (no-macro) registry, set `MODAL_RUST_PACKAGE`
  to your package name, or pass an explicit `RemoteConfig`.
- **`NotConnected`** — you called `.remote()`/`deploy`/`call` on an offline app.
  Use `App::connect(name)` (which reads your Modal token) instead of `App::local()`.
- **First `.remote()` call hangs for minutes** — that is the cold in-body
  `cargo build` compiling your dependency tree. Subsequent calls reuse the warm
  container and the persistent cargo cache. A too-small `timeout = ...` can starve
  the first cold build.
- **`panic` error kind missing / process aborts on a handler panic** — modal-rust
  needs the unwind panic strategy to capture panics. Do not set `panic = "abort"`
  for the runner profile.
- **Transient Modal capacity errors** (a call hangs, or "could not fetch task data")
  — these are transient; retry. They are not a modal-rust bug.

---

## Next steps

- Browse [`examples/`](../examples): `quickstart` (this guide — a pure library, no
  runner bin), `cli-workflow` (drive a pure-library crate entirely from the CLI),
  `own-runner-bin` (the bring-your-own-runner escape hatch), `add` (the manual
  no-macro reference), `add-macro` (the full decorator surface), `orchestrate`
  (a `.local()`/`.remote()`/`deploy` tour), and the GPU examples
  (`cuda-vector-add`, `burn-add`).
- Read `docs/PARITY.md` for Modal feature parity, and `docs/architecture.html` for
  the internals.
