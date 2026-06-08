# burn-add (GPU/ML — deploy this one)

A real ML workload: a Burn/CubeCL tensor add on the CUDA backend (kernels
JIT-compiled via NVRTC at runtime) on a T4, verified against a CPU reference.
Authored with
`#[modal_rust::function(gpu = "T4", name = "burn_add", memory = 8192)]` — the
decorator IS the config.

## Why deploy is recommended here (not "GPU forces deploy")

A GPU does **not** require `deploy`. The reason `deploy` is recommended for *this*
example is the size of the build, not the accelerator. This is a heavy ML crate
(Burn + CubeCL + the CUDA runtime), and `modal-rust run` builds the binary **in the
function body** at call time. That in-body `cargo` build is large enough to be
OOM-killed on a default container (you would see `GENERIC_STATUS_TERMINATED` with no
error output), which is why `memory =` matters on the run path. `deploy` instead
builds the binary **at image-build time** with full build resources — once — so the
deployed container only runs the prebuilt model and never pays a per-cold-container
rebuild.

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

## GPU `run` is supported (not deploy-only)

A GPU `run` of this function **is** supported — the base image is no longer just
env-driven. There are two ways to give the run path a CUDA-devel base so NVRTC can
JIT-compile the kernels:

- **Per-function image decorator (C1).** Declare the base right on the entrypoint:

  ```rust
  #[modal_rust::function(
      gpu = "T4",
      name = "burn_add",
      memory = 8192,
      image = Image(base = "nvidia/cuda:12.6.3-devel-ubuntu22.04", install_rust = true),
  )]
  ```

  `base`/`install_rust` override the path default; `apt`/`pip`/`run` (if you add
  them) prepend to the path-level steps.

- **Path-level base knobs.** The same `MODAL_RUST_BASE_IMAGE=nvidia/cuda:..-devel`
  + `MODAL_RUST_INSTALL_RUST=1` env vars (or `RemoteConfig.base_image` /
  `.install_rust`) shown above for `deploy` apply to `run` as well — same one
  command, just `run` instead of `deploy`:

  ```bash
  cd examples/burn-add
  MODAL_RUST_BASE_IMAGE=nvidia/cuda:12.6.3-devel-ubuntu22.04 MODAL_RUST_INSTALL_RUST=1 \
    modal-rust run burn_add --input '{"n":256}'
  ```

Either way, keep `memory = 8192` (or higher) so the in-body `cargo` build does not
OOM. `deploy` is still recommended here because it moves that heavy build to
image-build time (once, with full resources) instead of paying it on every cold
container — a performance/efficiency choice, not a requirement. (GPU `run`
end-to-end is not asserted as verified here; the mechanism above is what enables it.)

If a `run` is killed, the build log lives in `modal app logs` (it is lost
client-side when the container is killed). See
`docs/local/burn-add-run-failure.md` for the confirmed root cause.

## Prereqs

Modal credentials configured (`modal token new`) with GPU access. Run
`modal-rust doctor --rust` to check your environment first.
