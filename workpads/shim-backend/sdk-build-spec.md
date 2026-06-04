# modal-rust-sdk — Authoritative Build Spec (FILE mode, CBOR)

Single source of truth for building `crates/modal-rust-sdk` (package `modal-rust-sdk`,
lib `modal_rust_sdk`): a lean, first-party Rust gRPC client that talks to Modal's
control plane directly. Synthesized from three design notes (auth/channel; proto/RPC +
client-mount; layout/deps) and cross-checked against the canonical proto, the modal-rs
precedent, and the proven spike recipe. **Every field number and enum value below was
verified against the canonical proto** (see §0). Where the notes disagreed, the
resolution is called out inline and prefers what the spike actually proved.

All cited proto field/line numbers are from the **canonical proto**:
`references/modal-rs/references/modal-client/modal_proto/api.proto`
(4390 LOC, `package modal.client`, `service ModalClient` @4129).

---

## 0. Verified facts (do not re-derive)

- Canonical proto imports ONLY 5 google well-known types (lines 7-11): `any.proto`,
  `empty.proto`, `struct.proto`, `timestamp.proto`, `wrappers.proto`. All satisfied by
  `protoc-bin-vendored`'s include path. **Nothing custom to vendor.**
- `task_command_router.proto` is NOT imported by api.proto and is sandbox-only.
  **We compile ONLY `api.proto`** (modal-rs compiled both; we do not).
- `ClientType::CLIENT_TYPE_CLIENT = 1` (line 84). `DataFormat`: `DATA_FORMAT_PICKLE = 1`,
  `DATA_FORMAT_CBOR = 4` (lines 112, 115). `DeploymentNamespace::DEPLOYMENT_NAMESPACE_GLOBAL = 3`
  (line 121). `ObjectCreationType`: `UNSPECIFIED = 0` ("just lookup"),
  `CREATE_IF_MISSING = 1`, `ANONYMOUS_OWNED_BY_APP = 4` (lines 208-212).
- `Function` field numbers (verified @1646-1810): `module_name=1`, `function_name=2`,
  `mount_ids=3`, `image_id=4`, `function_serialized=6`, `definition_type=7`,
  `function_type=8`, `resources=9`, `timeout_secs=21`, `app_name=31`, `volume_mounts=33`,
  `supported_input_formats=87`, `supported_output_formats=88`.
- `FunctionInput` (@2163): `args=1` / `args_blob_id=7` (oneof `args_oneof`), `final_input=9`,
  `data_format=10`, `method_name=11`. `FunctionPutInputsItem` (@2238): `idx=1`, `input=2`.
  `FunctionMapRequest` (@2174): `function_id=1`, `parent_input_id=2`, `return_exceptions=3`,
  `function_call_type=4`, `pipelined_inputs=5`, `function_call_invocation_type=6`,
  `from_spawn_map=7`. `GenericResult` (@2343): `status=1`, `exception=2`,
  `GENERIC_STATUS_SUCCESS=1`.
- `AppCreateRequest` (@364): `client_id=1`, `description=2`, `environment_name=5`,
  `app_state=6`, `tags=7`.
- The whole stack **compiles cleanly** with tonic 0.14.x + tonic-prost 0.14.x +
  prost 0.14.3 + tonic-prost-build 0.14.x + protoc-bin-vendored 3, `build_client(true)`
  + `build_server(false)`. Generated module: `tonic::include_proto!("modal.client")`
  → `modal_client_client::ModalClientClient`, zero server symbols.
- Workspace root `Cargo.toml`: `members` = lines 3-11, `default-members` = lines 20-27,
  `[profile.release/dev] panic = "unwind"` = lines 33-37 (workspace-wide, covers the new
  crate, no edit needed). Root defines NO `[workspace.package]`/`[workspace.dependencies]`
  today → pin versions inline.

---

## 1. File layout (each file ~300-500 LOC, one responsibility)

```
crates/modal-rust-sdk/
├── Cargo.toml
├── build.rs                    # tonic-prost-build + protoc-bin-vendored, client-only, api.proto only
├── NOTICE                      # attribution: modal-client (Apache-2.0/MIT), modal-rs (MIT)
├── proto/
│   └── api.proto               # VENDORED canonical copy (verbatim)
└── src/
    ├── lib.rs                  # crate docs + attribution comment + module decls + re-exports (~120 LOC)
    ├── proto.rs                # tonic::include_proto!("modal.client") wrapper + `pub(crate) use ... as api` (~30 LOC)
    ├── error.rs                # Error enum + Result alias + From impls + helper ctors (~120 LOC)
    ├── config.rs               # ModalConfig + ModalProfile + read_modal_config (env + ~/.modal.toml) (~300 LOC)
    ├── auth.rs                 # AuthInterceptor (full Python header set) + CLIENT_VERSION/CLIENT_TYPE (~150 LOC)
    ├── channel.rs              # Channel/TLS construction + server-url normalize + endpoint hardening (~120 LOC)
    ├── client.rs               # ModalClient: connect()/from_config()/connect_with_credentials(); inner_mut(); ops accessors (~250 LOC)
    ├── codec.rs                # CBOR encode/decode of (args, kwargs) tuple; DataFormat dispatch; PICKLE passthrough (~150 LOC)
    └── ops/
        ├── mod.rs              # re-export ops submodules + shared small helpers (poll/retry) (~60 LOC)
        ├── app.rs              # AppGetOrCreate (preferred) / AppCreate (ephemeral) + AppPublish (fix #2) (~250 LOC)
        ├── image.rs            # ImageGetOrCreate (from_registry python:3-slim + wrapper bake) + ImageJoinStreaming poll (~350 LOC)
        ├── mount.rs            # client-mount resolution: MountGetOrCreate "modal-client-mount-{version}" GLOBAL → mount_id (~200 LOC)
        ├── function.rs         # FunctionPrecreate + FunctionCreate (fix #1) FILE-mode + FunctionGet (~450 LOC)
        └── invoke.rs           # FunctionMap → FunctionPutInputs fallback (fix #3) → poll FunctionGetOutputs (~400 LOC)
```

