# spawn-map-foreach

The rest of the map family: `.spawn_map()` (fire-and-forget fan-out) and
`.for_each()` (side-effect map that waits and discards results). `notify` sends
a notification to a recipient and returns a receipt — each invocation is an
independent unit of work that is textbook fan-out.

## Run it

A single invocation (one recipient):

```bash
cd examples/spawn-map-foreach
modal-rust run notify --input '{"name":"ada","channel":"email"}'
```

Expected output (shape; `receipt_id` is derived from the input):

```json
{"ok":true,"value":{"name":"ada","channel":"email","receipt_id":"...","sent":"..."}}
```

Note: `notify` requires `--input` with `name` and `channel` fields. The CLI
validates the input shape locally and fails fast (without calling Modal) if it
does not match.

Driver tour — exercises `.spawn_map()` and `.for_each()` in-process (no Modal
credentials needed):

```bash
cargo run -p example-spawn-map-foreach --bin spawn_map_foreach
```

The live `.spawn_map([...])` / `.for_each([...])` shapes are driven from the
facade `App` API; see `src/lib.rs` and the crate's `tests/`.

## Prereqs

Modal credentials configured (`modal token new`). Run `modal-rust doctor --rust`
to check your environment first.
