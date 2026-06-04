# DEPLOY-path build spec (P5): copy=True image, deploy wrapper, App::deploy/call, ephemeral-run fix

Build-ready synthesis. All paths absolute. Line cites are to files read this session.
Where the two source notes disagreed, this spec **prefers the proven `deploy_app.py` recipe**
and marks the one live-verify decision point explicitly (§A4).

## Invariant (do NOT violate)
DEPLOY = build at IMAGE-BUILD time. The deployed runtime NEVER runs cargo and NEVER mounts source;
it execs ONLY the prebuilt `/app/modal_runner` baked into an image layer. `call` performs NO upload
and NO build. cargo runs ONLY during the image build. (boundaries.md run-vs-deploy boundary; design
stance #2.) DEPLOY is the ONLY path that uses persistent `AppPublish`; RUN becomes ephemeral (§C).

---

## 0. Ground truth confirmed (proto + Python client + prototype)

- `crates/modal-rust-sdk/proto/api.proto:2384` `message Image` carries the deploy fields:
  `repeated ImageContextFile context_files = 7`, `string context_mount_id = 15`
  (also `repeated BaseImage base_images = 5`, `repeated string dockerfile_commands = 6`).
  `ImageContextFile {filename=1, data=2}` (`api.proto:2413`). `BaseImage {image_id=1, docker_tag=2}` (`api.proto:809`).
- `ImageGetOrCreateRequest` (`api.proto:2431`): `image=2`, `app_id=4`, `builder_version=9`.
  Our `image_get_or_create` (`crates/modal-rust-sdk/src/ops/image.rs:203-246`) already sets
  `image`, `app_id`, `builder_version` and polls the build via `ImageJoinStreaming`
  (`image.rs:306-312`, `[image-build]` logs). **No RPC change needed; only `ImageSpec::to_proto` gains fields.**
- Python `add_local_dir(local, remote, copy=True)` (`references/modal-client/py/modal/_image.py:771-847`)
  → `_add_mount_layer_or_copy(mount, copy=True)` (`:452-454`) → `_copy_mount(mount, "/")` (`:718-733`),
  which builds `dockerfile_commands = ["FROM base", "COPY . /"]` and sets the mount as the build
  context. The wiring is `_image.py:631-644`: `Image(..., context_files=..., context_mount_id=mount.object_id)`.
  **So `context_mount_id` = the uploaded source mount's `mount_id` — exactly what our `mount_local_dir` returns.**
- The mount carries files prefixed at its `remote_path` (e.g. `/app/src/<rel>`), and a `COPY . /`
  drops the context tree at root so files land at `/app/src`. Our `mount_local_dir(local, remote_path, ...)`
  (`crates/modal-rust-sdk/src/ops/local_dir.rs:57-111`) already lays files at `<remote_path>/<rel>`.
- `from_registry(add_python=…)` uses the context-mount slot for a python-standalone mount
  (`_image.py:2131-2156`, `_registry_setup_commands :2036-2081`). Our proven image does NOT use
  `add_python`; it uses apt `python3` + `python-is-python3` + `pip --break-system-packages modal`
  (`crates/modal-rust/src/remote.rs:236-241`, validated 2026-06-04). **The context-mount slot is therefore
  FREE for the deploy source mount. No conflict.**
- Proven RUN recipe was already EPHEMERAL-app based: `AppCreate(ephemeral) → Image → Precreate →
  FunctionCreate(FILE) → AppPublish → from_name → invoke` (`knowledge.md:639-643`). The facade
  regressed to `app_get_or_create_id` (persistent) — that is the crash-loop-clutter root cause (§C).
- Proven DEPLOY recipe (`workpads/prototype/deploy_app.py:53-96`):
  `from_registry("rust:1-slim", add_python) → entrypoint([]) → env → add_local_dir(src,"/app/src",copy=True)
  → run("cd /app/src && cargo build --release -p <pkg> --bin modal_runner")
  → run("cp .../target/release/modal_runner /app/modal_runner && chmod +x")`; bare deploy handler writes
  `/tmp/in.json` and execs ONLY `/app/modal_runner` (NO cargo, NO mount).

---

## A. SDK additive change — `crates/modal-rust-sdk/src/ops/image.rs`

The RUN image attaches source as a runtime `Function.mount_id` and builds in-body. DEPLOY needs the
source in the image BUILD CONTEXT so `cargo build` runs at build time. Modal does this via
`Image.context_mount_id` + a Dockerfile `COPY`. We keep our proven SINGLE-`FROM` single-layer model
(`image.rs:140-167`); we do NOT adopt the Python client's layered `FROM base` indirection.

### A1. `ImageSpec` gains two fields (additive; do NOT touch existing fields/order)
Add to `struct ImageSpec` (`image.rs:51`):
```rust
/// Image build CONTEXT mount id (a mount_id from `mount_local_dir`). When set,
/// emitted as Image.context_mount_id (proto field 15); a `COPY` step then brings
/// the context into an image LAYER at build time. None for RUN (default); DEPLOY-only.
pub context_mount_id: Option<String>,
/// Inline small context files -> Image.context_files (proto field 7). Unused by the
/// rust deploy recipe (source rides the context mount); kept for proto parity. Default empty.
pub context_files: Vec<(String, Vec<u8>)>,
```
Init both in `from_registry`/constructors (`image.rs:75-88`): `context_mount_id: None`, `context_files: Vec::new()`.
RUN stays byte-identical: `None` → empty proto string (proto default), `Vec::new()` → empty repeated.

### A2. Builders (additive, after `with_pip_install_modal` `:129`)
```rust
/// Set the image build-context mount (a mount_id from `mount_local_dir`). The caller adds
/// the matching `COPY` step (use `with_command("COPY . <dest>")`).
pub fn with_context_mount(mut self, mount_id: impl Into<String>) -> Self {
    self.context_mount_id = Some(mount_id.into());
    self
}
```
COPY and the cargo-build/cp `RUN`s are raw Dockerfile passthrough via the EXISTING `with_command`
(they ride `extra_commands`, rendered last by `dockerfile_commands` `image.rs:165`). No new field for them.

### A3. `to_proto` emits the new fields (`image.rs:171-176`)
```rust
fn to_proto(&self) -> Image {
    Image {
        dockerfile_commands: self.dockerfile_commands(),
        context_mount_id: self.context_mount_id.clone().unwrap_or_default(),
        context_files: self.context_files.iter().map(|(filename, data)| {
            ImageContextFile { filename: filename.clone(), data: data.clone() }
        }).collect(),
        ..Default::default()
    }
}
```
Import `ImageContextFile` alongside `Image, ImageGetOrCreateRequest, …` at `image.rs:38`. `base_images`
stays empty (single-`FROM` model). **Render order is load-bearing** (`dockerfile_commands()` `:140-167`):
`FROM → pre_bake(apt) → pip → wrapper bake → extra_commands`. The `COPY` and cargo/cp `RUN`s are
appended as extra_commands, so they run AFTER python/pip/wrapper exist and AFTER the context is
available — correct for a build-time compile, matching `deploy_app.py` ordering.

### A4. COPY / context-mount-path — DECISION POINT (prefer proven recipe; one live check)
`deploy_app.py` calls `add_local_dir(src, "/app/src", copy=True)`: the mount carries files prefixed at
`/app/src`, and Modal's `_copy_mount` emits `COPY . /` (context root → `/`), landing files at `/app/src`.
This is the **primary, proven** form. So:

- **Primary (matches deploy_app.py + note-1 confirmed mount layout):**
  `mount_local_dir(local_root, "/app/src", &ignore, None)` → `with_command("COPY . /")`.
  Files arrive at `/app/src/<rel>`; cargo builds at `/app/src`.
- **Fallback (only if a live build shows COPY context-root differs):**
  `mount_local_dir(local_root, "/", &ignore, None)` → `with_command("COPY . /app/src")`.

The SDK code is agnostic to which is chosen (both are `with_context_mount(id)` + a raw `with_command`).
**Decide at live-verify time; default to Primary.** Do NOT route COPY through `with_apt`/`with_pip`.

### A5. Caller composition (deploy `ensure` sequence)
```rust
let client_mount_id  = client.client_mount_id(None).await?;                       // modal importable
let source_mount_id  = client.mount_local_dir(&cfg.local_root, "/app/src",        // Primary (§A4)
                                              &cfg.ignore, None).await?;          // UPLOAD = build context
let app_id           = client.app_get_or_create_id(&cfg.app_name, None).await?;   // PERSISTENT named app
let spec = ImageSpec::from_registry(cfg.base_image.clone())                       // "rust:1-slim"
    .with_apt(&["python3", "python3-pip", "python-is-python3"])                   // same crash-loop fix as RUN
    .with_pip_install_modal()                                                     // --break-system-packages modal
    .with_wrapper_module(DEPLOY_WRAPPER_MODULE, DEPLOY_WRAPPER_SRC)               // §B1, no {{PACKAGE}}
    .with_context_mount(source_mount_id)                                          // -> context_mount_id (field 15)
    .with_command("COPY . /")                                                     // context -> /app/src layer (§A4)
    .with_command(format!(
        "RUN cd /app/src && cargo build --release -p {} --bin modal_runner", cfg.package)) // cargo AT BUILD TIME
    .with_command(
        "RUN cp /app/src/target/release/modal_runner /app/modal_runner && chmod +x /app/modal_runner")
    .with_command("ENV RUST_BACKTRACE=1")
    .with_command("ENTRYPOINT []");
let image_id = client.image_get_or_create(&app_id, &spec).await?;                 // cargo runs HERE (build logs)
```
Must match the proven recipe (`deploy_app.py:53-69`): `-p <PACKAGE>` required (shared `modal_runner`
bin name across workspace members); default `CARGO_TARGET_DIR` ⇒ binary at `/app/src/target/release/modal_runner`;
cp+chmod bakes it to the fixed `/app/modal_runner`. `with_apt` renders the single-RUN update+install+clean
(`image.rs:117-124`); `with_pip_install_modal` emits
`python3 -m pip install --no-cache-dir --break-system-packages modal` (`image.rs:157-160`). Both reused unchanged.

### A6. Offline tests (extend `image.rs` tests `:332-404`; existing RUN tests stay green)
- `with_context_mount` ⇒ `to_proto().context_mount_id == "<id>"`; default/RUN spec ⇒ `""`.
- Deploy `dockerfile_commands()` contains `COPY . /` then a `cargo build --release -p ... --bin modal_runner`
  line then the `cp ... /app/modal_runner` line, IN THAT ORDER, with apt/pip BEFORE them.
- `DEPLOY_WRAPPER_SRC` contains `/app/modal_runner`, `--input-file`; contains NO `cargo`, NO `/src`,
  NO `CARGO_` (proves the deployed wrapper never builds/mounts source).

---

## B. DEPLOY surface — `crates/modal-rust/src/deploy.rs` (new sibling to remote.rs)

### B1. DEPLOY FILE-mode wrapper (distinct module name; never collides with the run wrapper)
Constants in `deploy.rs`:
- `DEPLOY_WRAPPER_MODULE = "modal_rust_deploy_wrapper"` (baked to `/root/modal_rust_deploy_wrapper.py`).
- `DEPLOY_WRAPPER_CALLABLE = "handler"`.
- No `{{PACKAGE}}` substitution ⇒ `DEPLOY_WRAPPER_SRC` is a `&'static str` (simpler than run_wrapper_src,
  `remote.rs:124-126`).

Ports `deploy_app.py:77-96` `call_entrypoint`, FILE-mode shape:
```python
"""modal-rust FILE-mode DEPLOY wrapper (ports deploy_app.py call_entrypoint).

Baked to /root/modal_rust_deploy_wrapper.py. Deployed-runtime invariant: NO cargo, NO source
mount, NO cache volume. Execs ONLY the prebuilt /app/modal_runner baked at IMAGE-BUILD time,
and RETURNS the one-line JSON envelope verbatim (the facade parses it)."""
import subprocess, sys

_RUNNER = "/app/modal_runner"   # baked at IMAGE-BUILD time; never rebuilt


def handler(entrypoint, input_json):
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
```
Byte-distinct from the run wrapper: no `os/shutil`, no `_env`/`_build_dir`/`_build`, no `CARGO_*`,
no `/src`, no `cargo`. Same call contract as RUN: invoked with `args=(entrypoint, input_json)`,
`kwargs={}`; returns the runner stdout envelope verbatim, so `remote::parse_envelope`
(`remote.rs:276-288`) is REUSED unchanged.

### B2. `DeployConfig` (mirror `RemoteConfig`, `remote.rs:129-172`)
Same fields (`local_root`, `package`, `ignore`, `base_image = "rust:1-slim"`, `timeout_secs`) PLUS:
- `app_name: String` — STABLE deploy app name; default `"modal-rust-add-deploy"`, override env
  `MODAL_RUST_DEPLOY_APP`. Re-deploys REPLACE under this name (AppPublish is set-state, `app.rs:109-118`),
  so re-runs do NOT accumulate (verification rule).
- No runtime source mount path; the deploy image COPYs to `/app/src` (constant). `timeout_secs` may be
  the SDK default; a modest 300s is fine (no in-body build).

### B3. `deploy::deploy_function(...) -> Result<DeployedApp>` and `App::deploy(name) -> Result<DeployedApp>`
PERSISTENT deploy. Reuses existing ops verbatim; the only structural change vs RUN is source-as-context-mount
(not a runtime `Function.mount_id`):
1. `client_mount_id(None)` (`mount.rs:38`) — modal importable.
2. `mount_local_dir(local_root, "/app/src", &ignore, None)` (`local_dir.rs:57`) — UPLOAD source as BUILD CONTEXT.
3. `app_get_or_create_id(app_name, None)` (`app.rs:41`) — PERSISTENT named app id.
4. Build deploy image (§A5): `image_get_or_create(&app_id, &spec)` — **cargo runs HERE, at image-build time**
   (build logs show `Compiling`/`cargo build --release` during `ImageJoinStreaming`).
5. `function_precreate(&app_id, DEPLOY_WRAPPER_CALLABLE)` (`function.rs:133`).
6. `function_create` FILE mode (`function.rs:172`):
   `FunctionSpec::new(DEPLOY_WRAPPER_MODULE, DEPLOY_WRAPPER_CALLABLE, &image_id)
        .with_mount_ids(vec![client_mount_id]).with_timeout_secs(cfg.timeout_secs)`
   — **client mount ONLY; NO source mount** (the binary is baked in the image layer). Hard invariant in the API.
7. **PERSISTENT** `app_publish(&app_id, app_name, {"handler"->fid}, definition_ids)` (`app.rs:102`) — deploy-only.
8. `function_from_name(app_name, DEPLOY_WRAPPER_CALLABLE, None)` (`function.rs:240`) →
   return `DeployedApp { name, function_id, image_id, url }`.

### B4. `call` — deployed-function invocation (NO upload, NO build at call time)
`App::call(&self, app_name, entrypoint, input) -> Result<Out>` (or a method on `DeployedApp`):
1. `fid = function_from_name(app_name, "handler", None)` (`function.rs:240`).
2. `envelope: String = invoke_cbor(&fid, &(entrypoint, input_json), &empty_kwargs)` (`invoke.rs:77`) —
   **default ~600s deadline** (no in-body build at call time; binary is prebuilt). Do NOT use the
   1800+120s RUN deadline.
3. `parse_envelope::<Out>(&envelope)` (reuse `remote.rs:276` verbatim).

`call` does NOT touch `mount_local_dir`, `image_get_or_create`, or `app_publish`. That absence IS the
deploy invariant: no upload, no build at call time.

---

## C. RUN-vs-DEPLOY app lifecycle FIX (crash-loop-clutter root cause)

**Root cause:** `App::connect_with_registry` (`app.rs:74-87`) calls `app_get_or_create_id(name, None)`
→ a PERSISTENT named app; `ensure_function` (`remote.rs:255-264`) then `app_publish`-es into it ⇒ every
`.remote()` leaves a lingering persistent app (pre-fix ones crash-loop; a stale 11:30 deploy was found
crash-looping on `ModuleNotFoundError: typing_extensions`).

**Fix — make the RUN app EPHEMERAL. DEPLOY stays the only persistent path:**

1. **`app.rs connect_with_registry`** (`app.rs:75-76`): replace
   ```rust
   let app_id = client.app_get_or_create_id(name, None).await?;
   ```
   with
   ```rust
   let app_id = client.app_create_ephemeral(name, None).await?;  // RUN = ephemeral, auto-GC on disconnect
   ```
   `app_create_ephemeral` already exists (`ops/app.rs:67-92`, `AppState::Ephemeral`); pass `name` as the
   description. The stored `app_name` (`app.rs:82`) stays `= name` for `from_name` resolution. Thread an
   `is_ephemeral` flag (or distinct `RemoteHandle`) so the facade chooses ephemeral for `.remote()` and
   persistent only via `App::deploy` (§B3 step 3).

2. **`remote.rs ensure_function` — KEEP `app_publish`** (the subtle, proven part, `knowledge.md:639-643`):
   the working spike was `AppCreate(ephemeral) → … → AppPublish → from_name → invoke`. `AppPublish` on an
   EPHEMERAL `app_id` publishes the function WITHIN the ephemeral app's scope (so `function_from_name`
   resolves for invocation) but creates NO persistent named deployment — the app is GC'd when the client
   disconnects. The `app_publish` CALL is shared; the app STATE (ephemeral vs persistent-named) is what
   differs. **Do NOT remove the publish; only the app-creation RPC swaps.** This is the minimal change and
   needs NO change to the create/invoke sequence (the original `{sum:42}` proof used exactly this shape).

   > Note left in code: RUN now publishes into an ephemeral scope; persistent publish is DEPLOY-only.

