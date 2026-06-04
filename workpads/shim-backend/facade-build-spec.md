# Facade build spec — `crates/modal-rust`

Authoritative, build-ready spec for the user-facing facade crate. Merges the
layout/re-export note and the App/Function/`.local()` note; all contradictions
resolved below in favor of the **frozen runtime's actual API** + knowledge.md §D.
Implement from this without re-deriving.

This milestone delivers: (1) the crate + re-exports, (2) `App`/`Function` with a
**real** in-process `.local()` via the frozen `Registry`, (3) the
`.remote()`/`.spawn()`/`.map()` async **surface** locked but returning
`Error::NotImplemented`. The live remote body is the NEXT milestone (it needs SDK
source-upload, which `modal-rust-sdk` does not have yet).

---

## 0. Verified seam facts (confirmed against source — load-bearing)

**Runtime — `crates/modal-rust-runtime/src/lib.rs` (FROZEN, do NOT edit):**
- `pub type HandlerFn = fn(&[u8]) -> Result<Vec<u8>, RunnerError>;` (line 33) — bare fn ptr, `Copy`.
- `pub enum RunnerError { Decode, UnknownEntrypoint, Function{..}, Encode, Panic{..} }` (line 39) — `Debug + Display + std::error::Error`; has `kind()/message()/details()/backtrace()/to_envelope()`.
- `pub struct Registration { pub name: &'static str, pub handler: HandlerFn }` (line 267), public.
- `pub struct Registry` (line 281): `new()` (287), `from_inventory()` (307), `function(self, name:&'static str, HandlerFn) -> Self` (320), `get(&self, name:&str) -> Option<HandlerFn>` (328, returns owned/`Copy`), `names(&self) -> impl Iterator<Item = &&'static str>` (333).
- `#[macro_export] macro_rules! typed` (line 188) — lives at the runtime crate root; uses `$crate::` internally; re-exportable via crate root.
- **`Registry::get(None)` does NOT produce a `RunnerError`.** The runtime only builds `UnknownEntrypoint` inside `run_cli`. The facade MUST construct its own unknown-entrypoint error.
- `.local()` dispatch is exactly (knowledge.md §D 556-560): `serde_json::to_vec(&input)` → `(handler)(&bytes)` → `serde_json::from_slice(&out)`.

**SDK — `crates/modal-rust-sdk/src/lib.rs` (lib `modal_rust_sdk`):**
- Re-exports (lines 55-70): `ModalClient`, `ModalClientStub`, `Error`, `Result`, `PublishedApp`, `CreatedFunction`, `FunctionResources`, `FunctionSpec`, `ImageSpec`, `Invocation`, `DEFAULT_BASE_IMAGE`, config items, auth items.
- `ModalClient::connect() -> Result<Self>` (async, client.rs:46); `app_get_or_create(...)` (client.rs:108); `app_get_or_create_id(...)` (ops/app.rs:40); `invoke_cbor<A,K,R>(...)` (ops/invoke.rs:71). SDK `Error` is a flat enum, `Debug + Display + Error`; `pub type Result<T>` (error.rs:112).
- Referenced this milestone only in `.remote()`/`connect()` signatures + the `Error::Sdk` wrap.

**Macros — `crates/modal-rust-macros/src/lib.rs` (FROZEN):** exports exactly ONE
symbol, `function` (no `app` macro exists). Expansion is verbatim:
```rust
::inventory::submit! {
    ::modal_rust_runtime::Registration {
        name: #entry_name,
        handler: ::modal_rust_runtime::typed!(#fn_ident),
    }
}
```
Both `::modal_rust_runtime` and `::inventory` are **absolute paths into the
calling crate's extern prelude** — NOT relative to wherever `function` is
invoked, and NOT satisfied by a facade re-export. `examples/add-macro` works
because it lists `modal-rust-runtime` + `modal-rust-macros` + `inventory` as its
own direct deps and aliases via `extern crate modal_rust_macros as modal_rust;`.

