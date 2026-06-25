//! The ONE control-plane provisioning sequence behind the RUN, DEPLOY, and DUMP
//! paths — the driver seam that collapses the three formerly-duplicated copies of
//! "AppCreate/Get → mounts → ImageGetOrCreate → per-entrypoint Precreate+Create →
//! AppPublish" into a single, generic [`provision`] function.
//!
//! ## The seam
//!
//! [`provision`] is written ONCE and is generic over a [`ControlPlane`] — the small,
//! cohesive set of async control-plane operations the sequence needs (~8 methods).
//! Two implementations drive it:
//!
//! - [`LiveControlPlane`] — the real [`modal_rust_sdk::ModalClient`]. It ENCAPSULATES
//!   the live-only mess inside its methods: the `ImageJoinStreaming` poll loop +
//!   reconnect/retry live inside the SDK's `image_get_or_create` (called by
//!   [`ControlPlane::ensure_image`]); blob upload lives inside the SDK's mount upload
//!   (called by [`ControlPlane::ensure_source_mount`]). The sequence never sees
//!   streaming/retry/blob.
//! - [`RecordingControlPlane`] — the offline DRY-RUN / DUMP. Each method RECORDS the
//!   request it was handed and returns a DETERMINISTIC fake id (`ap-1`, `im-1`,
//!   `mo-{n}`, …) so the sequence keeps threading. The recorded requests ARE the
//!   manifest, so [`crate::App::dry_run`] / [`crate::App::dump_deploy_manifest`] are
//!   literally `provision(RecordingControlPlane, …)` and therefore cannot drift from
//!   the live path. See [`crate::dump`].
//!
//! ## The ONLY run/deploy divergence — the Boundary
//!
//! The whole sequence is identical across run/deploy EXCEPT the four inputs captured
//! by [`Boundary`] (app state, source delivery, build timing, publish state). The
//! divergence is isolated to that value, the [`ProvisionPlan`] it constructs (the
//! entrypoint arity + publish cadence [`provision`] consults), the pure
//! [`build_image_spec`] / [`build_function_spec`] functions, and the deploy-only
//! [`deploy_gates`] — there is NO scattered `if run {…} else {…}` in [`provision`].
//!
//! | aspect          | run                              | deploy                                   |
//! | --------------- | -------------------------------- | ---------------------------------------- |
//! | app state       | ephemeral (existing id)          | deployed (get-or-create)                 |
//! | source delivery | source MOUNT on the function     | source COPIED into the image build ctx   |
//! | build timing    | cargo build in the function body | cargo build in a Dockerfile RUN layer    |
//! | publish state   | ephemeral                        | deployed                                 |
//!
//! ## Dump fidelity boundary
//!
//! The dump captures the TOP-LEVEL RPCs the live path sends (with fabricated ids),
//! in order — NOT the live-only `ImageJoinStreaming` poll loop or the per-file
//! mount-upload PUT/probe traffic, which are encapsulated inside [`LiveControlPlane`].

use std::collections::HashMap;

use modal_rust_sdk::{FunctionAutoscaler, FunctionSpec, ImageSpec, ModalClient, WebhookSpec};

use crate::deploy::{
    baked_deploy_wrapper_src, DEPLOY_RUNNER, DEPLOY_SRC, DEPLOY_WRAPPER_CALLABLE,
    DEPLOY_WRAPPER_MODULE,
};
use crate::remote::{
    run_wrapper_config_env, run_wrapper_src, CACHE_MOUNT, CACHE_VOLUME_NAME, PYTHON_SERIES,
    WRAPPER_CALLABLE, WRAPPER_MODULE,
};
use crate::{Error, FunctionOptions, Result};

/// How the app object is resolved + the state it is published in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppState {
    /// The RUN path: an EPHEMERAL app. It is created at `App::connect` time
    /// (`AppCreate`), so [`provision`] receives the already-resolved `app_id` and the
    /// [`ControlPlane`] does not re-create it on the live path. Published with
    /// `APP_STATE_EPHEMERAL`.
    Ephemeral,
    /// The DEPLOY path: a PERSISTENT named app, resolved via `AppGetOrCreate`
    /// (create-if-missing; re-deploys REPLACE under the name). Published with
    /// `APP_STATE_DEPLOYED`.
    Deployed,
}

/// How the user's source crate reaches the container — the build boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceDelivery {
    /// RUN: the source is MOUNTED at `/src` (`add_local_dir(copy=False)` equivalent)
    /// and attached to the function via `mount_ids`; `cargo build` runs IN THE
    /// FUNCTION BODY at invoke time. The run image carries the cargo cache / user
    /// volumes / secrets on each function.
    RunMount,
    /// DEPLOY: the source rides the image BUILD CONTEXT (`Image.context_mount_id` + a
    /// `COPY` step); `cargo build` runs in a Dockerfile `RUN` layer at image-build
    /// time. The deployed function attaches the CLIENT mount ONLY — the prebuilt
    /// `/app/modal_runner` is baked into the image.
    DeployContext,
}

/// The ONLY run/deploy divergence, modeled as data (not parallel code). Everything
/// else in [`provision`] is structurally identical across the two paths.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Boundary {
    /// App resolution + publish state (ephemeral vs deployed).
    pub app_state: AppState,
    /// Source delivery + build timing (mount/defer-build vs COPY/cargo-layer).
    pub source_delivery: SourceDelivery,
}

/// The RUN boundary: ephemeral app, source mount, defer the build to the function
/// body, ephemeral publish.
pub(crate) const RUN_BOUNDARY: Boundary = Boundary {
    app_state: AppState::Ephemeral,
    source_delivery: SourceDelivery::RunMount,
};

/// The DEPLOY boundary: persistent app, source copied into the image build context,
/// build at image-build time, deployed publish.
pub(crate) const DEPLOY_BOUNDARY: Boundary = Boundary {
    app_state: AppState::Deployed,
    source_delivery: SourceDelivery::DeployContext,
};

/// The entrypoint ARITY + publish cadence each boundary owns — the run/deploy shape
/// difference [`provision`] CONSULTS instead of re-deriving `if run {…}` booleans
/// inline (the module-doc "no scattered branches" contract). Constructed ONCE per
/// call by [`Boundary::plan`], so the RUN single-entrypoint contract lives with the
/// boundary owner rather than as ad-hoc checks scattered through the sequence.
#[derive(Clone, Copy)]
enum ProvisionPlan<'a> {
    /// RUN: exactly ONE entrypoint per [`provision`] call (the caller memoizes per
    /// entrypoint and threads the cumulative publish union across calls). The app is
    /// resolved FIRST (it was created at connect time) along with the run-level
    /// cache/secrets/volumes, and the cumulative union is RE-PUBLISHED after EACH
    /// create.
    Single(&'a Entrypoint),
    /// DEPLOY: every entrypoint in one call. The app is resolved AFTER the mounts,
    /// per-entrypoint resources are resolved inside the create loop, and ONE
    /// persistent publish carries the union after the loop.
    Many,
}

impl Boundary {
    /// Construct the [`ProvisionPlan`] this boundary owns. The RUN path provisions
    /// exactly ONE entrypoint per call, enforced HERE (once, up front) instead of
    /// mid-sequence.
    fn plan<'a>(&self, entrypoints: &'a [Entrypoint]) -> Result<ProvisionPlan<'a>> {
        match self.source_delivery {
            SourceDelivery::RunMount => {
                entrypoints
                    .first()
                    .map(ProvisionPlan::Single)
                    .ok_or_else(|| {
                        Error::config("RUN provision requires exactly one entrypoint".to_string())
                    })
            }
            SourceDelivery::DeployContext => Ok(ProvisionPlan::Many),
        }
    }
}

/// The DEPLOY-only IMAGE gates, computed PURELY from the entrypoints. Kept next to
/// [`DEPLOY_BOUNDARY`] so each new deploy-only feature lands its gate HERE — not as
/// another inline block inside [`provision`]'s image arm. Both gates are opt-in, so
/// an undecorated deploy renders byte-identically (the off-path guarantee).
struct DeployGates<'a> {
    /// Bake `ENV MODAL_RUST_SNAPSHOT_PRIME=1` into the top layer: `true` only when
    /// ANY deployed entrypoint opted into `enable_memory_snapshot` (§9).
    bake_snapshot_prime: bool,
    /// Web endpoints §4: every deployed entrypoint that declared `webhook_method`
    /// gets a generated `web_<sanitized>` adapter line in the baked wrapper (the
    /// endpoint function's `implementation_name`). Empty ⇒ the plain static wrapper.
    endpoint_entrypoints: Vec<&'a str>,
    /// Web server §5: every deployed `#[web_server]` entrypoint as `(name, port)` gets a
    /// generated `web_server_<sanitized> = _make_web_server_handler(<port>, "<name>")`
    /// launcher line in the baked wrapper. Empty ⇒ the plain static wrapper.
    web_server_entrypoints: Vec<(&'a str, u16)>,
}

fn deploy_gates(entrypoints: &[Entrypoint]) -> DeployGates<'_> {
    DeployGates {
        bake_snapshot_prime: entrypoints
            .iter()
            .any(|ep| ep.options.enable_memory_snapshot),
        endpoint_entrypoints: entrypoints
            .iter()
            // A `#[web_server]` (web_server_port set) is NOT a FUNCTION endpoint, even
            // though it shares the webhook path; only true `#[endpoint]`s list here.
            .filter(|ep| {
                ep.options.webhook_method.is_some() && ep.options.web_server_port.is_none()
            })
            .map(|ep| ep.name.as_str())
            .collect(),
        web_server_entrypoints: entrypoints
            .iter()
            .filter_map(|ep| ep.options.web_server_port.map(|p| (ep.name.as_str(), p)))
            .collect(),
    }
}