Splitting rationale:
- `channel.rs` is split out of `client.rs` (modal-rs bundles them) to isolate TLS/endpoint
  logic and keep each file ≤300 LOC.
- `ops/` is a directory module; each Modal operation family is its own ≤450 LOC file.
  `function.rs` is the largest; if it exceeds ~500 LOC, split `FunctionGet`/`FunctionPrecreate`
  into `ops/function_get.rs`.
- The wrapper Python module (FILE mode) is baked via `run_commands` for now (documented
  shortcut, same as spike); the **client mount in `ops/mount.rs` is the Modal-native path**
  for making `modal` importable — prefer it over the spike's `pip install modal` (keep pip
  as a documented fallback). See §6.

---

## 2. Cargo.toml (exact)

```toml
[package]
name = "modal-rust-sdk"
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"
description = "First-party lean Rust gRPC client for Modal's control plane (modal_rust::sdk)"
build = "build.rs"

[lib]
name = "modal_rust_sdk"

[dependencies]
# gRPC stack — versions MATCH the proven modal-rs stack so the proto compiles cleanly.
# default-features = false + explicit client features DROPS the axum (server) dependency.
tonic = { version = "0.14.3", default-features = false, features = ["channel", "transport", "tls-native-roots", "tls-ring", "codegen"] }
tonic-prost = "0.14.3"
prost = "0.14.3"
prost-types = "0.14.3"
# Runtime + serialization
tokio = { version = "1.49", features = ["full"] }
serde = { version = "1.0.228", features = ["derive"] }
serde_cbor = "0.11"            # MATCH modal-rs cbor.rs codec
toml = "0.9"                   # parse ~/.modal.toml
base64 = "0.22"               # bake wrapper source into dockerfile_commands
sha2 = "0.10"                 # content hashing for mounts/images
anyhow = "1"                  # ops-level error context (NOT the public error type — see error.rs)

[build-dependencies]
tonic-prost-build = "0.14.3"
protoc-bin-vendored = "3"

[dev-dependencies]
tokio = { version = "1.49", features = ["full", "test-util"] }
```

Notes on the minimal set:
- **No `reqwest`**: FILE mode needs no blob upload (defer to the SERIALIZED/blob milestone).
- **No `serde_json`, `serde-pickle`, `tokio-stream`, `futures-util`** in the initial cut.
  Add `tokio-stream` only when the `ImageJoinStreaming` / `FunctionMap` streaming response
  bodies are consumed (tonic re-exposes the stream types; pull `tokio-stream` then).
- `edition = "2021"` to match the existing modal-rust crates. Do NOT inherit modal-rs's 2024.
- If the workspace later adds `[workspace.package]`/`[workspace.dependencies]`, switch to
  `.workspace = true` inheritance. Today the root defines neither → pin inline as above.

---

## 3. build.rs (exact)

```rust
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let protoc_path = protoc_bin_vendored::protoc_bin_path()?;
    // SAFETY: build scripts run single-threaded before protobuf compilation.
    unsafe { std::env::set_var("PROTOC", protoc_path) };
    let protoc_include = protoc_bin_vendored::include_path()?;

    tonic_prost_build::configure()
        .build_client(true)
        .build_server(false) // client only — drops server codegen (and axum)
        .compile_protos(
            &["proto/api.proto"],
            &[
                "proto",
                protoc_include.to_str().ok_or("invalid protoc include path")?,
            ],
        )?;

    Ok(())
}
```

`src/proto.rs`:

```rust
pub mod modal {
    pub mod client {
        #![allow(clippy::large_enum_variant, clippy::enum_variant_names, dead_code)]
        tonic::include_proto!("modal.client");
    }
}
pub(crate) use modal::client as api;
```

The `#![allow(...)]` lines are REQUIRED so `cargo clippy -- -D warnings` stays green on
prost-generated code (verified pattern from modal-rs `lib.rs`).

**Vendoring**: copy the canonical proto VERBATIM (do not reference the gitignored clone):
```
cp references/modal-rs/references/modal-client/modal_proto/api.proto \
   crates/modal-rust-sdk/proto/api.proto
```
The modal-rs vendored copy is the fallback ONLY if a future proto bump introduces an
unsatisfiable import.

---

## 4. Auth / channel / config

### 4.1 ModalConfig (config.rs)

