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
pub(crate) const DEPLOY_WRAPPER_SRC: &str = include_str!("deploy/wrapper.py");

/// Web endpoints §4: the GENERATED per-endpoint adapter suffix appended to
/// [`DEPLOY_WRAPPER_SRC`] at deploy-bake time — one module-level
/// `web_<sanitized> = _make_web_handler("<entrypoint>")` line per endpoint
/// entrypoint, so the deployed endpoint function's `implementation_name`
/// ([`crate::remote::web_endpoint_attr`], `web_<sanitized>`) resolves a REAL
/// module attribute in-container. Pure (no I/O); empty input ⇒ empty suffix.
pub(crate) fn web_adapter_suffix(endpoint_entrypoints: &[&str]) -> String {
    endpoint_entrypoints
        .iter()
        .map(|ep| {
            let attr = crate::remote::web_endpoint_attr(ep);
            format!("{attr} = _make_web_handler(\"{ep}\")\n")
        })
        .collect()
}

/// The deploy-wrapper source actually BAKED into the image
/// ([`crate::control_plane::build_deploy_top_layer_spec`]): the static
/// [`DEPLOY_WRAPPER_SRC`] plus the generated [`web_adapter_suffix`] when any deployed
/// entrypoint is a web endpoint. NO endpoints ⇒ the plain [`DEPLOY_WRAPPER_SRC`],
/// byte-identical (web endpoints forward-safety §6).
pub(crate) fn baked_deploy_wrapper_src(endpoint_entrypoints: &[&str]) -> String {
    if endpoint_entrypoints.is_empty() {
        return DEPLOY_WRAPPER_SRC.to_string();
    }
    format!(
        "{DEPLOY_WRAPPER_SRC}\n{}",
        web_adapter_suffix(endpoint_entrypoints)
    )
}

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
    /// OPT-IN: degrade a FAILED memory-snapshot prime to lazy `#[enter]` instead of
    /// failing the container init loudly. Default `false` = STRICT — a broken
    /// `#[enter]`/prime FAILS the deploy visibly (raising at wrapper import) rather
    /// than hiding as a silent per-cold-start perf cliff. When `true` (or the deploy-time
    /// env `MODAL_RUST_SNAPSHOT_BEST_EFFORT` is truthy), the image bakes
    /// `ENV MODAL_RUST_SNAPSHOT_BEST_EFFORT=1` and the wrapper logs + falls back to the
    /// lazy `#[enter]` path instead. Only meaningful when an entrypoint sets
    /// `enable_memory_snapshot`.
    pub snapshot_best_effort: bool,
    /// Owned per-function Modal options used by the manual/no-decorator fallback
    /// function. Decorated entrypoints carry their own [`FunctionOptions`] in
    /// [`DeployEntrypoint`].
    pub options: FunctionOptions,
}

