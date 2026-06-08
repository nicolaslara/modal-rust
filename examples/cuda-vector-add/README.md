# cuda-vector-add (GPU — deploy this one)

A real GPU kernel: the `cudarc` Driver API + a precompiled PTX kernel running an
element-wise vector add on a T4, verified against a CPU reference. Authored with
`#[modal_rust::function(gpu = "T4", name = "vector_add", memory = 8192)]` — the
decorator IS the config.

## Why deploy is recommended here (it is not a GPU requirement)

GPU alone does not force a deploy. The reason this example benefits from `deploy`
is purely about *where the build happens*. `modal-rust run` builds the runner
binary **in the function container, at call time** — so a cold call pays a full
`cargo` compile, and for a heavy CUDA crate that build is large enough to be
OOM-killed on a default container (you would see `GENERIC_STATUS_TERMINATED` with
no error output). `deploy` builds the binary **once, at image-build time** with
full build resources, so the deployed container only runs the prebuilt kernel and
never recompiles on a cold start.

So a GPU `run` is genuinely supported — you just need (a) a CUDA-devel base so the
container carries the CUDA libraries and a Rust toolchain, and (b) enough memory
for the in-body build. Two ways to set the base:

- **Per-function image (C1):** put it on the decorator —
  `#[modal_rust::function(gpu = "T4", name = "vector_add", memory = 8192,
  image = Image(base = "nvidia/cuda:12.6.3-devel-ubuntu22.04", install_rust = true))]`.
  This sets *this* entrypoint's base for the `run` path too.
- **Path-level base knobs:** `MODAL_RUST_BASE_IMAGE` / `MODAL_RUST_INSTALL_RUST`
  (or the equivalent run-config fields) applied to the whole crate.

Either way, set `memory =` high enough for the heavy compile. We document the
mechanism rather than asserting a specific verified GPU `run` command; for the
heavy in-body build, `deploy` is the recommended, repeatable path:

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

The `memory = 8192` on the decorator gives a real `run` more headroom, and the
`run`-path base image is now fully settable too — either per-function via
`image = Image(base = "…-devel", install_rust = true)` (C1) or via the path-level
`MODAL_RUST_BASE_IMAGE` / `MODAL_RUST_INSTALL_RUST` knobs. The remaining reason to
prefer `deploy` is the **cost of the in-body build**: `run` recompiles the heavy
crate on every cold container (slow, and OOM-prone unless `memory =` is high),
whereas `deploy` compiles once at image-build time and amortizes it across every
call. If a `run` is killed, the build log lives in `modal app logs` (it is lost
client-side when the container is killed).

## Prereqs

Modal credentials configured (`modal token new`) with GPU access. Run
`modal-rust doctor --rust` to check your environment first.
