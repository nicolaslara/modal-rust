# fan-out-map

Embarrassingly-parallel scale-out with `.map()`: one `#[function]` mapped over N
independent inputs returns `Vec<Out>` in input order. `analyze` reads a document
and returns word count + estimated reading time — a self-contained per-record task
that is textbook "embarrassingly parallel".

## Run it

A single invocation (one document):

```bash
cd examples/fan-out-map
modal-rust run analyze --input '{"title":"doc","body":"the quick brown fox"}'
```

Expected output:

```json
{"ok":true,"value":{"title":"doc","words":4,"minutes":1}}
```

Note: `analyze` requires `--input` with `title` and `body` fields. The CLI
validates the input shape locally and fails fast (without calling Modal) if it
does not match.

Driver tour — runs the fan-out locally in-process (no Modal credentials needed):

```bash
cargo run -p example-fan-out-map --bin fan_out_map
```

The live `.map([...])` shape (one `#[function]` over many documents in parallel)
is driven from the facade `App` API; see `src/lib.rs` and the crate's `tests/`.

## Prereqs

Modal credentials configured (`modal token new`). Run `modal-rust doctor --rust`
to check your environment first.
