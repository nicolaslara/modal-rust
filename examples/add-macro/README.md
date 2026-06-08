# add-macro

The same `add` as `quickstart` in three lines, plus the full decorator config
tour (`gpu`/`timeout`/`cache`/`secrets`/`volumes`) kept in `proof.rs`. The macro
generates the input struct, registration, and the typed `app.add(2, 3)` method.

## Run it

```bash
cd examples/add-macro
modal-rust run add --input '{"a":2,"b":3}'
```

Expected output:

```json
{"ok":true,"value":5}
```

The typed facade form is `app.add(2, 3).remote().await?`; see `src/lib.rs` and
`src/proof.rs` for the decorator-config and secrets/volumes coverage.

## Prereqs

Modal credentials configured (`modal token new`). Run `modal-rust doctor --rust`
to check your environment first.
