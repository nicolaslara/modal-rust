# cli-workflow

Drive your crate from the `modal-rust` CLI end to end — `doctor` (preflight),
`run`, `deploy`, `call` — with no driver binary. `summarize` (a
`#[function(name = "summarize")]`) returns word/char counts and an estimated read
time for a document.

## Run it

```bash
cd examples/cli-workflow
modal-rust doctor --rust
modal-rust run summarize --input '{"text":"the quick brown fox"}'
```

Expected output:

```json
{"ok":true,"value":{"words":4,"chars":19,"read_minutes":1}}
```

Deploy then call (build once, call many):

```bash
modal-rust deploy summarize --app cli-workflow
modal-rust call summarize --app cli-workflow --input '{"text":"hello world"}'
```

## Prereqs

Modal credentials configured (`modal token new`). `modal-rust doctor --rust`
checks your toolchain and Modal auth.