**Workspace — `/Users/nicolas/devel/modal-rust/Cargo.toml`:** `[profile.release]`
and `[profile.dev]` both `panic = "unwind"`, workspace-global — they cover any new
member automatically. **No profile edits.** The facade is pure-Rust (no CUDA), so
it goes in BOTH `members` and `default-members`.

**Facade crate does not exist yet** (`crates/modal-rust` absent — confirmed).

---

## 1. `crates/modal-rust/Cargo.toml` (exact)

```toml
[package]
name = "modal-rust"
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"
description = "Ergonomic user-facing facade for modal-rust: App/Function handles with in-process .local() (frozen Registry) and the locked .remote()/.spawn()/.map() async surface. Re-exports modal_rust_sdk as `sdk`."
publish = false

[lib]
name = "modal_rust"

[dependencies]
modal-rust-sdk     = { path = "../modal-rust-sdk" }
modal-rust-runtime = { path = "../modal-rust-runtime" }
# Re-exported so users can spell `#[modal_rust::function]` (see §3 caveat).
modal-rust-macros  = { path = "../modal-rust-macros" }
serde      = { version = "1", features = ["derive"] }
serde_json = "1"

[dev-dependencies]
# The real .local() proof: example-add::modal_registry() gives a frozen Registry.
# Acyclic — example-add depends only on modal-rust-runtime, never on modal-rust.
example-add = { path = "../../examples/add" }
```

**Dependency rulings (resolving the two notes):**
- **No `inventory` dep.** Note 1 carried it "for the facade's own use", but the
  facade hosts no inventory-driven helpers this milestone, and it does NOT fix the
  macro hygiene (§3). Adding `inventory` to the facade would only earn an unused-dep
  smell. Omit it. (Macro users add their own — §3.)
- **No `anyhow` dep.** The facade `Error` is a hand-rolled enum (§2); `anyhow`
  buys nothing.
- **No `tokio` dep.** `.local()` is sync; the `.remote()/.spawn()/.map()` stubs are
  `async fn` that return immediately — `async fn` desugars with **no runtime
  present**, so they compile without `tokio`. (If a `#[tokio::test]` is wanted for
  the not-implemented path, add `tokio = { version = "1", features = ["rt","macros"] }`
  to `[dev-dependencies]` only — but the simpler proof drives the stub via
  `futures`-free blocking or skips it; see §7. Default: do not add tokio.)
- **Keep `modal-rust-macros`** as a real dep — re-exporting `function` compiles
  cleanly and is a genuine ergonomic win (§3).
- **`serde_json`, not `serde_cbor`:** `.local()` uses the runtime's JSON codec.
  CBOR is SDK-only and lands with the future `.remote()` body.
- **Profiles: no change** — workspace `[profile.*]` already pin `panic = "unwind"`.

---

## 2. `src/lib.rs` — re-export surface (exact)

```rust
//! modal-rust: the user-facing facade. One dependency for App/Function + sdk.
//!
//! - `.local()` runs the registered handler IN-PROCESS via the frozen Registry
//!   (zero Modal, zero network) and returns the typed output.
//! - `.remote()/.spawn()/.map()` are the LOCKED async surface; they return
//!   `Error::NotImplemented` this milestone (remote needs SDK source-upload,
//!   tracked as the next workflow).

// (1) Control-plane SDK, namespaced exactly as the knowledge.md §D sketch promises.
pub use modal_rust_sdk as sdk;

// (2) Runtime essentials that appear in the facade's public API / error paths.
//     Selective — NOT a glob — so `__macro_support`/`codec` stay out of the
//     facade's stable surface.
pub use modal_rust_runtime::{HandlerFn, Registration, Registry, RunnerError};
// `typed!` is #[macro_export] at the runtime crate root; re-export for users who
// build a Registry by hand through the facade.
pub use modal_rust_runtime::typed;

// (3) Proc-macro re-export. Makes `#[modal_rust::function]` spellable WITHOUT the
//     `extern crate ... as modal_rust;` alias hack. NOTE: this is a CONVENIENCE
//     re-export, NOT a single-dep story — see §3. Only `function` exists; there is
//     NO `app` macro (`modal_rust::App` is the struct below). Do NOT write
//     `pub use modal_rust_macros::{function, app};` — that is a compile error.
pub use modal_rust_macros::function;

