# modal-rust

Rust functions on [Modal](https://modal.com), with a Rust-native authoring and
calling API.

> [!WARNING]
> **Work in progress.** `modal-rust` is early and the public API is still moving.
> The CPU `add` path is proven for local execution, remote Modal execution, and
> deploy/call, but this is not ready to treat as stable infrastructure yet.

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

## Install

`modal-rust` is not published to crates.io yet. Add it from GitHub — one
dependency covers both the macro and the manual authoring paths:

```toml
[dependencies]
modal-rust = { git = "https://github.com/nicolaslara/modal-rust" }
serde = { version = "1", features = ["derive"] }
anyhow = "1"
```

The `#[modal_rust::function]` macro routes its generated code through the
`modal-rust` facade, so you never add `modal-rust-runtime` or `inventory`
directly — just like `serde_derive` routes through `serde`.

For live Modal calls, configure Modal credentials with either `~/.modal.toml` or
the `MODAL_TOKEN_ID` and `MODAL_TOKEN_SECRET` environment variables.

## CLI Usage

Install the CLI from GitHub:

```bash
cargo install --git https://github.com/nicolaslara/modal-rust --package modal-rust-cli
```

From a local checkout, install the CLI you are editing with:

```bash
cargo install --path crates/modal-rust-cli
```

The CLI drives the first-party SDK directly — it builds your crate, reads
`modal_runner --describe`, and creates/invokes the function over gRPC. There is no
generated Python and no dependency on the `modal` CLI; just configure Modal
credentials.

Check your machine first:

```bash
modal-rust doctor --rust --project examples/add
```

Run a registered Rust function remotely on Modal:

```bash
modal-rust run add \
  --project examples/add \
  --input '{"a":40,"b":2}'
```

Deploy the project as a persistent Modal app:

```bash
modal-rust deploy add \
  --project examples/add \
  --app modal-rust-add-poc
```

Call the deployed function without rebuilding:

```bash
modal-rust call add \
  --app modal-rust-add-poc \
  --input '{"a":40,"b":2}'
```

For your own project, point `--project` at the crate that defines the
`modal_runner` binary and registered entrypoints. `--input` accepts inline JSON
or `@path/to/input.json`.

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

`App::from_inventory()` collects every annotated function, and each one gets a
typed method — there is no input or output type to name at the call site:

```rust
use modal_rust::App;

async fn example() -> anyhow::Result<()> {
    let app = App::from_inventory();

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
on the attribute — `gpu`, `timeout`, `cache`, `secrets`, and `volumes` — and is
read from the registry at call time (there are no extra CLI flags):

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
    timeout = 1800,                 // wall-clock seconds
    cache = false,                  // opt out of the cargo build cache (default: on)
    secrets = ["my-api-key"],       // named Modal secrets, injected as env vars
    volumes = ["/data=my-dataset"], // a Modal Volume `my-dataset` mounted at /data
)]
pub fn train(input: TrainInput) -> anyhow::Result<TrainOutput> {
    let _key = std::env::var("API_KEY")?;        // from the secret
    std::fs::write("/data/checkpoint", b"...")?; // persisted on the volume
    Ok(TrainOutput { ok: true })
}
```

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
    let app = App::from_inventory();

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

Then hand the registry to `App` instead of using `from_inventory()`:

- `App::new(modal_registry())` for offline `.local()` calls, and
- `App::connect_with_registry("my-rust-app", modal_registry()).await?` for live
  `.remote()` / deploy / call.

Everything else — `.local()`/`.remote()`/`deploy`+`call` and `.map`/`.spawn` — is
exactly as shown for the macro path. Non-macro users set the same per-function
config (`gpu`, `timeout`, `cache`, `secrets`, `volumes`) on `RemoteConfig` /
`DeployConfig` instead of on the decorator.

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

### Build cache

To keep the `.remote()` development loop fast, the in-container Cargo build is
cached **on by default**: `CARGO_HOME` (and the build `target/`) are persisted as
a single compressed archive on a Modal Volume, unpacked on container start and
repacked on exit. A warm run skips the registry fetch and recompilation — on a
heavy crate this turns a cold rebuild into a `Fresh` no-op. A cache miss only ever
costs time; it never changes the result. Disable it per function with
`#[function(cache = false)]`, or globally with `MODAL_RUST_NO_CACHE=1`. (`deploy`
builds once at image-build time, so the cache applies to the `run` path only.)

## Architecture

The workspace is split into focused crates:

| Crate | Purpose |
| --- | --- |
| `modal-rust` | User-facing `App`, `Function`, `.local()`, `.remote()`, deploy, and call API |
| `modal-rust-runtime` | Handler registry, typed wrappers, runner protocol, and error envelopes |
| `modal-rust-macros` | `#[modal_rust::function]` registration macro |
| `modal-rust-sdk` | First-party Rust gRPC client for Modal control-plane operations |
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
  image uses a `nvidia/cuda:*-devel` base with the Rust toolchain installed; set
  this with `MODAL_RUST_BASE_IMAGE` + `MODAL_RUST_INSTALL_RUST=1` (or
  `RemoteConfig`/`DeployConfig`). For the heavy CUDA build, prefer `deploy` +
  `call` so the build happens once at image-build time.

The GPU spec maps to Modal exactly: `"TYPE[:count]"` (e.g. `"H100:4"`); memory
variants like `"A100-80GB"` pass through as the GPU type.

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

Everything on `#[function(...)]` — `gpu`, `timeout`, `cache`, `secrets`,
`volumes` — is sourced from the registry at call time. The decorator is the
config; there are no extra CLI flags. (Non-macro users can set the same fields on
`RemoteConfig` / `DeployConfig`.)

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
| `examples/add` | **(manual / no-macro)** The walking skeleton: a hand-written `modal_registry()` with `typed!(add)`, plus named entrypoints exercising every runner error kind. The teaching reference for the no-macro path. |
| `examples/add-macro` | **(macro)** The same `add` authored with `#[modal_rust::function]` — plus the auto-I/O plain-signature twin `add_plain(a, b)` callable as `app.add_plain(2, 3).local()/.remote()`, and the full decorator config (`gpu`/`timeout`/`cache`/`secrets`/`volumes`). |
| `examples/orchestrate` | A tour of the facade driving `add` via `.local()`, `.remote()`, and `deploy`+`call` — through BOTH the manual `App::new(modal_registry())` and the macro `App::from_inventory()` + typed `app.add_plain(2, 3)` paths. |
| `examples/cuda-vector-add` | **(macro)** A real GPU kernel — `cudarc` Driver API + precompiled PTX — authored with `#[modal_rust::function(gpu = "T4", name = "vector_add")]`; the decorator IS the config, run on a T4 via `.remote()`. |
| `examples/burn-add` | **(macro)** A real ML workload — a Burn/CubeCL tensor op (NVRTC at runtime) authored with `#[modal_rust::function(gpu = "T4", name = "burn_add")]`, deployed and called on a T4. |

The macro path is the ergonomic one — decorate a plain function and call it as a
typed method, no input/output struct named:

```rust
#[modal_rust::function]                       // auto-I/O from the plain signature
pub fn add_plain(a: i64, b: i64) -> anyhow::Result<i64> { Ok(a + b) }

#[modal_rust::function(gpu = "T4")]           // the decorator IS the config
pub fn vector_add(input: VectorAddInput) -> anyhow::Result<VectorAddOutput> { /* … */ }

// …then, against an inventory-built App:
let app = modal_rust::App::from_inventory();
let five: i64 = app.add_plain(2, 3).local()?;             // offline, zero Modal
let out = app.add_plain(2, 3).remote().await?;            // on Modal
```

Run the local tour (no Modal credentials needed); it runs `add` in-process through
BOTH the manual registry and the macro/inventory path, printing:

```text
local: add(40, 2) -> {sum: 42}
local (macro/inventory): add(40, 2) -> {sum: 42}
local (macro auto-I/O):  add_plain(2, 3) -> 5
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
[GPU](#gpu) section above. You can prove the decorator's gpu rides through inventory
OFFLINE (no GPU, no Modal) with the runner's additive `--describe` manifest:

```bash
# Offline proof that the decorator IS the config (no GPU, no Modal):
cargo run -p example-cuda-vector-add --bin modal_runner -- --describe
# -> {"schema":"modal-rust/describe@1","entrypoints":[{"name":"vector_add",
#     "config":{"gpu":"T4","timeout_secs":null,"cache":null,"secrets":[],"volumes":[]}}]}
```

## License

Licensed under either of:

- Apache License, Version 2.0
- MIT license
