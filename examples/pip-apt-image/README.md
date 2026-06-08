# pip-apt-image

Teach the image-builder steps API: add arbitrary system packages, Python
packages, and shell commands to the build image through `RemoteConfig::image_steps`
(`ImageStep::apt` / `ImageStep::pip` / `ImageStep::run`) without touching the
function body. Steps mirror Modal Python's `Image.apt_install(...)`,
`.pip_install(...)`, and `.run_commands(...)`, and are rendered into the image
Dockerfile in the order you chain them.

## Run it

### Offline driver (no credentials needed)

The primary lesson is the image the facade renders. The offline driver builds a
`RemoteConfig` with three chained steps, calls `App::dry_run`, and prints the
rendered Dockerfile lines — no Modal, no network.

```bash
cd examples/pip-apt-image
cargo run -p example-pip-apt-image --bin pip_apt_image
```

Expected output (the three step lines, in chain order):

```
apt: RUN apt-get install -y libpng-dev libjpeg-dev
pip: RUN pip install numpy pillow
run: RUN echo built > /opt/marker
```

### Live run (Modal credentials required)

`render` is a plain CPU function; it runs a deterministic hash of the input so
you can confirm the body ran.

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

## Prereqs

Modal credentials configured (`modal token new`) for the live run; the offline
driver needs none. Run `modal-rust doctor --rust` to check your environment first.