/// One entrypoint to provision: its NAME (the Modal object TAG) plus its effective
/// per-entrypoint [`FunctionOptions`] (gpu/timeout/cache/secrets/volumes). The shared
/// image bakes the one `modal_runner` that dispatches all entrypoints, so each of
/// these becomes a DISTINCT Modal function over the SAME image, carrying its OWN
/// config — divergent configs coexist instead of clobbering one shared tag.
#[derive(Debug, Clone)]
pub(crate) struct Entrypoint {
    /// The entrypoint name = the Modal object TAG.
    pub name: String,
    /// Owned per-entrypoint Modal options.
    pub options: FunctionOptions,
    /// The effective function timeout (seconds), already resolved (decorator override
    /// applied over the path default by the caller).
    pub timeout_secs: u32,
}

/// All path-independent inputs [`provision`] threads. The [`Boundary`] decides the
/// few run/deploy-divergent choices; everything here is shared.
pub(crate) struct ProvisionInputs<'a> {
    /// App name (the ephemeral RUN name, or the persistent DEPLOY name).
    pub app_name: &'a str,
    /// Pre-resolved app id, when the app already exists (the RUN path creates the
    /// ephemeral app at connect time). `None` ⇒ the [`ControlPlane`] resolves it.
    pub app_id: Option<&'a str>,
    /// Source-upload root + cargo package + scoping/ignore knobs.
    pub source: SourceInputs<'a>,
    /// Base registry image tag.
    pub base_image: &'a str,
    /// Install the Rust toolchain (rustup) + CUDA env into the image (a non-Rust
    /// base, e.g. a CUDA-devel Tier-1 base).
    pub install_rust: bool,
    /// Ordered image-builder steps (`apt_install` / `pip_install` / `run_commands`,
    /// PARITY.md §3) rendered into the image dockerfile AFTER provisioning and BEFORE
    /// the wrapper bake. Applies to BOTH paths: the RUN single layer and the DEPLOY
    /// BASE layer (so the top layer's `cargo build` inherits the deps). Empty by
    /// default ⇒ byte-identical default path.
    pub image_steps: &'a [crate::remote::ImageStep],
    /// Enable the P6 cargo build cache (RUN path only; DEPLOY ignores it — it builds
    /// at image-build time).
    pub cache: bool,
    /// OPT-IN: degrade a FAILED memory-snapshot prime to lazy `#[enter]` instead of
    /// failing the container init loudly (DEPLOY path only; bakes
    /// `ENV MODAL_RUST_SNAPSHOT_BEST_EFFORT=1` next to the prime ENV). Default `false`
    /// = STRICT: a broken `#[enter]`/prime fails the deploy visibly rather than hiding
    /// as a per-cold-start perf cliff. From [`crate::DeployConfig::snapshot_best_effort`]
    /// (env `MODAL_RUST_SNAPSHOT_BEST_EFFORT`).
    pub snapshot_best_effort: bool,
    /// DEFAULT ON (DEPLOY path only): insert a dependency-PREBUILD image layer between
    /// the BASE and TOP layers that COPYs a SYNTHESIZED stub source tree and `cargo
    /// build`s the dependency closure ONCE, so warm redeploys hit Modal's
    /// content-addressed layer cache. When `false` — or when no stub mount can be built
    /// (cargo scoping off / metadata unavailable) — [`provision`] renders the EXACT
    /// historical two-layer proto. The RUN path never reads it. From
    /// [`crate::DeployConfig::dep_prebuild`] (env `MODAL_RUST_DEP_PREBUILD`).
    pub dep_prebuild: bool,
    /// The entrypoints to create (one DISTINCT Modal function per entrypoint).
    pub entrypoints: &'a [Entrypoint],
}

/// The source-upload inputs (shared by the RUN source mount + the DEPLOY build
/// context — same scoping + ignore resolution).
pub(crate) struct SourceInputs<'a> {
    /// Workspace root uploaded as the source (mount root / build context root).
    pub local_root: &'a std::path::Path,
    /// Cargo package owning the entrypoints (`cargo -p <package>`).
    pub package: &'a str,
    /// Scope the upload to the target package's cargo dependency closure.
    pub use_cargo_scoping: bool,
    /// Highest-precedence ignore filename (default `.modalignore`).
    pub modalignore_name: &'a str,
    /// Where the source mount lands in-container (RUN: `/src`).
    pub remote_src: &'a str,
}

/// The pure image spec(s) for a boundary — the ONLY image divergence between run and
/// deploy. RUN renders ONE layer (rust base + add_python + the baked run wrapper, NO
/// cargo line — the build is deferred to the function body). DEPLOY renders TWO
/// layers: a base (add_python on the rust base) and a top that COPYs the source and
/// runs `cargo build --release` in a `RUN` layer, baking `/app/modal_runner`.
///
/// `mount_ids` carries the resolved mount id(s) each layer needs (RUN: the
/// python-standalone mount; DEPLOY base: the python-standalone mount; DEPLOY top: the
/// source build-context mount). The base layer id is threaded by [`provision`] after
/// the base layer builds, so DEPLOY's top layer is rendered separately via
/// [`build_deploy_top_layer_spec`].
pub(crate) fn build_image_spec(
    boundary: &Boundary,
    inputs: &ProvisionInputs<'_>,
    py_mount_id: &str,
) -> ImageSpec {
    match boundary.source_delivery {
        // RUN: rust base + add_python(standalone) + the baked run wrapper. NO cargo
        // line — the build runs in the function body at invoke time.
        SourceDelivery::RunMount => {
            let mut spec = ImageSpec::from_registry(inputs.base_image.to_string())
                .with_add_python(PYTHON_SERIES)
                .with_python_standalone_mount_id(py_mount_id.to_string());
            // `zstd` for the build-cache archive: without the binary, `tar --zstd`
            // fails and the wrapper silently degrades to gzip — several times
            // slower both packing and unpacking the (now target/-bearing) archive,
            // which dominated warm fresh-container latency. One tiny apt package.
            if inputs.cache {
                spec = spec.with_command(
                    "RUN apt-get update && apt-get install -y --no-install-recommends zstd && rm -rf /var/lib/apt/lists/*",
                );
            }
            if inputs.install_rust {
                spec = spec.with_rust_toolchain();
            }
            // User image-builder steps (apt/pip/run), in chain order. The SDK renders
            // them after provisioning and before the wrapper bake.
            spec = crate::remote::apply_image_steps(spec, inputs.image_steps);
            let mut spec = spec
                .with_wrapper_module(WRAPPER_MODULE, run_wrapper_src())
                .with_command(run_wrapper_config_env(
                    inputs.source.package,
                    inputs.cache,
                    inputs.source.remote_src,
                ))
                .with_command("ENV RUST_BACKTRACE=1");
            // target/ caching is DEFAULT ON (both here and in the wrapper — see
            // `discover_cache_target`); only an explicit local OPT-OUT must cross to
            // the container, so we bake `=0` then. The default path bakes nothing —
            // rendered image byte-identical to before the default flip.
            if inputs.cache && !crate::remote::discover_cache_target() {
                spec = spec.with_command(format!("ENV {}=0", crate::env::CACHE_TARGET));
            }
            spec.with_command("ENTRYPOINT []")
        }
        // DEPLOY: the BASE layer (add_python on the rust base). The TOP layer
        // (source COPY + cargo build) is built separately by
        // `build_deploy_top_layer_spec` once this base layer has an id.
        SourceDelivery::DeployContext => {
            let mut spec = ImageSpec::from_registry(inputs.base_image.to_string())
                .with_add_python(PYTHON_SERIES)
                .with_python_standalone_mount_id(py_mount_id.to_string());
            if inputs.install_rust {
                spec = spec.with_rust_toolchain();
            }
            // User image-builder steps ride the DEPLOY BASE layer, so the TOP layer's
            // image-build-time `cargo build` (and the deployed runtime) inherit the
            // installed deps. The top layer (COPY + cargo) is rendered separately.
            crate::remote::apply_image_steps(spec, inputs.image_steps)
        }
    }
}

/// The image-build-time `cargo build` of the runner binary, shared VERBATIM by the
/// DEPLOY top layer and the dependency-prebuild layer so the two cannot drift (a drift
/// would break dep-`.rlib` reuse — the top layer must run the same build over the
/// prebuilt `target/`). `-p` disambiguates the shared `modal_runner` bin in a
/// multi-crate closure. The RUN-path in-body build (`remote/wrapper.py`) issues the
/// SAME command in Python; keep them in sync (cross-language, can't share the literal).
fn deploy_runner_build_command(package: &str) -> String {
    format!("RUN cd {DEPLOY_SRC} && cargo build --release -p {package} --bin modal_runner")
}

