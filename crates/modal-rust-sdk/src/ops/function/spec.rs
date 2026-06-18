//! The public function-spec vocabulary: [`FunctionSpec`] + its config structs
//! ([`FunctionResources`], [`FunctionVolumeMount`], [`FunctionAutoscaler`],
//! [`WebhookSpec`]) and their `with_*` builders. Split out of `function.rs`
//! mechanically (M1); all paths re-exported from the parent module.

use super::parse::{
    parse_gpu_config, parse_retries_spec, parse_schedule, RETRY_DEFAULT_BACKOFF_COEFFICIENT,
    RETRY_DEFAULT_INITIAL_DELAY_MS, RETRY_DEFAULT_MAX_DELAY_MS,
};
use crate::error::{Error, Result};
use crate::proto::api::{
    AutoscalerSettings, FunctionRetryPolicy, Resources, Schedule, VolumeMount,
};

/// CPU/memory/GPU request for a created function. FILE-mode CPU functions can use
/// the zero default (`Resources::default()`); set modest values to be explicit.
#[derive(Debug, Clone, Default)]
pub struct FunctionResources {
    /// Requested memory (MiB). `0` = server default.
    pub memory_mb: u32,
    /// Requested CPU (milli-cores). `0` = server default.
    pub milli_cpu: u32,
    /// Optional GPU spec (`"T4"`, `"A100"`, `"A100-80GB"`, `"H100:4"`). `None` =
    /// CPU-only (empty `gpu_config`, mirroring `parse_gpu_config(None)`). Validated
    /// up front by [`FunctionSpec::with_gpu`], so [`Self::to_proto`] re-parses
    /// infallibly.
    pub gpu: Option<String>,
}

impl FunctionResources {
    pub(super) fn to_proto(&self) -> Resources {
        // CPU path keeps `gpu_config: None` (proto field 4 unset) — wire-equivalent
        // to today. The GPU string was validated at set time (`with_gpu`), so the
        // re-parse here is infallible (`unwrap_or_default` for total safety).
        let gpu_config = self
            .gpu
            .as_deref()
            .map(|s| parse_gpu_config(s).unwrap_or_default());
        Resources {
            memory_mb: self.memory_mb,
            milli_cpu: self.milli_cpu,
            gpu_config,
            ..Default::default()
        }
    }
}

/// One persistent-volume attachment for a function. Maps to proto `VolumeMount`
/// on `Function.volume_mounts`. Additive: a spec with an empty `volume_mounts`
/// is wire-identical to before P6.
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

    pub(super) fn to_proto(&self) -> VolumeMount {
        VolumeMount {
            volume_id: self.volume_id.clone(),
            mount_path: self.mount_path.clone(),
            allow_background_commits: self.allow_background_commits,
            read_only: false, // cargo cache must be writable
            sub_path: None,   // field 5 unset
        }
    }
}

/// Autoscaler controls for a function → `Function.autoscaler_settings` (proto field
/// 79) plus the deprecated mirror fields Modal still sets. Each knob is `Option<u32>`
/// (unset = leave to the server). DEFAULT all-`None`: an autoscaler with no knobs set
/// emits NOTHING (the spec leaves `autoscaler_settings` and every legacy mirror at
/// their zero/unset default), so the create is byte-identical to before for every
/// function that does not configure autoscaling.
///
/// Mirrors Modal's `app.function(min_containers, max_containers, buffer_containers,
/// scaledown_window)` (`_functions.py:660-768,1019-1022`): the modern
/// `AutoscalerSettings` carries the values, and Modal ALSO populates the legacy
/// `warm_pool_size` / `concurrency_limit` / `_experimental_buffer_containers` /
/// `task_idle_timeout_secs` fields from the same values for server-side
/// backward-compatibility — so we set both.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FunctionAutoscaler {
    /// Minimum containers to keep running (scale-to-zero floor; Modal `min_containers`,
    /// pka `keep_warm`/`warm_pool_size`). `None` = scale to zero.
    pub min_containers: Option<u32>,
    /// Maximum concurrent containers (ceiling; Modal `max_containers`, pka
    /// `concurrency_limit`). `None` = no client-set ceiling.
    pub max_containers: Option<u32>,
    /// Extra warm containers to keep beyond demand (Modal `buffer_containers`). `None`
    /// = no buffer.
    pub buffer_containers: Option<u32>,
    /// Seconds an idle container waits before scaling down (Modal `scaledown_window`,
    /// pka `container_idle_timeout`). `None` = server default.
    pub scaledown_window: Option<u32>,
}

