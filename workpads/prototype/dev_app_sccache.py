"""modal-rust prototype dev shim — sccache-backed `run` path (M6b).

This is a variant of `dev_app.py`'s M4 `run` (dev) shim. It keeps the SAME
mounted-workspace control plane (`add_local_dir(copy=False)`, build in the
FUNCTION BODY at execution time, exec the freshly built `modal_runner`, return
the single stdout JSON envelope), but swaps the cache strategy to address M6's
null/neutral result.

Why this exists (M6 -> M6b, tasks.md M6/M6b + boundaries.md §7):
  M6 mounted a Volume for CARGO_HOME (crates.io index + downloaded `.crate`
  tarballs) but kept CARGO_TARGET_DIR ephemeral on /tmp. So a warm rebuild only
  saved DOWNLOAD/index time, never COMPILE time -> the measured warm speedup was
  null/neutral. M6b's goal is to cache COMPILED ARTIFACTS so a warm `run` rebuild
  can SKIP recompiling unchanged crates.

Approach (preferred per the task brief + boundaries.md §7):
  Use `sccache` as `RUSTC_WRAPPER`, with `SCCACHE_DIR` on a Modal Volume. sccache
  is CONTENT-ADDRESSABLE: it hashes each rustc invocation (compiler + flags +
  preprocessed source) and stores the resulting object under that hash. This
  sidesteps the reasons a Volume `CARGO_TARGET_DIR` was rejected in §4/§7:
    - No cargo single-writer target-dir + network-FS lock contention: sccache
      writes immutable content-addressed entries, not cargo's many small
      stat/lock/rename ops into a shared target tree.
    - CARGO_TARGET_DIR stays on local /tmp (fast), so cargo's hot path is local;
      only the (read-mostly, content-addressed) compiled objects live on the
      Volume.

Correctness first (boundaries.md §7 "cache state is advisory"):
  sccache only ever substitutes a cached object when the FULL rustc input hash
  matches (compiler binary, target, flags, and preprocessed source). A cache
  MISS recompiles from scratch -> a miss only costs time, never a wrong result.
  `RUSTC_WRAPPER` is transparent to cargo; if sccache fails it errors loudly
  rather than silently returning a wrong object.

Build boundary unchanged (boundaries.md §4): this is the `run` (dev) side only —
  source is mounted at startup (`copy=False`) and `cargo build` runs in the
  function body at execution time. Deploy still bakes the binary at image-build
  time and never runs cargo at call time (deploy_app.py / call_app.py).

Flag-mapping (tasks.md, authoritative): `modal run` auto-binds CLI flags only to
  a `@app.local_entrypoint()`, by parameter name. The run path is driven by
  `main(entrypoint, input_json)` -> `--entrypoint` / `--input-json`, forwarding
  to the bare `@app.function` body via `.remote()`.

Acceptance (M6b sccache experiment): a single
  `modal run dev_app_sccache.py::main` returns
  `{"ok":true,"value":{"sum":42}}` and `sccache --show-stats` is printed to
  stderr so cache hits are visible. Cold vs warm wall-clock + the warm compile
  speedup (or an honestly-recorded null/neutral) is the experimental deliverable.
"""

import subprocess

import modal

# --- injected params (the CLI would normalize these for the M9a diff) ---
APP_NAME = "modal-rust-poc-dev-sccache"
RUST_VER = "1"
LOCAL_SRC = "/Users/nicolas/devel/modal-rust"
# The cargo PACKAGE to build (`-p <pkg>`). Required because multiple workspace
# members share the `modal_runner` bin name, so a bare `--bin modal_runner` is
# AMBIGUOUS; the CLI derives this from the `--project`'s `[package].name`.
PACKAGE = "example-add"
REMOTE_SRC = "/src"

# Prebuilt sccache: a STATIC x86_64 musl binary from the upstream GitHub release.
# Installed via `curl | tar` at IMAGE-BUILD time (FAST — NOT `cargo install
# sccache`, which would compile sccache + its large dep graph from source). musl
# static linkage means it runs unchanged on the Debian-based `rust:*-slim` image.
SCCACHE_VER = "v0.15.0"
SCCACHE_TARBALL = f"sccache-{SCCACHE_VER}-x86_64-unknown-linux-musl"
SCCACHE_URL = (
    f"https://github.com/mozilla/sccache/releases/download/{SCCACHE_VER}/"
    f"{SCCACHE_TARBALL}.tar.gz"
)