/// The DEPLOY TOP layer (layer 2): bases on the add_python base layer via `FROM
/// base`, bakes the deploy wrapper, COPYs the SOURCE (this layer's build context),
/// then runs `cargo build --release` + `cp`/bake of `/app/modal_runner`. cargo runs
/// AT image-build time; the deployed runtime never repeats it. Pure (no I/O).
///
/// `bake_snapshot_prime` bakes `ENV MODAL_RUST_SNAPSHOT_PRIME=1` (next to the existing
/// `RUST_BACKTRACE` ENV) so the deploy wrapper's import-time prime block (deploy.rs §6)
/// fires before Modal's snapshot point. It is `true` only when ANY deployed entrypoint
/// opted into `enable_memory_snapshot`; when `false` the layer renders byte-identically
/// to a non-snapshot deploy (the off-path forward-safety guarantee).
///
/// `endpoint_entrypoints` (web endpoints §4) are the deployed entrypoints that declared
/// `webhook_method`: the baked wrapper module becomes [`baked_deploy_wrapper_src`] —
/// the static source plus one generated `web_<sanitized> = _make_web_handler("<ep>")`
/// adapter line per endpoint, the FILE-mode `implementation_name` target. Empty ⇒ the
/// plain static wrapper source, byte-identical (the same off-path guarantee).
pub(crate) fn build_deploy_top_layer_spec(
    base_image_id: &str,
    source_mount_id: &str,
    package: &str,
    bake_snapshot_prime: bool,
    bake_snapshot_best_effort: bool,
    endpoint_entrypoints: &[&str],
    web_server_entrypoints: &[(&str, u16)],
) -> ImageSpec {
    let mut spec = ImageSpec::from_registry(String::new()) // FROM replaced by `FROM base`.
        .with_base_image(base_image_id)
        .with_wrapper_module(
            DEPLOY_WRAPPER_MODULE,
            baked_deploy_wrapper_src(endpoint_entrypoints, web_server_entrypoints),
        )
        .with_context_mount(source_mount_id)
        // Context root → /, so the /app/src-prefixed tree lands at /app/src.
        .with_command("COPY . /")
        // cargo build AT IMAGE-BUILD time; shared verbatim with the dep-prebuild layer.
        .with_command(deploy_runner_build_command(package))
        // Bake the freshly built binary to the fixed path the deployed body execs.
        .with_command(format!(
            "RUN cp {DEPLOY_SRC}/target/release/modal_runner {DEPLOY_RUNNER} \
             && chmod +x {DEPLOY_RUNNER}"
        ))
        .with_command("ENV RUST_BACKTRACE=1");
    // Snapshot prime is opt-in: only baked when a deployed entrypoint enabled memory
    // snapshot, so the default deploy image is byte-identical (no extra ENV layer).
    if bake_snapshot_prime {
        spec = spec.with_command(format!("ENV {}=1", crate::env::SNAPSHOT_PRIME));
        // STRICT by default: a failed prime FAILS the container init loudly (a hidden
        // perf cliff otherwise). The operator opts into degrade-to-lazy explicitly
        // (DeployConfig::snapshot_best_effort / MODAL_RUST_SNAPSHOT_BEST_EFFORT).
        if bake_snapshot_best_effort {
            spec = spec.with_command(format!("ENV {}=1", crate::env::SNAPSHOT_BEST_EFFORT));
        }
    }
    spec.with_command("ENTRYPOINT []")
}

/// The DEPLOY dependency-PREBUILD layer (layer 1, inserted between BASE and TOP when
/// `dep_prebuild` is ON): bases on the add_python base layer via `FROM base`, COPYs the
/// SYNTHESIZED STUB source tree (this layer's build context — manifests + `Cargo.lock` +
/// empty stub sources, NO real source), then runs `cargo build --release -p {package}
/// --bin modal_runner` under [`DEPLOY_SRC`]. That compiles the entire heavy git/registry
/// dependency closure ONCE; because the stub's content is invariant to real source edits,
/// Modal's content-addressed layer cache hits this layer on every warm redeploy, so the
/// dep closure is built once and skipped thereafter. The TOP layer then bases on THIS
/// layer's image and COPYs the REAL source over the stub at the SAME `{DEPLOY_SRC}` path,
/// so cargo reuses the prebuilt dep `.rlib`s in `{DEPLOY_SRC}/target/release/deps` and
/// only recompiles the changed leaf crate.
///
/// The stub bin `modal_runner` exists (synthesized empty-`fn main(){}` or the injected
/// runner), so `--bin modal_runner` resolves and the whole dep graph compiles. NO wrapper
/// module is baked (this layer only compiles deps), NO snapshot/endpoint ENV (those ride
/// the TOP layer). Pure (no I/O).
pub(crate) fn build_deploy_dep_layer_spec(
    base_image_id: &str,
    stub_mount_id: &str,
    package: &str,
) -> ImageSpec {
    ImageSpec::from_registry(String::new()) // FROM replaced by `FROM base`.
        .with_base_image(base_image_id)
        .with_context_mount(stub_mount_id)
        // Context root → /, so the /app/src-prefixed stub tree lands at /app/src.
        .with_command("COPY . /")
        // Compile the dep closure ONCE against the stub sources at image-build time.
        .with_command(deploy_runner_build_command(package))
        .with_command("ENTRYPOINT []")
}

/// Build the FILE-mode [`FunctionSpec`] for one entrypoint — the ONLY function-create
/// divergence between run and deploy. RUN: the run wrapper module, the client +
/// source mounts, the cargo-cache + user volumes, secrets, the entrypoint timeout.
/// DEPLOY: the deploy wrapper module, the CLIENT mount ONLY (the binary is baked in
/// the image layer — that absence IS the deploy invariant), user volumes, secrets.
///
/// Object TAG = the entrypoint (unique per app, its own config); the IN-CONTAINER
/// callable stays the shared dispatch `"handler"` (rolled onto `implementation_name`
/// by the SDK builder), so divergent per-entrypoint configs COEXIST.
/// The per-entrypoint resolved resources [`build_function_spec`] attaches: the
/// resolved mount id(s), the cargo-cache volume (RUN only), the user volumes, and the
/// secret ids. Bundled so the builder stays a small two-argument call.
struct AttachedResources {
    /// The hosted modal-client mount id (attached on BOTH paths).
    client_mount_id: String,
    /// The uploaded source mount id (attached on the RUN path only).
    source_mount_id: String,
    /// The P6 cargo-cache volume id, attached at `/cache` (RUN + caching-on only).
    cache_vol_id: Option<String>,
    /// Resolved secret ids → `Function.secret_ids`.
    secret_ids: Vec<String>,
    /// User volume mounts as `(volume_id, mount_path)` pairs.
    user_volume_mounts: Vec<(String, String)>,
}

fn build_function_spec(
    boundary: &Boundary,
    ep: &Entrypoint,
    image_id: &str,
    res: &AttachedResources,
) -> Result<FunctionSpec> {
    let object_tag = crate::remote::sanitize_object_tag(&ep.name);
    // Web endpoint AND web server are DEPLOY-only in v0 (D5: the URL is deploy-only), so
    // build the webhook spec ONLY on the DEPLOY boundary. RUN stays wire-identical even
    // when decorated — exactly like `enable_memory_snapshot` below.
    //
    // A `#[web_server]` (`web_server_port` set) takes precedence and produces a
    // WEB_SERVER raw-port-proxy webhook (Modal's `WEBHOOK_TYPE_WEB_SERVER`); an
    // `#[endpoint]` (`webhook_method` set) produces the FUNCTION request/response webhook.
    let webhook = match (
        ep.options.web_server_port,
        &ep.options.webhook_method,
        boundary.app_state,
    ) {
        (Some(port), _, AppState::Deployed) => Some(WebhookSpec {
            method: String::new(), // a raw port proxy has no per-request HTTP method
            requires_proxy_auth: ep.options.webhook_requires_proxy_auth,
            web_server_port: Some(port),
            web_server_startup_timeout: ep.options.web_server_startup_timeout,
        }),
        (None, Some(method), AppState::Deployed) => Some(WebhookSpec {
            method: method.clone(),
            requires_proxy_auth: ep.options.webhook_requires_proxy_auth,
            ..Default::default()
        }),
        _ => None,
    };
    let is_web_server = webhook
        .as_ref()
        .is_some_and(|w| w.web_server_port.is_some());
    let (module, callable, mount_ids) = match boundary.source_delivery {
        SourceDelivery::RunMount => (
            WRAPPER_MODULE,
            WRAPPER_CALLABLE.to_string(),
            vec![res.client_mount_id.clone(), res.source_mount_id.clone()],
        ),
        // DEPLOY attaches the CLIENT mount ONLY (no source mount).
        SourceDelivery::DeployContext => {
            // A web-exposed entrypoint's FILE-mode callable is its PER-ENTRYPOINT adapter
            // (generated into the baked deploy wrapper) instead of the shared `handler`
            // dispatch:
            // - ENDPOINT (FUNCTION webhook): `web_<sanitized>` — Modal introspects the
            //   callable's `(request)` signature, so each endpoint needs its own adapter.
            // - WEB_SERVER (raw port proxy): `web_server_<sanitized>` — invoked ONCE at
            //   container start to `Popen` the `--web-server --port` runner, then return.
            // The object TAG below stays the entrypoint name, so the typed
            // FunctionGet-by-tag path resolves the SAME function. Non-web entrypoints
            // (webhook None) keep the shared dispatch — byte-identical.
            let callable = if is_web_server {
                crate::remote::web_server_attr(&ep.name)
            } else if webhook.is_some() {
                crate::remote::web_endpoint_attr(&ep.name)
            } else {
                DEPLOY_WRAPPER_CALLABLE.to_string()
            };
            (
                DEPLOY_WRAPPER_MODULE,
                callable,
                vec![res.client_mount_id.clone()],
            )
        }
    };
    let mut fn_spec = FunctionSpec::new(module, callable, image_id)
        .with_app_function_name(&object_tag)
        .with_mount_ids(mount_ids)
        .with_mount_client_dependencies(true)
        .with_timeout_secs(ep.timeout_secs)
        .with_gpu(ep.options.gpu.clone())?
        // cpu/memory ride into FunctionResources (milli_cpu/memory_mb). `None` leaves
        // the server default (0), so an unset decorator stays wire-identical.
        .with_milli_cpu(ep.options.milli_cpu)
        .with_memory_mb(ep.options.memory_mb)
        // Per-container input concurrency rides into the top-level Function scalars
        // (proto fields 34 + 64), NOT AutoscalerSettings. All-`None` leaves both at 0,
        // so an unset decorator is byte-identical; target > max / max == 0 are rejected
        // up front (mirrors `with_autoscaler`).
        .with_concurrency(
            ep.options.max_concurrent_inputs,
            ep.options.target_concurrent_inputs,
        )?
        // schedule rides into Function.schedule (Cron/Period). `None` leaves the field
        // unset, so an unset decorator is byte-identical; a malformed spec is rejected
        // up front (mirrors `with_gpu`).
        .with_schedule(ep.options.schedule.as_deref())?
        // autoscaling rides into Function.autoscaler_settings (+ the legacy mirror
        // fields). An all-`None` autoscaler emits nothing, so an unset decorator is
        // byte-identical; invalid bounds (max < min, scaledown_window == 0) are
        // rejected up front (mirrors `with_gpu`/`with_schedule`).
        .with_autoscaler(FunctionAutoscaler {
            min_containers: ep.options.min_containers,
            max_containers: ep.options.max_containers,
            buffer_containers: ep.options.buffer_containers,
            scaledown_window: ep.options.scaledown_window,
        })?
        // Memory snapshot is DEPLOY-only (Modal snapshots deployed apps), so set
        // `checkpointing_enabled` ONLY when this is the DEPLOY boundary AND the
        // entrypoint opted in. RUN stays wire-identical even if the decorator opts in.
        .with_memory_snapshot(
            ep.options.enable_memory_snapshot && boundary.app_state == AppState::Deployed,
        )
        // The deploy-only webhook spec built above (`None` on RUN / non-endpoints ⇒
        // no `webhook_config` on the wire AND the formats stay PICKLE/CBOR —
        // byte-identical to before web endpoints).
        .with_webhook(webhook)?;
    // retries ride into Function.retry_policy. The `Retries(..)` STRUCT form (custom
    // backoff/delays, `retries_spec`) wins when present; otherwise the bare-int
    // `retries` fixed-interval shortcut applies. Both `None` ⇒ no policy ⇒ byte-identical
    // to before. The two are mutually exclusive (the macro emits at most one), but
    // `retries_spec` is checked FIRST so a struct form is never shadowed.
    fn_spec = match ep.options.retries_spec.as_deref() {
        Some(spec) => fn_spec.with_retry_policy(Some(spec))?,
        None => fn_spec.with_retries(ep.options.retries),
    };
    // P6 cargo-cache volume at /cache (RUN only; `cache_vol_id` is None otherwise).
    if let Some(vid) = &res.cache_vol_id {
        fn_spec = fn_spec.with_volume_mount(vid.clone(), CACHE_MOUNT);
    }
    // USER volumes at their DISTINCT mount paths (coexist with the cargo cache).
    for (vid, mount_path) in &res.user_volume_mounts {
        fn_spec = fn_spec.with_volume_mount(vid.clone(), mount_path.clone());
    }
    // USER secrets → Function.secret_ids (Modal injects key/values as ENV VARS).
    if !res.secret_ids.is_empty() {
        fn_spec = fn_spec.with_secret_ids(res.secret_ids.clone());
    }
    Ok(fn_spec)
}

