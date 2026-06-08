# own-runner-bin

The escape hatch: a macro crate that ships its **own** one-line
`src/bin/modal_runner.rs` (`modal_rust::modal_runner!(crate);`) — for when you
want to wrap startup. The `modal-rust` CLI auto-detects this bin and uses it as-is
instead of generating one, so `modal-rust run` works normally. `extract_metrics`
aggregates a batch of log lines.

## Run it

```bash
cd examples/own-runner-bin
modal-rust run extract_metrics --input '{"lines":["INFO source=web ok","ERROR source=db fail","INFO source=web ok"]}'
```

Expected output (shape):

```json
{"ok":true,"value":{"total":3,"errors":1,"busiest_source":"web"}}
```

You can also drive the bin directly:
`cargo run --bin modal_runner -- --entrypoint extract_metrics --input-json '{...}'`.

## Prereqs

Modal credentials configured (`modal token new`). Run `modal-rust doctor --rust`
to check your environment first.
