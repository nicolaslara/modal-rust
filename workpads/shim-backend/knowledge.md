# Shim Backend Knowledge

## Objective

Capture the design space for the Modal Python control-plane backend: how much
Python must exist, where it should live, and whether "generation" should mean
source code generation or just config/data generation. This workpad is
exploratory and must preserve the existing architecture constraints:

- `run` builds Rust at function-execution time (`copy=False` source mount, or a
  documented Sandbox fallback if required).
- `deploy` builds Rust at image-build time (`copy=True` + `run_commands`) and the
  deployed runtime never invokes `cargo`.
- `call` invokes an already-deployed function and never mounts source or builds.
- v0 currently uses generated Python + the official `modal` CLI as the known-good
  control path.

## Gate Status

Not passed yet.

The workpad passes when this file contains a decision-ready comparison of shim
backend alternatives, a static-shim config contract, a minimal spike plan, and a
recommendation or explicitly recorded blockers.

## Current Baseline

The current CLI uses parameterized fixture templates:

- `crates/modal-rust-cli/src/templates.rs` embeds three Python template files with
  `include_str!`.
- `dev_app.py.tmpl`, `deploy_app.py.tmpl`, and `call_app.py.tmpl` are mostly
  static Python shims with a few `{{PLACEHOLDER}}` constants.
- `main.rs` renders those strings into `.modal-rust/generated/{dev_app.py,
  deploy_app.py,call_app.py}` and invokes the official `modal` CLI with a file
  path.

The current placeholders are small:

- app names (`dev_app_name`, `deploy_app_name`, `call_app_name`);
- Rust image version;
- local source/workspace path;
- deploy app lookup name for `call`.

Entrypoint and input are already data. They flow through `modal run` flags into
the shim's `@app.local_entrypoint()` as `entrypoint` and `input_json`.

## Design Space

### Option A - Status quo: parameterized Python templates

Keep rendering Python source files under `.modal-rust/generated/`.

Pros:
- Already proven by M9a as byte-equivalent to the prototype shims.
- Simple to debug: the exact Python module passed to `modal` is visible.
- Matches the official `modal run <file>::main` / `modal deploy <file>` path.
- Low implementation risk.

Cons:
- Feels like generated source even though only a few constants vary.
- As config grows, Python templating can become a second product surface.
- Many apps/deployments may produce many near-identical files.
- Template substitutions require care around quoting/escaping if values become
  richer than paths and names.

### Option B - Fully static shims with `MODAL_RUST_CONFIG_JSON`

Ship static Python files and pass all import-time configuration through an env
var containing JSON.

Pros:
- Python source is always static; generated artifact becomes data.
- Simple for small configs and easy to hash/debug.
- Rust can own static shim bytes deterministically via `include_str!` or
  `include_bytes!`.
- Avoids adding a template language.

Cons:
- Env vars are awkward for large configs.
- Must avoid secrets in logged command lines/env dumps.
- The config must be present in the local environment when the Modal CLI imports
  the module, because app/image/function definitions happen at import time.
- Shell quoting can be painful unless Rust sets env directly on `Command`.

### Option C - Fully static shims with `MODAL_RUST_CONFIG_PATH`

Ship static Python files and pass a path to a generated JSON/TOML config file.

Pros:
- Better for large configs, many functions, volumes, secrets refs, GPU config,
  cache policies, and future app metadata.
- Easier debug artifact: inspect one config file plus one static shim version.
- Avoids command-line/env length and quoting concerns.

Cons:
- Still materializes a generated file, though it is data not code.
- The config path must be readable from the local process that imports the Modal
  module; if the shim is moved to a cache/package location, path handling must be
  deliberate.
- Need lifecycle rules for cleanup, cache invalidation, and `--keep`/debug dumps.

### Option D - Static shims as an installed/importable Python package

Install or bundle a Python package such as `modal_rust_backend.run_app` and invoke
it with `modal run -m ...` if Modal's module path supports the needed flow.

Pros:
- Can eliminate project-local shim files.
- Python source can be versioned/published with the CLI.
- Cleaner UX if the `modal` CLI module mode is reliable.

Cons:
- Adds Python packaging/install complexity to a Rust-first tool.
- Version skew between the Rust CLI and installed Python package becomes a real
  failure mode unless Rust materializes or verifies the exact package bytes.
- Still requires import-time config through env/path.

### Option E - Static shims materialized in OS temp/cache

Rust embeds static shim bytes and writes them to `$TMPDIR` or an OS cache
directory only because the official Modal CLI wants an importable file/module.

