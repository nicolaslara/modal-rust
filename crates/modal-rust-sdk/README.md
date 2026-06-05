# modal-rust-sdk

A lean, **first-party** Rust gRPC client for Modal's control plane. This crate
talks to Modal directly over its `modal.client.ModalClient` gRPC service — there
is **no dependency on the `modal` Python CLI, on `modal-rs`, or on any
per-project Python**. It is the durable transport foundation under the
higher-level `modal-rust` facade (decorator, `.local()` / `.remote()`, deploy +
`from_name` call).

## What's in the box

- **Lean dependency set, no SDK deps.** `tonic` (client-only; `axum`/server
  features dropped) + `prost`, `serde` + `serde_cbor`, `toml`, `reqwest`
  (rustls only — no system OpenSSL), `sha2`, `walkdir`, `ignore`. The vendored
  proto (`proto/api.proto`) is copied verbatim from the official Modal Python
  SDK; the auth/channel/codec *structure* follows the unofficial `modal-rs`, but
  **neither is a build- or run-time dependency** (both are read-only references;
  see `NOTICE`).
- **Auth + channel.** Credentials/endpoint resolve from the environment or
  `~/.modal.toml` (`config.rs`); an `AuthInterceptor` attaches the `x-modal-*`
  headers (`token-id`/`token-secret`/`client-type=1`/`client-version`/`platform`)
  to every RPC over a hardened TLS channel (`tls-native-roots`, ring). `connect()`
  performs a `ClientHello` handshake to fail fast on bad credentials.
- **CBOR codec.** Function payloads are the 2-tuple `(args, kwargs)`
  CBOR-encoded with `DATA_FORMAT_CBOR` end-to-end (`codec.rs`) — Modal's
  non-Python wire format. `PICKLE` outputs are surfaced as opaque bytes (no
  unpickling).
- **`retry_transient` / `retry_unary`.** Every control-plane unary call is
  wrapped in a transient-only retry (`retry.rs`): 8 attempts, exponential
  backoff with full jitter, bounded by a total deadline. Only errors classified
  `Error::is_transient` (e.g. UNAVAILABLE / connection reset) retry; auth,
  invalid-argument, and in-band build failures surface immediately. The blob
  object-store `PUT` has its own matching backoff.
- **FILE-mode recipe.** The `ops/*` surface is a native Rust port of the proven
  FILE-mode flow: a precreated function is identified purely by
  `module_name` + `function_name` (empty `function_serialized`, `allow_sparse_base`),
  the crate source is uploaded as an ephemeral Mount, and the wrapper boots via
  `python -m modal._container_entrypoint`. **SERIALIZED-mode (pickled bytecode)
  is not used.**
