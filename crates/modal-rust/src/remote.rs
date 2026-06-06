//! The RUN-path remote machinery behind [`Function::remote`](crate::Function::remote).
//!
//! This module holds the parts of `.remote()` that are pure (no `&App` borrow) or
//! self-contained: the FILE-mode run wrapper source + per-package substitution, the
//! [`RemoteConfig`] knobs, the ensure-created control-plane sequence, and the runner
//! envelope → `Result<Out, Error>` mapping that mirrors `.local()` byte-for-byte.
//!
//! ## The build boundary (RUN path)
//!
//! The source crate is MOUNTED (`add_local_dir(copy=False)` equivalent) at `/src`,
//! and `cargo build` runs IN THE FUNCTION BODY at execution time — never at
//! image-build time. The run image (`rust` base + python + the baked wrapper)
//! carries NO `cargo` line. The wrapper itself runs `cargo build --release -p
//! <PACKAGE> --bin modal_runner` the first time a container handles a call, then
//! execs the freshly built `modal_runner` via the frozen runner CLI protocol.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use modal_rust_sdk::{FunctionSpec, ImageSpec, ModalClient};

use crate::{Error, Result, RunnerError};

/// Fixed importable module name for the baked run wrapper
/// (`/root/modal_rust_run_wrapper.py`).
pub(crate) const WRAPPER_MODULE: &str = "modal_rust_run_wrapper";
/// Fixed IN-CONTAINER callable within the wrapper module. EVERY entrypoint shares
/// this one dispatch callable — `handler(entrypoint, input_json)` routes by the
/// per-call entrypoint arg. It is the FILE-mode `getattr` target (Modal's
/// `implementation_name`), DECOUPLED from the per-entrypoint Modal object TAG (see
/// [`sanitize_object_tag`]). This stays frozen — the runner wire is unchanged.
pub(crate) const WRAPPER_CALLABLE: &str = "handler";

/// Sanitize an entrypoint name into a Modal object TAG (the app-namespace function
/// name that makes a created function unique within an app). Rust fn names are
/// already tag-safe (`[A-Za-z0-9_]`); this only defends against an unusual manual
/// registry name by mapping any other byte to `_`. An empty result falls back to the
/// shared callable so a tag is never empty.
pub(crate) fn sanitize_object_tag(entrypoint: &str) -> String {
    let tag: String = entrypoint
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if tag.is_empty() {
        WRAPPER_CALLABLE.to_string()
    } else {
        tag
    }
}
/// Where the uploaded source mount lands inside the container.
pub(crate) const REMOTE_SRC: &str = "/src";
/// Rust base image major version tag (`rust:{ver}-slim`).
pub(crate) const RUST_VER: &str = "1";
/// Python series to provision via `add_python` (the hosted python-build-standalone
/// mount). `< 3.13`, so the image gets the auto `ln -s python3 python` the bare
/// `python` entrypoint needs. Shared by the RUN and DEPLOY images.
pub(crate) const PYTHON_SERIES: &str = "3.12";
/// In-body `cargo build` needs far longer than the SDK's 300s invoke default.
pub(crate) const REMOTE_TIMEOUT_SECS: u32 = 1800;

/// Stable in-container mount path for the cargo-cache V2 volume (P6). The single
/// archive object lives at `{CACHE_MOUNT}/{CACHE_ARCHIVE_NAME}`.
pub(crate) const CACHE_MOUNT: &str = "/cache";
/// The single compressed archive object persisted on the cache volume (P6).
pub(crate) const CACHE_ARCHIVE_NAME: &str = "cache.tar.zst";
/// Deployment name of the persistent V2 cargo-cache volume (knowledge.md §C item 4).
pub(crate) const CACHE_VOLUME_NAME: &str = "modal-rust-cargo-cache";

/// Cumulative set of RUN-path functions published into ONE ephemeral app. Because
/// `AppPublish` is a SET-STATE publish (it REPLACES the app's function set, not
/// appends), creating a second per-entrypoint function and re-publishing must carry
/// the UNION of every function created so far — otherwise the second publish would
/// de-invoke the first entrypoint. Keyed by object tag (entrypoint) → function_id,
/// plus the function_id → definition_id side map [`AppPublish`] needs.
#[derive(Debug, Default)]
pub(crate) struct PublishedFunctions {
    /// Object tag (sanitized entrypoint) → invokable `function_id`.
    function_ids: HashMap<String, String>,
    /// `function_id` → `definition_id` (only for functions that returned one).
    definition_ids: HashMap<String, String>,
}

impl PublishedFunctions {
    /// Record a freshly-created function under its object tag and return the FULL
    /// (cumulative) `(function_ids, definition_ids)` maps to publish.
    fn record(
        &mut self,
        object_tag: &str,
        function_id: &str,
        definition_id: &str,
    ) -> (HashMap<String, String>, HashMap<String, String>) {
        self.function_ids
            .insert(object_tag.to_string(), function_id.to_string());
        if !definition_id.is_empty() {
            self.definition_ids
                .insert(function_id.to_string(), definition_id.to_string());
        }
        (self.function_ids.clone(), self.definition_ids.clone())
    }
}

/// The FILE-mode run wrapper, ported from `workpads/prototype/dev_app.py`'s
/// `run_entrypoint`. The `{{PACKAGE}}` placeholder is substituted per package by
/// [`run_wrapper_src`] before being baked into the image (base64) — so no shell
/// quoting is required.
///
/// Modal FILE-mode resolves `import_module("modal_rust_run_wrapper")` +
/// `getattr(mod, "handler")`, then calls `handler(*args, **kwargs)`. The facade
/// invokes with `args = (entrypoint, input_json)`, `kwargs = {}`, so `handler`
/// receives TWO positional args. It builds the mounted crate in the function body,
/// execs `modal_runner`, and RETURNS the one-line JSON envelope string verbatim;
/// the facade parses it ([`parse_envelope`]).
const WRAPPER_SRC: &str = r#""""modal-rust FILE-mode run wrapper (ports dev_app.py run_entrypoint).

