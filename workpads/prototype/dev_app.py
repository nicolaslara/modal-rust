"""modal-rust prototype dev shim (the `run` / dev path).

This is the single generated-shim control plane that every later prototype
milestone reuses with ONE `modal run` call. It is authored by hand here; the
`modal-rust` CLI (M9a) must generate a byte-equivalent copy (modulo injected
params: entrypoint name, input path, app name, gpu/timeout, RUST_VER pin, and
the local source path).

Contract source of truth:
  - workpads/architecture/boundaries.md  (§4 build boundary, §5 shims, §10 ignore)
  - workpads/prototype/tasks.md          (M1 control, M2 mount, M3 toolchain, M4 run)

Design stances honored:
  1. Direct-execution-first; Sandbox is a documented fallback. Everything here
     runs on a normal `@app.function` (no Sandbox).
  2. The build boundary is the product. THIS is the `run` (dev) side: source is
     mounted at startup via `add_local_dir(copy=False)` and `cargo build` runs in
     the FUNCTION BODY at execution time (NOT at image-build time). The deploy
     side (build-at-image-time, never `cargo` at call time) lives in deploy_app.py.

Flag-mapping (tasks.md, authoritative):
  - `modal run` auto-binds CLI flags ONLY to a `@app.local_entrypoint()`, by
    parameter name. A bare `@app.function` does NOT bind flags.
  - The run path is driven by `main(entrypoint, input_json)` -> `--entrypoint` /
    `--input-json`. Probe/diagnostic targets (M2/M3/control) are selected with the
    `::<name>` selector that names a `@app.local_entrypoint()` of that name.

Targets (each = ONE `modal run` call for a later milestone):
  control       (M1, base image): runs `uname -a` via subprocess; auth + control plane.
  mount         (M2, mounted):    find /src + sha256sum Cargo.toml + write-probe.
  toolchain     (M3, mounted):    cargo/rustc/python --version + which -a python python3.
  run_entrypoint / main (M4, mounted, timeout=1800): cargo build + exec modal_runner.
"""

import json
import subprocess

import modal

# --- injected params (the CLI normalizes these for the M9a byte-equivalence diff) ---
APP_NAME = "modal-rust-poc-dev"
RUST_VER = "1"
LOCAL_SRC = "/Users/nicolas/devel/modal-rust"
REMOTE_SRC = "/src"

app = modal.App(APP_NAME)

# Base image: the Rust toolchain image with Modal's mandatory Python runtime.
#   - `add_python="3.12"` is mandatory: a bare `rust:` image is an invalid Function
#     image (boundaries.md §5).
#   - `.entrypoint([])` neutralizes the base ENTRYPOINT so Modal's Python runtime
#     can start.
#   - `RUST_BACKTRACE=1` so the runner's `panic` envelope carries a backtrace (§2).
base_image = (
    modal.Image.from_registry(f"rust:{RUST_VER}-slim", add_python="3.12")
    .entrypoint([])
    .env({"RUST_BACKTRACE": "1"})
)

# Mounted image: base + the repo source mounted (NOT copied) at startup. `copy=False`
# means the current local source is re-uploaded on each run (dev reactivity, M5), and
# `cargo build` happens in the function body, never in an image layer.
# Ignore rules per boundaries.md §10 (keep the upload minimal): build artifacts,
# git metadata, generated shims, and stray rlibs.
mounted_image = base_image.add_local_dir(
    LOCAL_SRC,
    REMOTE_SRC,
    copy=False,
    ignore=["target", ".git", ".modal-rust", "**/*.rlib"],
)


# --------------------------------------------------------------------------- M1
# Control path: base image only, no Rust source, no build. Proves Modal auth, app
# authoring, subprocess execution, and result marshalling end to end.
@app.function(image=base_image)
def control() -> str:
    proc = subprocess.run(["uname", "-a"], capture_output=True, text=True)
    return proc.stdout.strip()


@app.local_entrypoint()
def control_main():
    print(control.remote())


