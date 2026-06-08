# autoscaling

Control warm capacity and scale-to-zero for latency vs cost via the autoscaling
decorator fields (`min`/`max`/`buffer_containers`, `scaledown_window`). `embed`
turns a document into a fixed-width, L2-normalized feature vector — a believable
unit of work to scale out.

## Run it

```bash
cd examples/autoscaling
modal-rust run embed --input '{"text":"the quick brown fox"}'
```

Expected output (a fixed-width unit vector; `dimensions` is the vector length):

```json
{"ok":true,"value":{"text":"the quick brown fox","vector":[...],"dimensions":<n>}}
```

Note: `embed` requires `--input` with a `text` field. The CLI validates the input
shape locally and fails fast (without calling Modal) if it does not match.

## Prereqs

Modal credentials configured (`modal token new`). Run `modal-rust doctor --rust`
to check your environment first.
