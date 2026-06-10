"""Offline behavior tests for the deploy wrapper (architecture review H3).

The wrapper (`wrapper.py`) is the ONLY Python that runs in a deployed container,
and before this file NO test ever executed it — py_compile only proved it parses.
These tests run the real module against a FAKE modal_runner (a tiny script speaking
the runner's stdin/stdout protocol), with `fastapi` faked in sys.modules, so they
need NOTHING beyond the standard library and pin:

- the envelope passthrough (serve frames + the one-shot fallback paths);
- the per-warm-container serve-child REUSE;
- the web handler's HTTP mapping — 200 value, 422 decode_error, 500 handler error —
  and that error bodies are REDACTED to exactly {kind, message} (review in-flight #1);
- the snapshot prime: strict-by-default RuntimeError at import on a failed prime,
  MODAL_RUST_SNAPSHOT_BEST_EFFORT degrade, and the primed child surviving into the
  request path (load-once compose).

Run directly (`python3 wrapper_test.py`) or via the cargo harness
(`tests/deploy_wrapper_py.rs`), which runs in the normal test gate.
"""
import asyncio
import importlib.util
import json
import os
import sys
import tempfile
import types
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
WRAPPER = os.path.join(HERE, "wrapper.py")

# The fake /app/modal_runner: speaks the runner protocol (--serve line frames with
# request/prime kinds; one-shot --entrypoint/--input-file) and answers from a fixed
# entrypoint->envelope table. "SECRET-TRACE" stands in for a backtrace that must
# NEVER reach an HTTP response body.
FAKE_RUNNER = """#!/usr/bin/env python3
import json, os, sys

def envelope_for(entrypoint, input_json):
    if entrypoint == "ok":
        return json.dumps({"ok": True, "value": {"echo": json.loads(input_json)}})
    if entrypoint == "decode":
        return json.dumps({"ok": False, "error": {
            "kind": "decode_error", "message": "bad input", "backtrace": "SECRET-TRACE"}})
    if entrypoint == "boom":
        return json.dumps({"ok": False, "error": {
            "kind": "handler_error", "message": "kaboom", "backtrace": "SECRET-TRACE"}})
    return json.dumps({"ok": True, "value": entrypoint})

args = sys.argv[1:]
if args == ["--serve"]:
    if os.environ.get("FAKE_RUNNER_EXIT_IMMEDIATELY"):
        sys.exit(0)
    for line in sys.stdin:
        frame = json.loads(line)
        if frame.get("kind") == "prime":
            print(os.environ.get(
                "FAKE_RUNNER_PRIME_ACK",
                json.dumps({"primed": 1, "failed": 0, "errors": []})), flush=True)
            continue
        print(envelope_for(frame["entrypoint"], frame["input"]), flush=True)
    sys.exit(0)
ep = args[args.index("--entrypoint") + 1]
with open(args[args.index("--input-file") + 1]) as f:
    print(envelope_for(ep, f.read()))
"""


class FakeRequest:
    """Just enough of fastapi.Request for _make_web_handler: an async body()."""

    def __init__(self, body: bytes):
        self._body = body

    async def body(self):
        return self._body


class FakeResponse:
    """Just enough of fastapi.Response: captures content/status/media_type."""

    def __init__(self, content=None, status_code=200, media_type=None):
        self.body = content
        self.status_code = status_code
        self.media_type = media_type


def install_fake_fastapi():
    mod = types.ModuleType("fastapi")
    mod.Request = FakeRequest
    mod.Response = FakeResponse
    sys.modules["fastapi"] = mod


def load_wrapper():
    """Import wrapper.py fresh (its module-bottom _snapshot_prime() runs HERE,
    under whatever env the test set — exactly like the deployed import)."""
    spec = importlib.util.spec_from_file_location("wrapper_under_test", WRAPPER)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


ENV_KEYS = (
    "MODAL_RUST_SERVE",
    "MODAL_RUST_SNAPSHOT_PRIME",
    "MODAL_RUST_SNAPSHOT_BEST_EFFORT",
    "MODAL_RUST_RUNNER",
    "FAKE_RUNNER_EXIT_IMMEDIATELY",
    "FAKE_RUNNER_PRIME_ACK",
)


class WrapperTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        install_fake_fastapi()
        cls.tmp = tempfile.mkdtemp(prefix="wrapper-test-")
        cls.runner = os.path.join(cls.tmp, "fake_modal_runner")
        with open(cls.runner, "w") as f:
            f.write(FAKE_RUNNER)
        os.chmod(cls.runner, 0o755)

    def setUp(self):
        for key in ENV_KEYS:
            os.environ.pop(key, None)
        os.environ["MODAL_RUST_RUNNER"] = self.runner
        self.w = None

    def tearDown(self):
        serve = getattr(self.w, "_SERVE", None) if self.w else None
        if serve is not None and serve.poll() is None:
            serve.kill()
            serve.wait()

    def load(self):
        self.w = load_wrapper()
        return self.w

    # --- envelope passthrough -------------------------------------------------

    def test_serve_round_trip_reuses_one_child(self):
        w = self.load()
        env = json.loads(w.handler("ok", '{"a": 1}'))
        self.assertEqual(env, {"ok": True, "value": {"echo": {"a": 1}}})
        child = w._SERVE
        self.assertIsNotNone(child)
        json.loads(w.handler("ok", '{"a": 2}'))
        self.assertIs(w._SERVE, child, "warm container must reuse ONE serve child")

    def test_one_shot_when_serve_disabled(self):
        os.environ["MODAL_RUST_SERVE"] = "0"
        w = self.load()
        env = json.loads(w.handler("ok", '{"a": 3}'))
        self.assertEqual(env["value"], {"echo": {"a": 3}})
        self.assertIsNone(w._SERVE, "serve disabled: no persistent child")

    def test_serve_child_eof_falls_back_to_one_shot(self):
        os.environ["FAKE_RUNNER_EXIT_IMMEDIATELY"] = "1"
        w = self.load()
        env = json.loads(w.handler("ok", '{"a": 4}'))
        self.assertEqual(env["value"], {"echo": {"a": 4}},
                         "a dead serve child must degrade to the one-shot path")
        self.assertIsNone(w._SERVE, "dead child dropped so the next call respawns")

    # --- web handler HTTP mapping (review in-flight #1: redaction) -------------

    def call_web(self, entrypoint, body: bytes):
        web = self.w._make_web_handler(entrypoint)
        return asyncio.run(web(FakeRequest(body)))

    def test_web_ok_returns_decoded_value(self):
        self.load()
        out = self.call_web("ok", b'{"a": 5}')
        self.assertEqual(out, {"echo": {"a": 5}})

    def test_web_empty_body_defaults_to_empty_object(self):
        self.load()
        out = self.call_web("ok", b"")
        self.assertEqual(out, {"echo": {}})

    def test_web_decode_error_is_422_and_redacted(self):
        self.load()
        resp = self.call_web("decode", b"not json")
        self.assertEqual(resp.status_code, 422)
        self.assertEqual(resp.media_type, "application/json")
        # EXACTLY {kind, message} — the backtrace must never reach an HTTP caller.
        self.assertEqual(json.loads(resp.body),
                         {"kind": "decode_error", "message": "bad input"})
        self.assertNotIn("SECRET-TRACE", resp.body)

    def test_web_handler_error_is_500_and_redacted(self):
        self.load()
        resp = self.call_web("boom", b'{"a": 6}')
        self.assertEqual(resp.status_code, 500)
        self.assertEqual(json.loads(resp.body),
                         {"kind": "handler_error", "message": "kaboom"})
        self.assertNotIn("SECRET-TRACE", resp.body)

    # --- snapshot prime (strict default / best-effort opt-in) ------------------

    def test_prime_failure_raises_at_import_by_default(self):
        os.environ["MODAL_RUST_SNAPSHOT_PRIME"] = "1"
        os.environ["FAKE_RUNNER_PRIME_ACK"] = json.dumps(
            {"primed": 0, "failed": 1, "errors": ["enter blew up"]})
        with self.assertRaises(RuntimeError) as ctx:
            self.load()
        self.assertIn("MODAL_RUST_SNAPSHOT_BEST_EFFORT", str(ctx.exception),
                      "the failure must tell the operator about the opt-out")

    def test_prime_failure_degrades_under_best_effort(self):
        os.environ["MODAL_RUST_SNAPSHOT_PRIME"] = "1"
        os.environ["MODAL_RUST_SNAPSHOT_BEST_EFFORT"] = "1"
        os.environ["FAKE_RUNNER_PRIME_ACK"] = json.dumps(
            {"primed": 0, "failed": 1, "errors": ["enter blew up"]})
        w = self.load()  # must NOT raise
        self.assertIsNone(w._SERVE, "failed-prime child dropped for a clean respawn")
        # The lazy path still serves requests afterwards.
        del os.environ["FAKE_RUNNER_PRIME_ACK"]
        env = json.loads(w.handler("ok", '{"a": 7}'))
        self.assertEqual(env["value"], {"echo": {"a": 7}})

    def test_prime_success_keeps_the_primed_child_for_requests(self):
        os.environ["MODAL_RUST_SNAPSHOT_PRIME"] = "1"
        w = self.load()
        primed_child = w._SERVE
        self.assertIsNotNone(primed_child, "successful prime keeps the serve child")
        env = json.loads(w.handler("ok", '{"a": 8}'))
        self.assertEqual(env["value"], {"echo": {"a": 8}})
        self.assertIs(w._SERVE, primed_child,
                      "requests ride the SAME child the prime warmed (load-once)")


if __name__ == "__main__":
    unittest.main()
