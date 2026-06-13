# dict-kv

Shared state through a named **`modal.Dict`**: a `#[modal_rust::function]`
computes Scrabble word scores and writes them into
`Dict::from_name("dict-kv-scores")`; the CALLER — a different process that
shares nothing with the container but the NAME — opens the same Dict and reads
every score back typed:

```text
caller ──.remote()──▶ record_scores() ──put──▶ Dict "dict-kv-scores"
caller ◀────────────────────────────────get──┘   (read back typed)
```

Dict handles are orchestration (they open a gRPC client), so this lib carries
`modal-rust` with `features = ["client"]` in `[dependencies]` — same as
`examples/orchestrate`, unlike the pure decorator examples.

## Run it

Invoke the writer with the `modal-rust` CLI — `record_scores` scores each word
and `put`s `word -> score` into the named Dict, returning the entry count:

```bash
cd examples/dict-kv
modal-rust run record_scores --input '{"words":["jazz","quartz","modal","rust"]}'
```

Expected output (four entries written, now live in `Dict::from_name("dict-kv-scores")`):

```json
{"ok":true,"value":4}
```

A Python (or any) reader can then open the same Dict by name and read the scores
back (see the interop note below) — that is the shared-state point.

To see the **full writer + reader round-trip in one process** (function writes
in a container, this process reads back, then deletes the demo Dict), use the
driver:

```bash
RUN_REMOTE=1 cargo run -p example-dict-kv --bin dict_kv   # live round-trip
cargo run -p example-dict-kv --bin dict_kv                # offline: local scores only
cargo test -p example-dict-kv                             # offline write→read vs mock backend
```

Offline driver output:

```
local scores (what record_scores writes to the Dict):
  local: jazz -> 29
  local: quartz -> 24
  local: modal -> 8
  local: rust -> 4
(skipping live function-writes/caller-reads — set RUN_REMOTE=1 with Modal credentials to run it)
```

## The Python interop boundary (by design)

Dict keys and values ride the wire as restricted pickle, matching Modal's own
Go/JS clients — so **plain data interops with Python**: keys are strings, and
values round-trip for str/int/float/bool/bytes/lists/dicts/structs-as-dicts
(a Rust struct reads as a Python dict). A Python reader sees this example's
entries as ordinary `str -> int`:

```python
import modal
d = modal.Dict.from_name("dict-kv-scores")
print(d["jazz"])  # 29
```

Pickled **Python custom classes/functions do NOT interop**: reading one from
Rust fails with a typed codec error — never a panic, never a silent `None`.
`get_raw`/`put_raw` are the bring-your-own-codec escape hatch.

v0 surface notes: named Dicts only (`from_name`/`lookup`/`from_name_in`/
`delete`); partitions n/a, TTL knobs, ephemeral dicts, and `keys()/values()/
items()` iteration are deferred. Entries expire after 7 days of inactivity;
`len()` is expensive and caps at 100,000.
