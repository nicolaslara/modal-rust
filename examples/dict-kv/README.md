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

```bash
# Offline (default): the honest computation — score each word locally and show
# the entries the live path would write. No Modal, no credentials.
cargo run -p example-dict-kv --bin dict_kv

# The offline write→read round-trip against the in-process mock backend:
cargo test -p example-dict-kv
```

Expected offline output:

```
local scores (what record_scores writes to the Dict):
  local: jazz -> 29
  local: quartz -> 24
  local: modal -> 8
  local: rust -> 4
(skipping live function-writes/caller-reads — set RUN_REMOTE=1 with Modal credentials to run it)
```

With Modal credentials, run the real shared-state round-trip — the function
writes in a container, this process reads back, then deletes the demo Dict:

```bash
RUN_REMOTE=1 cargo run -p example-dict-kv --bin dict_kv
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
