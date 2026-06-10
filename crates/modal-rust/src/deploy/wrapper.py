"""modal-rust FILE-mode DEPLOY wrapper (ports deploy_app.py call_entrypoint).

Baked to /root/modal_rust_deploy_wrapper.py. Deployed-runtime invariant: this body
NEVER builds and NEVER mounts source. It execs ONLY the prebuilt /app/modal_runner
baked at IMAGE-BUILD time, and RETURNS the one-line JSON envelope verbatim (the
facade parses it).
"""
import json, os, subprocess, sys

_RUNNER = "/app/modal_runner"   # baked at IMAGE-BUILD time; never rebuilt

# ONE persistent `/app/modal_runner --serve` child per warm container so a `#[cls]`
# `#[enter]` runs once and is reused across calls (cls-design.md §2.1). Module-global,
# reused for the container's lifetime. ADDITIVE: the per-call envelope contract is
# unchanged and the cold one-shot fallback stays byte-identical.
_SERVE = None


def _serve_enabled():
    return os.environ.get("MODAL_RUST_SERVE", "1").strip().lower() not in (
        "0", "false", "no", "off",
    )


def _serve_child():
    global _SERVE
    if _SERVE is not None and _SERVE.poll() is None:
        return _SERVE
    print("[deploy] spawning persistent modal_runner --serve child", file=sys.stderr)
    _SERVE = subprocess.Popen(
        [_RUNNER, "--serve"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=sys.stderr, text=True,
    )
    return _SERVE


def _run_one_shot(entrypoint, input_json):
    with open("/tmp/in.json", "w") as f:
        f.write(input_json)
    proc = subprocess.run(
        [_RUNNER, "--entrypoint", entrypoint, "--input-file", "/tmp/in.json"],
        capture_output=True, text=True,
    )
    if proc.stderr:
        print(proc.stderr, file=sys.stderr)
    print(f"[deploy] modal_runner exit={proc.returncode}", file=sys.stderr)
    out = proc.stdout.strip()
    if not out:
        raise RuntimeError(
            f"modal_runner produced no envelope; exit={proc.returncode}; "
            f"stderr tail: {proc.stderr[-500:]!r}"
        )
    return out


def handler(entrypoint, input_json):
    if not _serve_enabled():
        return _run_one_shot(entrypoint, input_json)
    global _SERVE
    proc = _serve_child()
    frame = json.dumps({"entrypoint": entrypoint, "input": input_json})
    try:
        proc.stdin.write(frame + "\n")
        proc.stdin.flush()
        line = proc.stdout.readline()
    except Exception as e:
        print(f"[deploy] serve child IO failed ({e!r}); one-shot fallback", file=sys.stderr)
        _SERVE = None
        return _run_one_shot(entrypoint, input_json)
    out = line.strip()
    if not out:
        print(f"[deploy] serve child EOF (exit={proc.poll()}); one-shot fallback", file=sys.stderr)
        _SERVE = None
        return _run_one_shot(entrypoint, input_json)
    return out


def _snapshot_prime_enabled():
    return os.environ.get("MODAL_RUST_SNAPSHOT_PRIME", "").strip().lower() in (
        "1", "true", "yes", "on",
    )


def _snapshot_best_effort():
    # OPT-IN: degrade a FAILED prime to lazy `#[enter]` instead of failing loudly. Default
    # OFF (strict) — a broken prime must never be a hidden perf cliff. Baked from the
    # deploy-time MODAL_RUST_SNAPSHOT_BEST_EFFORT env when the operator opts in.
    return os.environ.get("MODAL_RUST_SNAPSHOT_BEST_EFFORT", "").strip().lower() in (
        "1", "true", "yes", "on",
    )


def _snapshot_prime_fail(msg, proc):
    # STRICT default: a failed prime FAILS LOUD — raise at import so the container fails to
    # boot and Modal surfaces it at DEPLOY time, instead of silently re-running `#[enter]`
    # on every cold start (a hidden perf cliff). The opt-in MODAL_RUST_SNAPSHOT_BEST_EFFORT
    # degrades to lazy `#[enter]` instead (the import continues; the first real request
    # runs `#[enter]` lazily). Drop the child so the lazy path respawns cleanly.
    global _SERVE
    if _snapshot_best_effort():
        print(f"[deploy] snapshot prime FAILED: {msg}; degrading to lazy #[enter] "
              "(MODAL_RUST_SNAPSHOT_BEST_EFFORT)", file=sys.stderr)
        _SERVE = None
        return
    raise RuntimeError(
        f"modal-rust memory-snapshot prime FAILED at container init: {msg}. Fix the "
        f"failing #[enter] (or the prime path), or set MODAL_RUST_SNAPSHOT_BEST_EFFORT=1 "
        "to degrade to lazy #[enter] instead of failing the deploy."
    )


def _snapshot_prime():
    # MODULE-GLOBAL eager prime (memory-snapshot v0 §6): runs at IMPORT, BEFORE Modal's
    # snapshot point, so the snapshot-enabled `#[cls]` `#[enter]` lands INSIDE the freeze
    # window and is restored (load-once-EVER) rather than re-run on every cold start.
    # Baked on ONLY when a deployed entrypoint opted into `enable_memory_snapshot`
    # (the MODAL_RUST_SNAPSHOT_PRIME ENV); off ⇒ no-op + byte-identical import.
    #
    # STRICT BY DEFAULT: any prime failure (IO error, missing/garbled ack, or a reported
    # `#[enter]` failure) FAILS LOUD via _snapshot_prime_fail. Opt into degrade-to-lazy
    # with MODAL_RUST_SNAPSHOT_BEST_EFFORT.
    if not (_serve_enabled() and _snapshot_prime_enabled()):
        return
    try:
        proc = _serve_child()
        proc.stdin.write(json.dumps({"kind": "prime"}) + "\n")
        proc.stdin.flush()
        ack = proc.stdout.readline().strip()
    except Exception as e:  # noqa: BLE001
        _snapshot_prime_fail(f"prime frame IO failed: {e!r}", _SERVE)
        return
    if not ack:
        _snapshot_prime_fail(f"runner produced no prime ack (child exit={proc.poll()})", proc)
        return
    try:
        report = json.loads(ack)
        failed = int(report.get("failed", 0))
        errors = report.get("errors", [])
    except Exception as e:  # noqa: BLE001
        _snapshot_prime_fail(f"unparseable prime ack {ack!r}: {e!r}", proc)
        return
    if failed:
        _snapshot_prime_fail(f"{failed} #[enter] prime(s) failed: {errors}", proc)
        return
    print(f"[deploy] snapshot prime ack: {ack!r}", file=sys.stderr)


_snapshot_prime()
