# pip-apt-image

Teach the image-builder steps API: add arbitrary system packages, Python
packages, and shell commands to the build image through `RemoteConfig::image_steps`
(`ImageStep::apt` / `ImageStep::pip` / `ImageStep::run`) without touching the
function body. Steps mirror Modal Python's `Image.apt_install(...)`,
`.pip_install(...)`, and `.run_commands(...)`, and are rendered into the image
Dockerfile in the order you chain them.

## Run it

`render` is a plain CPU function; it runs a deterministic hash of the input so
you can confirm the body ran on an image built from your chained steps.

```bash
cd examples/pip-apt-image
modal-rust run render --input '{"value":7}'
```

Expected output (`digest` is `mix(7)`, a wrapping-multiply of the input):

```json
{"ok":true,"value":{"digest":<u64>}}
```

Note: `render` requires `--input` with a `value` field (a `u64`). The CLI
validates the input shape locally and fails fast (without calling Modal) if it
does not match.

Inspect the rendered steps offline (no credentials): the driver builds a
`RemoteConfig` with three chained steps, calls `App::dry_run`, and prints the
rendered Dockerfile lines — no Modal, no network.

```bash
cargo run -p example-pip-apt-image --bin pip_apt_image
# apt: RUN apt-get install -y libpng-dev libjpeg-dev
# pip: RUN pip install numpy pillow
# run: RUN echo built > /opt/marker
```

## Prereqs

Modal credentials configured (`modal token new`) for the live run; the offline
driver needs none. Run `modal-rust doctor --rust` to check your environment first.
