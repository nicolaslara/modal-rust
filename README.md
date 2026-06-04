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

`modal-rust` is not published to crates.io yet. For the current macro-based
authoring path, add it from GitHub:

```toml
[dependencies]
modal-rust = { git = "https://github.com/nicolaslara/modal-rust" }
modal-rust-runtime = { git = "https://github.com/nicolaslara/modal-rust" }
inventory = "0.3"
serde = { version = "1", features = ["derive"] }
anyhow = "1"
```

The direct `modal-rust-runtime` and `inventory` dependencies are a temporary
ergonomics wart: the attribute macro expands to those crate paths. If you use
manual registration instead of the macro, `modal-rust` is enough:

```toml
[dependencies]
modal-rust = { git = "https://github.com/nicolaslara/modal-rust" }
serde = { version = "1", features = ["derive"] }
anyhow = "1"
```

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

By default the CLI drives the first-party SDK directly — it builds your crate,
reads `modal_runner --describe`, and creates/invokes the function over gRPC. No
generated Python and no `modal` CLI; just configure Modal credentials. (The legacy
Python-shim path is still available behind `--use-shim`, which does require the
`modal` CLI on your `PATH`.)

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

Define a Rust function with serializable input and output types:

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

Then call it through `App`:

```rust
use modal_rust::{App, DeployConfig};

# use serde::{Deserialize, Serialize};
# #[derive(Debug, Serialize, Deserialize)]
# pub struct AddInput { pub a: i64, pub b: i64 }
# #[derive(Debug, Serialize, Deserialize)]
# pub struct AddOutput { pub sum: i64 }
# async fn example() -> anyhow::Result<()> {
let app = App::from_inventory();

let out: AddOutput = app
    .function("add")
    .local(AddInput { a: 40, b: 2 })?;

assert_eq!(out.sum, 42);

let app = App::connect("my-rust-app").await?;

let out: AddOutput = app
    .function("add")
    .remote(AddInput { a: 40, b: 2 })
    .await?;

assert_eq!(out.sum, 42);

let deployed = app
    .deploy_with(DeployConfig::for_app("my-rust-app-prod"))
    .await?;

let out: AddOutput = app
    .call(&deployed.name, "add", AddInput { a: 40, b: 2 })
    .await?;

assert_eq!(out.sum, 42);
# Ok(())
# }
```

The manual registration path is also supported if you do not want to use the
attribute macro:

```rust
use modal_rust::{typed, Registry};

pub fn modal_registry() -> Registry {
    Registry::new().function("add", typed!(add))
}
```

Use `App::new(modal_registry())` for local calls or
`App::connect_with_registry("my-rust-app", modal_registry()).await?` for live
Modal calls.

## Run The Examples

Clone the repo and run the local tour:

```bash
git clone https://github.com/nicolaslara/modal-rust
cd modal-rust
cargo run -p example-orchestrate --bin orchestrate
```

That executes the registered `add` function in-process and prints:

```text
local: add(40, 2) -> {sum: 42}
```

With Modal credentials configured, run the live remote and deploy/call paths:

```bash
RUN_REMOTE=1 cargo run -p example-orchestrate --bin orchestrate
```

Expected flow:

```text
local:  add(40, 2) -> {sum: 42}
remote: add(40, 2) -> {sum: 42}
deployed app 'modal-rust-orchestrate-demo' (...)
call:   add(40, 2) -> {sum: 42}
```

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
variants like `"A100-80GB"` pass through as the GPU type. `gpu`, `timeout`, and
`cache` on `#[function(...)]` are sourced from the registry at call time — the
decorator is the config, no extra flags.

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

## License

Licensed under either of:

- Apache License, Version 2.0
- MIT license
