# scheduled-job

A deployed function that runs on a cron schedule — no caller needed
(`#[function(schedule = Cron("0 9 * * 1"))]`). `weekly_report` rolls up a batch
of events into per-source totals. Teaches the `schedule =` decorator field:
once deployed, Modal triggers the function automatically on the cadence; no
client code calls it.

## Run it

`deploy` is the primary command for this example — it registers the cron
schedule with Modal so the platform triggers `weekly_report` every Monday at
09:00 UTC:

```bash
cd examples/scheduled-job
modal-rust deploy weekly_report --app modal-rust-scheduled-job
```

Once deployed, Modal invokes the function on the schedule with no further
action needed. Check `modal app logs modal-rust-scheduled-job` to see output
from scheduled runs.

To invoke the function body directly (useful for testing the logic without
waiting for the schedule):

```bash
modal-rust call weekly_report --app modal-rust-scheduled-job \
  --input '{"dataset":"signups","events":[{"source":"web","count":3},{"source":"web","count":2},{"source":"ios","count":4}]}'
```

Expected output:

```json
{"ok":true,"value":{"dataset":"signups","rows":9,"by_source":{"ios":4,"web":5},"busiest":"web","note":"..."}}
```

You can also run it ephemerally (no deploy) to test the function body locally
without registering the schedule:

```bash
modal-rust run weekly_report \
  --input '{"dataset":"signups","events":[{"source":"web","count":3},{"source":"web","count":2},{"source":"ios","count":4}]}'
```

## Prereqs

Modal credentials configured (`modal token new`). Run `modal-rust doctor --rust`
to check your environment first.