3. **Lifetime of the ephemeral app across the call:** ephemeral apps are GC'd when the client disconnects
   (`ops/app.rs:80-81`). The `App` holds one `ModalClient` for its lifetime (`app.rs:33`), and
   `remote_invoke` creates-then-invokes within ONE live connection (`app.rs:96-139`), so the app is alive
   for the call's duration. No heartbeat needed for a single synchronous invoke in one process.
   - **Fallback A (only if live shows the ephemeral app GCs mid cold-build invoke):** spawn a `tokio` task
     calling a thin `client.app_heartbeat(app_id)` (`AppHeartbeat`, `api.proto:4144`) every ~25s for the
     call, then drop it.
   - **Fallback B (only if live shows ephemeral apps lingering):** after the final invoke (or on `App` drop),
     call a thin `client.app_stop(app_id)` (`AppStop`, `api.proto:4152`, `AppStopRequest{app_id, source}` `:602`)
     — one `retry_unary` wrapper mirroring `app_publish`.
   - Default design: rely on automatic GC; add A/B ONLY if a live run requires it. Do NOT add speculatively.
   - **Verification stance:** drive whichever variant is verified live to a terminal result; do not punt to a monitor.

4. **`function_id` memoization (`app.rs:40,105-117`) is unchanged and still correct** for RUN: within one
   `App`/connection the ephemeral app + published function persist for the process.

