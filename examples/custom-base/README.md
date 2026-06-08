# custom-base

Pick the RUN base image and install the Rust toolchain through exposed build-config
knobs (`RemoteConfig.base_image` / `.install_rust`, or the `MODAL_RUST_BASE_IMAGE` /
`MODAL_RUST_INSTALL_RUST` env vars) without editing the function body. `probe`
checksums an input value so you can confirm the body ran on your chosen image.

## Run it

### Offline driver (no credentials needed)

The primary lesson is the IMAGE the facade renders. The offline driver builds a
`RemoteConfig` pointed at a CUDA-devel base with `install_rust = true`, calls
`App::dry_run`, and prints the rendered Dockerfile lines — no Modal, no network.

```bash
cd examples/custom-base
cargo run -p example-custom-base --bin custom_base
```

Expected output:

```
base:   FROM nvidia/cuda:12.6.3-devel-ubuntu22.04
rustup: RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable ...
```

### Live run (Modal credentials required)

`probe` is a plain CPU function — it runs with any base image that has a Rust
toolchain. To run it on Modal you must set the base-image env vars (otherwise the
default `rust:*-slim` base is used and the knobs are not exercised):

```bash
cd examples/custom-base
MODAL_RUST_BASE_IMAGE=python:3.12-slim MODAL_RUST_INSTALL_RUST=1 \
  modal-rust run probe --input '{"value":42}'
```

Expected output (`checksum` is a deterministic FNV-1a of `value`):

```json
{"ok":true,"value":{"value":42,"checksum":<u64>}}
```

Note: `probe` requires `--input` with a `value` field. The CLI validates the input
shape locally and fails fast (without calling Modal) if it does not match.

## Prereqs

Modal credentials configured (`modal token new`) for the live run; the offline driver
needs none. Run `modal-rust doctor --rust` to check your environment first.