impl FunctionAutoscaler {
    /// `true` when NO knob is set — the spec then leaves the wire byte-identical to
    /// before (no `autoscaler_settings`, no legacy mirror fields).
    pub(super) fn is_empty(&self) -> bool {
        self.min_containers.is_none()
            && self.max_containers.is_none()
            && self.buffer_containers.is_none()
            && self.scaledown_window.is_none()
    }

    /// Project to the modern `AutoscalerSettings` proto (the optional knobs ride
    /// through verbatim; Flash-only fields stay unset).
    pub(super) fn to_settings(&self) -> AutoscalerSettings {
        AutoscalerSettings {
            min_containers: self.min_containers,
            max_containers: self.max_containers,
            buffer_containers: self.buffer_containers,
            scaledown_window: self.scaledown_window,
            ..Default::default()
        }
    }
}

/// Web-endpoint opt-in for a function → `Function.webhook_config` (proto field 15),
/// the single-function `@modal.fastapi_endpoint` shape (`WEBHOOK_TYPE_FUNCTION`).
///
/// Carried on [`FunctionSpec::webhook`]; `None` (the default) keeps the create
/// byte-identical to before web endpoints — no `webhook_config` on the wire AND the
/// advertised data formats stay `[PICKLE, CBOR]`. When set,
/// [`build_function_create_request`] ALSO swaps the advertised formats to the ASGI
/// pair Modal's web layer requires (spike finding 3: a web-endpoint function must
/// advertise `supported_input_formats = [ASGI]` and `supported_output_formats =
/// [ASGI, GENERATOR_DONE]`, else modal-http rejects the response as "unexpected data
/// format: Pickle").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebhookSpec {
    /// HTTP method for the single-function endpoint (`"GET"`/`"POST"`/`"PUT"`/
    /// `"DELETE"`/`"PATCH"`) → `WebhookConfig.method` (proto field 2). Validated
    /// upstream by the `#[endpoint(method = ..)]` macro; the SDK passes it verbatim.
    /// EMPTY for a WEB-SERVER webhook (a raw port proxy has no per-request method).
    pub method: String,
    /// Modal proxy-auth opt-in → `WebhookConfig.requires_proxy_auth` (proto field
    /// 10). `false` (Modal's default) = public URL; `true` = Modal rejects requests
    /// lacking the `Modal-Key`/`Modal-Secret` proxy-auth header pair BEFORE they
    /// reach the container.
    pub requires_proxy_auth: bool,
    /// Web-server bound TCP port (`#[web_server(port = ..)]`) → `WebhookConfig.type =
    /// WEBHOOK_TYPE_WEB_SERVER` + `WebhookConfig.web_server_port` (proto field 7).
    /// `None` (the default) ⇒ a FUNCTION webhook (the `#[endpoint]` request/response
    /// shape, formats swapped to ASGI); `Some(port)` ⇒ a WEB-SERVER raw-port proxy
    /// (Modal forwards all traffic to `port`; the data formats stay `[PICKLE, CBOR]`
    /// since the function is invoked once at container start, not per request).
    pub web_server_port: Option<u16>,
    /// Web-server startup timeout in seconds (`#[web_server(.., startup_timeout = ..)]`)
    /// → `WebhookConfig.web_server_startup_timeout` (proto field 8, a float). `None` ⇒
    /// Modal default (`0.0`). Inert unless `web_server_port` is set.
    pub web_server_startup_timeout: Option<u32>,
}