### Net lifecycle split after the fix
| | RUN (`.remote()`) | DEPLOY (`App::deploy` / `call`) |
|---|---|---|
| app create | `app_create_ephemeral(name)` (`app.rs`) | `app_get_or_create_id(name)` (`deploy.rs`) |
| app name | per-connect (ephemeral scope) | STABLE `modal-rust-add-deploy` |
| app_publish | yes (into ephemeral scope) | yes (PERSISTENT deploy) |
| source | runtime mount `/src` + cargo in-body | `context_mount_id` + COPY + cargo AT BUILD; NO runtime source mount |
| call time | builds cold (1800s deadline) | execs prebuilt `/app/modal_runner`, no build (~600s deadline) |
| lifetime | auto-GC on disconnect (no lingering) | survives until redeploy/replace |

---

## D. Verification plan (WORKING.md gates + live)

**Offline gates (default-members):** `cargo fmt --check`; `cargo clippy --all-targets -- -D warnings`;
`cargo build`; `cargo test`. New unit tests: §A6 (image context-mount/COPY/cargo-order); `deploy.rs`
`DEPLOY_WRAPPER_SRC` is python-ish, contains `/app/modal_runner` + `--input-file`, contains NO `cargo`,
NO `/src`, NO `CARGO_`; `DeployConfig` defaults (stable app name). Existing RUN tests stay green. Keep no-CUDA CI green.

