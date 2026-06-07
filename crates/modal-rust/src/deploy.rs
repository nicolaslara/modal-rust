//! The DEPLOY-path machinery behind [`App::deploy`](crate::App::deploy) /
//! [`App::call`](crate::App::call).
//!
//! This is the deploy-side counterpart of [`crate::remote`]. It proves the OTHER
//! half of the build boundary (`workpads/architecture/boundaries.md` §4/§5):
//!
//! ## The build boundary (DEPLOY path)
//!
//! DEPLOY = build at IMAGE-BUILD time. The source crate is COPIED into an image
//! LAYER via the image build CONTEXT (`Image.context_mount_id` + a `COPY` step),
//! and `cargo build --release` runs DURING the image build — NEVER in the function
//! body, NEVER at call time. The freshly built `modal_runner` is `cp`'d to a fixed
//! path (`/app/modal_runner`) and baked into the image.
//!
//! Deployed-runtime invariant (the hard, non-negotiable one): the deployed
//! function body execs ONLY the prebuilt `/app/modal_runner`. It mounts NO source
//! (only the modal client mount, so `modal` is importable), NEVER invokes `cargo`,
//! and `call` performs NO upload and NO build. cargo runs ONLY during the image
//! build.
//!
//! DEPLOY is the ONLY path that uses persistent `AppPublish`; the RUN path
//! ([`crate::remote`]) is ephemeral.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use modal_rust_sdk::ModalClient;

use crate::remote::RemoteConfig;
use crate::{Error, FunctionOptions, Result};

/// Fixed importable module name for the baked DEPLOY wrapper
/// (`/root/modal_rust_deploy_wrapper.py`). DISTINCT from the run wrapper module so
/// the two never collide in a container.
pub(crate) const DEPLOY_WRAPPER_MODULE: &str = "modal_rust_deploy_wrapper";
/// Fixed callable within the deploy wrapper module.
pub(crate) const DEPLOY_WRAPPER_CALLABLE: &str = "handler";
/// Where the source context lands inside the image (the `COPY . /` drops the
/// `/app/src`-prefixed context tree at root). cargo builds here AT image-build time.
pub(crate) const DEPLOY_SRC: &str = "/app/src";
/// The fixed path the freshly built `modal_runner` is baked to.
pub(crate) const DEPLOY_RUNNER: &str = "/app/modal_runner";
/// The SINGLE canonical default STABLE deploy app name (re-deploys REPLACE under
/// this name, so re-runs never accumulate). Override with `MODAL_RUST_DEPLOY_APP`.
///
/// This is the ONE source of truth for the default deploy app: it backs
/// [`DeployConfig::default`] AND the `modal-rust` CLI's `--app` default (which
/// re-exports this constant as `modal_rust::DEFAULT_DEPLOY_APP`), so the library and
/// CLI defaults cannot drift apart.
pub const DEFAULT_DEPLOY_APP: &str = "modal-rust-add-deploy";
/// Output-poll deadline for a deployed-function call. No in-body build at call
/// time (the binary is prebuilt), so the SDK default suffices.
pub(crate) const DEPLOY_CALL_DEADLINE: Duration = Duration::from_secs(600);

/// The FILE-mode DEPLOY wrapper, ported from `workpads/prototype/deploy_app.py`'s
/// `call_entrypoint`. Unlike the run wrapper, it has NO `{{PACKAGE}}` placeholder
/// (the package was already chosen at image-build time), so it is a plain
/// `&'static str`.
///
/// Deployed-runtime invariant in code: it execs ONLY the prebuilt
/// `/app/modal_runner`. It imports only `subprocess`/`sys` (no `os`/`shutil`), has
/// NO `cargo`, NO `/src`, NO `CARGO_*` — it never builds and never mounts source.
///
/// Modal FILE-mode resolves `import_module("modal_rust_deploy_wrapper")` +
/// `getattr(mod, "handler")`, then calls `handler(*args, **kwargs)`. The facade
/// invokes with `args = (entrypoint, input_json)`, `kwargs = {}`, so `handler`
/// receives TWO positional args and RETURNS the one-line JSON envelope string
/// verbatim — so [`crate::remote::parse_envelope`] is REUSED unchanged.
pub(crate) const DEPLOY_WRAPPER_SRC: &str = r#""""modal-rust FILE-mode DEPLOY wrapper (ports deploy_app.py call_entrypoint).