/// Declarative spec for a FILE-mode function create.
///
/// FILE mode carries NO serialized bytecode: the function is identified by
/// `module_name` + `function_name` and resolved in-container via
/// `importlib.import_module(module_name)` + `getattr(module, function_name)`.
///
/// # `None`-semantics rule (M8)
///
/// Every `Option`-taking `with_*` setter **clears** on `None` — it does NOT
/// keep the previous value. `None` always means "server default / unset on
/// the wire": for numeric fields (`milli_cpu`, `memory_mb`) `0` is already
/// the server-default sentinel that prost transmits as an omitted field; for
/// optional proto messages (`retry_policy`, `schedule`, `webhook`) `None`
/// leaves the proto field unset; for enum-like knobs (`gpu`) `None` means
/// CPU-only. Callers that want to PRESERVE an existing value must NOT call the
/// setter with `None`.
#[derive(Debug, Clone)]
pub struct FunctionSpec {
    /// Importable module name (e.g. the baked wrapper, `"spike_wrapper"`).
    pub module_name: String,
    /// Callable name within the module — the IN-CONTAINER `getattr` target Modal
    /// resolves in FILE mode (e.g. the shared dispatch callable `"handler"`). This is
    /// the *implementation* attribute, which may DIFFER from the app-namespace object
    /// tag ([`app_function_name`](FunctionSpec::app_function_name)). The wrapper
    /// dispatches by the per-call entrypoint arg, so every entrypoint shares ONE
    /// in-container callable while carrying its OWN object tag.
    pub function_name: String,
    /// The Modal app-namespace object TAG — the name that makes a function unique
    /// within an app (used by `FunctionPrecreate`, `AppPublish`, and `FunctionGet`).
    /// `None` ⇒ defaults to [`function_name`](FunctionSpec::function_name) (the
    /// single-callable shape: object tag == in-container callable). Set this to the
    /// ENTRYPOINT NAME so divergent per-entrypoint configs coexist as DISTINCT Modal
    /// functions instead of colliding on one shared `"handler"` tag.
    ///
    /// When set AND different from `function_name`, the built [`Function`] carries
    /// `function_name = app_function_name` (the object tag) and
    /// `implementation_name = function_name` (the module attribute), mirroring
    /// Modal's `@app.function(name=...)` mechanism.
    pub app_function_name: Option<String>,
    /// Built image id ([`ModalClient::image_get_or_create`]).
    pub image_id: String,
    /// Mount ids to attach — MUST include the client mount
    /// ([`ModalClient::client_mount_id`]) so `modal` is importable.
    pub mount_ids: Vec<String>,
    /// Function timeout in seconds.
    pub timeout_secs: u32,
    /// Resource request (always sent — fix #1).
    pub resources: FunctionResources,
    /// Request the worker to inject the modal client's third-party dependency
    /// closure (`typing_extensions`, `grpclib`, `protobuf`, `aiohttp`, …) into the
    /// container AT START (proto field 82, `mount_client_dependencies`). REQUIRED on
    /// the modern image builder (> "2024.10") when the image is provisioned via
    /// `add_python` rather than `pip install modal`: the client mount carries only
    /// the modal SOURCE, so without this the entrypoint crash-loops with
    /// `ModuleNotFoundError`. Mirrors `_functions.py:936-939`/`:1014`. Defaults to
    /// `true`.
    pub mount_client_dependencies: bool,
    /// Persistent-volume attachments → `Function.volume_mounts`. DEFAULT EMPTY: an
    /// unset list keeps the create wire-identical to pre-P6, so every existing
    /// function is unchanged. P6 pushes the cargo-cache volume here; user volumes
    /// (`#[function(volumes = [..])]`) push additional, DISTINCT-mount-path mounts.
    pub volume_mounts: Vec<FunctionVolumeMount>,
    /// Resolved secret ids → `Function.secret_ids` (proto field 10). DEFAULT EMPTY:
    /// an unset list keeps the create wire-identical to before, so every existing
    /// function is unchanged. The USER-facing `#[function(secrets = [..])]` path
    /// resolves named secrets via [`ModalClient::secret_get_or_create`] and pushes
    /// the ids here; Modal injects each secret's key/values as ENV VARS.
    pub secret_ids: Vec<String>,
    /// Retry policy → `Function.retry_policy` (proto field 18). DEFAULT `None`: an
    /// unset policy keeps prost from emitting field 18, so the create is
    /// byte-identical to before for every function that does not set `retries`. The
    /// USER-facing `#[function(retries = N)]` path sets this; Modal then automatically
    /// re-runs a failed call up to `retries` times.
    pub retry_policy: Option<FunctionRetryPolicy>,
    /// Schedule → `Function.schedule` (proto field 72). DEFAULT `None`: an unset
    /// schedule keeps prost from emitting field 72, so the create is byte-identical to
    /// before for every function that does not set `schedule`. The USER-facing
    /// `#[function(schedule = Cron(..)/Period(..))]` path sets this; Modal then runs the
    /// DEPLOYED function automatically on that cadence (no caller).
    pub schedule: Option<Schedule>,
    /// Autoscaler controls → `Function.autoscaler_settings` (proto field 79) + the
    /// deprecated mirror fields. DEFAULT all-`None`: an autoscaler with no knobs set
    /// emits nothing, so the create is byte-identical to before for every function that
    /// does not configure autoscaling. The USER-facing
    /// `#[function(min_containers = .., max_containers = .., buffer_containers = ..,
    /// scaledown_window = ..)]` path sets these; Modal then controls warm capacity and
    /// scale-to-zero accordingly.
    pub autoscaler: FunctionAutoscaler,
    /// Memory-snapshot (checkpoint/restore) opt-in → `Function.checkpointing_enabled`
    /// (proto field 41) + `Function.is_checkpointing_function` (proto field 40). DEFAULT
    /// `false`: both bools stay `false` ⇒ prost omits fields 40 + 41 ⇒ the create is
    /// byte-identical to before for every function that does not opt in. The facade only
    /// sets this on the DEPLOY boundary (Modal snapshots deployed apps), via
    /// [`with_memory_snapshot`](FunctionSpec::with_memory_snapshot); RUN stays
    /// wire-identical even when the decorator opts in.
    pub checkpointing_enabled: bool,
    /// Web-endpoint opt-in → `Function.webhook_config` (proto field 15) + the ASGI
    /// data-format swap. DEFAULT `None`: prost omits field 15 AND the advertised
    /// formats stay `[PICKLE, CBOR]` ⇒ the create is byte-identical to before web
    /// endpoints for every non-endpoint function. The facade only sets this on the
    /// DEPLOY boundary (the URL is deploy-only in v0), via
    /// [`with_webhook`](FunctionSpec::with_webhook); RUN stays wire-identical even
    /// when the decorator opts in — exactly like `checkpointing_enabled`.
    pub webhook: Option<WebhookSpec>,
    /// Per-container input concurrency → `Function.max_concurrent_inputs` (proto field
    /// 34). DEFAULT `None`: the scalar stays 0 ⇒ prost omits field 34 ⇒ byte-identical
    /// to before for every function that does not opt in. This is the MAX number of
    /// inputs a single replica processes at once (distinct from the `max_containers`
    /// scale-OUT count, and NOT part of `AutoscalerSettings`). Set via
    /// [`with_concurrency`](FunctionSpec::with_concurrency).
    pub max_concurrent_inputs: Option<u32>,
    /// Target per-container input concurrency → `Function.target_concurrent_inputs`
    /// (proto field 64). DEFAULT `None`: the scalar stays 0 ⇒ prost omits field 64 ⇒
    /// byte-identical to before. The TARGET concurrency the autoscaler aims for per
    /// replica; when unset Modal's worker falls back to `max_concurrent_inputs`. NOT
    /// part of `AutoscalerSettings`. Set via
    /// [`with_concurrency`](FunctionSpec::with_concurrency).
    pub target_concurrent_inputs: Option<u32>,
}