```rust
pub struct ModalConfig {
    pub profile: String,                       // selected TOML table name
    pub server_url: String,                    // default "https://api.modal.com"
    pub token_id: String,                      // required
    pub token_secret: String,                  // required
    pub environment: Option<String>,           // default env for ops; default "main" at call sites
    pub image_builder_version: Option<String>,
}
```

Resolution order (matches modal-rs `utils/config.rs:65-215` + Python `config.py` precedence):

1. **Config file path**: `MODAL_CONFIG_PATH` env if set, else `$HOME/.modal.toml`.
2. **Profile selection**: `MODAL_PROFILE` names the TOML table; else first profile with
   `active = true`; final fallback = first table in the file.
3. **Profile fields** (snake_case canonical, camelCase serde aliases as courtesy):
   `token_id`/`tokenId`, `token_secret`/`tokenSecret`, `server_url`/`serverUrl`
   (default `https://api.modal.com`), `environment`,
   `image_builder_version`/`imageBuilderVersion`, `active` (bool, selector).
4. **Env-var overrides** (applied last, win over file):
   `MODAL_TOKEN_ID`, `MODAL_TOKEN_SECRET`, `MODAL_SERVER_URL`, `MODAL_ENVIRONMENT`,
   `MODAL_IMAGE_BUILDER_VERSION`.
5. **Validation**: empty/whitespace `token_id` or `token_secret` is a hard `Error::Config`.
   Trim-then-empty → treated as unset throughout.
6. **DEVIATION FROM modal-rs (adopt)**: if `MODAL_TOKEN_ID` + `MODAL_TOKEN_SECRET` are both
   present in the env, allow construction even when no `~/.modal.toml` exists (CI/container
   friendliness). Make the file optional when env tokens are complete. modal-rs requires a
   readable file first; Python reads tokens from both env and file.
7. **DROP** modal-rs's `task_command_router_*` config fields (we never use the router).

### 4.2 Channel + TLS (channel.rs, tonic 0.14)

- Default endpoint `https://api.modal.com`. https → port 443, h2 + TLS.
- `normalize_server_url`: prepend `https://` if the URL has no `http://`/`https://` scheme.
- `ClientTlsConfig::new().with_native_roots()` (OS trust store; `tls-native-roots` feature;
  `tls-ring` selects the ring crypto backend). Apply TLS ONLY when the URL starts with
  `https://` so a local `http://` dev server still works.
- **Endpoint hardening (Python parity, grpc_utils.py:196-214)** — required for large
  image/CBOR payloads and long polls:

```rust
use tonic::transport::{Channel, ClientTlsConfig};
use std::time::Duration;

pub async fn build_channel(server_url: &str) -> Result<Channel, crate::Error> {
    let url = normalize_server_url(server_url);
    let mut endpoint = Channel::from_shared(url.clone())
        .map_err(|e| crate::Error::Invalid(format!("invalid Modal server url: {e}")))?
        .tcp_keepalive(Some(Duration::from_secs(30)))
        .http2_keep_alive_interval(Duration::from_secs(30))
        .keep_alive_timeout(Duration::from_secs(20))
        .initial_stream_window_size(Some(64 * 1024 * 1024))      // 64 MiB
        .initial_connection_window_size(Some(64 * 1024 * 1024));
    if url.starts_with("https://") {
        endpoint = endpoint.tls_config(ClientTlsConfig::new().with_native_roots())?;
    }
    endpoint.connect().await.map_err(crate::Error::from)
}

fn normalize_server_url(url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else {
        format!("https://{url}")
    }
}
```

### 4.3 AuthInterceptor (auth.rs) — EXACT gRPC metadata, every request

**RESOLVED CONTRADICTION**: the auth note says send the FULL Python header set; the
proto/RPC note and layout note describe modal-rs's 3-header subset. **Send the full Python
set** (superset; the layout/proto notes' 3-header description is the modal-rs precedent to
extend, not the target). The header that matters most for being treated as a first-class
client is `x-modal-client-type = "1"`.

```rust
const CLIENT_VERSION: &str = "1.3.2";   // our SDK version (bump from modal-rs "1.3.1"; matches Modal 1.3.2 facts)
const CLIENT_TYPE_CLIENT: &str = "1";   // ClientType::CLIENT_TYPE_CLIENT (api.proto:84), as the enum int string
```

Inserted on EVERY unary/stream call:

| header | value | notes |
|---|---|---|
| `x-modal-token-id` | `token_id` | **hard auth** |
| `x-modal-token-secret` | `token_secret` | **hard auth** |
| `x-modal-client-type` | `"1"` | **hard** — the Python identity (not libmodal 7/8/9); we replicate the Python FILE-mode path |
| `x-modal-client-version` | `CLIENT_VERSION` | **hard** |
| `x-modal-platform` | URL-encoded `"{system}-{release}-{machine}"` | diagnostic, harmless |
| `x-modal-timestamp` | `format!("{}", unix_secs_f64)` | per-call, grpc_utils.py:364 |

`x-modal-python-version`, `x-modal-node`, `x-modal-auth-token` are NOT needed for the
control-plane FILE-mode path; omit them. (`x-modal-auth-token` is input-plane/sandbox only.)

