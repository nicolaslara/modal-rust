# Mock backend (rust-native in-process gRPC mock) — build-ready spec

**Status:** Design + spike complete. The spike round-tripped a real
`modal_rust_sdk::ModalClient` against an in-process tonic mock and recorded the
request. Date: 2026-06-05. **SPEC_DONE.**

**What this delivers:** a NEW dev/test-only crate `crates/modal-rust-testkit`
(`modal_rust_testkit`) — an in-process tonic server implementing the
`ModalClient` gRPC service on a loopback port, that (1) RECORDS every request
(typed + queryable), and (2) returns ergonomically-configurable responses
(happy-path defaults + per-test overrides). Tests point the SDK / facade at the
mock's `http://127.0.0.1:<port>` with no transport change. Includes the example
tests (one facade-level end-to-end + one table test) that prove a main feature
offline.

This mirrors the Python `MockClientServicer` pattern (the "build our own"
recommendation from `docs/testing-strategy.md` §"Reusing the Python mock vs
building our own"): wire-compatibility gives wire-compat, not test-harness reuse,
so we mirror the pattern natively in Rust.

**FROZEN — additive only.** No change to SDK client behavior, the runner CLI
protocol, the 5 error kinds, the FILE-mode wire, the facade PUBLIC API, the
macro/runtime. The mock is new test infra: the testkit is a workspace member used
only as a `[dev-dependencies]` of the facade/SDK tests; the SHIPPED crates do NOT
depend on it. ONE small, additive, test-only facade injection seam is added
(`App::connect_at` / `App::with_remote`) because the facade cannot today be
pointed at a custom `server_url` per-App without it (see §8). `docs/testing-
strategy.md` is left untouched.

---

## 0. The spike (DONE — proof it round-trips)

A throwaway crate at `tmp/mock-spike/` (gitignored) proved the whole approach.
Artifacts copied into this workpad as evidence:
`mock-spike-main.rs.txt`, `mock-spike-Cargo.toml.txt`, `mock-spike-build.rs.txt`,
`mock-spike-run-output.txt`.

Run output (exact):

```
[spike] mock listening on http://127.0.0.1:64469
[spike] connected (ClientHello round-tripped)
[spike] function_from_name -> fu-mock-1
[spike] RECORDED FunctionGetRequest: app_name="some-app" object_tag="handler" env="main"
[spike] invoke_cbor envelope = {"ok":true,"value":{"sum":42}}
[spike] RECORDED invoke: FunctionMap.function_id="fu-mock-1", get_outputs polls=1
[spike] PASS — round-trip + recording + CBOR invoke confirmed
```

What the spike proved (each was a live technical unknown going in):

1. **The 201-method problem is solved by Option A** (testkit owns
   `build_server(true)` + a `mock_unimplemented!` macro stub). The
   `service ModalClient` has exactly **201 RPCs** (`crates/modal-rust-sdk/proto/
   api.proto:4129`; counted: 189 unary + 12 server-streaming). The mock
   hand-writes only the handful the SDK calls; the macro emits the rest as
   `Status::unimplemented`. The spike compiled with 5 hand-written + 196 macro
   stubs.

2. **A real `ModalClient::from_config(ModalConfig{ server_url, .. })` dials the
   mock with ZERO transport change.** `channel.rs:29` applies TLS only to
   `https://` and serves plain `http://`; `client.rs:63` injects the URL. The
   mock implemented `ClientHello` (issued by `from_config`, `client.rs:72`) and
   the connect succeeded.

3. **The hardest path round-trips:** `client.invoke_cbor::<_,_,String>(...)` —
   the EXACT call `.remote()` makes (`app.rs:217`, `R = String` = the runner
   envelope) — drove `FunctionMap` → `FunctionGetOutputs`; the mock returned a
   `GenericResult` whose `data` was **CBOR of the envelope string**
   `{"ok":true,"value":{"sum":42}}`; the SDK decoded it back; the test asserted
   `value.sum == 42`. Both requests were recorded.

4. **The one server-streaming RPC our flow can touch (`ImageJoinStreaming`) is
   implementable** with a concrete boxed stream yielding a terminal SUCCESS — but
   the happy path never reaches it because `image_get_or_create` short-circuits
   on an inline-success result (`ops/image.rs:501-502`). So streaming is optional
   for v1.

### Two non-obvious gotchas the spike caught (build these into the testkit)

- **GOTCHA 1 — `#[async_trait]` cannot see through a `macro_rules!`
  invocation.** The generated server trait is `#[async_trait]` (boxed-future
  desugaring; tonic 0.14 — verified in the generated `modal.client.rs`). An
  attribute macro on the `impl` runs BEFORE an inner `macro_rules!` expands, so
  plain `async fn`s emitted by the stub macro never get the async-trait lifetime
  rewrite → **E0195 ×189**. Fix: the macro emits the ALREADY-DESUGARED form —
  `fn name<'life0,'async_trait>(&'life0 self, req) -> Pin<Box<dyn Future<Output=
  Result<Response<R>,Status>> + Send + 'async_trait>> where 'life0:'async_trait,
  Self:'async_trait { Box::pin(async move { Err(Status::unimplemented(..)) }) }`.
  The hand-written RPCs stay normal `async fn` inside the `#[async_trait]` impl;
  the macro arms are invoked in the same impl and pass through untouched. (The
  exact working macro is in `mock-spike-main.rs.txt`.)

- **GOTCHA 2 — `Server::add_service` needs the `router` tonic feature.** The
  `server` feature alone does NOT enable it (only tonic's `default` does;
  verified in tonic-0.14.6 `Cargo.toml`: `add_service` is
  `#[cfg(feature = "router")]`). The testkit's `tonic` dep must enable
  `["server", "router", "transport", "codegen"]`. (`router` pulls `axum` — fine,
  it is a dev/test-only crate.)

---

## 1. New crate layout — `crates/modal-rust-testkit`

A workspace member, but a **dev/test concern**. Add `"crates/modal-rust-testkit"`
to `members` in the root `Cargo.toml`. Do **NOT** add it to `default-members`
(keep bare `cargo build`/`clippy`/`test` lean and to avoid the server/axum stack
in default builds) — but DO run it in the offline gate explicitly (see §10). The
SHIPPED crates (`modal-rust-sdk`, `modal-rust`, `modal-rust-runtime`,
`modal-rust-cli`, `modal-rust-macros`) keep zero dependency on it; it is wired
only as a `[dev-dependencies]` of `modal-rust` (and optionally `modal-rust-sdk`)
test targets.

```
crates/modal-rust-testkit/
  Cargo.toml
  build.rs                 # build_server(true) on the SAME proto (copied in)
  proto/api.proto          # copied verbatim from crates/modal-rust-sdk/proto/api.proto
  src/
    lib.rs                 # pub mod prelude; re-exports MockModal, builder, etc.
    proto.rs               # tonic::include_proto!("modal.client") behind #![allow(..)]
    macros.rs              # mock_unimplemented! (the desugared-form stub macro)
    record.rs              # RecordedRequest enum + RequestLog (Arc<Mutex<..>>) + typed accessors
    responder.rs           # Responses config: defaults + per-RPC override closures
    servicer.rs            # MockServicer: impl ModalClient (hand-written RPCs + macro stub)
    server.rs              # MockModal handle: start()/url()/shutdown(), owns the task
    builder.rs             # MockModal::builder() -> MockModalBuilder
```

### 1.1 `Cargo.toml`

```toml
[package]
name = "modal-rust-testkit"
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"
description = "In-process Modal gRPC mock for offline modal-rust tests (dev/test only)"
build = "build.rs"
publish = false           # never published; dev/test infra

[lib]
name = "modal_rust_testkit"

[dependencies]
# The mock generates ITS OWN message + server types from the same proto. These
# are wire-compatible with the SDK's client types; tests construct responses with
# the testkit types and assert on the testkit-recorded request types. That is
# fine and intended (the doc's Option A note).
modal-rust-sdk = { path = "../modal-rust-sdk" }   # for codec (CBOR) + ModalConfig in helpers
# Server-side tonic. `router` is REQUIRED for Server::add_service (GOTCHA 2).
tonic       = { version = "0.14.3", default-features = false, features = ["server", "router", "transport", "codegen"] }
tonic-prost = "0.14.3"
prost       = "0.14.3"
prost-types = "0.14.3"
tokio       = { version = "1.49", features = ["full"] }
tokio-stream = "0.1"
serde       = { version = "1", features = ["derive"] }
serde_json  = "1"

[build-dependencies]
tonic-prost-build = "0.14.3"
protoc-bin-vendored = "3"
```

### 1.2 `build.rs` (identical recipe to the spike, proven)

```rust
use std::error::Error;
fn main() -> Result<(), Box<dyn Error>> {
    let protoc_path = protoc_bin_vendored::protoc_bin_path()?;
    unsafe { std::env::set_var("PROTOC", protoc_path) };
    let protoc_include = protoc_bin_vendored::include_path()?;
    tonic_prost_build::configure()
        .build_client(false)
        .build_server(true) // server trait for the mock
        .compile_protos(
            &["proto/api.proto"],
            &["proto", protoc_include.to_str().ok_or("invalid protoc include path")?],
        )?;
    Ok(())
}
```

**Proto handling.** Copy `crates/modal-rust-sdk/proto/api.proto` into
`crates/modal-rust-testkit/proto/api.proto` (the spike did this). It is the same
vendored Modal proto. A `build.rs` `println!("cargo:rerun-if-changed=...")` plus a
ONE-LINE comment at the top of the copy ("copied verbatim from
crates/modal-rust-sdk/proto/api.proto; keep in sync") is enough; do not try to
point `compile_protos` across crate dirs (relative include paths get fragile).
The duplicate-codegen cost is paid only in the dev/test build.

### 1.3 `src/proto.rs`

```rust
pub mod modal {
    pub mod client {
        #![allow(clippy::all, dead_code)]
        tonic::include_proto!("modal.client");
    }
}
pub use modal::client as api;
```

(Mirrors `crates/modal-rust-sdk/src/proto.rs` — the `#![allow(..)]` keeps
`clippy -D warnings` green on the generated module.)

---

## 2. The `mock_unimplemented!` macro (`src/macros.rs`)

Exactly the spike macro (in `mock-spike-main.rs.txt`). Two arm kinds:

- `unary name(ReqTy) -> RespTy;` → emits one desugared async-trait method.
- `stream name[AssocStreamTy](ReqTy) -> ItemTy;` → emits the associated
  `type AssocStreamTy = Pin<Box<dyn Stream<Item=Result<ItemTy,Status>>+Send+'static>>`
  AND the method returning `Status::unimplemented`.

The arm list (189 unary + 12 stream) is generated ONCE from the proto. **Generate
it with the helper below and paste it into `servicer.rs`** (do not hand-type 201
lines). `google.protobuf.Empty` maps to `()` for both request and response (prost
behavior — verified: `app_client_disconnect(AppClientDisconnectRequest) -> ()`,
`client_hello(()) -> ClientHelloResponse`). The macro arms reference `gen::Foo`
(the testkit's own generated types), NOT `super::Foo` (GOTCHA: inside the impl
the path is `gen::`).

Helper to regenerate the arm list (the spike used this; keep it in
`crates/modal-rust-testkit/scripts/gen_arms.py` or inline in a doc-comment):

```python
import re
EMPTY='google.protobuf.Empty'
def ty(t): t=t.strip(); return '()' if t==EMPTY else f'gen::{t}'
HAND={...}  # snake_case names of hand-written RPCs to EXCLUDE
for line in rpcs:  # "Name|Req|Resp" extracted from the service block
    name,req,resp=line.split('|'); snake=re.sub(r'(?<!^)(?=[A-Z])','_',name).lower()
    if snake in HAND: continue
    if resp.startswith('stream'):
        inner=resp[6:].strip(); print(f"stream {snake}[{name}Stream]({ty(req)}) -> {ty(inner)};")
    else:
        print(f"unary {snake}({ty(req)}) -> {ty(resp)};")
```

(The PascalCase→snake_case method names and the `{Name}Stream` associated-type
names match tonic 0.14 codegen — verified against the generated trait.)

---

## 3. Request recording (`src/record.rs`)

Mirrors the Python servicer's `self.requests` (`conftest.py:656`) +
`ctx.get_requests("Method")` (`grpc_testing.py:155`).

```rust
use std::sync::{Arc, Mutex};
use crate::proto::api as gen;

/// One recorded request, typed. Variants for the ~18 RPCs the SDK uses; extend as
/// new ops appear. (The full set is small and closed for our flow.)
#[derive(Debug, Clone)]
pub enum RecordedRequest {
    ClientHello,
    AppGetOrCreate(gen::AppGetOrCreateRequest),
    AppCreate(gen::AppCreateRequest),
    AppPublish(gen::AppPublishRequest),
    EnvironmentGetOrCreate(gen::EnvironmentGetOrCreateRequest),
    BlobCreate(gen::BlobCreateRequest),
    MountGetOrCreate(gen::MountGetOrCreateRequest),
    MountPutFile(gen::MountPutFileRequest),
    ImageGetOrCreate(gen::ImageGetOrCreateRequest),
    FunctionPrecreate(gen::FunctionPrecreateRequest),
    FunctionCreate(gen::FunctionCreateRequest),
    FunctionGet(gen::FunctionGetRequest),
    FunctionMap(gen::FunctionMapRequest),
    FunctionPutInputs(gen::FunctionPutInputsRequest),
    FunctionGetOutputs(gen::FunctionGetOutputsRequest),
    SecretGetOrCreate(gen::SecretGetOrCreateRequest),
    VolumeGetOrCreate(gen::VolumeGetOrCreateRequest),
}

/// The shared, cloneable request log. Cheap `Arc<Mutex<..>>` clone shared between
/// the running server task and the test handle.
#[derive(Clone, Default)]
pub struct RequestLog {
    inner: Arc<Mutex<Vec<RecordedRequest>>>,
}

impl RequestLog {
    pub(crate) fn push(&self, r: RecordedRequest) {
        self.inner.lock().unwrap().push(r);
    }
    /// All recorded requests in arrival order.
    pub fn all(&self) -> Vec<RecordedRequest> { self.inner.lock().unwrap().clone() }
    pub fn len(&self) -> usize { self.inner.lock().unwrap().len() }
}
```

### 3.1 Typed, ergonomic accessors (the rust-like query surface)

A trait `FromRecorded` maps a message type → its enum variant, powering the
generic `requests::<T>()` / `last::<T>()` API the task asks for:

```rust
pub trait FromRecorded: Sized {
    fn extract(r: &RecordedRequest) -> Option<&Self>;
}
// one tiny impl per recorded type (macro-generated to avoid boilerplate):
// impl FromRecorded for gen::FunctionCreateRequest { fn extract(r) -> ... }

impl RequestLog {
    /// Every recorded request of type T, in order. e.g.
    /// `mock.requests::<FunctionCreateRequest>()`.
    pub fn requests<T: FromRecorded + Clone>(&self) -> Vec<T> {
        self.inner.lock().unwrap().iter().filter_map(T::extract).cloned().collect()
    }
    /// The LAST recorded request of type T (None if absent). e.g.
    /// `mock.last::<FunctionCreateRequest>()`.
    pub fn last<T: FromRecorded + Clone>(&self) -> Option<T> {
        self.requests::<T>().pop()
    }
    /// Count of recorded requests of type T (Python's
    /// `len(ctx.get_requests("X"))`).
    pub fn took<T: FromRecorded + Clone>(&self) -> usize { self.requests::<T>().len() }
}
```

Generate the ~18 `FromRecorded` impls with a small local
`impl_from_recorded! { FunctionCreateRequest => FunctionCreate, ... }` macro
(one line per type). `MockModal` (the handle, §5) derefs/forwards `requests`,
`last`, `took`, `all` so tests write `mock.requests::<…>()` directly.

---

## 4. Ergonomic response config (`src/responder.rs` + `src/builder.rs`)

Happy-path DEFAULTS + per-RPC OVERRIDES via builder + closures (mirrors Python's
`add_response`/`set_responder`/`function_body`, `grpc_testing.py:113-126`,
`conftest.py:919`). Deterministic ids/responses (no Date/random):
`ap-1`, `im-1`, `fu-1`, `de-1`, `fc-1`, `mo-1`, `bl-1`, `sc-1`, `vo-1`.

### 4.1 The two override forms

```rust
/// Per-test response config. Default = the happy path for a deploy/call/remote
/// flow. Override hooks are `Box<dyn Fn(&Req) -> Result<Resp, Status> + Send + Sync>`.
#[derive(Default)]
pub struct Responses {
    // The fake "function body": given the decoded (args, kwargs) bytes, produce the
    // runner-envelope bytes FunctionGetOutputs returns. Default echoes a fixed
    // success envelope; override per-test. (Python's `function_body`.)
    pub(crate) function_result: Option<FunctionResultFn>,
    pub(crate) on_function_get_outputs: Option<OverrideFn<gen::FunctionGetOutputsRequest, gen::FunctionGetOutputsResponse>>,
    pub(crate) on_function_create:      Option<OverrideFn<gen::FunctionCreateRequest, gen::FunctionCreateResponse>>,
    pub(crate) on_function_get:         Option<OverrideFn<gen::FunctionGetRequest, gen::FunctionGetResponse>>,
    // ... one optional hook per RPC a test may want to steer.
}
```

Two ergonomic surfaces, both on the builder:

- **`.function_result(...)`** — the common case. The test gives the canned
  OUTPUT (decoded) and the mock CBOR-encodes the runner envelope around it. Two
  flavours:
  - `.function_result_value(serde_json::json!({"sum": 42}))` → mock wraps it as
    `{"ok":true,"value":{...}}`, CBOR-encodes the envelope STRING (the shape
    `.remote()` expects, proven in the spike), and returns it.
  - `.function_result_envelope("{\"ok\":true,\"value\":{\"sum\":42}}")` → exact
    envelope string, for error-envelope cases.
  - `.function_body(|args_json: &str| -> serde_json::Value { ... })` → compute the
    output from the decoded input (Python parity). The mock decodes the inbound
    CBOR `(args, kwargs)` where `args = (entrypoint, input_json)` (the facade's
    shape, `app.rs:217`), passes `input_json` to the closure, wraps the result.

- **`.on_<rpc>(|req| Ok(resp))`** — the escape hatch for any RPC, to force a
  server warning, an error `Status`, or a specific id. Signature:
  `.on_function_get_outputs(|req: &FunctionGetOutputsRequest| Ok(custom_resp))`.

### 4.2 Builder

```rust
pub struct MockModalBuilder { responses: Responses }

impl MockModal {
    pub fn builder() -> MockModalBuilder { MockModalBuilder::default() }
}
impl MockModalBuilder {
    pub fn function_result_value(mut self, v: serde_json::Value) -> Self { ... ; self }
    pub fn function_result_envelope(mut self, s: impl Into<String>) -> Self { ... ; self }
    pub fn function_body<F>(mut self, f: F) -> Self
        where F: Fn(&str) -> serde_json::Value + Send + Sync + 'static { ... ; self }
    pub fn on_function_create<F>(mut self, f: F) -> Self
        where F: Fn(&FunctionCreateRequest) -> Result<FunctionCreateResponse, Status> + Send + Sync + 'static { ... ; self }
    // ... one on_<rpc> per steerable RPC.

    /// Bind a loopback port, start the server task, return the live handle.
    pub async fn start(self) -> std::io::Result<MockModal> { ... }
}

/// Zero-config happy-path mock (the table-test and quick-test common case).
impl MockModal {
    pub async fn start() -> std::io::Result<MockModal> { Self::builder().start().await }
}
```

---

## 5. The `MockModal` handle (`src/server.rs`)

Owns the server task + the shared `RequestLog`. Tears down on drop (abort the
task). Mirrors the spike's bring-up (proven), plus `router` (GOTCHA 2).

```rust
pub struct MockModal {
    addr: std::net::SocketAddr,
    log: RequestLog,
    task: tokio::task::JoinHandle<()>,   // aborted on Drop
}

impl MockModal {
    /// The mock's base URL, e.g. `http://127.0.0.1:54321`. Feed to the SDK / facade.
    pub fn url(&self) -> String { format!("http://{}", self.addr) }

    /// A ready-to-use ModalConfig pointed at this mock, with dummy creds (no real
    /// Modal). Convenience for the SDK-level tests.
    pub fn modal_config(&self) -> modal_rust_sdk::ModalConfig {
        modal_rust_sdk::ModalConfig {
            profile: "mock".into(), server_url: self.url(),
            token_id: "ak-mock".into(), token_secret: "as-mock".into(),
            environment: Some("main".into()), image_builder_version: None,
        }
    }

    // typed query surface forwarded from RequestLog:
    pub fn requests<T: FromRecorded + Clone>(&self) -> Vec<T> { self.log.requests::<T>() }
    pub fn last<T: FromRecorded + Clone>(&self) -> Option<T> { self.log.last::<T>() }
    pub fn took<T: FromRecorded + Clone>(&self) -> usize { self.log.took::<T>() }
    pub fn all_requests(&self) -> Vec<RecordedRequest> { self.log.all() }
}

impl Drop for MockModal { fn drop(&mut self) { self.task.abort(); } }
```

Bring-up body (proven in the spike):

```rust
let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
let addr = listener.local_addr()?;
let incoming = tonic::transport::server::TcpIncoming::from(listener).with_nodelay(Some(true));
let task = tokio::spawn(async move {
    let _ = tonic::transport::Server::builder()
        .add_service(ModalClientServer::new(servicer))
        .serve_with_incoming(incoming).await;
});
```

`MockModal` derefs to nothing magic — it just forwards the query methods. A tiny
`prelude` (`pub mod prelude { pub use crate::{MockModal, RecordedRequest, ...}; pub
use crate::proto::api::*; }`) lets tests write
`use modal_rust_testkit::prelude::*;` and name `FunctionCreateRequest` directly.

---

## 6. The MockServicer (`src/servicer.rs`)

`impl ModalClient for MockServicer` under `#[tonic::async_trait]`. Hand-write the
~18 RPCs the SDK calls; everything else via `mock_unimplemented!`. Each
hand-written RPC: build the `RecordedRequest` variant, `self.log.push(..)`, run
the per-RPC override if present, else return the deterministic default.

### 6.1 The exact RPCs to implement for the first cut + happy-path defaults

Derived from a grep of the SDK's actual stub calls (`ops/*.rs` + `client.rs`) —
this is the precise "~17" (it is **18** including the streaming one, which is
optional):

| # | RPC (snake) | Where the SDK calls it | Default response |
| --- | --- | --- | --- |
| 1 | `client_hello` | `client.rs:174` (every connect) | empty `ClientHelloResponse` |
| 2 | `app_get_or_create` | `client.rs:207`, `ops/app.rs` (deploy lookup) | `app_id="ap-1"` |
| 3 | `app_create` | `ops/app.rs` (`app_create_ephemeral`, RUN path) | `app_id="ap-1"` |
| 4 | `app_publish` | `ops/app.rs` (`app_publish_ephemeral` RUN, persistent DEPLOY) | `AppPublishResponse{}` (echo; the `_app_publish` id-shape check is optional) |
| 5 | `environment_get_or_create` | `client.rs:142` (builder-version resolution) | `metadata.settings.image_builder_version="2025.06"` (a modern value so the SDK's `mount_client_dependencies` gate stays consistent — see `client.rs:90-106`) |
| 6 | `blob_create` | `ops/blob.rs`, `ops/local_dir.rs` (large mount files) | `blob_id="bl-1"`, an upload URL that points back at a tiny in-mock PUT sink OR (simpler) return inline-only and never trigger blob upload for small example mounts |
| 7 | `mount_get_or_create` | `ops/mount.rs` (client mount, python-standalone mount, source mount) | `mount_id="mo-{n}"` (incrementing; content-dedup optional) |
| 8 | `mount_put_file` | `ops/mount.rs`/`ops/local_dir.rs` (per-file upload) | `MountPutFileResponse{ exists:true }` (accept) |
| 9 | `image_get_or_create` | `ops/image.rs:486` | `image_id="im-{n}"` + **inline `result.status=SUCCESS`** so the SDK SHORT-CIRCUITS and never opens `ImageJoinStreaming` (`ops/image.rs:501`) |
| 10 | `function_precreate` | `ops/function.rs:304` | `function_id="fu-1"` (non-empty; the SDK errors on empty, `function.rs:311`) |
| 11 | `function_create` | `ops/function.rs:370` | `function_id="fu-1"` + `handle_metadata.definition_id="de-1"` (non-empty required, `function.rs:377`) |
| 12 | `function_get` | `ops/function.rs:420` (DEPLOY `call` path `from_name`) | `function_id="fu-1"` |
| 13 | `function_map` | `ops/invoke.rs` (remote/spawn/map) | `function_call_id="fc-1"`, **echo pipelined inputs** so the SDK skips the fix-#3 `FunctionPutInputs` fallback (`invoke.rs:188`) — UNLESS testing the MAP path (empty pipelined → exercises put) |
| 14 | `function_put_inputs` | `ops/invoke.rs` (map path + fix-#3 fallback) | `inputs=[{idx,input_id}…]` (non-empty; SDK errors on empty, `invoke.rs:204`) |
| 15 | `function_get_outputs` | `ops/invoke.rs:260` (poll loop) | ONE terminal SUCCESS `GenericResult` whose `data = CBOR(envelope_string)`, `data_format = CBOR`, `idx=0`, plus `last_entry_id` advanced (PROVEN in the spike) |
| 16 | `secret_get_or_create` | `ops/secret.rs` (`#[function(secrets=..)]`) | `secret_id="sc-1"` |
| 17 | `volume_get_or_create` | `ops/volume.rs` (P6 cache + user volumes) | `volume_id="vo-{n}"` |
| 18 | `image_join_streaming` (STREAMING) | `ops/image.rs:567` (only if image result not inline-success) | OPTIONAL — implement as a concrete boxed stream yielding one terminal SUCCESS (proven in spike). Not needed when #9 returns inline success. |

Everything else (the other ~183 RPCs) → `mock_unimplemented!`.

**v1 scope note.** For the first cut, default #6/#8 so small example mounts never
need a real blob PUT (return `mount_put_file{exists:true}` and let
`mount_get_or_create` succeed with a canned id). If a test exercises a large file
that triggers `blob_create` + an HTTP PUT, either (a) add a tiny in-mock HTTP PUT
sink, or (b) keep example payloads small so the inline path is taken. Prefer (b)
for v1; note (a) as a follow-up.

### 6.2 The fake function body / envelope (the `.remote()`/`call` heart)

`function_get_outputs` (and the MAP loop) must return the runner ENVELOPE as the
facade expects (`R = String`, `app.rs:217`; `parse_envelope` then decodes
`value → Out`, `remote.rs:669`). Default + overrides:

- **Default:** echo a fixed success envelope `{"ok":true,"value":{}}` (works for
  `Out = ()`-ish), OR — better default — decode the inbound CBOR `(args,kwargs)`,
  pull `input_json` from `args.1`, and echo `{"ok":true,"value":<parsed input>}`
  (a useful "echo" body). Make the builder default explicit and documented.
- **`.function_result_value(json)`** → `{"ok":true,"value":json}`.
- **`.function_body(|input_json| json)`** → `{"ok":true,"value": f(input_json)}`
  (Python parity; lets a table test compute per-case outputs).

The mock CBOR-encodes the envelope STRING via `modal_rust_sdk::codec::encode`
(reuse the SDK codec so encode/decode are byte-identical — the spike did exactly
this). Error envelopes (`{"ok":false,"error":{kind,..}}`) are settable via
`.function_result_envelope(..)` to drive the `parse_envelope` error taxonomy
(`remote.rs:686`) offline.

---

## 7. Example tests (prove the mock works on a MAIN feature, offline)

Two test files in `crates/modal-rust/tests/` (facade dev-dependency on the
testkit), proving rust-like ergonomics BOTH ways. They run with NO Modal creds
and NO network beyond loopback.

### 7.1 Regular `#[tokio::test]` — facade end-to-end deploy/remote flow (PREFERRED)

Drives the REAL facade through the SDK against the mock and asserts offline. Uses
the test-only injection seam from §8 so the `App` dials the mock.

```rust
// crates/modal-rust/tests/mock_remote.rs
use modal_rust::App;
use modal_rust_testkit::prelude::*;          // MockModal + FunctionCreateRequest, ...
use example_add::{AddInput, AddOutput, modal_registry};

#[tokio::test]
async fn remote_add_against_mock_records_manifest_and_decodes_output() {
    // 1. Start the in-process mock with a canned function result.
    let mock = MockModal::builder()
        .function_result_value(serde_json::json!({ "sum": 42 }))  // the canned output
        .start().await.expect("mock up");

    // 2. Connect a REAL facade App at the mock (test-only seam, §8). No env vars.
    let app = App::connect_at("mock-app", modal_registry(), mock.url())
        .await.expect("connect");

    // 3. Exercise a MAIN feature: .remote() — drives the full ensure_function
    //    manifest (mounts, image, precreate, FunctionCreate, ephemeral publish)
    //    then FunctionMap/GetOutputs, and DECODES the canned output.
    let out: AddOutput = app.function("add")
        .remote(AddInput { a: 40, b: 2 }).await.expect("remote");
    assert_eq!(out.sum, 42);   // the facade decoded the mock's canned envelope

    // 4. Assert the captured manifest: the FunctionCreate the mock recorded.
    let fc = mock.last::<FunctionCreateRequest>().expect("FunctionCreate sent");
    let function = fc.function.expect("Function set (FILE mode, XOR with function_data)");
    assert!(fc.function_data.is_none(), "FILE-mode XOR invariant");
    assert_eq!(function.module_name, "modal_rust_run_wrapper"); // WRAPPER_MODULE (remote.rs:26)
    assert_eq!(function.function_name, "handler");
    assert_eq!(function.mount_ids.len(), 2, "RUN path: client + source mounts");
    // and the invoke fired:
    assert_eq!(mock.took::<FunctionMapRequest>(), 1);
    assert!(mock.took::<FunctionGetOutputsRequest>() >= 1);
}
```

This covers: App::connect (ephemeral `AppCreate` + `ClientHello`), the whole
`ensure_function` RUN manifest, the FunctionCreate field assertions (name/module/
mount-count/the FILE-mode XOR), the CBOR invoke round-trip, and the typed decode
— ALL offline. (If wiring the full `ensure_function` upload is too deep for the
very first commit, the narrower-but-real fallback is the SDK-ops test in §7.3 —
clearly state the coverage; no silent cap.)

### 7.2 Table test — captured FunctionCreate fields across decorator configs

Proves table-test ergonomics: the mock + its per-case config is a value built in
a loop. Asserts the captured manifest per case (gpu None/"T4", timeout, secrets,
volumes), which exercises the P4 decorator-config → `FunctionResources`/timeout
flow offline.

```rust
// crates/modal-rust/tests/mock_table.rs
use modal_rust_testkit::prelude::*;

struct Case { name: &'static str, gpu: Option<&'static str>, timeout: u32,
              expect_gpu_set: bool }

#[tokio::test]
async fn function_create_manifest_table() {
    let cases = [
        Case { name: "cpu",  gpu: None,       timeout: 600,  expect_gpu_set: false },
        Case { name: "t4",   gpu: Some("T4"), timeout: 1800, expect_gpu_set: true  },
    ];
    for c in cases {
        // Each case spins its OWN mock + its OWN facade App — independent loopback
        // ports, no shared global state (the env-var path could NOT do this).
        let mock = MockModal::start().await.expect("mock");
        // Build a RemoteConfig with this case's gpu/timeout (or via a decorated
        // App::from_inventory variant). Drive ONE .remote() to emit FunctionCreate.
        let app = App::connect_at_with("table-app", /* registry */, mock.url(),
                                       /* RemoteConfig{ gpu: c.gpu, timeout } */).await.unwrap();
        let _ = app.function("add").remote::<_, serde_json::Value>(
            serde_json::json!({"a":1,"b":2})).await;

        let fc = mock.last::<FunctionCreateRequest>()
            .unwrap_or_else(|| panic!("case {}: no FunctionCreate", c.name));
        let f = fc.function.unwrap();
        assert_eq!(f.timeout_secs, c.timeout, "case {}: timeout", c.name);
        let gpu_set = f.resources.and_then(|r| r.gpu_config).is_some();
        assert_eq!(gpu_set, c.expect_gpu_set, "case {}: gpu", c.name);
    }
}
```

(Exact field names: `FunctionResources::to_proto` populates `resources.gpu_config`
for a non-None GPU — see `ops/function.rs:441-610`. The table asserts that
projection. The case-shape is illustrative; the load-bearing point is "one mock +
one config per case in a loop", which the per-port `MockModal` makes clean.)

### 7.3 Narrower fallback (SDK-ops level, NO facade change) — keep as a 3rd test

Proves the same recording with zero facade dependency — exactly the spike shape,
promoted to a test in `crates/modal-rust-sdk/tests/mock_ops.rs`:

```rust
let mock = MockModal::start().await?;
let mut client = modal_rust_sdk::ModalClient::from_config(mock.modal_config()).await?;
let fid = client.function_create("ap-1", "fu-pre-1", &spec).await?;
assert_eq!(fid.function_id, "fu-1");
let fc = mock.last::<FunctionCreateRequest>().unwrap();
assert_eq!(fc.function.unwrap().image_id, spec.image_id);
```

This needs NO facade injection (uses `from_config` directly) and is the safest
first green test (it is literally the spike). Land it FIRST, then 7.1/7.2.

---

## 8. Facade `server_url` injection decision

**Decision: a small, additive, test-only injection seam is NEEDED for a
facade-level example test; the SDK-level test needs none.**

- **SDK level — no change needed.** `ModalClient::from_config(ModalConfig{
  server_url, .. })` already injects an arbitrary URL (`client.rs:63`) and the
  channel dials plain `http://` (`channel.rs:29`). The spike used exactly this.
  So §7.3 works today.

- **Facade level — env-var path EXISTS but is unsuitable.** `App::connect`
  → `connect_inner` → `ModalClient::connect()` (`app.rs:169`), which reads
  `read_modal_config()`; `apply_env_overrides` honors `MODAL_SERVER_URL` +
  `MODAL_TOKEN_ID`/`MODAL_TOKEN_SECRET` (`config.rs:204-219`). So setting those
  env vars points the facade at the mock with NO code change. **But env vars are
  process-global**, which breaks `cargo test`'s parallel execution and the table
  test (each case needs its OWN mock on its OWN port). Mutating `std::env` in
  tests is also `unsafe` in edition 2024-adjacent toolchains and racy. **Reject
  the env path for the example tests.**

- **Therefore add ONE additive, test-only constructor** on `App` that threads an
  explicit `server_url` into the `ModalConfig` (everything else unchanged). It
  reuses the existing private `connect_inner`; the only new code is building a
  `ModalConfig` with the given URL + dummy creds and calling
  `ModalClient::from_config` instead of `::connect`. Proposed signatures
  (`#[doc(hidden)]` or `#[cfg(any(test, feature = "test-inject"))]`-gated to keep
  it out of the public API surface):

  ```rust
  impl App {
      /// TEST-ONLY: connect at an explicit server_url (e.g. an in-process mock),
      /// using the given registry. Dummy creds. Additive; does not change connect().
      #[doc(hidden)]
      pub async fn connect_at(name: &str, registry: Registry, server_url: String)
          -> Result<Self> { /* build ModalConfig{server_url,..}; from_config; connect_inner shape */ }

      /// As connect_at, plus an explicit RemoteConfig (gpu/timeout/etc.) for the
      /// table test. Headless variant pairs with from_manifest.
      #[doc(hidden)]
      pub async fn connect_at_with(name: &str, registry: Registry, server_url: String,
          run_config: RemoteConfig) -> Result<Self> { ... }
  }
  ```

  Smallest correct change: refactor `connect_inner` to take the already-built
  `ModalClient` (or a `ModalConfig`) instead of calling `ModalClient::connect()`
  itself; `connect`/`connect_with_registry` keep calling `::connect()`, the new
  `connect_at*` call `::from_config(mock config)`. This is purely additive — the
  public `connect`/`connect_with_registry`/`deploy`/`call` API and all existing
  behavior are unchanged. **This is the only facade change in the whole effort,
  and it is allowed by the task's "small test-only injection IF the facade cannot
  already be pointed at a custom server" clause** (it can only via global env,
  which is unsuitable).

  Gate it so it is not part of the shipped public API: prefer
  `#[cfg(any(test, feature = "testkit"))]` with a `testkit` feature on
  `modal-rust` enabled only by the test target, OR `#[doc(hidden)]` if a method
  must be reachable from an integration test in `tests/` (integration tests link
  the crate as an external dependency, so `#[cfg(test)]` alone won't expose it —
  use the `testkit` feature, enabled in `[dev-dependencies]`/`required-features`).

---

## 9. Build order (smallest correct steps)

1. **Scaffold the crate** (`Cargo.toml` + `build.rs` + copied proto + `proto.rs`)
   and confirm `cargo build -p modal-rust-testkit` compiles the generated server.
2. **`macros.rs` + the generated arm list** + `servicer.rs` with `client_hello`
   + `function_get` hand-written, rest stubbed. Confirm it compiles (this is the
   spike state — already proven).
3. **`record.rs`** (enum + `RequestLog` + `FromRecorded` + accessors) and wire
   the two hand-written RPCs to record.
4. **`server.rs` + `builder.rs`** (`MockModal::builder().start()`, `url()`,
   Drop). Port the SDK-ops test §7.3 → first green offline test.
5. **Add the remaining ~16 hand-written RPCs** with their happy-path defaults
   (§6.1) + the `function_get_outputs` fake body (§6.2). Confirm a raw SDK
   `ensure_function`-shaped sequence records the full manifest.
6. **`responder.rs`** overrides (`.function_result_*`, `.function_body`,
   `.on_<rpc>`).
7. **Facade seam** `App::connect_at*` (§8) — the only facade change.
8. **Example tests** §7.1 (facade end-to-end) + §7.2 (table). Land both green.
9. **`prelude` + crate docs.**

---

## 10. Verification (offline = HARD gate; NO Modal, NO Python)

Run on default-members PLUS the testkit + facade tests explicitly:

```
cargo fmt --check
cargo clippy -p modal-rust-sdk -p modal-rust -p modal-rust-testkit --all-targets -- -D warnings
cargo build  -p modal-rust-sdk -p modal-rust -p modal-rust-testkit
cargo test   -p modal-rust-sdk -p modal-rust -p modal-rust-testkit
# (CI also runs the existing default-members gate unchanged.)
```

Acceptance:
- All four green. Paste exact output + the new test names
  (`remote_add_against_mock_records_manifest_and_decodes_output`,
  `function_create_manifest_table`, plus the SDK-ops `mock_ops` test).
- QUOTE the example tests so the rust-like ergonomics (`mock.last::<FunctionCreateRequest>()`,
  the table loop with a per-case `MockModal`) are visible.
- Confirm the shipped crates do NOT depend on the testkit: `grep` that
  `modal-rust-testkit` appears ONLY under `[dev-dependencies]` (and the testkit
  crate itself), and that it is NOT in root `default-members`. The example tests
  PASS offline (loopback only), with NO Modal credentials and NO `live` feature.

---

## 11. Citations (verified against the working tree, 2026-06-05)

- Service + RPC count: `crates/modal-rust-sdk/proto/api.proto:4129`
  (`service ModalClient`), 201 RPCs (189 unary + 12 server-streaming).
- Transport (zero-change dial): `channel.rs:29` (TLS only for `https://`),
  `client.rs:63` (`from_config` injects `server_url`), `client.rs:72`
  (`ClientHello` on connect), `config.rs:204-219` (`MODAL_SERVER_URL` /
  `MODAL_TOKEN_*` env overrides).
- SDK build flag: `crates/modal-rust-sdk/build.rs:10-11` (`build_server(false)`).
- The exact RPCs the SDK calls: `client.rs:142` (`environment_get_or_create`),
  `client.rs:174` (`client_hello`), `client.rs:207` (`app_get_or_create`),
  `ops/function.rs:304/370/420` (precreate/create/get),
  `ops/invoke.rs:173/198/260` (map/put/get_outputs), `ops/image.rs:486/501/567`
  (image get-or-create / inline-success short-circuit / streaming),
  `ops/mount.rs`/`ops/local_dir.rs` (mount + blob), `ops/app.rs`
  (create/publish), `ops/secret.rs`, `ops/volume.rs`.
- Facade RUN manifest + invoke shape: `remote.rs:458-664` (`ensure_function`),
  `app.rs:163-226` (`connect_inner` → `ModalClient::connect`; `remote_invoke`
  invokes with `(entrypoint, input_json)`, `R = String`), `remote.rs:669`
  (`parse_envelope`).
- Facade `.local()` proof / decode taxonomy: `function.rs:42-80`,
  `remote.rs:686` (error reconstruction).
- Python prior art (pattern only, not reused):
  `references/modal-client/py/test/conftest.py:625` (`MockClientServicer`),
  `:2013-2029` (FunctionGetOutputs fake body), `modal/_utils/grpc_testing.py:113`
  (`add_response`), `:126` (`set_responder`), `:155` (`get_requests`).
- Spike artifacts: `workpads/shim-backend/mock-spike-main.rs.txt`,
  `mock-spike-Cargo.toml.txt`, `mock-spike-build.rs.txt`,
  `mock-spike-run-output.txt`.
