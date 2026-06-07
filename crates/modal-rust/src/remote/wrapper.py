"""modal-rust FILE-mode run wrapper.

Baked to /root/modal_rust_run_wrapper.py. Builds the mounted Rust crate in the
function body (run boundary: cargo at execution time, never at image-build time),
execs the frozen modal_runner, and returns the one-line JSON envelope verbatim.
"""

import base64
import json
import os
import shutil
import subprocess
import sys

_CONFIG_ENV = "MODAL_RUST_RUN_CONFIG_JSON_B64"
_DEFAULT_REMOTE_SRC = "/src"
_DEFAULT_CACHE_MOUNT = "/cache"
_DEFAULT_CACHE_ARCHIVE_NAME = "cache.tar.zst"
_RUNNER = "/tmp/target/release/modal_runner"
_MARKER = "/tmp/.modal_rust_built"
_BUILT = False

# ONE persistent `modal_runner --serve` child per warm container (cls-design.md §2.1,
# Option 2a). Built lazily on the first call and reused for every later call so a Rust
# `OnceLock` singleton (an entered `Cls` struct) survives across calls — `#[enter]`
# runs ONCE per warm container. Module-global, exactly like `_BUILT`/`_MARKER`. This is
# ADDITIVE: the per-call `(entrypoint, input_json) -> envelope` contract is unchanged
# (serve just frames the same request + the SAME one-line envelope over a pipe), and
# the cold one-shot fallback (`_run_one_shot`) stays byte-identical to before.
_SERVE = None

# Lock files regenerate; excluding them avoids stale-lock churn in the archive.
_PACK_EXCLUDES = [
    "--exclude=cargo/registry/cache/.package-cache",
    "--exclude=cargo/.package-cache",
]


def _require_str(config, key, default=None):
    value = config.get(key, default)
    if not isinstance(value, str) or not value:
        raise RuntimeError(f"run wrapper config field {key!r} must be a non-empty string")
    return value


def _load_config():
    raw = os.environ.get(_CONFIG_ENV)
    if not raw:
        raise RuntimeError(f"missing required run wrapper config env {_CONFIG_ENV}")
    try:
        decoded = base64.b64decode(raw).decode("utf-8")
        config = json.loads(decoded)
    except Exception as e:
        raise RuntimeError(f"failed to decode run wrapper config env {_CONFIG_ENV}: {e!r}") from e
    if not isinstance(config, dict):
        raise RuntimeError("run wrapper config must decode to a JSON object")

    cache = config.get("cache", False)
    if not isinstance(cache, bool):
        raise RuntimeError("run wrapper config field 'cache' must be a bool")

    return {
        "package": _require_str(config, "package"),
        "cache": cache,
        "remote_src": _require_str(config, "remote_src", _DEFAULT_REMOTE_SRC),
        "cache_mount": _require_str(config, "cache_mount", _DEFAULT_CACHE_MOUNT),
        "cache_archive_name": _require_str(
            config, "cache_archive_name", _DEFAULT_CACHE_ARCHIVE_NAME
        ),
    }


def _archive_paths(cache_mount, archive_name):
    archive_zstd = f"{cache_mount}/{archive_name}"
    if archive_zstd.endswith(".zst"):
        archive_gzip = archive_zstd[: -len(".zst")] + ".gz"
    else:
        archive_gzip = archive_zstd + ".gz"
    return archive_zstd, archive_gzip


_CONFIG = _load_config()
PACKAGE = _CONFIG["package"]
CACHE_ON = _CONFIG["cache"]
REMOTE_SRC = _CONFIG["remote_src"]
_ARCHIVE_ZSTD, _ARCHIVE_GZIP = _archive_paths(
    _CONFIG["cache_mount"], _CONFIG["cache_archive_name"]
)


def _cache_target_on():
    # Optionally archive target/ too. Gated by env, default OFF in v0.
    return os.environ.get("MODAL_RUST_CACHE_TARGET", "").strip().lower() in (
        "1",
        "true",
        "yes",
        "on",
    )


def _existing_archive():
    # Prefer an archive that already exists (keeps cold/warm consistent in a volume).
    if os.path.exists(_ARCHIVE_ZSTD):
        return _ARCHIVE_ZSTD
    if os.path.exists(_ARCHIVE_GZIP):
        return _ARCHIVE_GZIP
    return None


def _unpack_cache():
    # Restore warm CARGO_HOME (and optionally target/) onto /tmp before cargo runs.
    # A missing/corrupt archive is treated as cold; it only costs time.
    if not CACHE_ON:
        return "disabled"
    archive = _existing_archive()
    if archive is None:
        return "COLD (no archive)"
    flag = "--zstd" if archive.endswith(".zst") else "-z"
    try:
        subprocess.run(
            ["tar", flag, "-xf", archive, "-C", "/tmp"],
            check=True,
            stdout=sys.stderr,
            stderr=sys.stderr,
        )
        return "WARM"
    except Exception as e:
        print(f"[cache] unpack failed (treated as COLD): {e!r}", file=sys.stderr)
        return "COLD (unpack failed)"


def _pack_one(archive, flag, dirs):
    tmp = archive + ".tmp"
    try:
        subprocess.run(
            ["tar", flag, *_PACK_EXCLUDES, "-cf", tmp, "-C", "/tmp", *dirs],
            check=True,
            stdout=sys.stderr,
            stderr=sys.stderr,
        )
    except Exception:
        if os.path.exists(tmp):
            os.remove(tmp)
        raise
    os.replace(tmp, archive)
    print(f"[cache] packed {archive}", file=sys.stderr)


