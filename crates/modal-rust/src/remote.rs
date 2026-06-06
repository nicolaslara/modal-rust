//! The RUN-path remote machinery behind [`Function::remote`](crate::Function::remote).
//!
//! This module holds the parts of `.remote()` that are pure (no `&App` borrow) or
//! self-contained: the FILE-mode run wrapper source + its serialized config, the
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

use base64::Engine;
use serde::Serialize;
use std::path::{Path, PathBuf};

use modal_rust_sdk::ModalClient;

use crate::{Error, FunctionOptions, Result, RunnerError};

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

/// The env var carrying the base64-encoded JSON config read by [`WRAPPER_SRC`].
pub(crate) const WRAPPER_CONFIG_ENV: &str = "MODAL_RUST_RUN_CONFIG_JSON_B64";

/// The FILE-mode run wrapper, ported from `workpads/prototype/dev_app.py`'s
/// `run_entrypoint`.
///
/// Modal FILE-mode resolves `import_module("modal_rust_run_wrapper")` +
/// `getattr(mod, "handler")`, then calls `handler(*args, **kwargs)`. The facade
/// invokes with `args = (entrypoint, input_json)`, `kwargs = {}`, so `handler`
/// receives TWO positional args. It builds the mounted crate in the function body,
/// execs `modal_runner`, and RETURNS the one-line JSON envelope string verbatim;
/// the facade parses it ([`parse_envelope`]). Runtime parameters are supplied as
/// base64-encoded JSON in [`WRAPPER_CONFIG_ENV`], not by templating this source.
pub(crate) const WRAPPER_SRC: &str = include_str!("remote/wrapper.py");

#[derive(Serialize)]
struct RunWrapperConfig<'a> {
    package: &'a str,
    cache: bool,
    remote_src: &'a str,
    cache_mount: &'a str,
    cache_archive_name: &'a str,
}

/// The real Python source baked into the image.
pub(crate) fn run_wrapper_src() -> &'static str {
    WRAPPER_SRC
}

/// Dockerfile `ENV` command carrying the run wrapper's base64-encoded JSON config.
///
/// The base64 layer avoids Dockerfile quote escaping entirely: Rust supplies data,
/// Python owns behavior, and the wrapper source stays a normal lintable/testable
/// Python file.
pub(crate) fn run_wrapper_config_env(package: &str, cache: bool, remote_src: &str) -> String {
    let json = serde_json::to_string(&RunWrapperConfig {
        package,
        cache,
        remote_src,
        cache_mount: CACHE_MOUNT,
        cache_archive_name: CACHE_ARCHIVE_NAME,
    })
    .expect("run wrapper config serializes");
    let encoded = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());
    format!("ENV {WRAPPER_CONFIG_ENV}={encoded}")
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
    /// Owned per-function Modal options after the inventory/manifest boundary.
    /// `timeout_secs` overrides this path's [`timeout_secs`](RemoteConfig::timeout_secs);
    /// `cache` has already been folded into [`cache`](RemoteConfig::cache) by
    /// `App::resolve_function` so `cache=None` can defer to the run-path default.
    /// Secrets and user volumes are resolved to ids immediately before
    /// `FunctionCreate`.
    pub options: FunctionOptions,
}