Baked to /root/modal_rust_deploy_wrapper.py. Deployed-runtime invariant: this body
NEVER builds and NEVER mounts source. It execs ONLY the prebuilt /app/modal_runner
baked at IMAGE-BUILD time, and RETURNS the one-line JSON envelope verbatim (the
facade parses it).
"""
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
"#;

/// All knobs for the DEPLOY path. Mirrors [`RemoteConfig`] but adds the STABLE
/// `app_name` and drops the runtime source-mount knobs (the deploy image COPYs the
/// source into a layer at the fixed [`DEPLOY_SRC`]).
///
/// The deploy context upload uses the SAME scoping + ignore resolution as the RUN
/// path (see [`RemoteConfig`]): cargo-metadata scoping to the target package's
/// dependency closure + the workspace `Cargo.toml`/`Cargo.lock`, pruned by
/// `.modalignore` > `.gitignore` > built-in defaults. Non-source assets belong in
/// **Modal Volumes**, not the deploy context.
#[derive(Debug, Clone)]
pub struct DeployConfig {
    /// STABLE deploy app name; re-deploys REPLACE under this name (so re-runs do
    /// NOT accumulate). Default [`DEFAULT_DEPLOY_APP`], override with
    /// `MODAL_RUST_DEPLOY_APP`.
    pub app_name: String,
    /// Directory uploaded as the image build CONTEXT (defaults to the cargo
    /// workspace root; override with `MODAL_RUST_SOURCE_DIR`). Also the workspace
    /// root for cargo-metadata scoping and ignore-file resolution.
    pub local_root: PathBuf,
    /// Cargo package owning the entrypoints (`cargo -p <package>`). Also the
    /// cargo-metadata scoping target. Override with `MODAL_RUST_PACKAGE`.
    pub package: String,
    /// Whether to scope the upload to the target package's cargo dependency closure
    /// via `cargo metadata` (default `true`). `false` forces the whole-`local_root`
    /// upload (still pruned by the resolved ignore files).
    pub use_cargo_scoping: bool,
    /// Highest-precedence ignore filename, read from the workspace root (default
    /// `.modalignore`). Falls through to `.gitignore` then the built-in defaults.
    pub modalignore_name: String,
    /// Base registry tag for the deploy image.
    pub base_image: String,
    /// Function timeout (seconds). No in-body build, so a modest default is fine.
    pub timeout_secs: u32,
    /// Install the Rust toolchain (rustup) + the CUDA build/run env into the deploy
    /// BASE layer. Set when [`base_image`](DeployConfig::base_image) is a non-Rust
    /// base (e.g. a `nvidia/cuda:<ver>-devel` Tier-1 base; boundaries.md §9) so the
    /// TOP layer's image-build-time `cargo build` inherits a toolchain + the CUDA
    /// headers. Default `false`. Inherited from [`RemoteConfig::default()`] in
    /// [`for_app`](DeployConfig::for_app), so the `MODAL_RUST_INSTALL_RUST` env default
    /// flows through automatically (parity with `base_image`).
    pub install_rust: bool,
    /// Ordered image-builder steps ([`ImageStep`](crate::ImageStep): `apt_install` /
    /// `pip_install` / `run_commands`, PARITY.md §3) rendered into the deploy BASE
    /// layer's dockerfile, in chain order — so the TOP layer's image-build-time
    /// `cargo build` AND the deployed runtime inherit the installed deps. BUILD-path
    /// config (like [`base_image`](DeployConfig::base_image)), not decorator config.
    /// Default empty ⇒ byte-identical default path.
    pub image_steps: Vec<crate::ImageStep>,
    /// Owned per-function Modal options used by the manual/no-decorator fallback
    /// function. Decorated entrypoints carry their own [`FunctionOptions`] in
    /// [`DeployEntrypoint`].
    pub options: FunctionOptions,
}

impl DeployConfig {
    /// Build a [`DeployConfig`] with the given STABLE app name, reusing the proven
    /// [`RemoteConfig`] defaults for `local_root` / `package` / scoping / ignore /
    /// `base_image` (so the deploy context upload matches the RUN-path upload).
    pub fn for_app(app_name: impl Into<String>) -> Self {
        let base = RemoteConfig::default();
        DeployConfig {
            app_name: app_name.into(),
            local_root: base.local_root,
            package: base.package,
            use_cargo_scoping: base.use_cargo_scoping,
            modalignore_name: base.modalignore_name,
            base_image: base.base_image,
            timeout_secs: 300,
            install_rust: base.install_rust,
            image_steps: base.image_steps,
            options: FunctionOptions::default(),
        }
    }
}

impl Default for DeployConfig {
    fn default() -> Self {
        DeployConfig::for_app(
            std::env::var("MODAL_RUST_DEPLOY_APP")
                .unwrap_or_else(|_| DEFAULT_DEPLOY_APP.to_string()),
        )
    }
}

/// A successfully-deployed app: the STABLE name plus the resolved deploy metadata.
/// Returned by [`App::deploy`](crate::App::deploy); pass its [`DeployedApp::name`]
/// to [`App::call`](crate::App::call) (or use
/// [`App::call_deployed`](crate::App::call_deployed) directly).
#[derive(Debug, Clone)]
pub struct DeployedApp {
    /// STABLE deploy app name (the same name `call` resolves via `from_name`).
    pub name: String,
    /// Invokable `function_id` of the deployed wrapper function.
    pub function_id: String,
    /// Built deploy `image_id` (the one carrying the baked `/app/modal_runner`).
    pub image_id: String,
    /// Deployed app URL (may be empty depending on the server response).
    pub url: String,
}

/// One entrypoint to deploy: its NAME (the Modal object tag) plus its effective
/// per-entrypoint deploy config (gpu/timeout/secrets/volumes). Built by
/// [`App::deploy_with`](crate::App::deploy_with) from each decorated entrypoint's
/// [`FunctionConfig`]. The shared deploy IMAGE bakes the one `modal_runner` that
/// handles ALL entrypoints (it dispatches by the per-call entrypoint arg), so each
/// of these becomes a DISTINCT Modal function over the SAME image, carrying its OWN
/// config — divergent gpu/timeout/secrets/volumes coexist instead of being rejected.
#[derive(Debug, Clone)]
pub(crate) struct DeployEntrypoint {
    /// The entrypoint name = the Modal object TAG (`from_name` resolves it at call).
    pub name: String,
    /// Owned per-entrypoint Modal options.
    pub options: FunctionOptions,
}

/// Deploy (persistently) ONE Modal function PER ENTRYPOINT under the STABLE app name
/// and return a [`DeployedApp`]. PERSISTENT: this is the ONLY path that uses
/// `AppPublish` into a named, get-or-created app.
///
/// The deploy IMAGE is built ONCE (it bakes the single `modal_runner` that handles
/// every entrypoint by dispatch); then each entrypoint in `entrypoints` is created
/// as a DISTINCT Modal function (object tag = the entrypoint), carrying its OWN
/// gpu/timeout/secrets/volumes. A single persistent `AppPublish` carries the UNION
/// so `call(app, entrypoint)` resolves the RIGHT function. When `entrypoints` is
/// empty (the manual/no-decorator path) a single default function is published under
/// the wrapper callable, byte-identical to the pre-fix single-function deploy.
///
/// Reuses the proven ops verbatim; the structural difference vs RUN is that the
/// source rides the image build CONTEXT (so cargo builds at image-build time) and
/// the function attaches ONLY the client mount (the binary is baked in the layer —
/// NO runtime source mount).
pub(crate) async fn deploy_function(
    client: &mut ModalClient,
    config: &DeployConfig,
    entrypoints: &[DeployEntrypoint],
) -> Result<DeployedApp> {
    use crate::control_plane::{
        provision, Entrypoint, LiveControlPlane, ProvisionInputs, Published, SourceInputs,
        DEPLOY_BOUNDARY,
    };

    // ONE Modal function PER ENTRYPOINT over the SHARED image: the deployed
    // `modal_runner` handles every entrypoint by dispatch, so each entrypoint is its
    // OWN Modal function (object tag = the entrypoint) carrying its OWN config. The
    // manual/no-decorator path (`entrypoints` empty) falls back to ONE function under
    // the wrapper callable — byte-identical to the pre-fix single-function deploy.
    let plan: Vec<Entrypoint> = if entrypoints.is_empty() {
        vec![Entrypoint {
            name: DEPLOY_WRAPPER_CALLABLE.to_string(),
            options: config.options.clone(),
            timeout_secs: config.timeout_secs,
        }]
    } else {
        entrypoints
            .iter()
            .map(|ep| Entrypoint {
                name: ep.name.clone(),
                options: ep.options.clone(),
                timeout_secs: ep.options.timeout_secs.unwrap_or(config.timeout_secs),
            })
            .collect()
    };
    let first_tag = crate::remote::sanitize_object_tag(&plan[0].name);

    // The whole MountGetOrCreate→persistent AppGetOrCreate→TWO image layers (the top
    // layer's `cargo build --release` at image-build time)→per-entrypoint Precreate+
    // Create (CLIENT mount ONLY)→deployed AppPublish sequence lives in the ONE
    // `provision()` driver. The DEPLOY divergence (source COPIED into the image build
    // context, cargo in a Dockerfile RUN layer, persistent app/publish) is isolated to
    // the boundary + `control_plane::build_image_spec` / `build_deploy_top_layer_spec`.
    let inputs = ProvisionInputs {
        app_name: &config.app_name,
        app_id: None, // resolved via AppGetOrCreate inside provision (persistent).
        source: SourceInputs {
            local_root: &config.local_root,
            package: &config.package,
            use_cargo_scoping: config.use_cargo_scoping,
            modalignore_name: &config.modalignore_name,
            remote_src: DEPLOY_SRC,
        },
        base_image: &config.base_image,
        install_rust: config.install_rust,
        image_steps: &config.image_steps,
        cache: false, // DEPLOY builds at image-build time — no run-path cargo cache.
        entrypoints: &plan,
    };
    let mut published = Published::default();
    let mut cp = LiveControlPlane { client };
    let provisioned = provision(&mut cp, &inputs, &DEPLOY_BOUNDARY, &mut published).await?;

    // Resolve ONE invokable function_id (the first entrypoint) to prove from_name
    // works post-deploy. `call(app, entrypoint)` re-resolves the right one by tag.
    let function_id = client
        .function_from_name(&config.app_name, &first_tag, None)
        .await?;

    Ok(DeployedApp {
        name: config.app_name.clone(),
        function_id,
        image_id: provisioned.image_id,
        url: provisioned.publish_url,
    })
}

/// Invoke a DEPLOYED function: resolve `from_name`, invoke with
/// `(entrypoint, input_json)`, and return the runner's one-line JSON envelope.
///
/// NO upload, NO image build, NO `app_publish` — that absence IS the deploy
/// invariant: the binary was prebuilt at deploy/image-build time, so `call` only
/// resolves + invokes.
pub(crate) async fn call_function(
    client: &mut ModalClient,
    app_name: &str,
    entrypoint: &str,
    input_json: String,
) -> Result<String> {
    // Resolve the PER-ENTRYPOINT deployed function by its object tag (the entrypoint),
    // NOT the shared wrapper callable — each entrypoint is its own Modal function now.
    let object_tag = crate::remote::sanitize_object_tag(entrypoint);
    let function_id = client
        .function_from_name(app_name, &object_tag, None)
        .await?;
    let empty_kwargs: HashMap<String, ()> = HashMap::new();
    let envelope: String = client
        .invoke_cbor_with_deadline(
            &function_id,
            &(entrypoint, input_json),
            &empty_kwargs,
            DEPLOY_CALL_DEADLINE,
        )
        .await?;
    Ok(envelope)
}

impl DeployedApp {
    /// Call this deployed app's `entrypoint` with `input` and return the typed
    /// output, with the SAME semantics as [`crate::Function::local`] /
    /// [`crate::Function::remote`]. Convenience for
    /// [`App::call`](crate::App::call) when you already hold the [`DeployedApp`].
    ///
    /// `client` is the live control-plane handle (the deployed function is resolved
    /// by name, so any connected [`ModalClient`] works). NO upload, NO build.
    pub(crate) async fn call_with<In, Out>(
        &self,
        client: &mut ModalClient,
        entrypoint: &str,
        input: In,
    ) -> Result<Out>
    where
        In: serde::Serialize,
        Out: serde::de::DeserializeOwned,
    {
        let input_json = serde_json::to_string(&input).map_err(Error::Encode)?;
        let envelope = call_function(client, &self.name, entrypoint, input_json).await?;
        crate::remote::parse_envelope::<Out>(&envelope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deploy_wrapper_src_is_pythonish_and_runtime_pure() {
        let src = DEPLOY_WRAPPER_SRC;
        // Load-bearing deployed-runtime lines: execs the prebuilt binary by path.
        assert!(src.contains("def handler(entrypoint, input_json):"));
        assert!(src.contains("/app/modal_runner"));
        assert!(src.contains("--input-file"));
        // Deployed-runtime invariant: NEVER builds, NEVER mounts source.
        assert!(!src.contains("cargo"), "deploy wrapper must not run cargo");
        assert!(!src.contains("/src"), "deploy wrapper must not touch /src");
        assert!(
            !src.contains("CARGO_"),
            "deploy wrapper must not set CARGO_* env"
        );
        // No {{PACKAGE}} placeholder — the package was chosen at image-build time.
        assert!(!src.contains("{{PACKAGE}}"));
    }

    #[test]
    fn deploy_config_default_has_stable_app_name() {
        // Serialized against other env-mutating tests (reads default MODAL_RUST_*);
        // see `crate::ENV_TEST_LOCK`.
        let _guard = crate::ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("MODAL_RUST_DEPLOY_APP");
        std::env::remove_var("MODAL_RUST_BASE_IMAGE");
        std::env::remove_var("MODAL_RUST_INSTALL_RUST");
        let cfg = DeployConfig::default();
        assert_eq!(cfg.app_name, "modal-rust-add-deploy");
        assert_eq!(cfg.base_image, "rust:1-slim");
        // The deploy context upload reuses the RUN-path scoping/ignore defaults.
        assert!(cfg.use_cargo_scoping, "cargo scoping is the default");
        assert_eq!(cfg.modalignore_name, ".modalignore");
        // install_rust is inherited from RemoteConfig::default() (env-aware) and
        // defaults OFF, so the default deploy path stays byte-identical.
        assert!(!cfg.install_rust, "install_rust defaults off");
        // User secrets/volumes default EMPTY (wire-identical to before).
        assert!(cfg.options.secrets.is_empty(), "secrets default empty");
        assert!(cfg.options.volumes.is_empty(), "volumes default empty");
    }

    #[test]
    fn deploy_config_secrets_volumes_are_settable_non_macro() {
        // Non-macro override: `DeployConfig.options` lets a builder/explicit caller
        // set secrets + user volumes WITHOUT the decorator.
        let _guard = crate::ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Struct-update over the env-aware config: a non-macro caller sets ONLY the
        // owned function options and keeps every other field.
        let cfg = DeployConfig {
            options: FunctionOptions {
                secrets: vec!["api-creds".to_string()],
                volumes: vec![("/models".to_string(), "weights".to_string())],
                ..FunctionOptions::default()
            },
            ..DeployConfig::for_app("my-app")
        };
        assert_eq!(cfg.options.secrets, vec!["api-creds".to_string()]);
        assert_eq!(
            cfg.options.volumes,
            vec![("/models".to_string(), "weights".to_string())]
        );
    }
}
