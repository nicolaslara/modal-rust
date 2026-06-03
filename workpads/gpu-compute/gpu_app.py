"""modal-rust GPU shim — M10 (`nvidia-smi` from Python) + M11 (`nvidia-smi` from RUST).

The gpu-compute workpad's first two GPU milestones, both Tier 0 (driver-only),
Burn-free and CUDA-toolkit-free (the Burn-free-first ordering — see `tasks.md`
M10/M11 and `../architecture/research-synthesis.md` §2.8 / §3):

  - M10 (`smi_py`): observe a real NVIDIA GPU from plain PYTHON, proving `gpu=`
    placement lands on a GPU machine (driver + `nvidia-smi` preinstalled) before
    any Rust or CUDA toolkit.
  - M11 (`gpu_info_rust`): observe the SAME GPU from RUST — a `modal_runner` built
    in the Function body shells out to `nvidia-smi` via `std::process::Command`
    and returns its stdout through the M0 JSON envelope. The ONLY new variable
    over the CPU-proven prototype M4/M7 build path is `gpu="T4"` placement; the
    build recipe is otherwise byte-identical to `dev_app.py`'s M4 path.

Contract source of truth:
  - workpads/gpu-compute/tasks.md            (M10/M11 acceptance + evidence)
  - workpads/architecture/boundaries.md §9   (GPU tiering + `gpu=` passthrough)
                              §4/§5          (the M4 build boundary + shim recipe)
  - workpads/architecture/research-synthesis.md §1.4, §2.8, §3 (M10/M11)

Tier 0 (driver-only), the ONLY new variable over the prototype's build path:
  - NO CUDA toolkit is installed: `libcuda.so` + `nvidia-smi` are PREINSTALLED on
    Modal GPU machines; `libcudart`/`nvcc`/`libnvrtc` are absent. The images here
    add NOTHING CUDA-related — and `cargo tree` shows NO `cudarc` (still Tier 0).
  - `gpu="T4"` (the cheapest GPU family) is the sole delta vs the CPU build path.
    `gpu=` is passed VERBATIM (boundaries.md §9): a good type places, a bad type
    surfaces Modal's error — the drifting catalog is NOT re-implemented here.

Flag-mapping (mirrors the prototype dev_app.py): `modal run` auto-binds CLI flags
ONLY to a `@app.local_entrypoint()`, by parameter name. A bare `@app.function`
does NOT bind flags. So each `@app.function(gpu="T4")` body is invoked by a
`@app.local_entrypoint()` that prints its result.

Run (cost-sensitive — every run attaches a GPU and costs real money):
  modal run workpads/gpu-compute/gpu_app.py::smi_py          # M10 (Python)
  modal run workpads/gpu-compute/gpu_app.py::gpu_info_rust   # M11 (Rust)
"""

import subprocess

import modal

# --- injected params (kept aligned with the prototype shims) ---
APP_NAME = "modal-rust-gpu-poc"
GPU = "T4"  # cheapest GPU family; passed VERBATIM to Modal (boundaries.md §9)
RUST_VER = "1"  # same toolchain pin as the prototype dev_app.py (M4)
LOCAL_SRC = "/Users/nicolas/devel/modal-rust"
REMOTE_SRC = "/src"

app = modal.App(APP_NAME)

# Tier 0 image: a stock CUDA-free base with Modal's runtime. We do NOT install any
# CUDA toolkit — the GPU machine preinstalls the NVIDIA driver + Driver API
# (`libcuda.so`) + `nvidia-smi`. `nvidia-smi` is all M10 needs.
#
# debian_slim already hosts Python+pip on $PATH, so it is a valid Function image
# with no `add_python` needed and nothing CUDA-related added (keeps M10 strictly
# Tier 0). The Rust toolchain is deliberately absent here: M10 is no-Rust.
image = modal.Image.debian_slim()


# --------------------------------------------------------------------------- M10
# A normal @app.function placed on a GPU. Its body shells out to `nvidia-smi` and
# returns the full stdout. This is Modal's own documented GPU sanity pattern,
# exercising `gpu=` placement on a real NVIDIA GPU from plain Python.
@app.function(image=image, gpu=GPU)
def smi() -> str:
    proc = subprocess.run(["nvidia-smi"], capture_output=True, text=True)
    out = proc.stdout
    if proc.stderr:
        out += "\n=== nvidia-smi stderr ===\n" + proc.stderr
    return out


