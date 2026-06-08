# timeout-and-cache

Operational knobs on the decorator: set a function `timeout` and use the
on-by-default cargo build cache for fast rebuilds
(`#[function(timeout = 1800, cache = true)]`). `spin` runs `iterations` of a
checksum fold and returns a deterministic result.

## Run it

```bash
cd examples/timeout-and-cache
modal-rust run spin --input '{"iterations":100000}'
```

Expected output (`checksum` is a pure function of `iterations`):

```json
{"ok":true,"value":{"iterations":100000,"checksum":<u64>}}
```

## Prereqs

Modal credentials configured (`modal token new`). Run `modal-rust doctor --rust`
to check your environment first.