```rust
use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::Interceptor;
use tonic::{Request, Status};

#[derive(Clone)]
pub struct AuthInterceptor {
    token_id: MetadataValue<Ascii>,
    token_secret: MetadataValue<Ascii>,
    client_version: MetadataValue<Ascii>,
    client_type: MetadataValue<Ascii>,
    platform: MetadataValue<Ascii>,
}

impl AuthInterceptor {
    pub fn new(token_id: &str, token_secret: &str) -> Result<Self, crate::Error> {
        Ok(Self {
            token_id: token_id.parse().map_err(crate::Error::invalid_metadata)?,
            token_secret: token_secret.parse().map_err(crate::Error::invalid_metadata)?,
            client_version: CLIENT_VERSION.parse().unwrap(),
            client_type: CLIENT_TYPE_CLIENT.parse().unwrap(),
            platform: platform_string().parse().unwrap_or_else(|_| "unknown".parse().unwrap()),
        })
    }
}

impl Interceptor for AuthInterceptor {
    fn call(&mut self, mut req: Request<()>) -> Result<Request<()>, Status> {
        let md = req.metadata_mut();
        md.insert("x-modal-token-id", self.token_id.clone());
        md.insert("x-modal-token-secret", self.token_secret.clone());
        md.insert("x-modal-client-type", self.client_type.clone());
        md.insert("x-modal-client-version", self.client_version.clone());
        md.insert("x-modal-platform", self.platform.clone());
        if let Ok(ts) = format_unix_secs().parse::<MetadataValue<Ascii>>() {
            md.insert("x-modal-timestamp", ts);
        }
        Ok(req)
    }
}
```

### 4.4 Client assembly + handshake (client.rs)

```rust
use tonic::codegen::InterceptedService;
use tonic::transport::Channel;
use crate::proto::api::modal_client_client::ModalClientClient;

pub struct ModalClient {
    inner: ModalClientClient<InterceptedService<Channel, AuthInterceptor>>,
    config: ModalConfig,
}

impl ModalClient {
    pub async fn connect() -> Result<Self, crate::Error> {                 // resolve config (§4.1)
        Self::from_config(read_modal_config()?).await
    }
    pub async fn connect_with_credentials(id: &str, secret: &str) -> Result<Self, crate::Error> {
        // bypass file; use https://api.modal.com
        let cfg = ModalConfig { /* id, secret, default url, .. */ };
        Self::from_config(cfg).await
    }
    pub async fn from_config(config: ModalConfig) -> Result<Self, crate::Error> {
        let channel = build_channel(&config.server_url).await?;
        let interceptor = AuthInterceptor::new(&config.token_id, &config.token_secret)?;
        let inner = ModalClientClient::with_interceptor(channel, interceptor);
        let mut client = Self { inner, config };
        client.client_hello().await?;   // fail fast on bad credentials
        Ok(client)
    }
    pub fn inner_mut(&mut self) -> &mut ModalClientClient<InterceptedService<Channel, AuthInterceptor>> {
        &mut self.inner   // low-level escape hatch for ops/
    }
    async fn client_hello(&mut self) -> Result<(), crate::Error> {
        let resp = self.inner.client_hello(()).await?.into_inner();
        // log resp.warning + resp.server_warnings; ignore deprecated image_builder_version
        let _ = resp;
        Ok(())
    }
}
```

- **Connect-time handshake**: `ClientHello(google.protobuf.Empty) -> ClientHelloResponse`
  (@4171). Free, no GPU/cost. Use as the post-connect auth probe; fail fast with a clean
  `Error::Auth`/`Error::Status`. `ClientHelloResponse` carries `warning`, deprecated
  `image_builder_version` (ignore — resolve from config/default), `server_warnings` (log).
- **DEVIATION FROM modal-rs (adopt)**: modal-rs does NOT call ClientHello; we do.
- **Cheapest safe live auth proof on the ops surface**: `AppGetOrCreate` ephemeral
  (§5.1) — free, no GPU, and it is the first real step of the recipe.
- **Retry transient flakes**: wrap connect and each RPC call site in retry-on-transient
  ("socket connection closed unexpectedly" etc. = transient capacity, NOT a design block).
  Retry with backoff; never mark "blocked on Modal".

### 4.5 error.rs

```rust
pub enum Error {
    Transport(tonic::transport::Error),
    Status(tonic::Status),
    Config(String),
    Invalid(String),
    Codec(String),
    Build(String),    // image/function build terminal failure (surface GenericResult.exception)
}
pub type Result<T> = std::result::Result<T, Error>;
// + From<tonic::transport::Error>, From<tonic::Status>; helper ctors invalid_metadata(..), etc.
```

---

## 5. RPC-by-RPC field map (FILE-mode recipe)

Recipe order: `AppGetOrCreate` → `MountGetOrCreate` (client mount) → `ImageGetOrCreate`
+ `ImageJoinStreaming` poll → `FunctionPrecreate` → `FunctionCreate` (FILE) → `AppPublish`
→ `FunctionGet` → `FunctionMap` → `FunctionPutInputs` fallback → `FunctionGetOutputs` poll.

### 5.1 App — AppGetOrCreate (preferred) / AppCreate (ephemeral)