@app.local_entrypoint()
def smi_py():
    print(smi.remote())


# --------------------------------------------------------------------------- M11
# `nvidia-smi` from RUST. This is the EXACT prototype M4 run-path build recipe
# (dev_app.py::run_entrypoint) with `gpu="T4"` as the ONLY new variable.
#
# Build boundary (boundaries.md §4): source is MOUNTED at startup via
# `add_local_dir(copy=False)` and `cargo build` runs in the FUNCTION BODY at
# execution time (NOT at image-build time). We build a known-writable LOCAL path
# (CARGO_HOME=/tmp/cargo, CARGO_TARGET_DIR=/tmp/target), copying /src -> /tmp/build
# if the mount is read-only — identical to dev_app.py.
#
# Two deltas vs dev_app.py::run_entrypoint, both required and neither CUDA:
#   1. `gpu="T4"` on the @app.function (the milestone's sole new variable).
#   2. The build is PACKAGE-QUALIFIED (`-p example-add`): multiple example crates
#      share the `modal_runner` bin name, so a bare `--bin modal_runner` is
#      ambiguous. We build + exec example-add's binary specifically.
#
# Tier 0 image: same Rust toolchain base as dev_app.py (NO CUDA toolkit added).
# `add_python="3.12"` is mandatory (a bare `rust:` image is an invalid Function
# image); `.entrypoint([])` neutralizes the base ENTRYPOINT.
mounted_image = (
    modal.Image.from_registry(f"rust:{RUST_VER}-slim", add_python="3.12")
    .entrypoint([])
    .env({"RUST_BACKTRACE": "1"})
    .add_local_dir(
        LOCAL_SRC,
        REMOTE_SRC,
        copy=False,
        ignore=["target", ".git", ".modal-rust", "**/*.rlib"],
    )
)


@app.function(image=mounted_image, gpu=GPU, timeout=1800)
def gpu_info_runner(entrypoint: str, input_json: str) -> str:
    import os
    import shutil
    import sys

    env = dict(os.environ)
    env["CARGO_HOME"] = "/tmp/cargo"
    env["CARGO_TARGET_DIR"] = "/tmp/target"
    env["RUST_BACKTRACE"] = "1"

    # Build location derived from mount writability (the prototype M2 probe).
    if os.access(REMOTE_SRC, os.W_OK):
        build_dir = REMOTE_SRC
        print(f"[m11] mount {REMOTE_SRC} is writable; building in place", file=sys.stderr)
    else:
        build_dir = "/tmp/build"
        print(
            f"[m11] mount {REMOTE_SRC} is read-only; cp -a {REMOTE_SRC} {build_dir}",
            file=sys.stderr,
        )
        if os.path.exists(build_dir):
            shutil.rmtree(build_dir)
        subprocess.run(["cp", "-a", REMOTE_SRC, build_dir], check=True)

    # cargo build --release -p example-add --bin modal_runner; PACKAGE-QUALIFIED
    # because multiple example crates share the `modal_runner` bin name. All logs
    # -> stderr (stdout stays a single JSON envelope, runner seam §2.2).
    build = subprocess.run(
        ["cargo", "build", "--release", "-p", "example-add", "--bin", "modal_runner"],
        cwd=build_dir,
        env=env,
        stdout=sys.stderr,
        stderr=sys.stderr,
    )
    if build.returncode != 0:
        raise RuntimeError(f"cargo build failed with exit code {build.returncode}")

    runner = "/tmp/target/release/modal_runner"

    with open("/tmp/in.json", "w") as f:
        f.write(input_json)

    proc = subprocess.run(
        [runner, "--entrypoint", entrypoint, "--input-file", "/tmp/in.json"],
        capture_output=True,
        text=True,
        env=env,
    )
    if proc.stderr:
        print(proc.stderr, file=sys.stderr)
    print(f"[m11] modal_runner exit={proc.returncode}", file=sys.stderr)
    return proc.stdout.strip()


