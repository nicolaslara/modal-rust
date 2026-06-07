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
//! divergence is isolated to that value plus the pure [`build_image_spec`] /
//! [`build_function_spec`] functions — there is NO scattered `if run {…} else {…}`
//! in [`provision`].
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

use modal_rust_sdk::{FunctionSpec, ImageSpec, ModalClient};

use crate::deploy::{
    DEPLOY_RUNNER, DEPLOY_SRC, DEPLOY_WRAPPER_CALLABLE, DEPLOY_WRAPPER_MODULE, DEPLOY_WRAPPER_SRC,
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
    /// Enable the P6 cargo build cache (RUN path only; DEPLOY ignores it — it builds
    /// at image-build time).
    pub cache: bool,
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
            if inputs.install_rust {
                spec = spec.with_rust_toolchain();
            }
            let mut spec = spec
                .with_wrapper_module(WRAPPER_MODULE, run_wrapper_src())
                .with_command(run_wrapper_config_env(
                    inputs.source.package,
                    inputs.cache,
                    inputs.source.remote_src,
                ))
                .with_command("ENV RUST_BACKTRACE=1");
            // P6: target/ caching is opt-in via MODAL_RUST_CACHE_TARGET (default OFF).
            // The wrapper reads this from the CONTAINER env, but the local process env
            // does not cross to Modal, so when caching is on AND the var is set
            // locally we BAKE it into the image ENV. Default path renders identically.
            if inputs.cache && crate::remote::discover_cache_target() {
                spec = spec.with_command("ENV MODAL_RUST_CACHE_TARGET=1");
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
            spec
        }
    }
}