**RESOLVED CONTRADICTION**: spike used the modal-rs `get_or_create_app` path (then published
Deployed). The notes split on AppGetOrCreate vs AppCreate. **Prefer `AppGetOrCreate`**
(idempotent, resume-friendly, doubles as the live auth proof); implement `AppCreate`
(ephemeral) as an alternate. Both are correct; AppGetOrCreate is the default.

- `AppGetOrCreate` (@4142):
  - `AppGetOrCreateRequest` (@487): `app_name=1`, `environment_name=2` (config `environment`,
    default `"main"`), `object_creation_type=3` = `OBJECT_CREATION_TYPE_CREATE_IF_MISSING=1`.
  - `AppGetOrCreateResponse` (@493): `app_id=1` ← threaded through everything.
- `AppCreate` (@4133, alternate ephemeral):
  - `AppCreateRequest` (@364): `client_id=1`, `description=2`, `environment_name=5`,
    `app_state=6` = `APP_STATE_EPHEMERAL=1`, `tags=7`.
  - `AppCreateResponse` (@372): `app_id=1`, `app_page_url=2`, `app_logs_url=3`.
- `AppState` (@30): `EPHEMERAL=1`, `DETACHED=2`, `DEPLOYED=3`, `STOPPED=5`.

### 5.2 Client mount — MountGetOrCreate (the Modal-native `modal`-importable path)

FILE-mode containers boot `python -m modal._container_entrypoint`, so the `modal` client
package MUST be importable in the container. Python attaches a hosted client mount
automatically (`_functions.py:730-734` prepends `_get_client_mount()`;
`mount.py:644-668,733-738`); we do the same. **This REPLACES the spike's `pip install modal`.**

- `MountGetOrCreate` (@4269):
  - `MountGetOrCreateRequest` (@2596): `deployment_name=1`, `namespace=2`,
    `environment_name=3`, `object_creation_type=4`, `files=5` (EMPTY for lookup), `app_id=6`.
  - `MountGetOrCreateResponse` (@2605): `mount_id=1`, `handle_metadata=2`
    (`MountHandleMetadata{content_checksum_sha256_hex}`).
- Resolution (Python `from_name` path):
  - `deployment_name = "modal-client-mount-{version}"` — `client_mount_name()`
    (mount.py:62-69) strips any `+githash`. **`{version}` MUST be the modal client version
    on the worker image we target** (config/const; align with `CLIENT_VERSION` family,
    e.g. `1.3.2`). Make it a single named constant so it is easy to bump.
  - `object_creation_type = OBJECT_CREATION_TYPE_UNSPECIFIED=0` (lookup only; Python's
    `from_name._load` sets no creation type).
  - **`namespace = DEPLOYMENT_NAMESPACE_GLOBAL=3`** (the key field — the hosted client
    mount lives in GLOBAL).
  - `environment_name` = active environment.
- **Attach**: put the returned `mount_id` into `Function.mount_ids` (field 3, @1649).
  Our `Function.mount_ids = [client_mount_id]` (+ wrapper mount id if we mount the wrapper
  rather than baking it).
- **Fallback (documented, NOT default)**: spike's `RUN pip install --no-cache-dir modal`
  baked via `run_commands`. Keep ONLY for when client-mount resolution is unavailable.
- **Wrapper module**: for now bake `/root/<wrapper>.py` via a `run_commands` heredoc
  (`/root` is on sys.path). Optionally later ship it as its own anonymous mount via
  `MountGetOrCreate{object_creation_type=ANONYMOUS_OWNED_BY_APP=4, app_id, files=[MountFile]}`
  + `MountPutFile` (@4270) — not required for the recipe.

### 5.3 Image — ImageGetOrCreate + ImageJoinStreaming poll

- `ImageGetOrCreate` (@4260):
  - `ImageGetOrCreateRequest` (@2431): `image=2`, `app_id=4`, `builder_version=9`,
    `force_build=7`, `namespace=8`, `existing_image_id=5` (ignore), `ignore_cache=11`.
    Set `image`, `app_id`, `builder_version` (from config; default if unset).
  - `ImageGetOrCreateResponse` (@2445): `image_id=1` (**set regardless of build state**),
    `result=2` (`GenericResult`, only when build finished), `metadata=3` (only on success).
- `Image` message (@2384) for `from_registry("python:3-slim")` + wrapper bake (single layer):
  - `dockerfile_commands=6` (repeated string): first line `"FROM python:3-slim"` (we prepend
    the `FROM <tag>`), then the run_commands, e.g. a heredoc that base64-decodes the wrapper
    source to `/root/<wrapper>.py`. (Fallback only: append `"RUN pip install --no-cache-dir modal"`.)
  - `base_images=5` (repeated `BaseImage`): **EMPTY** when basing on a registry tag (a
    `FROM <tag>` line). Only populated (`BaseImage{docker_tag:"base", image_id}`) for layered
    builds where `FROM base` references a prior image_id.
  - `context_files=7` empty; `secret_ids=12` empty; `gpu_config=16` None for CPU;
    `image_registry_config=17` only for private registries
    (`RegistryAuthType::REGISTRY_AUTH_TYPE_PUBLIC=3` / unset for public python image).