impl FunctionSpec {
    /// A FILE-mode function spec with sensible defaults (300s timeout, default
    /// resources, no mounts yet).
    pub fn new(
        module_name: impl Into<String>,
        function_name: impl Into<String>,
        image_id: impl Into<String>,
    ) -> Self {
        Self {
            module_name: module_name.into(),
            function_name: function_name.into(),
            app_function_name: None,
            image_id: image_id.into(),
            mount_ids: Vec::new(),
            timeout_secs: 300,
            resources: FunctionResources::default(),
            mount_client_dependencies: true,
            volume_mounts: Vec::new(),
            secret_ids: Vec::new(),
            retry_policy: None,
            schedule: None,
            autoscaler: FunctionAutoscaler::default(),
            checkpointing_enabled: false,
            webhook: None,
            max_concurrent_inputs: None,
            target_concurrent_inputs: None,
        }
    }

    /// Set the Modal app-namespace object TAG (the unique-within-an-app name used by
    /// precreate/publish/from_name), DECOUPLED from the in-container
    /// [`function_name`](FunctionSpec::function_name) callable. Set this to the
    /// ENTRYPOINT NAME so distinct entrypoints become distinct Modal functions (each
    /// carrying its own config) instead of clobbering one shared `"handler"` tag.
    pub fn with_app_function_name(mut self, name: impl Into<String>) -> Self {
        self.app_function_name = Some(name.into());
        self
    }

