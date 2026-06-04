# P6 — Cargo Build Cache (build-ready spec)

Single merged spec for P6: the cargo build cache for the **run path**. Merges the
SDK volume note and the wrapper/wiring note into one implementable plan.

All file:line refs are from the repo at `/Users/nicolas/devel/modal-rust`.
Proto idents below were verified against the generated `modal.client.rs`
(`tonic::include_proto!` target) — see §7.

---

## 0. Design anchor (authoritative — do NOT deviate)

`workpads/shim-backend/knowledge.md` §C ("archive-as-single-object on a V2
Volume"). Modal volumes degrade past ~50k files (latency scales with file count),
so CARGO_HOME/target NEVER sit directly on the mount — that is what the prototype
`dev_app.py` M6 (`run_entrypoint_cached`) does, and it is **REJECTED** for P6.

Chosen mechanism:
1. Build on FAST LOCAL DISK: `CARGO_HOME=/tmp/cargo`, `CARGO_TARGET_DIR=/tmp/target`
   (already set, `remote.rs:69-70`) — NEVER on the mounted volume.
2. Persist the cache as ONE compressed archive `cache.tar.zst` on a **V2** Volume
   mounted at a stable path `/cache`. On container START: if the archive exists,
   unpack to `/tmp` (WARM); else COLD. After the first cold build: repack the
   changed dirs into the single archive (atomic temp+rename) and rely on
   `allow_background_commits=true` — NO `vol.reload()`/`vol.commit()` on the hot
   path (cargo holds locks).
3. Scope: `CARGO_HOME` (registry index + downloaded `.crate` tarballs + git db —
   high value, mostly-read) + OPTIONALLY `target/` (env-gated, default OFF in v0).
4. V2 volume (concurrent writes): `VolumeGetOrCreate("modal-rust-cargo-cache", v2)`
   → `Function.volume_mounts[{volume_id, mount_path:"/cache",
   allow_background_commits:true}]`.
5. DEFAULT ON; opt-out via `#[modal_rust::function(cache=false)]` (decorator), env
   `MODAL_RUST_NO_CACHE`, and the `RemoteConfig.cache` knob.

This is a **RUN-path optimization** (build-in-body). DEPLOY builds once at
image-build time (Modal caches the image), so the cargo cache is a no-op there and
must NOT attach a volume.

**Correctness rule (FROZEN, mechanically enforced):** a cache miss / corrupt
archive / failed pack ONLY costs time, NEVER changes the result. Build output is
determined by `/src` + `Cargo.lock`, never by cache presence.

**FROZEN invariants left untouched:** runner protocol / HandlerFn / `typed!` /
dispatch; FILE-mode wrapper shape + the one-JSON-envelope-on-stdout invariant;
run-vs-deploy build boundary; `retry_transient` on all RPCs; ephemeral-run vs
persistent-deploy; add_python / CUDA image paths; cargo-scoped upload; spawn/map.
README.md is NOT touched. We ADD a volume to the SDK + facade and ADD an
unpack/repack step inside the existing wrapper — no rewrite of create/invoke.

---

## 1. SDK — Volume op (`VolumeGetOrCreate`)

### 1.1 New file `crates/modal-rust-sdk/src/ops/volume.rs`

Mirror `ops/mount.rs::global_mount_id` (`mount.rs:129-154`): build request →
`let stub = self.stub();` → `retry_unary("volume_get_or_create", || { clone stub;
clone req; async move { Ok(stub.volume_get_or_create(req).await?.into_inner()) }
})` → guard empty id.

```rust
//! Volume resolution — `VolumeGetOrCreate` (api.proto:3880, rpc :4373).
//!
//! Used by the P6 cargo build cache: resolve a persistent V2 volume by name
//! (create-if-missing) and return its `volume_id`, for attaching to a function
//! via `FunctionSpec.volume_mounts` -> `Function.volume_mounts` (field 33).
//!
//! Mirrors `Volume.from_name(..., create_if_missing, version)` (volume.py:585-630):
//! sets ONLY deployment_name, environment_name, object_creation_type, version.
//! Leaves `namespace` (reserved 2) and `app_id` (field 5) UNSET.

use crate::client::ModalClient;
use crate::error::{Error, Result};
use crate::proto::api::{ObjectCreationType, VolumeFsVersion, VolumeGetOrCreateRequest};
use crate::retry::retry_unary;

impl ModalClient {
    /// Resolve a persistent Volume by deployment name, creating it if missing,
    /// and return its `volume_id`.
    ///
    /// - `name`: deployment name (e.g. `"modal-rust-cargo-cache"`).
    /// - `v2`: `true` => `VolumeFsVersion::V2` (concurrent writes — required for
    ///   the cargo cache); `false` => `VolumeFsVersion::Unspecified` (server
    ///   default == V1; matches Python `version=None`).
    /// - `create_if_missing`: `true` => `ObjectCreationType::CreateIfMissing`
    ///   (idempotent, retry-safe); `false` => `Unspecified` (pure lookup).
    /// - `environment`: defaults to the configured environment (or `"main"`).
    pub async fn volume_get_or_create(
        &mut self,
        name: &str,
        v2: bool,
        create_if_missing: bool,
        environment: Option<&str>,
    ) -> Result<String> {
        let environment_name = self.env_or_default(environment);
        let version = if v2 {
            VolumeFsVersion::V2 as i32
        } else {
            VolumeFsVersion::Unspecified as i32 // == Python version=None
        };
        let object_creation_type = if create_if_missing {
            ObjectCreationType::CreateIfMissing as i32
        } else {
            ObjectCreationType::Unspecified as i32 // pure lookup
        };
        let req = VolumeGetOrCreateRequest {
            deployment_name: name.to_string(),
            environment_name,
            object_creation_type,
            version,
            ..Default::default() // app_id empty; reserved-2 namespace never set
        };
        let stub = self.stub();
        // CREATE_IF_MISSING is idempotent server-side, so a retry after a dropped
        // response re-resolves the same volume_id (mirrors mount_get_or_create).
        let resp = retry_unary("volume_get_or_create", || {
            let mut stub = stub.clone();
            let req = req.clone();
            async move { Ok(stub.volume_get_or_create(req).await?.into_inner()) }
        })
        .await?;

        if resp.volume_id.is_empty() {
            return Err(Error::build(format!(
                "VolumeGetOrCreate for '{name}' returned an empty volume_id"
            )));
        }
        Ok(resp.volume_id)
    }
}

#[cfg(test)]
mod tests {
    use crate::proto::api::{ObjectCreationType, VolumeFsVersion};

    #[test]
    fn version_flag_maps_to_fs_version() {
        assert_eq!(VolumeFsVersion::V2 as i32, 2);
        assert_eq!(VolumeFsVersion::Unspecified as i32, 0);
    }

    #[test]
    fn create_flag_maps_to_creation_type() {
        assert_eq!(ObjectCreationType::CreateIfMissing as i32, 1);
        assert_eq!(ObjectCreationType::Unspecified as i32, 0);
    }
}
```

> **Resolved contradiction (signature).** The wrapper note hard-coded
> V2/create-if-missing as a 2-arg method; the SDK note made `v2` /
> `create_if_missing` explicit params. We keep the **explicit-flag signature**
> (`name, v2, create_if_missing, environment`) — more honest, unit-testable, and
> the run-path caller (§3.3) always passes `(CACHE_VOLUME_NAME, true, true, None)`.

### 1.2 Register the module — `crates/modal-rust-sdk/src/ops/mod.rs`

Add to the `pub mod` list (alphabetized; after `mount`):
```rust
pub mod volume;
```
And a bullet to the module doc list (after the `mount` bullet):
```
//! - [`volume`] — `VolumeGetOrCreate` (create-if-missing, V2) → `volume_id` (P6 cargo cache).
```

---

## 2. SDK — `FunctionSpec.volume_mounts` (additive, default empty)

In `crates/modal-rust-sdk/src/ops/function.rs`:

### 2.1 Import `VolumeMount` (extend the `proto::api` use block, `function.rs:19`)
```rust
use crate::proto::api::{
    DataFormat, Function, FunctionCreateRequest, FunctionGetRequest, FunctionPrecreateRequest,
    GpuConfig, Resources, VolumeMount,
};
```

### 2.2 Public SDK struct `FunctionVolumeMount` (above `FunctionSpec`)

A thin named struct so callers don't touch raw proto. Only the three fields P6
needs are exposed; `read_only`/`sub_path` default to `false`/`None` in `to_proto`
(correct for a writable cargo cache).

> **Resolved contradiction (shape).** The wrapper note used raw proto
> `Vec<VolumeMount>` + a 3-arg `with_volume_mount`; the SDK note introduced a named
> `FunctionVolumeMount`. We keep the **named struct** (cleaner public surface, no
> raw-proto leakage), and ALSO provide a 3-arg convenience `with_volume_mount`
> (§2.5) so the §3.3 call site reads naturally.

```rust
/// One persistent-volume attachment for a function. Maps to proto `VolumeMount`
/// (api.proto:3944) on `Function.volume_mounts` (field 33). Additive: a spec with
/// an empty `volume_mounts` is wire-identical to before P6.
#[derive(Debug, Clone)]
pub struct FunctionVolumeMount {
    /// Resolved volume id ([`ModalClient::volume_get_or_create`]).
    pub volume_id: String,
    /// In-container mount path (e.g. `"/cache"` for the cargo archive).
    pub mount_path: String,
    /// Enable automatic background commits (proto field 3). `true` for the cargo
    /// cache so the repacked archive is persisted without a hot-path `reload()`.
    pub allow_background_commits: bool,
}

impl FunctionVolumeMount {
    /// New mount with background commits ENABLED (the cargo-cache default).
    pub fn new(volume_id: impl Into<String>, mount_path: impl Into<String>) -> Self {
        Self {
            volume_id: volume_id.into(),
            mount_path: mount_path.into(),
            allow_background_commits: true,
        }
    }

    fn to_proto(&self) -> VolumeMount {
        VolumeMount {
            volume_id: self.volume_id.clone(),
            mount_path: self.mount_path.clone(),
            allow_background_commits: self.allow_background_commits,
            read_only: false, // cargo cache must be writable
            sub_path: None,   // field 5 unset
        }
    }
}
```

### 2.3 Field on `FunctionSpec` (after `mount_client_dependencies`, `function.rs:113`)
```rust
    /// Persistent-volume attachments → `Function.volume_mounts` (proto field 33).
    /// DEFAULT EMPTY: an unset list keeps the create wire-identical to pre-P6, so
    /// every existing function is unchanged. P6 pushes the cargo-cache volume here.
    pub volume_mounts: Vec<FunctionVolumeMount>,
```

### 2.4 Initialize in `FunctionSpec::new` (struct literal, `function.rs:119-133`)
```rust
            volume_mounts: Vec::new(),
```

### 2.5 Builder methods (after `with_mount_client_dependencies`, `function.rs:180`)
```rust
    /// Attach volume mounts (e.g. the cargo build cache). Replaces any existing list.
    pub fn with_volume_mounts(mut self, volume_mounts: Vec<FunctionVolumeMount>) -> Self {
        self.volume_mounts = volume_mounts;
        self
    }

    /// Append a single volume mount (background commits ENABLED). Convenience for
    /// the cargo-cache attach: `with_volume_mount(vid, "/cache")`.
    pub fn with_volume_mount(
        mut self,
        volume_id: impl Into<String>,
        mount_path: impl Into<String>,
    ) -> Self {
        self.volume_mounts
            .push(FunctionVolumeMount::new(volume_id, mount_path));
        self
    }
```

### 2.6 Wire into the `Function` literal in `function_create` (`function.rs:253-269`)

Add ONE line (the only behavioral wire change in the SDK):
```rust
            volume_mounts: spec.volume_mounts.iter().map(|m| m.to_proto()).collect(),
```
When `spec.volume_mounts` is empty this yields an empty `Vec`, which prost
serializes as the field being absent → **byte-identical to today** for all existing
callers (the additivity / FROZEN-invariant requirement). The rest of the `Function`
literal and the create/precreate/get logic are UNCHANGED.

### 2.7 Tests (in the existing `#[cfg(test)] mod tests`)
- `volume_mounts_default_empty`: `FunctionSpec::new(...).volume_mounts.is_empty()`.
- `with_volume_mount_appends_and_to_proto`: build via `with_volume_mount("vo-1",
  "/cache")`, assert through `to_proto()` that `allow_background_commits == true`,
  `read_only == false`, `sub_path.is_none()`, `mount_path == "/cache"`.
- Extend `spec_defaults_and_builders` (`function.rs:~360`) to also assert
  `volume_mounts` is empty by default.

---

## 3. Facade — wrapper cache + cache-on-by-default wiring

### 3.1 Constants (`crates/modal-rust/src/remote.rs`)
```rust
pub(crate) const CACHE_MOUNT: &str = "/cache";                 // stable V2 mount path
pub(crate) const CACHE_ARCHIVE_NAME: &str = "cache.tar.zst";   // single persisted object
pub(crate) const CACHE_VOLUME_NAME: &str = "modal-rust-cargo-cache"; // knowledge.md §C item 4
```
(In the wrapper the archive is the literal `/cache/cache.tar.zst`; `CARGO_HOME` /
`CARGO_TARGET_DIR` are already `/tmp/cargo` / `/tmp/target`, `remote.rs:69-70`.)

### 3.2 Wrapper cache logic — modify `WRAPPER_SRC` (`remote.rs:52-123`)

The wrapper is base64-baked verbatim (`image.rs` bake) — no shell quoting; only
placeholders are substituted. Add a SECOND placeholder `{{CACHE}}` (rendered as
the literal Python `True`/`False`) so the facade compiles cache on/off INTO the
wrapper (the wrapper has no other config channel).

**Scope of the archive:**
- ALWAYS `CARGO_HOME = /tmp/cargo` — registry index + downloaded `.crate` tarballs
  + git db. Scope to the read-mostly subtrees `cargo/registry/` and `cargo/git/`;
  exclude lock files (`cargo/registry/cache/.package-cache`, `cargo/.package-cache`)
  that regenerate.
- OPTIONALLY `CARGO_TARGET_DIR = /tmp/target` — gated by env
  `MODAL_RUST_CACHE_TARGET` (truthy), default **OFF** in v0. `target/` is the
  largest/most-churning tree; including it gives the biggest warm win but the
  biggest pack cost. Flip ON for the burn-add benchmark.
- NEVER archive `/src` (arrives fresh via the source mount each run).

**Tar + zstd (build relative to `/tmp` so paths round-trip):** prefer
`tar --zstd`; fall back to `-z` (gzip → `cache.tar.gz`) if the `zstd` binary is
absent on the base. Selection is by archive existence/extension so cold↔warm stays
consistent within a volume. Default zstd level (`-3`) — do NOT raise it (pack time
is on the critical path). PACK is atomic: write to a temp on the SAME fs, then
`mv -f` (rename within `/cache` is atomic → a reader never sees a half archive). NO
`vol.commit()`/`vol.reload()` — `allow_background_commits=true` flushes the renamed
object automatically.

**Lifecycle (a FILE-mode container handles MANY invokes; `_BUILT`/`_MARKER` make
only the first invoke per container compile, `remote.rs:63-64,88-101`):**
- UNPACK once per container, at the first `_build` entry (guarded by
  `_BUILT`/`_MARKER` exactly like the build), BEFORE cargo runs, so cargo sees a
  warm CARGO_HOME.
- PACK after the FIRST successful cold build only (cold→built transition). Skip on
  warm invokes / warm containers (nothing new). Packing in-band right after the
  cold build (not at process exit) is robust: Modal gives the FILE-mode wrapper no
  reliable atexit hook, and the cold build already dominates wall-clock so pack cost
  is amortized. Wrap pack in try/except → log to stderr, NEVER raise.

**Revised wrapper body (shape; keeps everything below `_build` byte-for-byte):**
```python
PACKAGE  = "{{PACKAGE}}"
CACHE_ON = {{CACHE}}                       # injected True / False
ARCHIVE  = "/cache/cache.tar.zst"

def _unpack_cache():
    if not CACHE_ON:
        return "disabled"
    if not os.path.exists(ARCHIVE):
        return "COLD (no archive)"
    try:
        subprocess.run(["tar", "--zstd", "-xf", ARCHIVE, "-C", "/tmp"], check=True,
                       stdout=sys.stderr, stderr=sys.stderr)
        return "WARM"
    except Exception as e:                  # corrupt/partial archive => treat as COLD
        print(f"[cache] unpack failed (treated as COLD): {e!r}", file=sys.stderr)
        return "COLD (unpack failed)"

def _pack_cache():
    if not CACHE_ON:
        return
    dirs = ["cargo"]
    if os.environ.get("MODAL_RUST_CACHE_TARGET", "").lower() in ("1", "true", "yes", "on"):
        dirs.append("target")
    tmp = ARCHIVE + ".tmp"
    try:
        subprocess.run(
            ["tar", "--zstd",
             "--exclude=cargo/registry/cache/.package-cache",
             "--exclude=cargo/.package-cache",
             "-cf", tmp, "-C", "/tmp", *dirs],
            check=True, stdout=sys.stderr, stderr=sys.stderr,
        )
        os.replace(tmp, ARCHIVE)            # atomic rename on same fs; no reload/commit
        print(f"[cache] packed {ARCHIVE}", file=sys.stderr)
    except Exception as e:                  # a failed pack must NOT fail the call
        print(f"[cache] pack failed (ignored): {e!r}", file=sys.stderr)

def _build(env):
    global _BUILT
    if _BUILT or os.path.exists(_MARKER):
        _BUILT = True
        print("[run] build cached (warm container); skipping cargo build", file=sys.stderr)
        return
    print(f"[cache] {_unpack_cache()}", file=sys.stderr)   # cold path: warm CARGO_HOME if archive present
    build_dir = _build_dir()
    b = subprocess.run(
        ["cargo", "build", "--release", "-p", PACKAGE, "--bin", "modal_runner"],
        cwd=build_dir, env=env, stdout=sys.stderr, stderr=sys.stderr,
    )
    if b.returncode != 0:
        raise RuntimeError(f"cargo build failed with exit code {b.returncode}")
    open(_MARKER, "w").close()
    _BUILT = True
    _pack_cache()                            # cold path only; persist enriched archive
```
`handler` and everything below `_build` (`/tmp/in.json`, exec `_RUNNER`, one-line
envelope on stdout) are UNCHANGED — the runner protocol and the
one-JSON-envelope-on-stdout invariant are preserved; ALL cache logs go to stderr.
(Add a graceful gzip fallback for the `zstd`-missing base: try `--zstd`, except →
retry with `-z` and a `.tar.gz` archive name; the example above shows the zstd
happy path.)

### 3.3 `run_wrapper_src` gains a `cache: bool` param (`remote.rs:128-130`)
```rust
pub(crate) fn run_wrapper_src(package: &str, cache: bool) -> String {
    WRAPPER_SRC
        .replace("{{PACKAGE}}", package)
        .replace("{{CACHE}}", if cache { "True" } else { "False" })
}
```
Update the unit test `wrapper_src_substitutes_package_and_is_pythonish`
(`remote.rs:463-472`): call `run_wrapper_src("example-add", true)`, assert BOTH
`{{PACKAGE}}` and `{{CACHE}}` are gone and `CACHE_ON = True` renders. Add a
`run_wrapper_src("example-add", false)` assertion: `CACHE_ON = False` and (no
unpack/pack reachable) — guarantees `cache=false` is a no-op-shaped wrapper.

### 3.4 `RemoteConfig.cache` (`remote.rs:148-205`)

Add a field to `RemoteConfig`, mirroring the `discover_install_rust` pattern:
```rust
    /// Enable the P6 cargo build cache (archive on a V2 volume at `/cache`).
    /// DEFAULT ON. Env opt-out: `MODAL_RUST_NO_CACHE` truthy. The decorator
    /// `#[function(cache=false)]` overrides this per-entrypoint (app.rs §3.6).
    pub cache: bool,
```
Initialize in `Default` (`remote.rs:188-205`): `cache: discover_cache(),` and:
```rust
/// Discover whether the cargo build cache is ON: default ON; `MODAL_RUST_NO_CACHE`
/// truthy (`1`/`true`/`yes`/`on`, case-insensitive) ⇒ OFF.
fn discover_cache() -> bool {
    !std::env::var("MODAL_RUST_NO_CACHE")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}
```

### 3.5 `ensure_function` resolves + attaches the volume (RUN path only, `remote.rs:280-406`)

Before building `fn_spec`, resolve the volume when caching is on:
```rust
let cache_vol_id: Option<String> = if config.cache {
    Some(
        client
            .volume_get_or_create(CACHE_VOLUME_NAME, true /*v2*/, true /*create*/, None)
            .await?,
    )
} else {
    None
};
```
Bake the cache flag into the wrapper (`remote.rs:355`):
```rust
.with_wrapper_module(WRAPPER_MODULE, run_wrapper_src(&config.package, config.cache))
```
Attach the mount on the FunctionSpec (`remote.rs:373-377`):
```rust
let mut fn_spec = FunctionSpec::new(WRAPPER_MODULE, WRAPPER_CALLABLE, &image_id)
    .with_mount_ids(vec![client_mount_id, source_mount_id])
    .with_mount_client_dependencies(true)
    .with_timeout_secs(timeout)
    .with_gpu(config.gpu.clone())?;
if let Some(vid) = cache_vol_id {
    fn_spec = fn_spec.with_volume_mount(vid, CACHE_MOUNT); // /cache, bg-commits=true
}
```
(`with_gpu` returns `Result`, so keep the `?` placement as today; the `if let`
attach happens after.)

### 3.6 Decorator override in `resolve_function` (`crates/modal-rust/src/app.rs:244-262`)

`FunctionConfig.cache: Option<bool>` already exists (`runtime/lib.rs:300`) and is
captured by `config_for`. Apply it to the per-call `run_config` clone next to the
existing gpu/timeout overrides (`app.rs:258-260`):
```rust
let cfg = self.config_for(entrypoint);            // app.rs:249
let cfg_cache: Option<bool> = cfg.cache;          // decorator override (None = defer)
// ... inside the run_config clone block (app.rs:258-260):
run_config.cache = cfg_cache.unwrap_or(run_config.cache); // Some(false) wins; None keeps default
```
**Precedence (final):** `#[function(cache=false)]` (explicit `Some(false)`) >
`MODAL_RUST_NO_CACHE` (folded into `RemoteConfig::default().cache`) > default ON.
The decorator wins because `Some(_)` overrides; `None` (bare `#[function]`) defers
to the env/default base. Matches the existing gpu/timeout override semantics.

### 3.7 DEPLOY path stays cache-OFF (`crates/modal-rust/src/app.rs` `deploy*`)

Deploy builds once at image-build time (Modal caches the image), so the cargo cache
is a no-op and would only leave a lingering volume mount on the persistent function.
P6 is RUN-only. **If the deploy bake shares `run_wrapper_src`/`ensure_function`,
force `cache=false` for the deploy bake AND attach no volume.** Verify at implement
time which build path `deploy_with` uses (`app.rs:378-409`); if it reuses the run
path, set `config.cache = false` for the deploy `RemoteConfig` so no volume is
resolved/attached and the wrapper renders `CACHE_ON = False`.

### 3.8 Opt-out / reset summary

| Layer | Mechanism | Effect |
|---|---|---|
| Decorator | `#[modal_rust::function(cache=false)]` → `FunctionConfig.cache=Some(false)` | per-entrypoint off (highest precedence) |
| Env | `MODAL_RUST_NO_CACHE=1` | process-wide off (folds into `RemoteConfig::default().cache`) |
| Default | none | ON |

Reset for a true cold run: `modal volume rm modal-rust-cargo-cache`
(or `modal volume rm -r`).

---

## 4. Correctness invariant (mechanically enforced)

- `CACHE_ON=False` ⇒ wrapper behavior is the CURRENT one (no unpack, no pack); the
  `{{CACHE}}` substitution renders the wrapper shape-identical to today.
- Corrupt/partial archive ⇒ `tar -xf` failure is caught ⇒ treated as COLD (logged,
  build proceeds). A failed PACK is caught ⇒ logged, call succeeds. A cache
  miss/failure thus ONLY costs time.
- Build NEVER on `/cache`: `CARGO_HOME`/`CARGO_TARGET_DIR` stay on `/tmp`; the
  volume holds ONLY the archive object.
- Empty `volume_mounts` ⇒ proto omits field 33 ⇒ byte-identical wire for the
  no-cache path. The SDK layer only adds the ABILITY to attach a volume; build
  correctness never depends on it.

---

## 5. Benchmark / live proof plan (behind `#[ignore]` + the live feature)

`retry_transient` on all RPCs; Modal flakiness ⇒ retry. Use ephemeral run apps;
reset the cache volume between cold runs.

### 5.1 Primary — cold-vs-warm on `example-burn-add` (heavy CUDA build)
Set `MODAL_RUST_CACHE_TARGET=1` (archive `target/` for max warm win).
1. RESET: `modal volume rm modal-rust-cargo-cache` (true cold).
2. COLD run (ephemeral, CUDA image): record `[cache] COLD (no archive)` + cargo
   build wall-clock from stderr; confirm `[cache] packed` wrote
   `/cache/cache.tar.zst`.
3. WARM run (a SECOND fresh ephemeral run; force a fresh container so warm-container
   reuse doesn't mask the test): record `[cache] WARM` + the new build wall-clock.
4. ASSERT warm build << cold; record the delta.
5. OPT-OUT: run with `cache=false` (decorator) AND `MODAL_RUST_NO_CACHE=1` ⇒
   `[cache] disabled` in stderr, NO volume mount attached, full cold each time.

### 5.2 Cheaper mechanism check (no GPU) — acceptable fallback
A heavy-ish PURE-CPU crate (or `example-add`'s crate graph as a smoke harness) on
`rust:1-slim`:
- COLD ⇒ assert archive WRITTEN (`/cache/cache.tar.zst` exists; log its byte size
  or `modal volume ls modal-rust-cargo-cache`).
- WARM ⇒ assert archive REUSED (`[cache] WARM`) + warm CARGO_HOME present
  (`/tmp/cargo/registry` populated before build) + a measurable cold-vs-warm delta.
- Per the task's "How to return", this (archive written/reused + warm cache present
  + recorded delta + `cache=false` opt-out) is the accepted mechanism proof if the
  full burn-add CUDA benchmark is too flaky.

---

## 6. Verification gates (WORKING.md)

On default-members, all green:
`cargo fmt --check`; `cargo clippy --all-targets -- -D warnings`; `cargo build`;
`cargo test`. New unit tests: §1.1 enum-mapping; §2.7 `volume_mounts`/`to_proto`;
`run_wrapper_src(pkg, false)` renders `CACHE_ON = False` and `(pkg, true)` renders
`CACHE_ON = True`; `RemoteConfig::default().cache` ON / `MODAL_RUST_NO_CACHE` flips
it off; decorator `cache=Some(false)` wins in `resolve_function`. All RPCs via
`retry_unary`/`retry_transient`. Live cache tests behind `#[ignore]` + live feature.

---

## 7. Proto ground truth (verified against generated `modal.client.rs`)

- `VolumeGetOrCreateRequest`: `deployment_name=1`, `reserved 2` (namespace — never
  set), `environment_name=3`, `object_creation_type=4` (`ObjectCreationType`),
  `app_id=5` (anonymous-app only), `version=6` (`VolumeFsVersion`). (api.proto:3880-3887)
- `VolumeGetOrCreateResponse`: `volume_id=1`, `version=2`, `metadata=3`. (api.proto:3889-3893)
- `VolumeFsVersion`: `Unspecified=0`, `V1=1`, `V2=2` (prost idents confirmed —
  `VolumeFsVersion::V2`, NOT `VolumeFsVersionV2`). (api.proto:312-315)
- `ObjectCreationType`: `Unspecified=0` (lookup), `CreateIfMissing=1`, … (prost
  idents confirmed). (api.proto:207-213)
- `VolumeMount`: `volume_id=1`, `mount_path=2`, `allow_background_commits=3`,
  `read_only=4`, `sub_path=5` (`Option<String>`). (api.proto:3944-3950)
- `Function.volume_mounts` = field **33**, `repeated VolumeMount`. (api.proto:1702)
- Stub method `volume_get_or_create` confirmed in the generated client
  (`modal.client.rs:12339`), mirroring `mount_get_or_create`. rpc `VolumeGetOrCreate`
  at api.proto:4373.
- **No proto regeneration needed** — all types/fields already exist in the
  generated code. Python parity: `Volume.from_name` (volume.py:585-630) sets ONLY
  `deployment_name`, `environment_name`, `object_creation_type`, `version`; does
  NOT set `namespace` (reserved 2) or `app_id`; `allow_background_commits=True` is a
  per-mount default (volume.py:369).

---

## 8. Touch list (additive only — no rewrite of working create/invoke)

- **NEW** `crates/modal-rust-sdk/src/ops/volume.rs` — `volume_get_or_create(name,
  v2, create_if_missing, env) -> volume_id` (§1.1).
- `crates/modal-rust-sdk/src/ops/mod.rs` — register `pub mod volume;` + doc bullet
  (§1.2).
- `crates/modal-rust-sdk/src/ops/function.rs` — import `VolumeMount`; add
  `FunctionVolumeMount`; `FunctionSpec.volume_mounts` (default empty) +
  `with_volume_mounts`/`with_volume_mount`; write field 33 in `function_create`;
  unit tests (§2).
- `crates/modal-rust/src/remote.rs` — `WRAPPER_SRC` cache logic + `{{CACHE}}`
  placeholder; `run_wrapper_src(pkg, cache)`; `RemoteConfig.cache` + `discover_cache`;
  constants `CACHE_MOUNT`/`CACHE_VOLUME_NAME`/`CACHE_ARCHIVE_NAME`; volume resolve +
  attach + cache-baked wrapper in `ensure_function` (§3.1-3.5).
- `crates/modal-rust/src/app.rs` — cache override in `resolve_function`
  (`app.rs:249-260`); force `cache=false` (no volume) on the DEPLOY bake (§3.6-3.7).
- **No change to:** runner / registry / macros / `FunctionConfig` (field already
  exists, `runtime/lib.rs:300`) / README.md.

---

RESULT: SPEC_DONE — merged P6 spec written to
`/Users/nicolas/devel/modal-rust/workpads/shim-backend/p6-cache-spec.md`.
