# Programmatic Backend Tasks — staged milestone plan (pivot)

Re-architect the Modal AUTHORING/control layer: drive Modal **programmatically** from
Rust (forked modal-rs / vendored gRPC) instead of generating per-project Python shims +
shelling out to the `modal` CLI. See `knowledge.md` →
"Programmatic backend — grounded findings + plan (2026-06-04)" for the verified facts.

## Frozen invariants (must NOT change)

- The **runner CLI protocol** (`modal_runner --entrypoint … --input-file …`, ONE JSON
  envelope on stdout, five error kinds) — `../architecture/research-synthesis.md` §2.2.
- The **inventory `Registry`** + `typed!()` / `#[modal_rust::function/app(...)]` macros —
  §2.3. The decorator IS the config (gpu/cache/timeout).
- The **run-vs-deploy build boundary** (`../architecture/boundaries.md`):
  `run` = build at function-execution time (`copy=False`); `deploy` = build at image-build
  time (`copy=True` + `run_commands`), deployed runtime never invokes `cargo`; `call` =
  lookup/invoke only.

## Method

**Validate one boundary per task.** Each task crosses exactly ONE new boundary, has a
single failure reason, and ends with captured evidence. Build stages are strictly ordered;
do not start a stage until its `depends_on` are green. The forward path is the programmatic
control plane; the **fallback** (static-shim Option 2) is preserved as a clean revert at
every stage (it changes only the control layer).

## Gate