@app.local_entrypoint()
def gpu_info_rust(entrypoint: str = "gpu_info", input_json: str = "{}"):
    print(gpu_info_runner.remote(entrypoint, input_json))


# --------------------------------------------------------------------------- M12
# Real Rust GPU COMPUTE: a cudarc vector-add via the CUDA Driver API, loading a
# PRECOMPILED PTX kernel (Tier 0, driver-only). This is the project's first GPU
# *compute* proof and is deliberately Burn-free (the Burn-free-first ordering:
# nvidia-smi -> rust nvidia-smi -> cudarc -> Burn).
#
# Build boundary (boundaries.md §4) — IDENTICAL to M11/the prototype M4 recipe:
# source MOUNTED at startup via `add_local_dir(copy=False)`; `cargo build` runs
# in the FUNCTION BODY at execution time into a known-writable LOCAL path
# (CARGO_HOME=/tmp/cargo, CARGO_TARGET_DIR=/tmp/target), copying /src -> /tmp/build
# if the mount is read-only. The image is the SAME Tier 0 base as M11
# (`rust:1-slim` + `add_python="3.12"`, `.entrypoint([])`) — NO CUDA toolkit added.
#
# Two deltas vs dev_app.py::run_entrypoint, both required and neither installs CUDA:
#   1. `gpu="T4"` on the @app.function (the milestone's sole new variable).
#   2. The build is PACKAGE-QUALIFIED for the cuda-vector-add crate
#      (`-p example-cuda-vector-add`): multiple example crates share the
#      `modal_runner` bin name, so a bare `--bin modal_runner` is ambiguous.
#
# Tier 0 (driver-only) holds at runtime: cudarc uses `dynamic-loading` (links with
# NO CUDA at build time; dlopens `libcuda` at runtime). The kernel ships as a
# checked-in precompiled PTX (`examples/cuda-vector-add/kernels/vector_add.ptx`),
# loaded through the Driver API (`cuModuleLoadData`) — NO runtime NVRTC, NO nvcc,
# NO libcudart. A startup self-check (`CudaContext::new`) dlopens `libcuda` and
# fails loudly if missing (the dynamic-loading footgun).
@app.function(image=mounted_image, gpu=GPU, timeout=1800)
def cuda_vector_add_runner(entrypoint: str, input_json: str) -> str:
    import os
    import shutil
    import sys

    env = dict(os.environ)
    env["CARGO_HOME"] = "/tmp/cargo"
    env["CARGO_TARGET_DIR"] = "/tmp/target"
    env["RUST_BACKTRACE"] = "1"

    if os.access(REMOTE_SRC, os.W_OK):
        build_dir = REMOTE_SRC
        print(f"[m12] mount {REMOTE_SRC} is writable; building in place", file=sys.stderr)
    else:
        build_dir = "/tmp/build"
        print(
            f"[m12] mount {REMOTE_SRC} is read-only; cp -a {REMOTE_SRC} {build_dir}",
            file=sys.stderr,
        )
        if os.path.exists(build_dir):
            shutil.rmtree(build_dir)
        subprocess.run(["cp", "-a", REMOTE_SRC, build_dir], check=True)

    # cargo build --release -p example-cuda-vector-add --bin modal_runner;
    # PACKAGE-QUALIFIED because multiple example crates share the `modal_runner`
    # bin name. cudarc `dynamic-loading` links with NO CUDA present at build time.
    # All logs -> stderr (stdout stays a single JSON envelope, runner seam §2.2).
    build = subprocess.run(
        [
            "cargo", "build", "--release",
            "-p", "example-cuda-vector-add", "--bin", "modal_runner",
        ],
        cwd=build_dir,
        env=env,
        stdout=sys.stderr,
        stderr=sys.stderr,
    )
    if build.returncode != 0:
        raise RuntimeError(f"cargo build failed with exit code {build.returncode}")

    runner = "/tmp/target/release/modal_runner"

    with open("/tmp/in.json", "w") as f:
        f.write(input_json)

    proc = subprocess.run(
        [runner, "--entrypoint", entrypoint, "--input-file", "/tmp/in.json"],
        capture_output=True,
        text=True,
        env=env,
    )
    if proc.stderr:
        print(proc.stderr, file=sys.stderr)
    print(f"[m12] modal_runner exit={proc.returncode}", file=sys.stderr)
    return proc.stdout.strip()


