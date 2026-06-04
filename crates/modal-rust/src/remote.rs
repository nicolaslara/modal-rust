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
/// Fixed callable within the wrapper module. ONE Modal function serves EVERY
/// entrypoint in the crate; the user entrypoint is a per-call invoke arg.
pub(crate) const WRAPPER_CALLABLE: &str = "handler";
/// Where the uploaded source mount lands inside the container.
pub(crate) const REMOTE_SRC: &str = "/src";
/// Rust base image major version tag (`rust:{ver}-slim`).
pub(crate) const RUST_VER: &str = "1";
/// In-body `cargo build` needs far longer than the SDK's 300s invoke default.
pub(crate) const REMOTE_TIMEOUT_SECS: u32 = 1800;

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
    build_dir = _build_dir()
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
"#;

/// Substitute `{{PACKAGE}}` into [`WRAPPER_SRC`]. `package` is a cargo package name
/// (crate-name-shaped: `[A-Za-z0-9_-]`); it is NOT shell-quoted because the source
/// is base64-baked into the Dockerfile.
pub(crate) fn run_wrapper_src(package: &str) -> String {
    WRAPPER_SRC.replace("{{PACKAGE}}", package)
}

/// All knobs for the RUN path. One struct, no per-project file.
#[derive(Debug, Clone)]
pub struct RemoteConfig {
    /// Directory uploaded as the source mount (defaults to the cargo workspace
    /// root; override with `MODAL_RUST_SOURCE_DIR`).
    pub local_root: PathBuf,
    /// Cargo package owning the entrypoints (`cargo -p <package>`). The
    /// `modal_runner` bin name is shared across workspace members, so this
    /// disambiguates. Override with `MODAL_RUST_PACKAGE`.
    pub package: String,
    /// Where the source mount lands in-container.
    pub remote_src: String,
    /// Ignore patterns for the source-dir walk (build artifacts, VCS).
    pub ignore: Vec<String>,
    /// Base registry tag for the run image.
    pub base_image: String,
    /// Function timeout (seconds) — covers the in-body cargo build.
    pub timeout_secs: u32,
}

impl Default for RemoteConfig {
    fn default() -> Self {
        RemoteConfig {
            local_root: discover_local_root(),
            package: discover_package(),
            remote_src: REMOTE_SRC.to_string(),
            ignore: vec![
                "target".to_string(),      // build artifacts (already pruned early)
                ".git".to_string(),        // VCS
                ".modal-rust".to_string(), // generated scratch / shims
                "references".to_string(), // FIX: vendored modal-rs + modal-client clones (~14 MB, gitignored)
                "workpads".to_string(),   // planning docs — not build input
                ".github".to_string(),    // CI config — not build input
                ".claude".to_string(),    // agent config — not build input
                ".cursor".to_string(),    // editor config
                ".opencode".to_string(),  // agent config
                "tmp".to_string(),        // .gitignore scratch
                ".research".to_string(),  // .gitignore scratch
                "**/*.rlib".to_string(),  // stray rust libs
            ],
            base_image: format!("rust:{RUST_VER}-slim"),
            timeout_secs: REMOTE_TIMEOUT_SECS,
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
/// default `"example-add"`. Registry-derived package selection is a later milestone.
fn discover_package() -> String {
    std::env::var("MODAL_RUST_PACKAGE").unwrap_or_else(|_| "example-add".to_string())
}

/// Ensure the run function exists on Modal and return its invokable `function_id`.
///
/// Runs the full create sequence (client mount, uploaded source mount, run image,
/// precreate, FunctionCreate FILE, **EPHEMERAL** AppPublish, from_name).
/// Idempotent at the Modal level (get-or-create semantics); callers memoize the
/// result per App so it runs at most once per process.
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
    config: &RemoteConfig,
) -> Result<String> {
    // 2. Client mount (modal source importable in the FILE-mode container).
    let client_mount_id = client.client_mount_id(None).await?;

    // 3. Source mount (UPLOAD the user's crate; `cargo build` reads it at /src).
    let ignore: Vec<&str> = config.ignore.iter().map(String::as_str).collect();
    let source_mount_id = client
        .mount_local_dir(&config.local_root, &config.remote_src, &ignore, None)
        .await?;

    // 4. Run image: rust base + python3/pip + modal deps + the baked wrapper.
    //
    // `python-is-python3` is REQUIRED, not cosmetic: Modal's container entrypoint
    // (dumb-init) execs bare `python`, but `rust:slim` + apt `python3` provides
    // only `python3` — so without the `/usr/bin/python -> python3` symlink the
    // container crash-loops at startup with "[dumb-init] python: No such file or
    // directory" (live-verified 2026-06-04) and the function never produces output.
    let spec = ImageSpec::from_registry(config.base_image.clone())
        .with_apt(&["python3", "python3-pip", "python-is-python3"])
        .with_pip_install_modal()
        .with_wrapper_module(WRAPPER_MODULE, run_wrapper_src(&config.package))
        .with_command("ENV RUST_BACKTRACE=1")
        .with_command("ENTRYPOINT []");
    let image_id = client.image_get_or_create(app_id, &spec).await?;

    // 5. Precreate the function (name = the wrapper callable, "handler").
    let precreate_id = client.function_precreate(app_id, WRAPPER_CALLABLE).await?;

    // 6. FunctionCreate (FILE mode): both mounts attach via Function.mount_ids.
    let fn_spec = FunctionSpec::new(WRAPPER_MODULE, WRAPPER_CALLABLE, &image_id)
        .with_mount_ids(vec![client_mount_id, source_mount_id])
        .with_timeout_secs(config.timeout_secs);
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
    let mut function_ids = HashMap::new();
    function_ids.insert(WRAPPER_CALLABLE.to_string(), created.function_id.clone());
    let mut definition_ids = HashMap::new();
    if !created.definition_id.is_empty() {
        definition_ids.insert(created.function_id.clone(), created.definition_id.clone());
    }
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
    fn wrapper_src_substitutes_package_and_is_pythonish() {
        let src = run_wrapper_src("example-add");
        assert!(!src.contains("{{PACKAGE}}"), "placeholder must be replaced");
        assert!(src.contains(r#"PACKAGE    = "example-add""#));
        // Load-bearing run-path lines.
        assert!(src.contains("def handler(entrypoint, input_json):"));
        assert!(src.contains("cargo"));
        assert!(src.contains("modal_runner"));
        assert!(src.contains("/tmp/in.json"));
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
        // package default and ignore set are load-bearing (the source-mount walk).
        std::env::remove_var("MODAL_RUST_PACKAGE");
        let cfg = RemoteConfig::default();
        assert_eq!(cfg.remote_src, "/src");
        assert_eq!(cfg.base_image, "rust:1-slim");
        assert_eq!(cfg.timeout_secs, 1800);
        assert!(cfg.ignore.iter().any(|p| p == "target"));
        assert!(cfg.ignore.iter().any(|p| p == "**/*.rlib"));
        // The load-bearing upload fix: references/ (the 14 MB vendored clones) must
        // be excluded so .remote() never uploads them.
        assert!(
            cfg.ignore.iter().any(|p| p == "references"),
            "references/ MUST be in the default ignore list"
        );
        // Other non-source dirs are belt-and-suspenders excluded too.
        for seg in ["workpads", ".github", ".claude"] {
            assert!(
                cfg.ignore.iter().any(|p| p == seg),
                "{seg} should be ignored"
            );
        }
    }
}
