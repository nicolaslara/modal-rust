# Harden-Build Spec — image (`add_python`) + upload (cargo-metadata + ignore)

Build-ready spec for the robustness pass. Two independent tracks. All `file:line`
citations are against the repo at `/Users/nicolas/devel/modal-rust`. Paths under
`references/` are READ-ONLY ground truth (gitignored — never a build dep).

## FROZEN invariants (do NOT change)
- Runner CLI protocol, Registry/macros, the **run-vs-deploy build boundary**
  (`workpads/architecture/boundaries.md`): RUN = build-in-body on a runtime source
  mount; DEPLOY = build-at-image-time, runtime execs the prebuilt
  `/app/modal_runner`, mounts NO source.
- `retry_transient` / `retry_unary` stays wrapped around every unary RPC.
- Do NOT rewrite the proven create/precreate/invoke/publish/deploy logic. This pass
  swaps exactly two mechanisms: the **Python-provisioning** of the image, and the
  **file-selection** of the source upload. Everything downstream is untouched.

## Verification gates (WORKING.md), on default-members
`cargo fmt --check` · `cargo clippy --all-targets -- -D warnings` · `cargo build` ·
`cargo test`. Keep no-CUDA CI green. One new dep only: `ignore = "0.4"` (in cache:
`ignore-0.4.25.crate`). Live tests behind `#[ignore]` + `live` feature. Use the
STABLE deploy app name (`modal-rust-add-deploy`); drive live proofs to terminal.

---

# TRACK A — Provision Python via `add_python` (replicate the official client)

## A0. Today's three hacks (all on a `rust:1-slim`/`python:3-slim` base)
- `with_apt(["python3", "python3-pip", "python-is-python3"])` — `remote.rs:252`,
  `deploy.rs:171`. `python-is-python3` exists because Modal's entrypoint execs bare
  `python`; apt gives only `python3`.
- `--break-system-packages` — `image.rs:184`. Debian system Python is PEP-668
  externally-managed; pip refuses without it.
- `pip install modal` — `image.rs:144`/`with_pip_install_modal()`. The client mount
  carries `modal` **source** only (mounted at `/pkg`), not its pip dep closure
  (`typing_extensions`, `grpclib`, `protobuf`, `aiohttp`, `cbor2`, `rich`, `toml`,
  `watchfiles`, …); without those, `python -m modal._container_entrypoint`
  crash-loops with `ModuleNotFoundError` (live-documented at `image.rs:10-29`).

## A1. Verified mechanism the official client uses

### A1a. The python-standalone mount (HOSTED, resolved by name — no apt, no build)
`references/modal-client/py/modal/mount.py`:
- `PYTHON_STANDALONE_VERSIONS` (mount.py:45-52) maps a series → `(release, full)`:
  `"3.12":("20240107","3.12.1")`, `"3.13":("20241008","3.13.0")`, etc.
- `python_standalone_mount_name(version)` (mount.py:72-86): default libc `gnu`
  (only `gnu` supported), returns
  `f"python-build-standalone.{release}.{full}-{libc}"` →
  `python-build-standalone.20240107.3.12.1-gnu` for `"3.12"`.
- Resolved EXACTLY like the client mount: `_Mount.from_name(name,
  namespace=GLOBAL)` → `MountGetOrCreateRequest{deployment_name, namespace=GLOBAL,
  environment_name}` with NO `object_creation_type` (a pure GLOBAL lookup). This is
  byte-for-byte our existing `mount_id_for_version` pattern
  (`crates/modal-rust-sdk/src/ops/mount.rs:45-77`) — only the deployment name
  differs.

### A1b. How it lands on PATH — the Dockerfile side (`_image.py:2042-2059`)
The `add_python` branch of `_registry_setup_commands` emits:
```
COPY /python/. /usr/local
ENV TERMINFO_DIRS=/etc/terminfo:/lib/terminfo:/usr/share/terminfo:/usr/lib/terminfo
# inserted at index 1, ONLY for python_minor < 13:
RUN ln -s /usr/local/bin/python3 /usr/local/bin/python
```
The standalone mount is attached as the image **build context**
(`Image.context_mount_id`, `_image.py:2131-2139`, `_image.py:636`); `COPY
/python/. /usr/local` drops `bin/`, `lib/`, … into `/usr/local`, putting `python3`
(and for 3.13+ also `python`) on PATH. For our `"3.12"` choice, `3.12 < 3.13` ⇒ the
single `ln -s python3 python` IS emitted automatically — the client-blessed
equivalent of `python-is-python3`, but a symlink against the standalone install,
NOT an apt package.