mod app;
mod error;
mod function;

pub use app::App;
pub use error::{Error, Result};
pub use function::{Function, FunctionCall};
```

Hard rule: the task brief's "`#[modal_rust::app]`" does not exist — `modal_rust::App`
is the struct. Re-export `function` only.

---

## 3. Macro-hygiene decision (the load-bearing question)

**Verdict: re-export `function`, but it is NOT a zero-extra-dep story. Leave the
frozen macro unchanged and DOCUMENT the two extra deps macro users need.**

- `pub use modal_rust_macros::function;` makes the **attribute callable** as
  `#[modal_rust::function]`. Good — ergonomic, no alias hack, shares the
  `modal_rust::` namespace with `App`/`Function`.
- But the **generated code does not compile** in a crate whose only dep is
  `modal-rust`. The macro expands to absolute `::modal_rust_runtime::...` and
  `::inventory::submit!`. Rust resolves `::foo` against *the compiling crate's own
  direct deps* (its extern prelude) — a `pub use` from the facade creates a name in
  the facade's module tree; it does **not** inject `modal_rust_runtime` or
  `inventory` into the downstream crate's extern prelude. Result without them:
  `E0433: failed to resolve: use of undeclared crate or module 'modal_rust_runtime'`
  (and likewise `inventory`).
- Re-exporting `inventory` from the facade (`pub use inventory;`) does NOT fix it
  either — the macro emits `::inventory`, not `::modal_rust::inventory`.

**Therefore, document in `src/lib.rs` doc + the macro's downstream story:** a crate
using `#[modal_rust::function]` must add three direct deps of its own:
```toml
modal-rust         = { path = ... }   # facade: App/Function/sdk + the `function` attr
modal-rust-runtime = { path = ... }   # macro expands to ::modal_rust_runtime
inventory          = "0.3"            # macro expands to ::inventory::submit!
```
So: the facade ships **App/Function/.local()/.remote() as a true single-dep
story**; the proc-macro path keeps its current 3-dep footprint and gains only the
`#[modal_rust::function]` spelling. This is the priority-correct, no-frozen-change
outcome. Making it zero-extra-dep would require editing the frozen macro's
expansion (facade-relative paths) and would break `examples/add-macro` — explicitly
out of scope.

---

## 4. Facade `Error` type — `src/error.rs`

```rust
use crate::RunnerError; // re-exported from modal_rust_runtime

#[derive(Debug)]
pub enum Error {
    /// Entrypoint name absent from the App's Registry. Carries the requested name
    /// plus the known names (mirrors run_cli's diagnostic). Built by the facade —
    /// Registry::get(None) yields no RunnerError.
    UnknownEntrypoint { name: String, known: Vec<String> },
    /// In-process handler failed: the FROZEN five-kind taxonomy wrapped verbatim
    /// (Decode In / Function body / Encode Out / Panic / runtime UnknownEntrypoint).
    Runner(RunnerError),
    /// Serializing the `.local()` input to JSON failed (BEFORE the handler ran).
    Encode(serde_json::Error),
    /// Deserializing the handler's JSON output into `Out` failed (AFTER it ran).
    Decode(serde_json::Error),
    /// A control-plane (remote) operation failed. Reserved for .remote/.spawn/.map.
    Sdk(modal_rust_sdk::Error),
    /// A surface intentionally not wired this milestone (.remote/.spawn/.map):
    /// carries a message pointing to the next workflow.
    NotImplemented(String),
}

pub type Result<T> = std::result::Result<T, Error>;
```

