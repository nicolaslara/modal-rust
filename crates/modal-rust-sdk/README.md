# modal-rust-sdk

A third-party Rust client for Modal's control plane, used by the
[`modal-rust` facade](../modal-rust/README.md). It connects directly to the
`modal.client.ModalClient` gRPC service. Local client operations do not require
the `modal` Python CLI or the separate `modal-rs` crate.

This README describes the current source checkout. The crate is pre-release;
see [`Cargo.toml`](Cargo.toml) for its version and
[the roadmap](../../docs/ROADMAP.md) for open work.

## Role in the Rust execution path

This crate owns authentication, transport, codecs, and control-plane operations.
The facade adds typed function calls, macros, source packaging, and the
run/deploy workflow. Its `client` feature enables this SDK; authoring functions
and calling `.local()` do not require it.

The remote execution path still uses Python inside Modal. Functions are created
in FILE mode, with an importable Python module and callable rather than pickled
function bytecode. The facade's adapters forward JSON requests to a persistent
Rust `modal_runner --serve` process and return its JSON result envelopes.
Users do not need to write those adapters or install the Python CLI locally.

For `run`, the Rust build happens inside the function container, with cached
binaries/build artifacts available for reuse. For `deploy`, the binary is built
during image creation and the deployed adapter executes the prebuilt runner.
See the [run adapter](../modal-rust/src/remote/wrapper.py) and
[deploy adapter](../modal-rust/src/deploy/wrapper.py).

## Implemented surface

The table describes the typed operations in this crate. A vendored protocol
definition or generated method alone does not imply a supported high-level API.

| Area | Implemented | Boundary |
| --- | --- | --- |
| Connection | Token credentials from the environment or `~/.modal.toml`, explicit credentials, `ClientHello`, HTTPS, and worker Unix-socket connections inside containers | No OAuth integration or refresh-token manager |
| Apps | `AppGetOrCreate`, `AppCreate`, and `AppPublish`; ephemeral run apps and persistent deployments | No stop, list, logs, rollback, heartbeat, or tag API |
| Environments | Resolve the configured environment and its image-builder version through `EnvironmentGetOrCreate` | No environment administration API |
| Images | `ImageGetOrCreate` and `ImageJoinStreaming`; registry bases, Python provisioning, wrapper modules, build contexts, apt/pip installation, shell commands, and Rust toolchain installation | A subset of Modal's image-building API; no image lookup-by-ID or deletion wrapper |
| Function definitions | `FunctionPrecreate`, `FunctionCreate`, and `FunctionGet`; resource, timeout, retry-policy, schedule, autoscaler, checkpoint, webhook, and input-concurrency fields | Configuration support does not imply full runtime semantics; see the limits below |
| Invocation | Synchronous calls, spawn, ordered map collection, spawn-map, and result polling through `FunctionMap`, `FunctionPutInputs`, and `FunctionGetOutputs` | Control-plane path only; no input-plane attempt routing or remote cancellation |
| Mounts | Hosted client/Python mounts and local source uploads through `MountGetOrCreate` and `MountPutFile`; Cargo dependency-closure uploads and ignore rules | Large mount files use single-part blob upload |
| Volumes | Named resolution, function mounts, and local file/directory uploads using `VolumePutFiles2` with 8 MiB blocks | No download, listing, commit/reload, rename, copy, or deletion wrapper |
| Secrets | Named lookup with required keys and creation from an environment map through `SecretGetOrCreate` | No update, delete, or list wrapper |
| Dicts | Named lookup/create, get, put, put-if-absent, pop, contains, length, clear, and delete | No iteration, ephemeral lifecycle, or high-level batch update |
| Queues | Named lookup/create, put/put-many, get/get-many, blocking polling, length, clear, and delete | Default partition only; no exposed partition or time-to-live settings, iteration, ephemeral lifecycle, or block-on-full put retry |
| Blobs | `BlobCreate` plus single-part HTTP upload for large mount files | Multipart upload is rejected; blob download and function-payload offloading are not implemented |
| Offline planning | Image and function request projections used by the facade's dry-run/dump tools | Request inspection does not execute or validate a deployment |

