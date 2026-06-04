# remote-build-spec.md — local-source UPLOAD + real `.remote()` (RUN path)

Authoritative, build-ready spec. Merges three notes (SDK upload capability; FILE-mode
run wrapper + run image; `Function::remote` wiring) into one. Concrete enough to
implement without re-deriving. Cited against the live tree (verified 2026-06-04).

## 0. Goal & the hard build boundary

Make this true, run live on Modal, with the user's REAL Rust `add` built **in the
function body** (not an echo), with NO modal CLI and NO per-project `.py`:

```rust
app.function("add").remote(AddInput{a:40,b:2}).await? == AddOutput{sum:42}
```

**BUILD BOUNDARY (non-negotiable, RUN path):** source is MOUNTED (`copy=False`
equivalent) and `cargo build` runs IN THE FUNCTION BODY at execution time — NEVER at
image-build time. There is NO `cargo` line in `dockerfile_commands`. The live proof
MUST show `cargo` in the FUNCTION/runtime logs (wrapper stderr), not the image-build
log. (Deploy = build-at-image-time is a LATER milestone; do not do it here.)

**FROZEN — do not change:** `modal-rust-runtime` (Registry/HandlerFn/typed!/run_cli/
RunnerError), the runner CLI protocol, the macros, the run-vs-deploy split. Do NOT
rewrite the proven SDK ops or the facade `.local()`; ADD to them.

## 1. The three pieces (and how they connect)

```
facade Function::remote(input)                         [crates/modal-rust]
  └─ App::remote_invoke(entrypoint, input_json)
       ├─ ensure_function() once per App (OnceCell):
       │    client_mount_id()                          [SDK ops/mount.rs:37  — EXISTS]
       │    mount_local_dir(root,"/src",ignore) ──────►[SDK NEW: ops/local_dir.rs + ops/blob.rs]  §2
       │    image_get_or_create(rust+python+wrapper) ──[SDK ops/image.rs + small additive §3.4]
       │    function_precreate / function_create FILE  [SDK ops/function.rs — EXISTS]
       │    app_publish / function_from_name           [SDK ops/{app,function}.rs — EXISTS]
       └─ invoke_cbor((entrypoint,input_json),{})─►R=String   [SDK ops/invoke.rs:71 — EXISTS]
            wrapper handler(entrypoint,input_json):    [baked /root/modal_rust_run_wrapper.py]  §3
              cargo build -p PKG --bin modal_runner    ◄── cargo IN FUNCTION BODY (boundary)
              exec modal_runner --entrypoint ... --input-file /tmp/in.json
              return runner stdout envelope (string)
       parse envelope String → Result<Out, Error>      §4
```

---

## 2. SDK capability: `mount_local_dir` (local-dir UPLOAD → mount_id)

### 2.1 Where it lives
- NEW module `crates/modal-rust-sdk/src/ops/local_dir.rs` (walk + ignore + remote-path
  mapping + per-file orchestration + the `MountGetOrCreate` finalize).
- NEW module `crates/modal-rust-sdk/src/ops/blob.rs` (`BlobCreate` → HTTP PUT).
- Register both in `ops/mod.rs` (`pub mod local_dir; pub mod blob;`).
- Do NOT touch `ops/mount.rs` — it is lookup-only (client mount) and stays as-is. The
  modal-rs `mount.rs`/`blob_transfer.rs` are reference precedent only (gitignored);
  port the LOGIC, not the file.

### 2.2 Public API (on `ModalClient`)
```rust
impl ModalClient {
    /// Upload a local directory as an EPHEMERAL Modal mount; return its mount_id.
    /// Files map to "<remote_path>/<rel-as-posix>". `ignore` is the small pattern
    /// subset of §2.3 matched against the path RELATIVE to `local_dir`.
    pub async fn mount_local_dir(
        &mut self,
        local_dir: impl AsRef<std::path::Path>,
        remote_path: &str,            // "/src"
        ignore: &[&str],              // ["target",".git",".modal-rust","**/*.rlib"]
        environment: Option<&str>,
    ) -> Result<String>;              // mount_id, e.g. "mo-..."
}
```
Use `self.env_or_default(environment)` (`client.rs:75`) and `self.inner_mut()`
(`client.rs:88`), same pattern as `ops/mount.rs:52`.

### 2.3 Walk + ignore + remote mapping
Mirrors `dev_app.py:69-74` and Python `_MountDir.get_files_to_upload`
(`references/.../mount.py:161-190`).
- Walk `local_dir` recursively with **early directory pruning** (use `walkdir`,
  recommended). Pruning `target/` is CRITICAL — it can hold tens of thousands of files.
