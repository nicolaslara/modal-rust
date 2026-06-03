"""M12 PTX builder (Tier 2, build-time only — NOT a runtime path).

Generates the precompiled `vector_add.ptx` for the M12 cudarc vector-add by
compiling a tiny CUDA C kernel with `nvcc` inside a Tier-2 `nvidia/cuda:*-devel-*`
image, then printing the PTX to stdout. We run this ONCE to produce the
checked-in `examples/cuda-vector-add/kernels/vector_add.ptx`; the M12 runtime
image stays Tier 0 (driver-only) and never sees nvcc/NVRTC.

This is exactly the tasks.md M12 option: "generated at deploy/image-build time in
a Tier-2 builder". PTX (`.target sm_*`) is driver-JIT + forward-compatible, so a
single PTX runs on T4 (sm_75) via the driver's JIT at module-load time.

Run:
  modal run workpads/gpu-compute/ptx_builder.py::gen_ptx > /tmp/vector_add.ptx
"""

import modal

app = modal.App("modal-rust-ptx-builder")

# Tier 2: nvcc present. CUDA 12.6 toolkit (major 12 <= host driver's supported
# major; 12.x/13.x guaranteed compatible). No GPU needed to COMPILE to PTX.
builder_image = modal.Image.from_registry(
    "nvidia/cuda:12.6.2-devel-ubuntu22.04", add_python="3.12"
).entrypoint([])

KERNEL_CU = r"""
extern "C" __global__
void vector_add(float *out, const float *a, const float *b, int n) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i < n) {
        out[i] = a[i] + b[i];
    }
}
"""


@app.function(image=builder_image)
def build_ptx() -> str:
    import subprocess

    with open("/tmp/vector_add.cu", "w") as f:
        f.write(KERNEL_CU)

    # -arch=sm_52 -> low, forward-compatible target; the driver JITs forward to
    # the actual GPU at load time. PTX output (-ptx), not cubin.
    subprocess.run(
        [
            "nvcc", "-ptx",
            "-arch=compute_52",
            "-o", "/tmp/vector_add.ptx",
            "/tmp/vector_add.cu",
        ],
        check=True,
    )
    with open("/tmp/vector_add.ptx") as f:
        return f.read()


@app.local_entrypoint()
def gen_ptx():
    print(build_ptx.remote(), end="")
