# retries

Make a flaky function self-heal with an automatic retry policy
(`#[function(retries = 5)]`). `fetch` simulates a downstream that fails the first
two attempts and succeeds on the third; the retry policy drives it to success
without any retry loop in your code.

## Run it

```bash
cd examples/retries
modal-rust run fetch --input '{"resource":"db","attempt":3}'
```

Expected output (attempt 3 is the settling attempt, so the call succeeds):

```json
{"ok":true,"value":{"resource":"db","attempt":3}}
```

`attempt` is a demo field that lets you control which numbered attempt is
simulated. On a real Modal deployment you would not pass `attempt` yourself —
the retry policy re-runs the whole call automatically up to 5 times.

## Prereqs

Modal credentials configured (`modal token new`). Run `modal-rust doctor` to
check your environment first.