- `rel = file.strip_prefix(local_dir)`.
- Ignore matcher — implement EXACTLY the 4-pattern subset `dev_app.py` proves works; do
  NOT pull a full gitignore engine (keep CI green, dep-light):
  - bare segment (`target`, `.git`, `.modal-rust`) → prune any dir/file whose **path
    contains that component**; prune the directory early so we never descend.
  - `**/*.rlib` (and any `*.<ext>` / `**/*.<ext>`) → ignore any file whose name ends in
    `.rlib`.
- `mount_filename = "<remote_path>/<rel-as-posix>"`, always POSIX `/`, no leading `//`.
  So `<root>/examples/add/Cargo.toml` → `/src/examples/add/Cargo.toml`. This is what the
  wrapper's `cargo build` in `/src` expects.
- mode: capture Unix `st_mode & 0o7777` → `MountFile.mode` (`Option<u32>`); `None` on
  non-Unix.

### 2.4 Per-file sha256 + inline-vs-blob threshold
- `LARGE_FILE_LIMIT = 4 * 1024 * 1024` (4 MiB), comparison `>=` (Python
  `blob_utils.py:43,459`; modal-rs `blob_transfer.rs:8`, `mount.rs:335`).
- `size < 4 MiB` → INLINE (`MountPutFile.data`). `size >= 4 MiB` → BLOB
  (`BlobCreate` → HTTP PUT → `MountPutFile.data_blob_id`).
- `sha256_hex(&[u8]) -> String` (lowercase hex via `Sha256::digest`; `sha2` already a
  dep). For `add`, every file is tiny → inline; blob path is correct but rarely hit.