Impls:
- `impl std::fmt::Display for Error` — human-readable per variant.
- `impl std::error::Error for Error` — `source()` returns the inner
  `Runner`/`Encode`/`Decode`/`Sdk` where applicable; `None` for `UnknownEntrypoint`
  and `NotImplemented`.
- `impl From<RunnerError> for Error` → `Error::Runner` (for `?` ergonomics).
- `impl From<modal_rust_sdk::Error> for Error` → `Error::Sdk` (used by `connect`).
- **NO blanket `From<serde_json::Error>`** — the same serde error type covers both
  encode (input) and decode (output) and they MUST map to distinct variants.
  Construct `Error::Encode` / `Error::Decode` explicitly at the two call sites
  (`.map_err(Error::Encode)?` / `.map_err(Error::Decode)`).
- Helper producing the standard message:
  ```rust
  impl Error {
      pub(crate) fn not_implemented(surface: &str) -> Error {
          Error::NotImplemented(format!(
              "`{surface}` is not implemented yet: remote execution needs SDK \
               source-upload (MountPutFile/blob), which modal-rust-sdk does not \
               have yet. Tracked as the next workflow milestone. Use .local() for \
               in-process execution today."
          ))
      }
  }
  ```

---

## 5. `App` — `src/app.rs`

```rust
use crate::{Error, Function, Result, Registry};

pub struct App {
    registry: Registry,            // owned; the ONLY field .local() needs
    remote: Option<RemoteHandle>,  // None until connect(); used by .remote later
}

struct RemoteHandle {              // private; built by connect()
    client: modal_rust_sdk::ModalClient,
    app_id: String,
}
```

Constructors — all **sync, zero Modal, zero network** (so `.local()` works without
`connect`):
```rust
impl App {
    /// Build from an explicit Registry (manual builder path — e.g.
    /// example_add::modal_registry()). Zero Modal.
    pub fn new(registry: Registry) -> Self {
        App { registry, remote: None }
    }

    /// Build from the inventory-collected Registry (the #[modal_rust::function]
    /// macro path). Zero Modal.
    pub fn from_inventory() -> Self {
        App::new(Registry::from_inventory())
    }
}
```

`connect()` — **only for the future remote path; never required by `.local()`**.
Wire it for real (the SDK supports `connect` + `app_get_or_create` end-to-end), but
it is only ever called from live/`#[ignore]`-gated tests, so offline gates stay
green:
```rust
impl App {
    /// Connect to Modal's control plane for the remote path: build a
    /// sdk::ModalClient (reads ~/.modal.toml / MODAL_TOKEN_*) and resolve an
    /// app_id via AppGetOrCreate. Uses the inventory Registry. `.remote()/.spawn()/
    /// .map()` still return Error::NotImplemented THIS milestone; `.local()` never
    /// needs this call.
    pub async fn connect(name: &str) -> Result<Self> {
        App::connect_with_registry(name, Registry::from_inventory()).await
    }

    /// As `connect`, with an explicit Registry combined with a live remote handle.
    pub async fn connect_with_registry(name: &str, registry: Registry) -> Result<Self> {
        let client = modal_rust_sdk::ModalClient::connect().await?;     // From<sdk::Error>
        let app_id = client.app_get_or_create_id(name /* + env per SDK sig */).await?;
        Ok(App { registry, remote: Some(RemoteHandle { client, app_id }) })
    }
}
```
(If the exact `app_get_or_create_id` argument shape causes friction at build time,
the minimal-shape fallback is `connect` returning `Error::not_implemented`. Prefer
wiring it for real — it makes the next milestone a pure addition. Either choice
keeps offline gates green because no unit test calls `connect`.)

Accessor (the single entry to a `Function` handle):
```rust
impl App {
    /// Get a Function handle by entrypoint name. Resolves the HandlerFn from the
    /// Registry NOW (cheap, Copy) so an unknown name surfaces a clear error with
    /// the full known-names list when .local()/.remote() is actually called.
    /// Does NOT error eagerly — keeps the API fluent (app.function("add").local(..)).
    pub fn function(&self, name: &str) -> Function<'_> {
        Function {
            app: self,
            name: name.to_string(),
            handler: self.registry.get(name), // Option<HandlerFn>
        }
    }

    pub(crate) fn known_names(&self) -> Vec<String> {
        self.registry.names().map(|n| n.to_string()).collect()
    }
}
```

---

## 6. `Function<'a>` — `src/function.rs`