### A1c. How the client + its deps get in — the decisive finding
On image **builder version > "2024.10"** (the current default), the modal client
AND its third-party dep closure are NOT installed into the image — the **worker
injects them at container start**:
- `_image.py:2061-2074`, `2147-2148`: `pip install modal` / `COPY
  modal_requirements.txt` are emitted **only when builder_version ≤ "2024.10"**.
  Comment: *"past 2024.10, client dependencies are mounted at runtime."*
- `debian_slim` for `version > "2024.10"` (`_image.py:2456-2497`) has **no** pip
  install and **no** requirements copy.
- The injection is requested by the Function proto:
  `mount_client_dependencies = image_builder_version > "2024.10"`
  (`_functions.py:936-939`, sent at `_functions.py:1014`). Proto field **82**,
  present in our vendored proto at
  `crates/modal-rust-sdk/proto/api.proto:1801` (VERIFIED). With the flag set, the
  worker mounts the version-matched dep closure (`typing_extensions`, `grpclib`,
  `protobuf`, `aiohttp`, …) into the container at boot.
- The modal **source** rides the separate client mount (`mount.py:698-741`,
  mounted at `/pkg`), which we already attach via `Function.mount_ids`
  (`mount.rs`, used at `remote.rs:236` / `deploy.rs:203`).

**Conclusion:** real Modal images don't `pip install modal` because the worker
mounts both the source (client mount) and the dep closure (server-side, gated by
`mount_client_dependencies`). Our `pip install modal` exists ONLY because we never
set that flag. Setting it makes the entire pip layer unnecessary; the standalone
mount supplies a relocatable, NON-PEP-668 `python`/`python3`/`pip`.

### A1d. Net effect: all three hacks dissolve
| Hack | `add_python` resolution |
|---|---|
| `python-is-python3` | auto `RUN ln -s /usr/local/bin/python3 /usr/local/bin/python` for series < 3.13 (`_image.py:2054-2059`). No apt. |
| `--break-system-packages` | we stop pip-installing entirely; and the standalone interpreter is not externally-managed anyway. |
| `pip install modal` (+ apt python3/pip) | set `Function.mount_client_dependencies=true` → worker injects the dep closure at runtime. |

Net image: `FROM <base>` + `COPY /python/. /usr/local` + `ENV TERMINFO_DIRS=…` +
(`RUN ln -s python3 python` for <3.13) + the wrapper bake (+ for deploy, the
`COPY . /` + `cargo build` + `cp`). NO apt layer, NO pip layer → a short
`ImageJoinStreaming` stream, far fewer transport resets.

### A1e. modal-rs is NOT a precedent — do not copy
`references/modal-rs/.../image.rs:290-309,902-915` implements `add_python` as
`apt-get install python… && ln -sf $(command -v python3) …` — the same apt approach
we're replacing. The **Python client** is ground truth; disregard modal-rs here.

## A2. Base image
Both paths keep the rust base (it carries `cargo`, required for the in-body/at-image
build) and add Python via the standalone mount: `rust:{RUST_VER}-slim` +
`add_python("3.12")`. `RemoteConfig::default().base_image == "rust:1-slim"`
(`remote.rs:168`, VERIFIED); `DeployConfig` mirrors it (`deploy.rs:131`). The rust
base ships no Python ⇒ `add_python` provides it (the canonical use case,
`_image.py:2097-2099`). Series `"3.12"` matches our prior `python:3.12-slim` intent;
FILE mode carries no pickled bytecode, so the exact micro version is irrelevant —
`add_python` resolves it from `PYTHON_STANDALONE_VERSIONS`.

## A3. SDK changes — `crates/modal-rust-sdk/src/ops/mount.rs`
Add, mirroring `client_mount_name` / `mount_id_for_version` exactly:
```rust
pub fn python_standalone_mount_name(series: &str) -> String
  // "python-build-standalone.{release}.{full}-gnu"
pub async fn python_standalone_mount_id(&mut self, series: &str, env: Option<&str>)
  -> Result<String>
```
- Bake `PYTHON_STANDALONE_VERSIONS` as a small const `&[(&str,(&str,&str))]` (just
  the pairs from mount.py:45-52; `"3.12" → ("20240107","3.12.1")`).