Pros:
- Keeps user projects cleaner than `.modal-rust/generated/`.
- Static bytes can be content-addressed by shim version/hash.
- Works with the current official CLI path.
- Debug mode can copy the exact shim/config into `.modal-rust/debug/`.

Cons:
- Hidden temp files can make debugging harder unless the CLI prints/dumps them on
  request.
- Cache cleanup and stale files need a simple policy.
- File paths in Modal logs may point outside the project.

### Option F - Static shims baked into a Modal image/base image

Put the shim backend into an image layer or base image.

Pros:
- Useful for runtime pieces that do not need to vary per project.
- Can reduce repeated upload of static Python source after the app is authored.

Cons:
- Does not remove the local authoring problem: `modal deploy` still needs Python
  code locally to define `app`, `image`, and functions.
- Image-baked code is less convenient during local iteration.
- Must not blur the build boundary: deploy image build may bake the runner, while
  run mode must still build at function-execution time.

### Option G - Python SDK subprocess/backend without `modal run <file>`

Rust could spawn Python and call Modal SDK internals directly, feeding static code
through stdin or `python -c`, rather than invoking the official `modal` CLI with a
module path.

Pros:
- Could reduce or eliminate local shim file materialization.
- Gives Rust more direct control over config and orchestration.

Cons:
- Becomes a new Modal control path, not the currently validated pure wrapper.
- Risks relying on Modal Python internals instead of the public CLI.
- Needs fresh empirical validation for run/deploy/call behavior and logs.

### Option H - Lower-level `modal-rs`/protobuf authoring backend

Use Modal's API/protobuf surface directly from Rust, avoiding Python shim source.

Pros:
- Long-term Rust-native control plane, if feasible.
- Could make uploads/deploys more deterministic from Rust.

Cons:
- Prior research found Modal function creation is still Python-shaped: prepared
  functions need a serialized Python callable/image relationship.
- High compatibility risk around Modal's private protocol and Python
  serialization.
- Too large for v0 unless a focused spike proves the missing authoring pieces.

### Option I - Hybrid: static embedded bytes plus optional debug dump

Default to static shims embedded in Rust and config as data, materialized only in
a hidden cache/temp path; add `--dump-shim`/`--keep-shim` for debugging.

Pros:
- Keeps Python source static and deterministic.
- Preserves the official Modal CLI path.
- Gives good debug escape hatches.
- Scales better to many apps because app-specific values are config, not source.

Cons:
- More moving pieces than the current generated-template path.
- Requires a config contract and compatibility/versioning story.

## Key Distinctions

- **No generated Python source**: plausible soon. Static shim source plus generated
  config/data.
- **No local shim file**: plausible through an installed module path or deeper SDK
  backend; otherwise the official CLI still needs something importable.
- **No Python control plane at all**: a much larger backend change and not yet
  proven feasible.

## Static Config Contract Sketch

Open draft; exact field names are not locked.

```json
{
  "schema_version": 1,
  "mode": "run",
  "app_name": "modal-rust-poc-dev",
  "deploy_app_name": "modal-rust-add-poc",
  "call_app_name": "modal-rust-call",
  "rust_image": "rust:1-slim",
  "python_version": "3.12",
  "local_src": "/Users/nicolas/devel/modal-rust",
  "remote_src": "/src",
  "copy": false,
  "ignore": ["target", ".git", ".modal-rust", "**/*.rlib"],
  "remote_env": {"RUST_BACKTRACE": "1"},
  "timeout_seconds": 1800,
  "build": {
    "kind": "function_body",
    "commands": ["cargo build --release --bin modal_runner"],
    "cargo_home": "/tmp/cargo",
    "cargo_target_dir": "/tmp/target"
  },
  "gpu": null,
  "volumes": [],
  "secrets": [],
  "cache": null
}
```

Import-time fields:
- `mode`, `app_name`, `rust_image`, `python_version`, `local_src`, `remote_src`,
  `copy`, `ignore`, `remote_env`, `timeout_seconds`, GPU, volumes, secrets, and
  deploy `run_commands`.
- Reason: the Python module constructs `modal.App`, `modal.Image`, decorators, and
  `@app.function` definitions when imported by the Modal CLI.

Local-entrypoint runtime fields:
- `entrypoint`, `input_json`, and possibly debug flags.
- Reason: these are parameters to `main(...)` and do not affect Modal app/image
  construction.

Remote-function runtime fields:
- Runner invocation arguments, input file path, build directory selection, and
  runtime env such as `CARGO_HOME`/`CARGO_TARGET_DIR`.

Config transport:
- Small configs can use `MODAL_RUST_CONFIG_JSON`.
- Larger configs should use `MODAL_RUST_CONFIG_PATH`.
- The static shim should support both, preferring JSON when present and otherwise
  reading the path.