/// The small, cohesive control-plane seam [`provision`] drives. Each method maps to
/// exactly ONE top-level control-plane RPC (or, for the source mount, the ONE
/// logical upload-and-finalize step the dump records as a single `MountGetOrCreate`).
/// Live-only concerns (the `ImageJoinStreaming` poll loop, retry, blob upload, the
/// per-file mount PUT/probe traffic) are HIDDEN inside [`LiveControlPlane`]'s methods
/// and never appear in this trait or in [`provision`].
///
/// `async_trait`-free: the methods return boxed futures via the `async fn in trait`
/// support; both impls are object-safe-free (we use static dispatch from
/// [`provision`]'s generic `C: ControlPlane`).
#[allow(async_fn_in_trait)]
pub(crate) trait ControlPlane {
    /// Resolve the app object and return its `app_id`. For [`AppState::Ephemeral`]
    /// the app was already created at connect time, so the live impl returns the
    /// pre-resolved id without an RPC; the recording impl records the `AppCreate`
    /// the connect-time create issued. For [`AppState::Deployed`] this issues
    /// `AppGetOrCreate` (persistent, create-if-missing).
    async fn ensure_app(
        &mut self,
        app_name: &str,
        pre_resolved: Option<&str>,
        state: AppState,
    ) -> Result<String>;

    /// Resolve a Volume by name (create-if-missing) → `volume_id`. `v2` selects the
    /// V2 filesystem (the cargo cache); `false` ⇒ V1 (a user volume).
    async fn ensure_volume(&mut self, name: &str, v2: bool) -> Result<String>;

    /// Resolve a named Secret by `from_name` lookup → `secret_id`. `required_keys` are
    /// asserted-present keys on the secret (empty = no assertion); the server errors if
    /// a key is missing. Mirrors `Secret.from_name(.., required_keys=[..])`.
    async fn ensure_secret(&mut self, name: &str, required_keys: &[String]) -> Result<String>;

    /// Resolve an INLINE Secret from a `{key: value}` env map (`#[function(env = {..})]`)
    /// by `from_dict` (CREATE_IF_MISSING, idempotent) → `secret_id`. `name` is the
    /// deterministic per-entrypoint deployment name the facade derives. Mirrors
    /// `Secret.from_dict(env)`; the resulting id rides into the SAME `secret_ids` list.
    async fn ensure_inline_secret(
        &mut self,
        name: &str,
        env: &[(String, String)],
    ) -> Result<String>;

    /// Resolve the hosted modal-client mount → `mount_id`.
    async fn ensure_client_mount(&mut self) -> Result<String>;

    /// Upload + finalize the user's source as an ephemeral mount → `mount_id`. The
    /// per-file PUT/probe traffic + blob upload live INSIDE this method (live impl).
    /// `app_id` is the resolved app id (RUN passes the ephemeral id; DEPLOY passes
    /// the persistent id — only used by the live source upload's environment scoping).
    async fn ensure_source_mount(
        &mut self,
        source: &SourceInputs<'_>,
        remote_path: &str,
    ) -> Result<String>;

    /// Upload + finalize the dependency-PREBUILD STUB context as an ephemeral inline-only
    /// mount → `Some(mount_id)`, or `None` when no stub can be synthesized (cargo scoping
    /// off / metadata unavailable / target not a workspace member) so [`provision`] falls
    /// back to the historical two-layer deploy. The stub carries manifests, `Cargo.lock`,
    /// and synthesized empty stub sources ONLY (NO real source), at the SAME
    /// workspace-relative layout + `remote_path` as the source mount so the prebuilt dep
    /// `.rlib`s land where the TOP layer's COPY-over reuses them.
    async fn ensure_prebuild_mount(
        &mut self,
        source: &SourceInputs<'_>,
        remote_path: &str,
    ) -> Result<Option<String>>;

    /// Resolve the hosted python-build-standalone mount for `series` → `mount_id`.
    async fn ensure_python_mount(&mut self, series: &str) -> Result<String>;

    /// Build (or fetch) the image for `spec` under `app_id` → `image_id`. The
    /// `ImageJoinStreaming` poll loop + reconnect/retry live INSIDE this method (live
    /// impl). `layer` is the layer ordinal for the dump's projection (0 = base/run,
    /// 1 = deploy top).
    async fn ensure_image(&mut self, app_id: &str, spec: &ImageSpec, layer: u8) -> Result<String>;

    /// `FunctionPrecreate` under the per-entrypoint object tag → precreate id.
    async fn precreate(&mut self, app_id: &str, object_tag: &str) -> Result<String>;

    /// `FunctionCreate` (FILE mode) → [`Created`] (ids + the served `web_url` for
    /// webhook functions; empty otherwise / on the recording plane).
    async fn create(
        &mut self,
        app_id: &str,
        precreate_id: &str,
        spec: &FunctionSpec,
    ) -> Result<Created>;

    /// `AppPublish` the cumulative `(function_ids, definition_ids)` union in `state`.
    /// Returns the deployed app URL (empty for ephemeral / recording).
    async fn publish(
        &mut self,
        app_id: &str,
        app_name: &str,
        function_ids: HashMap<String, String>,
        definition_ids: HashMap<String, String>,
        state: AppState,
    ) -> Result<String>;
}

/// The summary [`provision`] returns: the built (top) image id and the publish URL
/// (empty on the RUN/ephemeral + recording paths). The cumulative `function_ids`
/// union lives in the caller-threaded [`Published`]; the resolved app id is already
/// held by the caller (RUN: the connect-time id; DEPLOY: the config app name).
#[derive(Debug, Clone, Default)]
pub(crate) struct Provisioned {
    /// The built image id (RUN: the run image; DEPLOY: the top layer carrying the
    /// baked `/app/modal_runner`).
    pub image_id: String,
    /// The deployed app URL from `AppPublish` (empty for ephemeral / recording).
    pub publish_url: String,
}

/// What ONE `FunctionCreate` returned: the invokable id, the definition id (may be
/// empty), and — for webhook (web-endpoint) functions — the server-assigned `web_url`
/// from the create response's handle metadata (empty for plain functions and on the
/// recording plane, which never talks to a server).
#[derive(Debug, Clone, Default)]
pub(crate) struct Created {
    pub function_id: String,
    pub definition_id: String,
    pub web_url: String,
}

/// The cumulative set of functions published into ONE app. Because `AppPublish` is a
/// SET-STATE publish (it REPLACES the app's function set, not appends), creating a
/// second per-entrypoint function and re-publishing must carry the UNION of every
/// function created so far — otherwise the second publish would de-invoke the first.
///
/// The DEPLOY path fills this once across its per-entrypoint loop. The RUN path
/// provisions ONE entrypoint per [`provision`] call (memoized per entrypoint by the
/// caller) and threads the SAME accumulator across calls, so the cumulative union
/// spans the whole ephemeral app's lifetime.
#[derive(Debug, Default)]
pub(crate) struct Published {
    /// Object tag (sanitized entrypoint) → invokable `function_id`.
    pub function_ids: HashMap<String, String>,
    /// `function_id` → `definition_id` (only for functions that returned one).
    pub definition_ids: HashMap<String, String>,
    /// Object tag → served `web_url` (only for webhook functions that returned one).
    /// BTreeMap so callers print endpoint URLs in a deterministic order.
    pub endpoint_urls: std::collections::BTreeMap<String, String>,
}

impl Published {
    fn record(&mut self, object_tag: &str, created: &Created) {
        self.function_ids
            .insert(object_tag.to_string(), created.function_id.clone());
        if !created.definition_id.is_empty() {
            self.definition_ids
                .insert(created.function_id.clone(), created.definition_id.clone());
        }
        if !created.web_url.is_empty() {
            self.endpoint_urls
                .insert(object_tag.to_string(), created.web_url.clone());
        }
    }
}