Baked to /root/modal_rust_run_wrapper.py. Builds the mounted Rust crate IN THE
FUNCTION BODY (run boundary: cargo at execution time, never at image-build time),
execs the frozen modal_runner, and RETURNS the one-line JSON envelope verbatim.
"""
import os, shutil, subprocess, sys

PACKAGE    = "{{PACKAGE}}"      # injected: cargo -p <pkg>
CACHE_ON   = {{CACHE}}          # injected: True / False (P6 cargo build cache)
REMOTE_SRC = "/src"            # source mount path
_RUNNER    = "/tmp/target/release/modal_runner"
_MARKER    = "/tmp/.modal_rust_built"
_BUILT     = False

# P6 cache: a SINGLE compressed archive on a V2 volume mounted at /cache. We build
# on FAST LOCAL DISK (/tmp) and persist only this one object — the volume never
# holds CARGO_HOME/target directly (Modal volumes degrade past ~50k files). zstd is
# preferred; if the base lacks the `zstd` binary we fall back to gzip (.tar.gz).
# Selection is by existing-archive extension so cold<->warm stays consistent.
_ARCHIVE_ZSTD = "{{ARCHIVE_ZSTD}}"
_ARCHIVE_GZIP = "{{ARCHIVE_GZIP}}"
# Lock files regenerate; excluding them avoids stale-lock churn in the archive.
_PACK_EXCLUDES = [
    "--exclude=cargo/registry/cache/.package-cache",
    "--exclude=cargo/.package-cache",
]


def _cache_target_on():
    # OPTIONALLY archive target/ too (largest tree). Gated by env, default OFF in v0;
    # flip ON (MODAL_RUST_CACHE_TARGET=1) for the heavy burn-add benchmark.
    return os.environ.get("MODAL_RUST_CACHE_TARGET", "").strip().lower() in ("1", "true", "yes", "on")


def _existing_archive():
    # Prefer an archive that already exists (keeps cold<->warm consistent in a volume).
    if os.path.exists(_ARCHIVE_ZSTD):
        return _ARCHIVE_ZSTD
    if os.path.exists(_ARCHIVE_GZIP):
        return _ARCHIVE_GZIP
    return None


def _unpack_cache():
    # Restore the warm CARGO_HOME (and optionally target/) onto /tmp BEFORE cargo runs.
    # A missing/corrupt archive is treated as COLD (logged) — a cache miss only costs
    # time, never changes the build result.
    if not CACHE_ON:
        return "disabled"
    archive = _existing_archive()
    if archive is None:
        return "COLD (no archive)"
    flag = "--zstd" if archive.endswith(".zst") else "-z"
    try:
        subprocess.run(["tar", flag, "-xf", archive, "-C", "/tmp"], check=True,
                       stdout=sys.stderr, stderr=sys.stderr)
        return "WARM"
    except Exception as e:  # corrupt/partial archive => treat as COLD, never raise
        print(f"[cache] unpack failed (treated as COLD): {e!r}", file=sys.stderr)
        return "COLD (unpack failed)"


def _pack_one(archive, flag, dirs):
    tmp = archive + ".tmp"
    try:
        subprocess.run(
            ["tar", flag, *_PACK_EXCLUDES, "-cf", tmp, "-C", "/tmp", *dirs],
            check=True, stdout=sys.stderr, stderr=sys.stderr,
        )
    except Exception:
        # tar failed mid-write (e.g. `--zstd` on a base without the zstd binary):
        # remove the partial temp so it never lingers on the volume, then re-raise so
        # the caller can fall back (gzip) or log+ignore. Atomic temp+rename means a
        # reader never sees a half archive; this just keeps the volume clean.
        if os.path.exists(tmp):
            os.remove(tmp)
        raise
    os.replace(tmp, archive)  # atomic rename on the same fs; no reload/commit needed
    print(f"[cache] packed {archive}", file=sys.stderr)


def _pack_cache():
    # Persist the enriched archive after the FIRST cold build only. Atomic temp+rename
    # within /cache; allow_background_commits flushes it (NO vol.reload/commit on the
    # hot path — cargo holds locks). A failed pack must NEVER fail the call.
    if not CACHE_ON:
        return
    dirs = ["cargo"]
    if _cache_target_on():
        dirs.append("target")
    # Keep the same format as any existing archive; default to zstd, fall back to gzip
    # if the `zstd` binary is missing on the base image.
    existing = _existing_archive()
    try:
        if existing == _ARCHIVE_GZIP:
            _pack_one(_ARCHIVE_GZIP, "-z", dirs)
        else:
            try:
                _pack_one(_ARCHIVE_ZSTD, "--zstd", dirs)
            except Exception as e:
                print(f"[cache] zstd pack unavailable ({e!r}); falling back to gzip", file=sys.stderr)
                _pack_one(_ARCHIVE_GZIP, "-z", dirs)
    except Exception as e:  # a failed pack must NOT fail the call
        print(f"[cache] pack failed (ignored): {e!r}", file=sys.stderr)


def _env():
    e = dict(os.environ)
    e["CARGO_HOME"] = "/tmp/cargo"
    e["CARGO_TARGET_DIR"] = "/tmp/target"
    e["RUST_BACKTRACE"] = "1"
    return e


def _build_dir():
    if os.access(REMOTE_SRC, os.W_OK):
        print(f"[run] mount {REMOTE_SRC} writable; building in place", file=sys.stderr)
        return REMOTE_SRC
    build_dir = "/tmp/build"
    print(f"[run] mount {REMOTE_SRC} read-only; cp -a -> {build_dir}", file=sys.stderr)
    if os.path.exists(build_dir):
        shutil.rmtree(build_dir)
    subprocess.run(["cp", "-a", REMOTE_SRC, build_dir], check=True)
    return build_dir


def _build(env):
    global _BUILT
    if _BUILT or os.path.exists(_MARKER):
        _BUILT = True
        print("[run] build cached (warm container); skipping cargo build", file=sys.stderr)
        return
    print(f"[cache] {_unpack_cache()}", file=sys.stderr)  # warm CARGO_HOME if archive present
    build_dir = _build_dir()
    # Capture combined cargo output so a build FAILURE surfaces its CAUSE in the
    # raised error (rides back through the envelope), not just an opaque exit code.
    # Output is also echoed to Modal logs verbatim for the full transcript.
    b = subprocess.run(
        ["cargo", "build", "--release", "-p", PACKAGE, "--bin", "modal_runner"],
        cwd=build_dir, env=env, capture_output=True, text=True,
    )
    if b.stdout:
        print(b.stdout, file=sys.stderr)
    if b.stderr:
        print(b.stderr, file=sys.stderr)
    if b.returncode != 0:
        # The last lines of cargo's stderr carry the actual rustc/linker error; a
        # tail keeps the message bounded while still naming the failing crate/error.
        tail = (b.stderr or b.stdout or "")[-1500:]
        raise RuntimeError(
            f"cargo build failed with exit code {b.returncode}; stderr tail:\n{tail}"
        )
    open(_MARKER, "w").close()
    _BUILT = True
    _pack_cache()  # cold path only; persist the enriched archive (best-effort)


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
"#;

/// Substitute `{{PACKAGE}}` + `{{CACHE}}` (+ the archive paths) into [`WRAPPER_SRC`].
/// `package` is a cargo package name (crate-name-shaped: `[A-Za-z0-9_-]`); it is NOT
/// shell-quoted because the source is base64-baked into the Dockerfile. `cache`
/// renders the literal Python `True`/`False` for `CACHE_ON` (the wrapper's only config
/// channel for P6) — with `False` the unpack/pack are no-ops and the wrapper is
/// shape-identical to pre-P6.
///
/// The archive paths are derived from [`CACHE_MOUNT`] + [`CACHE_ARCHIVE_NAME`] (the
/// gzip fallback swaps `.zst` → `.gz`), so the Python literals can never drift from
/// the Rust constants used to mount + attach the volume.
pub(crate) fn run_wrapper_src(package: &str, cache: bool) -> String {
    let archive_zstd = format!("{CACHE_MOUNT}/{CACHE_ARCHIVE_NAME}");
    let archive_gzip = archive_zstd
        .strip_suffix(".zst")
        .map(|stem| format!("{stem}.gz"))
        .unwrap_or_else(|| format!("{archive_zstd}.gz"));
    WRAPPER_SRC
        .replace("{{PACKAGE}}", package)
        .replace("{{CACHE}}", if cache { "True" } else { "False" })
        .replace("{{ARCHIVE_ZSTD}}", &archive_zstd)
        .replace("{{ARCHIVE_GZIP}}", &archive_gzip)
}

/// All knobs for the RUN path. One struct, no per-project file.
///
/// ## Source-upload scoping & ignore resolution
///
/// The source upload carries ONLY the cargo dependency closure of the target
/// [`package`](RemoteConfig::package) — its workspace-member normal path deps — plus
/// the workspace `Cargo.toml`/`Cargo.lock` (when [`use_cargo_scoping`] is `true` and
/// `cargo metadata` is available; otherwise the whole [`local_root`] is uploaded
/// minus ignored files). Non-source assets (datasets, model weights, fixtures) are
/// NOT uploaded with the source — attach them via **Modal Volumes**.
///
/// Within the uploaded directories, files are pruned by ignore-file precedence
/// (highest → lowest): [`modalignore_name`](RemoteConfig::modalignore_name) (default
/// `.modalignore`) → `.gitignore` → built-in defaults (`target/`, `.git/`,
/// `**/*.rlib`). Both ignore files are read from the workspace root.
#[derive(Debug, Clone)]
pub struct RemoteConfig {
    /// Directory uploaded as the source mount (defaults to the cargo workspace
    /// root; override with `MODAL_RUST_SOURCE_DIR`). Also the workspace root for
    /// cargo-metadata scoping and ignore-file resolution.
    pub local_root: PathBuf,
    /// Cargo package owning the entrypoints (`cargo -p <package>`). The
    /// `modal_runner` bin name is shared across workspace members, so this
    /// disambiguates. Also the cargo-metadata scoping target. Override with
    /// `MODAL_RUST_PACKAGE`.
    pub package: String,
    /// Where the source mount lands in-container.
    pub remote_src: String,
    /// Whether to scope the upload to the target package's cargo dependency closure
    /// via `cargo metadata` (default `true`). `false` forces the whole-`local_root`
    /// upload (still pruned by the resolved ignore files).
    pub use_cargo_scoping: bool,
    /// Highest-precedence ignore filename, read from the workspace root (default
    /// `.modalignore`). Falls through to `.gitignore` then the built-in defaults.
    pub modalignore_name: String,
    /// Base registry tag for the run image.
    pub base_image: String,
    /// Function timeout (seconds) — covers the in-body cargo build.
    pub timeout_secs: u32,
    /// GPU spec for this run's entrypoint (from the decorator [`FunctionConfig`]).
    /// `None` = CPU. Set by `App::remote_invoke` from `config_for(entrypoint)`
    /// before [`ensure_function`].
    pub gpu: Option<String>,
    /// Per-entrypoint timeout override (decorator `FunctionConfig.timeout_secs`).
    /// When `Some`, REPLACES the path default [`timeout_secs`](RemoteConfig::timeout_secs).
    pub timeout_override_secs: Option<u32>,
    /// Install the Rust toolchain (rustup) + the CUDA build/run env into the run
    /// image. Set when [`base_image`](RemoteConfig::base_image) is a non-Rust base
    /// (e.g. a `nvidia/cuda:<ver>-devel` Tier-1 base; boundaries.md §9) so the
    /// in-body `cargo build` finds a toolchain on PATH. Default `false` (the
    /// `rust:1-slim` base already carries Rust). Env override:
    /// `MODAL_RUST_INSTALL_RUST` (`1`/`true`/`yes`/`on`).
    pub install_rust: bool,
    /// Enable the P6 cargo build cache (one archive on a V2 volume at `/cache`).
    /// DEFAULT ON. Env opt-out: `MODAL_RUST_NO_CACHE` truthy. The decorator
    /// `#[function(cache=false)]` overrides this per-entrypoint (app.rs). A cache
    /// miss/failure only costs time — it NEVER changes the build result.
    pub cache: bool,
    /// Named Modal secrets to attach (from `#[function(secrets = [..])]`). Each name
    /// is resolved to a `secret_id` via [`ModalClient::secret_get_or_create`] and
    /// attached to `FunctionCreate.secret_ids`; Modal injects the secret's
    /// key/values as ENV VARS in the container. DEFAULT EMPTY (wire-identical to
    /// before). Set by `App::resolve_function` from `config_for(entrypoint)`.
    pub secrets: Vec<String>,
    /// User volumes to attach as `(mount_path, volume_name)` pairs (from
    /// `#[function(volumes = ["/data=my-vol"])]`). Each `volume_name` is resolved via
    /// [`ModalClient::volume_get_or_create`] and mounted at `mount_path` — a SEPARATE
    /// mount from the P6 cargo cache (`/cache`), so both coexist. DEFAULT EMPTY.
    pub volumes: Vec<(String, String)>,
}

impl RemoteConfig {
    /// Fill in [`package`](RemoteConfig::package) from the macro-captured inventory
    /// package when the user has NOT set `MODAL_RUST_PACKAGE`.
    ///
    /// Precedence (highest → lowest): `MODAL_RUST_PACKAGE` (`env_override`) → the
    /// macro-captured `env!("CARGO_PKG_NAME")` (`detected`, from
    /// [`modal_rust_runtime::package_from_inventory`]) → the existing value (the v0
    /// default left by [`discover_package`]). `env_override`/`detected` are passed
    /// in (rather than read here) so this stays a pure, unit-testable transform.
    ///
    /// This is what makes the library `App::connect(..).remote()` path build the
    /// RIGHT package automatically: the macro ran in the user's crate, so `detected`
    /// is the user's package, not the facade's hardcoded `"example-add"`.
    pub fn with_detected_package(
        mut self,
        env_override: Option<&str>,
        detected: Option<&str>,
    ) -> Self {
        // The env var (when set) already won inside `discover_package`; only fill
        // from the detected inventory package when there is no env override.
        if env_override.is_none() {
            if let Some(pkg) = detected {
                self.package = pkg.to_string();
            }
        }
        self
    }
}

impl Default for RemoteConfig {
    fn default() -> Self {
        RemoteConfig {
            local_root: discover_local_root(),
            package: discover_package(),
            remote_src: REMOTE_SRC.to_string(),
            use_cargo_scoping: true,
            modalignore_name: modal_rust_sdk::DEFAULT_MODALIGNORE_NAME.to_string(),
            base_image: discover_base_image(),
            timeout_secs: REMOTE_TIMEOUT_SECS,
            gpu: None,
            timeout_override_secs: None,
            install_rust: discover_install_rust(),
            cache: discover_cache(),
            secrets: Vec::new(),
            volumes: Vec::new(),
        }
    }
}

/// Discover the source dir to upload: `MODAL_RUST_SOURCE_DIR` if set, else the
/// nearest ancestor `Cargo.toml` containing `[workspace]` (walking up from CWD),
/// else the nearest `Cargo.toml` dir, else CWD.
fn discover_local_root() -> PathBuf {
    if let Ok(dir) = std::env::var("MODAL_RUST_SOURCE_DIR") {
        return PathBuf::from(dir);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut nearest_manifest: Option<PathBuf> = None;
    let mut cur: Option<&Path> = Some(cwd.as_path());
    while let Some(dir) = cur {
        let manifest = dir.join("Cargo.toml");
        if manifest.is_file() {
            if nearest_manifest.is_none() {
                nearest_manifest = Some(dir.to_path_buf());
            }
            if std::fs::read_to_string(&manifest)
                .map(|s| s.contains("[workspace]"))
                .unwrap_or(false)
            {
                return dir.to_path_buf();
            }
        }
        cur = dir.parent();
    }
    nearest_manifest.unwrap_or(cwd)
}

/// Discover the cargo package for `-p`: `MODAL_RUST_PACKAGE` if set, else the v0
/// default `"example-add"`.
///
/// This is the BASE; the real package usually comes from AUTO-DETECT — the
/// `#[modal_rust::function]` macro captures the user crate's `env!("CARGO_PKG_NAME")`
/// into the inventory, and [`App::connect`](crate::App::connect) folds it in via
/// [`RemoteConfig::with_detected_package`]. The env var still OVERRIDES both. The v0
/// default only survives when there is NO env var AND no decorated handler (a manual
/// registry with no `#[function]`), in which case the user supplies an explicit
/// `RemoteConfig` or sets `MODAL_RUST_PACKAGE`.
fn discover_package() -> String {
    std::env::var("MODAL_RUST_PACKAGE").unwrap_or_else(|_| "example-add".to_string())
}

/// Discover the run base image: `MODAL_RUST_BASE_IMAGE` if set, else the default
/// `rust:{RUST_VER}-slim`. An env-driven run path can point at the CUDA-devel base
/// (e.g. `nvidia/cuda:12.6.3-devel-ubuntu22.04`) without touching code — parity with
/// `MODAL_RUST_SOURCE_DIR` / `MODAL_RUST_PACKAGE`.
fn discover_base_image() -> String {
    std::env::var("MODAL_RUST_BASE_IMAGE").unwrap_or_else(|_| format!("rust:{RUST_VER}-slim"))
}

/// Discover whether to install the Rust toolchain into the run image:
/// `MODAL_RUST_INSTALL_RUST` truthy (`1`/`true`/`yes`/`on`, case-insensitive) ⇒
/// `true`, else `false`. Paired with `MODAL_RUST_BASE_IMAGE` for an env-driven CUDA
/// run path (the CUDA base has no Rust).
fn discover_install_rust() -> bool {
    std::env::var("MODAL_RUST_INSTALL_RUST")
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            matches!(v.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false)
}

/// Discover whether the cargo build cache is ON: default ON; `MODAL_RUST_NO_CACHE`
/// truthy (`1`/`true`/`yes`/`on`, case-insensitive) ⇒ OFF.
fn discover_cache() -> bool {
    !std::env::var("MODAL_RUST_NO_CACHE")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Discover whether to ALSO archive `target/` (not just CARGO_HOME) in the cache:
/// `MODAL_RUST_CACHE_TARGET` truthy (`1`/`true`/`yes`/`on`, case-insensitive) ⇒
/// `true`. Default OFF. When ON (and caching is on) the facade bakes the same var
/// into the image ENV so the remote wrapper packs/unpacks `target/` too — the local
/// process env does NOT otherwise reach the Modal container.
fn discover_cache_target() -> bool {
    std::env::var("MODAL_RUST_CACHE_TARGET")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Ensure the run function for `entrypoint` exists on Modal and return its invokable
/// `function_id`.
///
/// Runs the full create sequence (client mount, uploaded source mount, run image,
/// precreate, FunctionCreate FILE, **EPHEMERAL** AppPublish, from_name).
/// Idempotent at the Modal level (get-or-create semantics); callers memoize the
/// result per (App, entrypoint-config) so it runs at most once per process.
///
/// ## One Modal function PER ENTRYPOINT (the object-tag decoupling)
///
/// Each entrypoint is created as a DISTINCT Modal function whose object TAG is the
/// (sanitized) entrypoint name, carrying its OWN config (gpu/timeout/cache/secrets/
/// volumes). The IN-CONTAINER callable stays the shared `handler(entrypoint,
/// input_json)` dispatch ([`WRAPPER_CALLABLE`], rolled onto `implementation_name`),
/// so divergent per-entrypoint configs COEXIST instead of clobbering one shared
/// `"handler"` tag in the same app. Same-config entrypoints still single-flight via
/// the caller's per-key memo.
///
/// ## RUN publishes EPHEMERAL, not DEPLOYED
///
/// The RUN path runs inside an EPHEMERAL app ([`crate::App::connect`] uses
/// `app_create_ephemeral`). It DOES call `AppPublish` — publishing is REQUIRED to
/// make the created function invokable (without it, `FunctionMap` fails "function
/// not found", live-verified 2026-06-04) — but with `APP_STATE_EPHEMERAL`, NOT
/// `APP_STATE_DEPLOYED`. The ephemeral state keeps the app "discharged when the
/// client disconnects" (proto), so a `.remote()` leaves NO lingering persistent
/// deploy. Publishing with `APP_STATE_DEPLOYED` (the prior bug) promoted the
/// ephemeral app to a PERSISTENT `deployed` one that lingered (`modal app list`
/// showed `modal-rust-live-remote` `deployed`, `Stopped at: None`). This mirrors
/// Modal Python's `runner.py`, which publishes ephemeral runs and deploys alike,
/// differing ONLY in the state. PERSISTENT (DEPLOYED) publish is DEPLOY-only
/// ([`crate::App::deploy`]).
pub(crate) async fn ensure_function(
    client: &mut ModalClient,
    app_id: &str,
    app_name: &str,
    entrypoint: &str,
    config: &RemoteConfig,
    published: &mut PublishedFunctions,
) -> Result<String> {
    // 1. Cargo-cache volume (P6, RUN path only): resolve the persistent V2 volume
    //    (create-if-missing) when caching is on, so we can attach it below. When
    //    caching is off (decorator `cache=false` / `MODAL_RUST_NO_CACHE`) we resolve
    //    NO volume and the wrapper renders `CACHE_ON = False` — wire-identical to
    //    pre-P6. The DEPLOY path never reaches here (it has its own build at
    //    image-build time and never attaches a volume).
    let cache_vol_id: Option<String> = if config.cache {
        Some(
            client
                .volume_get_or_create(
                    CACHE_VOLUME_NAME,
                    true, /* v2 */
                    true, /* create */
                    None,
                )
                .await?,
        )
    } else {
        None
    };

    // 1b. User secrets (USER-facing `#[function(secrets=..)]`): resolve each named
    //     Modal secret to a secret_id (pure lookup via from_name semantics) so it can
    //     be attached below. Modal injects the secret's key/values as ENV VARS in the
    //     container. EMPTY ⇒ no secrets ⇒ wire-identical to before. Values never
    //     logged. SEPARATE concern from the cargo cache.
    let mut secret_ids: Vec<String> = Vec::with_capacity(config.secrets.len());
    for name in &config.secrets {
        secret_ids.push(client.secret_get_or_create(name, &[], None).await?);
    }

    // 1c. User volumes (USER-facing `#[function(volumes=["/mount=name"])]`): resolve
    //     each named Modal volume to a volume_id (create-if-missing) so it can be
    //     attached at its mount_path below. These are DISTINCT mounts from the P6
    //     cargo cache (`/cache`) — both coexist. User volumes use V1 (server default;
    //     general-purpose persistent storage), not the V2 the cargo cache needs. A
    //     user mount at `/cache` would collide with the cargo cache, so reject it.
    let mut user_volume_mounts: Vec<(String, String)> = Vec::with_capacity(config.volumes.len());
    for (mount_path, name) in &config.volumes {
        if config.cache && mount_path == CACHE_MOUNT {
            return Err(Error::config(format!(
                "user volume mount path {CACHE_MOUNT:?} collides with the cargo-cache \
                 volume; choose a different mount path (or disable the cache)"
            )));
        }
        let vid = client
            .volume_get_or_create(name, false /* v1 */, true /* create */, None)
            .await?;
        user_volume_mounts.push((vid, mount_path.clone()));
    }

    // 2. Client mount (modal source importable in the FILE-mode container).
    let client_mount_id = client.client_mount_id(None).await?;

    // 3. Source mount (UPLOAD the user's crate; `cargo build` reads it at /src).
    //    PRIMARY: cargo-metadata scoping uploads only the target package's
    //    workspace-member dependency-closure crate dirs + the workspace
    //    Cargo.toml/Cargo.lock. FALLBACK (cargo metadata unavailable): the whole
    //    `local_root` minus ignored files. Both prune via `.modalignore` >
    //    `.gitignore` > built-in defaults (resolved in the SDK).
    let source_mount_id = match (
        config.use_cargo_scoping,
        crate::scope::workspace_closure(&config.local_root, &config.package),
    ) {
        (true, Some(closure)) => {
            let spec = modal_rust_sdk::WorkspaceClosureSpec {
                workspace_root: &config.local_root,
                crate_dirs: &closure.dirs,
                extra_files: &closure.extra_files,
                extra_inline_files: &closure.inline_files,
                modalignore_name: &config.modalignore_name,
            };
            client
                .mount_workspace_closure(&spec, &config.remote_src, None)
                .await?
        }
        _ => {
            client
                .mount_local_dir(
                    &config.local_root,
                    &config.remote_src,
                    &config.modalignore_name,
                    None,
                )
                .await?
        }
    };

    // 3b. Python-standalone mount (the HOSTED python-build-standalone, resolved by
    //     name exactly like the client mount). Supplies `/python` to the image's
    //     `COPY /python/. /usr/local`.
    let py_mount_id = client
        .python_standalone_mount_id(PYTHON_SERIES, None)
        .await?;

    // 4. Run image: rust base + add_python(standalone) + the baked wrapper.
    //
    // We provision Python the way the official client does — via the python-build-
    // standalone mount (`add_python`), NOT apt. The rendered image emits
    // `COPY /python/. /usr/local`, an auto `RUN ln -s /usr/local/bin/python3
    // /usr/local/bin/python` for series < 3.13 (the client-equivalent of
    // `python-is-python3`: a symlink against the standalone install, not an apt
    // package — so Modal's bare `python` entrypoint resolves), and the TERMINFO ENV.
    // The standalone interpreter is relocatable and NOT PEP-668 externally-managed,
    // so `--break-system-packages` is moot. The modal client SOURCE rides the client
    // mount; its dep closure is injected by the worker at container start
    // (FunctionSpec defaults `mount_client_dependencies = true`) — so there is NO apt
    // layer and NO `pip install modal`. (apt+pip is retained as a documented fallback
    // in the SDK, selected only when `add_python` is unset.)
    // When the base image carries NO Rust (a CUDA-devel Tier-1 base set via
    // `MODAL_RUST_BASE_IMAGE` + `MODAL_RUST_INSTALL_RUST`), add the rustup + CUDA-env
    // step so the in-body `cargo build` finds a toolchain on PATH. The default
    // `rust:1-slim` base never sets this (byte-identical default render).
    let mut spec = ImageSpec::from_registry(config.base_image.clone())
        .with_add_python(PYTHON_SERIES)
        .with_python_standalone_mount_id(py_mount_id);
    if config.install_rust {
        spec = spec.with_rust_toolchain();
    }
    let mut spec = spec
        .with_wrapper_module(
            WRAPPER_MODULE,
            run_wrapper_src(&config.package, config.cache),
        )
        .with_command("ENV RUST_BACKTRACE=1");
    // P6: target/ caching is opt-in via `MODAL_RUST_CACHE_TARGET` (default OFF: in v0
    // only CARGO_HOME — registry index + crate tarballs — is archived). The wrapper
    // reads this var from the CONTAINER env, but the local process env does NOT cross
    // to Modal, so when caching is on AND the var is set locally we BAKE it into the
    // image ENV. This is what makes the warm build skip recompilation (cargo sees a
    // restored `target/` → `Fresh`, not `Compiling`), which is the dominant warm win
    // on a heavy crate. Default path (var unset) renders byte-identical to pre-P6.
    if config.cache && discover_cache_target() {
        spec = spec.with_command("ENV MODAL_RUST_CACHE_TARGET=1");
    }
    let spec = spec.with_command("ENTRYPOINT []");
    let image_id = client.image_get_or_create(app_id, &spec).await?;

    // 5. Precreate the function under the PER-ENTRYPOINT object tag (the sanitized
    //    entrypoint name), NOT the shared "handler" callable — so each entrypoint
    //    registers a DISTINCT Modal function and divergent configs never collide.
    let object_tag = sanitize_object_tag(entrypoint);
    let precreate_id = client.function_precreate(app_id, &object_tag).await?;

    // 6. FunctionCreate (FILE mode): both mounts attach via Function.mount_ids.
    //    `mount_client_dependencies` defaults true (set explicitly here) so the
    //    worker injects the modal client's dep closure at container start — the
    //    add_python image carries no `pip install modal` layer.
    // Decorator config: a `timeout` decorator OVERRIDES the path default literally
    // (Python honors it literally too). DOC: RUN-path timeouts must budget for the
    // cold in-body `cargo build`; a too-small decorator timeout can starve the first
    // cold build (no floor is imposed). `with_gpu(None)` is a CPU no-op (gpu_config
    // stays unset → CPU wire bytes identical).
    let timeout = config.timeout_override_secs.unwrap_or(config.timeout_secs);
    // Object TAG = the entrypoint (unique per app, its own config); IN-CONTAINER
    // callable stays WRAPPER_CALLABLE ("handler"), rolled onto `implementation_name`.
    let mut fn_spec = FunctionSpec::new(WRAPPER_MODULE, WRAPPER_CALLABLE, &image_id)
        .with_app_function_name(&object_tag)
        .with_mount_ids(vec![client_mount_id, source_mount_id])
        .with_mount_client_dependencies(true)
        .with_timeout_secs(timeout)
        .with_gpu(config.gpu.clone())?;
    // P6: attach the resolved cargo-cache volume at /cache with background commits
    // ENABLED. Only when caching is on (else `cache_vol_id` is None ⇒ no volume ⇒
    // wire-identical to pre-P6).
    if let Some(vid) = cache_vol_id {
        fn_spec = fn_spec.with_volume_mount(vid, CACHE_MOUNT);
    }
    // USER volumes: attach each resolved (volume_id, mount_path) at its DISTINCT
    // mount path — SEPARATE from the cargo cache above (both coexist). Empty ⇒ no-op.
    for (vid, mount_path) in user_volume_mounts {
        fn_spec = fn_spec.with_volume_mount(vid, mount_path);
    }
    // USER secrets: attach the resolved secret ids → Function.secret_ids (Modal
    // injects their key/values as ENV VARS). Empty ⇒ no-op (wire-identical).
    if !secret_ids.is_empty() {
        fn_spec = fn_spec.with_secret_ids(secret_ids);
    }
    let created = client
        .function_create(app_id, &precreate_id, &fn_spec)
        .await?;

    // 7. AppPublish with APP_STATE_EPHEMERAL. Publishing is REQUIRED to make the
    //    created function INVOKABLE (without it, FunctionMap fails "function not
    //    found" — live-verified 2026-06-04). The EPHEMERAL state keeps the app
    //    throwaway: it is "discharged when the client disconnects" (proto), so the
    //    RUN path leaves NO lingering deploy. PERSISTENT (DEPLOYED) publish is
    //    DEPLOY-only (`crate::deploy`). Mirrors Modal Python's `runner.py`, which
    //    publishes ephemeral runs and deploys alike, differing only in state.
    //
    //    Publish the CUMULATIVE union of every entrypoint created on this app under
    //    its OWN object tag: `AppPublish` REPLACES the function set, so a second
    //    per-entrypoint create must re-publish the first one too or it would be
    //    de-invoked. The function is keyed by the entrypoint object tag, not the
    //    shared "handler" callable, so distinct entrypoints coexist as distinct
    //    functions.
    let (function_ids, definition_ids) =
        published.record(&object_tag, &created.function_id, &created.definition_id);
    client
        .app_publish_ephemeral(app_id, app_name, function_ids, definition_ids)
        .await?;

    // 8. Invoke via the FunctionCreate `function_id` DIRECTLY — NOT `from_name`.
    //    `from_name`/`FunctionGet` is the DEPLOYED lookup; an EPHEMERAL app is not
    //    name-resolvable in the environment (live-verified 2026-06-04: from_name on
    //    the ephemeral app failed "App '...' not found in environment 'main'").
    //    Modal Python's ephemeral `app.run()` likewise invokes the loaded function
    //    handle by its `object_id`, never re-resolving by name.
    Ok(created.function_id)
}

/// Parse the runner's one-line JSON envelope into `Result<Out, Error>`, mirroring
/// `.local()` (`Function::local`) EXACTLY: `ok:true` → decode `value` into `Out`;
/// otherwise reconstruct the frozen [`RunnerError`] and wrap as [`Error::Runner`].
pub(crate) fn parse_envelope<Out>(envelope: &str) -> Result<Out>
where
    Out: serde::de::DeserializeOwned,
{
    let v: serde_json::Value = serde_json::from_str(envelope).map_err(Error::Decode)?;
    if v.get("ok") == Some(&serde_json::Value::Bool(true)) {
        let value = v.get("value").cloned().unwrap_or(serde_json::Value::Null);
        serde_json::from_value::<Out>(value).map_err(Error::Decode)
    } else {
        let err = v.get("error").cloned().unwrap_or(serde_json::Value::Null);
        Err(Error::Runner(reconstruct_runner_error(&err)))
    }
}

/// Map a `{"kind","message","details","backtrace"}` failure object back to the
/// FROZEN five-kind [`RunnerError`] taxonomy. An unrecognized kind degrades to
/// [`RunnerError::Decode`] with a clear message (never a panic).
fn reconstruct_runner_error(error: &serde_json::Value) -> RunnerError {
    let kind = error.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    let message = error
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    match kind {
        "decode_error" => RunnerError::Decode(message),
        "unknown_entrypoint" => RunnerError::UnknownEntrypoint(message),
        "function_error" => {
            let details = match error.get("details") {
                Some(serde_json::Value::Null) | None => None,
                Some(other) => Some(other.clone()),
            };
            RunnerError::Function { message, details }
        }
        "encode_error" => RunnerError::Encode(message),
        "panic" => {
            let backtrace = error
                .get("backtrace")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            RunnerError::Panic { message, backtrace }
        }
        other => RunnerError::Decode(format!("unrecognized error kind: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_object_tag_passes_rust_fn_names_and_maps_others() {
        // Rust fn names are already tag-safe (alphanumeric + `_`).
        assert_eq!(sanitize_object_tag("add"), "add");
        assert_eq!(sanitize_object_tag("add_gpu"), "add_gpu");
        assert_eq!(sanitize_object_tag("Train2"), "Train2");
        // `-` and `.` are allowed verbatim.
        assert_eq!(sanitize_object_tag("my-fn.v2"), "my-fn.v2");
        // Anything else maps to `_`.
        assert_eq!(sanitize_object_tag("a b/c"), "a_b_c");
        // An empty/all-mapped name never yields an empty tag.
        assert_eq!(sanitize_object_tag(""), WRAPPER_CALLABLE);
    }

    #[test]
    fn published_functions_accumulate_union_for_set_state_publish() {
        // AppPublish REPLACES the function set, so a second per-entrypoint create must
        // re-publish the UNION (else the first entrypoint is de-invoked).
        let mut pubd = PublishedFunctions::default();
        let (fns, defs) = pubd.record("add", "fu-1", "de-1");
        assert_eq!(fns.get("add"), Some(&"fu-1".to_string()));
        assert_eq!(defs.get("fu-1"), Some(&"de-1".to_string()));
        // Second entrypoint: both are present (cumulative).
        let (fns, defs) = pubd.record("add_gpu", "fu-2", "de-2");
        assert_eq!(fns.len(), 2);
        assert_eq!(fns.get("add"), Some(&"fu-1".to_string()));
        assert_eq!(fns.get("add_gpu"), Some(&"fu-2".to_string()));
        assert_eq!(defs.len(), 2);
    }

    #[test]
    fn wrapper_src_substitutes_package_and_is_pythonish() {
        let src = run_wrapper_src("example-add", true);
        assert!(!src.contains("{{PACKAGE}}"), "placeholder must be replaced");
        assert!(
            !src.contains("{{CACHE}}"),
            "cache placeholder must be replaced"
        );
        assert!(src.contains(r#"PACKAGE    = "example-add""#));
        // Load-bearing run-path lines.
        assert!(src.contains("def handler(entrypoint, input_json):"));
        assert!(src.contains("cargo"));
        assert!(src.contains("modal_runner"));
        assert!(src.contains("/tmp/in.json"));
    }

    #[test]
    fn wrapper_src_renders_cache_flag_both_ways() {
        // cache=true => CACHE_ON = True, and the unpack/pack machinery is present.
        let on = run_wrapper_src("example-add", true);
        assert!(
            !on.contains("{{CACHE}}"),
            "cache placeholder must be replaced"
        );
        assert!(on.contains("CACHE_ON   = True"));
        assert!(on.contains("def _unpack_cache():"));
        assert!(on.contains("def _pack_cache():"));
        // The wrapper's archive path MUST match the Rust constants (guards against
        // drift between the `/cache` mount + `cache.tar.zst` name and the Python
        // literal the wrapper packs/unpacks).
        let archive_path = format!("{CACHE_MOUNT}/{CACHE_ARCHIVE_NAME}");
        assert_eq!(archive_path, "/cache/cache.tar.zst");
        assert!(
            !on.contains("{{ARCHIVE_ZSTD}}"),
            "archive placeholder replaced"
        );
        assert!(
            !on.contains("{{ARCHIVE_GZIP}}"),
            "archive placeholder replaced"
        );
        assert!(
            on.contains(&format!(r#"_ARCHIVE_ZSTD = "{archive_path}""#)),
            "wrapper archive path must match CACHE_MOUNT/CACHE_ARCHIVE_NAME"
        );
        assert!(
            on.contains(r#"_ARCHIVE_GZIP = "/cache/cache.tar.gz""#),
            "gzip fallback path derived from the zstd archive"
        );

        // cache=false => CACHE_ON = False (no-op-shaped wrapper). The unpack/pack
        // functions still EXIST (rendered verbatim) but both short-circuit on
        // `if not CACHE_ON`, so no volume archive is ever read/written.
        let off = run_wrapper_src("example-add", false);
        assert!(off.contains("CACHE_ON   = False"));
        assert!(!off.contains("CACHE_ON   = True"));
        // The build pipeline (the load-bearing run path) is unchanged either way.
        assert!(off.contains("def handler(entrypoint, input_json):"));
        assert!(off.contains("modal_runner"));
    }

    #[test]
    fn discover_cache_target_default_off_env_flips_on() {
        // Serialized against other env-mutating tests (see `crate::ENV_TEST_LOCK`).
        let _guard = crate::ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("MODAL_RUST_CACHE_TARGET");
        assert!(!discover_cache_target(), "target caching defaults OFF");
        for truthy in ["1", "true", "YES", "On"] {
            std::env::set_var("MODAL_RUST_CACHE_TARGET", truthy);
            assert!(
                discover_cache_target(),
                "MODAL_RUST_CACHE_TARGET={truthy:?} must turn target caching ON"
            );
        }
        std::env::set_var("MODAL_RUST_CACHE_TARGET", "no");
        assert!(!discover_cache_target(), "non-truthy value keeps it OFF");
        std::env::remove_var("MODAL_RUST_CACHE_TARGET");
    }

    #[test]
    fn discover_cache_default_on_env_flips_off() {
        // Serialized against other env-mutating tests (see `crate::ENV_TEST_LOCK`).
        let _guard = crate::ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("MODAL_RUST_NO_CACHE");
        assert!(discover_cache(), "cache defaults ON");
        assert!(
            RemoteConfig::default().cache,
            "RemoteConfig::default().cache defaults ON"
        );

        for truthy in ["1", "true", "YES", "On"] {
            std::env::set_var("MODAL_RUST_NO_CACHE", truthy);
            assert!(
                !discover_cache(),
                "MODAL_RUST_NO_CACHE={truthy:?} must turn cache OFF"
            );
            assert!(
                !RemoteConfig::default().cache,
                "MODAL_RUST_NO_CACHE={truthy:?} flips RemoteConfig::default().cache OFF"
            );
        }
        // A non-truthy value leaves cache ON.
        std::env::set_var("MODAL_RUST_NO_CACHE", "no");
        assert!(discover_cache(), "non-truthy value keeps cache ON");
        std::env::remove_var("MODAL_RUST_NO_CACHE");
    }

    #[test]
    fn remote_config_secrets_volumes_are_settable_non_macro() {
        // Non-macro override: `RemoteConfig`'s public fields let a builder/explicit
        // caller set secrets + user volumes WITHOUT the decorator. Serialized against
        // env-mutating tests (RemoteConfig::default reads MODAL_RUST_*).
        let _guard = crate::ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Struct-update over the env-aware default: a non-macro caller sets ONLY
        // secrets/volumes and keeps every discovered default.
        let cfg = RemoteConfig {
            secrets: vec!["api-creds".to_string()],
            volumes: vec![("/data".to_string(), "my-vol".to_string())],
            ..RemoteConfig::default()
        };
        assert_eq!(cfg.secrets, vec!["api-creds".to_string()]);
        assert_eq!(
            cfg.volumes,
            vec![("/data".to_string(), "my-vol".to_string())]
        );
        // The user volume mount must not be the reserved cargo-cache path.
        assert_ne!(cfg.volumes[0].0, CACHE_MOUNT);
    }

    #[test]
    fn with_detected_package_precedence() {
        // PACKAGE AUTO-DETECT precedence (P2): env override > macro-detected > base.
        // Serialized against env-mutating tests (RemoteConfig::default reads env).
        let _guard = crate::ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("MODAL_RUST_PACKAGE");

        // No env override, a detected package => the detected package is used (this is
        // the headline win: a fresh user crate gets `-p <their-crate>` automatically,
        // never the hardcoded v0 `example-add`). This is exactly what `App::connect`
        // does: `with_detected_package(env_override.as_deref(), package_from_inventory())`.
        let cfg = RemoteConfig::default().with_detected_package(None, Some("my-user-crate"));
        assert_eq!(cfg.package, "my-user-crate");

        // No env override AND no detected package (a manual registry, no `#[function]`)
        // => the base (v0 default) survives.
        let base = RemoteConfig::default().package.clone();
        let cfg = RemoteConfig::default().with_detected_package(None, None);
        assert_eq!(cfg.package, base);

        // MODAL_RUST_PACKAGE still OVERRIDES auto-detect end-to-end: with the env var
        // set, `discover_package()` shapes `default().package` to the override, and
        // `with_detected_package` (called with `env_override = Some(..)`) leaves it —
        // it does NOT clobber the env value with the detected one. This is the
        // `App::connect` call shape with the env var present.
        std::env::set_var("MODAL_RUST_PACKAGE", "forced-pkg");
        let env_override = std::env::var("MODAL_RUST_PACKAGE").ok();
        let cfg = RemoteConfig::default()
            .with_detected_package(env_override.as_deref(), Some("my-user-crate"));
        assert_eq!(
            cfg.package, "forced-pkg",
            "MODAL_RUST_PACKAGE still overrides auto-detect"
        );
        std::env::remove_var("MODAL_RUST_PACKAGE");
    }

    #[test]
    fn parse_envelope_ok_decodes_value() {
        #[derive(serde::Deserialize, PartialEq, Debug)]
        struct Out {
            sum: i64,
        }
        let out: Out = parse_envelope(r#"{"ok":true,"value":{"sum":42}}"#).unwrap();
        assert_eq!(out, Out { sum: 42 });
    }

    #[test]
    fn parse_envelope_decode_kind_maps_like_local() {
        let env = r#"{"ok":false,"error":{"kind":"decode_error","message":"bad in","details":null,"backtrace":""}}"#;
        let err = parse_envelope::<i64>(env).unwrap_err();
        match err {
            Error::Runner(RunnerError::Decode(m)) => assert_eq!(m, "bad in"),
            other => panic!("expected Runner(Decode), got {other:?}"),
        }
    }

    #[test]
    fn parse_envelope_unknown_entrypoint_kind() {
        let env = r#"{"ok":false,"error":{"kind":"unknown_entrypoint","message":"no fn","details":null,"backtrace":""}}"#;
        match parse_envelope::<i64>(env).unwrap_err() {
            Error::Runner(RunnerError::UnknownEntrypoint(m)) => assert_eq!(m, "no fn"),
            other => panic!("expected UnknownEntrypoint, got {other:?}"),
        }
    }

    #[test]
    fn parse_envelope_function_error_carries_details() {
        let env = r#"{"ok":false,"error":{"kind":"function_error","message":"boom","details":{"code":7},"backtrace":""}}"#;
        match parse_envelope::<i64>(env).unwrap_err() {
            Error::Runner(RunnerError::Function { message, details }) => {
                assert_eq!(message, "boom");
                assert_eq!(details, Some(serde_json::json!({"code": 7})));
            }
            other => panic!("expected Function, got {other:?}"),
        }
    }

    #[test]
    fn parse_envelope_function_error_null_details_is_none() {
        let env = r#"{"ok":false,"error":{"kind":"function_error","message":"boom","details":null,"backtrace":""}}"#;
        match parse_envelope::<i64>(env).unwrap_err() {
            Error::Runner(RunnerError::Function { details, .. }) => assert_eq!(details, None),
            other => panic!("expected Function, got {other:?}"),
        }
    }

    #[test]
    fn parse_envelope_encode_kind() {
        let env = r#"{"ok":false,"error":{"kind":"encode_error","message":"enc","details":null,"backtrace":""}}"#;
        match parse_envelope::<i64>(env).unwrap_err() {
            Error::Runner(RunnerError::Encode(m)) => assert_eq!(m, "enc"),
            other => panic!("expected Encode, got {other:?}"),
        }
    }

    #[test]
    fn parse_envelope_panic_kind_carries_backtrace() {
        let env = r#"{"ok":false,"error":{"kind":"panic","message":"oops","details":null,"backtrace":"frame0\nframe1"}}"#;
        match parse_envelope::<i64>(env).unwrap_err() {
            Error::Runner(RunnerError::Panic { message, backtrace }) => {
                assert_eq!(message, "oops");
                assert_eq!(backtrace, "frame0\nframe1");
            }
            other => panic!("expected Panic, got {other:?}"),
        }
    }

    #[test]
    fn parse_envelope_unknown_kind_degrades_to_decode() {
        let env =
            r#"{"ok":false,"error":{"kind":"wat","message":"x","details":null,"backtrace":""}}"#;
        match parse_envelope::<i64>(env).unwrap_err() {
            Error::Runner(RunnerError::Decode(m)) => {
                assert!(m.contains("unrecognized error kind: wat"))
            }
            other => panic!("expected Decode fallback, got {other:?}"),
        }
    }

    #[test]
    fn parse_envelope_malformed_json_is_decode_error() {
        match parse_envelope::<i64>("not json").unwrap_err() {
            Error::Decode(_) => {}
            other => panic!("expected Decode, got {other:?}"),
        }
    }

    #[test]
    fn default_config_has_expected_shape() {
        // The scoping defaults are load-bearing for the source upload: cargo-metadata
        // scoping ON, .modalignore as the highest-precedence ignore file. The old
        // hardcoded ignore list is gone — ignore resolution now layers .modalignore >
        // .gitignore > built-in defaults (so e.g. references/ is excluded via the
        // repo .gitignore, no hardcoded entry needed).
        //
        // Serialized against other env-mutating tests (this body both reads default
        // env AND sets MODAL_RUST_* below); see `crate::ENV_TEST_LOCK`.
        let _guard = crate::ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("MODAL_RUST_PACKAGE");
        std::env::remove_var("MODAL_RUST_BASE_IMAGE");
        std::env::remove_var("MODAL_RUST_INSTALL_RUST");
        std::env::remove_var("MODAL_RUST_NO_CACHE");
        let cfg = RemoteConfig::default();
        assert_eq!(cfg.remote_src, "/src");
        assert_eq!(cfg.base_image, "rust:1-slim");
        assert_eq!(cfg.timeout_secs, 1800);
        assert!(cfg.use_cargo_scoping, "cargo scoping is the default");
        assert_eq!(cfg.modalignore_name, ".modalignore");
        // The CUDA Tier-1 knob defaults OFF, so the default rust:1-slim path stays
        // byte-identical (no rustup, no CUDA env).
        assert!(!cfg.install_rust, "install_rust defaults off");
        // P6: the cargo build cache is ON by default.
        assert!(cfg.cache, "cache defaults ON");
        // User secrets/volumes default EMPTY (wire-identical to before).
        assert!(cfg.secrets.is_empty(), "secrets default empty");
        assert!(cfg.volumes.is_empty(), "volumes default empty");

        // Same test (one process-env mutation site, no cross-test race): the env
        // overrides flip the CUDA Tier-1 knob + base image. `MODAL_RUST_INSTALL_RUST`
        // is parsed truthily; `MODAL_RUST_BASE_IMAGE` points at the CUDA-devel base
        // (parity with MODAL_RUST_SOURCE_DIR / MODAL_RUST_PACKAGE).
        std::env::set_var("MODAL_RUST_INSTALL_RUST", "1");
        std::env::set_var(
            "MODAL_RUST_BASE_IMAGE",
            "nvidia/cuda:12.6.3-devel-ubuntu22.04",
        );
        let cuda = RemoteConfig::default();
        assert!(cuda.install_rust, "truthy MODAL_RUST_INSTALL_RUST => true");
        assert_eq!(cuda.base_image, "nvidia/cuda:12.6.3-devel-ubuntu22.04");
        std::env::remove_var("MODAL_RUST_INSTALL_RUST");
        std::env::remove_var("MODAL_RUST_BASE_IMAGE");
    }
}
