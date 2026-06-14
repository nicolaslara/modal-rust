import base64
import importlib.util
import json
import os
import pathlib
import subprocess
import sys
import unittest
from unittest import mock


ROOT = pathlib.Path(__file__).with_name("wrapper.py")
CONFIG_ENV = "MODAL_RUST_RUN_CONFIG_JSON_B64"


def encoded_config(config):
    payload = json.dumps(config, separators=(",", ":")).encode("utf-8")
    return base64.b64encode(payload).decode("ascii")


def load_wrapper(config=None):
    if config is None:
        config = {
            "package": "example-add",
            "cache": True,
            "remote_src": "/src",
            "cache_mount": "/cache",
            "cache_archive_name": "cache.tar.zst",
        }
    os.environ[CONFIG_ENV] = encoded_config(config)
    name = f"modal_rust_run_wrapper_test_{id(config)}"
    spec = importlib.util.spec_from_file_location(name, ROOT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class WrapperTests(unittest.TestCase):
    def test_file_compiles(self):
        subprocess.run([sys.executable, "-m", "py_compile", str(ROOT)], check=True)

    def test_loads_config_from_env(self):
        module = load_wrapper(
            {
                "package": "example-add",
                "cache": False,
                "remote_src": "/mounted-src",
                "cache_mount": "/cache",
                "cache_archive_name": "cache.tar.zst",
            }
        )
        self.assertEqual(module.PACKAGE, "example-add")
        self.assertFalse(module.CACHE_ON)
        self.assertEqual(module.REMOTE_SRC, "/mounted-src")
        self.assertEqual(module._ARCHIVE_ZSTD, "/cache/cache.tar.zst")
        self.assertEqual(module._ARCHIVE_GZIP, "/cache/cache.tar.gz")

    def test_missing_config_env_errors_at_import(self):
        os.environ.pop(CONFIG_ENV, None)
        name = "modal_rust_run_wrapper_missing_config_test"
        spec = importlib.util.spec_from_file_location(name, ROOT)
        module = importlib.util.module_from_spec(spec)
        with self.assertRaisesRegex(RuntimeError, "missing required run wrapper config env"):
            spec.loader.exec_module(module)

    def test_handler_builds_then_execs_runner_one_shot(self):
        # The legacy cold one-shot path (serve disabled) stays byte-identical: build
        # once, then exec a fresh `modal_runner --entrypoint .. --input-file ..`.
        module = load_wrapper()
        build_envs = []

        def fake_build(env):
            build_envs.append(env)

        class Proc:
            returncode = 0
            stderr = "[runner] ok\n"
            stdout = '{"ok":true,"value":null}\n'

        def fake_run(args, capture_output, text, env):
            self.assertEqual(
                args,
                [
                    module._RUNNER,
                    "--entrypoint",
                    "add",
                    "--input-file",
                    "/tmp/in.json",
                ],
            )
            self.assertTrue(capture_output)
            self.assertTrue(text)
            self.assertEqual(env["CARGO_HOME"], "/tmp/cargo")
            return Proc()

        module._build = fake_build
        with mock.patch.dict(os.environ, {"MODAL_RUST_SERVE": "0"}):
            with mock.patch.object(module.subprocess, "run", fake_run):
                out = module.handler("add", '{"a":40,"b":2}')

        self.assertEqual(out, '{"ok":true,"value":null}')
        self.assertEqual(len(build_envs), 1)
        with open("/tmp/in.json") as f:
            self.assertEqual(f.read(), '{"a":40,"b":2}')

    def test_handler_serve_path_spawns_one_child_and_reuses_it(self):
        # The DEFAULT warm path: ONE persistent `modal_runner --serve` child, framed
        # request in, one frozen envelope line out. A second call reuses the SAME child
        # (load-once) — proven by `Popen` being called exactly once across two handler
        # calls. The request frame carries the SAME entrypoint + per-call input JSON.
        module = load_wrapper()
        module._build = lambda env: None

        spawned = []

        class FakeChild:
            def __init__(self):
                self.stdin = mock.Mock()
                self.stdout = mock.Mock()
                # one envelope line per readline call
                self.stdout.readline = mock.Mock(
                    side_effect=[
                        '{"ok":true,"value":9}\n',
                        '{"ok":true,"value":7}\n',
                    ]
                )

            def poll(self):
                return None  # still alive

        def fake_popen(args, stdin, stdout, stderr, text, env):
            self.assertEqual(args, [module._RUNNER, "--serve"])
            child = FakeChild()
            spawned.append(child)
            return child

        with mock.patch.object(module.subprocess, "Popen", fake_popen):
            out1 = module.handler("Embedder.embed", '{"text":"hi"}')
            out2 = module.handler("Embedder.dim", "null")

        self.assertEqual(out1, '{"ok":true,"value":9}')
        self.assertEqual(out2, '{"ok":true,"value":7}')
        # The whole point: ONE child across BOTH calls (the singleton stays warm).
        self.assertEqual(len(spawned), 1)
        # The first framed request carried the right entrypoint + input.
        first_frame = spawned[0].stdin.write.call_args_list[0].args[0]
        self.assertEqual(
            json.loads(first_frame),
            {"entrypoint": "Embedder.embed", "input": '{"text":"hi"}'},
        )

    def test_handler_serve_falls_back_to_one_shot_on_empty_envelope(self):
        # A crashed serve child (empty stdout / closed pipe) must not lose the call:
        # the wrapper drops the child and falls back to a fresh one-shot exec.
        module = load_wrapper()
        module._build = lambda env: None

        class DeadChild:
            def __init__(self):
                self.stdin = mock.Mock()
                self.stdout = mock.Mock()
                self.stdout.readline = mock.Mock(return_value="")  # EOF

            def poll(self):
                return 101  # crashed

        class Proc:
            returncode = 0
            stderr = ""
            stdout = '{"ok":true,"value":1}\n'

        with mock.patch.object(module.subprocess, "Popen", lambda *a, **k: DeadChild()):
            with mock.patch.object(module.subprocess, "run", lambda *a, **k: Proc()):
                out = module.handler("Embedder.dim", "null")

        self.assertEqual(out, '{"ok":true,"value":1}')

    def test_handler_describe_sentinel_runs_one_shot_describe(self):
        # S2: the reserved describe sentinel builds (populating the runner cache) then
        # runs `modal_runner --describe` ONE-SHOT and returns its manifest verbatim —
        # never spawning the serve child, never exec'ing a real --entrypoint.
        module = load_wrapper()
        build_envs = []

        def fake_build(env):
            build_envs.append(env)

        manifest_line = '{"schema":"modal-rust/describe@1","entrypoints":[]}'

        class Proc:
            returncode = 0
            stderr = ""
            stdout = manifest_line + "\n"

        def fake_run(args, capture_output, text, env):
            self.assertEqual(args, [module._RUNNER, "--describe"])
            self.assertTrue(capture_output)
            self.assertTrue(text)
            return Proc()

        module._build = fake_build

        def boom_popen(*a, **k):
            raise AssertionError("describe must not spawn the serve child")

        with mock.patch.object(module.subprocess, "run", fake_run):
            with mock.patch.object(module.subprocess, "Popen", boom_popen):
                out = module.handler(module._DESCRIBE_SENTINEL, "")

        self.assertEqual(out, manifest_line)
        self.assertEqual(len(build_envs), 1, "describe still builds (populates runner cache)")

    def test_describe_sentinel_is_lowercase_not_a_modal_rust_literal(self):
        # The sentinel must stay lowercase so it never trips the MODAL_RUST_ env
        # drift-guard (it is a reserved entrypoint NAME, not an env var).
        module = load_wrapper()
        self.assertEqual(module._DESCRIBE_SENTINEL, "__modal_rust_describe__")
        self.assertNotIn("MODAL_RUST_", module._DESCRIBE_SENTINEL)

    def test_cache_target_default_on_mirrors_rust(self):
        # CROSS-LANGUAGE DRIFT GUARD: the Rust `discover_cache_target()` and this
        # wrapper's `_cache_target_on()` must agree — default ON, the falsy set
        # opts out. (The cache pair was the one env knob WITHOUT this guard, and
        # the default mismatch went unnoticed — see 2026-06-11.)
        module = load_wrapper()
        with mock.patch.dict(os.environ):
            os.environ.pop("MODAL_RUST_CACHE_TARGET", None)
            self.assertTrue(module._cache_target_on(), "default must be ON")
            for falsy in ("0", "false", "NO", "Off"):
                os.environ["MODAL_RUST_CACHE_TARGET"] = falsy
                self.assertFalse(module._cache_target_on(), falsy)
            os.environ["MODAL_RUST_CACHE_TARGET"] = "1"
            self.assertTrue(module._cache_target_on())

    def test_pack_cache_includes_target_by_default(self):
        # The user-visible promise: fresh containers reuse COMPILED deps. That is
        # only true if target/ actually rides the archive.
        module = load_wrapper()
        calls = []
        with mock.patch.dict(os.environ):
            os.environ.pop("MODAL_RUST_CACHE_TARGET", None)
            with mock.patch.object(module, "_pack_one", lambda a, f, dirs: calls.append(dirs)):
                module._pack_cache()
        self.assertEqual(calls, [["cargo", "target"]])

    def test_source_key_is_content_addressed_and_package_scoped(self):
        import tempfile

        src = tempfile.mkdtemp(prefix="wrapper-src-")
        with open(os.path.join(src, "lib.rs"), "w") as f:
            f.write("fn a() {}")
        module = load_wrapper(
            {
                "package": "pkg-a",
                "cache": True,
                "remote_src": src,
                "cache_mount": "/cache",
                "cache_archive_name": "cache.tar.zst",
            }
        )
        k1 = module._source_key()
        self.assertEqual(k1, module._source_key(), "key must be deterministic")
        # Content change changes the key.
        with open(os.path.join(src, "lib.rs"), "w") as f:
            f.write("fn b() {}")
        self.assertNotEqual(k1, module._source_key())
        # Same source, different package: different key (one shared workspace
        # upload can build two different -p targets).
        module_b = load_wrapper(
            {
                "package": "pkg-b",
                "cache": True,
                "remote_src": src,
                "cache_mount": "/cache",
                "cache_archive_name": "cache.tar.zst",
            }
        )
        self.assertNotEqual(module.PACKAGE, module_b.PACKAGE)
        self.assertNotEqual(module._source_key(), module_b._source_key())

    def test_runner_binary_cache_roundtrip_and_hit_skips_build(self):
        import tempfile

        src = tempfile.mkdtemp(prefix="wrapper-src-")
        with open(os.path.join(src, "lib.rs"), "w") as f:
            f.write("fn a() {}")
        cache = tempfile.mkdtemp(prefix="wrapper-cache-")
        module = load_wrapper(
            {
                "package": "pkg-a",
                "cache": True,
                "remote_src": src,
                "cache_mount": cache,
                "cache_archive_name": "cache.tar.zst",
            }
        )
        tmp = tempfile.mkdtemp(prefix="wrapper-tmp-")
        runner = os.path.join(tmp, "modal_runner")
        marker = os.path.join(tmp, ".built")
        with mock.patch.object(module, "_RUNNER", runner), mock.patch.object(
            module, "_MARKER", marker
        ):
            key = module._source_key()
            # Miss: nothing stored yet.
            self.assertFalse(module._try_cached_runner(key))
            # Store, then a fresh "container" (no marker, no /tmp runner) hits.
            with open(runner, "wb") as f:
                f.write(b"\x7fELF fake-runner-bytes")
            module._store_cached_runner(key)
            os.remove(runner)
            self.assertTrue(module._try_cached_runner(key))
            with open(runner, "rb") as f:
                self.assertEqual(f.read(), b"\x7fELF fake-runner-bytes")
            # The full _build flow on the hit path: NO cargo run at all.
            def boom(*a, **k):
                raise AssertionError("cargo must not run on a runner-cache hit")

            with mock.patch.object(module.subprocess, "run", boom):
                module._build(dict(os.environ))
            self.assertTrue(os.path.exists(marker))


if __name__ == "__main__":
    unittest.main()
