# modal-rust ↔ Modal Python parity

An honest, code-verified inventory of what `modal-rust` (facade + `modal-rust-sdk` + `#[modal_rust::function]` macro + `modal-rust` CLI) covers versus the official
Modal Python client, so the maintainer can prioritise. Status legend:

- ✅ **Have** — implemented end-to-end and live-proven.
- 🟡 **Partial** — works in a narrower form than Modal, or the plumbing exists in the
  SDK but is not exposed through the facade/macro.
- ❌ **Missing** — not implemented.

Modal references are file:line into `references/modal-client/py/modal/` (the
official client; `_*.py` files hold the real implementations — the public
`image.py`/`functions.py` etc. are thin `synchronize_api` shims). Everything below
was checked against the actual Rust source and the Modal source on
2026-06-10; no Modal feature is asserted that is not in that tree.

---

## 0. The one structural difference that frames everything: FILE mode only

Modal supports two function definition modes:

- **SERIALIZED** (`serialized=True`, the default for `app.function` on a callable):
  the Python callable is cloudpickled (`_functions.py`, `function_serialized`) and
  the container is a generic Python worker that unpickles and calls it. This is what
  makes "write a plain Python function, no Dockerfile thinking" work.
- **FILE** mode: the function is identified by `module_name` + `function_name` and
  resolved in-container via `importlib.import_module(...)` + `getattr(...)`.

`modal-rust` is **FILE-mode only** (`crates/modal-rust-sdk/src/ops/function.rs`:
`DefinitionType::File`, empty `function_serialized`, `existing_function_id` =
precreate id to legalise the empty serialized blob). We bake a tiny Python wrapper
module into the image whose `handler` shells out to a `modal_runner` Rust binary
(built in-body on the RUN path, baked at image-build time on the DEPLOY path) that
dispatches to the registered Rust handler over a JSON envelope. There is **no Rust
equivalent of cloudpickle**, and Modal's container entrypoint
(`_container_entrypoint.py`) is Python-callable-shaped, so a "serialized Rust
closure" mode is not on the table. This is a deliberate, foundational choice, not a
gap to close — but it explains why several Modal surfaces (generators, and the deeper
`Cls` lifecycle bits like `@exit`) do not map cleanly. The core `Cls`
load-once-serve-many pattern still landed in FILE mode (a per-method dotted
entrypoint + a warm-container `--serve` loop — see §7), and so did single-function
**web endpoints** (a per-endpoint FastAPI-shaped adapter in the baked wrapper, fed by
the same `--serve` loop — see §8).

---

## 1. What IS at parity (the proven core)

A short summary of the surface that is genuinely done:

- **Authoring a function**: an ordinary Rust `fn(In) -> anyhow::Result<Out>`
  registered via the manual `Registry` builder OR `#[modal_rust::function]`. The
  macro mirrors Modal's `@app.function` decorator ergonomically.
- **Invocation**: `.local()` (in-process, mirrors `Function.local`), `.remote()`
  (mirrors `Function.remote`), `.spawn()` → `FunctionCall` + `FunctionCall::get()`
  (mirrors `Function.spawn` / `FunctionCall.get`), and `.map()` (ordered fan-out,
  mirrors `Function.map`). `crates/modal-rust/src/function.rs`.
- **Two build paths**: RUN (ephemeral app, build in the function body) and DEPLOY +
  `call` (persistent `AppPublish`, build at image-build time, `from_name` lookup).
  Mirrors `modal run` vs `modal deploy` + `Function.from_name`.
- **GPU**: full `parse_gpu_config` parity (`"T4"`, `"H100:4"`, `"A100-80GB"`),
  live-proven with a real Burn/CUDA workload. `ops/function.rs`.
- **Images**: `from_registry` + `add_python` (matching the client's
  python-build-standalone `_registry_setup_commands` branch), `run_commands`/apt,
  `pip install modal` fallback, image build context + `COPY`, and a Rust/CUDA
  toolchain layer. Live image builds via `ImageGetOrCreate` +
  `ImageJoinStreaming`. `ops/image.rs`.
- **Secrets (named)**: `#[function(secrets = ["name"])]` → `SecretGetOrCreate`
  by name → `Function.secret_ids`; injected as env vars. `ops/secret.rs`.
- **Volumes (user + cache)**: `#[function(volumes = ["/m=name"])]` →
  `VolumeGetOrCreate` → `Function.volume_mounts`, plus the on-by-default cargo
  build cache as a V2 volume. `ops/volume.rs`, `ops/function.rs`.