- `python_standalone_mount_id` reuses the GLOBAL `MountGetOrCreate` lookup body of
  `mount_id_for_version` (mount.rs:45-77) verbatim — only `deployment_name =
  python_standalone_mount_name(series)`. `OBJECT_CREATION_TYPE_UNSPECIFIED`, GLOBAL
  namespace, idempotent, retry-safe, same empty-id guard.

## A4. SDK changes — `crates/modal-rust-sdk/src/ops/image.rs`
Additive on `ImageSpec` (struct at image.rs:52-84); keep apt+pip as a documented
fallback (DEFAULT must be `add_python`):
1. `add_python: Option<String>` (series, e.g. `"3.12"`) + `with_add_python(series)`.
2. `python_standalone_mount_id: Option<String>` +
   `with_python_standalone_mount_id(id)` — the standalone analogue of how the source
   context mount id is threaded.
3. In `dockerfile_commands` (image.rs:166-193): when `add_python` is `Some(series)`,
   after the `FROM` line and BEFORE the wrapper bakes, emit (replicating
   `_registry_setup_commands`):
   - `COPY /python/. /usr/local`
   - if series minor < 13: `RUN ln -s /usr/local/bin/python3 /usr/local/bin/python`
     (between the COPY and the ENV, matching the client's `insert(1, …)`)
   - `ENV TERMINFO_DIRS=/etc/terminfo:/lib/terminfo:/usr/share/terminfo:/usr/lib/terminfo`

   When `add_python` is set, do NOT render `pre_bake_commands`'s apt line or the
   `pip_install_modal` line. The bake (`bake_command`, image.rs:222-228) uses
   `python3 -c` — the standalone provides it — so it works unchanged.
4. In `to_proto` (image.rs:202-216): `context_mount_id` is set from
   `python_standalone_mount_id` when `add_python` is set AND no source context mount
   is in play (the RUN path). For DEPLOY see A6 — the source already owns
   `context_mount_id`, so a base layer is needed.
5. Keep the fallback: retain `with_apt` / `with_pip_install_modal` /
   `pip_install_modal` and the existing render branch, gated so it's only used when
   `add_python` is unset (e.g. a `provision: PythonProvisioning::{AddPython, AptPip}`
   field defaulting to `AddPython`, or simply `add_python.is_none()` selecting the
   legacy branch). Update the module docs (image.rs:1-29) to state `add_python` +
   `mount_client_dependencies` is primary; pip is the documented fallback for a base
   that already carries the deps or an env where runtime dep-mounting is unavailable.

## A5. SDK changes — `crates/modal-rust-sdk/src/ops/function.rs` (REQUIRED)
`add_python` only works with runtime dep injection:
- Add `mount_client_dependencies: bool` to `FunctionSpec` (struct at
  function.rs:51-65), default `true`, with `with_mount_client_dependencies(bool)`.
  Update `FunctionSpec::new` (function.rs:70-83) to default it `true`.
- In the `Function { … }` literal of `function_create` (function.rs:178-191) set
  `mount_client_dependencies: spec.mount_client_dependencies` (proto field 82,
  api.proto:1801). A constant `true` is correct for the current modern builder.
  (Optional refinement: gate on the resolved builder version like
  `_functions.py:936-939`; not required for this pass.)

## A6. Facade adoption

### RUN path — `crates/modal-rust/src/remote.rs:235-268` (clean: no context mount today)
1. Keep `client_mount_id = client.client_mount_id(None)` (remote.rs:236) and the
   source mount (remote.rs:240-242 — Track B may change WHICH files; the mount id
   still flows into `mount_ids`).
2. NEW: `let py_mount_id = client.python_standalone_mount_id("3.12", None).await?;`
3. Replace the image spec (remote.rs:251-256), dropping `with_apt` +
   `with_pip_install_modal`:
   ```rust
   let spec = ImageSpec::from_registry(config.base_image.clone()) // rust:1-slim
       .with_add_python("3.12")
       .with_python_standalone_mount_id(py_mount_id) // → context_mount_id (field 15)
       .with_wrapper_module(WRAPPER_MODULE, run_wrapper_src(&config.package))
       .with_command("ENV RUST_BACKTRACE=1")
       .with_command("ENTRYPOINT []");
   ```
4. FunctionSpec (remote.rs:263-265): rely on the `true` default or
   `.with_mount_client_dependencies(true)`. Mounts stay
   `[client_mount_id, source_mount_id]`.
5. Update the stale apt/`python-is-python3` comments (remote.rs:244-250) to describe
   `add_python` + the auto `ln -s` for series < 3.13.

### DEPLOY path — `crates/modal-rust/src/deploy.rs:169-188` (needs layering)
Constraint: `Image` has ONE `context_mount_id` (api.proto:2392) and `repeated
context_files` (api.proto:2387). The deploy image ALREADY uses `context_mount_id`
for the SOURCE (`deploy.rs:174 .with_context_mount(source_mount_id)` → `COPY . /`).
The standalone needs its OWN context (`COPY /python/. /usr/local`). One layer cannot
carry two context mounts. The client solves this with **image layering**
(`Image.base_images`, proto field 5, api.proto:2385; `_image.py:502-636`).

**Target (A): two-layer deploy image (matches the client's layering).**
- Layer 1 (base): `from_registry(rust:1-slim).with_add_python("3.12")
  .with_python_standalone_mount_id(py_mount_id)` → `context_mount_id` = the
  standalone mount, emits `COPY /python/. /usr/local` + `ln -s` + ENV.
- Layer 2 (top): `base_images=[layer1]`, `context_mount_id` = the source mount,
  emits the wrapper bake + `COPY . /` + `cargo build` + `cp`/bake.
- SDK extension: add `base_image_id: Option<String>` to `ImageSpec` +
  `with_base_image(id)`; in `to_proto`, when set, populate `Image.base_images`
  (field 5) and OMIT the `FROM` line (layered builds have no `FROM`). The top layer
  puts the source mount in its `context_mount_id`; the base layer puts the
  standalone mount in its `context_mount_id`. Build layer 1 via
  `image_get_or_create` first, then layer 2 referencing layer 1's id.
- FunctionSpec (deploy.rs:227-229): set `mount_client_dependencies=true` so the
  deployed `python -m modal._container_entrypoint` finds its deps with no pip layer.
  Mounts stay `[client_mount_id]` only — the deployed body still mounts NO source
  (the runtime invariant; the binary is baked into layer 2).

**Documented DEPLOY fallback** (if layering is deferred): keep `add_python` for the
RUN image (where its context slot is free) and retain the apt+pip path for the
DEPLOY image behind the fallback, with a TODO to layer it. The spec's target is (A).

Rejected: inlining the standalone via `context_files` — it's tens of MB;
`context_files` is for small inline files (api.proto:2387, image.rs:81-83).

## A7. TRACK A acceptance evidence
- Rendered RUN/DEPLOY Dockerfiles contain `COPY /python/. /usr/local` and (for 3.12)
  `RUN ln -s /usr/local/bin/python3 /usr/local/bin/python`, and contain NO `apt-get`,
  NO `pip install`, NO `python-is-python3`, NO `--break-system-packages`.
- `Function.mount_client_dependencies == true` on both create paths.
- `ImageJoinStreaming` log shows no apt/pip steps (just COPY/ENV/ln + bake, and for
  deploy the cargo compile) → measurably faster build, fewer transport resets.
- RUN `.remote()` and DEPLOY `App::deploy`/`call` still return `{sum:42}`.

---

# TRACK B — cargo-metadata-scoped upload + ignore-file resolution

## B0. Today's brittle upload
- A hardcoded 12-entry ignore list lives in `RemoteConfig::default()`
  (`remote.rs:154-167`) and is mirrored by `DeployConfig::for_app`
  (`deploy.rs:124-134`). The `references` entry (remote.rs:158) is a post-hoc patch
  for a real bug — proof the list is brittle.
- `mount_local_dir` (`local_dir.rs:57-111`) walks the ENTIRE `local_root` and prunes
  only via `IgnoreMatcher` (local_dir.rs:307-360), a 4-pattern non-gitignore matcher
  (bare segments + `*.<ext>`).
- Both call sites pass `local_root` + one prefix + that flat list:
  `remote.rs:239-242`, `deploy.rs:207-210`.

VERIFIED win for the default target `example-add`: the workspace-member path-dep
closure is `{examples/add, crates/modal-rust-runtime}` + the workspace
`Cargo.toml`/`Cargo.lock` ≈ a few hundred KB, vs the whole tree.
Correct-by-construction: `cargo build -p example-add` needs exactly those dirs;
crates.io deps are fetched by cargo on Modal.

## B1. cargo-metadata SCOPING (PRIMARY)

### Invocation (at `local_root`, the resolved workspace root from `discover_local_root`)
```
cargo metadata --format-version 1 --no-deps --manifest-path <local_root>/Cargo.toml
```
`--no-deps` is sufficient: the path-dep closure is computed from
`packages[].dependencies[].path` without resolving the crates.io graph (faster, no
network). Parse stdout with `serde_json`.

NOTE (correction to the source note): `serde_json` is a dep of the **facade**
`crates/modal-rust/Cargo.toml` (VERIFIED — it is NOT in modal-rust-sdk). This is
fine because the cargo-metadata `scope` module lives facade-side (B3.2). The new
`ignore` crate goes in `modal-rust-sdk` (where `local_dir.rs` lives).

### JSON fields consumed
Top-level: `workspace_root`, `workspace_members` (member package-id set),
`packages[]` with `.id`, `.name`, `.manifest_path`, and `.dependencies[]` with
`.name`, `.path` (present only for path deps), `.kind` (`null` | `"dev"` |
`"build"`).

### Closure algorithm (workspace-member normal path-dep closure)
```
dir(p)       = p.manifest_path without trailing "/Cargo.toml"
member_dirs  = { dir(p) : p.id ∈ workspace_members }
target       = packages.find(p => p.name == config.package)
closure = {}; stack = [ dir(target) ]
while stack:
    cur = stack.pop(); if cur ∈ closure: continue
    closure.add(cur); p = package whose dir == cur
    for d in p.dependencies where d.kind == null and d.path != null:
        if d.path ∈ member_dirs and d.path ∉ closure: stack.push(d.path)
```
- Follow ONLY `kind == null` (normal) deps. This is LOAD-BEARING and VERIFIED:
  `modal-rust` has a **`dev`** path-dep on `example-add` (`deploy`/`facade`
  dev-dep) — following it would create a cycle and pull example bloat. cargo build
  of the runner binary needs only normal deps.
- VERIFIED results: `example-add → {examples/add, crates/modal-rust-runtime}`;
  `modal-rust → {modal-rust, modal-rust-macros, modal-rust-runtime, modal-rust-sdk}`
  (dev-dep on example-add correctly excluded). No member dir nests inside another
  (9 members, none an ancestor of another) → per-dir walks never double-count.

### Upload set
Each crate dir in `closure` + the workspace `Cargo.toml` + `Cargo.lock` (both at
`workspace_root`; `Cargo.lock` is load-bearing for reproducible builds). Each file's
mount path PRESERVES its path RELATIVE to `workspace_root` under the same remote
prefix, so the in-container layout matches:
`<workspace_root>/examples/add/src/lib.rs` → `<remote_prefix>/examples/add/src/lib.rs`;
`<workspace_root>/Cargo.toml` → `<remote_prefix>/Cargo.toml`. The SDK's
within-crate build resource (`proto/api.proto` read by its `build.rs`) is captured
because the WHOLE crate dir is uploaded.

### Fallback (whole-`local_root`-minus-ignore) when ANY of:
`cargo metadata` is missing / non-zero / unparseable; `local_root` has no
`Cargo.toml`; it's not a `[workspace]` root; or `config.package` is absent from
`packages[]`. Fallback keeps the current single-root walk (with the new ignore
resolution from B2, not the hardcoded list). Emit a `tracing`/stderr note:
`"cargo metadata unavailable (<reason>); uploading whole source root minus ignore files"`.

## B2. IGNORE-FILE resolution (pruning WITHIN the uploaded dirs)
New dep (the only one): add to `crates/modal-rust-sdk/Cargo.toml` deps:
`ignore = "0.4"` (ripgrep's gitignore engine; `ignore-0.4.25.crate` in cache).

### Precedence (highest → lowest), one matcher rooted at `workspace_root`
1. **`.modalignore`** at `workspace_root` — HIGHEST; gitignore syntax incl. `!`
   negation; authoritative if present.
2. **`.gitignore`** — standard. VERIFIED that the repo `.gitignore` already excludes
   `target`, `**/target`, `references/`, `.modal-rust/`, `tmp/`, `.research/`,
   `.env*`, `.venv/`, `__pycache__/`, `*.pyc`, `.DS_Store`. **The `references/` bug
   disappears for free.**
3. **Built-in defaults** (the floor for a project with no `.gitignore`): `target/`,
   `.git/`, `**/*.rlib`.

Disable surprise sources: `.git_global(false)`, `.parents(false)`, `.ignore(false)`
(no auto `.ignore` pickup); keep `.git_ignore(true)`. Keep `.hidden(false)` so
dotfiles like a crate's `.cargo/config.toml` are NOT auto-dropped (gitignore rules
decide).

### Composition with cargo scoping
cargo-metadata picks WHICH dirs (B1); the ignore matcher prunes WITHIN them.
- PRIMARY: walk EACH closure crate dir; query the root-anchored matcher with the
  path RELATIVE to `workspace_root`; prune ignored dirs early (stops descent into
  `target/`). The root `Cargo.toml`/`Cargo.lock` are added explicitly and EXEMPT
  from ignore matching (never ignorable build inputs).
- FALLBACK: walk the single `local_root` with the same matcher.

## B3. SDK + facade changes

### B3.1 `crates/modal-rust-sdk/src/ops/local_dir.rs`
Replace the hand-rolled `IgnoreMatcher` (local_dir.rs:307-360) and add a
multi-dir collector; keep `upload_files`/`ensure_file_uploaded`/probe/blob logic
(local_dir.rs:113-219) UNCHANGED — only file SELECTION changes.
- `build_matcher(workspace_root) -> ignore::gitignore::Gitignore`: layer via
  `GitignoreBuilder` in `matched_path_or_any_parents` "last match wins" order — add
  **defaults first, then `.gitignore`, then `.modalignore` LAST** so `.modalignore`
  (and its negations) win.
- `collect_files_for_dirs(workspace_root, dirs: &[PathBuf], extra_files: &[PathBuf],
  remote_prefix, &matcher)`: walk each `dir` (via `ignore::Walk` or the retained
  `walkdir`), emit `<remote_prefix>/<rel-to-workspace_root>` (reuse
  `to_posix`/`normalize_remote_prefix`/`file_mode` unchanged, local_dir.rs:283-303),
  then append `extra_files` verbatim. Prefer `ignore::Walk` to retire `walkdir`;
  minimal-diff = keep both. Keep `collect_files` (local_dir.rs:231-281) for the
  fallback path, but matched by the new `build_matcher` instead of `IgnoreMatcher`.
- New entrypoint alongside `mount_local_dir` (signature of `mount_local_dir` at
  local_dir.rs:57-63 stays the fallback entrypoint):
  ```rust
  pub async fn mount_workspace_closure(
      &mut self, workspace_root: &Path, crate_dirs: &[PathBuf],
      extra_files: &[PathBuf], remote_path: &str, environment: Option<&str>,
  ) -> Result<String>
  ```
  Both feed the same `upload_files` + EPHEMERAL `MountGetOrCreate` (local_dir.rs:82-111).

### B3.2 `crates/modal-rust/src/scope.rs` (NEW small facade module)
```rust
pub(crate) fn workspace_closure(workspace_root: &Path, package: &str)
    -> Option<(Vec<PathBuf> /*crate dirs*/, Vec<PathBuf> /*root Cargo.toml,lock*/)>
```
Shells `cargo metadata --no-deps`, parses with `serde_json`, runs the B1 algorithm.
Returns `None` to signal fallback (drives B1's fallback list).

### B3.3 `RemoteConfig` (remote.rs:130-172) / `DeployConfig` (deploy.rs:99-135)
- Drop the hardcoded `ignore: Vec<String>` default (remote.rs:154-167;
  deploy.rs:113,130). New ignore behavior = built-in defaults + auto-discovered
  `.gitignore`/`.modalignore`, resolved in the SDK.
- Add `pub modalignore_name: String` (default `".modalignore"`) and
  `pub use_cargo_scoping: bool` (default `true`; `false` forces the whole-root
  fallback). `RemoteConfig::default()` / `App::deploy` zero-config keeps working.
- Reuse `package` and `local_root` as-is — `package` is the scoping target,
  `local_root` is `workspace_root`. `DeployConfig::for_app` (deploy.rs:124) keeps
  mirroring `RemoteConfig` so the deploy upload matches the run upload.

### B3.4 Call-site wiring (the only behavioral diff)
At `remote.rs:239-242` and `deploy.rs:207-210`, replace the flat-list call with:
```rust
match (config.use_cargo_scoping,
       scope::workspace_closure(&config.local_root, &config.package)) {
    (true, Some((dirs, extras))) =>
        client.mount_workspace_closure(
            &config.local_root, &dirs, &extras, PREFIX, None).await?,
    _ => client.mount_local_dir(&config.local_root, PREFIX, &[], None).await?,
            // fallback uses the new build_matcher (defaults+.gitignore+.modalignore)
}
```
where `PREFIX` is `config.remote_src` (RUN) / `DEPLOY_SRC` (deploy). Everything
downstream — client/standalone mounts, image build, FunctionCreate, publish, invoke —
is UNTOUCHED; `retry_unary` on every RPC and the create/invoke/deploy logic
(remote.rs:229-294, deploy.rs:198-259) stay exactly as-is.

### B3.5 Doc note (required)
On `RemoteConfig`/`DeployConfig` and the `local_dir` module header:
> The source upload carries ONLY the cargo dependency closure of the target package
> (its workspace-member normal path deps) plus the workspace `Cargo.toml`/`Cargo.lock`.
> Non-source assets (datasets, model weights, fixtures) are NOT uploaded with the
> source and must be attached via **Modal Volumes**, not the source mount.
> Ignore-file precedence: `.modalignore` (highest) → `.gitignore` → built-in defaults
> (`target/`, `.git/`, `**/*.rlib`).

## B4. Tests to add/port
- Replace the `ignore_matcher_*` unit tests with `Gitignore`-precedence tests
  (`.modalignore` overrides `.gitignore` overrides defaults; negation re-includes)
  in `local_dir.rs` (tests at local_dir.rs:363+).
- Add a `workspace_closure` test on a temp workspace fixture asserting
  `example-add → {add, runtime}` and that the `modal-rust → example-add` dev-dep is
  excluded.
- Update `default_config_has_expected_shape` (remote.rs:454-476): it currently
  asserts the `references`/`workpads` ignore entries (remote.rs:461-473) — those go
  away. Assert instead `use_cargo_scoping == true`,
  `modalignore_name == ".modalignore"`.

## B5. TRACK B acceptance evidence
- For `example-add`, the uploaded mount contains only `examples/add/**`,
  `crates/modal-rust-runtime/**`, and the workspace `Cargo.toml`/`Cargo.lock` (no
  `references/`, `workpads/`, sibling crates, or `target/`).
- Removing the hardcoded `references` entry no longer leaks it (the `.gitignore`
  layer excludes it).
- RUN `.remote()` and DEPLOY `App::deploy`/`call` still return `{sum:42}` (the
  scoped closure compiles on Modal).

---

# Files changed + new deps (consolidated)
SDK (`crates/modal-rust-sdk`):
- `src/ops/mount.rs` — A3 `python_standalone_mount_name` + `python_standalone_mount_id`.
- `src/ops/image.rs` — A4 `add_python` / standalone-context render + base-layer
  support (`base_image_id`); apt+pip kept behind fallback.
- `src/ops/function.rs` — A5 `mount_client_dependencies` (default true) → proto field 82.
- `src/ops/local_dir.rs` — B3.1 `build_matcher` + `collect_files_for_dirs` +
  `mount_workspace_closure`; retire `IgnoreMatcher`.
- `Cargo.toml` — NEW dep `ignore = "0.4"`.

Facade (`crates/modal-rust`):
- `src/scope.rs` — NEW (B3.2 `workspace_closure`; uses existing `serde_json`).
- `src/remote.rs` — A6 RUN image (`add_python`, drop apt/pip), B3.3/B3.4 config +
  scoped upload wiring; update tests + comments.
- `src/deploy.rs` — A6 DEPLOY two-layer image (or documented fallback), B3.3/B3.4
  config + scoped upload wiring.

Proto: no changes — `mount_client_dependencies=82`, `base_images=5`,
`context_files=7`, `context_mount_id=15` already present (api.proto:1801, 2385,
2387, 2392 — VERIFIED).
