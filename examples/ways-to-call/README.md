# ways-to-call

One function (`square(n: i64)`), four invocation patterns side by side:
`.local()`, `.remote().await`, `.spawn()` + `.get()`, and `.map([...])`.
Teaches *how to actually call* a `#[modal_rust::function]` and when to reach
for each shape — all through the same typed `app.square(n)` method the macro
generates, no input/output type named at the call site.

## Run it

**Via the CLI** (single remote call, one result):

```bash
cd examples/ways-to-call
modal-rust run square --input '{"n":6}'
```

Expected output:

```json
{"ok":true,"value":36}
```

**Via the driver binary** (all four shapes; offline `.local()` runs by default,
live shapes require `RUN_REMOTE=1` and Modal credentials):

```bash
cargo run -p example-ways-to-call --bin ways_to_call
```

Expected output (offline only):

```
local:  square(6) -> 36
(skipping live .remote()/.spawn()/.map() — set RUN_REMOTE=1 with Modal credentials to run them)
```

To also run the three live shapes (`.remote()`, `.spawn()`, `.map()`):

```bash
RUN_REMOTE=1 cargo run -p example-ways-to-call --bin ways_to_call
```

## Prereqs

Modal credentials configured (`modal token new`). Run `modal-rust doctor` to
check your toolchain and Modal auth before the first run.