```rust
use crate::{Error, HandlerFn, Result};

pub struct Function<'a> {
    pub(crate) app: &'a crate::App,
    pub(crate) name: String,            // owned: App::function takes &str
    pub(crate) handler: Option<HandlerFn>,  // Some if found, None if unknown
}
```

`.local()` — **fully implemented this milestone** (the M0 proof):
```rust
impl<'a> Function<'a> {
    /// Run the registered handler IN-PROCESS via the FROZEN Registry: serialize
    /// `input` to JSON, invoke the HandlerFn, deserialize JSON output to `Out`.
    /// Zero Modal, zero network. Mirrors Modal Python Function.local() = raw_f(..).
    pub fn local<In, Out>(&self, input: In) -> Result<Out>
    where
        In: serde::Serialize,
        Out: serde::de::DeserializeOwned,
    {
        let handler = self.handler.ok_or_else(|| self.unknown())?;
        let bytes = serde_json::to_vec(&input).map_err(Error::Encode)?;
        let out = handler(&bytes).map_err(Error::Runner)?;
        serde_json::from_slice(&out).map_err(Error::Decode)
    }

    fn unknown(&self) -> Error {
        Error::UnknownEntrypoint {
            name: self.name.clone(),
            known: self.app.known_names(),
        }
    }
}
```

Error mapping (all five frozen kinds preserved via the single `Error::Runner` wrap):

| failure | where | facade variant |
|---|---|---|
| input not Serialize-able | `to_vec` | `Error::Encode` |
| handler decode of `In` (bad shape) | `RunnerError::Decode` | `Error::Runner` |
| handler body returns `Err` | `RunnerError::Function` | `Error::Runner` |
| handler `Out` not encodable | `RunnerError::Encode` | `Error::Runner` |
| handler panics | `RunnerError::Panic` | `Error::Runner` |
| handler output not `Out`-shaped | `from_slice` | `Error::Decode` |
| name absent from Registry | `handler == None` | `Error::UnknownEntrypoint` |

The double JSON round-trip is intentional/correct: input → JSON → handler's own
`codec::decode` → `In`; handler's `codec::encode` → JSON → facade `from_slice` →
`Out`. Identical to running the runner without a subprocess, so
`add(AddInput{40,2})` yields `AddOutput{sum:42}`.