@app.local_entrypoint()
def cuda_vector_add(entrypoint: str = "vector_add", input_json: str = '{"n":1024}'):
    print(cuda_vector_add_runner.remote(entrypoint, input_json))


# --------------------------------------------------------------------------- M13
# Burn tensor smoke: a minimal Burn CUDA-backend tensor add `c = a + b`. This is
# the project's downstream-consumer GPU proof and the LAST step of the
# Burn-free-first ordering (nvidia-smi -> rust nvidia-smi -> cudarc -> *Burn*).
#
# Unlike M12 (cudarc + a precompiled PTX kernel through the Driver API, Tier 0),
# Burn drives CubeCL, which JIT-compiles its kernels via NVRTC AT RUNTIME. That
# makes M13 the first **Tier 1** milestone: `libnvrtc.so` AND `libcudart.so` MUST
# be on the loader path (boundaries.md §9; tasks.md M13).
#
# TIER 1 RECIPE (chosen): `nvidia/cuda:12.6.3-devel-ubuntu22.04` + a Rust
# toolchain installed via rustup at image-build time + `add_python="3.12"`
# (Modal's runtime requirement). CUDA 12.x <= the host driver's supported major
# (observed Driver API 13.0; drifts — NOT hardcoded), so the container toolkit
# major <= host (12.x/13.x guaranteed compatible).
#
# EMPIRICAL FINDING (why -devel-, not -runtime-): CubeCL JIT-compiles its CUDA C
# kernels via NVRTC at runtime, and the generated source `#include
# <cuda_runtime.h>`. cubecl-cuda passes `--include-path=$CUDA_PATH/include` (or
# `/usr/local/cuda/include`) to NVRTC, so the CUDA **headers** must be present —
# not just the runtime shared libs. The `*-runtime-*` image ships `libcudart` +
# `libnvrtc` but NOT the headers, so NVRTC fails with `catastrophic error: cannot
# open source file "cuda_runtime.h"`. The `*-devel-*` image ships the headers at
# `/usr/local/cuda/include` (and `libnvrtc`/`libcudart`), which is what "Burn
# requires CUDA 12.x on PATH" means in practice. We do NOT invoke `nvcc`
# ourselves — NVRTC (a runtime/Tier-1 mechanism) does the compiling — but it
# needs the toolkit headers the devel image provides. (`CUDA_PATH` is set so
# CubeCL's include-path resolution is explicit, not just the /usr/local/cuda
# default.)
#
# The BUILD PATH is otherwise the validated M4/M7/M11/M12 recipe: source MOUNTED
# at startup via `add_local_dir(copy=False)`; `cargo build` runs in the FUNCTION
# BODY at execution time into a known-writable LOCAL path
# (CARGO_HOME=/tmp/cargo, CARGO_TARGET_DIR=/tmp/target), copying /src -> /tmp/build
# if the mount is read-only. The image tier is the ONLY new variable over M11/M12.
#
# Two deltas vs dev_app.py::run_entrypoint, both required:
#   1. `gpu="T4"` on the @app.function (the cheapest GPU family; sole GPU var).
#   2. PACKAGE-QUALIFIED build (`-p example-burn-add`): multiple example crates
#      share the `modal_runner` bin name, so a bare `--bin modal_runner` is
#      ambiguous. We build + exec example-burn-add's binary specifically.
#
# A HARD startup self-check inside the runner (`example_burn_add::tier1_self_check`)
# `dlopen`s `libnvrtc` + `libcudart` BEFORE touching Burn and fails LOUDLY if the
# image is accidentally Tier 0 (dynamic loading otherwise hides a missing lib
# until the first kernel launch — the Burn-on-driver-only footgun).
CUDA_DEVEL_TAG = "nvidia/cuda:12.6.3-devel-ubuntu22.04"  # CUDA headers + libnvrtc + libcudart; 12.x <= host major

