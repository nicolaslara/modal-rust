# stateful-class (GPU — deploy this one)

Load-once-serve-many with `#[modal_rust::cls]`. An `#[enter]` method loads an
expensive resource (here an embedding model) **once** per warm container; each
`#[method]` reuses it by `&self`. Each method becomes its own dotted entrypoint
(`Embedder.embed` with `gpu = "A10G"`, `Embedder.dim` with `gpu = "T4"`), with
merged class + method-override config proven offline by `tests/manifest.rs`.

## Run it

This example requests GPU resources (`gpu = "T4"` class default, `gpu = "A10G"`
on `embed`). Deploy first so Modal pre-schedules the container with the right
hardware; then call:

```bash
cd examples/stateful-class
modal-rust deploy Embedder.embed --app modal-rust-stateful-class
modal-rust call Embedder.embed --app modal-rust-stateful-class --input '{"text":"hello"}'
```

Expected output (a fixed-width 8-element unit-length embedding vector):

```json
{"ok":true,"value":[...]}
```

The dimensionality method takes no input:

```bash
modal-rust call Embedder.dim --app modal-rust-stateful-class --input '{}'
```

```json
{"ok":true,"value":8}
```

Note: `Embedder.embed` requires `--input` with a `text` field. The CLI validates
the input shape locally and fails fast (without calling Modal) if it does not
match.

## Prereqs

Modal credentials configured (`modal token new`) with GPU access. Run
`modal-rust doctor --rust` to check your environment first.
