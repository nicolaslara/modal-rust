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

use crate::remote::{RemoteConfig, PYTHON_SERIES};
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
    /// GPU spec for the deployed entrypoint (from the decorator [`FunctionConfig`]).
    /// `None` = CPU. Set by `App::deploy_with` from the decorated entrypoint's
    /// config before [`deploy_function`].
    pub gpu: Option<String>,
    /// Per-entrypoint timeout override (decorator `FunctionConfig.timeout_secs`).
    /// When `Some`, REPLACES [`timeout_secs`](DeployConfig::timeout_secs).
    pub timeout_override_secs: Option<u32>,
    /// Install the Rust toolchain (rustup) + the CUDA build/run env into the deploy
    /// BASE layer. Set when [`base_image`](DeployConfig::base_image) is a non-Rust
    /// base (e.g. a `nvidia/cuda:<ver>-devel` Tier-1 base; boundaries.md §9) so the
    /// TOP layer's image-build-time `cargo build` inherits a toolchain + the CUDA
    /// headers. Default `false`. Inherited from [`RemoteConfig::default()`] in
    /// [`for_app`](DeployConfig::for_app), so the `MODAL_RUST_INSTALL_RUST` env default
    /// flows through automatically (parity with `base_image`).
    pub install_rust: bool,
    /// Named Modal secrets to attach (from `#[function(secrets = [..])]`). Resolved
    /// to `secret_id`s and attached to `Function.secret_ids`; Modal injects their
    /// key/values as ENV VARS. DEFAULT EMPTY. Set by `App::deploy_with` from the
    /// decorated entrypoint's config (parity with the RUN path).
    pub secrets: Vec<String>,
    /// User volumes to attach as `(mount_path, volume_name)` pairs (from
    /// `#[function(volumes = ["/data=my-vol"])]`). Each `volume_name` is resolved via
    /// [`ModalClient::volume_get_or_create`] and mounted at `mount_path`. The DEPLOY
    /// path has no cargo cache, so there is no `/cache` collision concern. DEFAULT
    /// EMPTY.
    pub volumes: Vec<(String, String)>,
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
            gpu: None,
            timeout_override_secs: None,
            install_rust: base.install_rust,
            secrets: Vec::new(),
            volumes: Vec::new(),
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

/// Render the DEPLOY BASE layer (layer 1): `rust:1-slim` + `add_python` (the hosted
/// python-build-standalone mount). This layer owns the standalone mount as its build
/// CONTEXT so its `COPY /python/. /usr/local` has a source; it carries no wrapper and
/// no source. The TOP layer ([`deploy_top_layer_spec`]) bases on it via `FROM base`.
///
/// Two layers are REQUIRED because an `Image` has ONE `context_mount_id`: the source
/// (top layer) and the standalone (base layer) each need their own. This mirrors the
/// official client's image layering (`base_images={"base": self}`).
fn deploy_base_layer_spec(
    python_standalone_mount_id: &str,
    base_image: &str,
    install_rust: bool,
) -> ImageSpec {
    // The rust toolchain + CUDA env ride the BASE layer (which already owns
    // add_python) so the toolchain + CUDA headers are present BEFORE the TOP layer's
    // `cargo build` runs. Default base never sets this (byte-identical default render).
    let mut spec = ImageSpec::from_registry(base_image.to_string())
        .with_add_python(PYTHON_SERIES)
        .with_python_standalone_mount_id(python_standalone_mount_id);
    if install_rust {
        spec = spec.with_rust_toolchain();
    }
    spec
}