**Live (behind `#[ignore]` + the `live` feature):**
- **DEPLOY** → STABLE app `modal-rust-add-deploy`; re-runs REPLACE (no accumulation; FIXED image, no
  crash-loop; do not leave broken deploys behind). Assert
  `App::call("modal-rust-add-deploy", "add", AddInput{40,2}).await? == {sum:42}`.
  **Cargo-at-build-not-call evidence:** image-build logs (streamed via `image_get_or_create`/`ImageJoinStreaming`,
  `image.rs:306-312` `[image-build]`) contain the `cargo build --release` / `Compiling` lines; the CALL
  produces the envelope with NO cargo in the function's runtime stderr.
- **RUN regression:** `.remote()` still yields `{sum:42}` AND leaves NO persistent app behind (after the
  process exits, `app list` shows no lingering named RUN app — only the ephemeral, which GCs).
- Reuse `retry_unary`/`retry_transient` on EVERY new/reused RPC (and the conditional `app_stop`/`app_heartbeat`).
  Modal flakiness ⇒ RETRY, never block. Drive the live proof to a terminal result.

---

## E. Files to touch
- `crates/modal-rust-sdk/src/ops/image.rs` — add `context_mount_id` + `context_files` fields,
  `with_context_mount`, `to_proto` emit, `ImageContextFile` import, tests (§A, additive only).