This workpad's build phase passes when a local Rust `main()` can `app.function("add")
.remote(cfg).await? == {"sum":42}` on Modal with **no `modal` CLI and no per-project Python
file**, per-function gpu/cache/timeout flowing dynamically from the registry, cache ON by
default, and `.local()` running the same handler in-process — with the runner/registry/macros
unchanged.

---

## P-research — primary-source research (DONE)

Status: done

- Modal 1.3.2 serialization + FunctionCreate (FILE vs SERIALIZED, CBOR boundary, resources/
  gpu CopyFrom, volume_mounts); modal-rs 0.1.3 surface + GPU gap + `inner_mut()` dead-end;
  native Volume copy/snapshot answer; local-orchestration semantics.
- **Evidence:** four research passes folded into `knowledge.md` (2026-06-04 section), with
  file:line citations re-verified against the installed 1.3.2 source.

## P-spike — executable feasibility spike (DONE-on-paper; live run OUTSTANDING)

Status: done (paper) / **BLOCKED (executable)** — must re-run before P1+

- Goal: prove Rust programmatically creates + invokes a Modal function → `{"sum":42}` with
  NO `modal` CLI and NO generated per-project file (FILE mode + CBOR + forked modal-rs).
- **Outcome:** paper-feasibility = FEASIBLE (all facts source-verified). The **live spike was
  blocked by an infra error** ("API Error: socket connection closed unexpectedly"), NOT a
  Modal/Rust limitation — no `{"sum":42}` round-trip was produced.
- Acceptance (for the re-run): a throwaway Rust binary creates an app, builds an image
  carrying a hand-written wrapper module, `FunctionCreate(FILE, function_serialized=b"")`,
  invokes with CBOR `(args,kwargs)`, prints the decoded result `== {"sum":42}`.
- **Evidence:** captured stdout of the round-trip + the gRPC request fields; OR, if blocked
  again, the exact error + retry plan. **This re-run gates P1.**
- depends_on: [P-research]
- fallback: if the executable spike fails for a *design* reason (not infra), STOP the pivot
  and adopt static-shim Option 2 (record the blocking reason here).

---

## P1 — Programmatic control-plane client (auth + a real FunctionGet/invoke)

Status: pending

- **Boundary crossed:** Rust authenticates to Modal and performs ONE real RPC round-trip
  against the live API (read path), proving the channel + auth + proto wiring before any
  authoring.
- **acceptance:** a `modal-rust-client` module wraps a forked/vendored modal-rs `ModalClient`
  (auth from `~/.modal.toml`/`MODAL_TOKEN_*`); `App::connect(name)` does `AppGetOrCreate`;
  `function(name).from_name(...)` does `FunctionGet`; invoking an **already-deployed** `add`
  function via `.remote((cfg,))` returns `{"sum":42}` decoded from CBOR. No `modal` CLI used.
- **evidence:** captured decoded result + the resolved auth source; the deployed fixture used.
- depends_on: [P-spike (live re-run green)]
- fallback: static-shim Option 2 (modal-rs confined to `call`, as in §2.7).

## P2 — Embedded-wrapper serialization decision (FILE-mode module; cbor-or-cloudpickle)

Status: pending

- **Boundary crossed:** the ONE embedded Python wrapper module exists in THIS crate and is
  proven importable + correct as a Modal function body, with the function-body
  representation chosen (FILE mode is the decision; SERIALIZED/cloudpickle is the stretch).
- **acceptance:** a single `modal_rust_wrapper.py` (in-crate, `include_str!`-embedded) with a
  top-level entry fn that receives the CBOR/JSON input, execs the frozen runner
  (`modal_runner --entrypoint <name> --input-file /tmp/in.json`), and returns the one-line
  JSON envelope string. Locally: `python -c "import modal_rust_wrapper; print(...)"` round-
  trips a sample envelope. Decision recorded: **FILE mode** (`function_serialized=b""` +
  `module_name`/`function_name`); cloudpickle-proto-4-from-Rust is the `dump`-tool stretch.
- **evidence:** the wrapper source + a local import/exec round-trip; the recorded decision +
  why FILE mode (cites `user_code_imports.py:475,488`).
- depends_on: [P-research]
- fallback: the same wrapper doubles as the static-shim's runner-exec body (no waste).

## P3 — Programmatic FunctionCreate (run path; no CLI, no file)

Status: pending

- **Boundary crossed:** Rust **creates** a function on Modal end-to-end (image build with the
  embedded wrapper baked in → precreate → FunctionCreate FILE mode → invoke), with NO `modal`
  CLI and NO per-project Python file — the `run` path, `copy=False` source mount preserved.
- **acceptance:** `app.function("add").remote(cfg).await? == {"sum":42}` where the function
  was created this run: `ImageGetOrCreate` (dockerfile adds the wrapper module + source),
  `FunctionPrecreate`, `FunctionCreate{definition_type=FILE, function_serialized=b"",
  module_name, function_name, image_id, supported_input_formats=[…,CBOR]}`. Build still
  happens in the function body at execution time (boundary intact).
- **evidence:** captured `{"sum":42}`; the FunctionCreate request fields; confirmation no
  `modal` CLI / no `.py` written to the user project.
- depends_on: [P1, P2]
- fallback: static-shim Option 2 (generate one static shim + `modal run`).

## P4 — Dynamic config from the registry (gpu/cache/timeout into FunctionCreate)

Status: pending

- **Boundary crossed:** per-function config sourced from the **Rust registry** (populated by
  `#[modal_rust::function/app(...)]`) flows into FunctionCreate at runtime — and the old
  static path is removed (drop the `--gpu` flag + the CLI static attribute-parse /
  `--describe`-before-build).
