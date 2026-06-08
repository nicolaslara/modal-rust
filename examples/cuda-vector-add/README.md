# cuda-vector-add (GPU — deploy this one)

A real GPU kernel: the `cudarc` Driver API + a precompiled PTX kernel running an
element-wise vector add on a T4, verified against a CPU reference. Authored with
`#[modal_rust::function(gpu = "T4", name = "vector_add", memory = 8192)]` — the
decorator IS the config.

## Recommended: deploy, then call

This is a heavy CUDA crate. `modal-rust run` builds the binary **in the function
body** at call time, and that build is large enough to be OOM-killed on a default
container (you would see `GENERIC_STATUS_TERMINATED` with no error output).
`deploy` builds the binary **at image-build time** with full build resources, so
the deployed container only runs the prebuilt kernel.

Deploy with a CUDA base image (so the runtime carries the CUDA libraries) and
install the Rust toolchain at build time:

```bash
cd examples/cuda-vector-add
MODAL_RUST_BASE_IMAGE=nvidia/cuda:12.6.3-devel-ubuntu22.04 MODAL_RUST_INSTALL_RUST=1 \
  modal-rust deploy vector_add --app cuda-vector-add
modal-rust call vector_add --app cuda-vector-add --input '{"n":1024}'
```

Expected output (shape; `gpu_name`/`driver_version` reflect the live device):

```json
{"ok":true,"value":{"valid":true,"n":1024,"gpu_name":"Tesla T4","driver_version":<int>,"samples":[...]}}
```

The `memory = 8192` on the decorator gives a real run more headroom, but the base
image for a `run`-path build is still env-driven (a custom per-function image is a
roadmap item), so **deploy remains the recommended path**. If a `run` is killed,
the build log lives in `modal app logs` (it is lost client-side when the container
is killed).

## Prereqs

Modal credentials configured (`modal token new`) with GPU access. Run
`modal-rust doctor --rust` to check your environment first.