- **Config**: `gpu`, `cpu`, `memory`, `timeout`, `retries` (int + `Retries(..)`
  struct), `schedule` (`Cron`/`Period`), `cache`, `secrets`, `required_keys`,
  inline `env={..}` (a `Secret.from_dict` that composes with `secrets`), `volumes`,
  autoscaling (`min`/`max`/`buffer_containers` + `scaledown_window`), and a
  per-function custom `image = Image(..)` on the decorator.
- **Source upload**: cargo-metadata-scoped, `.modalignore` > `.gitignore` >
  defaults, matching `mount.py`/`file_pattern_matcher.py` precedence intent.
- **Transport**: first-party gRPC over our own vendored `api.proto` (auth, retries,
  blob upload, mounts), no `modal` CLI / `modal-rs` / per-project Python.

Everything past this point is a gap.

---

## 2. Secrets

`secret.py` factories: `from_name` (402), `from_dict` (278), `from_dotenv` (341),
`from_local_environ` (315).

| Feature | Status | Note (Modal ref) |
|---|---|---|
| Named secret in decorator (`from_name`) | ✅ | `#[function(secrets=["x"])]` → `secret_get_or_create` → `Function.secret_ids`. `ops/secret.rs:40`. |
| `required_keys` assertion on `from_name` | ✅ | `#[function(secrets=["x"], required_keys=["API_KEY", ..])]` threads the asserted keys into `secret_get_or_create` → `SecretGetOrCreateRequest.required_keys`, so Modal errors if a key is missing. One flat list applied to all named secrets (v0). Modal `secret.py:406`. `ops/secret.rs:40`. |
| Inline `Secret.from_dict({...})` in decorator | ✅ | `#[function(env={"K"="V", ..})]` mirrors Modal's `app.function(env=..)` (`app.py:889` → `Secret.from_dict(env)`): the facade derives a deterministic per-entrypoint secret name and resolves it via `secret_from_dict` (`ops/secret.rs:72`, `CREATE_IF_MISSING`, idempotent), pushing the id into the SAME `Function.secret_ids` list named secrets use — so `env` and `secrets` compose. |
| `Secret.from_dotenv()` / `.env` file | ❌ | `secret.py:341`. No file parsing. |
| `Secret.from_local_environ([...])` | ❌ | `secret.py:315` (forward selected host env vars). |

---

## 3. Images

Our `ImageSpec` (`ops/image.rs`) is a single-builder recipe, not a chainable
`Image` algebra. Modal's `_Image` builder methods (`_image.py`):

