"""modal-rust M3 negative-control shim.

Same toolchain probe as `dev_app.py`, but the base image is a BARE
`rust:1-slim` WITHOUT `add_python` — to demonstrate that `add_python` is
mandatory: a bare `rust:` image is an invalid Modal Function image because it
lacks the Python runtime Modal needs to start the container (boundaries.md §5).

Expected result: the Function fails to start / the run errors. If it
unexpectedly works, that is recorded per the M3 acceptance.

Selector: `modal run dev_app_no_python.py::toolchain_probe` binds the
local_entrypoint (Python-named `toolchain_probe`), which calls the bare
@app.function body `_toolchain_probe_fn.remote()`.
"""

import subprocess

import modal

APP_NAME = "modal-rust-poc-dev-no-python"
RUST_VER = "1"

app = modal.App(APP_NAME)

# NEGATIVE CONTROL: NO add_python. A bare rust: image lacks Modal's Python runtime.
no_python_image = (
    modal.Image.from_registry(f"rust:{RUST_VER}-slim")
    .entrypoint([])
    .env({"RUST_BACKTRACE": "1"})
)


@app.function(image=no_python_image)
def _toolchain_probe_fn() -> str:
    def cap(cmd):
        p = subprocess.run(cmd, capture_output=True, text=True)
        return (p.stdout + p.stderr).strip()

    return (
        f"cargo:  {cap(['cargo', '--version'])}\n"
        f"rustc:  {cap(['rustc', '--version'])}\n"
        f"python: {cap(['python', '--version'])}\n"
        f"which -a python python3:\n"
        f"{cap(['bash', '-c', 'which -a python python3'])}"
    )


@app.local_entrypoint()
def toolchain_probe():
    print(_toolchain_probe_fn.remote())