# Modal Volume holding the sccache content-addressable object cache (and
# CARGO_HOME downloads). `create_if_missing=True` so the first (cold) run creates
# an EMPTY volume; the second (warm) run sees the objects sccache wrote. Mounted
# at a STABLE path held constant across cold + warm runs.
SCCACHE_VOLUME = modal.Volume.from_name(
    "modal-rust-sccache", create_if_missing=True
)
CACHE_MOUNT = "/cache"
SCCACHE_DIR = f"{CACHE_MOUNT}/sccache"  # on the Volume (content-addressable objects)
CARGO_HOME = f"{CACHE_MOUNT}/cargo"  # on the Volume (registry index + .crate downloads)
SCCACHE_BIN = "/usr/local/bin/sccache"

app = modal.App(APP_NAME)

# Base image: Rust toolchain + Modal's mandatory Python runtime, + the prebuilt
# sccache binary installed into /usr/local/bin at image-build time.
#   - `add_python="3.12"` mandatory (a bare `rust:` image is an invalid Function
#     image; boundaries.md §5).
#   - `.entrypoint([])` neutralizes the base ENTRYPOINT so Modal's Python runtime
#     starts.
#   - `RUST_BACKTRACE=1` so the runner's `panic` envelope carries a backtrace (§2).
#   - The sccache install is a single fast `run_commands` (download + untar +
#     chmod); no compilation.
base_image = (
    modal.Image.from_registry(f"rust:{RUST_VER}-slim", add_python="3.12")
    .entrypoint([])
    .env({"RUST_BACKTRACE": "1"})
    .apt_install("curl", "ca-certificates")
    .run_commands(
        f"curl -sSfL {SCCACHE_URL} -o /tmp/sccache.tar.gz",
        f"tar -xzf /tmp/sccache.tar.gz -C /tmp",
        f"install -m 0755 /tmp/{SCCACHE_TARBALL}/sccache {SCCACHE_BIN}",
        f"{SCCACHE_BIN} --version",
    )
)

# Mounted image: base + the repo source mounted (NOT copied) at startup.
# `copy=False` re-uploads current local source each run (dev reactivity, M5), and
# `cargo build` happens in the function body. Ignore rules per boundaries.md §10.
mounted_image = base_image.add_local_dir(
    LOCAL_SRC,
    REMOTE_SRC,
    copy=False,
    ignore=["target", ".git", ".modal-rust", "**/*.rlib"],
)


