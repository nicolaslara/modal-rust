# queue-pipeline

A producer/consumer pipeline through a named **`modal.Queue`**: the producer
(the caller) `put_many`s jobs — plain numbers — into
`Queue::from_name("queue-pipeline-jobs")`; a `#[modal_rust::function]` consumer
drains it with **blocking `get(timeout)`** and computes each job's Collatz
stopping time, returning a typed summary:

```text
producer ──put_many──▶ Queue "queue-pipeline-jobs" ──get(timeout)──▶ drain_jobs()
producer ◀──────────────────── DrainSummary ─────────────.remote()──┘
```

The timeout convention (Python's `get(block=True, timeout=..)` without the
boolean): `None` blocks forever, `Some(d)` waits ~`d` then yields `None`,
`Some(Duration::ZERO)` is one non-blocking poll. The consumer uses `Some(d)` as
an IDLE timeout — "stop once the queue stays empty for `d`" — the standard way
to drain a finite batch. Blocking gets never ride one gRPC deadline: the SDK
polls with per-RPC caps, mirroring the Python client.

Queue handles are orchestration (they open a gRPC client), so this lib carries
`modal-rust` with `features = ["client"]` in `[dependencies]` — same as
`examples/orchestrate`, unlike the pure decorator examples.

## Run it

The consumer (`drain_jobs`) and the producer (`put_many`) are two halves: the
consumer is the `#[function]`, the producer is caller-side code. So running
`drain_jobs` against an **empty** queue is well-defined but returns zeros
(`{"jobs":0,...}`) — it blocks `idle_ms`, finds nothing, and exits. Populate the
queue first, then drain it:

```bash
cd examples/queue-pipeline
cargo run -p example-queue-pipeline --bin produce       # 1. enqueue [27, 6, 97, 9]
modal-rust run drain_jobs --input '{"idle_ms":2000}'    # 2. drain the named Queue
```

Expected output from step 2 (the consumer drains the four jobs FIFO):

```json
{"ok":true,"value":{"jobs":4,"total_steps":256,"max_steps":118}}
```

The producer shares **nothing** with the consumer but the name
`queue-pipeline-jobs`, so any process can populate it — a Python `q.put_many([27,
6, 97, 9])` works too (see the interop note). Drain within the idle window: a
populated queue drains immediately.

To see the **full producer + consumer pipeline in one process** (produce here,
drain in a container, then delete the demo Queue), use the driver instead:

```bash
RUN_REMOTE=1 cargo run -p example-queue-pipeline --bin queue_pipeline   # live pipeline
cargo run -p example-queue-pipeline --bin queue_pipeline                # offline: local stopping times
cargo test -p example-queue-pipeline                                    # offline FIFO + idle timeout vs mock
```

Offline driver output:

```
local stopping times (what drain_jobs computes per job):
  local: 27 -> 111 steps
  local: 6 -> 8 steps
  local: 97 -> 118 steps
  local: 9 -> 19 steps
local: 4 jobs, 256 total steps
(skipping live produce + remote drain — set RUN_REMOTE=1 with Modal credentials to run it)
```

## The Python interop boundary (by design)

Queue items ride the wire as restricted pickle, matching Modal's own Go/JS
clients — so **plain data interops with Python**: a Python producer can feed
this consumer with just `q.put(27)`, and values round-trip for str/int/float/
bool/bytes/lists/dicts/structs-as-dicts (a Rust struct reads as a Python dict).

Pickled **Python custom classes/functions do NOT interop**: reading one from
Rust fails with a typed codec error — never a panic, never a silent `None`.
`get_raw`/`put_raw` are the bring-your-own-codec escape hatch.

v0 surface notes: named Queues only (`from_name`/`lookup`/`from_name_in`/
`delete`); the default partition only (partition keys/TTL knobs, ephemeral
queues, and non-destructive `iterate()` are deferred). Limits: 5,000 items per
partition, 1 MiB per item.
