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

use modal_rust_sdk::{FunctionSpec, ImageSpec, ModalClient};

use crate::remote::RemoteConfig;
use crate::{Error, Result};

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
/// Default STABLE deploy app name (re-deploys REPLACE under this name, so re-runs
/// never accumulate). Override with `MODAL_RUST_DEPLOY_APP`.
pub(crate) const DEFAULT_DEPLOY_APP: &str = "modal-rust-add-deploy";
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
#[derive(Debug, Clone)]
pub struct DeployConfig {
    /// STABLE deploy app name; re-deploys REPLACE under this name (so re-runs do
    /// NOT accumulate). Default [`DEFAULT_DEPLOY_APP`], override with
    /// `MODAL_RUST_DEPLOY_APP`.
    pub app_name: String,
    /// Directory uploaded as the image build CONTEXT (defaults to the cargo
    /// workspace root; override with `MODAL_RUST_SOURCE_DIR`).
    pub local_root: PathBuf,
    /// Cargo package owning the entrypoints (`cargo -p <package>`). Override with
    /// `MODAL_RUST_PACKAGE`.
    pub package: String,
    /// Ignore patterns for the source-dir walk (build artifacts, VCS, vendored
    /// reference clones).
    pub ignore: Vec<String>,
    /// Base registry tag for the deploy image.
    pub base_image: String,
    /// Function timeout (seconds). No in-body build, so a modest default is fine.
    pub timeout_secs: u32,
}

