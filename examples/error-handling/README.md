# error-handling

How a failure crosses the Modal boundary. `withdraw` returns a plain `anyhow`
error (opaque: `details: null`); `withdraw_checked` returns a structured
`Serialize` error that arrives as machine-readable `details` the caller can
deserialize and branch on. Same frozen `function_error` kind, different
`details`.

## Run it

A successful withdrawal:

```bash
cd examples/error-handling
modal-rust run withdraw --input '{"amount":30,"balance":100}'
```

Expected output:

```json
{"ok":true,"value":{"withdrawn":30,"remaining":70}}
```

An overdraw showing the opaque `anyhow` error path (`details: null`):

```bash
modal-rust run withdraw --input '{"amount":150,"balance":100}'
```

```json
{"ok":false,"error":{"kind":"function_error","message":"insufficient funds: asked 150, have 100","details":null}}
```

The structured variant — same overdraw, but `details` carries the typed error
the caller can branch on:

```bash
modal-rust run withdraw_checked --input '{"amount":200,"balance":100}'
```

```json
{"ok":false,"error":{"kind":"function_error","message":"...","details":{"code":"insufficient_funds","shortfall":100}}}
```

To tour the full opaque-vs-structured comparison locally (no Modal credentials
needed for the offline path):

```bash
cargo run -p example-error-handling --bin error_handling
```

## Prereqs

Modal credentials configured (`modal token new`). Run `modal-rust doctor` to
check your environment first.