- **acceptance:** `#[modal_rust::function(gpu="A100", timeout=1800)]` → the registry's
  `FunctionConfig` → `FunctionCreate.resources.gpu_config` + `timeout_secs` (requires the
  **forked modal-rs GPU/resources setter**, since 0.1.3 leaves `resources` default and
  `inner_mut()` can't reach private proto). A GPU **list** routes through `ranked_functions`.
  Config is read dynamically — NO pre-build parse, NO `--describe`, NO `--gpu` CLI flag.
- **evidence:** a function created with `gpu_config` set verified server-side (or in the
  request bytes); the deleted `--gpu` flag / static-parse code; a non-GPU fn (timeout/volumes
  via as-is modal-rs) and a GPU fn (forked path) both created.
- depends_on: [P3]
- fallback: static-shim Option 2 reads config into the static shim's env (`MODAL_RUST_CONFIG_JSON`).

## P5 — Deploy path (programmatic AppPublish; build at image-build time)

Status: pending

- **Boundary crossed:** the **deploy** boundary — a persistent app published programmatically,
  with Rust built at IMAGE-BUILD time (`copy=True` + `run_commands(cargo build)`), the
  deployed runtime execing only the baked binary (never `cargo`).
- **acceptance:** `modal-rust deploy add` (or `App::deploy`) builds an image with the source
  COPIED + `cargo build --release` in a build step, `FunctionCreate` against a published app
  (`AppPublish`), then `call`/`.remote()` invokes the deployed function → `{"sum":42}` with no
  source mount and no build at call time. Boundary asserted: deployed body has no `cargo`.
- **evidence:** captured deploy output + a subsequent cold `call` result; proof the runtime
  image execs the prebuilt binary (no cargo in the run step).
- depends_on: [P3]
- fallback: static-shim `deploy_app.py` + `modal deploy`.

## P6 — Cache ON by default (V2 Volume, archive-as-single-object)

Status: pending

- **Boundary crossed:** the cargo cache is attached + warmed automatically on the `run` path
  (default ON), using the chosen mechanism — NO native snapshot exists, so an archive object
  on a V2 Volume.
- **acceptance:** `run` attaches `Volume.from_name("modal-rust-cargo-cache",
  create_if_missing=True, version=2)` via `FunctionVolumeMount.with_allow_background_commits
  (true)` from the registry config; the wrapper unpacks `cache.tar.zst` → `/tmp` on start and
  repacks on exit; build on `/tmp` (`CARGO_TARGET_DIR=/tmp/target`), never on the mount; a
  second `run` is measurably faster (cache hit). `--no-cache` + `modal volume rm` reset works.
  No `vol.reload()` on the hot path.
- **evidence:** two timed runs (cold vs warm) showing speedup; the volume_mount fields; the
  `--no-cache` escape verified. (Best-effort — not a dependency of deploy.)
- depends_on: [P4]
- fallback: cache OFF (null-result escape hatch), per M6.

## P7 — Local orchestration (`.remote().await` + `.local()`, feature-gated CUDA)

Status: pending

- **Boundary crossed:** a local Rust `main()` orchestrates Modal like
  `@app.local_entrypoint()` — and `.local()` runs the same handler IN-PROCESS with no Modal,
  on a dev machine with no CUDA installed.
- **acceptance:** `App::connect` + `function(name).remote(cfg).await?` (remote) and
  `.local(cfg)?` (in-process Registry dispatch → `{"sum":42}`) both work from one `main()`.
  The crate compiles on a Mac with NO CUDA: GPU bodies behind `#[cfg(feature="cuda")]`,
  cudarc `dynamic-loading`; the burn-add default-members workspace exclusion is removed and
  `cargo build --workspace` is clean. Add `.spawn()`/`.map()` if cheap.
- **evidence:** one `main()` exercising `.remote()` + `.local()`; clean `cargo build
  --workspace` without CUDA; the removed default-members exclusion.
- depends_on: [P3]
- fallback: `.local()` works standalone (zero Modal) even if `.remote()` reverts to static shim.

## P8 — `dump`/debug tool

Status: pending

- **Boundary crossed:** the escape hatch — users can emit + inspect the exact embedded
  wrapper, the image dockerfile commands, and the FunctionCreate request that Rust would send.
- **acceptance:** `modal-rust dump [run|deploy|call] <entrypoint>` writes the wrapper module,
  the resolved config, and a human-readable FunctionCreate summary to `.modal-rust/debug/`
  without contacting Modal. Doubles as the proof-of-correctness for the SERIALIZED-mode
  stretch (it can emit a candidate cloudpickle blob for inspection).
- **evidence:** the dumped artifacts for the `add` example.
- depends_on: [P3]
- fallback: trivially compatible with static-shim (dump the static shim + config instead).

## P9 — Migrate the CLI off codegen

Status: pending

- **Boundary crossed:** the `modal-rust run/deploy/call` commands route through the
  programmatic client by default; the Python template renderer + `modal` CLI shell-out are
  removed from the default path.
- **acceptance:** `modal-rust run/deploy/call` produce identical results to the previous
  generated-shim path (same `{"sum":42}`, same envelope) but emit NO `.modal-rust/generated/
  *.py` and invoke NO `modal` CLI. `doctor` updated (drops `modal` CLI on `$PATH` as a hard
  requirement for the default path; keeps auth check).
- **evidence:** before/after parity on `run`/`deploy`/`call`; confirmation no generated `.py`
  and no `modal` subprocess; updated `doctor`.
- depends_on: [P3, P4, P5]
- fallback: keep `templates.rs` + `modal` CLI behind a `--use-shim` flag as the documented
  fallback control path (clean revert).

## P10 — Remove the per-project shim

Status: pending

- **Boundary crossed:** the final cleanup — no per-project Python file is materialized
  anywhere on the default path; the embedded wrapper lives only in this crate / the image.
- **acceptance:** a clean `examples/add` run/deploy/call leaves zero generated `.py` in the
  user project; `templates.rs`'s per-project templates are deleted (or moved behind the
  `--use-shim` fallback only). "No Python file visible anywhere" (Rust-emitted cloudpickle,
  SERIALIZED mode) remains a documented stretch validated by `dump`, not required here.