    /// The effective Modal object TAG: [`app_function_name`](FunctionSpec::app_function_name)
    /// when set, else [`function_name`](FunctionSpec::function_name). This is the name
    /// to feed `FunctionPrecreate` / `AppPublish` / `FunctionGet` so the registered
    /// tag matches the created function.
    pub fn object_tag(&self) -> &str {
        self.app_function_name
            .as_deref()
            .unwrap_or(&self.function_name)
    }

    /// Attach mount ids (e.g. the client mount). Replaces any existing list.
    pub fn with_mount_ids(mut self, mount_ids: Vec<String>) -> Self {
        self.mount_ids = mount_ids;
        self
    }

    /// Append a single mount id (e.g. the resolved client mount).
    pub fn with_mount_id(mut self, mount_id: impl Into<String>) -> Self {
        self.mount_ids.push(mount_id.into());
        self
    }

    /// Override the function timeout (seconds).
    pub fn with_timeout_secs(mut self, secs: u32) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Set the resource request.
    pub fn with_resources(mut self, resources: FunctionResources) -> Self {
        self.resources = resources;
        self
    }

    /// Set the requested CPU (milli-cores) on the function's resources.
    /// `None` **clears** to `0` (server default, wire-identical to unset).
    /// See the [`FunctionSpec`] `None`-semantics rule.
    pub fn with_milli_cpu(mut self, milli_cpu: Option<u32>) -> Self {
        self.resources.milli_cpu = milli_cpu.unwrap_or(0);
        self
    }

    /// Set the requested memory (MiB) on the function's resources.
    /// `None` **clears** to `0` (server default, wire-identical to unset).
    /// See the [`FunctionSpec`] `None`-semantics rule.
    pub fn with_memory_mb(mut self, memory_mb: Option<u32>) -> Self {
        self.resources.memory_mb = memory_mb.unwrap_or(0);
        self
    }

    /// Set the GPU spec on the function's resources (validated NOW so
    /// [`FunctionResources::to_proto`] stays infallible). `None` = CPU-only (no
    /// `gpu_config`, byte-identical to today).
    ///
    /// Mirrors `parse_gpu_config`: `"TYPE"`, `"TYPE:count"`, or `"TYPE-MEM"` (the
    /// mem suffix rides inside `gpu_type`); uppercased; `count` defaults to `1`. A
    /// bad (non-integer) count returns [`Error::invalid`] — fix the decorator value.
    pub fn with_gpu(mut self, gpu: Option<impl Into<String>>) -> Result<Self> {
        let gpu = gpu.map(Into::into);
        if let Some(spec) = gpu.as_deref() {
            parse_gpu_config(spec)?; // validate up front
        }
        self.resources.gpu = gpu;
        Ok(self)
    }

    /// Override whether the worker injects the modal client's dependency closure at
    /// container start (proto field 82). Defaults to `true`; set `false` only for an
    /// image that already carries the deps (e.g. the legacy `pip install modal`
    /// fallback) or where runtime dep-mounting is unavailable.
    pub fn with_mount_client_dependencies(mut self, enabled: bool) -> Self {
        self.mount_client_dependencies = enabled;
        self
    }

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

    /// Attach resolved secret ids (→ `Function.secret_ids`). Replaces any existing
    /// list. EMPTY keeps the create wire-identical to before.
    pub fn with_secret_ids(mut self, secret_ids: Vec<String>) -> Self {
        self.secret_ids = secret_ids;
        self
    }

    /// Append a single resolved secret id (→ `Function.secret_ids`).
    pub fn with_secret_id(mut self, secret_id: impl Into<String>) -> Self {
        self.secret_ids.push(secret_id.into());
        self
    }