Implementation references: [client](src/client.rs),
[images](src/ops/image/render.rs), [function definitions](src/ops/function/spec.rs),
[invocation](src/ops/invoke.rs), [mount uploads](src/ops/local_dir.rs),
[Volumes](src/ops/volume.rs), [Secrets](src/ops/secret.rs),
[Dicts](src/ops/dict.rs), [Queues](src/ops/queue.rs), and
[planning](src/ops/planning.rs).

The facade exposes `#[cls]` through a Rust singleton and separate function
entrypoints for its methods. This does not implement native parameterized Modal
class binding. Its `#[endpoint]` and `#[web_server]` features use the function
webhook configuration on deployment. Schedules and static autoscaler settings
also travel through function creation; live autoscaler updates are not wrapped.

## Serialization

Function arguments use Concise Binary Object Representation (CBOR) to encode
`(args, kwargs)`. The low-level `invoke_cbor` API accepts these values directly.
The facade uses that transport to carry `(entrypoint, input_json)` and receives
a JSON envelope as a CBOR string.

Raw function results retain their format and bytes. Typed CBOR decoding rejects
other formats; function results in Python's pickle format are not automatically
unpickled.

Dict and Queue operations carry opaque bytes at the SDK operation layer. The
facade uses the [restricted pickle codec](src/pickle.rs) for Python-compatible
plain data: strings, numbers, booleans, bytes, lists, dictionaries, and Rust
structs represented as dictionaries. Dict string keys use Python-compatible
protocol-4 bytes. Arbitrary pickled Python classes and functions are unsupported.

## Known invocation limits

These are open issues, not completed guarantees. The
[roadmap's invocation section](../../docs/ROADMAP.md#open-invocation-correctness-go-sdk-comparison-2026-10-21)
tracks the acceptance criteria.

1. **Calls through one facade `App` serialize.** Its client mutex stays locked
   throughout invocation and output polling. A pending `get()` can block another
   spawn. One `.map()` still submits multiple inputs for remote parallel
   execution. The low-level `ModalClient` is cloneable; the lock is in the facade.
2. **Rust handler errors do not reach Modal as failed calls.** The Python adapters
   return `{"ok":false,...}` as normal response data. The facade raises a Rust
   error after receiving that successful transport result. Configuring
   `#[function(retries = N)]` alone therefore does not cause Modal to retry
   ordinary Rust handler failures. Wrapper exceptions and infrastructure failures
   are separate cases.
3. **Function payloads are inline-only.** Arguments are not uploaded to blobs,
   and blob-backed outputs are rejected. Mount and Volume upload support does
   not provide large-payload support for function calls.
4. **Retries lack complete safety and cancellation semantics.** Transport retries
   do not retain an idempotency key, so a lost enqueue response can lead to
   duplicate work. There is no public remote call cancellation API. Deadline
   checks happen between awaited operations, so they do not enforce a hard cap
   on an in-flight request.

The unary retry helper permits up to eight attempts with exponential backoff and
jitter. It retries errors classified as transient and returns other errors
immediately. This transport retry loop does not implement user-function retry
semantics. See [retry policy](src/retry.rs) and
[enqueue handling](src/ops/transport.rs).

Map calls currently materialize all inputs and send them in one enqueue request.
Partial acceptance returns an error. This is not a streaming map implementation
with bounded buffering and backpressure.

## Remaining platform gaps

The typed surface does not yet cover Sandboxes, native parameterized class
binding, network file systems, cloud-bucket mounts, proxies, tunnels, or clustered
functions. General function/app logs, metrics, statistics, call graphs, and live
autoscaler updates are also absent.

`ModalClient::inner_mut()` exposes the generated stub for operations outside the
typed surface. Callers using it own their request construction, retry behavior,
and compatibility with the server.

## Protocol provenance and verification

The client uses `tonic`/`prost`, `reqwest` with Rustls, Serde, CBOR, and the
restricted pickle codec. The vendored protocol comes from Modal's official
client repository; [`proto/SOURCE.toml`](proto/SOURCE.toml) records its revision
and update procedure. See [`NOTICE`](NOTICE) for attribution, including the
`modal-rs` transport and codec references.

Offline tests cover request construction, codecs, and operations against the
in-process mock backend. Live tests are opt-in. A passing mock test establishes
the tested client behavior, not full compatibility with production Modal.