# --------------------------------------------------------------------------- M2
# Source-mount probe: lists the mounted tree (ignore patterns applied client-side),
# hashes Cargo.toml for a byte-identity check, and write-probes the mount. The
# write-probe result gates M4's build location (in-place vs cp-to-/tmp/build).
@app.function(image=mounted_image)
def mount() -> str:
    listing = subprocess.run(
        ["bash", "-c", f"find {REMOTE_SRC} -maxdepth 2 -type f | sort"],
        capture_output=True,
        text=True,
    ).stdout
    sha = subprocess.run(
        ["sha256sum", f"{REMOTE_SRC}/Cargo.toml"],
        capture_output=True,
        text=True,
    ).stdout.strip()
    write_probe = subprocess.run(
        ["bash", "-c", f"touch {REMOTE_SRC}/.wp 2>&1 || echo READONLY"],
        capture_output=True,
        text=True,
    ).stdout.strip()
    return (
        f"=== find {REMOTE_SRC} -maxdepth 2 -type f ===\n{listing}\n"
        f"=== sha256sum {REMOTE_SRC}/Cargo.toml ===\n{sha}\n"
        f"=== write-probe (touch {REMOTE_SRC}/.wp) ===\n{write_probe or 'WRITABLE'}"
    )


@app.local_entrypoint()
def mount_main():
    print(mount.remote())


# --------------------------------------------------------------------------- M3
# Toolchain probe: proves one image hosts BOTH the Rust toolchain AND Modal's
# Python runtime, and that `add_python` + any system python3 coexist cleanly.
#
# Selector binding (verified empirically against modal 1.3.x): `modal run X::NAME`
# matches the *Python function name* of a @app.local_entrypoint(); it does NOT
# match a function/entrypoint registered tag. A bare @app.function and a
# local_entrypoint cannot share a Python name (the second `def` shadows the first,
# so `.remote` is lost). Therefore the local_entrypoint that the `::toolchain_probe`
# selector binds is Python-named `toolchain_probe`; the bare @app.function body it
# invokes is `_toolchain_probe_fn` (distinct name so its `.remote` stays callable).
@app.function(image=mounted_image)
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


# --------------------------------------------------------------------------- M4
# THE central validation: a normal @app.function builds the mounted Rust source in
# its body at execution time, execs the freshly built modal_runner via the M0
# protocol, and returns the single stdout JSON envelope.
#
# Build location (boundaries.md §4): build into a known-writable LOCAL path.
#   - CARGO_HOME=/tmp/cargo, CARGO_TARGET_DIR=/tmp/target  (NOT a Volume).
#   - If the mount is read-only, cp -a /src -> /tmp/build and build there; else
#     build in-place in /src. Either way the run-vs-deploy split is unchanged.
@app.function(image=mounted_image, timeout=1800)
def run_entrypoint(entrypoint: str, input_json: str) -> str:
    import os
    import shutil
    import sys

    env = dict(os.environ)
    env["CARGO_HOME"] = "/tmp/cargo"
    env["CARGO_TARGET_DIR"] = "/tmp/target"
    env["RUST_BACKTRACE"] = "1"

    # Build location derived from mount writability (the M2 probe result).
    if os.access(REMOTE_SRC, os.W_OK):
        build_dir = REMOTE_SRC
        print(f"[run] mount {REMOTE_SRC} is writable; building in place", file=sys.stderr)
    else:
        build_dir = "/tmp/build"
        print(
            f"[run] mount {REMOTE_SRC} is read-only; cp -a {REMOTE_SRC} {build_dir}",
            file=sys.stderr,
        )
        if os.path.exists(build_dir):
            shutil.rmtree(build_dir)
        # cp -a preserves attributes; equivalent to shutil.copytree with symlinks.
        subprocess.run(["cp", "-a", REMOTE_SRC, build_dir], check=True)

    # cargo build --release --bin modal_runner; all logs -> stderr (stdout stays a
    # single JSON envelope, per the runner seam §2.2).
    build = subprocess.run(
        ["cargo", "build", "--release", "--bin", "modal_runner"],
        cwd=build_dir,
        env=env,
        stdout=sys.stderr,
        stderr=sys.stderr,
    )
    if build.returncode != 0:
        raise RuntimeError(f"cargo build failed with exit code {build.returncode}")

    runner = "/tmp/target/release/modal_runner"

    # Write the input to /tmp/in.json and feed the runner via --input-file (avoids
    # argv-length limits / the gRPC payload ceiling; §2.2).
    with open("/tmp/in.json", "w") as f:
        f.write(input_json)

    proc = subprocess.run(
        [runner, "--entrypoint", entrypoint, "--input-file", "/tmp/in.json"],
        capture_output=True,
        text=True,
        env=env,
    )
    # Runner diagnostics (if any) go to the function's stderr; stdout is the one
    # JSON envelope line we return.
    if proc.stderr:
        print(proc.stderr, file=sys.stderr)
    print(f"[run] modal_runner exit={proc.returncode}", file=sys.stderr)
    return proc.stdout.strip()


@app.local_entrypoint()
def main(entrypoint: str = "add", input_json: str = '{"a":40,"b":2}'):
    print(run_entrypoint.remote(entrypoint, input_json))