- Readiness / poll (`ImageJoinStreaming` @4261; proven loop):
  - If `ImageGetOrCreateResponse.result` is None OR `result.status == 0` (UNSPECIFIED, still
    building), open the stream.
  - `ImageJoinStreamingRequest` (@2454): `image_id=1`, `timeout=2` (~55.0),
    `last_entry_id=3` (advance across reconnects), `include_logs_for_finished=4` (false).
  - `ImageJoinStreamingResponse` (@2461): `result=1` (`GenericResult`),
    `task_logs=2` (drain), `entry_id=3` (→ last_entry_id), `eof=4`, `metadata=5`.
  - Build done when a streamed item carries `result` with `status != 0`; re-open until a
    result arrives. Terminal via `GenericResult.status`: `SUCCESS=1` ok; FAILURE=2/
    TERMINATED=3/TIMEOUT=4/INIT_FAILURE=5/INTERNAL_FAILURE=6 → `Error::Build` (surface
    `result.exception`).
  - Output: `image_id` (from the initial response).

### 5.4 FunctionPrecreate

- `FunctionPrecreate` (@4250):
  - `FunctionPrecreateRequest` (@2218): set `app_id=1`, `function_name=2` (`"handler"`),
    `function_type=4` = `FUNCTION_TYPE_FUNCTION=2`,
    `supported_input_formats=10` = `[DATA_FORMAT_PICKLE=1, DATA_FORMAT_CBOR=4]`,
    `supported_output_formats=11` = `[PICKLE, CBOR]`. (Class-method fields 6/7/8 N/A.)
  - `FunctionPrecreateResponse` (@2233): `function_id=1` ← **carry into
    `FunctionCreate.existing_function_id`** (this is what makes empty `function_serialized`
    legal: sets `allow_sparse_base=true`, bypassing the empty-serialized guard);
    `handle_metadata=2`.

### 5.5 FunctionCreate (FILE mode) — FIX #1

- `FunctionCreate` (@4240):
  - `FunctionCreateRequest` (@1911): `function=1`, `app_id=2`, `existing_function_id=7`,
    `function_data=9`. **FIX #1: send EXACTLY ONE of `function` / `function_data` (XOR).**
    Use the single-Function path: set `function`, set `existing_function_id` = precreate id,
    leave `function_data` UNSET. (`function_data` is GPU-fallback ranked lists only — defer.)
    modal-rs's bug was sending BOTH → server "Internal error / contact support".
  - `FunctionCreateResponse` (@1920): `function_id=1`, `function=4` (echoed),
    `handle_metadata=5` (read `definition_id` for AppPublish), `server_warnings=6`.
- `Function` fields to set (FILE mode) — field numbers VERIFIED @1646-1810:
  - `module_name=1` = wrapper module (e.g. `"spike_wrapper"`); `function_name=2` = `"handler"`.
  - `mount_ids=3` = `[client_mount_id]` (§5.2) (+ wrapper mount id if mounted vs baked).
  - `image_id=4` = built image id (§5.3).
  - `function_serialized=6` = **EMPTY bytes `b""`** (FILE mode; Python `function_serialized or b""`).
  - `definition_type=7` = `DEFINITION_TYPE_FILE=2`.
  - `function_type=8` = `FUNCTION_TYPE_FUNCTION=2`.
  - `resources=9` (`Resources`) = **FIX #1: ALWAYS set** (omitting it contributed to the
    server error). CPU default: `Resources{}` (zeros OK) or modest `milli_cpu`/`memory_mb`.
  - `timeout_secs=21` = e.g. `300`.
  - `volume_mounts=33` (repeated `VolumeMount` @3944: `volume_id=1`, `mount_path=2`,
    `allow_background_commits=3`, `read_only=4`, `sub_path=5`) — empty for the basic recipe
    (cache-volume attach point later).
  - `supported_input_formats=87` = `[PICKLE=1, CBOR=4]`; `supported_output_formats=88` =
    `[PICKLE, CBOR]`. Advertising CBOR is what lets us force CBOR end-to-end.
  - `app_name=31` optional; leave class/web/schedule/autoscaler fields unset.
- `Resources` (@2987): `memory_mb=2`, `milli_cpu=3`, `gpu_config=4` (`GPUConfig`),
  `memory_mb_max=5`, `ephemeral_disk_mb=6`, `milli_cpu_max=7`, `rdma=8`.
- GPU path (later milestone): `Resources.gpu_config=4` → `GPUConfig` (@2327):
  `type=1` (legacy `GPUType` — leave UNSPECIFIED), `count=2`, `gpu_type=4` (string — the
  field current clients use, e.g. `"A100"`, upper-cased per `parse_gpu_config`).

### 5.6 AppPublish (deploy) — FIX #2