| Modal method (`_image.py`) | Status | Note |
|---|---|---|
| `from_registry` (2084) | ✅ | `ImageSpec::from_registry`. |
| `add_python` provisioning | ✅ | python-build-standalone `COPY`/`ln`/`ENV` branch, `_image.py:2041-2059`. `ImageSpec::with_add_python`. |
| `run_commands` (1893) | ✅ | `ImageSpec::with_run_commands([..])` (a general chainable build step, each → `RUN <cmd>`), plus the raw `ImageSpec::with_command` escape hatch. Exposed via `RemoteConfig::image_steps` / `DeployConfig::image_steps` as `ImageStep::run`. |
| `apt_install` (2508) | ✅ | `ImageSpec::with_apt_install([..])` — a general chainable step (`RUN apt-get update && install … && clean`). Exposed via `ImageStep::apt`. (`with_apt` still targets `pre_bake_commands` for the bake-runtime fallback.) |
| `pip_install` (992) | ✅ | `ImageSpec::with_pip_install([..])` — arbitrary Python packages (`RUN python3 -m pip install --no-cache-dir …`). Exposed via `ImageStep::pip`. (`with_pip_install_modal()` still provisions the modal client closure.) |
| context mount + `COPY` (`add_local_dir`/`copy=True`, 771) | ✅ | `with_context_mount` + a `COPY` command, used by the DEPLOY path. |
| Rust/CUDA toolchain layer | ✅ | `with_rust_toolchain` (project-specific, no Modal analogue — that's fine). |
| `dockerfile_commands` (1765) | 🟡 | We emit a `dockerfile_commands` list internally, but there is no public "give me a list of Dockerfile lines" builder method on a user-facing Image type. |
| `from_dockerfile` (2281) | ❌ | Build an image from an existing Dockerfile path. |
| `env` (2677) / `workdir` (2707) / `entrypoint` (1853) / `cmd` (2736) | 🟡 | Achievable only by hand-writing `ENV`/`WORKDIR`/`ENTRYPOINT`/`CMD` via `with_command`; no typed helpers. |
| `pip_install_from_requirements` (1198) / `pip_install_from_pyproject` (1259) / `poetry_install_from_file` (1473) | ❌ | |
| `uv_pip_install` (1342) / `uv_sync` (1582) | ❌ | |
| `micromamba` (1937) / `micromamba_install` (1977) | ❌ | conda/mamba environments. |
| `run_function` (2559) | ❌ | Run a Modal Function as a build step — depends on serialized mode, so structurally hard for us. |
| `from_gcp_artifact_registry` (2159) / `from_aws_ecr` (2220) | ❌ | Private-registry auth via a Secret. |
| `from_scratch` (2408) / `debian_slim` (2443) | 🟡 / ❌ | We default to a registry base; no `debian_slim` convenience or `from_scratch`. |
| `add_local_file`/`add_local_dir`/`add_local_python_source` (735/771/849) | 🟡 | We mount source as the build context for the deploy image; we do not expose Modal's general local-add-with-`copy` semantics (lazy mount vs baked layer) as a user API. |
| `pip_install_private_repos` (1078) | ❌ | |

Net: we have the **registry + add_python + raw-commands + context-COPY + toolchain**
slice that the Rust build path needs, plus general chainable **`apt_install` /
`pip_install` / `run_commands`** image-builder steps (exposed as
`RemoteConfig::image_steps` / `DeployConfig::image_steps` `ImageStep`s; see
`examples/pip-apt-image`) for arbitrary system/runtime deps a Rust binary may
dynamically link. The remaining Python-ecosystem package-management surface
(uv/poetry/micromamba/requirements) is ❌ — and mostly irrelevant to a Rust
workload.

---

## 4. Function config (the decorator)

Modal `app.function` signature: `app.py:778-815`. Our `FunctionConfig`
(`modal-rust/src/registration.rs`, emitted by the macros) carries: `gpu`, `timeout_secs`, `cache`, `milli_cpu`,
`memory_mb`, `retries`, `schedule`, `min_containers`, `max_containers`,
`buffer_containers`, `scaledown_window`, `secrets`, `volumes`. `cache` is
**modal-rust-specific** (cargo build cache toggle), not a Modal concept.

| Modal kwarg (`app.py`) | Status | Note |
|---|---|---|
| `gpu` (786) | ✅ | Full `parse_gpu_config`. |
| `timeout` (799) | ✅ | `timeout = <secs>`. |
| `secrets` (785) | ✅ (named only) | See §2. |
| `volumes` (789) | ✅ (Volume only) | `CloudBucketMount` value variant Missing — see §5. |
| `image` (782) | ✅ (per-function custom image) | `#[function(image = Image(base = "..", install_rust = <bool>, apt = [..], pip = [..], run = [..]))]` mirrors Modal's `app.function(image=..)` — that entrypoint builds on the declared base. `base`/`install_rust` **override** the path-level default; `apt`/`pip`/`run` **prepend** to the path-level `image_steps`. Path-level defaults still exist too (`RemoteConfig::base_image`/`install_rust` or `MODAL_RUST_BASE_IMAGE`/`MODAL_RUST_INSTALL_RUST`; `examples/custom-base`). Remaining gap: the richer Modal `Image` builder chain (`from_registry` layering, `dockerfile_commands`, etc. — see §3), not a fully chainable per-function `Image` algebra. |
| `name` (801) | ✅ | `#[function(name = "...")]`. |
| `cpu` (790) | ✅ | `#[function(cpu = 2.0)]` → `FunctionResources.milli_cpu` (`ops/function/spec.rs:22`) → `Resources` (the macro parses `is_ident("cpu")` and converts the `float` cores to milli-CPU). Remaining gap: Modal's `(request, limit)` tuple form — we take only the scalar request. |
| `memory` (791) | ✅ | `#[function(memory = 4096)]` → `FunctionResources.memory_mb` (the macro parses `is_ident("memory")`; e.g. `examples/burn-add` uses `memory = 8192`). Remaining gap: Modal's `(request, limit)` tuple form — we take only the scalar request. |
| `retries` (798) | ✅ (int + struct form) | `#[function(retries = N)]` → Modal's fixed-interval `FunctionRetryPolicy` (backoff `1.0`, 1s initial / 60s max delay, N retries), mirroring `_parse_retries(int)`. The STRUCT form `#[function(retries = Retries(max_retries = N[, backoff_coefficient = f][, initial_delay = s][, max_delay = s]))]` sets custom backoff/delays (seconds → `initial_delay_ms`/`max_delay_ms`), mirroring `Retries(..)` (`retries.py`). Both ride into `Function.retry_policy`. `ops/function.rs` `with_retries` / `with_retry_policy`. |
| `schedule` (783) | ✅ | `#[function(schedule = Cron("..")/Period(..))]` → `Function.schedule` (field 72) as a `Schedule.Cron`/`Schedule.Period`, mirroring `schedule.py:12/61`. The macro canonicalizes the call form to a spec the SDK's `parse_schedule` parses; `with_schedule` rides it into the deploy FunctionCreate. See §8. |
| `min_containers` / `max_containers` / `buffer_containers` (793-795) | ✅ | `#[function(min_containers = .., max_containers = .., buffer_containers = ..)]` → `Function.autoscaler_settings` (field 79) + the deprecated mirror fields Modal still sets (`warm_pool_size`/`concurrency_limit`/`_experimental_buffer_containers`), mirroring `_functions.py:764-768,1019-1021`. Validated like Modal (`max >= min`). `ops/function.rs` `with_autoscaler`; `examples/autoscaling`. |
| `scaledown_window` (796) | ✅ | `#[function(scaledown_window = <secs>)]` → `Function.autoscaler_settings.scaledown_window` + the legacy `task_idle_timeout_secs`, mirroring `_functions.py:768,1022`. Validated `> 0` (Modal `_functions.py:761`). `with_autoscaler`. |
| `@concurrent` (input concurrency) | ❌ | `_partial_function.py:700` `_concurrent` (replaces `allow_concurrent_inputs`); sets `max_concurrent_inputs`. We run one input per container. |
| `@batched` | ❌ | `_partial_function.py:639` `_batched` — server-side input batching. |
| `region` (804) / `cloud` (803) | ❌ | Region/cloud placement (`scheduler_placement.py`). |
| `proxy` (797) | ❌ | `_Proxy` egress (`proxy.py`). See §8. |
| `ephemeral_disk` (792) | ❌ | Scratch disk sizing. |
| `enable_memory_snapshot` (807) | ✅ (`#[cls]`, CPU) | `#[cls(enable_memory_snapshot = true)]` — the expensive `#[enter]` load runs **once ever** on a DEPLOYED app, Modal snapshots the loaded process, and every later (even cold) container RESTORES it instead of re-running the build. DEPLOY-ONLY: the flag rides into `Function.checkpointing_enabled`/`is_checkpointing_function` (proto 41/40) only at the deploy boundary; RUN stays wire-identical. A deploy bakes `MODAL_RUST_SNAPSHOT_PRIME=1` so the wrapper primes `#[enter]` inside the snapshot window. `#[function(enable_memory_snapshot)]` is a `compile_error` (cls-only in v0). `examples/snapshot-class`. Remaining gap: the GPU snap/restore split (a `restore` frame + `#[restore]` hook) — see §8. Modal `snapshot.py`. |
| `block_network` (808) | ❌ | Network isolation. |
| `restrict_modal_access` (809) | ❌ | |
| `network_file_systems` (788) | ❌ | See §5. |
| `is_generator` (802) | ❌ | Generator/streaming functions — see §6. |
| `serialized` (787) | ❌ (by design) | We are FILE-mode only — see §0. |
| `max_inputs` / `single_use_containers` (815) | ❌ | Single-use containers. |
| Clustered (`i6pn`, `cluster_size`, `rdma`) | ❌ | Multi-node clustered functions (`_clustered_functions.py`, experimental). |

The high-value, cheap wins here — **`cpu` / `memory`**, **`retries`** (both the
int form AND the `Retries(...)` struct form for custom backoff/delays),
**autoscaling** (`min`/`max`/`buffer_containers` + `scaledown_window`), and a
**per-function custom `image = Image(..)`** — are all now ✅. The remaining
decorator gaps (`@concurrent`, `@batched`, region/cloud, …) are M-sized or
runtime-coupled.

---

## 5. Volumes, NFS, CloudBucketMount

Modal `Volume` (`volume.py`) is a rich, callable object; we use only the
attach-by-name + mount slice.

| Capability | Status | Note (`volume.py`) |
|---|---|---|
| `Volume.from_name(create_if_missing=...)` + mount | ✅ | `volume_get_or_create` (`ops/volume.rs:27`) → `Function.volume_mounts`. |
| Cargo build cache as a V2 volume | ✅ | modal-rust-specific; on by default. |
| `commit` (793) / `reload` (811) | ❌ | Manual persist/refresh from inside a container. |
| `read_file` (878) / `read_file_into_fileobj` (923) | ❌ | |
| `iterdir`/`listdir` (838/868) | ❌ | |
| `batch_upload` (1066) / `copy_files` (1019) / `remove_file` (1003) | ❌ | Host-side volume file management. |
| `with_mount_options` / `read_only` (476/445) | ❌ | We always mount read-write with background commits. |
| `ephemeral` (691) / `delete` (1104) / `rename` (1121) / `info`/`list` | ❌ | Volume lifecycle/management RPCs. |
| `NetworkFileSystem` (`network_file_system.py`) | ❌ | The `network_file_systems=` mount type entirely. |
| `CloudBucketMount` (`cloud_bucket_mount.py`) | ❌ | S3/GCS/R2 bucket mounts (a valid value in `volumes=`). |

We do attach + mount; we do **not** offer any of Modal's volume *data* API
(read/write/list/commit from host or container).

---

## 6. Invocation

`_functions.py` invocation surface. Ours: `crates/modal-rust/src/function.rs`.

| Method | Status | Note |
|---|---|---|
| `.local()` (1761) | ✅ | In-process via the frozen Registry. |
| `.remote()` (1703) | ✅ | |
| `.spawn()` (1860) + `FunctionCall.get()` | ✅ | One input; `get(timeout)`. |
| `.map()` (1922) | ✅ | Ordered fan-out, fail-fast. |
| `.starmap()` (1923) | ✅ (single-arg framing) | `Function::starmap` — each input item IS the one named-object `In` (a tuple/sequence shape); shares `.map()`'s ordered wire path. True multi-arg positional spread is still gated on multi-arg (see below). `examples/spawn-map-foreach`. |
| `.for_each()` (1924) | ✅ | `Function::for_each` — runs N inputs across containers, WAITS, discards outputs (returns `()`). Built on the proven ordered-map collect (decodes into `IgnoredAny`), fail-fast. `examples/spawn-map-foreach`. |
| `.spawn_map()` (1925) | ✅ | `Function::spawn_map` — fire-and-forget fan-out: opens an ASYNC MAP call, enqueues N inputs, returns a `FunctionCall` handle immediately (no result collection). SDK `spawn_map_cbor`. `examples/spawn-map-foreach`. |
| `.map.aio` / async variants | ❌ | Modal exposes `.aio` async forms; our methods are already `async fn`, but there is no sync/streaming-iterator distinction. |
| Streaming/unordered map results | ❌ | Modal can yield outputs as they complete (`order_outputs=False`); we collect all in input order before returning a `Vec`. |
| `.remote_gen()` (1724) / generators | ❌ | Streaming/generator returns — depends on `is_generator`; no Rust analogue yet. |
| `FunctionCall.get(timeout)` partial-timeout, `cancel`, `gather` | 🟡 | We have `get(timeout)`; no `cancel`; Modal's free `gather()` (`_functions.py:2099`) over many calls is absent. |
| `get_current_stats` (1895) / `update_autoscaler` (1152) | ❌ | Runtime introspection / live autoscaler control. |
| Multi-arg / positional args | ❌ (by design today) | The macro accepts exactly one named-object `In` (`modal-rust-macros/src/emit.rs:104`); multi-arg + `starmap` are reserved but unimplemented. Modal passes `*args, **kwargs`. |

---

## 7. Classes / `Cls` and lifecycle

✅ (v0, Shape A) for the core load-once-serve-many pattern; the rest of the
lifecycle is deferred. Modal's `Cls` (`cls.py:446` `_Cls`) plus the partial-function
decorators in `_partial_function.py`:

| Modal feature | Status | Ref |
|---|---|---|
| `@app.cls(...)` stateful classes | ✅ (v0, Shape A: `#[cls]` on an `impl` block) | `cls.py`, `@cls` (885). |
| `@method` | ✅ | per-method dotted entrypoint + merged class/method config; `_partial_function.py:282`. |
| `@enter` | ✅ | load-once `OnceLock` singleton + `modal_runner --serve`; `_partial_function.py:588`. |
| `@exit` | ❌ (deferred to Shape B) | marker reserved; emits a `compile_error` for now. `_partial_function.py:616`. |
| `modal.parameter(...)` class params | ❌ (deferred to Shape B) | use `#[cls(secrets=[..])]` + `std::env` in `#[enter]` for now. `cls.py:935`. |
| `@concurrent` / `@batched` on methods | ❌ | `_partial_function.py:700` / `639`. |

The Rust shape: a plain struct holds the state, and a `#[cls(gpu=.., timeout=..)]`
attribute on its `impl` block carries the class-level config. Inside, `#[enter] fn
load() -> anyhow::Result<Self>` builds the expensive state ONCE per warm container —
the macro moves the built value into a process-lifetime `OnceLock` singleton and adds
an additive `modal_runner --serve` loop so a warm container loads once and serves
many inputs. Each `#[method(gpu=..)] fn embed(&self, ..)` becomes its OWN per-method
entrypoint under the **dotted `"<Class>.<method>"` name** (e.g. `Embedder.embed`),
carrying its fully-resolved class-default + method-override config. The dotted object
tag is **live-confirmed on a T4** — Modal accepts it on both `run` and `deploy`.
Callers use a generated `EmbedderHandle` + `EmbedderCls` extension trait (brought in
with `use <crate>::*;`): `app.embedder().embed("hi".into()).remote().await?` (or
`.local()?`). See `examples/stateful-class` and `crates/modal-rust/tests/cls.rs`.

Caveat: two methods with DIFFERENT effective config become DIFFERENT Modal functions
(different containers), so warm load-once reuse holds across methods that share the
same effective config (the common all-inherit case).

Deferred to Shape B: `#[exit]` (the marker is reserved but emits a `compile_error`)
and class parameters (`modal.parameter`). Until then, inject config via
`#[cls(secrets=[..])]` + `std::env` reads in `#[enter]`.

---

## 8. Other object types and platform surfaces

| Surface | Status | Modal ref |
|---|---|---|
| **Web endpoints** — `@fastapi_endpoint`/`@web_endpoint` | ✅ (`#[endpoint]`, FUNCTION type; live-proven 2026-06-10: POST 200 + computed JSON, GET 405, 422/500 redacted) | `_partial_function.py:336` / `400`. `#[modal_rust::endpoint(method = "POST", <any #[function] config>)]` on a plain handler → `webhook_config{type: FUNCTION, method, requires_proxy_auth}` + the ASGI data-format pair on the DEPLOY `FunctionCreate`; DEPLOY-ONLY (the URL is assigned on deploy; RUN stays wire-identical, like `enable_memory_snapshot`). The deploy image auto-adds `fastapi[standard]` (Modal rejects FUNCTION webhooks without it) and the baked wrapper gains a per-endpoint `(request: Request)` adapter reusing the SAME `--serve` child — so `#[cls]` load-once and memory snapshot compose with endpoints. HTTP contract = the auto-IO contract (body in, value out; envelope errors → 422 decode / 500 handler). Public by default; `requires_proxy_auth = true` opts into Modal proxy-auth. The fn stays a normal function for `.local()` and RUN (webhook suppressed); the DEPLOYED endpoint is HTTP-only in v0 — Modal's worker ASGI-wraps the callable, so envelope `.remote()`/`call` against it is rejected (live-verified); both-surfaces = Modal's own idiom (plain `#[function]` + thin `#[endpoint]` caller). `examples/web-endpoint`, §8. Remaining gaps: custom domains, `requested_suffix`, `web_endpoint_docs`, the ephemeral `modal serve`-style dev URL, `#[endpoint]` on `#[cls]` methods. |
| `@web_server` | ✅ Have (v0, deploy-only) | `_partial_function.py:525`. `#[modal_rust::web_server(port = <u16>, <any #[function] config>, startup_timeout = <secs>)]` on a `(port: u16) -> anyhow::Result<()>` fn that launches a real HTTP server and blocks → `webhook_config{type: WEB_SERVER, port}` on the DEPLOY `FunctionCreate`. Modal proxies the raw port, so multi-route apps, SSE streaming, and websockets work as-is. DEPLOY-only (URL assigned on deploy; RUN stays a plain function). Dogfood: `examples/burn-lm-bench` (burn-lm-http GPU inference server). Remaining: the ephemeral `modal serve`-style RUN URL. |
| `@asgi_app` / `@wsgi_app` | ❌ | `_partial_function.py:413` / `468`. A *Python* ASGI/WSGI callable almost certainly never maps to Rust — use `#[web_server]` for full Rust HTTP apps. |
| **Sandboxes** (`Sandbox.create`, `exec`, filesystem, tunnels, snapshots) | ❌ | `sandbox.py:450`, `1605`, `1907`, `1427`. A large, self-contained subsystem. |
| **`Dict`** (distributed key/value) | ✅ (v0 subset) | `dict.py`. `modal_rust::Dict` (client-gated): named lifecycle (`from_name`/`lookup`/`from_name_in`/`delete`) + `get`/`put`/`put_if_absent`/`pop`/`contains`/`len`/`clear`, per-call generic values (`d.get::<i64>("k")`), `&str` keys, plus `_raw` byte escape hatches. **Python interop by design**: keys are byte-exact CPython protocol-4 pickle, values are restricted pickle (serde-pickle) — plain data (str/int/float/bool/bytes/lists/dicts/structs-as-dicts) round-trips with Python (the Go/JS-client precedent); a pickled Python custom class fails with a typed codec error. `examples/dict-kv` — offline mock round-trips + python3 interop tests, **live-proven 2026-06-11**: a container `#[function]` wrote 4 scores into the named Dict over the in-container unix-socket client and the caller read them back typed (`jazz -> Some(29)`); fresh-container reruns ~9-10 s vs minutes cold (runner-binary cache + target/ archive + concurrent upload probes); entries visible from Modal's own `modal dict items` (cross-language proof). Deferred: partitions n/a; ephemeral dicts + heartbeat, `keys()/values()/items()` iteration (`DictContents` streaming), batch `update`, `DictList`/`GetById`. |
| **`Queue`** (distributed queue) | ✅ (v0 subset) | `queue.py`. `modal_rust::Queue` (client-gated): named lifecycle (`from_name`/`lookup`/`from_name_in`/`delete`) + `put`/`put_many`/`get`/`get_many`/`len`/`clear`, plus `_raw` escape hatches. Blocking `get(timeout)` mirrors Python (`None` = block forever, `Some(d)` = timeout → `Ok(None)`, `Some(ZERO)` = non-blocking poll) via the SDK's client-side poll loop (per-RPC cap, never one long gRPC deadline). Same restricted-pickle interop boundary as Dict. `examples/queue-pipeline` — offline mock round-trips, **live-proven 2026-06-11**: a container worker drained 4 jobs / 256 steps FIFO from the named Queue (blocking `get` live) and the demo Queue was deleted on exit. Deferred: partition keys/TTL knobs (builders default empty + 24 h), ephemeral queues + heartbeat, `iterate()` (`QueueNextItems`), block-on-full put retry, `QueueList`/`GetById`. |
| **Schedules** — `Cron` / `Period` | ✅ | `schedule.py:12` / `61`; wired via the `schedule=` decorator field (§4) → `Function.schedule`. `examples/scheduled-job` is a deployed cron function. |
| **`Proxy`** (static-egress proxy) | ❌ | `proxy.py`. |
| **Scaling / autoscaler control** | 🟡 | Static config `min/max/buffer_containers` + `scaledown_window` are ✅ (§4) → `Function.autoscaler_settings`. Live `update_autoscaler` (§6) is still Missing. |
| **Tunnels** (`forward`) | ❌ | `_tunnel.py`. |
| **Cls-based memory snapshot / checkpointing** | ✅ (`#[cls]`, CPU) | `snapshot.py`. `#[cls(enable_memory_snapshot = true)]` pays the expensive `#[enter]` load **once ever**, snapshots the loaded process, and restores it on every (even cold) container start — extending load-once-serve-many across cold starts. DEPLOY-ONLY (RUN stays wire-identical); a typed `prime` lifecycle frame on the `--serve` loop runs `#[enter]` inside Modal's snapshot window; a FAILED prime fails container init loudly by default (`MODAL_RUST_SNAPSHOT_BEST_EFFORT=1` opts into degrading to the lazy `#[enter]` path). `examples/snapshot-class`, §4. The GPU snap/restore split (load CPU in the snapshot window, move to GPU after a `restore` frame + `#[restore]` hook) is the remaining gap. |
| **Logs streaming / `modal logs`** | 🟡 | We stream image-build logs (`ImageJoinStreaming`) and function-output logs inline; no general `app logs` / live function log tail API. |
| **Environments / Workspaces management** | 🟡 | We resolve/use the configured environment (`env_or_default`); no create/list environment RPCs (`environments.py`, `workspace.py`). |
| **Billing / call graph / clustered functions** | ❌ | `billing.py`, `call_graph.py`, `_clustered_functions.py`. |

---

## 9. CLI parity (`modal-rust` vs `modal`)

Our CLI is first-party and programmatic (the Python-shim codegen was deleted). It
covers the run/deploy/call flow for Rust entrypoints. Modal's `cli/` is far broader
(`modal run`/`deploy`/`serve`/`shell`/`app`/`volume`/`secret`/`dict`/`queue`/
`nfs`/`environment`/`token`/`launch`/`container`/`profile` subcommands). We do not
aim for `modal` CLI surface parity — most of those subcommands manage object types
we do not implement (§5, §8). `modal serve` (hot-reload dev server, `serving.py`)
has no equivalent.

---

## 10. Suggested priority for closing gaps

Ordered by value-to-effort for a Rust-on-Modal runtime:

1. ~~**`cpu` / `memory` decorator fields**~~ — DONE: `#[function(cpu = 2.0, memory =
   4096)]` → `FunctionResources` (`milli_cpu`/`memory_mb`); the macro parses both
   (`examples/burn-add` uses `memory = 8192`). Remaining gap: Modal's `(request, limit)`
   tuple form.
2. ~~**`retries`**~~ — DONE (int + struct form): `#[function(retries = N)]` and
   `#[function(retries = Retries(max_retries = N, backoff_coefficient = f,
   initial_delay = s, max_delay = s))]` → `retry_policy` (`with_retries` /
   `with_retry_policy`).
3. ~~**General `pip_install` / `apt_install` / `run_commands` image steps**~~ — DONE:
   `RemoteConfig::image_steps` / `DeployConfig::image_steps` carry ordered `ImageStep`s
   (`apt`/`pip`/`run`) rendered into the image dockerfile; `examples/pip-apt-image`.
   Also DONE: a **per-function custom `image = Image(base/install_rust/apt/pip/run)`**
   decorator field (the `app.function(image=..)` analogue) — `base`/`install_rust`
   override the path default, `apt`/`pip`/`run` prepend to the path steps.
4. ~~**Inline `secrets = {dict}` / `required_keys`**~~ — DONE:
   `#[function(secrets=[..], required_keys=[..])]` threads asserted keys into
   `from_name`; `#[function(env={"K"="V", ..})]` resolves an inline `Secret.from_dict`
   into the same `Function.secret_ids` (so `env` + `secrets` compose).
5. ~~**`min/max/buffer_containers` + `scaledown_window`**~~ — DONE: static autoscaling
   control via `#[function(min_containers = .., max_containers = .., buffer_containers =
   .., scaledown_window = ..)]` → `Function.autoscaler_settings`; `examples/autoscaling`.
6. ~~**`schedule` (`Cron`/`Period`)**~~ — DONE: `#[function(schedule = Cron(..)/Period(..))]`
   → `Function.schedule`; `examples/scheduled-job` is a deployed cron job.
7. ~~**`starmap` / `for_each` / `spawn_map`**~~ — DONE: the rest of the map family on
   the facade `Function` (`Function::starmap`/`for_each`/`spawn_map`, built on the
   proven `.map`/`.spawn` plumbing + SDK `spawn_map_cbor`); `examples/spawn-map-foreach`.
   `starmap` is single-arg-framed (each item IS the one named-object input); true
   multi-arg positional spread is still gated on multi-arg.
8. ~~**`Cls` (load-once-serve-many)**~~ — DONE (v0, Shape A): `#[cls]` on an `impl`
   block with `#[enter]` (load-once `OnceLock` + `modal_runner --serve`) and per-method
   dotted `"<Class>.<method>"` entrypoints with merged config; live-confirmed on a T4.
   `examples/stateful-class`. Deferred to Shape B: `#[exit]` + `modal.parameter` class
   params (see §7).
9. **Bigger subsystems** — each is a milestone-sized effort. ~~**Web endpoints**~~ —
   DONE (v0, `#[endpoint]`, FUNCTION type; see §8 — `@web_server`/ASGI shapes and
   custom domains remain follow-ups). ~~**Dict / Queue**~~ — DONE (v0 subset; see §8 —
   named-only, restricted-pickle Python interop for plain data; partitions/TTL/
   ephemeral/iteration deferred). With those landed, **Sandboxes** is the largest
   remaining gap.