- `crates/modal-rust/src/deploy.rs` — NEW: `DEPLOY_WRAPPER_MODULE`/`DEPLOY_WRAPPER_CALLABLE`/`DEPLOY_WRAPPER_SRC`,
  `DeployConfig`, `DeployedApp`, `deploy_function`, `call`; reuse `remote::parse_envelope` (§B).
- `crates/modal-rust/src/app.rs` — `connect_with_registry`: `app_get_or_create_id` → `app_create_ephemeral`
  (the ~1-line lifecycle fix); add `App::deploy` + `App::call` driving `deploy.rs`; add `mod deploy;` (§C).
- `crates/modal-rust/src/remote.rs` — UNCHANGED (KEEP `app_publish`; correctness comes from the ephemeral
  app, not from removing publish). Optionally add a comment noting RUN now publishes into an ephemeral scope.
- (Conditional) `crates/modal-rust-sdk/src/ops/app.rs` — add `app_heartbeat` and/or `app_stop` ONLY if
  live-verify shows the ephemeral app GCs mid-call or lingers.

---

## Citations
`remote.rs:24-29/48-126/154-167/214-271/255-264/276-288`; `app.rs:33/40/41/67-92/74-87/96-139/102/105-118`;
`ops/function.rs:133/172/212-225/240-268`; `ops/image.rs:38/51/64/75-88/117-124/129/140-167/171-176/203-246/306-312/332-404`;
`ops/local_dir.rs:57-111`; `ops/mount.rs:38`; `ops/invoke.rs:77/96`;
`api.proto:602/809/2384/2392/2413/2431/4144/4152`; `_image.py:452-454/631-644/718-733/771-847/2036-2081/2131-2156`;
`knowledge.md:639-643`; `deploy_app.py:24/31/53-96`; `dev_app.py:1-19/41/50`.