/// The DEPLOY TOP layer (layer 2): bases on the add_python base layer via `FROM
/// base`, bakes the deploy wrapper, COPYs the SOURCE (this layer's build context),
/// then runs `cargo build --release` + `cp`/bake of `/app/modal_runner`. cargo runs
/// AT image-build time; the deployed runtime never repeats it. Pure (no I/O).
pub(crate) fn build_deploy_top_layer_spec(
    base_image_id: &str,
    source_mount_id: &str,
    package: &str,
) -> ImageSpec {
    ImageSpec::from_registry(String::new()) // FROM replaced by `FROM base` (layered).
        .with_base_image(base_image_id)
        .with_wrapper_module(DEPLOY_WRAPPER_MODULE, DEPLOY_WRAPPER_SRC)
        .with_context_mount(source_mount_id)
        // Context root → /, so the /app/src-prefixed tree lands at /app/src.
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
    let (module, callable, mount_ids) = match boundary.source_delivery {
        SourceDelivery::RunMount => (
            WRAPPER_MODULE,
            WRAPPER_CALLABLE,
            vec![res.client_mount_id.clone(), res.source_mount_id.clone()],
        ),
        // DEPLOY attaches the CLIENT mount ONLY (no source mount).
        SourceDelivery::DeployContext => (
            DEPLOY_WRAPPER_MODULE,
            DEPLOY_WRAPPER_CALLABLE,
            vec![res.client_mount_id.clone()],
        ),
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
        // retries ride into Function.retry_policy. `None` leaves the field unset, so
        // an unset decorator is byte-identical to before.
        .with_retries(ep.options.retries);
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

    /// Resolve a named Secret by `from_name` lookup → `secret_id`.
    async fn ensure_secret(&mut self, name: &str) -> Result<String>;

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

    /// Resolve the hosted python-build-standalone mount for `series` → `mount_id`.
    async fn ensure_python_mount(&mut self, series: &str) -> Result<String>;

    /// Build (or fetch) the image for `spec` under `app_id` → `image_id`. The
    /// `ImageJoinStreaming` poll loop + reconnect/retry live INSIDE this method (live
    /// impl). `layer` is the layer ordinal for the dump's projection (0 = base/run,
    /// 1 = deploy top).
    async fn ensure_image(&mut self, app_id: &str, spec: &ImageSpec, layer: u8) -> Result<String>;

    /// `FunctionPrecreate` under the per-entrypoint object tag → precreate id.
    async fn precreate(&mut self, app_id: &str, object_tag: &str) -> Result<String>;

    /// `FunctionCreate` (FILE mode) → `(function_id, definition_id)`.
    async fn create(
        &mut self,
        app_id: &str,
        precreate_id: &str,
        spec: &FunctionSpec,
    ) -> Result<(String, String)>;

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
}

impl Published {
    fn record(&mut self, object_tag: &str, function_id: &str, definition_id: &str) {
        self.function_ids
            .insert(object_tag.to_string(), function_id.to_string());
        if !definition_id.is_empty() {
            self.definition_ids
                .insert(function_id.to_string(), definition_id.to_string());
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
    // PER ENTRYPOINT inside the create loop. That ordering difference is the only
    // place the Boundary reorders steps; it is expressed as the two arms below, each
    // delegating to the SAME `ControlPlane` methods — no duplicated request building.
    let run = matches!(boundary.source_delivery, SourceDelivery::RunMount);

    // Per-run resources resolved once before the create loop (RUN only): the cargo
    // cache + the single RUN entrypoint's secrets/user-volumes, in the live wire order.
    let mut cache_vol_id: Option<String> = None;
    let mut run_secret_ids: Vec<String> = Vec::new();
    let mut run_user_volume_mounts: Vec<(String, String)> = Vec::new();

    // App (RUN: ephemeral, resolved from the connect-time id; DEPLOY: persistent
    // get-or-create AFTER the mounts — so the RUN arm resolves it here, DEPLOY later).
    let app_id = if run {
        let app_id = cp
            .ensure_app(inputs.app_name, inputs.app_id, boundary.app_state)
            .await?;
        // Cargo-cache volume (RUN only), when caching is on.
        if inputs.cache {
            cache_vol_id = Some(cp.ensure_volume(CACHE_VOLUME_NAME, true).await?);
        }
        // Run-level secrets + user volumes (the single RUN entrypoint's config).
        let ep = single_entrypoint(inputs)?;
        for name in &ep.options.secrets {
            run_secret_ids.push(cp.ensure_secret(name).await?);
        }
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
            let top_spec = build_deploy_top_layer_spec(
                &base_image_id,
                &source_mount_id,
                inputs.source.package,
            );
            cp.ensure_image(&app_id, &top_spec, 1).await?
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
        let (cache_for_ep, secret_ids, user_volume_mounts) = if run {
            (
                cache_vol_id.clone(),
                run_secret_ids.clone(),
                run_user_volume_mounts.clone(),
            )
        } else {
            let mut secret_ids: Vec<String> = Vec::with_capacity(ep.options.secrets.len());
            for name in &ep.options.secrets {
                secret_ids.push(cp.ensure_secret(name).await?);
            }
            let mut user_volume_mounts: Vec<(String, String)> =
                Vec::with_capacity(ep.options.volumes.len());
            for (mount_path, name) in &ep.options.volumes {
                let vid = cp.ensure_volume(name, false).await?;
                user_volume_mounts.push((vid, mount_path.clone()));
            }
            (None, secret_ids, user_volume_mounts)
        };

        let res = AttachedResources {
            client_mount_id: client_mount_id.clone(),
            source_mount_id: source_mount_id.clone(),
            cache_vol_id: cache_for_ep,
            secret_ids,
            user_volume_mounts,
        };
        let fn_spec = build_function_spec(boundary, ep, &image_id, &res)?;
        let (function_id, definition_id) = cp.create(&app_id, &precreate_id, &fn_spec).await?;
        published.record(&object_tag, &function_id, &definition_id);

        // RUN re-publishes the CUMULATIVE union after EACH create — `AppPublish`
        // REPLACES the function set, so a per-entrypoint create must re-publish every
        // prior one too (across calls, via the threaded `published`) or it would
        // de-invoke them. DEPLOY publishes ONCE after the whole loop.
        if run {
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

    // DEPLOY: one persistent publish carrying the UNION of every per-entrypoint fn.
    if !run {
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

/// The RUN path provisions exactly ONE entrypoint per [`provision`] call (the caller
/// memoizes per entrypoint and maintains the cumulative publish union). Return it.
fn single_entrypoint<'a>(inputs: &'a ProvisionInputs<'a>) -> Result<&'a Entrypoint> {
    inputs
        .entrypoints
        .first()
        .ok_or_else(|| Error::config("RUN provision requires exactly one entrypoint".to_string()))
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

    async fn ensure_secret(&mut self, name: &str) -> Result<String> {
        Ok(self.client.secret_get_or_create(name, &[], None).await?)
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
        match (
            source.use_cargo_scoping,
            crate::scope::workspace_closure(source.local_root, source.package),
        ) {
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
            _ => Ok(self
                .client
                .mount_local_dir(
                    source.local_root,
                    remote_path,
                    source.modalignore_name,
                    None,
                )
                .await?),
        }
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
    ) -> Result<(String, String)> {
        let created = self
            .client
            .function_create(app_id, precreate_id, spec)
            .await?;
        Ok((created.function_id, created.definition_id))
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
            cache: true,
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
        let spec = build_deploy_top_layer_spec("im-layer1", "mo-deploy-src", "example-add");
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
    }

    #[test]
    fn published_accumulates_union_for_set_state_publish() {
        // AppPublish REPLACES the function set, so a second per-entrypoint create must
        // re-publish the UNION (else the first entrypoint is de-invoked).
        let mut p = Published::default();
        p.record("add", "fu-1", "de-1");
        assert_eq!(p.function_ids.get("add"), Some(&"fu-1".to_string()));
        assert_eq!(p.definition_ids.get("fu-1"), Some(&"de-1".to_string()));
        p.record("add_gpu", "fu-2", "de-2");
        assert_eq!(p.function_ids.len(), 2);
        assert_eq!(p.function_ids.get("add"), Some(&"fu-1".to_string()));
        assert_eq!(p.function_ids.get("add_gpu"), Some(&"fu-2".to_string()));
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