/// Render the DEPLOY TOP layer (layer 2): bases on the add_python layer via
/// `FROM base` ([`ImageSpec::with_base_image`]), bakes the deploy wrapper, then COPYs
/// the SOURCE (this layer's build CONTEXT) and runs the cargo build + `cp`/bake. cargo
/// runs AT image-build time against the rust+python from layer 1; the deployed runtime
/// never repeats it.
///
/// Python comes from layer 1 (`add_python`), NOT apt — same provisioning as the RUN
/// path. The auto `ln -s python3 python` (series < 3.13, emitted in layer 1) satisfies
/// Modal's bare `python` entrypoint.
fn deploy_top_layer_spec(base_image_id: &str, source_mount_id: &str, package: &str) -> ImageSpec {
    ImageSpec::from_registry(String::new()) // FROM is replaced by `FROM base` (layered).
        .with_base_image(base_image_id)
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
    //    at /app/src/<rel>; the COPY . / drops it at /app/src). Same scoping as the
    //    RUN path: PRIMARY = cargo-metadata closure of the target package + the
    //    workspace Cargo.toml/Cargo.lock; FALLBACK = whole local_root minus ignored
    //    files. Both prune via `.modalignore` > `.gitignore` > built-in defaults.
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
                .mount_workspace_closure(&spec, DEPLOY_SRC, None)
                .await?
        }
        _ => {
            client
                .mount_local_dir(
                    &config.local_root,
                    DEPLOY_SRC,
                    &config.modalignore_name,
                    None,
                )
                .await?
        }
    };

    // 2b. Python-standalone mount (HOSTED, resolved by name) → the BASE layer's
    //     build context for `add_python`.
    let py_mount_id = client
        .python_standalone_mount_id(PYTHON_SERIES, None)
        .await?;

    // 3. PERSISTENT named app id (deploy-only; re-deploys REPLACE under this name).
    let app_id = client.app_get_or_create_id(&config.app_name, None).await?;

    // 4. Build the deploy image as TWO LAYERS — cargo runs HERE, AT image-build time
    //    (the build logs stream `Compiling`/`cargo build --release` via
    //    ImageJoinStreaming). Two layers are required because an Image has ONE
    //    context_mount_id: layer 1 (add_python) owns the standalone mount; layer 2
    //    (source + cargo build) owns the source mount. This mirrors the official
    //    client's image layering and provisions Python via add_python, NOT apt/pip.
    let base_spec = deploy_base_layer_spec(&py_mount_id, &config.base_image, config.install_rust);
    let base_image_id = client.image_get_or_create(&app_id, &base_spec).await?;
    let spec = deploy_top_layer_spec(&base_image_id, &source_mount_id, &config.package);
    let image_id = client.image_get_or_create(&app_id, &spec).await?;

    // 5. Precreate the function (name = the wrapper callable, "handler").
    let precreate_id = client
        .function_precreate(&app_id, DEPLOY_WRAPPER_CALLABLE)
        .await?;

    // 6. FunctionCreate (FILE mode): CLIENT mount ONLY — NO source mount (the
    //    binary is baked in the image layer). This absence IS the deploy invariant.
    //    `mount_client_dependencies = true` (default, explicit) so the worker injects
    //    the modal client dep closure at start — the add_python image has no pip layer.
    // Decorator config: gpu rides into Resources.gpu_config; a `timeout` decorator
    // overrides the deploy default. Deploy has no in-body build, so its timeout is
    // purely the function's. `with_gpu(None)` is a CPU no-op (wire bytes identical).
    let timeout = config.timeout_override_secs.unwrap_or(config.timeout_secs);
    let mut fn_spec = FunctionSpec::new(DEPLOY_WRAPPER_MODULE, DEPLOY_WRAPPER_CALLABLE, &image_id)
        .with_mount_ids(vec![client_mount_id])
        .with_mount_client_dependencies(true)
        .with_timeout_secs(timeout)
        .with_gpu(config.gpu.clone())?;
    // 6b. USER secrets (`#[function(secrets=..)]`): resolve each named secret to an
    //     id (from_name lookup) and attach → Function.secret_ids (Modal injects their
    //     key/values as ENV VARS). Empty ⇒ no-op (wire-identical to before). Values
    //     never logged. The DEPLOY runtime never builds, so secrets are purely the
    //     function's runtime env — consistent with the RUN path.
    let mut secret_ids: Vec<String> = Vec::with_capacity(config.secrets.len());
    for name in &config.secrets {
        secret_ids.push(client.secret_get_or_create(name, &[], None).await?);
    }
    if !secret_ids.is_empty() {
        fn_spec = fn_spec.with_secret_ids(secret_ids);
    }
    // 6c. USER volumes (`#[function(volumes=["/mount=name"])]`): resolve each named
    //     volume (create-if-missing, V1) and attach at its mount_path. DEPLOY has no
    //     cargo cache, so no `/cache` collision concern. Empty ⇒ no-op.
    for (mount_path, name) in &config.volumes {
        let vid = client
            .volume_get_or_create(name, false /* v1 */, true /* create */, None)
            .await?;
        fn_spec = fn_spec.with_volume_mount(vid, mount_path.clone());
    }
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
    fn deploy_base_layer_provisions_python_via_add_python() {
        // Layer 1: add_python(3.12) on the rust base, with the standalone mount as
        // the build context. NO apt, NO pip — Python comes from the standalone mount.
        let base = deploy_base_layer_spec("mo-py-standalone", "rust:1-slim", false);
        assert_eq!(base.base_image, "rust:1-slim");
        assert_eq!(base.add_python.as_deref(), Some("3.12"));
        assert_eq!(
            base.python_standalone_mount_id.as_deref(),
            Some("mo-py-standalone")
        );
        // No apt/pip fallback on the base layer.
        assert!(base.pre_bake_commands.is_empty());
        assert!(!base.pip_install_modal);
        // Default base layer (install_rust=false) carries no rust install. The actual
        // rendered-command suppression is asserted SDK-side
        // (image::tests::default_path_renders_none_of_the_rust_toolchain_steps), since
        // `dockerfile_commands` is private to the SDK crate; here we assert the public
        // field the facade controls.
        assert!(!base.install_rust, "default base layer has no rust install");
    }

    #[test]
    fn deploy_base_layer_installs_rust_for_cuda_base() {
        // CUDA Tier-1 deploy: a `nvidia/cuda:<ver>-devel` base + add_python +
        // with_rust_toolchain on the BASE layer, so the rust toolchain + CUDA headers
        // are present before the TOP layer's `cargo build`. The base layer owns
        // add_python (its standalone mount as build context), so the rustup + CUDA env
        // render before any top-layer cargo build. The rendered ordering (add_python
        // COPY < rustup < PATH/CUDA_PATH ENV < bake) is asserted SDK-side
        // (image::tests::rust_toolchain_renders_after_add_python_and_before_bakes);
        // here we assert the public fields the facade controls.
        let base = deploy_base_layer_spec(
            "mo-py-standalone",
            "nvidia/cuda:12.6.3-devel-ubuntu22.04",
            true,
        );
        assert_eq!(base.base_image, "nvidia/cuda:12.6.3-devel-ubuntu22.04");
        assert_eq!(base.add_python.as_deref(), Some("3.12"));
        assert!(base.install_rust, "CUDA base layer installs rust");
    }

    #[test]
    fn deploy_top_layer_rides_source_on_the_build_context() {
        // Layer 2: bases on layer 1 (FROM base), the source rides this layer's build
        // CONTEXT (proto field 15) so cargo compiles it AT image-build time. The
        // COPY/cargo/cp dockerfile ordering is asserted in the SDK-side image.rs
        // test; here we assert the public fields the facade controls.
        let spec = deploy_top_layer_spec("im-layer1", "mo-deploy-src", "example-add");
        assert_eq!(spec.base_image_id.as_deref(), Some("im-layer1"));
        assert_eq!(spec.context_mount_id.as_deref(), Some("mo-deploy-src"));
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
        // No apt/pip on the top layer either (Python is inherited from layer 1).
        assert!(spec.pre_bake_commands.is_empty());
        assert!(!spec.pip_install_modal);
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
        assert!(cfg.secrets.is_empty(), "secrets default empty");
        assert!(cfg.volumes.is_empty(), "volumes default empty");
    }

    #[test]
    fn deploy_config_secrets_volumes_are_settable_non_macro() {
        // Non-macro override: `DeployConfig`'s public fields let a builder/explicit
        // caller set secrets + user volumes WITHOUT the decorator.
        let _guard = crate::ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Struct-update over the env-aware config: a non-macro caller sets ONLY
        // secrets/volumes and keeps every other field.
        let cfg = DeployConfig {
            secrets: vec!["api-creds".to_string()],
            volumes: vec![("/models".to_string(), "weights".to_string())],
            ..DeployConfig::for_app("my-app")
        };
        assert_eq!(cfg.secrets, vec!["api-creds".to_string()]);
        assert_eq!(
            cfg.volumes,
            vec![("/models".to_string(), "weights".to_string())]
        );
    }
}