impl DeployConfig {
    /// Build a [`DeployConfig`] with the given STABLE app name, reusing the proven
    /// [`RemoteConfig`] defaults for `local_root` / `package` / `ignore` /
    /// `base_image` (so the deploy context upload matches the RUN-path upload).
    pub fn for_app(app_name: impl Into<String>) -> Self {
        let base = RemoteConfig::default();
        DeployConfig {
            app_name: app_name.into(),
            local_root: base.local_root,
            package: base.package,
            ignore: base.ignore,
            base_image: base.base_image,
            timeout_secs: 300,
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

/// Render the deploy [`ImageSpec`]: rust base + python3/pip + the baked deploy
/// wrapper, then the source as the build CONTEXT (`COPY . /`) and the cargo build +
/// `cp`/bake `RUN`s. cargo runs AT image-build time; the deployed runtime never
/// repeats it.
///
/// `python-is-python3` is REQUIRED (same crash-loop fix as the RUN path): Modal's
/// container entrypoint execs bare `python`.
fn deploy_image_spec(source_mount_id: &str, package: &str, base_image: &str) -> ImageSpec {
    ImageSpec::from_registry(base_image.to_string())
        .with_apt(&["python3", "python3-pip", "python-is-python3"])
        .with_pip_install_modal()
        .with_wrapper_module(DEPLOY_WRAPPER_MODULE, DEPLOY_WRAPPER_SRC)
        .with_context_mount(source_mount_id)
        // Context root → /, so the /app/src-prefixed tree lands at /app/src (§A4 Primary).
        .with_command("COPY . /")
        // cargo build AT IMAGE-BUILD time; -p disambiguates the shared modal_runner bin.
        .with_command(format!(
            "RUN cd {DEPLOY_SRC} && cargo build --release -p {package} --bin modal_runner"
        ))
        // Bake the freshly built binary to the fixed path the deployed body execs.
        .with_command(format!(
            "RUN cp {DEPLOY_SRC}/target/release/modal_runner {DEPLOY_RUNNER} \
             && chmod +x {DEPLOY_RUNNER}"
        ))
        .with_command("ENV RUST_BACKTRACE=1")
        .with_command("ENTRYPOINT []")
}

/// Deploy (persistently) the wrapper function under the STABLE app name and return
/// a [`DeployedApp`]. PERSISTENT: this is the ONLY path that uses `AppPublish` into
/// a named, get-or-created app.
///
/// Reuses the proven ops verbatim; the structural difference vs RUN is that the
/// source rides the image build CONTEXT (so cargo builds at image-build time) and
/// the function attaches ONLY the client mount (the binary is baked in the layer —
/// NO runtime source mount).
pub(crate) async fn deploy_function(
    client: &mut ModalClient,
    config: &DeployConfig,
) -> Result<DeployedApp> {
    // 1. Client mount (modal source importable in the FILE-mode container).
    let client_mount_id = client.client_mount_id(None).await?;

    // 2. Source mount — UPLOAD the user's crate as the image BUILD CONTEXT (lands
    //    at /app/src/<rel>; the COPY . / drops it at /app/src).
    let ignore: Vec<&str> = config.ignore.iter().map(String::as_str).collect();
    let source_mount_id = client
        .mount_local_dir(&config.local_root, DEPLOY_SRC, &ignore, None)
        .await?;

    // 3. PERSISTENT named app id (deploy-only; re-deploys REPLACE under this name).
    let app_id = client.app_get_or_create_id(&config.app_name, None).await?;

    // 4. Build the deploy image — cargo runs HERE, AT image-build time (the build
    //    logs stream `Compiling`/`cargo build --release` via ImageJoinStreaming).
    let spec = deploy_image_spec(&source_mount_id, &config.package, &config.base_image);
    let image_id = client.image_get_or_create(&app_id, &spec).await?;

    // 5. Precreate the function (name = the wrapper callable, "handler").
    let precreate_id = client
        .function_precreate(&app_id, DEPLOY_WRAPPER_CALLABLE)
        .await?;

    // 6. FunctionCreate (FILE mode): CLIENT mount ONLY — NO source mount (the
    //    binary is baked in the image layer). This absence IS the deploy invariant.
    let fn_spec = FunctionSpec::new(DEPLOY_WRAPPER_MODULE, DEPLOY_WRAPPER_CALLABLE, &image_id)
        .with_mount_ids(vec![client_mount_id])
        .with_timeout_secs(config.timeout_secs);
    let created = client
        .function_create(&app_id, &precreate_id, &fn_spec)
        .await?;

    // 7. PERSISTENT AppPublish so the deploy survives and from_name resolves it.
    let mut function_ids = HashMap::new();
    function_ids.insert(
        DEPLOY_WRAPPER_CALLABLE.to_string(),
        created.function_id.clone(),
    );
    let mut definition_ids = HashMap::new();
    if !created.definition_id.is_empty() {
        definition_ids.insert(created.function_id.clone(), created.definition_id.clone());
    }
    let published = client
        .app_publish_deployed(&app_id, &config.app_name, function_ids, definition_ids)
        .await?;

    // 8. Resolve the invokable function_id (proves from_name works post-deploy).
    let function_id = client
        .function_from_name(&config.app_name, DEPLOY_WRAPPER_CALLABLE, None)
        .await?;

    Ok(DeployedApp {
        name: config.app_name.clone(),
        function_id,
        image_id,
        url: published.url,
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
    let function_id = client
        .function_from_name(app_name, DEPLOY_WRAPPER_CALLABLE, None)
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
    fn deploy_image_spec_rides_source_on_the_build_context() {
        // The uploaded source rides the image build CONTEXT (proto field 15) so
        // cargo can compile it AT image-build time. The COPY/cargo/cp dockerfile
        // ordering is asserted in the SDK-side image.rs test (dockerfile_commands
        // is private to that crate); here we assert the public field the facade
        // controls plus the base/package wiring.
        let spec = deploy_image_spec("mo-deploy-src", "example-add", "rust:1-slim");
        assert_eq!(spec.context_mount_id.as_deref(), Some("mo-deploy-src"));
        assert_eq!(spec.base_image, "rust:1-slim");
        // The cargo-build RUN (an extra command) names the package and target bin.
        assert!(spec
            .extra_commands
            .iter()
            .any(|c| c.contains("cargo build --release -p example-add --bin modal_runner")));
        // The cp/bake RUN bakes the binary to the fixed deployed path.
        assert!(spec
            .extra_commands
            .iter()
            .any(|c| c.contains("cp /app/src/target/release/modal_runner /app/modal_runner")));
        // The COPY brings the context into a layer.
        assert!(spec.extra_commands.iter().any(|c| c.contains("COPY . /")));
    }

    #[test]
    fn deploy_config_default_has_stable_app_name() {
        std::env::remove_var("MODAL_RUST_DEPLOY_APP");
        let cfg = DeployConfig::default();
        assert_eq!(cfg.app_name, "modal-rust-add-deploy");
        assert_eq!(cfg.base_image, "rust:1-slim");
        // The deploy context upload reuses the RUN-path ignore set (no references/).
        assert!(cfg.ignore.iter().any(|p| p == "references"));
        assert!(cfg.ignore.iter().any(|p| p == "target"));
    }
}
