# background-jobs

Fire-and-forget with `.spawn()`: enqueue a job and get a handle back
immediately, do other work, then collect the result later with
`.get(timeout)`. Teaches the async-job pattern — the difference from
`.remote()`, which blocks until the result is ready.

## Run it

```bash
cd examples/background-jobs
modal-rust run run_job --input '{"label":"nightly","rounds":1000}'
```

Expected output (`digest` is deterministic for the given input):

```json
{"ok":true,"value":{"label":"nightly","rounds":1000,"digest":<u64>}}
```

To tour the `.spawn()` + `.get(timeout)` flow locally (no Modal credentials
needed for the offline path):

```bash
cargo run -p example-background-jobs --bin background_jobs
```

## Prereqs

Modal credentials configured (`modal token new`). Run `modal-rust doctor --rust`
to check your environment first.
