# secrets

Attach a named Modal secret on the decorator
(`#[function(secrets = ["my-api-key"])]`) and read it as an env var inside the
function. `check_secret` reports whether `MY_API_KEY` was injected and its length
— it never returns the value.

## Run it

```bash
cd examples/secrets
modal-rust run check_secret --input '{}'
```

Expected output (when the secret `my-api-key` exists and carries `MY_API_KEY`):

```json
{"ok":true,"value":{"present":true,"len":<n>}}
```

## Prereqs

Modal credentials configured (`modal token new`), and a Modal secret named
`my-api-key` containing `MY_API_KEY`
(`modal secret create my-api-key MY_API_KEY=...`). Run `modal-rust doctor --rust`
to check your environment first.
