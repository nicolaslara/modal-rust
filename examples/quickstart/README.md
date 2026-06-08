# quickstart

The whole pitch in one screen: a plain `fn add(a, b) -> Result<i64>` becomes a
Modal function with a single `#[modal_rust::function]` attribute. No Dockerfile,
no struct, no runner bin — the macro infers the I/O types from the signature and
the CLI generates the runner automatically.

## Run it

```bash
cd examples/quickstart
modal-rust run add --input '{"a":2,"b":3}'
```

Expected output:

```json
{"ok":true,"value":5}
```

## Prereqs

Modal credentials configured (`modal token new`). Run `modal-rust doctor` to check your toolchain and Modal auth before the first run.