/// Deploy-time env discovery for [`DeployConfig::snapshot_best_effort`]
/// (`MODAL_RUST_SNAPSHOT_BEST_EFFORT` truthy ⇒ opt into degrade-to-lazy).
fn discover_snapshot_best_effort() -> bool {
    std::env::var("MODAL_RUST_SNAPSHOT_BEST_EFFORT")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
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
            snapshot_best_effort: discover_snapshot_best_effort(),
            options: FunctionOptions::default(),
        }
    }

    /// C1: fold a per-function `image = Image(..)` declaration into the SHARED deploy
    /// image (base/install_rust/steps). Deploy bakes ONE `modal_runner` over ONE image,
    /// so v0 uses a single shared image for all entrypoints: the FIRST entrypoint that
    /// declares an `image` wins (entrypoints are visited in deploy-plan order). A
    /// distinct second image declaration is a known v0 limitation — the env-only base
    /// (`MODAL_RUST_BASE_IMAGE`) + path `image_steps` still apply when no entrypoint
    /// declares one. Reuses [`crate::remote::apply_function_image`] via a throwaway
    /// [`RemoteConfig`] view so run and deploy fold images identically (no drift).
    pub(crate) fn with_function_image<'a>(
        mut self,
        entrypoint_images: impl IntoIterator<Item = Option<&'a str>>,
    ) -> Result<Self> {
        let Some(spec) = entrypoint_images.into_iter().flatten().next() else {
            return Ok(self);
        };
        // Borrow the deploy build fields into a RemoteConfig view, fold the image, copy
        // back. (RemoteConfig is the canonical build-config carrier the fold helper
        // operates on; deploy reuses the same fields.)
        let view = RemoteConfig {
            base_image: std::mem::take(&mut self.base_image),
            install_rust: self.install_rust,
            image_steps: std::mem::take(&mut self.image_steps),
            ..RemoteConfig::default()
        };
        let view = crate::remote::apply_function_image(view, Some(spec))?;
        self.base_image = view.base_image;
        self.install_rust = view.install_rust;
        self.image_steps = view.image_steps;
        Ok(self)
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
    /// Entrypoint name → served web endpoint URL, for entrypoints declared with
    /// `#[endpoint(..)]` (empty for plain functions). BTreeMap so iteration (and any
    /// printed listing) is deterministically ordered.
    pub endpoint_urls: std::collections::BTreeMap<String, String>,
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

/// The pip requirement the deploy BASE layer auto-installs for web endpoints. Modal's
/// `WEBHOOK_TYPE_FUNCTION` worker wraps the in-container callable in a FastAPI app, and
/// FunctionCreate REJECTS an endpoint function whose image lacks FastAPI — so an
/// endpoint deploy must carry it (web endpoints §3, spike finding 2). `fastapi[standard]`
/// is Modal's own recommended extra.
const ENDPOINT_FASTAPI_REQUIREMENT: &str = "fastapi[standard]";

/// Web endpoints §3: append `pip_install(["fastapi[standard]"])` to the deploy
/// BASE-layer image steps when ANY deployed entrypoint declares `webhook_method`.
///
/// Deploy-time and deploy-only — called where the deploy plan is assembled in
/// [`deploy_function`] (NOT in [`DeployConfig::for_app`]), so constructing a config
/// never mutates the steps and the RUN path is untouched. The step rides the BASE
/// layer, so the TOP layer's image-build-time `cargo build` AND the deployed runtime
/// inherit FastAPI (exactly like user `image_steps`).
///
/// SKIPPED when the user's own steps already pip-install fastapi (package-NAME match
/// over `pip_install` packages), so a user-pinned `fastapi==..` / `fastapi[standard]` wins —
/// including pins folded from a per-function `image = Image(pip = [..])`, which is why
/// this runs AFTER the C1 image fold. No endpoint ⇒ the steps are left UNTOUCHED ⇒ the
/// rendered image stays byte-identical (forward-safety §6).
fn append_endpoint_pip_step<'a>(
    image_steps: &mut Vec<crate::ImageStep>,
    entrypoint_options: impl IntoIterator<Item = &'a FunctionOptions>,
) {
    let any_endpoint = entrypoint_options
        .into_iter()
        .any(|options| options.webhook_method.is_some());
    if !any_endpoint {
        return;
    }
    let user_pip_installs_fastapi = image_steps.iter().any(|step| match step {
        crate::ImageStep::Pip(packages) => packages.iter().any(|pkg| {
            // Match the PACKAGE NAME, not a substring: `fastapi`, `fastapi[standard]`,
            // `fastapi==0.110`, `FastAPI >=0.1` all count; `fastapi-utils` must NOT.
            let name_end = pkg
                .find(['[', '=', '<', '>', '!', '~', ' ', ';'])
                .unwrap_or(pkg.len());
            pkg[..name_end].trim().eq_ignore_ascii_case("fastapi")
        }),
        _ => false,
    });
    if !user_pip_installs_fastapi {
        image_steps.push(crate::ImageStep::pip([ENDPOINT_FASTAPI_REQUIREMENT]));
    }
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

    // C1: fold a per-function `image = Image(..)` into the SHARED deploy image (v0: the
    // first entrypoint that declares one wins). Done up front so the build context +
    // both image layers render on the declared base.
    let mut config = config
        .clone()
        .with_function_image(entrypoints.iter().map(|ep| ep.options.image.as_deref()))?;

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

    // Web endpoints §3: ANY planned entrypoint with `webhook_method` ⇒ the deploy BASE
    // layer auto-installs FastAPI (Modal's FUNCTION webhook requires it in the image).
    // Gated over the PLAN (like the control plane's snapshot-prime gate) so the
    // manual/no-decorator fallback options count too, and AFTER the C1 fold so a
    // user-declared image's own fastapi pip step suppresses the auto-step. No endpoint
    // ⇒ steps untouched ⇒ the deploy image stays byte-identical.
    append_endpoint_pip_step(&mut config.image_steps, plan.iter().map(|ep| &ep.options));
    let config = &config;
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
        snapshot_best_effort: config.snapshot_best_effort,
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

    // Map served endpoint URLs back from object tags to the user's entrypoint names
    // (provision keys `endpoint_urls` on the sanitized object tag).
    let endpoint_urls = plan
        .iter()
        .filter_map(|ep| {
            let tag = crate::remote::sanitize_object_tag(&ep.name);
            published
                .endpoint_urls
                .get(&tag)
                .map(|url| (ep.name.clone(), url.clone()))
        })
        .collect();

    Ok(DeployedApp {
        name: config.app_name.clone(),
        function_id,
        image_id: provisioned.image_id,
        url: provisioned.publish_url,
        endpoint_urls,
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
    fn deploy_wrapper_src_has_module_global_snapshot_prime() {
        let src = DEPLOY_WRAPPER_SRC;
        // The eager prime is a MODULE-GLOBAL call (runs at import, before the snapshot),
        // gated on _serve_enabled() AND the MODAL_RUST_SNAPSHOT_PRIME env.
        assert!(
            src.contains("def _snapshot_prime():"),
            "the import-time prime helper must be defined"
        );
        assert!(
            src.contains("MODAL_RUST_SNAPSHOT_PRIME"),
            "the prime must gate on the MODAL_RUST_SNAPSHOT_PRIME env"
        );
        assert!(
            src.contains(r#"json.dumps({"kind": "prime"})"#),
            "the prime must write a `{{\"kind\":\"prime\"}}` frame"
        );
        assert!(
            src.contains("_serve_enabled() and _snapshot_prime_enabled()"),
            "the prime must be gated on both serve-enabled and the prime env"
        );
        // It is INVOKED at module-global scope (after the helpers + handler are defined),
        // so it fires at import — before Modal takes the memory snapshot.
        let call_at = src
            .rfind("\n_snapshot_prime()")
            .expect("module-global _snapshot_prime() call must be present");
        let def_at = src
            .find("def _snapshot_prime():")
            .expect("def present (checked above)");
        assert!(
            call_at > def_at,
            "the module-global prime call must come AFTER its definition"
        );
        // FAIL-LOUD CONTRACT: a failed prime RAISES at import by default (the container
        // fails to boot, surfacing the broken `#[enter]` at deploy time instead of
        // hiding it as a per-cold-start perf cliff)...
        assert!(
            src.contains("raise RuntimeError"),
            "a failed prime must raise (strict default), not be swallowed"
        );
        assert!(
            src.contains("memory-snapshot prime FAILED"),
            "the raise must carry an actionable message"
        );
        // ...and the ack's failure report is what drives it (failed/errors from the
        // runner's prime ack — a reported #[enter] failure counts as failure).
        assert!(
            src.contains(r#"report.get("failed""#),
            "the wrapper must parse the ack's failure report"
        );
        // The degrade-to-lazy path exists ONLY behind the explicit opt-in env.
        assert!(
            src.contains("MODAL_RUST_SNAPSHOT_BEST_EFFORT"),
            "degrade-to-lazy must be gated on the explicit best-effort opt-in"
        );
    }

    #[test]
    fn deploy_wrapper_src_has_web_handler_factory() {
        let src = DEPLOY_WRAPPER_SRC;
        // The per-endpoint adapter FACTORY (web endpoints §4): the deploy bake appends
        // generated `web_<sanitized> = _make_web_handler("<ep>")` lines that call it.
        let def_at = src
            .find("def _make_web_handler(entrypoint):")
            .expect("the web-handler factory must be defined");
        // The adapter is ASYNC and takes the FastAPI Request (Modal's FUNCTION webhook
        // introspects this signature), reads the RAW body (empty body -> "{}")...
        assert!(
            src.contains("async def _web(request: Request):"),
            "the adapter must be async over the FastAPI Request"
        );
        assert!(
            src.contains(r#"(await request.body()).decode() or "{}""#),
            "the adapter must read the raw body, defaulting empty to {{}}"
        );
        // ...frames through the EXISTING handler() ⇒ the SAME serve child (so `#[cls]`
        // load-once + the memory-snapshot prime compose with endpoints for free)...
        assert!(
            src.contains("handler(entrypoint, body)"),
            "the adapter must frame through the existing handler()"
        );
        // ...returns the decoded envelope value on ok, else a JSON error Response:
        // decode_error -> 422, anything else -> 500.
        assert!(src.contains(r#"if env.get("ok"):"#));
        assert!(
            src.contains(r#"422 if err.get("kind") == "decode_error" else 500"#),
            "the error contract must map decode_error to 422, otherwise 500"
        );
        assert!(
            src.contains(r#"media_type="application/json""#),
            "the error Response must be JSON"
        );
        // FastAPI is imported LOCALLY inside the factory — a non-endpoint deploy never
        // calls it, so the import never runs off-path. Exactly ONE fastapi import, and
        // it sits AFTER the factory def (i.e. inside it, not at module top level).
        let import_at = src
            .find("from fastapi import Request, Response")
            .expect("the factory must import fastapi locally");
        assert!(
            import_at > def_at,
            "the fastapi import must be INSIDE the factory, not module-level"
        );
        assert_eq!(
            src.matches("fastapi").count(),
            1,
            "exactly one (local) fastapi import — module import must stay fastapi-free"
        );
    }

    #[test]
    fn web_adapter_suffix_generates_one_line_per_endpoint() {
        // The pure suffix builder (web endpoints §4): one module-level
        // `web_<sanitized> = _make_web_handler("<entrypoint>")` line per endpoint.
        assert_eq!(
            web_adapter_suffix(&["add"]),
            "web_add = _make_web_handler(\"add\")\n"
        );
        // The attr sanitizes dots/dashes to `_` (a valid Python identifier) while the
        // factory arg keeps the RAW entrypoint name (the handler dispatch key).
        assert_eq!(
            web_adapter_suffix(&["predict", "my-fn.v2"]),
            "web_predict = _make_web_handler(\"predict\")\n\
             web_my_fn_v2 = _make_web_handler(\"my-fn.v2\")\n"
        );
        assert_eq!(web_adapter_suffix(&[]), "", "no endpoints ⇒ empty suffix");
    }

    #[test]
    fn baked_deploy_wrapper_src_is_byte_identical_without_endpoints() {
        // Off-path forward safety (web endpoints §6): no endpoints ⇒ the baked wrapper
        // is the plain static source, BYTE-IDENTICAL.
        assert_eq!(baked_deploy_wrapper_src(&[]), DEPLOY_WRAPPER_SRC);
    }

    #[test]
    fn baked_deploy_wrapper_src_appends_the_adapter_suffix_after_the_static_source() {
        let src = baked_deploy_wrapper_src(&["add"]);
        assert!(
            src.starts_with(DEPLOY_WRAPPER_SRC),
            "the static wrapper source must lead, unmodified"
        );
        assert_eq!(
            &src[DEPLOY_WRAPPER_SRC.len()..],
            "\nweb_add = _make_web_handler(\"add\")\n",
            "the generated adapter lines ride after a separating newline"
        );
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

    #[test]
    fn endpoint_deploy_appends_and_bakes_the_fastapi_pip_step() {
        use crate::control_plane::{
            build_image_spec, Entrypoint, ProvisionInputs, SourceInputs, DEPLOY_BOUNDARY,
        };
        // Web endpoints §3 (spike finding 2): ANY deployed endpoint ⇒ the deploy BASE
        // layer gains pip_install(["fastapi[standard]"]) AFTER the user's own steps.
        let plan = [Entrypoint {
            name: "predict".to_string(),
            options: FunctionOptions {
                webhook_method: Some("POST".to_string()),
                ..FunctionOptions::default()
            },
            timeout_secs: 300,
        }];
        let mut steps = vec![crate::ImageStep::apt(["libssl-dev"])];
        append_endpoint_pip_step(&mut steps, plan.iter().map(|ep| &ep.options));
        assert_eq!(
            steps,
            vec![
                crate::ImageStep::apt(["libssl-dev"]),
                crate::ImageStep::pip(["fastapi[standard]"]),
            ],
            "endpoint deploy appends the fastapi auto-step after the user steps"
        );
        // ...and the DEPLOY BASE layer bakes it as the canonical pip RUN line (the TOP
        // layer + deployed runtime inherit it).
        let inputs = ProvisionInputs {
            app_name: "app",
            app_id: Some("ap-1"),
            source: SourceInputs {
                local_root: std::path::Path::new("/ws"),
                package: "example-add",
                use_cargo_scoping: false,
                modalignore_name: ".modalignore",
                remote_src: DEPLOY_SRC,
            },
            base_image: "rust:1-slim",
            install_rust: false,
            image_steps: &steps,
            cache: false,
            snapshot_best_effort: false,
            entrypoints: &plan,
        };
        let base = build_image_spec(&DEPLOY_BOUNDARY, &inputs, "mo-py-standalone");
        assert!(
            base.builder_steps
                .iter()
                .any(|c| c == "RUN python3 -m pip install --no-cache-dir fastapi[standard]"),
            "deploy BASE layer must bake the fastapi pip step, got: {:?}",
            base.builder_steps
        );
    }

    #[test]
    fn non_endpoint_deploy_image_steps_stay_byte_identical() {
        // Off-path forward safety (web endpoints §6): NO endpoint ⇒ the steps are
        // UNTOUCHED (both the empty default and user-supplied steps) ⇒ the rendered
        // deploy image is byte-identical to before web endpoints.
        let plain = FunctionOptions::default();
        let mut steps: Vec<crate::ImageStep> = Vec::new();
        append_endpoint_pip_step(&mut steps, [&plain]);
        assert!(
            steps.is_empty(),
            "no endpoint ⇒ no auto-step on the default path"
        );

        let user = vec![
            crate::ImageStep::pip(["requests"]),
            crate::ImageStep::run(["echo ok"]),
        ];
        let mut steps = user.clone();
        append_endpoint_pip_step(&mut steps, [&plain]);
        assert_eq!(steps, user, "no endpoint ⇒ user steps stay byte-identical");
    }

    #[test]
    fn endpoint_deploy_skips_the_auto_step_when_user_pip_installs_fastapi() {
        // Dedup (§3): the user already pip-installs fastapi (substring check over
        // pip_install packages — a pinned `fastapi==..` counts) ⇒ the auto-step is
        // SKIPPED so the user's pin wins.
        let endpoint = FunctionOptions {
            webhook_method: Some("GET".to_string()),
            ..FunctionOptions::default()
        };
        let user = vec![crate::ImageStep::pip(["fastapi==0.115.0", "uvloop"])];
        let mut steps = user.clone();
        append_endpoint_pip_step(&mut steps, [&endpoint]);
        assert_eq!(
            steps, user,
            "a user fastapi pip step suppresses the auto-step"
        );

        // Only pip_install packages count: a non-pip step mentioning fastapi does not
        // install the Python package, so the auto-step still rides.
        let mut steps = vec![crate::ImageStep::run(["echo fastapi"])];
        append_endpoint_pip_step(&mut steps, [&endpoint]);
        assert_eq!(
            steps,
            vec![
                crate::ImageStep::run(["echo fastapi"]),
                crate::ImageStep::pip(["fastapi[standard]"]),
            ],
            "non-pip steps do not suppress the auto-step"
        );
    }
}
