# modal-rust examples

Every directory here is a **real, compiled, tested crate** — written exactly as a
downstream user would write it. They are the live reference for the `modal-rust`
surface: each demonstrates one concept, ships its own `README.md` with the exact
run command and expected output, and is covered by tests so a stale README is a
test failure.

How they run:

- Most examples are **pure library crates** — just a `modal-rust` dependency and
  one or more `#[modal_rust::function]` fns. They have no runner binary; the
  `modal-rust` CLI generates one automatically. You run them with
  `modal-rust run / deploy / call <entrypoint>`.
- A few ship a **driver binary** (`src/bin/<name>.rs` with a `main()`) that tours
  the facade API offline — those are run with `cargo run -p <crate> --bin <name>`.
  The live (`.remote()`/`deploy`/`call`) paths in those drivers are gated behind
  `RUN_REMOTE=1` + Modal credentials.
- `examples/add` is the only one you invoke by hand (it ships its own
  `add-runner` bin); it is the low-level internals reference, not how a typical
  user writes code.

The two **GPU/CUDA examples** (`cuda-vector-add`, `burn-add`) are excluded from
`default-members`, so a bare `cargo build`/`test`/`clippy` and CI skip them — they
only compile against a CUDA toolkit (present on Modal, absent on a typical dev
box). `burn-add` is additionally a full workspace member only (not in the default
set at all). Build them on a CUDA host or on Modal.

Before any live run: `modal token new` (credentials) and `modal-rust doctor
--rust` (preflight). The full Rust-vs-Python parity inventory is in
[`docs/PARITY.md`](../docs/PARITY.md); the caveats below are drawn from it.

## Index