@app.function(
    image=mounted_image,
    timeout=1800,
    volumes={CACHE_MOUNT: SCCACHE_VOLUME},
)
def run_entrypoint(entrypoint: str, input_json: str) -> str:
    """Build the mounted Rust source in-body with sccache as RUSTC_WRAPPER, exec
    the freshly built modal_runner, and return the single stdout JSON envelope."""
    import os
    import shutil
    import sys
    import time

    env = dict(os.environ)
    # --- sccache wiring (the M6b core) ---
    # RUSTC_WRAPPER makes cargo invoke `sccache rustc ...` for every crate. sccache
    # hashes the full rustc invocation and serves a cached object on an exact hash
    # match (content-addressable), else compiles and stores the result.
    env["RUSTC_WRAPPER"] = SCCACHE_BIN
    env["SCCACHE_DIR"] = SCCACHE_DIR  # content-addressable objects on the Volume
    # Generous cap so the warm run isn't evicting its own objects on this tiny
    # graph (and headroom for larger dep graphs later).
    env["SCCACHE_CACHE_SIZE"] = "10G"
    # Be explicit that the local-disk backend is used (the SCCACHE_DIR cache),
    # never an inherited cloud backend from the ambient env.
    env.pop("SCCACHE_BUCKET", None)
    env.pop("SCCACHE_GCS_BUCKET", None)
    env.pop("SCCACHE_REDIS", None)
    # CARGO_HOME on the Volume too (warms registry index + .crate downloads, as in
    # M6). CARGO_TARGET_DIR stays on LOCAL /tmp so cargo's many small stat/lock
    # ops stay off the network FS (boundaries.md §4/§7); only the content-addressed
    # compiled objects (via sccache) live on the Volume.
    env["CARGO_HOME"] = CARGO_HOME
    env["CARGO_TARGET_DIR"] = "/tmp/target"
    env["RUST_BACKTRACE"] = "1"
    os.makedirs(SCCACHE_DIR, exist_ok=True)
    os.makedirs(CARGO_HOME, exist_ok=True)

    # Report cache warmth as an explicit signal (cold = empty SCCACHE_DIR).
    warm = os.path.isdir(SCCACHE_DIR) and bool(os.listdir(SCCACHE_DIR))
    print(
        f"[run-sccache] SCCACHE_DIR={SCCACHE_DIR} on Volume 'modal-rust-sccache' "
        f"at {CACHE_MOUNT}; cache {'WARM' if warm else 'COLD (empty)'}",
        file=sys.stderr,
    )

    # Start the sccache server explicitly so it picks up SCCACHE_DIR/SCCACHE_CACHE_SIZE
    # from our env (the server reads config at start). Each Modal container is fresh,
    # so no stale server with a different cache dir can be running. Zero the in-process
    # stats counters first so the printed stats reflect THIS build only.
    subprocess.run([SCCACHE_BIN, "--stop-server"], env=env,
                   stdout=sys.stderr, stderr=sys.stderr)
    start = subprocess.run([SCCACHE_BIN, "--start-server"], env=env,
                           capture_output=True, text=True)
    print(f"[run-sccache] start-server rc={start.returncode} "
          f"{(start.stdout + start.stderr).strip()}", file=sys.stderr)

    # Build location derived from mount writability (M2 probe = WRITABLE in place).
    if os.access(REMOTE_SRC, os.W_OK):
        build_dir = REMOTE_SRC
        print(f"[run-sccache] mount {REMOTE_SRC} writable; building in place",
              file=sys.stderr)
    else:
        build_dir = "/tmp/build"
        print(f"[run-sccache] mount {REMOTE_SRC} read-only; cp -a {REMOTE_SRC} {build_dir}",
              file=sys.stderr)
        if os.path.exists(build_dir):
            shutil.rmtree(build_dir)
        subprocess.run(["cp", "-a", REMOTE_SRC, build_dir], check=True)

    # cargo build --release -p <pkg> --bin modal_runner; logs -> stderr (stdout
    # stays one JSON envelope). `-p PACKAGE` disambiguates the shared `modal_runner`
    # bin across workspace members. Timed so cold-vs-warm wall-clock is in-band.
    t0 = time.monotonic()
    build = subprocess.run(
        ["cargo", "build", "--release", "-p", PACKAGE, "--bin", "modal_runner"],
        cwd=build_dir,
        env=env,
        stdout=sys.stderr,
        stderr=sys.stderr,
    )
    build_secs = time.monotonic() - t0
    if build.returncode != 0:
        # Show stats even on failure so a sccache misconfiguration is diagnosable.
        subprocess.run([SCCACHE_BIN, "--show-stats"], env=env,
                       stdout=sys.stderr, stderr=sys.stderr)
        raise RuntimeError(f"cargo build failed with exit code {build.returncode}")
    print(f"[run-sccache] cargo build wall-clock: {build_secs:.2f}s", file=sys.stderr)

    # Print sccache stats to stderr so cache hits/misses are VISIBLE (acceptance).
    # A warm run should show non-zero "Cache hits"; a cold run is all misses.
    print("[run-sccache] === sccache --show-stats ===", file=sys.stderr)
    subprocess.run([SCCACHE_BIN, "--show-stats"], env=env,
                   stdout=sys.stderr, stderr=sys.stderr)

    runner = "/tmp/target/release/modal_runner"

    # Write the input to /tmp/in.json and feed the runner via --input-file
    # (avoids argv-length limits / the gRPC payload ceiling; §2.2).
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
    print(f"[run-sccache] modal_runner exit={proc.returncode}", file=sys.stderr)

    # Stop the server so its final flush to SCCACHE_DIR completes BEFORE the
    # container shuts down (cargo has exited and all file handles are closed, so
    # this is the safe point — boundaries.md §7). Modal's automatic background +
    # shutdown commits then persist the Volume; deliberately NO vol.reload() on the
    # build path (cargo holds locks during the build -> "volume busy").
    subprocess.run([SCCACHE_BIN, "--stop-server"], env=env,
                   stdout=sys.stderr, stderr=sys.stderr)
    return proc.stdout.strip()


@app.local_entrypoint()
def main(entrypoint: str = "add", input_json: str = '{"a":40,"b":2}'):
    print(run_entrypoint.remote(entrypoint, input_json))