/// The ONE provisioning sequence — written once, generic over the [`ControlPlane`].
///
/// Run = `provision(LiveControlPlane, …, RUN_BOUNDARY)`; deploy =
/// `provision(LiveControlPlane, …, DEPLOY_BOUNDARY)`; dump =
/// `provision(RecordingControlPlane, …)`. The only run/deploy divergence is the
/// [`Boundary`] + the pure [`build_image_spec`] / [`build_function_spec`] choices it
/// makes; response threading (`let id = cp.x().await?`) is free in this imperative
/// body, so there is no step-enum interpreter and no placeholder ids.
///
/// `published` is the cumulative publish union (object tag → `function_id`), seeded
/// by the caller. The RUN path threads ONE accumulator across its per-entrypoint
/// calls so each re-publish carries every prior function; DEPLOY passes a fresh one.
/// On return, `published.function_ids` holds the full union (the caller picks the
/// id(s) it needs — RUN memoizes per entrypoint; DEPLOY re-resolves by name).
pub(crate) async fn provision<C: ControlPlane>(
    cp: &mut C,
    inputs: &ProvisionInputs<'_>,
    boundary: &Boundary,
    published: &mut Published,
) -> Result<Provisioned> {
    // The RUN path resolves the app FIRST (it was created at connect time), then the
    // cargo cache + run-scoped secrets/user-volumes, then the mounts. The DEPLOY path
    // resolves the mounts first, then the app, then resolves secrets/user-volumes
    // PER ENTRYPOINT inside the create loop. That ordering difference (plus the
    // publish cadence) is the [`ProvisionPlan`] the boundary owns; the sequence below
    // consults the plan, each arm delegating to the SAME `ControlPlane` methods — no
    // duplicated request building.
    let plan = boundary.plan(inputs.entrypoints)?;

    // Per-run resources resolved once before the create loop (RUN only): the cargo
    // cache + the single RUN entrypoint's secrets/user-volumes, in the live wire order.
    let mut cache_vol_id: Option<String> = None;
    let mut run_secret_ids: Vec<String> = Vec::new();
    let mut run_user_volume_mounts: Vec<(String, String)> = Vec::new();

    // App (RUN: ephemeral, resolved from the connect-time id; DEPLOY: persistent
    // get-or-create AFTER the mounts — so the Single arm resolves it here, Many later).
    let app_id = if let ProvisionPlan::Single(ep) = plan {
        let app_id = cp
            .ensure_app(inputs.app_name, inputs.app_id, boundary.app_state)
            .await?;
        // Cargo-cache volume (RUN only), when caching is on.
        if inputs.cache {
            cache_vol_id = Some(cp.ensure_volume(CACHE_VOLUME_NAME, true).await?);
        }
        // Run-level secrets + user volumes (the single RUN entrypoint's config). Named
        // secrets carry `required_keys`; a non-empty inline `env` adds one more id.
        let object_tag = crate::remote::sanitize_object_tag(&ep.name);
        run_secret_ids =
            resolve_entrypoint_secrets(cp, inputs.app_name, &object_tag, &ep.options).await?;
        for (mount_path, name) in &ep.options.volumes {
            reject_cache_collision(inputs.cache, mount_path)?;
            let vid = cp.ensure_volume(name, false).await?;
            run_user_volume_mounts.push((vid, mount_path.clone()));
        }
        Some(app_id)
    } else {
        None
    };

    // --- Mounts (common): client, source, python-standalone. ---
    let client_mount_id = cp.ensure_client_mount().await?;
    let source_remote = match boundary.source_delivery {
        SourceDelivery::RunMount => inputs.source.remote_src,
        SourceDelivery::DeployContext => DEPLOY_SRC,
    };
    let source_mount_id = cp
        .ensure_source_mount(&inputs.source, source_remote)
        .await?;
    let py_mount_id = cp.ensure_python_mount(PYTHON_SERIES).await?;

    // DEPLOY resolves the persistent app AFTER the mounts (RUN already has its id).
    let app_id = match app_id {
        Some(id) => id,
        None => {
            cp.ensure_app(inputs.app_name, inputs.app_id, boundary.app_state)
                .await?
        }
    };

    // --- Image (the build boundary): one layer (RUN) or two (DEPLOY). ---
    let base_spec = build_image_spec(boundary, inputs, &py_mount_id);
    let image_id = match boundary.source_delivery {
        SourceDelivery::RunMount => cp.ensure_image(&app_id, &base_spec, 0).await?,
        SourceDelivery::DeployContext => {
            let base_image_id = cp.ensure_image(&app_id, &base_spec, 0).await?;
            // Dependency-PREBUILD layer (default ON): compile the heavy dep closure ONCE
            // against a synthesized stub so warm redeploys hit the content-addressed
            // cache and the TOP layer only recompiles the changed leaf. The TOP layer
            // then bases on THIS image. When the flag is OFF — or no stub can be built
            // (cargo scoping off / metadata unavailable) — `top_base` stays the BASE
            // image and the rendered proto is the EXACT historical two-layer sequence.
            let top_base = if inputs.dep_prebuild {
                match cp
                    .ensure_prebuild_mount(&inputs.source, source_remote)
                    .await?
                {
                    Some(stub_mount_id) => {
                        let dep_spec = build_deploy_dep_layer_spec(
                            &base_image_id,
                            &stub_mount_id,
                            inputs.source.package,
                        );
                        cp.ensure_image(&app_id, &dep_spec, 1).await?
                    }
                    None => base_image_id.clone(),
                }
            } else {
                base_image_id.clone()
            };
            // The TOP layer's ordinal is 2 when the prebuild layer rode (dump projection
            // only; the live impl ignores it), else 1 — preserving the historical
            // off-path ordinal sequence.
            let top_layer_ordinal = if top_base == base_image_id { 1 } else { 2 };
            // The deploy-only image gates (snapshot prime ENV, endpoint adapter
            // lines), computed purely by [`deploy_gates`] next to [`DEPLOY_BOUNDARY`].
            // Reached only on the DEPLOY boundary, so the deploy app_state is implied;
            // both gates off ⇒ byte-identical image (the off-path guarantee).
            let gates = deploy_gates(inputs.entrypoints);
            let top_spec = build_deploy_top_layer_spec(
                &top_base,
                &source_mount_id,
                inputs.source.package,
                gates.bake_snapshot_prime,
                inputs.snapshot_best_effort,
                &gates.endpoint_entrypoints,
                &gates.web_server_entrypoints,
            );
            cp.ensure_image(&app_id, &top_spec, top_layer_ordinal)
                .await?
        }
    };

    // --- Per-entrypoint Precreate + Create, cumulative publish. ---
    let mut publish_url = String::new();
    for ep in inputs.entrypoints {
        let object_tag = crate::remote::sanitize_object_tag(&ep.name);
        // Precreate under the PER-ENTRYPOINT object tag (NOT the shared callable).
        let precreate_id = cp.precreate(&app_id, &object_tag).await?;

        // DEPLOY resolves secrets + user volumes PER ENTRYPOINT, here in the loop.
        // RUN reuses the run-level resources resolved above (its single entrypoint).
        let (cache_for_ep, secret_ids, user_volume_mounts) = match plan {
            ProvisionPlan::Single(_) => (
                cache_vol_id.clone(),
                run_secret_ids.clone(),
                run_user_volume_mounts.clone(),
            ),
            ProvisionPlan::Many => {
                // Named secrets (with `required_keys`) + the inline `env` secret, resolved
                // PER ENTRYPOINT here in the loop (the inline name keys on this object tag).
                let secret_ids =
                    resolve_entrypoint_secrets(cp, inputs.app_name, &object_tag, &ep.options)
                        .await?;
                let mut user_volume_mounts: Vec<(String, String)> =
                    Vec::with_capacity(ep.options.volumes.len());
                for (mount_path, name) in &ep.options.volumes {
                    let vid = cp.ensure_volume(name, false).await?;
                    user_volume_mounts.push((vid, mount_path.clone()));
                }
                (None, secret_ids, user_volume_mounts)
            }
        };

        let res = AttachedResources {
            client_mount_id: client_mount_id.clone(),
            source_mount_id: source_mount_id.clone(),
            cache_vol_id: cache_for_ep,
            secret_ids,
            user_volume_mounts,
        };
        let fn_spec = build_function_spec(boundary, ep, &image_id, &res)?;
        let created = cp.create(&app_id, &precreate_id, &fn_spec).await?;
        published.record(&object_tag, &created);

        // RUN (Single) re-publishes the CUMULATIVE union after EACH create —
        // `AppPublish` REPLACES the function set, so a per-entrypoint create must
        // re-publish every prior one too (across calls, via the threaded `published`)
        // or it would de-invoke them. DEPLOY (Many) publishes ONCE after the loop.
        if matches!(plan, ProvisionPlan::Single(_)) {
            publish_url = cp
                .publish(
                    &app_id,
                    inputs.app_name,
                    published.function_ids.clone(),
                    published.definition_ids.clone(),
                    boundary.app_state,
                )
                .await?;
        }
    }

    // DEPLOY (Many): one persistent publish carrying the UNION of every entrypoint fn.
    if matches!(plan, ProvisionPlan::Many) {
        publish_url = cp
            .publish(
                &app_id,
                inputs.app_name,
                published.function_ids.clone(),
                published.definition_ids.clone(),
                boundary.app_state,
            )
            .await?;
    }

    Ok(Provisioned {
        image_id,
        publish_url,
    })
}

/// Derive the DETERMINISTIC deployment name for an entrypoint's INLINE secret
/// (`#[function(env = {..})]`). Keyed on the app name + the (sanitized) entrypoint so
/// re-runs of the SAME app+entrypoint resolve the SAME `Secret.from_dict`
/// (CREATE_IF_MISSING is idempotent on a stable name). Distinct entrypoints get
/// distinct inline secrets, so two functions' `env` maps never collide.
fn inline_secret_name(app_name: &str, object_tag: &str) -> String {
    format!("modal-rust-inline-env-{app_name}-{object_tag}")
}

