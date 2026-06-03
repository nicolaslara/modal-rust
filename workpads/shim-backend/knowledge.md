# Shim Backend Knowledge

## Objective

Capture the design space for the Modal Python control-plane backend: how much
Python must exist, where it should live, and whether "generation" should mean
source code generation or just config/data generation. This workpad is
exploratory and must preserve the existing architecture constraints:

- `run` builds Rust at function-execution time (`copy=False` source mount, or a
  documented Sandbox fallback if required).
- `deploy` builds Rust at image-build time (`copy=True` + `run_commands`) and the
  deployed runtime never invokes `cargo`.
- `call` invokes an already-deployed function and never mounts source or builds.
- v0 currently uses generated Python + the official `modal` CLI as the known-good
  control path.

## Gate Status

Not passed yet.

The workpad passes when this file contains a decision-ready comparison of shim
backend alternatives, a static-shim config contract, a minimal spike plan, and a
recommendation or explicitly recorded blockers.

## Current Baseline

The current CLI uses parameterized fixture templates:

- `crates/modal-rust-cli/src/templates.rs` embeds three Python template files with
  `include_str!`.
- `dev_app.py.tmpl`, `deploy_app.py.tmpl`, and `call_app.py.tmpl` are mostly
  static Python shims with a few `{{PLACEHOLDER}}` constants.
- `main.rs` renders those strings into `.modal-rust/generated/{dev_app.py,
  deploy_app.py,call_app.py}` and invokes the official `modal` CLI with a file
  path.

The current placeholders are small:

- app names (`dev_app_name`, `deploy_app_name`, `call_app_name`);
- Rust image version;
- local source/workspace path;
- deploy app lookup name for `call`.

Entrypoint and input are already data. They flow through `modal run` flags into
the shim's `@app.local_entrypoint()` as `entrypoint` and `input_json`.

## Design Space

### Option A - Status quo: parameterized Python templates

Keep rendering Python source files under `.modal-rust/generated/`.

Pros:
- Already proven by M9a as byte-equivalent to the prototype shims.
- Simple to debug: the exact Python module passed to `modal` is visible.
- Matches the official `modal run <file>::main` / `modal deploy <file>` path.
- Low implementation risk.

Cons:
- Feels like generated source even though only a few constants vary.
- As config grows, Python templating can become a second product surface.
- Many apps/deployments may produce many near-identical files.
- Template substitutions require care around quoting/escaping if values become
  richer than paths and names.

### Option B - Fully static shims with `MODAL_RUST_CONFIG_JSON`

Ship static Python files and pass all import-time configuration through an env
var containing JSON.

Pros:
- Python source is always static; generated artifact becomes data.
- Simple for small configs and easy to hash/debug.
- Rust can own static shim bytes deterministically via `include_str!` or
  `include_bytes!`.
- Avoids adding a template language.

Cons:
- Env vars are awkward for large configs.
- Must avoid secrets in logged command lines/env dumps.
- The config must be present in the local environment when the Modal CLI imports
  the module, because app/image/function definitions happen at import time.
- Shell quoting can be painful unless Rust sets env directly on `Command`.

### Option C - Fully static shims with `MODAL_RUST_CONFIG_PATH`

Ship static Python files and pass a path to a generated JSON/TOML config file.

Pros:
- Better for large configs, many functions, volumes, secrets refs, GPU config,
  cache policies, and future app metadata.
- Easier debug artifact: inspect one config file plus one static shim version.
- Avoids command-line/env length and quoting concerns.

Cons:
- Still materializes a generated file, though it is data not code.
- The config path must be readable from the local process that imports the Modal
  module; if the shim is moved to a cache/package location, path handling must be
  deliberate.
- Need lifecycle rules for cleanup, cache invalidation, and `--keep`/debug dumps.

### Option D - Static shims as an installed/importable Python package

Install or bundle a Python package such as `modal_rust_backend.run_app` and invoke
it with `modal run -m ...` if Modal's module path supports the needed flow.

Pros:
- Can eliminate project-local shim files.
- Python source can be versioned/published with the CLI.
- Cleaner UX if the `modal` CLI module mode is reliable.

Cons:
- Adds Python packaging/install complexity to a Rust-first tool.
- Version skew between the Rust CLI and installed Python package becomes a real
  failure mode unless Rust materializes or verifies the exact package bytes.
- Still requires import-time config through env/path.

### Option E - Static shims materialized in OS temp/cache

Rust embeds static shim bytes and writes them to `$TMPDIR` or an OS cache
directory only because the official Modal CLI wants an importable file/module.

Pros:
- Keeps user projects cleaner than `.modal-rust/generated/`.
- Static bytes can be content-addressed by shim version/hash.
- Works with the current official CLI path.
- Debug mode can copy the exact shim/config into `.modal-rust/debug/`.

Cons:
- Hidden temp files can make debugging harder unless the CLI prints/dumps them on
  request.
- Cache cleanup and stale files need a simple policy.
- File paths in Modal logs may point outside the project.

### Option F - Static shims baked into a Modal image/base image

Put the shim backend into an image layer or base image.

Pros:
- Useful for runtime pieces that do not need to vary per project.
- Can reduce repeated upload of static Python source after the app is authored.

Cons:
- Does not remove the local authoring problem: `modal deploy` still needs Python
  code locally to define `app`, `image`, and functions.