## Template Language Assessment

A template language is probably not needed if the Python source becomes static.
The current substitution set is small, and richer values are better represented
as typed config data than as generated Python syntax. If Python source generation
continues and the variable surface expands, a template engine could improve
escaping and readability, but that would also entrench Python codegen as a
product surface.

## Open Questions

- Should static config be JSON only, or TOML for human-written defaults plus JSON
  for generated invocation config?
- Should static shims live under `.modal-rust/generated/`, `$TMPDIR`, an OS cache
  directory, or an installed Python package?
- Should the default debug behavior print the shim path, the config path, both,
  or only print them under `--verbose`?
- How should secrets be represented so they are Modal secret references, never raw
  secret values in config logs?
- Do GPU, volumes, and future concurrent-input/autoscaling knobs fit one generic
  static shim per mode, or do they require mode-specific static variants?
- Can `modal run -m <module>` satisfy all run/call/deploy needs, or is a file path
  simpler and more reliable?
- Is a lower-level `modal-rs`/protobuf authoring path worth another spike after
  v0, or should it remain call-only unless Modal exposes a stable authoring API?

## Initial Recommendation

Do not replace the current M9a path before GPU work. For the next refinement,
explore **Option I: static embedded shim bytes + env/path config + optional debug
dump**. It keeps the official Modal CLI path and the build-boundary proof, while
turning app-specific variation into data instead of generated Python source.

---

### Design review — avoid generating shims? (2026-06-04, bg agent)

**Verdict:** per-project Python *codegen* can be eliminated; Python in the
authoring path cannot (Modal `FunctionCreate` needs a serialized Python callable
+ image_id — §1.5). Separate three claims: (1) no generated *source* — achievable,
low risk; (2) no Python *file* — achievable, medium; (3) no Python *at all* via
modal-rs authoring — **blocked** (FunctionCreate + pickle proto 2/3 vs 4); keep
modal-rs to `call` only.

**Recommendation: Option 2 — ONE static parameterized shim + typed config-as-data**
(JSON via `Command` env, no shell-quoting), materialized to an OS cache dir with
`--dump-shim` debug escape (Option 5E). Stays on the validated `modal` CLI path;
per-project variation becomes typed Rust `RunConfig` fields, not template
placeholders or `.py` files; best substrate for every future knob (cache, per-fn
gpu, CUDA tiers, local-vs-modal target). Low, mechanical migration. Forward-compatible
with a later pip package (Option 4) and with modal-rs `call` — no contract change.

**Highest-leverage idea — a runner `--describe` JSON manifest (Rust = source of truth).**
Extend the `inventory` `Registration` (today: name+handler) with optional
`FunctionConfig {gpu, cache, timeout}` populated by the proc-macro from
`#[modal_rust::function(gpu="A100")]` / `#[modal_rust::app(cache=...)]`; add
`modal_runner --describe` → JSON manifest of all functions+config. The CLI reads it
(fast local build / cached manifest) to learn per-function config BEFORE the remote
build — instead of brittle static attribute-parsing. The decorator *is* the config
(matches Modal Python). Additive to the frozen runner protocol. Caveat: `--describe`
needs a LOCAL compile of the user crate → motivates feature-gated CUDA (crate compiles
locally without CUDA; also removes the burn-add default-members exclusion).

**Open questions (for the user):** (1) per-function config (gpu/cache/timeout per fn,
like Modal) vs app-level-per-invocation? (2) source of truth: Rust attributes (via
--describe) vs modal-rust.toml vs CLI flags? (3) "no codegen" enough, or "no Python
file anywhere" (pip package)? Review recommends per-fn + attributes + no-codegen.

**Spikes to de-risk:** (a) static shim reads `MODAL_RUST_CONFIG_JSON` at IMPORT time +
builds App/Image/decorators from it (offline `python -c` then one `modal run`); (b)
`--describe` manifest cost / local metadata build; (c) [deferred] modal-rs minimal-
Python-entrypoint deploy round-trip (only if Option 3 ever revived).

---

### DIRECTION LOCKED — Rust as a programmatic Modal control plane (2026-06-04, user steer)

The shim review concluded "avoid codegen." The user pushed further: two smells —
"why must the CLI read config pre-build? isn't it dynamic?" and "I want a local
main that orchestrates Modal like Python's local_entrypoint + .remote().await" —
**converge on the same pivot: drive Modal PROGRAMMATICALLY from Rust (modal-rs /
our own gRPC client), not generate-shims + shell-out to the `modal` CLI.**

