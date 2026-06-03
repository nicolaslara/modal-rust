# Shim Backend Tasks

Explore how `modal-rust` should represent and drive the Modal Python control
plane as apps grow beyond the `add` POC. This is an exploratory workpad: it does
not change the active GPU/prototype gates, and it must not weaken the hard
run-vs-deploy build boundary in `../architecture/boundaries.md`.

## Objective

Define the design space for replacing "generated Python source" with a cleaner
shim backend. The current CLI renders mostly-static Python templates into
`.modal-rust/generated/` and invokes the official `modal` CLI. As apps grow to
many functions and many deployments, decide whether the Python files should stay
parameterized templates, become fully static data-driven shims, move to an
installed/cache module, be baked into an image, or eventually be replaced by a
different Modal authoring backend. Keep the work open to other designs.

The preferred hypothesis to examine first is: **static Python shim source +
config as data**. Entrypoint/input already flow as runtime data; app name, source
root, image/build knobs, GPU, volumes, secrets, and deploy target may also be
data available at module import time via env or a config path. Rust can embed the
static shim bytes (`include_str!`/`include_bytes!`), hash/version them, and
materialize them only when the official Modal CLI needs an importable file or
module.

## Gate

This workpad passes when `knowledge.md` records a decision-ready design note with
at least: (1) the design space and alternatives; (2) pros/cons and failure modes;
(3) which values must be available at Python module-import time versus
`@app.local_entrypoint()` runtime; (4) how the choice handles many functions,
many apps, debug dumps, and deterministic Rust-owned shim bytes; and (5) what
small spike, if any, is needed before changing the current CLI.

The gate does **not** require implementing the chosen backend. If implementation
is recommended, open follow-up tasks in the appropriate phase.

## S1 - Design matrix for shim representation and materialization

Status: pending

Acceptance:
- Catalog the alternatives below, plus any better ideas found during review:
  - status quo: parameterized Python source templates under `.modal-rust/generated/`;
  - fully static shims with config in `MODAL_RUST_CONFIG_JSON`;
  - fully static shims with `MODAL_RUST_CONFIG_PATH`;
  - static shims as an installed/importable Python package/module;
  - static shims materialized in an OS temp/cache directory;
  - static shims baked into a Modal image or base image;
  - Python SDK subprocess/backend that avoids `modal run <file>` as the primary
    authoring path;
  - lower-level `modal-rs`/protobuf authoring backend with no Python shim source;
  - hybrid: static embedded bytes plus optional debug dump.
- For each option, record pros/cons across: Modal CLI compatibility, import-time
  config availability, many-functions scaling, many-apps/deployments scaling,
  debug ergonomics, reproducibility/hashability, security/secrets handling,
  packaging/install complexity, and risk of becoming a second control path.
- Distinguish "no generated Python source", "no local shim file", and "no Python
  control plane at all"; do not collapse them.
- Record whether a template language is still needed if config moves to data.

Evidence:
- A completed design matrix in `knowledge.md`.
- Local references in `references.md` to the current template renderer, generated
  templates, CLI invocation sites, and architecture contracts.

## S2 - Static-shim config contract sketch

Status: pending

Acceptance:
- Sketch a versioned config shape for static shims, including at minimum:
  `mode`, `app_name`, `deploy_app_name`/lookup target, `rust_image`,
  `python_version`, `local_src`, `remote_src`, `copy`, `ignore`, build commands,
  remote env, timeout, optional GPU, optional volumes, optional secrets, and
  optional cache policy.
- Mark every field as import-time, local-entrypoint runtime, or remote-function
  runtime.
- Decide whether small configs should use `MODAL_RUST_CONFIG_JSON`, large configs
  should use `MODAL_RUST_CONFIG_PATH`, or both should be supported.
- Explain how Rust embeds, hashes, versions, materializes, and optionally dumps
  the static shim source and config for debugging.
- Preserve the run-vs-deploy boundary explicitly:
  - `run`: `copy=False`, runtime build in the Function body or documented
    Sandbox fallback;
  - `deploy`: `copy=True`, `run_commands(cargo build)`, deployed body never
    invokes `cargo`;
  - `call`: lookup/call only, no source/build.

Evidence:
- A config contract section in `knowledge.md` with example JSON for `run`,
  `deploy`, and `call`.
- A note identifying which fields cannot safely be passed only as
  `main(...)` CLI flags because Modal constructs `app`, `image`, and functions at
  Python module import time.

## S3 - Minimal static-shim spike plan

Status: pending

Acceptance:
- Propose the smallest spike that could prove or disprove static shims without
  disrupting M10+ GPU work. Prefer an offline/local smoke first, then one cheap
  `modal run` only if needed.
- The spike should test that a static file can construct `modal.App`,
  `modal.Image.add_local_dir(...)`, and `@app.function` from env/config available
  at module import time, then still accept `--entrypoint`/`--input-json` at
  local-entrypoint runtime.
- Include test/evidence expectations and how to compare output to the current
  generated templates.
- Keep the spike behind a separate command/feature or scratch path; do not replace
  the current generated-template CLI path until the spike is reviewed.

Evidence:
- Spike plan recorded in `knowledge.md`.
- If run, exact command(s), output, and any Modal cost/risk notes recorded.

## S4 - Recommendation and follow-up issue/task split

Status: pending

Acceptance:
- Recommend one default path and one fallback path, or explicitly leave the
  decision open with blockers.
- State whether to:
  - keep current template generation for v0;
  - refactor to static shims + config before more app complexity;
  - add `--shim-dir`, `--keep-shim`, or `--dump-shim` debug controls;
  - add an installed Python package/module path;
  - defer direct `modal-rs`/protobuf authoring.
- Break any implementation into small follow-up tasks with acceptance criteria.

Evidence:
- Final recommendation in `knowledge.md`.
- Any follow-up tasks added to the relevant workpad or explicitly deferred.