- Image-baked code is less convenient during local iteration.
- Must not blur the build boundary: deploy image build may bake the runner, while
  run mode must still build at function-execution time.

### Option G - Python SDK subprocess/backend without `modal run <file>`

Rust could spawn Python and call Modal SDK internals directly, feeding static code
through stdin or `python -c`, rather than invoking the official `modal` CLI with a
module path.

Pros:
- Could reduce or eliminate local shim file materialization.
- Gives Rust more direct control over config and orchestration.

Cons:
- Becomes a new Modal control path, not the currently validated pure wrapper.
- Risks relying on Modal Python internals instead of the public CLI.
- Needs fresh empirical validation for run/deploy/call behavior and logs.

### Option H - Lower-level `modal-rs`/protobuf authoring backend

Use Modal's API/protobuf surface directly from Rust, avoiding Python shim source.

Pros:
- Long-term Rust-native control plane, if feasible.
- Could make uploads/deploys more deterministic from Rust.

Cons:
- Prior research found Modal function creation is still Python-shaped: prepared
  functions need a serialized Python callable/image relationship.
- High compatibility risk around Modal's private protocol and Python
  serialization.
- Too large for v0 unless a focused spike proves the missing authoring pieces.

### Option I - Hybrid: static embedded bytes plus optional debug dump

Default to static shims embedded in Rust and config as data, materialized only in
a hidden cache/temp path; add `--dump-shim`/`--keep-shim` for debugging.

Pros:
- Keeps Python source static and deterministic.
- Preserves the official Modal CLI path.
- Gives good debug escape hatches.
- Scales better to many apps because app-specific values are config, not source.

Cons:
- More moving pieces than the current generated-template path.
- Requires a config contract and compatibility/versioning story.

## Key Distinctions

- **No generated Python source**: plausible soon. Static shim source plus generated
  config/data.
- **No local shim file**: plausible through an installed module path or deeper SDK
  backend; otherwise the official CLI still needs something importable.
- **No Python control plane at all**: a much larger backend change and not yet
  proven feasible.

## Static Config Contract Sketch

Open draft; exact field names are not locked.

```json
{
  "schema_version": 1,
  "mode": "run",
  "app_name": "modal-rust-poc-dev",
  "deploy_app_name": "modal-rust-add-poc",
  "call_app_name": "modal-rust-call",
  "rust_image": "rust:1-slim",
  "python_version": "3.12",
  "local_src": "/Users/nicolas/devel/modal-rust",
  "remote_src": "/src",
  "copy": false,
  "ignore": ["target", ".git", ".modal-rust", "**/*.rlib"],
  "remote_env": {"RUST_BACKTRACE": "1"},
  "timeout_seconds": 1800,
  "build": {
    "kind": "function_body",
    "commands": ["cargo build --release --bin modal_runner"],
    "cargo_home": "/tmp/cargo",
    "cargo_target_dir": "/tmp/target"
  },
  "gpu": null,
  "volumes": [],
  "secrets": [],
  "cache": null
}
```

Import-time fields:
- `mode`, `app_name`, `rust_image`, `python_version`, `local_src`, `remote_src`,
  `copy`, `ignore`, `remote_env`, `timeout_seconds`, GPU, volumes, secrets, and
  deploy `run_commands`.
- Reason: the Python module constructs `modal.App`, `modal.Image`, decorators, and
  `@app.function` definitions when imported by the Modal CLI.

Local-entrypoint runtime fields:
- `entrypoint`, `input_json`, and possibly debug flags.
- Reason: these are parameters to `main(...)` and do not affect Modal app/image
  construction.

Remote-function runtime fields:
- Runner invocation arguments, input file path, build directory selection, and
  runtime env such as `CARGO_HOME`/`CARGO_TARGET_DIR`.

Config transport:
- Small configs can use `MODAL_RUST_CONFIG_JSON`.
- Larger configs should use `MODAL_RUST_CONFIG_PATH`.
- The static shim should support both, preferring JSON when present and otherwise
  reading the path.

## Template Language Assessment

A template language is probably not needed if the Python source becomes static.
The current substitution set is small, and richer values are better represented
as typed config data than as generated Python syntax. If Python source generation
continues and the variable surface expands, a template engine could improve
escaping and readability, but that would also entrench Python codegen as a
product surface.

## Open Questions

- Should static config be JSON only, or TOML for human-written defaults plus JSON
  for generated invocation config?
- Should static shims live under `.modal-rust/generated/`, `$TMPDIR`, an OS cache
  directory, or an installed Python package?
- Should the default debug behavior print the shim path, the config path, both,
  or only print them under `--verbose`?
- How should secrets be represented so they are Modal secret references, never raw
  secret values in config logs?
- Do GPU, volumes, and future concurrent-input/autoscaling knobs fit one generic
  static shim per mode, or do they require mode-specific static variants?
- Can `modal run -m <module>` satisfy all run/call/deploy needs, or is a file path
  simpler and more reliable?
- Is a lower-level `modal-rs`/protobuf authoring path worth another spike after
  v0, or should it remain call-only unless Modal exposes a stable authoring API?

## Initial Recommendation

Do not replace the current M9a path before GPU work. For the next refinement,
explore **Option I: static embedded shim bytes + env/path config + optional debug
dump**. It keeps the official Modal CLI path and the build-boundary proof, while
turning app-specific variation into data instead of generated Python source.