- `AppPublish` (@4147). **FIX #2: use AppPublish ONLY.** The legacy `AppSetObjects` handler
  is server-broken (`module 'grpc' has no attribute 'experimental'`). Mirrors Python
  `runner._publish_app`. The spike confirmed: skip `AppSetObjects`, AppPublish alone deploys.
  - `AppPublishRequest` (@552): `app_id=1`, `name=2` (app name), `deployment_tag=3`,
    `app_state=4` = `APP_STATE_DEPLOYED=3`, `function_ids=5` (map name→function_id, e.g.
    `{"handler": fu-…}`), `class_ids=6` (N/A), `definition_ids=7` (map
    **function_id→definition_id** from `FunctionCreateResponse.handle_metadata.definition_id`),
    `commit_info=10`, `tags=11`.
  - `AppPublishResponse` (@566): `url=1`, `server_warnings=3`, `deployed_at=4`.

### 5.7 FunctionGet (from_name)

- `FunctionGet` (@4242):
  - `FunctionGetRequest` (@2114): `app_name=1`, `object_tag=2` (= function name, `"handler"`),
    `environment_name=4`, `app_version=5`.
  - `FunctionGetResponse` (@2122): `function_id=1` ← the handle id for invoke;
    `handle_metadata=2` (read `supported_input_formats=50`/`supported_output_formats=51` to
    confirm CBOR is advertised; `max_object_size_bytes=48` for the inline-vs-blob threshold);
    `server_warnings=4`.

### 5.8 Invoke — FunctionMap → (FIX #3) FunctionPutInputs fallback → poll FunctionGetOutputs

Sequencing from `_functions.py:156-215` + `_create_input` (`function_utils.py:577-625`).

**Encode the input** (codec, §6):
- Payload = the **tuple `(args, kwargs)`** — args a positional tuple, kwargs a dict. Spike
  invoked `(payload,)`, kwargs `{}` → encode `((payload,), {})`.
- `data_format = DATA_FORMAT_CBOR=4` (we authorized it). `args_serialized = cbor((args, kwargs))`.
- Build `FunctionPutInputsItem` (@2238): `idx=1` (0), `input=2` (`FunctionInput`).
- `FunctionInput` (@2163): oneof `args_oneof { args=1 (bytes inline) | args_blob_id=7 }`,
  `final_input=9`, `data_format=10` (= CBOR=4, for args_oneof), `method_name=11` (empty).
  Inline `args` when small; if `len(args_serialized) > max_object_size_bytes`, blob-upload
  and set `args_blob_id` (defer blob upload; inline is fine for the recipe).

**Step 1 — FunctionMap** (@4249):
- `FunctionMapRequest` (@2174): `function_id=1` (from FunctionGet), `parent_input_id=2` (""),
  `return_exceptions=3`, `function_call_type=4` = `FUNCTION_CALL_TYPE_UNARY=1`,
  `pipelined_inputs=5` = `[item]`,
  `function_call_invocation_type=6` = `FUNCTION_CALL_INVOCATION_TYPE_SYNC=4`,
  `from_spawn_map=7` (false).
- `FunctionMapResponse` (@2184): `function_call_id=1`, `pipelined_inputs=2`
  (repeated `FunctionPutInputsResponseItem`), `retry_policy=3`, `function_call_jwt=4`, …

**Step 2 — FIX #3 fallback**: if `FunctionMapResponse.pipelined_inputs` is **non-empty**,
the input was accepted. If **EMPTY**, the input was NOT enqueued → call `FunctionPutInputs`
(@4251) to actually enqueue (the bug modal-rs missed → "Function call not found"):
- `FunctionPutInputsRequest` (@2246): `function_id=1`, `function_call_id=3` (from Map),
  `inputs=4` = `[item]`.
