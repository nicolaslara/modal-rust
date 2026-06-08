# custom-types

Real functions take and return your own `struct`s. Derive
`Serialize`/`Deserialize`, take a struct, return a struct — the macro infers the
typed I/O from the signature. Here `score(Player) -> Scored` turns a match record
into a score.

## Run it

```bash
cd examples/custom-types
modal-rust run score --input '{"name":"ada","hits":7,"shots":10}'
```

Expected output:

```json
{"ok":true,"value":{"name":"ada","points":700,"accuracy_pct":70}}
```

## Prereqs

Modal credentials configured (`modal token new`). Run `modal-rust doctor --rust`
to check your environment first.