| Example | Concept | Primary command |
|---|---|---|
| [quickstart](#quickstart) | plain `fn` → Modal function, 3 lines | `modal-rust run` |
| [add](#add) | hand-built registry (no macro), internals reference | `cargo run --bin add-runner` |
| [add-macro](#add-macro) | macro twin of `add` + full decorator tour | `modal-rust run` |
| [custom-types](#custom-types) | your own structs as I/O | `modal-rust run` |
| [orchestrate](#orchestrate) | the `App`/`Function` facade tour | `cargo run --bin orchestrate` |
| [ways-to-call](#ways-to-call) | `.local()`/`.remote()`/`.spawn()`/`.map()` | `modal-rust run` |
| [fan-out-map](#fan-out-map) | `.map()` fan-out | `modal-rust run` |
| [background-jobs](#background-jobs) | `.spawn()` + `.get(timeout)` | `modal-rust run` |
| [spawn-map-foreach](#spawn-map-foreach) | `.spawn_map()` / `.for_each()` | `modal-rust run` |
| [error-handling](#error-handling) | opaque vs structured errors across the boundary | `modal-rust run` |
| [secrets](#secrets) | `secrets = [..]` decorator field | `modal-rust run` |
| [volumes](#volumes) | `volumes = [..]` persistent storage | `modal-rust run` |
| [timeout-and-cache](#timeout-and-cache) | `timeout` / `cache` knobs | `modal-rust run` |
| [cpu-memory](#cpu-memory) | `cpu` / `memory` knobs | `modal-rust run` |
| [retries](#retries) | `retries = N` policy | `modal-rust run` |
| [autoscaling](#autoscaling) | `min`/`max`/`buffer`/`scaledown` | `modal-rust run` |
| [scheduled-job](#scheduled-job) | `schedule = Cron(..)` deployed cron | `modal-rust deploy` |
| [stateful-class](#stateful-class) | `#[cls]` load-once-serve-many | `modal-rust run` |
| [snapshot-class](#snapshot-class) | `#[cls(enable_memory_snapshot)]` | `modal-rust deploy` |
| [web-endpoint](#web-endpoint) | `#[endpoint]` HTTP function | `modal-rust deploy` + `curl` |
| [custom-base](#custom-base) | choose RUN base image / install Rust | `modal-rust run` |
| [pip-apt-image](#pip-apt-image) | `apt`/`pip`/`run` image steps | `modal-rust run` |
| [deploy-and-call](#deploy-and-call) | the run-vs-deploy build boundary | `modal-rust deploy` + `call` |
| [cli-workflow](#cli-workflow) | drive a crate from the CLI end-to-end | `modal-rust run`/`deploy`/`call` |
| [dict-kv](#dict-kv) | shared state via `modal.Dict` | `modal-rust run` |
| [queue-pipeline](#queue-pipeline) | producer/consumer via `modal.Queue` | `modal-rust run` |
| [cuda-vector-add](#cuda-vector-add) | GPU CUDA kernel (cudarc) | `modal-rust deploy` + `call` |
| [burn-add](#burn-add) | GPU ML tensor add (Burn/CubeCL) | `modal-rust deploy` + `call` |

---

## Core / getting started

The newcomer surface: a plain function becomes a Modal function, with your own
types, called every which way.

### quickstart

A plain `fn add(a, b) -> Result<i64>` becomes a Modal function with a single
`#[modal_rust::function]` — no Dockerfile, no struct, no runner bin. The macro
infers the I/O from the signature and the CLI generates the runner.

```bash
cd examples/quickstart
modal-rust run add --input '{"a":2,"b":3}'   # => {"ok":true,"value":5}
```

Caveat: there is no `modal.App` Python object to import — you do not write a
Python entrypoint at all; the Rust CLI builds and runs the function for you.

### add

The same `add` written **by hand**: the input/output structs, the `typed!(fn)`
registration, and the `modal_registry()` builder — i.e. everything the macro
generates. It depends on `modal-rust-runtime` directly (no facade) and ships its
own `add-runner` bin, so the CLI cannot generate a runner for it. It also defines
entrypoints exercising every runner error kind (`fail`, `fail_structured`,
`bad_encode`, `will_panic`, `gpu_info`).

```bash
cd examples/add
cargo run --bin add-runner -- --entrypoint add --input-json '{"a":40,"b":2}'
```

Caveat: this is the **internals reference**, not the normal authoring path — a
real user writes `quickstart`/`add-macro`. It runs the local runner binary
directly with no Modal call.

### add-macro

The macro path twin of `add`: the same function in three lines, plus a full
decorator-config tour (`gpu`/`timeout`/`cache`/`secrets`/`volumes`) in
`proof.rs`. Proves the macro produces the same registry/runner shape as the
manual `typed!` path.

```bash
cd examples/add-macro
modal-rust run add --input '{"a":2,"b":3}'   # => {"ok":true,"value":5}
```

### custom-types

A `#[function]` takes and returns **your own structs** — derive
`Serialize`/`Deserialize` and the macro infers the wire I/O from the signature.
Here `score(Player) -> Scored` turns a match record into a score.

```bash
cd examples/custom-types
modal-rust run score --input '{"name":"ada","hits":7,"shots":10}'
```

### orchestrate

A runnable tour of the `App`/`Function` facade: offline `.local()` (zero-Modal)
plus the live `.remote()` and `deploy`+`call` round-trips. Reuses `add` from
`add`/`add-macro` to show the manual registry and macro registry are equivalent.
Driven with `cargo`, not the CLI.

```bash
cd examples/orchestrate
cargo run --bin orchestrate                  # offline path
RUN_REMOTE=1 cargo run --bin orchestrate     # + live remote/deploy/call
```

### ways-to-call

One function (`square(n)`), four invocation patterns side by side: `.local()`,
`.remote().await`, `.spawn()` + `.get()`, and `.map([..])` — all through the same
typed `app.square(n)` method, no I/O type named at the call site.

```bash
cd examples/ways-to-call
modal-rust run square --input '{"n":6}'      # single remote call => 36
cargo run -p example-ways-to-call --bin ways_to_call   # offline tour
```

## Fan-out / async invocation

How to scale one function across many inputs and run work in the background.

### fan-out-map

Embarrassingly-parallel scale-out with `.map()`: one `#[function]` mapped over N
inputs returns `Vec<Out>` in input order. `analyze` reads a document and returns
word count + reading time.

```bash
cd examples/fan-out-map
modal-rust run analyze --input '{"title":"doc","body":"the quick brown fox"}'
cargo run -p example-fan-out-map --bin fan_out_map   # offline tour
```

### background-jobs

Fire-and-forget with `.spawn()`: enqueue a job, get a handle immediately, do
other work, then collect with `.get(timeout)`. Contrasts with `.remote()`, which
blocks until the result is ready.

```bash
cd examples/background-jobs
modal-rust run run_job --input '{"label":"nightly","rounds":1000}'
```

### spawn-map-foreach

The rest of the map family: `.spawn_map()` (fire-and-forget fan-out) and
`.for_each()` (side-effect map that waits and discards results). `notify` sends a
notification and returns a receipt.

```bash
cd examples/spawn-map-foreach
modal-rust run notify --input '{"name":"ada","channel":"email"}'
cargo run -p example-spawn-map-foreach --bin spawn_map_foreach   # offline tour
```

Caveat: `starmap` is **single-arg-framed** in v0 — each mapped item is the one
named-object input; a true multi-positional-arg `starmap` is not yet exposed.

## Config & decorators

Operational knobs set on the `#[function(..)]` decorator — none of them touch the
function body.

### error-handling

How a failure crosses the Modal boundary. `withdraw` returns a plain `anyhow`
error (opaque: `details: null`); `withdraw_checked` returns a structured
`Serialize` error that arrives as machine-readable `details` the caller can
deserialize and branch on. Same frozen `function_error` kind, different
`details`.

```bash
cd examples/error-handling
modal-rust run withdraw --input '{"amount":150,"balance":100}'
modal-rust run withdraw_checked --input '{"amount":200,"balance":100}'
cargo run -p example-error-handling --bin error_handling   # offline comparison
```

Caveat: the error envelope is a frozen **five-kind** enum
(`decode_error`/`unknown_entrypoint`/`function_error`/`encode_error`/`panic`) —
not Python's open exception model. Structured `details` only survive if your error
type derives `Serialize`.

### secrets

Attach a named Modal secret on the decorator (`secrets = ["my-api-key"]`) and
read it as an env var inside the function. `check_secret` reports whether
`MY_API_KEY` was injected and its length (never the value).

```bash
cd examples/secrets
modal-rust run check_secret --input '{}'
```

Caveat: needs a pre-existing Modal secret
(`modal secret create my-api-key MY_API_KEY=...`). Inline/dict secrets defined in
code (Python's `Secret.from_dict(...)`) are referenced **by name** here, not built
inline.

### volumes

Mount a Volume on the decorator (`volumes = ["/data=my-vol"]`), write a file, and
read it back on the next call — persistent storage across invocations.
`record_visit` appends to a log and returns the running count.

```bash
cd examples/volumes
modal-rust run record_visit --input '{"label":"first"}'
```

Caveat: the Volume (`modal volume create my-vol`) must exist first. As in Python,
writes are committed on container exit; concurrent writers need the usual Volume
care.

### timeout-and-cache

Set a function `timeout` and use the on-by-default cargo build cache for fast
rebuilds (`timeout = 1800, cache = true`). `spin` runs a checksum fold.

```bash
cd examples/timeout-and-cache
modal-rust run spin --input '{"iterations":100000}'
```

Caveat: `cache` here is the **Rust build cache** (a modal-rust concept that speeds
up the in-body `cargo build` on the run path); it has no Python equivalent.

### cpu-memory

Request CPU cores and RAM on the decorator (`cpu = 2.0, memory = 4096`). `crunch`
folds over a batch of records. The knobs size the container without touching the
body.

```bash
cd examples/cpu-memory
modal-rust run crunch --input '{"records":100000}'
```

Caveat: set `memory` high enough for any **in-body `cargo` build** on the run path
— a heavy compile on a default container can be OOM-killed (surfaces as
`GENERIC_STATUS_TERMINATED` with no error output).

### retries

Make a flaky function self-heal with a retry policy (`retries = 5`). `fetch`
simulates a downstream that fails twice then succeeds; the policy drives it to
success with no retry loop in your code.

```bash
cd examples/retries
modal-rust run fetch --input '{"resource":"db","attempt":3}'
```

Caveat: the `attempt` field is a demo control to pick which simulated attempt
runs — on a real deploy you do not pass it; Modal re-runs the whole call.

### autoscaling

Control warm capacity and scale-to-zero via the autoscaling decorator fields
(`min`/`max`/`buffer_containers`, `scaledown_window`). `embed` turns a document
into an L2-normalized feature vector.

```bash
cd examples/autoscaling
modal-rust run embed --input '{"text":"the quick brown fox"}'
```

Caveat: only **static** autoscaler config is supported; the live
`update_autoscaler` RPC (changing limits on a running app) is not yet available.

### scheduled-job

A deployed function that runs on a cron schedule with no caller
(`schedule = Cron("0 9 * * 1")`). `weekly_report` rolls events into per-source
totals. **Deploy is the primary command** — it registers the schedule.

```bash
cd examples/scheduled-job
modal-rust deploy weekly_report --app modal-rust-scheduled-job
# invoke the body directly for testing:
modal-rust call weekly_report --app modal-rust-scheduled-job --input '{...}'
modal-rust run  weekly_report --input '{...}'   # ephemeral, no schedule
```

Caveat: the schedule only fires once **deployed**; `run`/`call` invoke the body
on demand but do not register the cron.

## Images & the build boundary

Where the build happens and what base image carries it.

### custom-base

Pick the RUN base image and install the Rust toolchain via build-config knobs
(`RemoteConfig.base_image` / `.install_rust`, or `MODAL_RUST_BASE_IMAGE` /
`MODAL_RUST_INSTALL_RUST`). `probe` checksums an input so you can confirm which
image ran.

```bash
cd examples/custom-base
MODAL_RUST_BASE_IMAGE=python:3.12-slim MODAL_RUST_INSTALL_RUST=1 \
  modal-rust run probe --input '{"value":42}'
cargo run -p example-custom-base --bin custom_base   # offline: prints rendered Dockerfile
```

### pip-apt-image

The image-builder steps API: add system packages, Python packages, and shell
commands via `RemoteConfig::image_steps` (`ImageStep::apt`/`pip`/`run`),
mirroring Python's `Image.apt_install`/`.pip_install`/`.run_commands`.

```bash
cd examples/pip-apt-image
modal-rust run render --input '{"value":7}'
cargo run -p example-pip-apt-image --bin pip_apt_image   # offline: prints rendered steps
```

Caveat: image steps are set through the Rust `RemoteConfig` builder, not a
chained Python `Image` object — same rendered Dockerfile, different authoring
surface.

### deploy-and-call

The run-vs-deploy build boundary made explicit: `.remote()` uploads source and
runs `cargo build` **in the function body** on every cold start; `deploy` runs
`cargo build --release` **once** at image-build time and bakes the binary in, so
each `call` invokes the prebuilt binary with no rebuild.

```bash
cd examples/deploy-and-call
modal-rust run  fib --input '{"n":10}'                          # builds in-body
modal-rust deploy fib --app deploy-and-call                     # builds once
modal-rust call   fib --app deploy-and-call --input '{"n":10}'  # no rebuild
cargo run -p example-deploy-and-call --bin deploy_and_call      # offline proof
```

Caveat: this build split is **the** modal-rust invariant and has no Python
analogue — Python ships source/bytecode, not a compiled binary, so there is no
"build at deploy time vs build at call time" distinction.

### cli-workflow

Drive your crate from the `modal-rust` CLI end to end — `doctor` (preflight),
`run`, `deploy`, `call` — with no driver binary. `summarize` returns word/char
counts and a read-time estimate.

```bash
cd examples/cli-workflow
modal-rust doctor --rust
modal-rust run summarize --input '{"text":"the quick brown fox"}'
modal-rust deploy summarize --app cli-workflow
modal-rust call   summarize --app cli-workflow --input '{"text":"hello world"}'
```

## Cls / stateful

Load-once-serve-many: an expensive resource loaded once, reused across calls.

### stateful-class

`#[modal_rust::cls]`: an `#[enter]` method loads an expensive resource once per
warm container; each `#[method]` reuses it by `&self`. Every method becomes its
own dotted entrypoint (`Embedder.embed`, `Embedder.dim`) carrying merged
class + method config. Directly runnable — no deploy required, and GPU on the
decorator does not force a deploy.

```bash
cd examples/stateful-class
modal-rust run Embedder.dim   --input '{}'
modal-rust run Embedder.embed --input '{"text":"hello"}'
```

Caveat: `#[cls]` is **Shape A only** in v0 — `#[enter]` + `#[method]`. No
`#[exit]` finalizer and no class parameters (`modal.parameter`) yet.

### snapshot-class

`#[modal_rust::cls(enable_memory_snapshot = true)]`: pay an expensive `#[enter]`
build **once, ever**. A deployed app runs `#[enter]` once, Modal snapshots the
loaded process, and every later container — cold ones included — restores the
built state instead of re-running the build.

```bash
cd examples/snapshot-class
modal-rust deploy Concordance.search --app modal-rust-snapshot-class
modal-rust call   Concordance.search --app modal-rust-snapshot-class --input '{"prefix":"wa"}'
modal-rust run    Concordance.search --input '{"prefix":"wa"}'   # plain #[cls], no snapshot
```

Caveats: the snapshot is **deploy-only** (RUN stays wire-identical to a plain
`#[cls]`). v0 is **CPU-only** — the GPU snap/restore split (load on CPU in the
snapshot window, move to GPU after restore) is not yet implemented. A failed
`#[enter]` prime fails container init **loudly** by default
(`MODAL_RUST_SNAPSHOT_BEST_EFFORT=1` opts into degrading to lazy load). And as in
Python, anything `#[enter]` captures (env vars, wall clock, RNG seeds, open
connections) is **frozen** into the snapshot across all restores.

## Web endpoints

### web-endpoint

Expose a plain function over HTTP with one attribute,
`#[modal_rust::endpoint(method = "POST")]`. An endpoint is a normal
`#[function]`-shaped handler (same auto-IO, same decorators, same typed
`app.summarize(..)` surface) that **also** gets a public URL when deployed —
Modal wraps it in a FastAPI app and POSTs the input JSON as the body. No
web-framework dependency in your crate.

```bash
cd examples/web-endpoint
modal-rust deploy summarize --app modal-rust-web-endpoint
curl -X POST "https://<workspace>--modal-rust-web-endpoint-summarize.modal.run" \
  -H 'content-type: application/json' \
  -d '{"text":"...","max_sentences":2}'
modal-rust run summarize --input '{"text":"...","max_sentences":1}'   # typed, no URL
```

Caveats (DEPLOY-ONLY HTTP, v0):

- The URL is **assigned on deploy only** — there is no `modal serve`-style
  ephemeral dev URL. `modal-rust run` stays wire-identical to a plain
  `#[function]` and exposes **no URL**.
- The **deployed** endpoint is **HTTP-only**: the typed envelope path
  (`.remote()` / `modal-rust call`) against the deployed app is rejected (Modal's
  worker ASGI-wraps the callable). Need both surfaces? Use Modal's own idiom — a
  plain `#[function]` plus a thin `#[endpoint]` that calls it.
- Public by default (matching Modal); opt into auth with
  `requires_proxy_auth = true`.
- v0 is one method, one request/response per fn. Routing, multiple methods,
  streaming, websockets, `@asgi_app`/`@wsgi_app`/`@web_server`, and custom domains
  are not yet available.

## Data primitives (Dict / Queue)

Named distributed objects with a deliberate Python-interop boundary.

### dict-kv

Shared state through a named `modal.Dict`: a `#[function]` writes Scrabble scores
into `Dict::from_name("dict-kv-scores")`, and a separate caller process opens the
same Dict by name and reads them back typed. Carries `features = ["client"]`
because Dict handles open a gRPC client.

```bash
cd examples/dict-kv
modal-rust run record_scores --input '{"words":["jazz","quartz","modal","rust"]}'   # => 4 entries written
RUN_REMOTE=1 cargo run -p example-dict-kv --bin dict_kv   # full writer+reader round-trip
cargo run -p example-dict-kv --bin dict_kv               # offline: local scores
cargo test -p example-dict-kv                            # offline write→read vs mock
```

Caveats: **Python interop is by design but partial** — keys/values ride as
restricted pickle, so plain data (str/int/float/bool/bytes/lists/dicts/structs)
round-trips with Python (a Rust struct reads as a Python dict), but a pickled
Python **custom class/function does NOT interop** (typed codec error, never a
panic or silent `None`; `get_raw`/`put_raw` are the escape hatch). v0 surface is
**named Dicts only** — partitions are n/a, and TTL knobs, ephemeral dicts, and
`keys()/values()/items()` iteration are deferred. Entries expire after 7 days of
inactivity; `len()` is expensive and caps at 100,000.

### queue-pipeline

A producer/consumer pipeline through a named `modal.Queue`: the caller
`put_many`s jobs into `Queue::from_name("queue-pipeline-jobs")` and a `#[function]`
consumer drains it with **blocking `get(timeout)`**, computing each job's Collatz
stopping time. Also carries `features = ["client"]`.

```bash
cd examples/queue-pipeline
modal-rust run drain_jobs --input '{"idle_ms":2000}'           # consume an already-populated queue
RUN_REMOTE=1 cargo run -p example-queue-pipeline --bin queue_pipeline   # full producer+consumer pipeline
cargo run -p example-queue-pipeline --bin queue_pipeline       # offline: local stopping times
cargo test -p example-queue-pipeline                           # offline FIFO + idle timeout
```

Caveats: the `get(timeout)` convention mirrors Python's
`get(block=True, timeout=..)` **without** the boolean — `None` blocks forever,
`Some(d)` waits then yields `None`, `Some(ZERO)` is one non-blocking poll. Same
restricted-pickle interop boundary as Dict (custom Python classes do not
interop). v0 is **named Queues, default partition only** — partition keys/TTL,
ephemeral queues, and non-destructive `iterate()` are deferred. Limits: 5,000
items per partition, 1 MiB per item.

## GPU (CUDA-only — excluded from default builds)

Real GPU workloads. Both are excluded from `default-members`; build on a CUDA
host or on Modal. `deploy` is recommended for both — not because GPU forces it,
but because the heavy in-body `cargo` build on the `run` path is large enough to
risk OOM on a default container.

### cuda-vector-add

A real GPU kernel: the `cudarc` Driver API + a precompiled PTX kernel running an
element-wise vector add on a T4, verified against a CPU reference. Authored with
`#[function(gpu = "T4", name = "vector_add", memory = 8192)]`.

```bash
cd examples/cuda-vector-add
MODAL_RUST_BASE_IMAGE=nvidia/cuda:12.6.3-devel-ubuntu22.04 MODAL_RUST_INSTALL_RUST=1 \
  modal-rust deploy vector_add --app cuda-vector-add
modal-rust call vector_add --app cuda-vector-add --input '{"n":1024}'
```

Caveat: a GPU `run` is supported (set a CUDA-devel base + enough `memory`), but
the README documents `deploy` as the repeatable path rather than asserting a
specific verified `run` command — the heavy in-body build is OOM-prone on `run`.

### burn-add

A real ML workload: a Burn/CubeCL tensor add on the CUDA backend (kernels
JIT-compiled via NVRTC at runtime) on a T4, verified against a CPU reference.
Authored with the full decorator including a per-function CUDA-devel `image =
Image(..)`. The only example that is a workspace member but entirely outside the
default set.

```bash
cd examples/burn-add
MODAL_RUST_BASE_IMAGE=nvidia/cuda:12.6.3-devel-ubuntu22.04 MODAL_RUST_INSTALL_RUST=1 \
  modal-rust deploy burn_add --app modal-rust-burn-add-example
modal-rust call burn_add --app modal-rust-burn-add-example --input '{"n":256}'
```

Caveat: a GPU `run` is verified to work here (with the `image` decorator above),
but needs `memory = 16384` — `8192` OOMs the heavy CubeCL release build
(`GENERIC_STATUS_TERMINATED`). `deploy` is recommended to move that build to
image-build time once. If a `run` is killed, the build log lives in
`modal app logs` (it is lost client-side when the container is killed).

---

## Bring-your-own-runner

### own-runner-bin

The escape hatch: a macro crate that ships its **own** one-line
`src/bin/modal_runner.rs` (`modal_rust::modal_runner!(crate);`) for wrapping
startup. The CLI auto-detects this bin (via `cargo metadata`) and uses it as-is
instead of generating one, so `modal-rust run` works normally. `extract_metrics`
aggregates a batch of log lines.

```bash
cd examples/own-runner-bin
modal-rust run extract_metrics --input '{"lines":["INFO source=web ok","ERROR source=db fail","INFO source=web ok"]}'
```

Caveat: this is the **only** workspace crate that ships a bin named
`modal_runner`. Every other example relies on the CLI generating one — you only
need your own bin if you want custom startup logic around the runner.
