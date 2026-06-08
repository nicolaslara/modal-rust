# cpu-memory

Right-size compute: request CPU cores and RAM on the decorator
(`#[function(cpu = 2.0, memory = 4096)]`). `crunch` folds over a batch of
records and returns a deterministic checksum — the resource knobs size the
container (`cpu = 2.0` -> 2000 milli-cores, `memory = 4096` -> 4 GiB) without
touching the function body at all.

## Run it

```bash
cd examples/cpu-memory
modal-rust run crunch --input '{"records":100000}'
```

Expected output (`checksum` is a deterministic function of `records`):

```json
{"ok":true,"value":{"records":100000,"checksum":<u64>}}
```

Note: `crunch` requires `--input` with a `records` field. The CLI validates the
input shape locally and fails fast (without calling Modal) if it does not match.

## Prereqs

Modal credentials configured (`modal token new`). Run `modal-rust doctor --rust`
to check your environment first.