    /// Set an automatic retry policy from a bare retry COUNT (`#[function(retries =
    /// N)]`), mirroring Modal's `_parse_retries(int)`: a fixed-interval policy with
    /// `backoff_coefficient = 1.0`, `initial_delay = 1s`, `max_delay = 60s`. `None`
    /// leaves the field UNSET so the create is byte-identical to before (no
    /// `retry_policy` on the wire).
    pub fn with_retries(mut self, retries: Option<u32>) -> Self {
        self.retry_policy = retries.map(|n| FunctionRetryPolicy {
            backoff_coefficient: RETRY_DEFAULT_BACKOFF_COEFFICIENT,
            initial_delay_ms: RETRY_DEFAULT_INITIAL_DELAY_MS,
            max_delay_ms: RETRY_DEFAULT_MAX_DELAY_MS,
            retries: n,
        });
        self
    }

    /// Set a CUSTOM retry policy from a modal-rust retry SPEC string (the `Retries(..)`
    /// STRUCT form, `#[function(retries = Retries(max_retries = N, backoff = f,
    /// initial_delay = s, max_delay = s))]`), parsed by [`parse_retries_spec`] into the
    /// four [`FunctionRetryPolicy`] fields. `None` leaves the field UNSET so the create
    /// is byte-identical to before (no `retry_policy` on the wire). A malformed spec
    /// returns [`Error::invalid`] — fix the spec string. This is the custom-backoff
    /// sibling of [`with_retries`] (the bare-int fixed-interval shortcut); the two are
    /// mutually exclusive (the macro emits at most one).
    pub fn with_retry_policy(mut self, spec: Option<&str>) -> Result<Self> {
        self.retry_policy = spec.map(parse_retries_spec).transpose()?;
        Ok(self)
    }

    /// Set a run schedule from a modal-rust schedule SPEC string
    /// (`#[function(schedule = Cron("..")/Period(..))]`), parsed by [`parse_schedule`]
    /// into Modal's `Schedule` (a `Cron`/`Period` oneof). `None` leaves the field UNSET
    /// so the create is byte-identical to before (no `schedule` on the wire). A
    /// malformed spec returns [`Error::invalid`] — fix the spec string.
    pub fn with_schedule(mut self, spec: Option<&str>) -> Result<Self> {
        self.schedule = spec.map(parse_schedule).transpose()?;
        Ok(self)
    }

    /// Set autoscaler controls (`#[function(min_containers = .., max_containers = ..,
    /// buffer_containers = .., scaledown_window = ..)]`). An all-`None`
    /// [`FunctionAutoscaler`] (the default) leaves the wire byte-identical to before
    /// (no `autoscaler_settings`, no legacy mirror fields).
    ///
    /// Mirrors Modal's validation (`_functions.py:755-762`): `max_containers` must be
    /// >= `min_containers` when both are set, and `scaledown_window` (when set) must be
    /// > 0. A violation returns [`Error::invalid`] — fix the decorator value.
    pub fn with_autoscaler(mut self, autoscaler: FunctionAutoscaler) -> Result<Self> {
        if let (Some(min), Some(max)) = (autoscaler.min_containers, autoscaler.max_containers) {
            if max < min {
                return Err(Error::invalid(format!(
                    "`min_containers` ({min}) cannot be greater than `max_containers` ({max})"
                )));
            }
        }
        if let Some(window) = autoscaler.scaledown_window {
            if window == 0 {
                return Err(Error::invalid("`scaledown_window` must be > 0".to_string()));
            }
        }
        self.autoscaler = autoscaler;
        Ok(self)
    }