Why this resolves everything:
- **Dynamic config (no static parse / no --describe-before-build):** the Rust
  control plane reads its OWN registry (gpu/cache/timeout from `#[modal_rust::
  function/app(...)]`) and passes it straight into the `FunctionCreate` request at
  runtime. Config is dynamic, sourced from Rust — the decorator IS the config,
  like Modal Python.
- **Local orchestration (point 4 = `@app.local_entrypoint()`):** a normal local
  Rust `main()` does `app.function("train").remote(cfg).await?` to drive remote
  Modal functions; `.local()` runs in-process. Needs the programmatic client.

UNBLOCKING the review's "Option 3 blocked" finding (FunctionCreate needs a
serialized **Python** callable): we do NOT avoid the Python callable — we
**generate + serialize ONE embedded wrapper from Rust**, the same way Modal's SDK
does it (`function_serialized`; CBOR ideally, else cloudpickle/pickle), and send the
bytes over gRPC. The default Python wrapper lives in THIS crate (not per-project);
a debug **`dump`** tool lets users emit + customize it. → no per-project Python files;
"no Python file visible anywhere" is reachable once proven (initially keep a local
file for testing).

Pillars: (A) programmatic Modal control via modal-rs/gRPC; (B) one embedded wrapper,
serialized-from-Rust as Modal expects; (C) dynamic config from the Rust registry into
FunctionCreate; (D) local `main` orchestration `.remote().await` + `.local()`;
(E) cache ON by default, Volume-backed — OPEN: native Modal volume bulk-copy/snapshot
primitive vs DIY rsync; (F) `dump` debug/escape hatch; (G) feature-gated CUDA so the
crate compiles locally (enables `.local()`, local metadata, removes burn-add exclusion).
KEEP unchanged: the runner + `inventory` registry + `typed!`/`#[modal_rust::function]`
macros — this re-architects the AUTHORING/control layer only.

Plan = Research (Modal SDK serialization + FunctionCreate gRPC; modal-rs surface;
native Volume copy primitive; local-orchestration semantics) → **Feasibility spike**
(prove Rust can programmatically create+invoke a Modal function → {"sum":42} with NO
`modal` CLI and NO generated file) → Design → staged build. FALLBACK if the
programmatic FunctionCreate proves infeasible: the static-shim Option 2.
(Stashed snapshot-sync cache draft is superseded by this direction — keep only as ref.)

---

## Programmatic backend — grounded findings + plan (2026-06-04)

Synthesis of four primary-source research passes (Modal Python **1.3.2** at
`/Users/nicolas/.local/pipx/venvs/modal/lib/python3.14/site-packages/modal/`
incl. vendored `modal_proto/api.proto`; modal-rs **0.1.3** source). The pivotal facts
below were **re-verified directly** against the installed 1.3.2 source while writing
this section (file:line citations are from that tree).

### A. Verified Modal serialization / FunctionCreate facts

