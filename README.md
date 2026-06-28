# modal-rust

Rust functions on [Modal](https://modal.com), with a Rust-native authoring and
calling API.

> [!WARNING]
> **Work in progress.** `modal-rust` is early and the public API is still moving.
> A broad surface is implemented and live-proven — `.local()` / `.remote()` /
> `.spawn()` / `.map()`, `deploy` + `call`, GPU (cudarc + Burn on a T4), the full
> decorator config, `#[cls]` (load-once-serve-many + memory snapshot),
> `#[endpoint]` web endpoints, and `Dict` / `Queue` — but this is not ready to treat
> as stable infrastructure yet. See [`docs/PARITY.md`](docs/PARITY.md) for an honest,
> code-verified Have/Partial/Missing inventory vs the Modal Python client.

`modal-rust` lets you write normal Rust functions, register them as Modal
entrypoints, and call them three ways:

- `.local()` runs the handler in-process with no Modal credentials.
- `.remote()` uploads your Rust crate to Modal and builds it during the function
  invocation, which keeps the development loop close to the source tree.
- `deploy` + `call` builds once into a persistent Modal app and calls the
  prebuilt runner without rebuilding.

The default command-line path is the `modal-rust` CLI. The library API also
includes a first-party Rust client for Modal's control plane, so Rust code can
drive local, remote, and deployed calls directly.

## Quickstart

A whole modal-rust crate is a single `modal-rust` dependency and a plain
`#[function]` — no runner binary to write. The `modal-rust` CLI generates the
runner automatically. Write the function:

```rust quickstart
use modal_rust::function;

/// Add two integers — the whole function. `#[function]` generates the JSON
/// input/output plumbing, registers the entrypoint via `inventory`, and adds a
/// typed `app.add(2, 3)` method to `App` (brought into scope with one `use`:
/// `use quickstart::AddCall;`, or `use quickstart::*;`).
#[function]
pub fn add(a: i64, b: i64) -> anyhow::Result<i64> {
    Ok(a + b)
}
```

That is the entire authoring surface — no `src/bin/modal_runner.rs`, no
`__private`, no environment variable. Run it three ways via the typed
`app.add(2, 3)` method (brought into scope with one `use` of the generated
`AddCall` trait):

```rust
use modal_rust::App;
use quickstart::AddCall; // or `use quickstart::*;`

async fn run() -> anyhow::Result<()> {
    // `.local()` runs the handler in-process — no Modal, no network.
    let app = App::local();
    let sum: i64 = app.add(2, 3).local()?;
    assert_eq!(sum, 5);

    // `.remote()` uploads THIS crate and builds it on Modal at call time. The
    // package to build (`cargo build -p quickstart`) is auto-detected from the
    // macro — no `MODAL_RUST_PACKAGE` to set.
    let app = App::connect("my-rust-app").await?;
    let sum: i64 = app.add(2, 3).remote().await?;
    assert_eq!(sum, 5);
    Ok(())
}
```

That is the entire newcomer surface: **one `modal-rust` dependency, no rename, no
`__private`, no environment variable, no runner binary.** The
[`examples/quickstart`](examples/quickstart) crate is exactly this code (a
drift-guard test asserts the `rust quickstart` block above is its real source), so
`cargo test -p quickstart` proves it compiles and runs.

See the [step-by-step Getting Started guide](docs/getting-started.md) for
prerequisites (a Modal account + token), the run/deploy walkthrough, a
Python→Rust cheat sheet, and troubleshooting.

## Install

`modal-rust` is on crates.io. It is currently a **pre-release**, so pin the
version explicitly — `cargo add` / `cargo install` skip pre-releases by default.
One dependency covers both the macro and the manual authoring paths:

```toml
[dependencies]
modal-rust = "0.1.0-alpha.1"
serde = { version = "1", features = ["derive"] }
anyhow = "1"
```

Or track the latest unreleased code from GitHub instead:

```toml
modal-rust = { git = "https://github.com/nicolaslara/modal-rust" }
```

The `#[modal_rust::function]` macro routes its generated code through the
`modal-rust` facade, so you never add `modal-rust-runtime` or `inventory`
directly — just like `serde_derive` routes through `serde`.

### The `client` feature (talking to Modal vs. authoring)

The default `modal-rust` dependency is **light**: it pulls in only ~9 crates and
compiles in a couple of seconds. That is everything you need to author
`#[function]`s, call `.local()`, and let the `modal-rust` CLI run/deploy your crate.

The gRPC control-plane client (tonic/hyper/prost/reqwest — ~150 crates) lives behind
a **non-default `client` feature**. You only need it when your *own code* talks to
Modal directly — `App::connect(..)`, `.remote()`/`.spawn()`/`.map()`, `deploy`/`call`,
or the offline `dry_run`/`dump_deploy_manifest` dump:

```toml
# Function-only crate (authoring + .local() + `modal-rust run`/`deploy`): nothing to add.
modal-rust = "0.1.0-alpha.1"

# Orchestration code (your binary/tests call .remote()/deploy/connect): add the feature.
modal-rust = { version = "0.1.0-alpha.1", features = ["client"] }
```

If you forget the feature, the talk-to-Modal methods still compile — they return a
clear error at runtime telling you to add `features = ["client"]`. **You almost never
need it:** the `modal-rust` CLI enables `client` itself, so a normal `#[function]`
library that you `modal-rust run`/`deploy` stays light. Keeping orchestration calls in
a `[[bin]]` or `tests/` (with `client` on a `[dev-dependencies]` edge) keeps the
library itself tonic-free, so the in-container runner build stays fast too.

For live Modal calls, configure Modal credentials with either `~/.modal.toml` or
the `MODAL_TOKEN_ID` and `MODAL_TOKEN_SECRET` environment variables.

## CLI Usage

The `modal-rust` binary ships in the **`modal-rust-cli`** crate — the `modal-rust`
crate itself is the library, so `cargo install modal-rust` does **not** work (same
split as `wasmtime` / `wasmtime-cli`). Install the CLI from crates.io (explicit
version while it is a pre-release):

```bash
cargo install modal-rust-cli --version 0.1.0-alpha.1
```

Or from GitHub, or a local checkout you are editing:

```bash
cargo install --git https://github.com/nicolaslara/modal-rust --package modal-rust-cli
cargo install --path crates/modal-rust-cli
```

The CLI drives the first-party SDK directly — it builds your crate, generates the
`modal_runner` binary for it (or uses one you ship), reads its `--describe`
manifest, and creates/invokes the function over gRPC. There is no generated Python
and no dependency on the `modal` CLI; just configure Modal credentials.
(For platform-dependent dependencies, see §11 Troubleshooting in the getting-started
guide — the cudarc pattern keeps the describe build working everywhere.)

Your crate stays a **pure library** — a single `modal-rust` dependency and your
`#[function]`s, no runner binary. The examples below drive
[`examples/cli-workflow`](examples/cli-workflow), which is exactly that: a plain
`#[function(name = "summarize")]` library with no `src/bin/`.

Check your machine first:

```bash
modal-rust doctor --rust --project examples/cli-workflow
```

Run a registered Rust function remotely on Modal:

```bash
modal-rust run summarize \
  --project examples/cli-workflow \
  --input '{"text":"the quick brown fox"}'
```

Deploy the project as a persistent Modal app:

```bash
modal-rust deploy summarize \
  --project examples/cli-workflow \
  --app modal-rust-cli-workflow-example
```

Call the deployed function without rebuilding:

```bash
modal-rust call summarize \
  --app modal-rust-cli-workflow-example \
  --input '{"text":"the quick brown fox"}'
```

For your own project, point `--project` at your crate (the one with the
`#[function]`s) — it defaults to the current directory, so from inside your crate
you can omit `--project` entirely. The CLI auto-generates the runner; there is no
binary to write. `--input` accepts inline JSON or `@path/to/input.json`.

The CLI validates `--input` **locally before any Modal round-trip**: it decodes
your JSON against the function's expected input shape and, on a mismatch, fails
fast with the expected shape (a `decode_error`) instead of building and running on
Modal only to fail there. A typo in the entrypoint name or a missing runner
degrades gracefully to the normal remote check rather than a false rejection.

### Overriding config per run (`--gpu`/`--timeout`/`--cpu`/`--memory`)

The `#[function]` decorator is the source of truth for config, but you can override
a field for a single invocation with an inline flag — it works on the normal path
(the decorator is still read, then the flag wins for that field):

```bash
# Override just the GPU for this run; everything else comes from the decorator.
modal-rust run summarize --project examples/cli-workflow \
  --gpu A100 --input '{"text":"the quick brown fox"}'
```

These also apply on `modal-rust deploy` (overriding the selected entrypoint).

### When the local build can't run (`--no-local-build` / `--manifest`)

By default the CLI compiles your crate **locally** (a quick debug build) to read
its entrypoint manifest before talking to Modal. That local build can fail on a
machine where a *compile-time* dependency is unavailable — e.g. a linux-only
`-sys` crate on macOS — even though the remote Modal build (on Linux) would
succeed. When that happens the CLI prints a diagnostic naming the cause and points
you at two ways to skip the local build:

```bash
# 1. Skip the local build; the decorator can't be read, so supply config inline
#    (without --no-local-build these same flags just override the built manifest).
modal-rust run summarize --project examples/cli-workflow \
  --no-local-build --gpu T4 --timeout 600 \
  --input '{"text":"the quick brown fox"}'

# 2. Skip the local build; supply a hand-written describe@1 manifest file.
cat > manifest.json <<'JSON'
{"schema":"modal-rust/describe@1",
 "entrypoints":[{"name":"summarize","config":{"gpu":"T4","timeout_secs":600}}]}
JSON
modal-rust run summarize --project examples/cli-workflow \
  --manifest manifest.json \
  --input '{"text":"the quick brown fox"}'

# 3. Let Modal produce the manifest: build + run `--describe` ON MODAL (Linux),
#    cached locally so repeat runs are instant. No manual config to write.
modal-rust run summarize --project examples/cli-workflow \
  --remote-describe \
  --input '{"text":"the quick brown fox"}'
```

A bare local build failure never auto-escalates — it prints the diagnostic (which
names `--remote-describe`) and exits, so you never pay a surprise remote build. To
catch the subtler case where a crate compiles on *both* sides but registers a
*different* set of entrypoints (e.g. a `#[cfg(target_os = "linux")]`-gated
function), `--verify-manifest` builds the manifest both locally and on Modal and
fails on any divergence:

```bash
modal-rust run summarize --project examples/cli-workflow --verify-manifest \
  --input '{"text":"the quick brown fox"}'
```

All of these also work on `modal-rust deploy`. The best fix, though, is to keep
your crate compile-everywhere in the first place — see the cudarc pattern in the
[Getting Started troubleshooting](docs/getting-started.md#11-troubleshooting).

## Library API

There are two ways to register a function: the `#[modal_rust::function]`
attribute macro (the default, ergonomic path) and a manual `Registry` builder.
Both compile down to the same typed handler shape, so the calling API
(`App`/`Function`, `.local()`/`.remote()`/`deploy`+`call`, `.map`/`.spawn`) is
identical for both.

### Authoring with `#[modal_rust::function]` (the macro path)

Annotate a plain Rust function. The macro generates the input/output plumbing,
registers the function through `inventory` (so there is no `modal_registry()`
builder to maintain), and adds a typed method to `App` named after the function —
so **you never hand-write `AddInput`/`AddOutput` structs unless you want to**:

```rust
use modal_rust::function;

#[function]
pub fn add(a: i64, b: i64) -> anyhow::Result<i64> {
    Ok(a + b)
}
```

`App::local()` builds an in-process app over every annotated function, and each
one gets a typed method — there is no input or output type to name at the call
site. The typed method lives on a generated `AddCall` trait (named after the
function); bring it into scope with one `use` of your crate's `AddCall`, or a glob
(`use my_crate::*;`) to bring in every function's trait at once:

```rust
use modal_rust::App;
use my_crate::AddCall; // or `use my_crate::*;` — required for the typed `app.add(..)`

async fn example() -> anyhow::Result<()> {
    let app = App::local();

    // `.local()` runs the handler in-process — no Modal, no network.
    let sum: i64 = app.add(2, 3).local()?;
    assert_eq!(sum, 5);

    // `.remote()` uploads the crate and runs it on Modal; `.spawn()`
    // (fire-and-forget) and `.map()` (fan-out) hang off the same typed method.
    let app = App::connect("my-rust-app").await?;
    let sum: i64 = app.add(2, 3).remote().await?;
    assert_eq!(sum, 5);
    Ok(())
}
```

Under the hood the macro still generates a nameable `add::Input { a, b }` /
`add::Output` pair, so you can also call dynamically by string when you need to:
`app.function("add").remote(add::Input { a: 2, b: 3 })`.

The `modal-rust` CLI generates the `modal_runner` binary automatically for pure
library crates — you do not need to write a `src/bin/modal_runner.rs` at all.
Just run via the CLI:

```bash
cargo run -p modal-rust-cli -- run add --project path/to/my_crate --input '{"a":2,"b":3}'
```

If you want to bring your own runner (for advanced use cases, or to run the binary
directly without the CLI), add `src/bin/modal_runner.rs` with one line:

```rust
modal_rust::modal_runner!(my_crate);
```

This expands to the runner `main()` and runs the frozen runner protocol; you never
write `main()` or touch any internal `__private` path. See
[`examples/own-runner-bin`](examples/own-runner-bin) for an example of the
bring-your-own-runner pattern.

If you would rather define **named, documented I/O types** yourself, pass a single
serializable struct in and return one out. The macro detects this form and
compiles to a byte-identical handler — it is what the manual `Registry` path and
the call/deploy examples below use:

```rust
use modal_rust::function;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct AddInput {
    pub a: i64,
    pub b: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AddOutput {
    pub sum: i64,
}

#[function]
pub fn add(input: AddInput) -> anyhow::Result<AddOutput> {
    Ok(AddOutput {
        sum: input.a + input.b,
    })
}
```

The decorator is the config. Everything Modal needs to create the function lives
on the attribute — `gpu`, `cpu`, `memory`, `timeout`, `retries`, `schedule`,
autoscaling (`min_containers`/`max_containers`/`buffer_containers`/`scaledown_window`),
`cache`, `secrets`, `required_keys`, `env`, `volumes`, and a per-function `image` — and
is read from the registry at call time (there are no extra CLI flags):

```rust
use modal_rust::function;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct TrainInput {
    pub epochs: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TrainOutput {
    pub ok: bool,
}

#[function(
    gpu = "T4",                     // also: "A100", "A100-80GB", "H100:4", ...
    cpu = 2.0,                      // CPU cores (float); -> milli_cpu = int(1000 * cpu)
    memory = 4096,                  // requested RAM in MiB
    timeout = 1800,                 // wall-clock seconds
    retries = 3,                    // auto-retry N times (fixed interval); or the struct
                                    // form Retries(max_retries = 5, backoff_coefficient = 2.0,
                                    // initial_delay = 0.5, max_delay = 30.0) for custom backoff
    schedule = Cron("0 9 * * 1"),   // run on a cron cadence after deploy (or Period(days = 1))
    cache = false,                  // opt out of the cargo build cache (default: on)
    secrets = ["my-api-key"],       // named Modal secrets, injected as env vars
    required_keys = ["API_KEY"],    // assert these keys exist on the named secrets
    env = { "REGION" = "us-east" }, // an INLINE secret (Secret.from_dict), injected as env vars
    volumes = ["/data=my-dataset"], // a Modal Volume `my-dataset` mounted at /data
    image = Image(                  // a PER-FUNCTION image just for THIS entrypoint
        base = "nvidia/cuda:12.4.1-devel-ubuntu22.04", // override the base image
        install_rust = true,        // install the Rust toolchain on that base
        apt = ["libssl-dev"],       // apt packages (prepended to path-level steps)
        pip = ["numpy"],            // pip packages
        run = ["echo built"],       // arbitrary RUN commands
    ),
)]
pub fn train(input: TrainInput) -> anyhow::Result<TrainOutput> {
    let _key = std::env::var("API_KEY")?;        // from the secret
    std::fs::write("/data/checkpoint", b"...")?; // persisted on the volume
    Ok(TrainOutput { ok: true })
}
```

`image = Image(..)` is the per-function analogue of Python's
`app.function(image=..)`: it makes *this* entrypoint build on the declared base.
`base`/`install_rust` **override** the path-level default base (the same base you
can otherwise set globally via `RemoteConfig.base_image`/`.install_rust` or
`MODAL_RUST_BASE_IMAGE`/`MODAL_RUST_INSTALL_RUST`), while `apt`/`pip`/`run`
**prepend** to any path-level image steps. A bare `Image()` is a no-op. This is how
a single GPU function declares its CUDA-devel base inline — see
[Run vs Deploy](#run-vs-deploy) for the GPU `run` mechanics.

Resolve a `Function` handle by name from the inventory registry and call it three
ways:

```rust
use modal_rust::{App, DeployConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct AddInput {
    pub a: i64,
    pub b: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AddOutput {
    pub sum: i64,
}

async fn example() -> anyhow::Result<()> {
    let app = App::local();

    // `.local()` runs the handler in-process — no Modal, no network.
    let out: AddOutput = app
        .function("add")
        .local(AddInput { a: 40, b: 2 })?;

    assert_eq!(out.sum, 42);

    // `.remote()` uploads the crate and builds it in the Modal function body.
    let app = App::connect("my-rust-app").await?;

    let out: AddOutput = app
        .function("add")
        .remote(AddInput { a: 40, b: 2 })
        .await?;

    assert_eq!(out.sum, 42);

    // `deploy` builds once into a persistent app; `call` invokes with no rebuild.
    let deployed = app
        .deploy_with(DeployConfig::for_app("my-rust-app-prod"))
        .await?;

    let out: AddOutput = app
        .call(&deployed.name, "add", AddInput { a: 40, b: 2 })
        .await?;

    assert_eq!(out.sum, 42);
    Ok(())
}
```

`map` fans out across many inputs (results come back in input order), and
`spawn` is fire-and-forget — it returns a handle immediately that you poll later:

```rust
use modal_rust::App;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct AddInput {
    pub a: i64,
    pub b: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AddOutput {
    pub sum: i64,
}

async fn example(app: &App) -> anyhow::Result<()> {
    let sums: Vec<AddOutput> = app
        .function("add")
        .map(vec![AddInput { a: 1, b: 1 }, AddInput { a: 40, b: 2 }])
        .await?; // -> [{sum:2}, {sum:42}], in input order

    let call = app.function("add").spawn(AddInput { a: 40, b: 2 }).await?; // returns immediately
    let out: AddOutput = call.get().await?; // -> {sum:42}
    Ok(())
}
```

### Authoring with a manual `Registry` (the library path)

If you do not want the attribute macro, build a `Registry` by hand with `typed!`.
This needs only the `modal-rust` dependency. The `typed!` wrapper this produces is
byte-for-byte identical to what the macro emits:

```rust
use modal_rust::{typed, Registry};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct AddInput {
    pub a: i64,
    pub b: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AddOutput {
    pub sum: i64,
}

pub fn add(input: AddInput) -> anyhow::Result<AddOutput> {
    Ok(AddOutput {
        sum: input.a + input.b,
    })
}

pub fn modal_registry() -> Registry {
    Registry::new().function("add", typed!(add))
}
```

Then hand the registry to `App` instead of using `App::local()`:

- `App::local_with_registry(modal_registry())` for offline `.local()` calls, and
- `App::connect_with_registry("my-rust-app", modal_registry()).await?` for live
  `.remote()` / deploy / call.

Everything else — `.local()`/`.remote()`/`deploy`+`call` and `.map`/`.spawn` — is
exactly as shown for the macro path. Non-macro users set the same per-function
config (`gpu`, `timeout`, `cache`, `secrets`, `volumes`) on `RemoteConfig` /
`DeployConfig` instead of on the decorator.

### Stateful classes (`Cls`) — load once, serve many

Some workloads pay a large fixed cost before they can do any work: loading model
weights, opening a connection pool, warming a cache. A plain `#[function]` pays that
cost on *every* call. `#[modal_rust::cls]` pays it **once per warm container** and
reuses the result — mirroring Python's `@app.cls` + `@enter` + `@method`. You write a
plain struct for the state and a `#[cls(..)]` `impl` block for the behavior:

With `use modal_rust::cls;` in scope, the authored surface is exactly the
[`examples/stateful-class`](examples/stateful-class) crate (this block is drift-guarded
against it by `examples/stateful-class/tests/readme_drift.rs` — a stale README is a test
failure):

```rust cls
pub struct Embedder {
    model: Model,
}

#[cls(gpu = "T4", timeout = 600)] // CLASS-LEVEL default config -> inherited by every #[method].
impl Embedder {
    /// Runs ONCE per warm container (mirrors `@modal.enter()`). Loads the embedding
    /// model and returns the built value; the macro moves it into a process-lifetime
    /// singleton, so this expensive step happens a single time no matter how many method
    /// calls a warm container serves. (`Model::load` is offline + CPU-only here, but it
    /// stands in for the real "read weights from disk / warm a GPU" cost.)
    #[enter]
    fn load() -> anyhow::Result<Self> {
        Ok(Embedder {
            model: Model::load(),
        })
    }

    /// Embed `text` into a fixed-width unit-length vector, reusing the already-loaded
    /// model by `&self`. `#[method(gpu = "A10G")]` OVERRIDES the class default gpu
    /// (`T4`) for this method only; `timeout = 600` is still inherited from `#[cls]`.
    #[method(gpu = "A10G")]
    fn embed(&self, text: String) -> anyhow::Result<Vec<f32>> {
        Ok(self.model.embed(&text))
    }

    /// Report the model's output dimensionality, reusing the loaded model by `&self`.
    /// Bare `#[method]` — inherits BOTH `gpu = "T4"` and `timeout = 600` from `#[cls]`.
    #[method]
    fn dim(&self) -> anyhow::Result<usize> {
        Ok(self.model.dim())
    }
}
```

The macro generates an `EmbedderHandle` and an `EmbedderCls` extension trait (carrying
`app.embedder()`); bring them into scope with one glob (`use my_crate::*;`) and call a
method like any other typed function. `#[enter]` runs lazily on the first method call
in a warm container; every later call — same or different method — reuses the same
in-memory singleton, so the model is **loaded once and served many** (the runner adds
an additive `modal_runner --serve` loop to keep the process warm across inputs):

```rust
use modal_rust::App;
use my_crate::*; // brings the generated `EmbedderCls` trait (and `app.embedder()`) into scope

async fn example() -> anyhow::Result<()> {
    let app = App::local();

    // `.local()` runs in-process — no Modal, no network.
    let d: usize = app.embedder().dim().local()?;

    // `.remote()` runs on Modal; the model loads once per warm container, served many.
    let app = App::connect("my-rust-app").await?;
    let v: Vec<f32> = app.embedder().embed("hi".into()).remote().await?;
    Ok(())
}
```

Each `#[method]` becomes its OWN Modal entrypoint under the dotted `"<Class>.<method>"`
name (`Embedder.embed` / `Embedder.dim`), carrying its fully-resolved class-default +
method-override config. Two methods with *different* effective config become different
Modal functions (different containers), so warm load-once reuse holds across methods
that share the same effective config (the common all-inherit case). The dotted entrypoint
name is what you pass to the CLI — run the live [`examples/stateful-class`](examples/stateful-class)
method on a GPU with:

```bash
modal-rust run Embedder.embed --project examples/stateful-class --input '{"text":"hello"}'
```

Like the other talk-to-Modal surface, `.remote()` / `deploy` / `call` on a `Cls` method
need the `client` feature (see [The `client` feature](#the-client-feature-talking-to-modal-vs-authoring));
a `Cls`-authoring crate that only uses `.local()` (and is run via the CLI) stays light.
**Deferred (Shape B):** `#[exit]` (the marker is reserved but currently emits a
`compile_error`) and `modal.parameter` class params — for now inject config with
`#[cls(secrets = [..])]` + `std::env` reads in `#[enter]`.

#### Pay `#[enter]` once *ever* — `enable_memory_snapshot`

A plain `#[cls]` loads once per *warm* container, but a **cold** container (after
scale-to-zero) re-runs `#[enter]` from scratch — so for a genuinely expensive build
that cold-start cost recurs forever. Add one flag,
`#[cls(enable_memory_snapshot = true)]`, and a **deployed** app runs `#[enter]` once,
Modal snapshots the loaded process, and every later container — cold ones included —
**restores** the already-built state instead of re-running the build. This extends
load-once-serve-many across cold starts, not just within one warm container. Mirrors
Modal's `enable_memory_snapshot` (`Cls`, CPU). See
[`examples/snapshot-class`](examples/snapshot-class).

Three things to know:

- **Deploy-only.** Modal only snapshots *deployed* apps, so the flag takes effect on
  `deploy`, not `run`. On `deploy` it rides into the wire field Modal reads to
  snapshot the class; on `run` it is suppressed and the wire stays byte-identical to a
  non-snapshot `#[cls]` (which falls back to ordinary load-once, once per warm
  container). So you `run` to iterate and `deploy` to get the cold-start win.
- **The prime — fails loud by default.** A deploy makes `#[enter]` run *inside*
  Modal's snapshot window (otherwise it would run after restore — too late). If the
  prime **fails** (your `#[enter]` returned an error or panicked, or the prime path
  broke), the container init **fails visibly at deploy time** — a broken snapshot
  must never hide as a silent perf cliff where every cold start quietly re-pays the
  load. To opt into degrading instead (log the failure, fall back to lazy `#[enter]`
  on the first request), set `MODAL_RUST_SNAPSHOT_BEST_EFFORT=1` at deploy time (or
  `DeployConfig::snapshot_best_effort`).
- **Frozen-`#[enter]`-state caveat.** A snapshot freezes the *entire* process state at
  the moment `#[enter]` finishes and restores it on every cold container. Anything
  `#[enter]` captures — **environment variables, the wall clock / timestamps, RNG
  seeds, open connections / file handles** — is frozen identically across all
  restores. Do **not** capture per-container or time-sensitive values in `#[enter]`
  and expect them to differ across restores; per-container work that must be fresh
  belongs in the method body (which runs on every call).

v0 is CPU-only and `#[cls]`-only — `#[function(enable_memory_snapshot = true)]` is a
compile error. The GPU snap/restore split and a `#[restore]` hook are tracked in
[`docs/ROADMAP.md`](docs/ROADMAP.md); see [`docs/PARITY.md`](docs/PARITY.md) for status.

### Web endpoints — `#[endpoint]`: expose a function over HTTP

`#[modal_rust::endpoint(method = "POST")]` is `#[function]` plus an HTTP URL — the
`@modal.fastapi_endpoint` analogue (`WEBHOOK_TYPE_FUNCTION`). The handler stays an
ordinary Rust fn — same auto-IO, same decorator vocabulary (`gpu`/`timeout`/
`secrets`/…) — and on **deploy** Modal wraps the in-container callable in a FastAPI
app and assigns a stable URL. **No web-framework dependency in your crate.** With
`use modal_rust::endpoint;` in scope, the authored surface is exactly the
[`examples/web-endpoint`](examples/web-endpoint) crate (this block is drift-guarded
against it by `examples/web-endpoint/tests/readme_drift.rs` — a stale README is a
test failure):

```rust endpoint
/// The summary a POST returns — the response body, as JSON. Every field is computed
/// by the frequency model in `extractive.rs`, not fixed.
#[derive(Debug, Serialize, Deserialize)]
pub struct Summary {
    /// The selected sentences, joined in their original order.
    pub summary: String,
    /// How many sentences the summary kept.
    pub sentences_kept: usize,
    /// How many sentences the input text held.
    pub sentences_total: usize,
    /// How many words the input text held.
    pub words_total: usize,
}

/// Boil `text` down to its `max_sentences` most representative sentences. A normal
/// handler — IDENTICAL to a `#[function]` — but `#[endpoint]` ALSO exposes it over
/// HTTP on deploy: POST `{"text":"..","max_sentences":2}` (the auto-IO input JSON)
/// to the deployed URL and the response body is the [`Summary`] JSON. The typed call
/// surface keeps working alongside the URL: `app.summarize(text, 2).local()`.
#[endpoint(method = "POST")]
pub fn summarize(text: String, max_sentences: usize) -> anyhow::Result<Summary> {
    anyhow::ensure!(max_sentences > 0, "max_sentences must be at least 1");
    let sentences = extractive::split_sentences(&text);
    anyhow::ensure!(
        !sentences.is_empty(),
        "text holds no sentences to summarize"
    );
    let picked = extractive::pick_top(&sentences, max_sentences);
    Ok(Summary {
        sentences_kept: picked.len(),
        sentences_total: sentences.len(),
        words_total: extractive::word_count(&text),
        summary: picked.join(" "),
    })
}
```

The HTTP contract is the auto-IO contract: the request body **is** the function's
input JSON (the same shape `--input` takes, here `{"text":"..","max_sentences":2}`)
and the response body is the output JSON (the `Summary`). Errors come back as
`{"kind","message"}` JSON — **422** for a body that fails to decode, **500** for a
handler error. `method` is **required** — one of `"GET" | "POST" | "PUT" | "DELETE" |
"PATCH"` (a missing or invalid method is a compile error, never a deploy surprise).

```bash
cd examples/web-endpoint
modal-rust deploy summarize --app modal-rust-web-endpoint
curl -X POST "https://<workspace>--modal-rust-web-endpoint-summarize.modal.run" \
  -H 'content-type: application/json' \
  -d '{"text":"Rust guarantees memory safety. The borrow checker proves it.","max_sentences":1}'
```

Three things to know:

- **The URL is deploy-only (v0).** `modal-rust run summarize --input '{..}'` still
  works as a normal one-shot typed call, but the webhook is **suppressed** on the RUN
  boundary — the wire stays byte-identical to a plain `#[function]` and no URL exists.
  Deploying auto-installs `fastapi[standard]` into the deploy image (Modal requires
  FastAPI in the image for FUNCTION webhooks) — nothing to declare.
- **Public by default** (matching Modal): anyone with the URL can call it. Opt into
  auth with `#[endpoint(method = "POST", requires_proxy_auth = true)]` — Modal then
  rejects requests lacking the `Modal-Key`/`Modal-Secret` proxy-auth header pair
  *before* they reach the container.
- **The deployed endpoint is HTTP-only (v0).** The fn remains a normal function for
  `.local()` and `modal-rust run` (webhook suppressed), but Modal's worker wraps a
  webhook function's in-container callable in an ASGI app, so the typed envelope path
  (`.remote()` / `modal-rust call`) against the *deployed* app is rejected
  (live-verified). Want both surfaces on one deploy? Use Modal's own idiom: a plain
  `#[function]` for compute plus a thin `#[endpoint]` fn that calls it.

v0 is one method + one request/response per free fn. Routing, multiple methods,
streaming, and websockets are the `#[web_server]` follow-up (below), and
`#[endpoint]` on a `#[cls]` method is a compile error for now — see
[`docs/ROADMAP.md`](docs/ROADMAP.md) and [`docs/PARITY.md`](docs/PARITY.md) §8.

### Full HTTP apps — `#[web_server]`: own the port, serve forever

Where `#[endpoint]` maps ONE request/response onto a Modal function webhook,
`#[modal_rust::web_server(port = ..)]` is a **raw port proxy** (Modal
`WEBHOOK_TYPE_WEB_SERVER`). The annotated fn is `(port: u16) -> anyhow::Result<()>`
(sync or `async`) that LAUNCHES your own HTTP server bound to `port` and BLOCKS,
serving forever. On `modal-rust deploy` Modal assigns a public URL and forwards ALL
traffic to that port — so **multi-route apps, SSE streaming, and websockets work
as-is** (it is your server, not a per-request adapter). DEPLOY-only in v0.

```rust
use modal_rust::web_server;

#[web_server(port = 3000, gpu = "T4", memory = 16384,
    image = Image(base = "nvidia/cuda:12.4.1-devel-ubuntu22.04", install_rust = true))]
async fn serve(port: u16) -> anyhow::Result<()> {
    burn_lm_http::App::new(port).serve().await.map_err(|e| anyhow::anyhow!(e.to_string()))
}
```

Every other argument is the shared `#[function]` vocabulary
(`gpu`/`memory`/`timeout`/`image`/`secrets`/`volumes`), plus `startup_timeout = <secs>`
for how long Modal waits for the port to come up. The live dogfood —
wrapping the burn-lm-http GPU inference server and load-testing it — is
[`examples/burn-lm-bench`](examples/burn-lm-bench) (GPU/CUDA-only, excluded from
`default-members`).

## How It Works

Every user function is erased into a static handler table:

```text
Registry = BTreeMap<&'static str, fn(&[u8]) -> Result<Vec<u8>, RunnerError>>
```

The generated runner has a stable process contract:

```text
/app/modal_runner --entrypoint <name> --input-json <json>
```

On success it writes exactly one JSON envelope to stdout:

```json
{"ok":true,"value":{"sum":42}}
```

On failure it writes one structured error envelope:

```json
{"ok":false,"error":{"kind":"function_error","message":"...","details":null,"backtrace":"..."}}
```

The five error kinds are:

```text
decode_error | unknown_entrypoint | function_error | encode_error | panic
```

This runner protocol is the boundary between user Rust code and Modal execution.
Cargo output, Rust diagnostics, and user logs go to stderr so stdout stays
machine-readable.

## Run vs Deploy

`modal-rust` deliberately keeps development and production builds separate:

| Flow | Build timing | Modal app | Runtime behavior |
| --- | --- | --- | --- |
| `.local()` | No build beyond your local Cargo build | None | Calls the handler in-process |
| `.remote()` | Builds inside the Modal function body | Ephemeral | Uploads source, runs `cargo build`, then executes the runner |
| `deploy` + `call` | Builds during image creation | Persistent | Calls a prebuilt `/app/modal_runner`; no source upload or Cargo build at call time |

That split is the core product invariant. The development path optimizes for
fast iteration from local Rust source; the deployed path optimizes for stable
invocation without rebuilding.

### Rust-specific tradeoffs (things you do not think about in Python)

In Python, Modal pre-builds the image, so by the time your function runs there is
**no compile step** — the code is already importable. Rust is compiled, and that
moves where the build happens:

- **The `run` path compiles in the function container.** `.remote()` (and
  `modal-rust run`) upload your source and run `cargo build` *inside* the Modal
  function body, then execute the runner. So a cold call pays a compile, and a
  heavy crate (`burn`/`cubecl`, large transitive trees) can exhaust RAM during
  `rustc`/`nvcc` and get killed — Modal reports this as
  `GENERIC_STATUS_TERMINATED`. The fix is to give the build room with a higher
  `memory =` on the decorator (e.g. `memory = 8192` for the Burn example); the
  [Build cache](#build-cache) then makes warm runs skip the recompile entirely.
- **`deploy` moves the build to image-build time.** `deploy` + `call` runs
  `cargo build` **once**, at image creation, with full build resources, and bakes
  a prebuilt `/app/modal_runner` into a persistent image. There is no per-cold-
  container rebuild and no source upload at call time — the right choice for a
  heavy crate or a hot production endpoint.
- **The `client` feature shrinks the build massively.** Keeping the gRPC client
  behind the non-default `client` feature (see [Install](#the-client-feature-talking-to-modal-vs-authoring))
  collapses a ~150-crate tree to ~9, so the in-container `run` build stays small
  and fast. A normal `#[function]` library that you `modal-rust run`/`deploy` never
  compiles tonic at all.
- **`--serve` keeps one runner warm.** With `modal_runner --serve` (used by the
  `#[cls]` path) the process stays warm across inputs, so a `#[cls]` `#[enter]`
  runs **once per warm container** rather than once per call — load-once,
  serve-many. Add `#[cls(enable_memory_snapshot = true)]` and a *deployed* class
  pays `#[enter]` **once ever**: Modal snapshots the loaded process and restores it
  on every container start, cold ones included (see
  [Pay `#[enter]` once *ever*](#pay-enter-once-ever--enable_memory_snapshot)).
- **GPU does NOT imply deploy.** A GPU function runs fine on the `run` path **given
  a CUDA-devel base** for the toolkit (NVRTC/cudart) plus a high enough `memory =`
  for the in-body build. Set the base either per function with
  `#[function(image = Image(base = "nvidia/cuda:..-devel", install_rust = true))]`,
  or path-wide via `RemoteConfig.base_image`/`.install_rust` (or
  `MODAL_RUST_BASE_IMAGE`/`MODAL_RUST_INSTALL_RUST`). Because the heavy CUDA crate
  still compiles in the body on `run`, `deploy` is **recommended** so that build
  happens once at image-build time — but it is a performance/efficiency choice, not
  a requirement. *(Verified 2026-06-08: `modal-rust run burn_add` ran end-to-end on a
  T4 via the `image = Image(..)` decorator + `memory = 16384` — `8 GB` OOMs the
  CubeCL build, `16 GB` clears it. `deploy` stays recommended for heavy GPU crates to
  avoid the per-cold-container rebuild and the high runtime memory.)*

**When to run vs deploy.** Reach for **`run`** while iterating from local source:
it is the tightest edit→call loop, the build cache keeps warm runs near-instant,
and for light crates the in-body compile is a couple of seconds. Switch to
**`deploy`** when the build is expensive (heavy/GPU crates, where an in-body
compile risks `GENERIC_STATUS_TERMINATED` without enough `memory =`), when you want
a stable persistent endpoint that never rebuilds at call time, or when a
schedule/autoscaling config should stay live independent of your laptop.

### Build cache

To keep the `.remote()` development loop fast, the in-container Cargo build is
cached **on by default**: `CARGO_HOME` (and the build `target/`) are persisted as
a single compressed archive on a Modal Volume, unpacked on container start and
repacked on exit. A warm run skips the registry fetch and recompilation — on a
heavy crate this turns a cold rebuild into a `Fresh` no-op. A cache miss only ever
costs time; it never changes the result. Disable it per function with
`#[function(cache = false)]`, or globally with `MODAL_RUST_NO_CACHE=1`. (`deploy`
builds once at image-build time, so the cache applies to the `run` path only.)

### Speeding up local builds

The default `modal-rust` build is **light**: the gRPC client (tonic/hyper/prost/
reqwest) is behind the non-default `client` feature (see [Install](#the-client-feature-talking-to-modal-vs-authoring)),
so authoring a `#[function]` crate, `.local()`, and the in-container `modal_runner`
build never compile it — that collapses a ~150-crate tree down to ~9 and a ~30s cold
build to ~2-3s. A `rust-toolchain.toml` pins the channel so Cargo's fingerprints (and
any shared cache) stay stable across runs.

For teams that want even faster cold builds across machines, point Cargo at a shared
[`sccache`](https://github.com/mozilla/sccache):

```bash
cargo install sccache
export RUSTC_WRAPPER=sccache   # in your shell profile or CI env
```

That is purely local hygiene — it never changes wire bytes or build outputs.

### Environment variables

Every `MODAL_RUST_*` variable is declared once in `modal_rust::env` (the consts the
facade reads; a drift-guard test keeps every literal in the codebase — including
the Python wrappers — tied to that registry). Boolean knobs parse truthily:
`1`/`true`/`yes`/`on`, case-insensitive. The names are baked into deployed images,
so they never get renamed.

| Variable | Purpose | Read where | Audience |
| --- | --- | --- | --- |
| `MODAL_RUST_PACKAGE` | Cargo package to build/invoke remotely (overrides the macro-detected crate name) | local process (run/deploy config) | public |
| `MODAL_RUST_SOURCE_DIR` | Source dir to upload (default: nearest `[workspace]` `Cargo.toml`, else nearest `Cargo.toml`, else CWD) | local process (run/deploy config) | public |
| `MODAL_RUST_BASE_IMAGE` | Base image for the remote build (default `rust:<ver>-slim`); pair with `MODAL_RUST_INSTALL_RUST` for CUDA bases | local process (run/deploy config) | public |
| `MODAL_RUST_INSTALL_RUST` | Truthy ⇒ install the Rust toolchain into the image (for bases without one) | local process (run/deploy config) | public |
| `MODAL_RUST_NO_CACHE` | Truthy ⇒ disable the run-path build cache (default ON; see [Build cache](#build-cache)) | local process (run config) | public |
| `MODAL_RUST_CACHE_TARGET` | `target/` archiving in the build cache — default ON (fresh containers reuse compiled deps); `0` opts out | local process; an opt-out is baked into the run image `ENV` for the container wrapper | public |
| `MODAL_RUST_DEPLOY_APP` | Stable deploy app name override | local process (deploy config) | public |
| `MODAL_RUST_SNAPSHOT_BEST_EFFORT` | Truthy ⇒ a failed snapshot prime degrades to lazy `#[enter]` instead of failing container init | local process at deploy; baked into the image `ENV` for the deploy wrapper | public |
| `MODAL_RUST_SERVE` | Container-side escape hatch: falsy ⇒ wrappers print build/exec commands instead of running them | container (Python wrappers) | internal |
| `MODAL_RUST_SNAPSHOT_PRIME` | Baked when any entrypoint enables `enable_memory_snapshot`; gates the deploy wrapper's import-time prime | container (deploy wrapper) | internal |
| `MODAL_RUST_RUN_CONFIG_JSON_B64` | Base64 JSON run config the facade bakes for the run wrapper | container (run wrapper) | internal |
| `MODAL_RUST_TEST_SECRET` | Secret key round-tripped by the live secrets/volumes test | live test | test-only |
| `MODAL_RUST_RUNNER` | Overrides the deploy wrapper's `/app/modal_runner` path so its test can exec a stub | container (deploy wrapper test) | test-only |

## Architecture

The workspace is split into focused crates:

| Crate | Purpose |
| --- | --- |
| `modal-rust` | User-facing `App`, `Function`, `.local()`, `.remote()`, deploy, and call API |
| `modal-rust-runtime` | Handler registry, typed wrappers, runner protocol, and error envelopes |
| `modal-rust-macros` | `#[modal_rust::function]` registration macro |
| `modal-rust-sdk` | First-party Rust gRPC client for Modal control-plane operations (optional dep behind the facade's `client` feature) |
| `modal-rust-cli` | Command-line interface for `doctor`, `run`, `deploy`, and `call` |

The facade uses static dispatch where possible. The registry stores function
pointers rather than boxed trait objects, and the macro compiles to the same
typed wrapper shape as manual registration.

## GPU

Request a GPU directly on the function, and it flows into the Modal function's
resources when you `.remote()` / `deploy` it:

```rust
#[function(gpu = "T4")]              // also: "A100", "A100-80GB", "H100:4", ...
pub fn vector_add(input: VecInput) -> anyhow::Result<VecOutput> { /* ... */ }
```

This is proven live on a real GPU, both for a lightweight kernel and a real ML
workload:

- `examples/cuda-vector-add` — `cudarc` Driver API + a precompiled PTX kernel
  (driver-only image), run on a T4 via `.remote()`.
- `examples/burn-add` — a Burn/CubeCL tensor op on CUDA, deployed and called on a
  T4. Because it needs the CUDA toolkit (NVRTC/cudart) at build and run time, the
  image uses a `nvidia/cuda:*-devel` base with the Rust toolchain installed. Set
  that base **either** per function on the decorator —
  `#[function(image = Image(base = "nvidia/cuda:..-devel", install_rust = true))]`
  — **or** path-wide with `MODAL_RUST_BASE_IMAGE` + `MODAL_RUST_INSTALL_RUST=1`
  (or `RemoteConfig`/`DeployConfig`). The example also sets `memory = 8192` so the
  heavy in-body build does not get killed (`GENERIC_STATUS_TERMINATED`). For that
  heavy CUDA build, prefer `deploy` + `call` so it happens once at image-build time.

The GPU spec maps to Modal exactly: `"TYPE[:count]"` (e.g. `"H100:4"`); memory
variants like `"A100-80GB"` pass through as the GPU type.

Right-size plain compute with `cpu` and `memory` on the same decorator: `cpu` is a
number of CPU cores (a float, resolved to milli-cores as `int(1000 * cpu)` exactly
like Modal) and `memory` is requested RAM in MiB. Both default to the server default
when unset, so a bare `#[function]` is unchanged.

## Secrets and Volumes

Attach Modal secrets (injected as environment variables) and persistent volumes
(mounted at a path) the same way — on the function:

```rust
#[function(
    gpu = "T4",
    secrets = ["my-api-key"],          // injected as env vars in the container
    volumes = ["/data=my-dataset"],    // a Modal Volume mounted at /data (persists)
)]
pub fn train(input: TrainInput) -> anyhow::Result<TrainOutput> {
    let key = std::env::var("API_KEY")?;          // from the secret
    std::fs::write("/data/checkpoint", /* ... */)?; // persisted on the volume
    /* ... */
}
```

Everything on `#[function(...)]` — `gpu`, `cpu`, `memory`, `timeout`, `retries`,
`schedule`, autoscaling (`min_containers`/`max_containers`/`buffer_containers`/
`scaledown_window`), `cache`, `secrets`, `required_keys`, `env`, `volumes`, and a
per-function `image` — is sourced from the registry at call time. The decorator is the
config; there are no extra CLI flags. (Non-macro users can set the same fields on
`RemoteConfig` / `DeployConfig`.)

## Dicts and Queues

Share state between functions and callers through named, server-side objects —
`modal_rust::Dict` (key/value) and `modal_rust::Queue` (FIFO), mirroring
`modal.Dict`/`modal.Queue`. Both are app-independent handles behind the `client`
feature; `from_name` creates if missing (idempotent):

```rust
use modal_rust::{Dict, Queue};
use std::time::Duration;

// Dict: heterogeneous values, typed per call. Keys are &str.
let d = Dict::from_name("scores").await?;
d.put("alice", &42_i64).await?;
let v: Option<i64> = d.get("alice").await?;            // None = key absent
let created = d.put_if_absent("alice", &7_i64).await?; // false: already present

// Queue: blocking get with a timeout (None = block forever, ZERO = poll once).
let q = Queue::from_name("jobs").await?;
q.put_many(&[27_u64, 97, 9]).await?;
let next: Option<u64> = q.get(Some(Duration::from_secs(30))).await?; // None = timed out

Dict::delete("scores").await?;                          // by name; irreversible
Queue::delete("jobs").await?;
```

**The Python interop boundary (by design):** values ride a restricted-pickle
codec matching Modal's own Go/JS clients, so *plain data* (str/int/float/bool/
bytes/lists/dicts/structs-as-dicts) round-trips with Python, and `&str` Dict
keys are byte-exact CPython pickle so key lookups work both ways. Pickled
Python custom classes/functions do NOT interop — reading one is a typed codec
error, never a panic or a silent `None`. `get_raw`/`put_raw` are the
bring-your-own-codec escape hatch. v0 is named-only (ephemeral objects,
iteration, and queue partitions/TTL knobs are deferred). See
`examples/dict-kv` and `examples/queue-pipeline`.

## Development

Useful checks:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

Live tests are feature-gated and require Modal credentials:

```bash
cargo test -p modal-rust --features live --test live_remote -- --ignored
cargo test -p modal-rust --features live --test live_deploy -- --ignored
```

## Examples

The `examples/` directory holds runnable, live-proven crates:

| Example | What it shows |
| --- | --- |
| `examples/quickstart` | **(pure library)** The whole newcomer surface: a single `modal-rust` dep and a 3-line `#[function]` — **no runner bin**. Run it with the `modal-rust` CLI, which generates the runner. This is the README quickstart, drift-guarded against the README. |
| `examples/cli-workflow` | **(pure library + CLI)** A plain `#[function(name = "summarize")]` library with no driver and no runner bin, driven entirely by the `modal-rust` CLI: `doctor`, `run`, `deploy`, `call`. |
| `examples/own-runner-bin` | **(bring your own bin)** The escape hatch: ship your own one-line `src/bin/modal_runner.rs` (`modal_rust::modal_runner!(crate);`) when you want to wrap startup. The CLI **auto-detects** it and uses it as-is instead of generating one. |
| `examples/add` | **(manual / no-macro)** The same `add` written by hand — the input struct, the `typed!` registration, and `modal_registry()`, i.e. everything the macro generates for you. A low-level reference; it keeps a hand-written runner named `add-runner`. Plus named entrypoints exercising every runner error kind. |
| `examples/add-macro` | **(macro)** The same `add` in three lines: `#[modal_rust::function] fn add(a, b) -> anyhow::Result<i64>`, called `app.add(2, 3).remote().await?` — the macro generates the input struct, registration, and typed method. Plus the full decorator config (`gpu`/`timeout`/`cache`/`secrets`/`volumes`). |
| `examples/orchestrate` | A tour of the facade driving `add` via `.local()`, `.remote()`, and `deploy`+`call` — through BOTH the manual `App::local_with_registry(modal_registry())` and the macro `App::local()` + typed `app.add(2, 3)` paths. |
| `examples/error-handling` | **(macro)** How a failure crosses the boundary: a plain `anyhow::Result` error is opaque (`details: null`), while a `Serialize` error type rides through as machine-readable `details` the caller can deserialize and branch on. Same frozen `function_error` kind, different `details`. |
| `examples/cuda-vector-add` | **(macro)** A real GPU kernel — `cudarc` Driver API + precompiled PTX — authored with `#[modal_rust::function(gpu = "T4", name = "vector_add")]`; the decorator IS the config, run on a T4 via `.remote()`. |
| `examples/burn-add` | **(macro)** A real ML workload — a Burn/CubeCL tensor op (NVRTC at runtime) authored with `#[modal_rust::function(gpu = "T4", name = "burn_add")]`, deployed and called on a T4. |
| `examples/stateful-class` | **(macro / `Cls`)** Load-once-serve-many: a `#[modal_rust::cls]` impl with `#[enter]` (load an embedding model ONCE per warm container) + `#[method]`s reusing it by `&self`, called `app.embedder().embed("hi".into()).remote().await?`. Each method is its own dotted `Embedder.embed` / `Embedder.dim` entrypoint; live-confirmed on a T4. |
| `examples/snapshot-class` | **(macro / `Cls` / memory snapshot)** Pay `#[enter]` ONCE EVER: a `#[modal_rust::cls(enable_memory_snapshot = true)]` whose `#[enter]` builds a sorted word-concordance index. On a *deployed* app Modal snapshots the loaded process and restores it on every container — cold ones included — so the build is not re-run per cold start. Deploy-only (`run` stays wire-identical); offline tests prove load-once + that the flag rides into the DEPLOY `FunctionCreate` and not the RUN one. |
| `examples/web-endpoint` | **(macro / web endpoint)** Expose a plain function over HTTP with ONE attribute: `#[modal_rust::endpoint(method = "POST")]` on a real frequency-based extractive summarizer. On `modal-rust deploy` Modal wraps the in-container callable in a FastAPI app and assigns a public URL — `curl -X POST` the input JSON, get the output JSON back. The fn stays a normal function for `.local()` and `modal-rust run` (webhook suppressed, wire-identical); the *deployed* endpoint itself is HTTP-only in v0 (Modal's worker ASGI-wraps the callable). Offline tests prove the webhook rides the DEPLOY `FunctionCreate` and not the RUN one. |
| `examples/custom-types` | **(macro / I/O)** Real functions take and return your own `struct`s: derive `Serialize`/`Deserialize`, take a struct, return a struct, and the macro infers the typed I/O from the signature. `score(Player) -> Scored` turns a match record into a score. |
| `examples/ways-to-call` | **(macro / call shapes)** One function (`square(n: i64)`), four invocation patterns side by side — `.local()`, `.remote().await`, `.spawn()` + `.get()`, and `.map([..])` — all through the same typed `app.square(n)` method. The "how do I actually call this" tour. |
| `examples/deploy-and-call` | **(run vs deploy)** The build boundary made concrete: `.remote()` rebuilds in the function body on each cold start, while `deploy` runs `cargo build --release` ONCE at image-build time and bakes the binary in so each `call` skips the rebuild. |
| `examples/fan-out-map` | **(call shapes)** Embarrassingly-parallel scale-out with `.map()`: one `#[function]` mapped over N inputs returns `Vec<Out>` in input order. `analyze` returns a document's word count + estimated reading time. |
| `examples/background-jobs` | **(call shapes)** Fire-and-forget with `.spawn()`: enqueue a job, get a handle back immediately, do other work, then collect the result with `.get(timeout)` — the async-job pattern vs blocking `.remote()`. |
| `examples/spawn-map-foreach` | **(call shapes)** The rest of the map family: `.spawn_map()` (fire-and-forget fan-out) and `.for_each()` (side-effect map that waits and discards results). `notify` sends a per-recipient notification. |
| `examples/cpu-memory` | **(decorator config)** Right-size plain compute with `#[function(cpu = 2.0, memory = 4096)]` (2.0 cores -> 2000 milli-cores, 4096 MiB -> 4 GiB). `crunch` folds a batch of records into a deterministic checksum. |
| `examples/timeout-and-cache` | **(decorator config)** Operational knobs: a function `timeout` plus the on-by-default cargo build cache (`#[function(timeout = 1800, cache = true)]`). `spin` runs N checksum-fold iterations. |
| `examples/retries` | **(decorator config)** Self-healing with an automatic retry policy (`#[function(retries = 5)]`): `fetch` fails the first two attempts and succeeds on the third, with no retry loop in your code. |
| `examples/secrets` | **(decorator config)** Attach a named Modal secret (`#[function(secrets = ["my-api-key"])]`) and read it as an env var inside the function. `check_secret` reports whether `MY_API_KEY` was injected and its length — never the value. |
| `examples/volumes` | **(decorator config)** Mount a Volume (`#[function(volumes = ["/data=my-vol"])]`), write a file, and read it back on the next call — persistent storage across invocations. `record_visit` appends to a log and returns the running count. |
| `examples/autoscaling` | **(decorator config)** Control warm capacity and scale-to-zero (`min`/`max`/`buffer_containers`, `scaledown_window`) for the latency-vs-cost tradeoff. `embed` turns a document into an L2-normalized feature vector — a believable unit of work to scale out. |
| `examples/scheduled-job` | **(decorator config)** A deployed function that runs on a cron cadence with no caller (`#[function(schedule = Cron("0 9 * * 1"))]`): once deployed, Modal triggers `weekly_report` automatically. |
| `examples/dict-kv` | **(shared state / Dict)** Two parties that share only a NAME: a `#[function]` computes Scrabble scores and writes them into `Dict::from_name("dict-kv-scores")`; the caller opens the same name and reads them back typed. Plain data interops with Python by design; a mock-backed test proves the write→read round-trip offline. |
| `examples/queue-pipeline` | **(shared state / Queue)** A producer/consumer pipeline: the caller `put_many`s jobs into a named Queue; a `#[function]` consumer drains it with blocking `get(timeout)` (idle-timeout = batch done) and returns a typed summary of the Collatz stopping times it computed. FIFO + blocking-get proven offline against the mock. |
| `examples/custom-base` | **(image config)** Pick the RUN base image and install the Rust toolchain via the path-level knobs (`RemoteConfig.base_image`/`.install_rust`, or `MODAL_RUST_BASE_IMAGE`/`MODAL_RUST_INSTALL_RUST`) without editing the body. `probe` checksums an input so you can confirm the body ran on your chosen image. |
| `examples/pip-apt-image` | **(image config)** The image-builder steps API: add system packages, Python packages, and shell commands via `RemoteConfig::image_steps` (`ImageStep::apt`/`pip`/`run`), mirroring Python's `Image.apt_install(..)`/`pip_install(..)`/`run_commands(..)`. |

Every example runs offline (in-process, no Modal). Run them all and check their
output with `bash scripts/check-examples.sh`, or one at a time from the repo root:

```bash
# quickstart is a pure-library crate (no runner bin). Prove it offline in-process,
# and that the CLI's preflight passes for the project (cargo/rustc + panic profile):
cargo test -p quickstart -- typed_local_add_returns_5
# -> test result: ok
cargo run -p modal-rust-cli -- doctor --rust --project examples/quickstart
# -> modal-rust doctor — preflight (OFFLINE) …

# With Modal credentials, the same pure-library crate runs remotely via the CLI —
# the runner is generated for you, no src/bin/modal_runner.rs to write:
cargo run -p modal-rust-cli -- run add --project examples/quickstart --input '{"a":2,"b":3}'
# -> {"ok":true,"value":5}

# add-macro is also a pure library (no runner bin); the CLI resolves it the same way:
cargo run -p modal-rust-cli -- doctor --rust --project examples/add-macro
# -> modal-rust doctor — preflight (OFFLINE) …

# add is the manual / no-macro reference — it KEEPS a hand-written runner, named
# `add-runner` (not `modal_runner`, so it never collides with the generated one):
(cd examples/add        && cargo run --bin add-runner -- --entrypoint add --input-json '{"a":40,"b":2}')
# -> {"ok":true,"value":{"sum":42}}
```

The macro path is the ergonomic one — decorate a plain function and call it as a
typed method, no input/output struct named:

```rust
#[modal_rust::function]                       // auto-I/O from the plain signature
pub fn add(a: i64, b: i64) -> anyhow::Result<i64> { Ok(a + b) }

#[modal_rust::function(gpu = "T4")]           // the decorator IS the config
pub fn vector_add(input: VectorAddInput) -> anyhow::Result<VectorAddOutput> { /* … */ }

// …then, against an in-process (local) App:
let app = modal_rust::App::local();
let five: i64 = app.add(2, 3).local()?;                   // offline, zero Modal
let out = app.add(2, 3).remote().await?;                  // on Modal
```

Run the local tour (no Modal credentials needed); it runs `add` in-process through
BOTH the manual registry and the macro/inventory path, printing:

```text
local: add(40, 2) -> {sum: 42}
local (macro/inventory): registry resolves `add` by name
local (macro auto-I/O):  add(2, 3) -> 5
(skipping live .remote()/deploy/call — set RUN_REMOTE=1 with Modal credentials to run them)
```

```bash
git clone https://github.com/nicolaslara/modal-rust
cd modal-rust
cargo run -p example-orchestrate --bin orchestrate
```

With Modal credentials configured, set `RUN_REMOTE=1` to also run the live
`.remote()` and deploy/call round-trips:

```bash
RUN_REMOTE=1 cargo run -p example-orchestrate --bin orchestrate
```

```text
local:  add(40, 2) -> {sum: 42}
remote: add(40, 2) -> {sum: 42}
deployed app 'modal-rust-orchestrate-demo' (...)
call:   add(40, 2) -> {sum: 42}
```

The GPU examples (`cuda-vector-add`, `burn-add`) need a real GPU and Modal
credentials; run them via the CLI or their live tests as described in the
[GPU](#gpu) section above. Both are pure-library crates (no runner bin) — the
`modal-rust` CLI generates the runner. You can confirm OFFLINE (no GPU, no Modal)
that the CLI's preflight passes for the project (cargo/rustc + the release panic
profile the runner needs):

```bash
cargo run -p modal-rust-cli -- doctor --rust --project examples/cuda-vector-add
# -> modal-rust doctor — preflight (OFFLINE) …
```

## License

Licensed under either of:

- Apache License, Version 2.0
- MIT license