The locked remote surface — **all return `Error::NotImplemented` this milestone**
(signatures + docs frozen so the next milestone fills bodies with NO signature
change):
```rust
impl<'a> Function<'a> {
    /// Run the function body REMOTELY on Modal. NOT YET IMPLEMENTED: returns
    /// Error::NotImplemented — remote execution needs SDK source-upload, tracked as
    /// the next workflow milestone. Signature + docs are LOCKED now.
    #[allow(clippy::unused_async)] // body lands next milestone (sdk::invoke_cbor)
    pub async fn remote<In, Out>(&self, input: In) -> Result<Out>
    where
        In: serde::Serialize,
        Out: serde::de::DeserializeOwned,
    {
        let _ = input;
        Err(Error::not_implemented("Function::remote"))
    }

    /// Fire-and-forget spawn returning a FunctionCall handle. NOT YET IMPLEMENTED.
    #[allow(clippy::unused_async)]
    pub async fn spawn<In>(&self, input: In) -> Result<FunctionCall>
    where
        In: serde::Serialize,
    {
        let _ = input;
        Err(Error::not_implemented("Function::spawn"))
    }

    /// Fan-out over many inputs. NOT YET IMPLEMENTED.
    #[allow(clippy::unused_async)]
    pub async fn map<In, Out, I>(&self, inputs: I) -> Result<Vec<Out>>
    where
        In: serde::Serialize,
        Out: serde::de::DeserializeOwned,
        I: IntoIterator<Item = In>,
    {
        let _ = inputs;
        Err(Error::not_implemented("Function::map"))
    }
}

/// Handle returned by `spawn`. Locks the `spawn().get()` shape (knowledge.md §D)
/// without depending on the SDK's internal call-handle type yet.
pub struct FunctionCall {
    _private: (),
}

impl FunctionCall {
    /// Await the spawned call's result. NOT YET IMPLEMENTED.
    #[allow(clippy::unused_async)]
    pub async fn get<Out>(&self, timeout: Option<std::time::Duration>) -> Result<Out>
    where
        Out: serde::de::DeserializeOwned,
    {
        let _ = timeout;
        Err(Error::not_implemented("FunctionCall::get"))
    }
}
```

**Async strategy:** `.local()` is sync (matches the §D sketch — no `.await`).
`.remote()/.spawn()/.map()/get()` are `async fn`; they compile with no runtime
present. Each stub consumes its args (`let _ = …;`) to avoid unused warnings and
carries `#[allow(clippy::unused_async)]` with the doc note that the body lands next
milestone. Keep generics exactly as shown so the next milestone (CBOR via
`sdk::ModalClient::invoke_cbor`) fills bodies without a signature change.

---

## 7. Public API a user writes (ONE dep, App/Function path)

```rust
use modal_rust::{App, Error};
use modal_rust::sdk;                    // control-plane types for later

let app = App::new(example_add::modal_registry());     // or App::from_inventory()
let out: AddOutput = app.function("add").local(AddInput { a: 40, b: 2 })?;
assert_eq!(out.sum, 42);

// locked, not-yet-live:
// let out = app.function("add").remote(input).await?;   // Err(Error::NotImplemented)
```

---

## 8. Test plan — `crates/modal-rust/tests/local.rs` (the `.local()` proof)

Uses the `example-add` dev-dependency (acyclic: example-add → modal-rust-runtime
only, never → modal-rust).

```rust
use example_add::{modal_registry, AddInput, AddOutput};
use modal_rust::{App, Error};

#[test]
fn local_runs_real_add() {
    let app = App::new(modal_registry());
    let out: AddOutput = app.function("add").local(AddInput { a: 40, b: 2 }).unwrap();
    assert_eq!(out.sum, 42);                       // the M0 proof
}

#[test]
fn local_unknown_entrypoint_errors() {
    let app = App::new(modal_registry());
    let err = app
        .function("nope")
        .local::<_, AddOutput>(AddInput { a: 1, b: 2 })
        .unwrap_err();
    match err {
        Error::UnknownEntrypoint { name, known } => {
            assert_eq!(name, "nope");
            assert!(known.iter().any(|n| n == "add"));   // known names listed
        }
        other => panic!("expected UnknownEntrypoint, got {other:?}"),
    }
}

#[test]
fn local_surfaces_function_error() {
    // example_add::fail -> anyhow -> RunnerError::Function -> Error::Runner
    let app = App::new(modal_registry());
    let err = app
        .function("fail")
        .local::<_, AddOutput>(AddInput { a: 1, b: 2 })
        .unwrap_err();
    assert!(matches!(
        err,
        Error::Runner(modal_rust::RunnerError::Function { .. })
    ));
}

#[test]
fn remote_returns_not_implemented() {
    // Drive the async stub WITHOUT a tokio dep: poll a Ready future inline, OR
    // simply assert via the synchronous error constructor. Simplest: construct the
    // future and block with a no-dep micro-executor is overkill — instead test the
    // message path directly:
    let app = App::new(modal_registry());
    let f = app.function("add");
    let fut = f.remote::<_, AddOutput>(AddInput { a: 1, b: 2 });
    let res = futures_lite_block_on(fut); // see note
    assert!(matches!(res, Err(Error::NotImplemented(_))));
}
```
Note on the remote test: the stub future is immediately-ready, so any trivial
block-on works. To avoid a new dep, either (a) skip this test (the `.local()` +
unknown + function-error trio already proves the milestone), or (b) add
`tokio = { version = "1", features = ["rt","macros"] }` to `[dev-dependencies]`
and use `#[tokio::test]`. **Default recommendation: include the remote-stub test
behind `#[tokio::test]` with tokio as a dev-dep**, since it cheaply locks the
"NotImplemented, not a panic" contract. If keeping dev-deps minimal is preferred,
drop this one test — it is not load-bearing for the milestone proof.

