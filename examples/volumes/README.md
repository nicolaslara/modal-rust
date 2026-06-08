# volumes

Mount a Volume on the decorator (`#[function(volumes = ["/data=my-vol"])]`),
write a file, and read it back on the next call — persistent storage across
invocations. `record_visit` appends a label to a log on the volume and returns
the running count.

## Run it

```bash
cd examples/volumes
modal-rust run record_visit --input '{"label":"first"}'
```

Expected output (`count` grows by one on each subsequent call against the same
volume — a value > 1 proves the previous write survived):

```json
{"ok":true,"value":{"count":1,"recorded":"first"}}
```

## Prereqs

Modal credentials configured (`modal token new`), and a Modal Volume named
`my-vol` (`modal volume create my-vol`). Run `modal-rust doctor --rust` to check
your environment first.