### 2.5 Exact request fields + upload sequence
Proto verified at `crates/modal-rust-sdk/proto/api.proto`:
- `MountFile {1 filename; 3 sha256_hex; 4 optional uint64 size; 5 optional uint32 mode}`
  (`:2589`). Set all of filename/sha256_hex/size/mode (size comment says "ignored in
  MountBuild()", but Python+modal-rs both send it — match the proven shape).
- `MountGetOrCreateRequest {1 deployment_name; 2 namespace; 3 environment_name;
  4 object_creation_type; 5 repeated MountFile files; 6 app_id}` (`:2596`).
- `MountGetOrCreateResponse {1 mount_id; 2 handle_metadata}` (`:2605`).
- `MountPutFileRequest {2 sha256_hex; oneof data_oneof {3 bytes data; 5 string
  data_blob_id}}` (`:2614`).
- `MountPutFileResponse {2 bool exists}` (`:2623`).
- `ObjectCreationType`: `EPHEMERAL = 5`, `ANONYMOUS_OWNED_BY_APP = 4`,
  `UNSPECIFIED = 0` (`:207`).
- RPC stubs (already generated on `inner_mut()`): `mount_get_or_create`,
  `mount_put_file`, `blob_create`, `blob_get`.

**Step A — per-file existence-check + upload (dedup by sha256).** Track a
`HashSet<String>` of accounted sha256 (local in-run dedup; identical files upload once).
For each surviving file `(mount_filename, sha256_hex, size, mode, data)`:
1. If sha256 already accounted, skip RPCs; just record the `MountFile`.
2. **Existence probe**: `mount_put_file(MountPutFileRequest{ sha256_hex, data_oneof:
   None })`. If `resp.exists` → server already has it (cross-run/user dedup); record the
   `MountFile`, continue.
3. **Upload** (only if not existing):
   - inline (`< 4 MiB`): `MountPutFileRequest{ sha256_hex, data_oneof:
     Some(Data(bytes)) }`.
   - blob (`>= 4 MiB`): `blob_id = blob_create_and_put(data)` (§2.6); then
     `MountPutFileRequest{ sha256_hex, data_oneof: Some(DataBlobId(blob_id)) }`.
4. **Re-probe completion loop**: after the upload `MountPutFile`, the server may still
   return `exists=false` transiently. Loop re-issuing `MountPutFile{sha256_hex}`
   (probe shape) until `exists`, with a 10-minute deadline (`MOUNT_PUT_FILE_CLIENT_
   TIMEOUT`; modal-rs `mount.rs:350-366`). This is the proven completion gate — port it.
5. Record `MountFile{ filename: mount_filename, sha256_hex, size: Some(size), mode }`.

**Step B — finalize the mount (one `MountGetOrCreate`).** Build the mount with the
assembled files; use EPHEMERAL (no app_id, no deploy name — the run path):
```rust
MountGetOrCreateRequest {
    deployment_name: String::new(),
    namespace: DeploymentNamespace::Workspace as i32,
    environment_name,                                    // env_or_default
    object_creation_type: ObjectCreationType::Ephemeral as i32,
    files: mount_files,                                  // sorted by filename
    app_id: String::new(),
}
```
- Sort `mount_files` by `filename` for determinism; reject duplicate filenames.
- Read `resp.mount_id`; error (like `ops/mount.rs:64`) if empty. Return it.
- **Fallback (leave a TODO, do not implement first):** if the server rejects EPHEMERAL
  for use in `Function.mount_ids`, switch to `ANONYMOUS_OWNED_BY_APP` (=4) with
  `app_id` set from the facade's `app_get_or_create` result. EPHEMERAL is the first
  attempt (matches modal-rs's proven `build()`).

### 2.6 Blob path: `BlobCreate` → HTTP PUT
`BlobCreateRequest {1 content_md5; 2 content_sha256_base64; 3 int64 content_length}`
(`:815`). `BlobCreateResponse {2 blob_id; oneof upload_type_oneof {1 string upload_url;
3 MultiPartUpload multipart}; 4 repeated blob_ids; oneof upload_types_oneof {...}}`
(`:823`). Port modal-rs `upload_blob` (`blob_transfer.rs:20-66`):
1. `sha256_b64 = base64::STANDARD.encode(Sha256::digest(data))` (base64 already a dep).
2. `blob_create(BlobCreateRequest{ content_md5: String::new(),
   content_sha256_base64: sha256_b64, content_length: data.len() as i64 })`. md5 empty
   (single-part S3 md5 optional for our sizes).
3. Read the **singular** `upload_type_oneof`:
   - `Some(UploadUrl(url))` → reqwest `PUT url`, header `Content-Type:
     application/octet-stream`, body = bytes; require 2xx.
   - `Some(Multipart(_))` → `Err(Error::invalid("multipart blob uploads not yet
     supported"))` (files > ~1 GiB only; `add` never hits this).
4. Return `resp.blob_id`.
(modal-rs reads the singular oneof and it works live — match it; ignore the plural
`upload_types_oneof`.)

### 2.7 New SDK deps (keep CI green)
Add to `crates/modal-rust-sdk/Cargo.toml [dependencies]`:
- `reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"] }`
  — **no default features** (drops native-tls/OpenSSL system dep; pure-rustls keeps the
  no-CUDA CI box green). HTTP PUT for blobs.
- `walkdir = "2"` — recursive walk with early prune.
- `sha2` (already present), `base64` (already present) — reuse.

Any live mount-upload test goes behind `#[ignore]` + the existing `live` feature
(`Cargo.toml [features] live`).

---

## 3. FILE-mode run wrapper + run image

### 3.1 The wrapper module (baked via `with_wrapper_module`)
- **Module name (fixed):** `modal_rust_run_wrapper` → baked to
  `/root/modal_rust_run_wrapper.py` (`image.rs:83-91,134-140`).
- **Callable (fixed):** `handler`.
- **Signature (DECISION — two positional args):** `def handler(entrypoint,
  input_json):`. This is load-bearing and matches `dev_app.py:165
  run_entrypoint(entrypoint, input_json)` byte-for-byte, and lets ONE created Modal
  function serve EVERY entrypoint in the crate (entrypoint is per-CALL data, not image
  identity). **Resolves the contradiction** between the upload/wrapper notes (which
  sketched one-arg `handler(payload)`) and the wiring note (two-arg) in favor of the
  proven `dev_app.py` recipe: two args, entrypoint NOT baked.
- **Invoke contract:** the facade calls `invoke_cbor(&id, &(entrypoint, input_json),
  &empty_kwargs)` with `R = String`. The container does `handler(entrypoint,
  input_json)`. Both args arrive already CBOR→str-decoded by the Modal runtime.
- **`input_json`** is the serialized `In` produced by the facade as
  `serde_json::to_string(&input)` — the wrapper writes it VERBATIM to `/tmp/in.json`
  (no re-`json.dumps`, since it is already a JSON string; this matches `dev_app.py`
  which writes `input_json` directly).
- **Return (DECISION):** RETURN the runner's one-line stdout envelope as a **Python
  string** (NOT parsed). Modal CBOR-encodes the str on the wire; the facade decodes a
  `String` and parses the envelope (§4). Returning the raw string keeps the envelope
  byte-exact and lets the facade reuse exact `run_cli` semantics
  (`runtime/src/lib.rs:137-147` failure shape, `:570` success shape).

### 3.2 Per-function baked constant
Exactly ONE constant is substituted per function at bake time (the entrypoint is a
runtime arg, not baked):
```python
PACKAGE    = "{{PACKAGE}}"   # e.g. "example-add" — cargo -p <pkg> (disambiguates the
                             # shared modal_runner bin across workspace members)
REMOTE_SRC = "/src"          # where the source mount lands (fixed)
```
Substitute via `str::replace` on a validated crate-name-shaped value (no shell quoting
needed — source is base64-baked at `image.rs:137`).

### 3.3 Build/exec body (ported from `dev_app.py:164-223`) + build-once guard
All build logs → stderr; the function's return value = the single envelope string.
1. `env = dict(os.environ)`; set `CARGO_HOME=/tmp/cargo`, `CARGO_TARGET_DIR=/tmp/target`,
   `RUST_BACKTRACE=1`.
2. **Build-once guard** (warm-container optimization, not a correctness requirement —
   `cargo build` is incremental anyway): module-global `_BUILT=False` + marker file
   `/tmp/.modal_rust_built`. If either set → skip cargo. Else build, then touch marker
   and set `_BUILT=True`.
3. **Build location** (`dev_app.py:176-188`): if `os.access(REMOTE_SRC, os.W_OK)` →
   build in `/src`; else `build_dir=/tmp/build`, rmtree if present, `cp -a /src
   /tmp/build`. Log the branch to stderr.
4. **Build (the cargo-in-function-body proof):** `subprocess.run(["cargo","build",
   "--release","-p",PACKAGE,"--bin","modal_runner"], cwd=build_dir, env=env,
   stdout=sys.stderr, stderr=sys.stderr)`. If `returncode != 0` → `raise
   RuntimeError(...)` (this becomes a Modal function failure → `sdk::Error::Build` with
   the remote traceback → facade `Error::Sdk`; it is NOT a runner envelope).
5. **Write input:** `open("/tmp/in.json","w").write(input_json)` (verbatim — already a
   JSON string from the facade).
6. **Exec runner:** `subprocess.run(["/tmp/target/release/modal_runner","--entrypoint",
   entrypoint,"--input-file","/tmp/in.json"], capture_output=True, text=True, env=env)`.
   Runner flags confirmed at `runtime/src/lib.rs` (`--entrypoint`/`--input-file`).
7. **Diagnostics:** echo `proc.stderr` to `sys.stderr`; print `[run] modal_runner
   exit=<rc>`.
8. **Return** `proc.stdout.strip()`. Do NOT raise on `rc == 1`: a runner exit-1 is a
   STRUCTURED `{"ok":false,...}` envelope the facade must map to a typed Error (§4).
9. **Empty-stdout safety:** if stdout is empty (runner crashed/OOM before emitting),
   `raise RuntimeError("modal_runner produced no envelope; exit=...; stderr tail: ...")`
   so the facade sees a clear infra error, not silently-decoded empty bytes.

### 3.4 Full wrapper source (the spec target, ~55 LOC)
```python
"""modal-rust FILE-mode run wrapper (ports dev_app.py run_entrypoint).

Baked to /root/modal_rust_run_wrapper.py. Modal FILE-mode resolves
import_module("modal_rust_run_wrapper") + getattr(mod,"handler"), then calls
handler(*args, **kwargs). The facade invokes with args=(entrypoint, input_json),
kwargs={}, so handler receives TWO positional args.

handler builds the mounted Rust crate IN THE FUNCTION BODY (run boundary: cargo at
execution time, never at image-build time), execs the frozen modal_runner, and
RETURNS the one-line JSON envelope string verbatim; the facade parses it.
"""
import json, os, shutil, subprocess, sys

PACKAGE    = "{{PACKAGE}}"      # injected: cargo -p <pkg>
REMOTE_SRC = "/src"            # source mount path
_RUNNER    = "/tmp/target/release/modal_runner"
_MARKER    = "/tmp/.modal_rust_built"
_BUILT     = False

def _env():
    e = dict(os.environ)
    e["CARGO_HOME"] = "/tmp/cargo"
    e["CARGO_TARGET_DIR"] = "/tmp/target"
    e["RUST_BACKTRACE"] = "1"
    return e

def _build(env):
    global _BUILT
    if _BUILT or os.path.exists(_MARKER):
        _BUILT = True
        print("[run] build cached (warm container); skipping cargo build", file=sys.stderr)
        return
    if os.access(REMOTE_SRC, os.W_OK):
        build_dir = REMOTE_SRC
        print(f"[run] mount {REMOTE_SRC} writable; building in place", file=sys.stderr)
    else:
        build_dir = "/tmp/build"
        print(f"[run] mount {REMOTE_SRC} read-only; cp -a -> {build_dir}", file=sys.stderr)
        if os.path.exists(build_dir):
            shutil.rmtree(build_dir)
        subprocess.run(["cp", "-a", REMOTE_SRC, build_dir], check=True)
    b = subprocess.run(
        ["cargo", "build", "--release", "-p", PACKAGE, "--bin", "modal_runner"],
        cwd=build_dir, env=env, stdout=sys.stderr, stderr=sys.stderr,
    )
    if b.returncode != 0:
        raise RuntimeError(f"cargo build failed with exit code {b.returncode}")
    open(_MARKER, "w").close()
    _BUILT = True

def handler(entrypoint, input_json):
    env = _env()
    _build(env)
    with open("/tmp/in.json", "w") as f:
        f.write(input_json)
    proc = subprocess.run(
        [_RUNNER, "--entrypoint", entrypoint, "--input-file", "/tmp/in.json"],
        capture_output=True, text=True, env=env,
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
```
The `{{PACKAGE}}` placeholder is substituted (plain `str::replace`) before
`with_wrapper_module`. Store this as a `const WRAPPER_SRC: &str` + a fn
`run_wrapper_src(package: &str) -> String` in the facade (`crates/modal-rust/src/
remote.rs`) — one wrapper for the whole run path, never per-project.

### 3.5 The run IMAGE
- **Base:** `rust:{RUST_VER}-slim` (`RUST_VER="1"`) →
  `ImageSpec::from_registry(format!("rust:{ver}-slim"))`. Carries cargo/rustc.
- **Python provisioning — DECISION: `apt-get install python3 python3-pip`, NOT the
  hosted `add_python` mount.** Rationale: `rust:1-slim` has NO python; FILE-mode
  containers boot `python -m modal._container_entrypoint`, AND the wrapper bake step
  itself runs `python3 -c ...` (`image.rs:137`) at image-build time, so python3 must
  exist BEFORE the bake. The `add_python` python-standalone mount would need attaching
  a second build-time mount — strictly more SDK surface. One apt line is the simplest
  self-contained choice that works.
  - **Ordering constraint (load-bearing):** the apt line MUST render BEFORE the wrapper
    bake. Current render order is `FROM → pip → bakes → extra_commands`
    (`image.rs:109-119`), so apt CANNOT go in `extra_commands`. → minimal additive
    `image.rs` change (§3.6).
- **`with_pip_install_modal()` — REQUIRED on this bare base.** The client mount supplies
  only the modal *source* (`/pkg`), not its pip deps (`typing_extensions`, `grpclib`,
  `protobuf`, `aiohttp`, `cbor2`, …) — verified live finding (`image.rs:11-29`,
  knowledge.md). Without them the container crash-loops on
  `modal._container_entrypoint`. The mounted `/pkg` still wins on PYTHONPATH.
  - **CAVEAT:** the current pip line is bare `RUN pip install --no-cache-dir modal`
    (`image.rs:111-112`); the slim apt python may not expose a `pip` shim. → flip to
    `python3 -m pip` (§3.6), universal on both slim-python and `python:` bases.
- **`ENV RUST_BACKTRACE=1`** + **`ENTRYPOINT []`** (neutralize the rust base ENTRYPOINT
  so Modal's python runtime can start; `dev_app.py:60-61`). Both are pure metadata,
  order-insensitive → emit via `with_command` (extra_commands). (The SDK's existing
  `python:` base boots without `ENTRYPOINT []`; the rust base sets one, so include it.)
- **Source is NOT baked.** Image = rust base + python3/pip + modal pip deps + wrapper +
  ENV/ENTRYPOINT. The user's Rust source arrives at runtime as the uploaded source
  mount (§2) at `/src`. No `cargo` in `dockerfile_commands` → boundary held.

Resulting rendered `dockerfile_commands`:
```
FROM rust:1-slim
RUN apt-get update && apt-get install -y --no-install-recommends python3 python3-pip && rm -rf /var/lib/apt/lists/*
RUN python3 -m pip install --no-cache-dir modal           # add --break-system-packages if live build needs it
RUN python3 -c "import base64,pathlib; pathlib.Path('/root/modal_rust_run_wrapper.py').write_bytes(base64.b64decode('<b64>'))"
ENV RUST_BACKTRACE=1
ENTRYPOINT []
```

### 3.6 Minimal additive `image.rs` changes (all backward-compatible)
1. Add field `pre_bake_commands: Vec<String>` (default empty) + a typed builder
   `with_apt(packages: &[&str]) -> Self` that renders the canonical
   `RUN apt-get update && apt-get install -y --no-install-recommends <pkgs> && rm -rf
   /var/lib/apt/lists/*` line into `pre_bake_commands` (avoids quoting mistakes;
   preferred over a raw `with_pre_command`). In `dockerfile_commands()`, render order
   becomes `FROM → pre_bake_commands → optional pip → bakes → extra_commands`.
2. Change the emitted pip line from `RUN pip install --no-cache-dir modal` to
   `RUN python3 -m pip install --no-cache-dir modal` (universal launcher; existing
   `python:3.12-slim` path still works). Update the one assertion in the
   `pip_fallback_*` test to match. Add `--break-system-packages` only if the live build
   proves the env is externally-managed (Debian bookworm) — drop otherwise.
3. ENV/ENTRYPOINT reuse `with_command` (no new field).

These are additive (new field defaults empty; existing image tests pass unchanged
except the one pip-string assertion). Files stay ~300-500 LOC.

### 3.7 Facade assembly of the run image
```rust
ImageSpec::from_registry(format!("rust:{RUST_VER}-slim"))
    .with_apt(&["python3", "python3-pip"])          // NEW pre-bake slot — before the bake
    .with_pip_install_modal()                        // python3 -m pip install modal (client dep closure)
    .with_wrapper_module("modal_rust_run_wrapper", run_wrapper_src(&package))
    .with_command("ENV RUST_BACKTRACE=1")
    .with_command("ENTRYPOINT []")
```
Then `image_get_or_create(&app_id, &spec) → image_id`. `cargo` must NOT appear in this
build log (boundary).

---

## 4. `Function::remote` wiring (facade)

### 4.1 End-to-end sequence (first `.remote()`; mirrors `live_create_invoke.rs:135-246`)
1. **Require a connected App.** `.remote()` needs the `RemoteHandle` from
   `App::connect`. If absent → `Error::NotConnected` (§4.4).
2. **Client mount:** `client.client_mount_id(None)` (`ops/mount.rs:37`).
3. **Source mount (NEW):** `client.mount_local_dir(local_root, "/src",
   &["target",".git",".modal-rust","**/*.rlib"], None)` → `source_mount_id` (§2).
   `local_root`/`package` from §4.5.
4. **Run image:** build per §3.7 → `image_get_or_create(&app_id, &spec)` → `image_id`.
5. **Precreate:** `function_precreate(&app_id, "handler")` → `precreate_id`
   (`ops/function.rs:132`). The Modal function name is the wrapper callable `"handler"`
   (one function serves all entrypoints; the user entrypoint is an INVOKE arg).
6. **FunctionCreate (FILE):**
   `FunctionSpec::new("modal_rust_run_wrapper", "handler", &image_id)
   .with_mount_ids(vec![client_mount_id, source_mount_id])
   .with_timeout_secs(1800)` → `function_create(&app_id, &precreate_id, &spec)`
   (`ops/function.rs:165`; sends FILE mode, empty serialized, resources, `[PICKLE,CBOR]`;
   both mounts attach via `Function.mount_ids`). 1800s matches `dev_app.py:164` (the
   in-body build needs the long timeout, vs the SDK default 300s).
7. **AppPublish:** build `function_ids{"handler"->function_id}` +
   `definition_ids{function_id->definition_id}` (when non-empty); `app_publish(&app_id,
   &app_name, ...)` (`ops/app.rs`). Keep publish (so `from_name` resolves) — matches the
   live test.
8. **from_name:** `function_from_name(&app_name, "handler", None)` → `invoke_function_id`
   (`ops/function.rs:226`).
9. **Invoke (§4.2).**
10. **Parse envelope (§4.3).**

### 4.2 Wrapper arg shape (CBOR) — DECISION
Two positional args (matches §3.1 and `dev_app.py:165`):
```rust
let input_json = serde_json::to_string(&input).map_err(Error::Encode)?;
let empty_kwargs: std::collections::HashMap<String, ()> = HashMap::new();
let envelope: String = client
    .invoke_cbor::<_, _, String>(&invoke_function_id, &(entrypoint, input_json), &empty_kwargs)
    .await?;                       // invoke_cbor sends ((args),(kwargs)) (invoke.rs:82)
```
`R = String` because the wrapper returns the runner stdout envelope string. (Rejected
alternative: one arg with entrypoint baked → one function per entrypoint → more creates
+ cache keys.)

### 4.3 Envelope → `Result<Out, Error>` (SAME semantics as `.local()`)
After `envelope: String`, parse to mirror `.local()` (`function.rs:48-49`) exactly:
```rust
let v: serde_json::Value = serde_json::from_str(&envelope).map_err(Error::Decode)?;
if v["ok"] == serde_json::Value::Bool(true) {
    serde_json::from_value::<Out>(v["value"].clone()).map_err(Error::Decode)
} else {
    Err(Error::Runner(reconstruct_runner_error(&v["error"])))
}
```
`reconstruct_runner_error` maps `error.kind` → the FROZEN five-kind `RunnerError`
(`runtime/src/lib.rs:39-65`; kinds/shape confirmed: `to_envelope` emits
`{kind,message,details,backtrace}` at `:137-147`):
- `"decode_error"` → `RunnerError::Decode(message)`
- `"unknown_entrypoint"` → `RunnerError::UnknownEntrypoint(message)`
- `"function_error"` → `RunnerError::Function{ message, details: (Null → None) }`
- `"encode_error"` → `RunnerError::Encode(message)`
- `"panic"` → `RunnerError::Panic{ message, backtrace }`
- unknown kind → `RunnerError::Decode(format!("unrecognized error kind: {kind}"))`
Result: `.remote()` returns the SAME `Result<Out,Error>` as `.local()` for the same
input — `Err(Error::Runner(re))` exactly as `function.rs:49`. `Error::Runner` already
exists (`error.rs:27`); NO new envelope-path variant needed.

### 4.4 Boundary errors
- Input encode failure → `Error::Encode` (mirrors `function.rs:48`).
- Control-plane failures (connect/upload/image/create/publish/from_name/invoke) →
  `sdk::Error` → `Error::Sdk` via the existing `From` (`error.rs`). A remote `cargo
  build` failure surfaces here too (the wrapper raises → Modal function failure →
  `sdk::Error::Build` with traceback → `Error::Sdk`).
- Not-connected → NEW `Error::NotConnected(String)` (clear message: "call
  App::connect before .remote()"). Add the variant to `error.rs` (Display +
  `source()→None`). Keep `Error::NotImplemented` for `spawn`/`map`/`FunctionCall::get`
  (still stubbed). `Error::not_implemented` stays used by those.

### 4.5 Local crate root + PACKAGE (config, v0-minimal)
- **`local_root`** (dir to upload): default = the cargo workspace root discovered by
  walking up from CWD to the nearest `Cargo.toml` containing `[workspace]` (else nearest
  `Cargo.toml`). For `examples/add` this is the repo root (`example-add` is a workspace
  member and the build needs the workspace), matching `dev_app.py:43`. Override via env
  `MODAL_RUST_SOURCE_DIR`.
- **`PACKAGE`** (`-p`): the cargo package owning the entrypoint. v0: env
  `MODAL_RUST_PACKAGE`, else `"example-add"`. Load-bearing — the `modal_runner` bin
  name is shared across members. Derivation from the registry is a later milestone.
- Hold these on a small `RemoteConfig { local_root, package, remote_src: "/src",
  ignore, base_image: "rust:1-slim", rust_ver: "1", timeout_secs: 1800 }` with
  `Default`, stored on `App`. One struct, all knobs, no per-project file.

### 4.6 Caching within a process (do NOT recreate per call)
- The resolved invoke `function_id` is stable per App (one wrapper serves all
  entrypoints), so key the memo on the App. Add `function_id:
  tokio::sync::OnceCell<String>` to `RemoteHandle`; `get_or_try_init(async { steps
  2-8 })` gives correct single-flight under concurrent `.remote()`. Subsequent calls
  skip to step 9.
- **Interior mutability:** `App::function` returns `Function<'_>` borrowing `&App`
  (`app.rs:78`), but `invoke_cbor` needs `&mut ModalClient`. Change `RemoteHandle.client`
  from owned `ModalClient` to `tokio::sync::Mutex<ModalClient>`; `.remote()` locks it for
  ensure+invoke. (One structural change; everything else additive.)
- Add `app_name: String` to `RemoteHandle` (needed for `app_publish`/`from_name`;
  currently only `app_id` is stored — `connect_with_registry` has `name` in scope at
  `app.rs:64`). Also add `config: RemoteConfig`.

---

## 5. Files to change

SDK:
- NEW `crates/modal-rust-sdk/src/ops/local_dir.rs` (~250-350 LOC): `mount_local_dir`,
  walk+ignore matcher, per-file MountPutFile sequence + dedup + 10-min loop, EPHEMERAL
  finalize, `sha256_hex`.
- NEW `crates/modal-rust-sdk/src/ops/blob.rs` (~80-120 LOC): `blob_create_and_put`
  (`BlobCreate` → reqwest PUT).
- `crates/modal-rust-sdk/src/ops/mod.rs`: `pub mod local_dir; pub mod blob;`.
- `crates/modal-rust-sdk/src/ops/image.rs`: add `pre_bake_commands` + `with_apt`,
  reorder `dockerfile_commands()`, flip pip line to `python3 -m pip`, fix the one pip
  test assertion (§3.6).
- `crates/modal-rust-sdk/Cargo.toml`: add `reqwest` (rustls, no default) + `walkdir`
  (§2.7).
- Live: NEW `crates/modal-rust-sdk/tests/live_mount_upload.rs` behind `#[ignore]` +
  `live` (best-effort).

Facade:
- `crates/modal-rust/src/function.rs:59-66`: replace `remote` body → encode input →
  `self.app.remote_invoke(&self.name, input_json).await` → parse envelope (§4.3). Keep
  `spawn`/`map`/`FunctionCall::get` stubbed.
- `crates/modal-rust/src/app.rs`: `RemoteHandle { client: Mutex<ModalClient>, app_id,
  app_name, function_id: OnceCell<String>, config: RemoteConfig }`; populate `app_name`
  in `connect_with_registry`; add `remote_invoke(&self, entrypoint, input_json) ->
  Result<String>` (OnceCell ensure + locked invoke); `not_connected()` guard.
- NEW `crates/modal-rust/src/remote.rs` (~150-250 LOC): `RemoteConfig`,
  `WRAPPER_SRC`/`run_wrapper_src(package)`, `ensure_function(client,app_id,app_name,
  &config) -> Result<String>` (steps 2-8), `parse_envelope::<Out>(s) -> Result<Out>` +
  `reconstruct_runner_error`, `discover_local_root`/`package`.
- `crates/modal-rust/src/error.rs`: add `Error::NotConnected(String)`.
- `crates/modal-rust/Cargo.toml`: add a `live` feature; `tokio` with the
  `sync`/`rt`/`macros` features for `Mutex`/`OnceCell`.
- Tests: move the not-implemented assertion in `tests/local.rs` to `spawn`/`map` only
  (remote now needs a live App); NEW `crates/modal-rust/tests/live_remote.rs` behind
  `#[cfg(feature="live")] #[ignore]` asserting `add(AddInput{a:40,b:2}) ==
  AddOutput{sum:42}`, with the `retry!`/`is_transient` pattern from
  `live_create_invoke.rs`.

---

## 6. Verification

**Hard gates (WORKING.md, on default-members — NOT --workspace/--all-features):**
`cargo fmt --check` ; `cargo clippy --all-targets -- -D warnings` ; `cargo build` ;
`cargo test`. New deps (`reqwest` rustls-no-default, `walkdir`) must keep the no-CUDA CI
green.

**Offline unit tests (pure, no network):**
- `reconstruct_runner_error` + envelope parse: feed canned envelopes, assert each of the
  5 kinds maps IDENTICALLY to `.local()`'s `Error::Runner(...)`, plus the `ok:true`
  success path and the unknown-kind fallback.
- `run_wrapper_src(package)` substitution: `{{PACKAGE}}` replaced, valid Python.
- `mount_local_dir` ignore matcher: prunes `target`/`.git`/`.modal-rust`, drops
  `*.rlib`, keeps `.rs`/`.toml`; remote mapping yields `/src/...` POSIX paths.
- `ImageSpec` render order: apt BEFORE bake; pip line is `python3 -m pip`.

**Live proof (best-effort, retried; behind `live` + `#[ignore]`):** `live_remote.rs`
decodes `AddOutput{sum:42}` from the REAL Rust `add` (`examples/add/src/lib.rs:47`, NOT
an echo), and the FUNCTION/runtime logs (wrapper stderr) show `cargo build` lines —
proving cargo ran in the function body, not the image build (the boundary). Modal
flakiness is TRANSIENT — RETRY, never block; the hard gates are the offline compiles.

## 7. Resolved contradictions
- **Wrapper signature:** TWO positional args `handler(entrypoint, input_json)` (proven
  `dev_app.py` recipe + wiring note), superseding the one-arg `handler(payload)` sketch
  in the upload/wrapper notes. One Modal function serves all entrypoints; entrypoint is
  per-call, not baked.
- **`input_json` handling:** facade does `serde_json::to_string(&input)`; the wrapper
  writes it VERBATIM (no `json.dumps` in Python), matching `dev_app.py` (input arrives
  pre-stringified).
- **Mount creation type:** EPHEMERAL first (no app_id), `ANONYMOUS_OWNED_BY_APP`
  fallback documented as a TODO only.
- **Python provisioning:** `apt-get python3 python3-pip` (self-contained), NOT
  `add_python`; apt MUST precede the bake → the one additive `image.rs` slot.
- **pip launcher:** `python3 -m pip` (universal), replacing bare `pip`.