**The plan-reshaping correction: there are TWO ways to create a function, not one.**
The earlier synthesis (§1.5/§2.7) only described SERIALIZED mode ("FunctionCreate needs
a serialized Python callable"). That is true *only* in SERIALIZED mode. The default for a
normal `@app.function` in a `.py` file is **FILE mode**, which needs **no pickled callable
at all** — just two strings + an importable module on the image.

- `definition_type` is chosen by `FunctionInfo.get_definition_type()`
  (`_utils/function_utils.py:141-145`): `DEFINITION_TYPE_SERIALIZED` (=1) if
  `is_serialized()`, else `DEFINITION_TYPE_FILE` (=2).
- **SERIALIZED mode** (`_functions.py:933-937`): `function_serialized =
  info.serialized_function()` = `serialize(raw_f)` = `cloudpickle.Pickler(buf,
  protocol=4).dump(obj)` (`_serialization.py:32-37,100`). Size guard: 16 MiB hard error
  / 256 KiB warning (`_functions.py:941-950`). Container side **deserializes** the blob
  (`_runtime/user_code_imports.py:512`, `DEFINITION_TYPE_SERIALIZED`) — the blob carries
  a `types.CodeType` with **version-specific CPython bytecode** + a `STACK_GLOBAL` ref to
  `modal._vendor.cloudpickle._make_function`. Emitting this from Rust is impractical
  (CPython-bytecode-coupled, brittle across upgrades).
- **FILE mode** (the default): `function_serialized = None` →
  `function_serialized=function_serialized or b""` (**empty bytes**, `_functions.py:956,1001`).
  The function is identified purely by `module_name` + `function_name`
  (`_functions.py:994-995`). Container side does
  `module = importlib.import_module(function_def.module_name)` then
  `f = getattr(module, qual_name)` (`_runtime/user_code_imports.py:475,488`). **No pickle of
  the callable.** → This is the easy, robust target for a Rust control plane: ship ONE
  importable Python wrapper module on the image + send two strings.
- **Resources / GPU flow into FunctionCreate dynamically** (`_functions.py:1121-1125`):
  `function_definition.resources.CopyFrom(convert_fn_config_to_resources_config(cpu,
  memory, gpu, ephemeral_disk, rdma))`. A GPU **list** (fallback ranking) routes through
  `FunctionData.ranked_functions` (`_functions.py:1101-1116`). Proto: `Function.resources=9`
  → `Resources{ GPUConfig gpu_config{count, gpu_type}, milli_cpu, memory_mb, … }`.
  **This confirms config is dynamic at create-time — no pre-build static parse / no
  `--describe`-before-build is required by Modal.** It flows straight from the Rust registry.
- **Volumes** (`_functions.py:969-973`): `api_pb2.VolumeMount(volume_id, mount_path,
  allow_background_commits=True, read_only)`; attached as `function_definition.volume_mounts`.
- **Args/results format** (`_serialization.py:357-403`): `get_preferred_payload_format()`
  reads config `payload_format` (default `"pickle"`); `DATA_FORMAT_CBOR=4` →
  `cbor2.dumps((args,kwargs))` (a **tuple**), but only if `cbor2` is installed and the
  function advertises CBOR. A normal function advertises
  `supported_input_formats=[DATA_FORMAT_PICKLE, DATA_FORMAT_CBOR]` (`_functions.py:603`) —
  **we author this**, so we can force CBOR end-to-end and dodge the pickle proto-2/3-vs-4
  gap entirely. **CBOR governs ONLY the arg/result wire, NEVER the function body**
  (the body is always cloudpickle in SERIALIZED mode, or absent in FILE mode).
- gRPC path: `AppCreate` → `ImageGetOrCreate` (+ `ImageJoinStreaming` poll) →
  `FunctionPrecreate` → `FunctionCreate` (`_functions.py:1129-1136`) → invoke via
  `FunctionMap` + poll `FunctionGetOutputs`. Auth from `~/.modal.toml` / `MODAL_TOKEN_*`,
  grpclib/tonic over TLS.

**Decision — function body: use FILE mode (`definition_type=FILE`, `function_serialized=b""`)
with ONE embedded Python wrapper module baked into the image.** This satisfies the
locked direction's "ONE embedded wrapper, not per-project Python" while sidestepping
cloudpickle-proto-4-from-Rust. Args/results use **CBOR** (we authorize it). The locked
phrasing "serialize the wrapper from Rust the way Modal's SDK does" is reinterpreted:
in FILE mode there is nothing to serialize — we *ship the importable module*, which is
strictly simpler and equally "no per-project Python." Rust-emitted cloudpickle proto-4
(true SERIALIZED mode, "no Python file visible anywhere") is demoted to a **version-pinned
stretch goal** validated by the `dump` tool, not the build path.

### B. modal-rs verdict — **USE-AS-IS for invocation + non-GPU authoring; PATCH (small fork) for GPU**

modal-rs 0.1.3 ships a *complete typed authoring builder*, not just a raw hole
(`src/function_authoring.rs`): `FunctionDefinition` with `with_function_serialized`,
`with_image_id`, `with_definition_type({Serialized|File})`, `with_timeout_secs`,
`with_volume_mount(...).with_allow_background_commits(true)`, `with_secret_ids`,
`with_supported_input/output_formats({Pickle|Cbor})`, autoscaler knobs; wired to gRPC via
`FunctionService::{create,precreate,from_name}` (`src/function.rs`). Invocation
`.remote/.spawn/.map/.starmap` is generic over `Serialize`/`DeserializeOwned`, prefers CBOR
when advertised (`function.rs:626-644`). Auth reads `~/.modal.toml`/`MODAL_TOKEN_*`.

Compile-proven (research PROBE B): a non-GPU `FunctionCreate` with FILE-or-SERIALIZED
type + image_id + volume_mounts + CBOR formats compiles and is sendable today.

**The single blocking gap: GPU/CPU/memory is unreachable.** `to_proto_function` leaves
`resources=9` at `..Default::default()` and there is NO `with_gpu/with_resources/with_cpu/
with_memory` (PROBE C: `E0599 no method named with_gpu`). The `inner_mut()` escape hatch is
**dead for authoring** because the proto module is `pub(crate)` (`lib.rs:69`) — external
crates cannot name `api::FunctionCreateRequest`/`api::Resources` (PROBE A: `E0603 module api
is private`). The proto *field exists* (`api.proto` Function `Resources resources=9`,
`GPUConfig gpu_config`), and modal-rs already has `parse_gpu_config` for image-build GPU
(`image.rs:1140`) — so the fix is a **small additive patch to `FunctionDefinition` /
`to_proto_function`** setting `function.resources.gpu_config`, on a fork.

**Verdict matrix:**

| Capability | As-is | Path |
| --- | --- | --- |
| Invoke `.remote/.spawn/.map` (str/JSON/CBOR) | YES | use-as-is |
| Author non-GPU (timeout/volumes/secrets/concurrency) | YES | use-as-is |
| Author with GPU/CPU/memory | NO | **fork + small additive `with_gpu`/`resources` patch** (proto field exists) |
| `function_serialized` content | n/a in FILE mode | ship importable module; FILE mode, no pickle |

**Chosen approach:** vendor/fork modal-rs (it already vendors `api.proto`); use its auth +
channel + image-build + invocation as-is; add the GPU `resources` setter. Build-own-client
from scratch is the maximal-control **fallback**, not required. (If the fork friction is
high, a thin local `tonic` client vendoring only `Function`/`Resources`/`GPUConfig`/
`FunctionInput` messages + reusing modal-rs's auth pattern (`client.rs:49-156`) is
equivalent effort and fully under our control.)

### C. Native Volume bulk-copy answer + chosen cache-on-by-default mechanism

**There is NO native Volume snapshot and NO native "Volume → fast local disk" bulk-copy
primitive in Modal.** Full method set (`volume.py`): `create/list/delete/from_name/ephemeral/
create_deployed/info/commit/reload/iterdir/listdir/read_file/read_file_into_fileobj/
remove_file/copy_files/batch_upload/rename`. `copy_files` is **intra-volume** only
(docstring: "inside the volume"); `batch_upload` is local→volume; `read_file` is per-block
fetch ("primarily … outside of a Modal App"). Proto Volume RPCs contain **no VolumeSnapshot**.
"snapshot" in Modal = *Sandbox* memory snapshot (`snapshot.py`, `_SandboxSnapshot`, early
preview) — unrelated. modal-rs mirrors this (`volume.rs`: no snapshot; `upload/get_file/
copy` are **V2-volume-only** via `ensure_volume_v2`). There is also **no `modal.Cache`
class** and **no build cache** beyond Docker-style image-layer caching (build-time only,
irrelevant to the `run` path which builds at function-execution time).

Modal warns volumes degrade past ~50k files (latency scales linearly with file count) —
so building cargo's `target/`+`CARGO_HOME` (tens of thousands of small files) *directly on a
mounted volume* is the worst case, and a DIY per-file `cp` from the mount pays that same
per-file network latency.

**Chosen cache-on-by-default mechanism — archive-as-single-object on a V2 Volume:**
1. Build on fast local disk: `CARGO_TARGET_DIR=/tmp/target` (already locked in §2.4/M6).
   Never build on the mounted volume.
2. Persist the cache as **ONE compressed archive** (`cache.tar.zst`) on a Volume — N small
   network ops collapse to 1 large sequential read/write. On container start: pull + unpack
   to `/tmp`. On exit: repack changed dirs + write one object; rely on automatic background
   commit (`allow_background_commits=true`), no `vol.reload()` on the hot path.
3. Scope to `CARGO_HOME` (registry/index — high-value, mostly-read) + optionally `target/`.
4. Use a **V2 volume** (concurrent writes; also required for modal-rs's `upload`/`get_file`).
5. Wire dynamically from the Rust registry: `Volume.from_name("modal-rust-cargo-cache",
   create_if_missing=True, version=2)` → `FunctionVolumeMount::new(volume_id,"/cache")
   .with_allow_background_commits(true)` into FunctionCreate. Default ON; `--no-cache` +
   `modal volume rm` reset escape hatch.

(The archive pack/unpack itself can live inside the embedded Python wrapper or be a Rust
step in the wrapper's subprocess — it is a runtime concern, not a Modal authoring concern.)

### D. Local-orchestration Rust API sketch

A normal `#[tokio::main] async fn main()` is the analogue of `@app.local_entrypoint()`
(runs locally, orchestrates Modal). Semantics mirror Modal Python exactly:
`.remote()` = run body remotely (`FunctionMap` → `FunctionGetOutputs`); `.local()` =
run the raw fn **in-process** (Modal Python `Function.local()` = `raw_f(*args)`);
`.spawn()` = fire-and-forget handle; `.map()` = fan-out.

```rust
#[tokio::main]                              // ~ @app.local_entrypoint()
async fn main() -> anyhow::Result<()> {
    let app = modal_rust::App::connect("modal-rust-train").await?;   // reads ~/.modal.toml
    let out:  TrainOut = app.function("train").remote(cfg.clone()).await?; // runs on Modal
    let local: TrainOut = app.function("train").local(cfg.clone())?;        // in-process, no net
    let call = app.function("train").spawn(cfg2).await?;
    let out2: TrainOut = call.get(None).await?;
    let outs: Vec<TrainOut> = app.function("train").map(inputs).await?;
    Ok(())
}

pub struct App { client: ModalClient, app_id: String, registry: Registry /* FROZEN */ }
impl App {
    pub async fn connect(name: &str) -> Result<Self>;   // auth + AppGetOrCreate (ephemeral)
    pub fn function(&self, name: &str) -> Function<'_>;  // looks up name in the inventory Registry
}
pub struct Function<'a> { app:&'a App, name:&'static str, handler:HandlerFn, config:FunctionConfig }
impl<'a> Function<'a> {
    // REMOTE: first call ensures the fn exists on Modal — FunctionCreate(FILE mode, our embedded
    // wrapper module, image_id, resources.gpu_config + timeout + volume_mounts FROM self.config),
    // then invokes via CBOR args. Config is DYNAMIC from the registry — no static parse.
    pub async fn remote<In:Serialize, Out:DeserializeOwned>(&self, input:In) -> Result<Out, RemoteError>;
    pub async fn spawn<In:Serialize>(&self, input:In) -> Result<FunctionCall>;
    pub async fn map<In,Out,I>(&self, inputs:I) -> Result<Vec<Out>>;
    // LOCAL: pure in-process Registry dispatch — NO Modal, NO wire serialization.
    pub fn local<In:Serialize, Out:DeserializeOwned>(&self, input:In) -> Result<Out, RunnerError> {
        let bytes = serde_json::to_vec(&input)?;
        let out   = (self.handler)(&bytes)?;     // HandlerFn from typed!() on the FROZEN Registry
        Ok(serde_json::from_slice(&out)?)
    }
}
```

Key mappings: `.local()` is the cheapest, highest-value piece (zero Modal — identical to
running M0's runner without a subprocess), and it requires **feature-gated CUDA** so the
crate compiles on a dev Mac without CUDA (cudarc `dynamic-loading`; GPU bodies behind
`#[cfg(feature="cuda")]`; also removes the burn-add default-members exclusion). The embedded
wrapper (FILE mode) receives the CBOR/JSON input, writes `/tmp/in.json`, execs
`modal_runner --entrypoint <name> --input-file /tmp/in.json` (the **frozen seam**, §2.2),
returns the one-line JSON envelope string. The runner + inventory registry + macros stay
**unchanged** — this re-architects only the authoring/control layer.

### E. Spike feasibility verdict (+ fallback)

**The executable feasibility spike DID NOT RUN — it was blocked by an infrastructure error
("API Error: The socket connection was closed unexpectedly"), not by a Modal/Rust
limitation.** No live `{"sum":42}` round-trip was produced this pass. The verdict below is a
**static-analysis (paper) feasibility** grounded in primary sources, pending an executable
re-run (see tasks P-spike).

**Paper-feasibility verdict: FEASIBLE.** A Rust control plane can programmatically
create + invoke a Modal function with NO `modal` CLI and NO generated per-project file, via:
- **FILE mode** (`function_serialized=b""` + `module_name`/`function_name`) — verified
  default path; container does `importlib.import_module` + `getattr`
  (`user_code_imports.py:475,488`). One embedded wrapper module baked into the image.
- **CBOR** args/results (we author `supported_input_formats=[…,CBOR]`,
  `_functions.py:603`) — sidesteps the pickle proto-2/3-vs-4 gap entirely.
- **modal-rs as-is** for auth/channel/image-build/precreate/invoke; **forked** only to set
  `resources.gpu_config` (proto field exists at `Function.resources=9`).

Two confirmed risks gate the executable spike: (1) modal-rs's typed `to_proto_function`
guards against empty `function_serialized` unless `existing_function_id`/`allow_sparse_base`
— FILE mode is reachable but must bypass that guard (consistent with Python setting `b""`);
(2) the embedded wrapper must be present + importable on the built image (image-build step
must COPY/add it). Neither is blocking on paper.

**FALLBACK if executable FunctionCreate proves infeasible:** revert AUTHORING to the
**static-shim Option 2** (ONE static parameterized Python shim + typed config-as-data via
`Command` env, on the validated `modal` CLI path) — the runner/registry/macros and the
`.remote()/.local()` *ergonomics* can still be layered on top later. The fallback is
explicitly preserved per task and is a clean revert boundary (it changes only the control
layer, same as the forward plan).

**ONE-LINE FEASIBILITY:** Paper-feasible (FILE mode + CBOR + forked modal-rs for GPU,
all verified against Modal 1.3.2 source); the live spike is still OUTSTANDING (blocked by a
socket/infra error, not a design limit) and must be re-run before the build stages begin,
with the static-shim Option 2 as the recorded fallback.

---

### Cache benchmark — use a realistic HEAVY build, not `add` (2026-06-04, user steer)

The real Modal workloads are **computationally intensive with complex/heavy builds**
(ML/Burn-class, large dependency graphs), NOT minimal build + quick exec like `add`.
So `add` (a 16-crate, sub-second-per-crate graph) is the WORST case to benchmark a
build cache on — its M6b null/negative result is an artifact of that triviality (cache
sync overhead > the tiny recompile). The cache-sync spike MUST measure on a realistic
heavy build:
- Primary: **`example-burn-add`** (burn + cubecl + cudarc — a genuinely heavy graph).
- Optional cheaper proxy: a heavy pure-CPU-deps crate (what stresses a *build* cache is
  crate count + compile time, not GPU), to benchmark without GPU/Tier-1 cost.

Rationale for **cache ON by default**: the common case (heavy builds) is exactly where
warm-cache savings dominate the sync overhead. The rare **no-deps / trivial** workload
just sets `#[modal_rust::app(cache = false)]` and rebuilds — a clean opt-out, no
special-casing. So default-on is correct; the benchmark just has to be run on the case
the default actually targets.

---

### SPIKE VERDICT: FEASIBLE ✅ — programmatic FILE-mode create+invoke from Rust works (2026-06-04)

Rust created a FILE-mode Modal function (module+function name, **empty `function_serialized`**,
wrapper baked into the image via a `run_commands` heredoc — NOT local COPY) and invoked it →
`{"echoed":{"hi":1,"n":42},"ok":true,"source":"spike_wrapper.handler"}`. No cloudpickle, no
`modal` CLI for create/invoke, no per-project `.py`. **The pivot is validated.** Full log:
`workpads/shim-backend/spike-notes.md`; proven Rust recipe: `spike-main.rs.txt`.

**Proven recipe:** AppCreate(ephemeral) → ImageGetOrCreate (`from_registry("python:3-slim")` +
`run_commands` that write `/root/<wrapper>.py` AND **`pip install modal`**; `/root` is on sys.path)
→ FunctionPrecreate → FunctionCreate(FILE, module+fn name, `function_serialized=b""`,
**`with_existing_function_id(precreate_id)`** to bypass the empty-serialized guard via the public
API) → **AppPublish only** (function_ids+definition_ids) → from_name → `.remote((args,))`.

**Image MUST carry the `modal` pip package + the wrapper module** — FILE-mode containers boot via
`python -m modal._container_entrypoint`. (Python stays in the image, invisible to the user.)

**modal-rs 0.1.3 is close but needs 3 fixes (the "infra" failures were really these):**
1. **FunctionCreate** — modal-rs sends BOTH `function` and `function_data`; Modal expects exactly
   one (XOR) and always sets `resources`. (Fork fix.)
2. **Deploy** — modal-rs uses the legacy `AppSetObjects` RPC whose server handler is broken
   (`module 'grpc' has no attribute 'experimental'`); the modern path is **`AppPublish` only**. (Fix.)
3. **Invoke** — modal-rs only sends input as `FunctionMap.pipelined_inputs` and never falls back to
   `FunctionPutInputs` when the response doesn't echo them → input never enqueued → "Function call
   not found". Python falls back to `FunctionPutInputs`. (Fork fix.)
(The empty-`function_serialized` FILE-mode case needs NO fork — just `with_existing_function_id`.)
Scratch (ephemeral): `/tmp/modal-rust-spike` (crate+notes), `/tmp/modal-rs-fork` (patched SDK).

**OPEN DECISION for the build — control-plane client:** (a) maintain a **patched modal-rs fork**
(fastest; proven; but depends on an unmaintained pre-1.0 crate with a server-incompatible RPC), vs
(b) **our own thin tonic client** over Modal's vendored proto, implementing exactly what the Python
SDK does (the spike already reverse-engineered the 3 deltas) — clean, fully controlled, matches the
user's "do it the way their SDK does", more upfront work. Recommendation: (b) for the durable
foundation; optionally bootstrap from the fork to keep momentum.