impl RemoteConfig {
    /// Fill in [`package`](RemoteConfig::package) from the macro-captured inventory
    /// package when the user has NOT set `MODAL_RUST_PACKAGE`.
    ///
    /// Precedence (highest → lowest): `MODAL_RUST_PACKAGE` (`env_override`) → the
    /// macro-captured `env!("CARGO_PKG_NAME")` (`detected`, from
    /// [`crate::package_from_inventory`]) → the existing value (the v0
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
            install_rust: discover_install_rust(),
            cache: discover_cache(),
            options: FunctionOptions::default(),
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
pub(crate) fn discover_cache_target() -> bool {
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
    published: &mut crate::control_plane::Published,
) -> Result<String> {
    use crate::control_plane::{
        provision, Entrypoint, LiveControlPlane, ProvisionInputs, SourceInputs, RUN_BOUNDARY,
    };

    // The RUN path provisions exactly ONE Modal function per entrypoint (the caller
    // memoizes per entrypoint + threads the cumulative publish union via `published`).
    // It carries this entrypoint's effective config; the object TAG = the entrypoint
    // (unique per app, its own gpu/timeout/cache/secrets/volumes), while the
    // in-container callable stays the shared dispatch "handler" (decoupled in the SDK
    // FunctionCreate builder). The whole AppCreate(at connect)→cache→secrets/volumes→
    // mounts→ImageGetOrCreate→Precreate→Create→ephemeral AppPublish sequence lives in
    // the ONE `provision()` driver; this only assembles the inputs + the RUN boundary.
    //
    // `cargo build` runs IN THE FUNCTION BODY at execution time (the RUN boundary) —
    // the run image carries NO cargo line; that divergence is isolated to the boundary
    // + `control_plane::build_image_spec`.
    let timeout = config.options.timeout_secs.unwrap_or(config.timeout_secs);
    let entrypoints = [Entrypoint {
        name: entrypoint.to_string(),
        options: config.options.clone(),
        timeout_secs: timeout,
    }];
    let inputs = ProvisionInputs {
        app_name,
        app_id: Some(app_id),
        source: SourceInputs {
            local_root: &config.local_root,
            package: &config.package,
            use_cargo_scoping: config.use_cargo_scoping,
            modalignore_name: &config.modalignore_name,
            remote_src: &config.remote_src,
        },
        base_image: &config.base_image,
        install_rust: config.install_rust,
        // `cache` flows from the per-function override (`options.cache`), falling back to
        // the run-level default (`config.cache`) — symmetric with `timeout` above. The
        // flat `config.cache` is the default only; it is never overwritten per entrypoint.
        cache: config.options.cache.unwrap_or(config.cache),
        entrypoints: &entrypoints,
    };

    let mut cp = LiveControlPlane { client };
    // The RUN path needs only the threaded `published` union (its function id) — the
    // returned image id / publish url are DEPLOY-facing, so discard them here.
    let _ = provision(&mut cp, &inputs, &RUN_BOUNDARY, published).await?;

    // Invoke via the FunctionCreate `function_id` DIRECTLY — NOT `from_name`.
    // `from_name`/`FunctionGet` is the DEPLOYED lookup; an EPHEMERAL app is not
    // name-resolvable in the environment (live-verified 2026-06-04: from_name on
    // the ephemeral app failed "App '...' not found in environment 'main'"). Modal
    // Python's ephemeral `app.run()` likewise invokes by `object_id`, never by name.
    let object_tag = sanitize_object_tag(entrypoint);
    published
        .function_ids
        .get(&object_tag)
        .cloned()
        .ok_or_else(|| {
            Error::config(format!(
                "provision did not yield a function id for {entrypoint:?}"
            ))
        })
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
    fn wrapper_src_is_included_python_file_with_no_templates() {
        let src = run_wrapper_src();
        assert!(
            !src.contains("{{PACKAGE}}"),
            "wrapper source must not template package"
        );
        assert!(
            !src.contains("{{CACHE}}"),
            "wrapper source must not template cache"
        );
        assert!(
            !src.contains("{{ARCHIVE_ZSTD}}"),
            "wrapper source must not template archive paths"
        );
        assert!(
            !src.contains("{{ARCHIVE_GZIP}}"),
            "wrapper source must not template archive paths"
        );
        // Load-bearing run-path lines.
        assert!(src.contains(WRAPPER_CONFIG_ENV));
        assert!(src.contains("def handler(entrypoint, input_json):"));
        assert!(src.contains("cargo"));
        assert!(src.contains("modal_runner"));
        assert!(src.contains("/tmp/in.json"));
    }

    #[test]
    fn wrapper_config_env_renders_base64_json() {
        let env = run_wrapper_config_env("example-add", true, "/mounted-src");
        let prefix = format!("ENV {WRAPPER_CONFIG_ENV}=");
        let encoded = env
            .strip_prefix(&prefix)
            .expect("config env command has expected prefix");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("config env is base64");
        let value: serde_json::Value =
            serde_json::from_slice(&decoded).expect("config env is JSON");

        assert_eq!(value["package"], "example-add");
        assert_eq!(value["cache"], true);
        assert_eq!(value["remote_src"], "/mounted-src");
        assert_eq!(value["cache_mount"], CACHE_MOUNT);
        assert_eq!(value["cache_archive_name"], CACHE_ARCHIVE_NAME);

        // The config's archive fields MUST match the Rust constants (guards against
        // drift between the `/cache` mount + `cache.tar.zst` name and the Python
        // wrapper's runtime archive path derivation).
        let archive_path = format!("{CACHE_MOUNT}/{CACHE_ARCHIVE_NAME}");
        assert_eq!(archive_path, "/cache/cache.tar.zst");
    }

    #[test]
    fn wrapper_python_tests_pass() {
        let python = std::env::var("PYTHON").unwrap_or_else(|_| "python3".to_string());
        let test = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/remote/wrapper_test.py");
        match std::process::Command::new(&python).arg(&test).status() {
            Ok(status) => assert!(
                status.success(),
                "wrapper Python tests failed with status {status}"
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("skipping wrapper Python tests: {python} not found");
            }
            Err(e) => panic!("failed to run wrapper Python tests with {python}: {e}"),
        }
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
        // Non-macro override: `RemoteConfig.options` lets a builder/explicit caller
        // set secrets + user volumes WITHOUT the decorator. Serialized against
        // env-mutating tests (RemoteConfig::default reads MODAL_RUST_*).
        let _guard = crate::ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Struct-update over the env-aware default: a non-macro caller sets ONLY the
        // owned function options and keeps every discovered path default.
        let cfg = RemoteConfig {
            options: FunctionOptions {
                secrets: vec!["api-creds".to_string()],
                volumes: vec![("/data".to_string(), "my-vol".to_string())],
                ..FunctionOptions::default()
            },
            ..RemoteConfig::default()
        };
        assert_eq!(cfg.options.secrets, vec!["api-creds".to_string()]);
        assert_eq!(
            cfg.options.volumes,
            vec![("/data".to_string(), "my-vol".to_string())]
        );
        // The user volume mount must not be the reserved cargo-cache path.
        assert_ne!(cfg.options.volumes[0].0, CACHE_MOUNT);
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
        assert!(cfg.options.secrets.is_empty(), "secrets default empty");
        assert!(cfg.options.volumes.is_empty(), "volumes default empty");

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