/// Resolve an entrypoint's NAMED secrets (with `required_keys`) AND its inline `env`
/// secret into the combined `secret_ids` list (named first, then the inline id). Shared
/// by the RUN and DEPLOY arms so the resolution + ordering cannot drift. The inline
/// secret is resolved only when `env` is non-empty (so a bare entrypoint is
/// byte-identical: no extra `SecretGetOrCreate`).
async fn resolve_entrypoint_secrets<C: ControlPlane>(
    cp: &mut C,
    app_name: &str,
    object_tag: &str,
    options: &FunctionOptions,
) -> Result<Vec<String>> {
    let mut secret_ids: Vec<String> = Vec::with_capacity(options.secrets.len() + 1);
    for name in &options.secrets {
        secret_ids.push(cp.ensure_secret(name, &options.required_keys).await?);
    }
    if !options.env.is_empty() {
        let name = inline_secret_name(app_name, object_tag);
        secret_ids.push(cp.ensure_inline_secret(&name, &options.env).await?);
    }
    Ok(secret_ids)
}

/// Reject a user volume mounted at the reserved cargo-cache path (RUN path).
fn reject_cache_collision(cache: bool, mount_path: &str) -> Result<()> {
    if cache && mount_path == CACHE_MOUNT {
        return Err(Error::config(format!(
            "user volume mount path {CACHE_MOUNT:?} collides with the cargo-cache \
             volume; choose a different mount path (or disable the cache)"
        )));
    }
    Ok(())
}

/// The LIVE control plane: the real [`ModalClient`]. Encapsulates ALL live-only
/// concerns — the `ImageJoinStreaming` poll loop + reconnect/retry inside
/// `ensure_image`, the per-file mount PUT/probe + blob upload inside
/// `ensure_source_mount` — so [`provision`] (and the [`ControlPlane`] trait) never
/// see streaming/retry/blob.
///
/// The RUN path's cumulative publish union is maintained by the caller across
/// per-entrypoint `provision` calls; this impl carries the already-merged maps the
/// sequence threads.
pub(crate) struct LiveControlPlane<'c> {
    /// The live SDK client.
    pub client: &'c mut ModalClient,
}