    /// Set per-container input concurrency (`#[function(max_concurrent_inputs = ..,
    /// target_concurrent_inputs = ..)]`) → `Function.max_concurrent_inputs` (proto
    /// field 34) + `Function.target_concurrent_inputs` (proto field 64). These are the
    /// per-replica input-fan-in knobs, distinct from the `max_containers` scale-OUT
    /// count and NOT part of `AutoscalerSettings`. All-`None` (the default) leaves the
    /// wire byte-identical to before (both scalars stay 0 ⇒ prost omits fields 34 + 64).
    ///
    /// Mirrors Modal's client-side validation (`_partial_function.py:755-756`):
    /// both values must be >= 1 (a `Some(0)` is the ambiguous unset sentinel — proto3
    /// `u32` 0 serializes identically to omitted — and is rejected for each);
    /// `target_concurrent_inputs` requires `max_concurrent_inputs` to be set (Modal's
    /// client raises `missing required argument: max_inputs` when a target is given
    /// without a max); and `target_concurrent_inputs` must be <= `max_concurrent_inputs`
    /// when both are set (`max == target` is allowed). `target_concurrent_inputs` is left
    /// unset on the wire (0) when not given — Modal's worker applies its own `target or
    /// max` default rather than us guessing. A violation returns [`Error::invalid`] — fix
    /// the decorator value.
    pub fn with_concurrency(
        mut self,
        max_concurrent_inputs: Option<u32>,
        target_concurrent_inputs: Option<u32>,
    ) -> Result<Self> {
        if let Some(0) = max_concurrent_inputs {
            return Err(Error::invalid(
                "`max_concurrent_inputs` must be >= 1".to_string(),
            ));
        }
        if let Some(0) = target_concurrent_inputs {
            return Err(Error::invalid(
                "`target_concurrent_inputs` must be >= 1".to_string(),
            ));
        }
        if target_concurrent_inputs.is_some() && max_concurrent_inputs.is_none() {
            return Err(Error::invalid(
                "`target_concurrent_inputs` requires `max_concurrent_inputs` to be set".to_string(),
            ));
        }
        if let (Some(max), Some(target)) = (max_concurrent_inputs, target_concurrent_inputs) {
            if target > max {
                return Err(Error::invalid(format!(
                    "`target_concurrent_inputs` ({target}) cannot be greater than `max_concurrent_inputs` ({max})"
                )));
            }
        }
        self.max_concurrent_inputs = max_concurrent_inputs;
        self.target_concurrent_inputs = target_concurrent_inputs;
        Ok(self)
    }

    /// Opt into Modal memory snapshot (checkpoint/restore) for this function. When `on`,
    /// the built [`Function`] sets BOTH `checkpointing_enabled` (proto field 41) and
    /// `is_checkpointing_function` (proto field 40); when `false` (the default) both stay
    /// unset ⇒ prost omits them ⇒ byte-identical to before. The facade gates this to the
    /// DEPLOY boundary only (Modal snapshots deployed apps), so RUN stays wire-identical.
    pub fn with_memory_snapshot(mut self, on: bool) -> Self {
        self.checkpointing_enabled = on;
        self
    }

    /// Expose this function as a web endpoint (`WEBHOOK_TYPE_FUNCTION`). When `Some`,
    /// the built [`Function`] carries `webhook_config` (proto field 15) AND advertises
    /// the ASGI data formats Modal's web layer requires (input `[ASGI]`, output
    /// `[ASGI, GENERATOR_DONE]`); when `None` (the default) the wire is byte-identical
    /// to before — no `webhook_config`, formats stay `[PICKLE, CBOR]`. The facade
    /// gates this to the DEPLOY boundary only (the URL is deploy-only in v0), so RUN
    /// stays wire-identical — exactly like
    /// [`with_memory_snapshot`](FunctionSpec::with_memory_snapshot).
    /// A malformed `method` is rejected AT SET TIME like every sibling knob
    /// (gpu/retries/schedule/autoscaler validate at set time): the macro allowlists the
    /// same five methods, but the SDK is a public crate and must not trust upstream.
    pub fn with_webhook(mut self, webhook: Option<WebhookSpec>) -> Result<Self> {
        if let Some(w) = &webhook {
            // A WEB-SERVER webhook (`web_server_port` set) is a raw port proxy with no
            // per-request HTTP method, so its method is empty and the verb allowlist is
            // skipped; a FUNCTION webhook (`#[endpoint]`) still validates the verb.
            if w.web_server_port.is_none() {
                const METHODS: [&str; 5] = ["GET", "POST", "PUT", "DELETE", "PATCH"];
                if !METHODS.contains(&w.method.as_str()) {
                    return Err(Error::invalid(format!(
                        "invalid webhook method {:?}: expected one of GET, POST, PUT, DELETE, PATCH",
                        w.method
                    )));
                }
            }
        }
        self.webhook = webhook;
        Ok(self)
    }
}