def _pack_cache():
    # Persist the enriched archive after the first cold build only. A failed pack
    # must never fail the call.
    if not CACHE_ON:
        return
    dirs = ["cargo"]
    if _cache_target_on():
        dirs.append("target")
    existing = _existing_archive()
    try:
        if existing == _ARCHIVE_GZIP:
            _pack_one(_ARCHIVE_GZIP, "-z", dirs)
        else:
            try:
                _pack_one(_ARCHIVE_ZSTD, "--zstd", dirs)
            except Exception as e:
                print(
                    f"[cache] zstd pack unavailable ({e!r}); falling back to gzip",
                    file=sys.stderr,
                )
                _pack_one(_ARCHIVE_GZIP, "-z", dirs)
    except Exception as e:
        print(f"[cache] pack failed (ignored): {e!r}", file=sys.stderr)


def _env():
    e = dict(os.environ)
    e["CARGO_HOME"] = "/tmp/cargo"
    e["CARGO_TARGET_DIR"] = "/tmp/target"
    e["RUST_BACKTRACE"] = "1"
    return e


def _build_dir():
    if os.access(REMOTE_SRC, os.W_OK):
        print(f"[run] mount {REMOTE_SRC} writable; building in place", file=sys.stderr)
        return REMOTE_SRC
    build_dir = "/tmp/build"
    print(f"[run] mount {REMOTE_SRC} read-only; cp -a -> {build_dir}", file=sys.stderr)
    if os.path.exists(build_dir):
        shutil.rmtree(build_dir)
    subprocess.run(["cp", "-a", REMOTE_SRC, build_dir], check=True)
    return build_dir


def _build(env):
    global _BUILT
    if _BUILT or os.path.exists(_MARKER):
        _BUILT = True
        print("[run] build cached (warm container); skipping cargo build", file=sys.stderr)
        return
    print(f"[cache] {_unpack_cache()}", file=sys.stderr)
    build_dir = _build_dir()
    build = subprocess.run(
        ["cargo", "build", "--release", "-p", PACKAGE, "--bin", "modal_runner"],
        cwd=build_dir,
        env=env,
        capture_output=True,
        text=True,
    )
    if build.stdout:
        print(build.stdout, file=sys.stderr)
    if build.stderr:
        print(build.stderr, file=sys.stderr)
    if build.returncode != 0:
        tail = (build.stderr or build.stdout or "")[-1500:]
        raise RuntimeError(
            f"cargo build failed with exit code {build.returncode}; stderr tail:\n{tail}"
        )
    open(_MARKER, "w").close()
    _BUILT = True
    _pack_cache()


def _serve_enabled():
    # Default ON: routing every call through ONE persistent `modal_runner --serve`
    # child is what makes a `#[cls]` `#[enter]` run once per warm container, and it is
    # envelope-identical for plain `#[function]`s (the serve loop calls the SAME
    # handler + emits the SAME frozen envelope). An escape hatch forces the legacy
    # cold one-shot exec per call.
    return os.environ.get("MODAL_RUST_SERVE", "1").strip().lower() not in (
        "0",
        "false",
        "no",
        "off",
    )


def _serve_child(env):
    # Spawn (once) the long-lived `modal_runner --serve` child and reuse it. A dead
    # child (crashed process) is transparently respawned on the next call.
    global _SERVE
    if _SERVE is not None and _SERVE.poll() is None:
        return _SERVE
    print("[run] spawning persistent modal_runner --serve child", file=sys.stderr)
    _SERVE = subprocess.Popen(
        [_RUNNER, "--serve"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=sys.stderr,
        text=True,
        env=env,
    )
    return _SERVE


def _run_serve(entrypoint, input_json, env):
    # Frame ONE request as a single JSON line and read ONE envelope line back. The
    # request carries the SAME entrypoint + per-call input JSON the one-shot CLI takes;
    # the response is the SAME frozen one-line envelope. On any pipe/child failure, fall
    # back to a fresh one-shot exec (never lose a call to a broken serve child).
    proc = _serve_child(env)
    frame = json.dumps({"entrypoint": entrypoint, "input": input_json})
    try:
        proc.stdin.write(frame + "\n")
        proc.stdin.flush()
        line = proc.stdout.readline()
    except Exception as e:
        print(f"[run] serve child IO failed ({e!r}); falling back to one-shot", file=sys.stderr)
        global _SERVE
        _SERVE = None
        return _run_one_shot(entrypoint, input_json, env)
    out = line.strip()
    if not out:
        # The child closed its stdout (crash/EOF): drop it and fall back one-shot.
        print(
            f"[run] serve child produced no envelope (exit={proc.poll()}); "
            "falling back to one-shot",
            file=sys.stderr,
        )
        _SERVE = None
        return _run_one_shot(entrypoint, input_json, env)
    return out


def _run_one_shot(entrypoint, input_json, env):
    # The ORIGINAL cold path, byte-identical to before: exec a fresh `modal_runner`,
    # one envelope, then it exits. Used when serve is disabled or as the serve fallback.
    with open("/tmp/in.json", "w") as f:
        f.write(input_json)
    proc = subprocess.run(
        [_RUNNER, "--entrypoint", entrypoint, "--input-file", "/tmp/in.json"],
        capture_output=True,
        text=True,
        env=env,
    )
    if proc.stderr:
        print(proc.stderr, file=sys.stderr)
    print(f"[run] modal_runner exit={proc.returncode}", file=sys.stderr)
    out = proc.stdout.strip()
    if not out:
        raise RuntimeError(
            f"modal_runner produced no envelope; exit={proc.returncode}; "
            f"stderr tail: {proc.stderr[-500:]!r}"
        )
    return out


def handler(entrypoint, input_json):
    env = _env()
    _build(env)
    if _serve_enabled():
        return _run_serve(entrypoint, input_json, env)
    return _run_one_shot(entrypoint, input_json, env)