burn_image = (
    modal.Image.from_registry(CUDA_DEVEL_TAG, add_python="3.12")
    .entrypoint([])  # neutralize any base ENTRYPOINT so Modal's runtime starts
    .env({"RUST_BACKTRACE": "1", "CUDA_PATH": "/usr/local/cuda"})
    # Install the Rust toolchain at image-build time (the CUDA image has no Rust).
    # rustup is the documented, reproducible way; we put cargo on PATH.
    .apt_install("curl", "build-essential", "ca-certificates", "pkg-config")
    .run_commands(
        "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | "
        "sh -s -- -y --default-toolchain stable --profile minimal"
    )
    .env({
        "PATH": "/root/.cargo/bin:/usr/local/cuda/bin:/usr/local/sbin:"
        "/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    })
    .add_local_dir(
        LOCAL_SRC,
        REMOTE_SRC,
        copy=False,
        ignore=["target", ".git", ".modal-rust", "**/*.rlib"],
    )
)


@app.function(image=burn_image, gpu=GPU, timeout=1800)
def burn_add_runner(entrypoint: str, input_json: str) -> str:
    import os
    import shutil
    import sys

    env = dict(os.environ)
    env["CARGO_HOME"] = "/tmp/cargo"
    env["CARGO_TARGET_DIR"] = "/tmp/target"
    env["RUST_BACKTRACE"] = "1"
    # Ensure cargo + the CUDA libs are reachable (loader path) and CubeCL can
    # resolve the CUDA headers for its runtime NVRTC include-path.
    env["PATH"] = "/root/.cargo/bin:/usr/local/cuda/bin:" + env.get("PATH", "")
    env["CUDA_PATH"] = "/usr/local/cuda"

    # Quick Tier-1 visibility log: the CUDA runtime libs + the headers CubeCL's
    # runtime NVRTC needs (`cuda_runtime.h`) are on the image.
    subprocess.run(
        ["bash", "-lc",
         "ls -1 /usr/local/cuda/lib64/libnvrtc.so* /usr/local/cuda/lib64/libcudart.so* "
         "/usr/local/cuda/include/cuda_runtime.h 2>&1 | head -20 || true"],
        stdout=sys.stderr, stderr=sys.stderr,
    )

    if os.access(REMOTE_SRC, os.W_OK):
        build_dir = REMOTE_SRC
        print(f"[m13] mount {REMOTE_SRC} is writable; building in place", file=sys.stderr)
    else:
        build_dir = "/tmp/build"
        print(
            f"[m13] mount {REMOTE_SRC} is read-only; cp -a {REMOTE_SRC} {build_dir}",
            file=sys.stderr,
        )
        if os.path.exists(build_dir):
            shutil.rmtree(build_dir)
        subprocess.run(["cp", "-a", REMOTE_SRC, build_dir], check=True)

    # cargo build --release -p example-burn-add --bin modal_runner;
    # PACKAGE-QUALIFIED because multiple example crates share the `modal_runner`
    # bin name. burn-cuda pulls cudarc with dynamic loading, so this builds with
    # NO CUDA toolkit needed at build time. All logs -> stderr (stdout stays a
    # single JSON envelope, runner seam §2.2).
    build = subprocess.run(
        [
            "cargo", "build", "--release",
            "-p", "example-burn-add", "--bin", "modal_runner",
        ],
        cwd=build_dir,
        env=env,
        stdout=sys.stderr,
        stderr=sys.stderr,
    )
    if build.returncode != 0:
        raise RuntimeError(f"cargo build failed with exit code {build.returncode}")

    runner = "/tmp/target/release/modal_runner"

    with open("/tmp/in.json", "w") as f:
        f.write(input_json)

    proc = subprocess.run(
        [runner, "--entrypoint", entrypoint, "--input-file", "/tmp/in.json"],
        capture_output=True,
        text=True,
        env=env,
    )
    if proc.stderr:
        print(proc.stderr, file=sys.stderr)
    print(f"[m13] modal_runner exit={proc.returncode}", file=sys.stderr)
    return proc.stdout.strip()


@app.local_entrypoint()
def burn_add(entrypoint: str = "burn_add", input_json: str = '{"n":256}'):
    print(burn_add_runner.remote(entrypoint, input_json))