- **evidence:** a clean tree after a full run/deploy/call cycle; the removed templates.
- depends_on: [P9]
- fallback: retain the static shim materialized to an OS cache dir (Option 5E) — not the user
  project — if a per-project file proves unavoidable.

## P-fix-per-entrypoint-config — RUN path must not memoize first entrypoint config

Status: ✅ DONE (2026-06-05)

- **Boundary crossed:** facade/runtime correctness after P4/P6/secrets-volumes — one Rust app can
  expose multiple entrypoints with divergent decorator config, and `.remote()` must apply the
  invoked entrypoint's effective gpu/timeout/cache/secrets/volumes instead of silently reusing the
  first-created wrapper.
- **acceptance:** an offline `modal-rust-testkit` regression reproduces CPU-first then GPU-second
  order dependence against `FunctionCreate`; the fix keys created RUN wrappers by entrypoint +
  effective config so each entrypoint gets its own Modal object tag. Deploy publishes one function
  per entrypoint over one shared image, so divergent deploy-time configs coexist instead of
  being first-wins or rejected.
- **evidence:** failing test command + fixed test command; focused cargo fmt/clippy/test results;
  knowledge note explaining the key/limitation.

## P-harden — image + upload robustness pass (2026-06-04, user-requested)

Status: ✅ DONE (workflow `harden-image-upload`) — both tracks proven live; 3/3 reviews PASS.
Image = `add_python` (python-standalone mount, byte-for-byte the client's recipe), 3 hacks removed;
upload = `cargo metadata` closure scoping + `.modalignore`>`.gitignore`>defaults (`ignore` crate). Also
fixed: auto-resolve `image_builder_version` (>2024.10, else no dep mount → boot crash) + rewrite the
uploaded workspace `Cargo.toml` to the closure subset (`toml_edit`). Live upload: 7 files/187 KB (was 14 MB).

- **Boundary crossed:** the run/deploy images and the source upload stop relying on brittle hacks.
- **A — image:** do what the official modal client does — provision Python via **add_python** (the hosted
  python-standalone mount, resolved by name like the client mount) + the client mount, REMOVING the three
  hacks the live runs forced in: `python-is-python3`, `--break-system-packages`, and the bare
  `apt+pip install modal` (all symptoms of provisioning Python the crude way on a Debian `rust:slim` base).
  Faster, reset-resistant image build. apt+pip kept only as a documented fallback.
- **B — upload:** replace the hardcoded ignore list with **(1)** `cargo metadata` scoping — upload only the
  target package's workspace-member dependency closure + workspace `Cargo.toml`/`Cargo.lock` (correct by
  construction; the `references/` bug class disappears); **(2)** ignore resolution **`.modalignore` (highest)
  > `.gitignore` > defaults** via the ripgrep `ignore` crate. Non-source extras (data/models) attach via
  **volumes**, never the source upload.
- **acceptance:** `.remote()` + `deploy`/`call` re-prove `{sum:42}` live on the add_python image with NONE of
  the 3 hacks and a faster build; the upload ships only the dep-closure crates; `.modalignore`/`.gitignore`
  are honored.
- depends_on: [P3, P5] (both done + live).

---

## DAG

```
P-research → P-spike → P1 ─┐
P-research → P2 ───────────┤
                           ├→ P3 → P4 → {P6}
                           │    P3 → P5
                           │    P3 → P7
                           │    P3 → P8
                  {P3,P4,P5} → P9 → P10
```

P6 (cache) and P7 (local orchestration) are independent leaves off P3/P4 and are not
dependencies of the deploy/cleanup chain. P-spike's **live re-run gates the entire build
phase** — if it fails for a design reason, adopt static-shim Option 2 and re-scope P1–P10.
