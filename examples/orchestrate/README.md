# orchestrate

A runnable tour of the `modal-rust` facade: `App`/`Function` with offline
`.local()` (zero-Modal, zero-credentials) plus the live `.remote()` and
`deploy`+`call` round-trips (credential-gated). Reuses the `add` function from
`examples/add` and `examples/add-macro` to prove that the manual registry and
the `#[modal_rust::function]` macro registry are equivalent.

This example has its own `main()` and is driven with `cargo`, not `modal-rust run`.

## Run it

```bash
cd examples/orchestrate
cargo run --bin orchestrate
```

Expected output (offline path only — the default):

```
local: add(40, 2) -> {sum: 42}
local (macro/inventory): registry resolves `add` by name
local (macro auto-I/O):  add(2, 3) -> 5
(skipping live .remote()/deploy/call — set RUN_REMOTE=1 with Modal credentials to run them)
```

To also run the live `.remote()` and `deploy`+`call` paths, set `RUN_REMOTE=1`
with Modal credentials in your environment:

```bash
RUN_REMOTE=1 cargo run --bin orchestrate
```

## Prereqs

The offline path needs nothing. The remote/deploy path needs Modal credentials
configured (`modal token new`). Run `modal-rust doctor` to check your
environment first.