impl ControlPlane for LiveControlPlane<'_> {
    async fn ensure_app(
        &mut self,
        app_name: &str,
        pre_resolved: Option<&str>,
        state: AppState,
    ) -> Result<String> {
        match (state, pre_resolved) {
            // RUN: the ephemeral app was created at connect time; reuse its id (no RPC).
            (AppState::Ephemeral, Some(id)) => Ok(id.to_string()),
            // RUN with no pre-resolved id: create the ephemeral app now.
            (AppState::Ephemeral, None) => {
                Ok(self.client.app_create_ephemeral(app_name, None).await?)
            }
            // DEPLOY: persistent get-or-create (create-if-missing).
            (AppState::Deployed, _) => Ok(self.client.app_get_or_create_id(app_name, None).await?),
        }
    }

    async fn ensure_volume(&mut self, name: &str, v2: bool) -> Result<String> {
        Ok(self
            .client
            .volume_get_or_create(name, v2, true /* create */, None)
            .await?)
    }

    async fn ensure_secret(&mut self, name: &str, required_keys: &[String]) -> Result<String> {
        Ok(self
            .client
            .secret_get_or_create(name, required_keys, None)
            .await?)
    }

    async fn ensure_inline_secret(
        &mut self,
        name: &str,
        env: &[(String, String)],
    ) -> Result<String> {
        // CREATE_IF_MISSING is idempotent + retry-safe (re-running returns the same id),
        // so the deterministic per-entrypoint name makes inline env re-runs stable. The
        // VALUES are never logged (Modal/secrets rules).
        let env_map: std::collections::HashMap<String, String> = env.iter().cloned().collect();
        Ok(self.client.secret_from_dict(name, &env_map, None).await?)
    }

    async fn ensure_client_mount(&mut self) -> Result<String> {
        Ok(self.client.client_mount_id(None).await?)
    }

    async fn ensure_source_mount(
        &mut self,
        source: &SourceInputs<'_>,
        remote_path: &str,
    ) -> Result<String> {
        // PRIMARY: cargo-metadata scoping (the target package's workspace-member dep
        // closure + the workspace Cargo.toml/Cargo.lock). FALLBACK: the whole
        // local_root minus ignored files. Both prune via `.modalignore` > `.gitignore`
        // > built-in defaults (resolved in the SDK). The per-file PUT/probe + blob
        // upload are encapsulated inside these SDK calls.
        //
        // A HARD scoping error (e.g. a normal path-dep escaping the workspace, whose
        // source the upload cannot carry) is surfaced as `Error::Config` — NOT swallowed
        // into the whole-root fallback, which would fail the same cryptic way remotely.
        let scoped = if source.use_cargo_scoping {
            crate::scope::workspace_closure(source.local_root, source.package)
                .map_err(Error::config)?
        } else {
            None
        };
        match (source.use_cargo_scoping, scoped) {
            (true, Some(closure)) => {
                let spec = modal_rust_sdk::WorkspaceClosureSpec {
                    workspace_root: source.local_root,
                    crate_dirs: &closure.dirs,
                    extra_files: &closure.extra_files,
                    extra_inline_files: &closure.inline_files,
                    modalignore_name: source.modalignore_name,
                };
                Ok(self
                    .client
                    .mount_workspace_closure(&spec, remote_path, None)
                    .await?)
            }
            _ => {
                // FALLBACK whole-dir upload (no cargo scoping or metadata unavailable).
                // Still inject the tooling-generated `modal_runner` bin when the target
                // is generatable (this path runs its own `cargo metadata` via
                // `injected_runner_file`; on the mock's synthetic no-facade crate it
                // returns `None`, so the wire stays frozen). Auto-detect crates and
                // facade-less crates inject nothing.
                let inline =
                    crate::runner_gen::injected_runner_file(source.local_root, source.package)
                        .map(|f| vec![f])
                        .unwrap_or_default();
                Ok(self
                    .client
                    .mount_local_dir_with_inline(
                        source.local_root,
                        remote_path,
                        source.modalignore_name,
                        None,
                        &inline,
                    )
                    .await?)
            }
        }
    }

    async fn ensure_prebuild_mount(
        &mut self,
        source: &SourceInputs<'_>,
        remote_path: &str,
    ) -> Result<Option<String>> {
        // The stub mount is built ONLY from cargo-metadata scoping (it needs the closure
        // manifests + targets). Without scoping there is no stub → caller falls back to
        // the historical two-layer deploy. A HARD out-of-workspace error is surfaced
        // (same contract as `ensure_source_mount`), NOT swallowed.
        if !source.use_cargo_scoping {
            return Ok(None);
        }
        let stub = crate::scope::workspace_closure_stub(source.local_root, source.package)
            .map_err(Error::config)?;
        let Some(stub) = stub else {
            return Ok(None);
        };
        // INLINE-ONLY mount: empty `crate_dirs` so the SDK walks NO on-disk source; all
        // stub sources + manifests ride `extra_inline_files`, the verbatim Cargo.lock
        // rides `extra_files`. Same workspace-relative layout + remote_path as the source
        // mount (so the prebuilt deps land at the TOP layer's COPY-over path).
        let spec = modal_rust_sdk::WorkspaceClosureSpec {
            workspace_root: source.local_root,
            crate_dirs: &[],
            extra_files: &stub.extra_files,
            extra_inline_files: &stub.inline_files,
            modalignore_name: source.modalignore_name,
        };
        Ok(Some(
            self.client
                .mount_workspace_closure(&spec, remote_path, None)
                .await?,
        ))
    }

    async fn ensure_python_mount(&mut self, series: &str) -> Result<String> {
        Ok(self.client.python_standalone_mount_id(series, None).await?)
    }

    async fn ensure_image(&mut self, app_id: &str, spec: &ImageSpec, _layer: u8) -> Result<String> {
        // The ImageJoinStreaming poll loop + reconnect/retry are encapsulated inside
        // the SDK's image_get_or_create — never surfaced to provision().
        Ok(self.client.image_get_or_create(app_id, spec).await?)
    }

    async fn precreate(&mut self, app_id: &str, object_tag: &str) -> Result<String> {
        Ok(self.client.function_precreate(app_id, object_tag).await?)
    }

    async fn create(
        &mut self,
        app_id: &str,
        precreate_id: &str,
        spec: &FunctionSpec,
    ) -> Result<Created> {
        let created = self
            .client
            .function_create(app_id, precreate_id, spec)
            .await?;
        Ok(Created {
            function_id: created.function_id,
            definition_id: created.definition_id,
            web_url: created.web_url,
        })
    }

    async fn publish(
        &mut self,
        app_id: &str,
        app_name: &str,
        function_ids: HashMap<String, String>,
        definition_ids: HashMap<String, String>,
        state: AppState,
    ) -> Result<String> {
        let published = match state {
            AppState::Ephemeral => {
                self.client
                    .app_publish_ephemeral(app_id, app_name, function_ids, definition_ids)
                    .await?
            }
            AppState::Deployed => {
                self.client
                    .app_publish_deployed(app_id, app_name, function_ids, definition_ids)
                    .await?
            }
        };
        Ok(published.url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FunctionOptions;

    /// Minimal RUN inputs over a single CPU entrypoint (no network — the spec builders
    /// + Published accumulator are pure).
    fn run_inputs<'a>(entrypoints: &'a [Entrypoint], base_image: &'a str) -> ProvisionInputs<'a> {
        ProvisionInputs {
            app_name: "app",
            app_id: Some("ap-1"),
            source: SourceInputs {
                local_root: std::path::Path::new("/ws"),
                package: "example-add",
                use_cargo_scoping: false,
                modalignore_name: ".modalignore",
                remote_src: "/src",
            },
            base_image,
            install_rust: false,
            image_steps: &[],
            cache: true,
            snapshot_best_effort: false,
            dep_prebuild: false,
            entrypoints,
        }
    }

    #[test]
    fn run_image_spec_provisions_python_and_has_no_cargo_build() {
        // RUN (RunMount boundary): rust base + add_python + the run wrapper, NO cargo
        // line (the build is deferred to the function body). This is the relocated
        // deploy-base/run-image coverage, now asserted against the canonical builder.
        let eps = [Entrypoint {
            name: "add".to_string(),
            options: FunctionOptions::default(),
            timeout_secs: 1800,
        }];
        let inputs = run_inputs(&eps, "rust:1-slim");
        let spec = build_image_spec(&RUN_BOUNDARY, &inputs, "mo-py-standalone");
        assert_eq!(spec.base_image, "rust:1-slim");
        assert_eq!(spec.add_python.as_deref(), Some("3.12"));
        assert_eq!(
            spec.python_standalone_mount_id.as_deref(),
            Some("mo-py-standalone")
        );
        // No apt/pip fallback; no rust install on the default base.
        assert!(spec.pre_bake_commands.is_empty());
        assert!(!spec.pip_install_modal);
        assert!(!spec.install_rust, "default base layer has no rust install");
        // RUN image builds in-body — NO cargo build at image-build time.
        assert!(
            !spec
                .extra_commands
                .iter()
                .any(|c| c.contains("cargo build")),
            "RUN image must not carry a cargo build line"
        );
    }

    #[test]
    fn deploy_base_layer_installs_rust_for_cuda_base() {
        // CUDA Tier-1 deploy base: a `nvidia/cuda:<ver>-devel` base + add_python +
        // with_rust_toolchain on the BASE layer (DeployContext boundary, layer 0).
        let eps = [Entrypoint {
            name: "add".to_string(),
            options: FunctionOptions::default(),
            timeout_secs: 300,
        }];
        let mut inputs = run_inputs(&eps, "nvidia/cuda:12.6.3-devel-ubuntu22.04");
        inputs.install_rust = true;
        let base = build_image_spec(&DEPLOY_BOUNDARY, &inputs, "mo-py-standalone");
        assert_eq!(base.base_image, "nvidia/cuda:12.6.3-devel-ubuntu22.04");
        assert_eq!(base.add_python.as_deref(), Some("3.12"));
        assert!(base.install_rust, "CUDA base layer installs rust");
    }

    #[test]
    fn deploy_top_layer_rides_source_on_the_build_context() {
        // DEPLOY top layer: bases on layer 1 (FROM base), the source rides this
        // layer's build CONTEXT so cargo compiles it AT image-build time, then the
        // binary is baked to /app/modal_runner.
        let spec = build_deploy_top_layer_spec(
            "im-layer1",
            "mo-deploy-src",
            "example-add",
            false,
            false,
            &[],
            &[],
        );
        assert_eq!(spec.base_image_id.as_deref(), Some("im-layer1"));
        assert_eq!(spec.context_mount_id.as_deref(), Some("mo-deploy-src"));
        assert!(spec
            .extra_commands
            .iter()
            .any(|c| c.contains("cargo build --release -p example-add --bin modal_runner")));
        assert!(spec
            .extra_commands
            .iter()
            .any(|c| c.contains("cp /app/src/target/release/modal_runner /app/modal_runner")));
        assert!(spec.extra_commands.iter().any(|c| c.contains("COPY . /")));
        // No apt/pip on the top layer (Python is inherited from the base layer).
        assert!(spec.pre_bake_commands.is_empty());
        assert!(!spec.pip_install_modal);
        // Off-path (no snapshot entrypoint): the prime ENV is NOT baked ⇒ byte-identical.
        assert!(
            !spec
                .extra_commands
                .iter()
                .any(|c| c.contains("MODAL_RUST_SNAPSHOT_PRIME")),
            "default deploy image must not carry the snapshot-prime ENV"
        );
    }

    #[test]
    fn deploy_top_layer_bakes_snapshot_prime_env_when_enabled() {
        // A deployed entrypoint opted into memory snapshot ⇒ the top layer bakes
        // `ENV MODAL_RUST_SNAPSHOT_PRIME=1` next to `RUST_BACKTRACE`, so the wrapper's
        // import-time prime fires before Modal's snapshot point.
        let spec = build_deploy_top_layer_spec(
            "im-layer1",
            "mo-deploy-src",
            "example-add",
            true,
            false,
            &[],
            &[],
        );
        assert!(
            spec.extra_commands
                .iter()
                .any(|c| c == "ENV MODAL_RUST_SNAPSHOT_PRIME=1"),
            "snapshot deploy image must bake the prime ENV, got: {:?}",
            spec.extra_commands
        );
        // It rides alongside the existing RUST_BACKTRACE ENV (both present).
        assert!(spec
            .extra_commands
            .iter()
            .any(|c| c == "ENV RUST_BACKTRACE=1"));
        // STRICT is the default: no best-effort ENV unless the operator opted in.
        assert!(
            !spec
                .extra_commands
                .iter()
                .any(|c| c.contains("MODAL_RUST_SNAPSHOT_BEST_EFFORT")),
            "strict default must NOT bake the best-effort ENV"
        );
    }

    #[test]
    fn deploy_top_layer_bakes_best_effort_env_only_on_opt_in() {
        // The opt-in degrade (DeployConfig::snapshot_best_effort /
        // MODAL_RUST_SNAPSHOT_BEST_EFFORT) bakes the best-effort ENV NEXT TO the prime
        // ENV — and only when the prime itself is baked (it is meaningless without it).
        let spec = build_deploy_top_layer_spec(
            "im-layer1",
            "mo-deploy-src",
            "example-add",
            true,
            true,
            &[],
            &[],
        );
        assert!(spec
            .extra_commands
            .iter()
            .any(|c| c == "ENV MODAL_RUST_SNAPSHOT_PRIME=1"));
        assert!(spec
            .extra_commands
            .iter()
            .any(|c| c == "ENV MODAL_RUST_SNAPSHOT_BEST_EFFORT=1"));
        // best_effort WITHOUT the prime bakes neither (no orphan knob).
        let spec = build_deploy_top_layer_spec(
            "im-layer1",
            "mo-deploy-src",
            "example-add",
            false,
            true,
            &[],
            &[],
        );
        assert!(!spec
            .extra_commands
            .iter()
            .any(|c| c.contains("MODAL_RUST_SNAPSHOT")));
    }

    #[test]
    fn deploy_top_layer_bakes_plain_wrapper_when_no_endpoint() {
        // Off-path forward safety (web endpoints §6): NO endpoint ⇒ the baked wrapper
        // module is the plain static DEPLOY_WRAPPER_SRC, BYTE-IDENTICAL (no generated
        // adapter suffix, no trailing newline drift).
        let spec = build_deploy_top_layer_spec(
            "im-layer1",
            "mo-deploy-src",
            "example-add",
            false,
            false,
            &[],
            &[],
        );
        assert_eq!(spec.wrapper_modules.len(), 1);
        let (module, src) = &spec.wrapper_modules[0];
        assert_eq!(module, DEPLOY_WRAPPER_MODULE);
        assert_eq!(
            src,
            crate::deploy::DEPLOY_WRAPPER_SRC,
            "no-endpoint deploy must bake the static wrapper source byte-identically"
        );
    }

    #[test]
    fn deploy_top_layer_appends_web_adapter_lines_per_endpoint() {
        // Web endpoints §4: each endpoint entrypoint gets a GENERATED module-level
        // `web_<sanitized> = _make_web_handler("<entrypoint>")` line appended after
        // the static wrapper source — the FILE-mode `implementation_name` target
        // (`web_<sanitized>`, matching `remote::web_endpoint_attr`).
        let spec = build_deploy_top_layer_spec(
            "im-layer1",
            "mo-deploy-src",
            "example-add",
            false,
            false,
            &["predict", "my-fn.v2"],
            &[],
        );
        let (module, src) = &spec.wrapper_modules[0];
        assert_eq!(module, DEPLOY_WRAPPER_MODULE);
        assert!(
            src.starts_with(crate::deploy::DEPLOY_WRAPPER_SRC),
            "the static wrapper source must come first, unmodified"
        );
        assert!(
            src.contains("\nweb_predict = _make_web_handler(\"predict\")\n"),
            "per-endpoint adapter line must be generated, got tail: {:?}",
            &src[crate::deploy::DEPLOY_WRAPPER_SRC.len()..]
        );
        // The attr is sanitized to a Python identifier (dots/dashes -> `_`) while the
        // handler arg keeps the RAW entrypoint name (the dispatch key).
        assert!(src.contains("\nweb_my_fn_v2 = _make_web_handler(\"my-fn.v2\")\n"));
    }

    #[test]
    fn run_function_spec_attaches_client_and_source_mounts() {
        // RUN function: the run wrapper module + 2 mounts (client+source) + the cargo
        // cache at /cache.
        let ep = Entrypoint {
            name: "add".to_string(),
            options: FunctionOptions::default(),
            timeout_secs: 1800,
        };
        let spec = build_function_spec(
            &RUN_BOUNDARY,
            &ep,
            "im-1",
            &AttachedResources {
                client_mount_id: "mo-client".to_string(),
                source_mount_id: "mo-source".to_string(),
                cache_vol_id: Some("vo-1".to_string()),
                secret_ids: vec![],
                user_volume_mounts: vec![],
            },
        )
        .expect("run fn spec");
        assert_eq!(spec.module_name, WRAPPER_MODULE);
        assert_eq!(spec.function_name, WRAPPER_CALLABLE);
        assert_eq!(spec.object_tag(), "add");
        assert_eq!(spec.mount_ids, vec!["mo-client", "mo-source"]);
        assert_eq!(spec.timeout_secs, 1800);
        // The cargo cache mounted at /cache.
        assert_eq!(spec.volume_mounts.len(), 1);
        assert_eq!(spec.volume_mounts[0].mount_path, CACHE_MOUNT);
    }

    #[test]
    fn deploy_function_spec_attaches_client_mount_only() {
        // DEPLOY function: the deploy wrapper module + the CLIENT mount ONLY (the
        // binary is baked in the image — that absence IS the deploy invariant).
        let ep = Entrypoint {
            name: "add".to_string(),
            options: FunctionOptions {
                gpu: Some("A100".to_string()),
                ..FunctionOptions::default()
            },
            timeout_secs: 900,
        };
        let spec = build_function_spec(
            &DEPLOY_BOUNDARY,
            &ep,
            "im-1",
            &AttachedResources {
                client_mount_id: "mo-client".to_string(),
                source_mount_id: "mo-source".to_string(), // ignored on the deploy path
                cache_vol_id: None,
                secret_ids: vec![],
                user_volume_mounts: vec![],
            },
        )
        .expect("deploy fn spec");
        assert_eq!(spec.module_name, DEPLOY_WRAPPER_MODULE);
        assert_eq!(spec.function_name, DEPLOY_WRAPPER_CALLABLE);
        assert_eq!(
            spec.mount_ids,
            vec!["mo-client"],
            "deploy attaches the client mount ONLY"
        );
        assert!(spec.volume_mounts.is_empty(), "no cargo cache on deploy");
        // Off-path forward safety: a non-endpoint deploy carries NO webhook and keeps
        // the shared dispatch callable — byte-identical to before web endpoints.
        assert!(spec.webhook.is_none(), "non-endpoint deploy has no webhook");
    }

    #[test]
    fn deploy_endpoint_sets_webhook_and_per_endpoint_adapter_callable() {
        // An `#[endpoint(method = "POST")]` entrypoint on the DEPLOY boundary: the
        // webhook spec rides the FunctionSpec AND the FILE-mode callable becomes the
        // PER-ENDPOINT adapter `web_<sanitized_tag>` (the baked deploy wrapper's
        // generated attr), while the object TAG stays the entrypoint name — so the
        // typed FunctionGet-by-tag path resolves the same function (D2 dual surface).
        let ep = Entrypoint {
            name: "add".to_string(),
            options: FunctionOptions {
                webhook_method: Some("POST".to_string()),
                webhook_requires_proxy_auth: true,
                ..FunctionOptions::default()
            },
            timeout_secs: 900,
        };
        let spec = build_function_spec(
            &DEPLOY_BOUNDARY,
            &ep,
            "im-1",
            &AttachedResources {
                client_mount_id: "mo-client".to_string(),
                source_mount_id: "mo-source".to_string(),
                cache_vol_id: None,
                secret_ids: vec![],
                user_volume_mounts: vec![],
            },
        )
        .expect("deploy endpoint fn spec");
        let webhook = spec.webhook.as_ref().expect("DEPLOY endpoint sets webhook");
        assert_eq!(webhook.method, "POST");
        assert!(
            webhook.requires_proxy_auth,
            "proxy-auth opt-in rides through"
        );
        // implementation callable = the per-endpoint adapter; TAG = the entrypoint.
        assert_eq!(spec.module_name, DEPLOY_WRAPPER_MODULE);
        assert_eq!(spec.function_name, "web_add");
        assert_eq!(spec.object_tag(), "add", "object TAG stays the entrypoint");
    }

    #[test]
    fn run_endpoint_suppresses_webhook_and_stays_wire_identical() {
        // D5: the URL is DEPLOY-only. A RUN of a decorated endpoint suppresses the
        // webhook entirely — same module/callable/spec as a plain `#[function]` run.
        let options = FunctionOptions {
            webhook_method: Some("POST".to_string()),
            webhook_requires_proxy_auth: true,
            ..FunctionOptions::default()
        };
        let ep = Entrypoint {
            name: "add".to_string(),
            options,
            timeout_secs: 1800,
        };
        let res = AttachedResources {
            client_mount_id: "mo-client".to_string(),
            source_mount_id: "mo-source".to_string(),
            cache_vol_id: None,
            secret_ids: vec![],
            user_volume_mounts: vec![],
        };
        let spec = build_function_spec(&RUN_BOUNDARY, &ep, "im-1", &res).expect("run fn spec");
        assert!(
            spec.webhook.is_none(),
            "RUN suppresses the webhook even when decorated"
        );
        assert_eq!(spec.module_name, WRAPPER_MODULE);
        assert_eq!(
            spec.function_name, WRAPPER_CALLABLE,
            "RUN keeps the shared dispatch callable (wire-identical to a plain fn)"
        );
        assert_eq!(spec.object_tag(), "add");
    }

    #[test]
    fn deploy_web_server_sets_web_server_webhook_and_launcher_callable() {
        // A `#[web_server(port = 3000, startup_timeout = 30)]` entrypoint on the DEPLOY
        // boundary: the webhook spec rides the FunctionSpec carrying `web_server_port`
        // (which the SDK maps to `WEBHOOK_TYPE_WEB_SERVER`) + the startup timeout, and the
        // FILE-mode callable becomes the PER-ENTRYPOINT launcher `web_server_<tag>` while
        // the object TAG stays the entrypoint name. NO HTTP method (a raw port proxy).
        let ep = Entrypoint {
            name: "serve".to_string(),
            options: FunctionOptions {
                web_server_port: Some(3000),
                web_server_startup_timeout: Some(30),
                ..FunctionOptions::default()
            },
            timeout_secs: 1800,
        };
        let spec = build_function_spec(
            &DEPLOY_BOUNDARY,
            &ep,
            "im-1",
            &AttachedResources {
                client_mount_id: "mo-client".to_string(),
                source_mount_id: "mo-source".to_string(),
                cache_vol_id: None,
                secret_ids: vec![],
                user_volume_mounts: vec![],
            },
        )
        .expect("deploy web_server fn spec");
        let webhook = spec
            .webhook
            .as_ref()
            .expect("DEPLOY web_server sets webhook");
        assert_eq!(
            webhook.web_server_port,
            Some(3000),
            "the bound port rides the webhook (SDK ⇒ WEBHOOK_TYPE_WEB_SERVER)"
        );
        assert_eq!(webhook.web_server_startup_timeout, Some(30));
        assert!(
            webhook.method.is_empty(),
            "a raw port proxy has no per-request HTTP method"
        );
        // implementation callable = the per-entrypoint launcher; TAG = the entrypoint.
        assert_eq!(spec.module_name, DEPLOY_WRAPPER_MODULE);
        assert_eq!(spec.function_name, "web_server_serve");
        assert_eq!(
            spec.object_tag(),
            "serve",
            "object TAG stays the entrypoint"
        );
    }

    #[test]
    fn run_web_server_suppresses_webhook_and_stays_wire_identical() {
        // D5: the URL is DEPLOY-only. A RUN of a `#[web_server]` entrypoint suppresses the
        // webhook entirely — same module/callable/spec as a plain `#[function]` run
        // (byte-identical wire).
        let ep = Entrypoint {
            name: "serve".to_string(),
            options: FunctionOptions {
                web_server_port: Some(3000),
                web_server_startup_timeout: Some(30),
                ..FunctionOptions::default()
            },
            timeout_secs: 1800,
        };
        let res = AttachedResources {
            client_mount_id: "mo-client".to_string(),
            source_mount_id: "mo-source".to_string(),
            cache_vol_id: None,
            secret_ids: vec![],
            user_volume_mounts: vec![],
        };
        let spec =
            build_function_spec(&RUN_BOUNDARY, &ep, "im-1", &res).expect("run web_server fn spec");
        assert!(
            spec.webhook.is_none(),
            "RUN suppresses the web_server webhook even when decorated"
        );
        assert_eq!(spec.module_name, WRAPPER_MODULE);
        assert_eq!(
            spec.function_name, WRAPPER_CALLABLE,
            "RUN keeps the shared dispatch callable (wire-identical to a plain fn)"
        );
        assert_eq!(spec.object_tag(), "serve");
    }

    #[test]
    fn published_accumulates_union_for_set_state_publish() {
        // AppPublish REPLACES the function set, so a second per-entrypoint create must
        // re-publish the UNION (else the first entrypoint is de-invoked).
        let mut p = Published::default();
        let created = |f: &str, d: &str, url: &str| Created {
            function_id: f.to_string(),
            definition_id: d.to_string(),
            web_url: url.to_string(),
        };
        p.record("add", &created("fu-1", "de-1", ""));
        assert_eq!(p.function_ids.get("add"), Some(&"fu-1".to_string()));
        assert_eq!(p.definition_ids.get("fu-1"), Some(&"de-1".to_string()));
        // No web_url ⇒ no endpoint entry (plain functions never list a URL).
        assert!(p.endpoint_urls.is_empty());
        p.record("add_gpu", &created("fu-2", "de-2", ""));
        assert_eq!(p.function_ids.len(), 2);
        assert_eq!(p.function_ids.get("add"), Some(&"fu-1".to_string()));
        assert_eq!(p.function_ids.get("add_gpu"), Some(&"fu-2".to_string()));
        assert_eq!(p.definition_ids.len(), 2);
        // A webhook create's web_url is recorded under its object tag.
        p.record(
            "web_greet",
            &created("fu-3", "", "https://ws--app-web-greet.modal.run"),
        );
        assert_eq!(
            p.endpoint_urls.get("web_greet").map(String::as_str),
            Some("https://ws--app-web-greet.modal.run")
        );
        // Empty definition_id is NOT recorded.
        assert_eq!(p.definition_ids.len(), 2);
    }

    #[test]
    fn reject_cache_collision_only_when_cache_on_and_path_matches() {
        assert!(reject_cache_collision(true, CACHE_MOUNT).is_err());
        assert!(reject_cache_collision(true, "/data").is_ok());
        // Cache off: a /cache user mount is allowed (no cargo cache to collide with).
        assert!(reject_cache_collision(false, CACHE_MOUNT).is_ok());
    }
}