- `FunctionPutInputsResponse` (@2252): `inputs=1` — if empty → input queue full (error).
- Do NOT call `FunctionFinishInputs` (Python doesn't here).

**Step 3 — poll FunctionGetOutputs** (@4247):
- `FunctionGetOutputsRequest` (@2094): `function_call_id=1`, `max_values=2` (1),
  `timeout=3` (~55), `last_entry_id=6` (advance), `clear_on_success=7` (true once done),
  `requested_at=8`, `input_jwts=9`. Loop, advancing `last_entry_id`, until output arrives.
- `FunctionGetOutputsResponse` (@2107): `idxs=3`, `outputs=4` (repeated
  `FunctionGetOutputsItem`), `last_entry_id=5`, `num_unfinished_inputs=6` (>0 → keep polling).
- `FunctionGetOutputsItem` (@2082): `result=1` (`GenericResult`), `idx=2`, `input_id=3`,
  `data_format=5` (**for result.data_oneof** — read to pick decoder, will be CBOR=4),
  `task_id=6`, timing fields, `retry_count=9`.
- `GenericResult` (@2343): `status=1` (`GENERIC_STATUS_SUCCESS=1` → decode; FAILURE=2/
  TIMEOUT=4/etc → error, surface `exception=2`/`traceback=4`),
  oneof `data_oneof { data=5 (bytes inline) | data_blob_id=10 (blob fetch for large) }`.
- **Decode**: read `data` bytes (or fetch blob via `data_blob_id` — defer blob fetch),
  decode per `FunctionGetOutputsItem.data_format` (CBOR=4 → `codec::decode`). Decoded value
  is the function's return (spike got `{"echoed":…,"ok":true,"source":"spike_wrapper.handler"}`).

---

## 6. CBOR codec (codec.rs)

Verbatim from modal-rs `cbor.rs` (`serde_cbor` thin wrappers), applied to the TUPLE
`(args, kwargs)`:

```rust
use serde::{de::DeserializeOwned, Serialize};

pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, crate::Error> {
    serde_cbor::to_vec(value).map_err(|e| crate::Error::Codec(format!("cbor encode: {e}")))
}
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, crate::Error> {
    serde_cbor::from_slice(bytes).map_err(|e| crate::Error::Codec(format!("cbor decode: {e}")))
}
```

- The encoded object is the 2-tuple `(args, kwargs)` (`args` a positional tuple/array,
  `kwargs` a map). For the spike call: `((payload,), {})`.
- **DataFormat dispatch**: encode args with `DATA_FORMAT_CBOR=4` (set `FunctionInput.data_format`).
  On output, branch on `FunctionGetOutputsItem.data_format`: CBOR=4 → `decode`; PICKLE=1 →
  PICKLE is a passthrough byte format (return raw bytes / defer pickle decode — not needed
  when we advertise and request CBOR end-to-end).
- Round-trip test (port modal-rs's): `((vec![1,2,3], empty-map))` encode→decode equality.

---

## 7. Workspace manifest edit (`/Users/nicolas/devel/modal-rust/Cargo.toml`)

Add `"crates/modal-rust-sdk"` to BOTH `members` (lines 3-11) and `default-members`
(lines 20-27). It is pure-Rust (no CUDA) so it MUST be in default-members (the gate set).
Place it first for tidy ordering:

```toml
members = [
    "crates/modal-rust-sdk",
    "crates/modal-rust-runtime",
    "crates/modal-rust-cli",
    "crates/modal-rust-macros",
    "examples/add",
    "examples/add-macro",
    "examples/cuda-vector-add",
    "examples/burn-add",
]
default-members = [
    "crates/modal-rust-sdk",
    "crates/modal-rust-runtime",
    "crates/modal-rust-cli",
    "crates/modal-rust-macros",
    "examples/add",
    "examples/add-macro",
    "examples/cuda-vector-add",
]
```

`[profile.release/dev] panic = "unwind"` (lines 33-37) is workspace-wide and already
covers the new crate — no profile edit needed (unwind is harmless-to-beneficial for the SDK).
Do NOT add the SDK to `burn-add`'s position; it stays out of `default-members` (CUDA-only).

---

## 8. The 3 fixes (baked in — these are why modal-rs failed)

1. **FunctionCreate XOR + always resources** (§5.5): send EXACTLY ONE of `function` /
   `function_data`, and ALWAYS set `resources`. modal-rs sent both → server internal error.
2. **Deploy via AppPublish ONLY** (§5.6): never `AppSetObjects` (server-broken
   `grpc.experimental`). Spike-confirmed.
3. **Invoke fallback** (§5.8): after `FunctionMap`, if `pipelined_inputs` is EMPTY, fall
   back to `FunctionPutInputs` to actually enqueue, then poll `FunctionGetOutputs`. modal-rs
   missed this → "Function call not found".

---

## 9. Attribution (NOTICE + lib.rs comment)

Credit both reference SDKs in `NOTICE` and a short comment in `lib.rs`:
- modal-client (the official Modal Python SDK) — Apache-2.0 / MIT.
- modal-rs (the unofficial Rust SDK) — MIT.
We DEPEND on neither (both are gitignored references, never a Cargo dep, never imported).

---

## 10. Verification (offline = HARD gates; live = best-effort)

Run on **default-members only** (per WORKING.md + CI; NOT `--workspace`/`--all-features`,
which pull the CUDA-only `example-burn-add`):

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo build
cargo test
```

The proto-compile + crate-build are the HARD gates (already proven to compile). Live Modal
round-trips (ClientHello, AppGetOrCreate ephemeral, full FILE-mode recipe) are best-effort
evidence; retry on transient capacity errors ("socket connection closed unexpectedly") —
never mark "blocked on Modal".

---

## 11. Resolved contradictions (summary)

| Topic | Notes' disagreement | Resolution (this spec) |
|---|---|---|
| Header set | auth note = full Python set; proto/layout notes = modal-rs 3-header subset | **Full Python set** (superset); the 3-header is the precedent to extend. `x-modal-client-type="1"` is the one that matters. |
| App RPC | AppGetOrCreate vs AppCreate | **AppGetOrCreate default** (idempotent + auth proof); AppCreate = ephemeral alternate. Spike used get-or-create then published Deployed. |
| ClientHello | auth note adopts it; proto note says "not required" | **Adopt** as connect-time fail-fast probe (cheap, Python parity). Not a login RPC — auth is by headers. |
| `modal` importable | spike `pip install modal`; notes' client mount | **Client mount (MountGetOrCreate, GLOBAL) is default**; pip install is documented fallback only. |
| CLIENT_VERSION | modal-rs "1.3.1" | **"1.3.2"** (Modal 1.3.2 facts in knowledge.md). Keep client-mount `{version}` aligned. |
| tonic features | auth note keeps defaults; layout note trims | **Trim** (`default-features = false` + client features) to drop axum. Compile-proven. |
| Protos to compile | modal-rs compiles api.proto + task_command_router.proto | **api.proto ONLY** (router not imported, sandbox-only). |
