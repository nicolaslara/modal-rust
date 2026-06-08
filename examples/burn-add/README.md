# burn-add (GPU/ML — deploy this one)

A real ML workload: a Burn/CubeCL tensor add on the CUDA backend (kernels
JIT-compiled via NVRTC at runtime) on a T4, verified against a CPU reference.
Authored with
`#[modal_rust::function(gpu = "T4", name = "burn_add", memory = 8192)]` — the
decorator IS the config.

## Recommended: deploy, then call

This is a heavy ML crate (Burn + CubeCL + the CUDA runtime). `modal-rust run`
builds the binary **in the function body** at call time, and that build is large
enough to be OOM-killed on a default container (you would see
`GENERIC_STATUS_TERMINATED` with no error output). `deploy` builds the binary **at
image-build time** with full build resources, so the deployed container only runs
the prebuilt model.

Deploy with a CUDA base image (so the runtime carries `libnvrtc` + `libcudart`)
and install the Rust toolchain at build time:

```bash
cd examples/burn-add
MODAL_RUST_BASE_IMAGE=nvidia/cuda:12.6.3-devel-ubuntu22.04 MODAL_RUST_INSTALL_RUST=1 \
  modal-rust deploy burn_add --app modal-rust-burn-add-example
modal-rust call burn_add --app modal-rust-burn-add-example --input '{"n":256}'
```

Expected output (shape; `backend`/`libnvrtc`/`libcudart` are the live device
proof):

```json
{"ok":true,"value":{"valid":true,"n":256,"backend":"burn-cuda (CubeCL CUDA / cudarc)","libnvrtc":"...","libcudart":"...","samples":[...]}}
```

The `memory = 8192` on the decorator gives a real run more headroom, but the base
image for a `run`-path build is still env-driven (a custom per-function image is a
roadmap item), so **deploy remains the recommended path**. If a `run` is killed,
the build log lives in `modal app logs` (it is lost client-side when the container
is killed). See `docs/local/burn-add-run-failure.md` for the confirmed root cause.

## Prereqs

Modal credentials configured (`modal token new`) with GPU access. Run
`modal-rust doctor --rust` to check your environment first.