- **The three spike fixes** (why `modal-rs`'s flow failed; baked into `ops/*`):
  1. `FunctionCreate` sends **exactly one** of `function` / `function_data`
     (XOR) and **always** sets `resources` (`ops/function.rs`).
  2. Deploy uses `AppPublish` **only** — never the server-broken `AppSetObjects`
     (`ops/app.rs`).
  3. Invoke falls back to `FunctionPutInputs` when `FunctionMap` does not
     pipeline the input, then polls `FunctionGetOutputs` (`ops/invoke.rs`).

## Coverage matrix (control plane)

Scope: this crate currently calls **18 distinct RPCs** out of the ~200 in the
vendored `proto/api.proto`. The matrix below is grouped by object/area. Status:
**Implemented** = we call this RPC in `ops/*` or `client.rs`; **Partial** = the
RPC is called but only a subset of its semantics/fields is exercised;
**Not yet** = the object family is otherwise covered but this capability is
absent. Every "Implemented" row was verified against the actual call sites.

### App
| Capability | Status | RPC(s) | Note |
|---|---|---|---|
| Get-or-create (idempotent) | Implemented | `AppGetOrCreate` | Run + deploy entry; also the cheap live auth probe. |
| Create ephemeral app | Implemented | `AppCreate` | One-shot throwaway apps (GC'd on disconnect). |
| Publish (make functions invokable / deploy) | Implemented | `AppPublish` | Fix #2; `Ephemeral` (run) vs `Deployed` (deploy) state. |
| Stop / list / logs / rollback / heartbeat / tags / layout | Not yet | `AppStop`, `AppList`, `AppGetLogs`, `AppRollback`, `AppHeartbeat`, `AppSetTags`, `AppGetLayout`, … | App lifecycle/observability beyond create+publish not wired. |

### Environment
| Capability | Status | RPC(s) | Note |
|---|---|---|---|
| Resolve environment + image-builder version | Implemented | `EnvironmentGetOrCreate` | Idempotent lookup; reads `image_builder_version` from settings. |
| Create / update / delete / list / roles | Not yet | `EnvironmentCreate`, `EnvironmentUpdate`, `EnvironmentDelete`, `EnvironmentList`, … | Environment management is out of scope. |

### Image
| Capability | Status | RPC(s) | Note |
|---|---|---|---|
| Build image (`from_registry` + add_python + wrapper bake) | Implemented | `ImageGetOrCreate` | Renders `dockerfile_commands`; `add_python` via a python-standalone context mount, matching the official client. |
| Poll build to completion | Implemented | `ImageJoinStreaming` | Streams build status until terminal. |
| Delete / from-id | Not yet | `ImageDelete`, `ImageFromId` | Not wired. Image-builder breadth (full `Image` DSL) is out of scope. |

### Function — authoring
| Capability | Status | RPC(s) | Note |
|---|---|---|---|
| Precreate (reserve id, sparse base) | Implemented | `FunctionPrecreate` | Sets `allow_sparse_base`, making empty `function_serialized` legal (FILE mode). |
| Create (FILE mode) | Implemented | `FunctionCreate` | Fix #1 (XOR `function`/`function_data`, always `resources`); GPU/timeout/secret_ids/volume_mounts/mount_ids wired. |
| Resolve deployed function (`from_name`) | Implemented | `FunctionGet` | App name + object tag → invokable `function_id`. |
| Get serialized / bind params / call graph / scheduling | Not yet | `FunctionGetSerialized`, `FunctionBindParams`, `FunctionGetCallGraph`, `FunctionUpdateSchedulingParams`, … | Not wired. |

### Function — invocation
| Capability | Status | RPC(s) | Note |
|---|---|---|---|
| `.remote()` (sync call + wait) | Implemented | `FunctionMap` → `FunctionPutInputs` → `FunctionGetOutputs` | Fix #3 enqueue fallback; CBOR `(args, kwargs)`; deadline-bounded output long-poll (`0-0` cursor). |
| `.map()` (fan-out over many inputs) | Implemented | `FunctionMap` / `FunctionPutInputs` / `FunctionGetOutputs` | Same unary path, multiple inputs. |
| `.spawn()` (fire-and-forget) | Implemented | `FunctionMap` (ASYNC) (+ `FunctionPutInputs` fallback) | Returns `function_call_id`; result fetched later. |
| `FunctionCall::get` (poll a spawned call) | Implemented | `FunctionGetOutputs` | Polls by `function_call_id`. |
| Cancel / retry / async-invoke / current-stats / dynamic-concurrency | Not yet | `FunctionCallCancel`, `FunctionRetryInputs`, `FunctionAsyncInvoke`, `FunctionGetCurrentStats`, `FunctionGetDynamicConcurrency` | Invocation lifecycle/observability beyond call+poll not wired. |

### Mount
| Capability | Status | RPC(s) | Note |
|---|---|---|---|
| Resolve hosted client mount (GLOBAL) | Implemented | `MountGetOrCreate` | `modal-client-mount-{version}` + python-standalone mounts; pure GLOBAL lookup. |
| Upload local dir → ephemeral mount | Implemented | `MountGetOrCreate` + `MountPutFile` | Walks the cargo dep closure, hashes files, pushes inline (`MountPutFile.data`). `.modalignore` > `.gitignore` > defaults. |
| Large-file blob branch | Implemented | `BlobCreate` + object-store HTTPS `PUT` | Files ≥ 4 MiB go via presigned URL → `MountPutFile.data_blob_id`. Single-part only (multipart rejected). |

### Volume
| Capability | Status | RPC(s) | Note |
|---|---|---|---|
| Get-or-create (V1/V2) + attach as mount | Implemented | `VolumeGetOrCreate` | Powers the cargo build cache (V2, background commits) and user `volumes=["/m=name"]`; mount attached via `Function.volume_mounts`. |
| Commit / reload / list / get-file / put-files / copy / rename / delete | Not yet | `VolumeCommit`, `VolumeReload`, `VolumeListFiles`, `VolumeGetFile`, `VolumePutFiles`, `VolumeCopyFiles`, `VolumeRename`, `VolumeDelete`, … | Direct file I/O against a volume from the client is not wired (the container commits the archive itself). |

### Secret
| Capability | Status | RPC(s) | Note |
|---|---|---|---|
| `from_name` lookup (+ `required_keys` assert) | Implemented | `SecretGetOrCreate` | Pure lookup; powers `#[function(secrets=[…])]`. |
| `from_dict` create (idempotent) | Implemented | `SecretGetOrCreate` | `CREATE_IF_MISSING` + `env_dict`; values never logged. |
| Update / delete / list | Not yet | `SecretUpdate`, `SecretDelete`, `SecretList` | Not wired. |

### Blob
| Capability | Status | RPC(s) | Note |
|---|---|---|---|
| Single-part upload | Implemented | `BlobCreate` (+ HTTPS `PUT`) | See Mount large-file branch above. |
| Multipart upload | Not yet | `BlobCreate` (multipart plan) | Detected and rejected — single-part only. |
| Download | Not yet | `BlobGet` | Not wired. |

### Connect / handshake
| Capability | Status | RPC(s) | Note |
|---|---|---|---|
| Client handshake / auth probe | Implemented | `ClientHello` | Run on `connect()`; bad credentials surface here. |

## Not covered (honest surface boundary)

The crate is intentionally scoped to the run / deploy / call path. The following
proto areas are **not implemented** today (the object families exist in the
vendored proto but we issue none of their RPCs):

- **Sandbox** (the entire `Sandbox*` family — ~27 RPCs: create, exec, snapshot,
  fs, tunnels, terminate, …).
- **Dict** (`Dict*` — distributed dict).
- **Queue** (`Queue*` — distributed queue).
- **Cls / Class** (`ClassCreate` / `ClassGet` — Modal classes / parameterized
  `Cls`).
- **Web endpoints** (`EndpointCreate`/`List`/`Stop`, `TunnelStart`/`Stop`,
  `DomainCreate`, web/ASGI/WSGI endpoints — no web-serving functions).
- **Schedules / cron** — no schedule wiring (and no
  `FunctionUpdateSchedulingParams`).
- **NFS / shared volumes** (`SharedVolume*`) and **CloudBucket** mounts.
- **Proxy** (`Proxy*`), **Cluster** (`Cluster*`), **Flash** (`Flash*`),
  container exec/filesystem RPCs (`Container*`), and the
  **autoscaler / scaling / current-stats** RPCs
  (`FunctionGetCurrentStats`, `FunctionGetDynamicConcurrency`,
  `FunctionUpdateSchedulingParams`, …).
- **App lifecycle/observability** beyond create + publish (stop, logs, list,
  rollback, history, heartbeat, tags).
- **Volume / Secret / Environment management** RPCs (file I/O, update, delete,
  list) beyond the get-or-create paths above.
- **Token-flow auth** (`TokenFlow*`), workspace/billing RPCs, and the
  **map/attempt** server-driven invocation RPCs
  (`MapStartOrContinue`, `MapAwait`, `AttemptStart`/`Await`/`Retry`) — our
  invocation uses the `FunctionMap`/`PutInputs`/`GetOutputs` path instead.
- **SERIALIZED mode** (pickled function bytecode) — FILE mode only.
- **Image-builder breadth** — only `from_registry` + `add_python` + the wrapper
  bake are rendered; the full `Image` DSL (apt/pip/run/copy chains as a
  first-class API) is not exposed.

## Stability

This crate is **pre-1.0 (`0.1.0`) and internal-leaning** — its public surface is
shaped by what the `modal-rust` facade needs, and the `ModalClient::inner_mut()`
escape hatch exists precisely so callers can issue any other control-plane RPC
the typed surface does not yet wrap. It is, however, **designed to grow toward
fuller Modal client-SDK compatibility**: the auth/channel/codec/retry foundation
and the vendored full proto are already in place, so adding a new object family
(Dict, Queue, Sandbox, …) is a matter of adding an `ops/*` module — not
re-plumbing the transport.
