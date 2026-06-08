# add (manual / no-macro)

The same `add` as `quickstart`, written **by hand** — the input/output structs,
the `typed!(fn)` registration, and the `modal_registry()` builder, i.e. everything
the `#[function]` macro generates for you. A low-level reference. It also defines
named entrypoints exercising every runner error kind (`fail`, `fail_structured`,
`bad_encode`, `will_panic`, `gpu_info`).

## Run it (manual — not `modal-rust run`)

This crate has **no** `#[modal_rust::function]` and ships a hand-written runner
named `add-runner` (not `modal_runner`), so the `modal-rust` CLI cannot generate a
runner for it. Invoke its own bin directly:

```bash
cd examples/add
cargo run --bin add-runner -- --entrypoint add --input-json '{"a":40,"b":2}'
```

Expected output:

```json
{"ok":true,"value":{"sum":42}}
```

Other entrypoints (e.g. error kinds) follow the same form:
`--entrypoint fail --input-json '{"a":1,"b":2}'`.

## Prereqs

None — this runs the local runner binary directly (no Modal call).