---

## 9. Workspace edit — `/Users/nicolas/devel/modal-rust/Cargo.toml` (BOTH lists)

Insert `"crates/modal-rust",` adjacent to `"crates/modal-rust-sdk",` in BOTH
arrays. The crate is pure-Rust (no CUDA), so it MUST be in `default-members` (so
the WORKING.md gates cover it) as well as `members`. No `[profile.*]` edits —
workspace profiles already pin `panic = "unwind"`.

`members` becomes:
```toml
members = [
    "crates/modal-rust-sdk",
    "crates/modal-rust",
    "crates/modal-rust-runtime",
    "crates/modal-rust-cli",
    "crates/modal-rust-macros",
    "examples/add",
    "examples/add-macro",
    "examples/cuda-vector-add",
    "examples/burn-add",
]
```
`default-members` becomes (note `examples/burn-add` stays excluded — CUDA-only):
```toml
default-members = [
    "crates/modal-rust-sdk",
    "crates/modal-rust",
    "crates/modal-rust-runtime",
    "crates/modal-rust-cli",
    "crates/modal-rust-macros",
    "examples/add",
    "examples/add-macro",
    "examples/cuda-vector-add",
]
```

---

## 10. File layout & budget (WORKING.md: ~300-500 LOC max each; here each ~50-200)

- `crates/modal-rust/src/lib.rs` (~60) — crate docs + re-exports + module wiring.
- `crates/modal-rust/src/error.rs` (~90) — `Error`/`Result` + Display/Error/From + `not_implemented`.
- `crates/modal-rust/src/app.rs` (~110) — `App` (`new`, `from_inventory`, `connect`/`connect_with_registry`, `function`, `known_names`), private `RemoteHandle`.
- `crates/modal-rust/src/function.rs` (~180) — `Function<'a>` (real `.local()`, stub `.remote/.spawn/.map`), `FunctionCall`.
- `crates/modal-rust/tests/local.rs` (~70) — the `.local()` proof (+ optional remote-stub test).

---

## 11. Gate expectations (WORKING.md)

Gates run on **default-members** (NOT `--workspace`/`--all-features`). All must
stay green on a no-CUDA Linux box:
- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`  (async stubs carry
  `#[allow(clippy::unused_async)]`; stub args consumed via `let _ = …;` → no unused
  warnings)
- `cargo build`
- `cargo test`

The new crate is pure-Rust and in both members lists. `connect`/remote stubs make
no Modal call from any unit/integration test, so offline gates pass.

---

## 12. FROZEN invariants — do NOT touch

- `modal-rust-runtime` (Registry/HandlerFn/typed!/run_cli/RunnerError), the runner
  CLI protocol, the `modal-rust-macros` expansion, and the run-vs-deploy build
  boundary. This milestone ADDS `crates/modal-rust` and lightly re-exports — nothing
  more.
- Do not rewrite `modal-rust-sdk`. Do not break or rewire existing examples; the
  `.local()` proof uses `example-add` as a dev-dependency, not a rewire.
