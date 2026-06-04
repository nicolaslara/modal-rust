//! Function authoring: `FunctionPrecreate` + `FunctionCreate` (FILE mode) +
//! `FunctionGet` (`from_name`).
//!
//! ## Fix #1 — `FunctionCreate` sends EXACTLY ONE of `function` / `function_data`
//!
//! modal-rs sent BOTH `function` and `function_data` → server "Internal error".
//! We use the single-`Function` path: set `function`, set `existing_function_id`
//! to the precreate id, and leave `function_data` UNSET. We also ALWAYS set
//! `resources` (omitting it contributed to the same server error).
//!
//! The precreate id is what makes an empty `function_serialized` legal in FILE
//! mode: it sets `allow_sparse_base = true` server-side, bypassing the
//! empty-serialized guard, so the function is identified purely by
//! `module_name` + `function_name`.

use crate::client::ModalClient;
use crate::error::{Error, Result};
use crate::proto::api::function::{DefinitionType, FunctionType};
use crate::proto::api::{
    DataFormat, Function, FunctionCreateRequest, FunctionGetRequest, FunctionPrecreateRequest,
    GpuConfig, Resources, VolumeMount,
};
use crate::retry::retry_unary;

/// Parse a Modal GPU spec into a [`GpuConfig`], mirroring `parse_gpu_config`
/// (modal `_utils/function_utils.py:628`). Format: `"TYPE"` or `"TYPE:count"`.
///
/// The MEM suffix (`"A100-80GB"`) is NOT split — it stays inside `gpu_type`
/// verbatim. `gpu_type` is uppercased; `count` defaults to `1`; the deprecated
/// `type` field (proto field 1, `GPUType`) stays `0` (Python never sets it). A
/// non-integer count maps to [`Error::build`], mirroring Python's `InvalidError`.
fn parse_gpu_config(spec: &str) -> Result<GpuConfig> {
    // `split_once(':')` = Python's `value.split(":", 1)`.
    let (type_part, count) = match spec.split_once(':') {
        Some((lhs, rhs)) => {
            let count: u32 = rhs.trim().parse().map_err(|_| {
                Error::build(format!(
                    "Invalid GPU count: {rhs}. Value must be an integer."
                ))
            })?;
            (lhs, count)
        }
        None => (spec, 1),
    };
    Ok(GpuConfig {
        gpu_type: type_part.to_uppercase(), // `.upper()`
        count,
        ..Default::default() // r#type (deprecated GPUType, field 1) stays 0
    })
}

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
    fn to_proto(&self) -> Resources {
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

    fn to_proto(&self) -> VolumeMount {
        VolumeMount {
            volume_id: self.volume_id.clone(),
            mount_path: self.mount_path.clone(),
            allow_background_commits: self.allow_background_commits,
            read_only: false, // cargo cache must be writable
            sub_path: None,   // field 5 unset
        }
    }
}

/// Declarative spec for a FILE-mode function create.
///
/// FILE mode carries NO serialized bytecode: the function is identified by
/// `module_name` + `function_name` and resolved in-container via
/// `importlib.import_module(module_name)` + `getattr(module, function_name)`.
#[derive(Debug, Clone)]
pub struct FunctionSpec {
    /// Importable module name (e.g. the baked wrapper, `"spike_wrapper"`).
    pub module_name: String,
    /// Callable name within the module (e.g. `"handler"`).
    pub function_name: String,
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
            image_id: image_id.into(),
            mount_ids: Vec::new(),
            timeout_secs: 300,
            resources: FunctionResources::default(),
            mount_client_dependencies: true,
            volume_mounts: Vec::new(),
            secret_ids: Vec::new(),
        }
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

    /// Set the GPU spec on the function's resources (validated NOW so
    /// [`FunctionResources::to_proto`] stays infallible). `None` = CPU-only (no
    /// `gpu_config`, byte-identical to today).
    ///
    /// Mirrors `parse_gpu_config`: `"TYPE"`, `"TYPE:count"`, or `"TYPE-MEM"` (the
    /// mem suffix rides inside `gpu_type`); uppercased; `count` defaults to `1`. A
    /// bad (non-integer) count returns [`Error::build`].
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
}

/// Result of [`ModalClient::function_create`].
#[derive(Debug, Clone, Default)]
pub struct CreatedFunction {
    /// The created function id.
    pub function_id: String,
    /// `definition_id` from the create's `handle_metadata` (for `AppPublish`'s
    /// `definition_ids` map). Empty if the server did not return one.
    pub definition_id: String,
    /// Advisory server warnings (rendered text).
    pub warnings: Vec<String>,
}

/// The CBOR + PICKLE formats we advertise/support end-to-end.
fn supported_formats() -> Vec<i32> {
    vec![DataFormat::Pickle as i32, DataFormat::Cbor as i32]
}

impl ModalClient {
    /// `FunctionPrecreate` (api.proto:4250). Returns the precreate `function_id`,
    /// which is carried into [`ModalClient::function_create`] as
    /// `existing_function_id` to legalize an empty `function_serialized`.
    ///
    /// Advertises `[PICKLE, CBOR]` for both input and output formats.
    pub async fn function_precreate(
        &mut self,
        app_id: &str,
        function_name: &str,
    ) -> Result<String> {
        // Re-precreate of the same app_id+function_name returns a usable id; the
        // downstream function_create reconciles via existing_function_id, so a
        // retry after a dropped response is safe.
        let req = FunctionPrecreateRequest {
            app_id: app_id.to_string(),
            function_name: function_name.to_string(),
            function_type: FunctionType::Function as i32,
            supported_input_formats: supported_formats(),
            supported_output_formats: supported_formats(),
            ..Default::default()
        };
        let stub = self.stub();
        let resp = retry_unary("function_precreate", || {
            let mut stub = stub.clone();
            let req = req.clone();
            async move { Ok(stub.function_precreate(req).await?.into_inner()) }
        })
        .await?;

        if resp.function_id.is_empty() {
            return Err(Error::build(
                "FunctionPrecreate returned an empty function_id".to_string(),
            ));
        }
        Ok(resp.function_id)
    }

    /// `FunctionCreate` in FILE mode (api.proto:4240) — **fix #1**.
    ///
    /// Sends EXACTLY ONE of `function` / `function_data` (the single-`Function`
    /// path), ALWAYS sets `resources`, leaves `function_serialized` empty, and
    /// passes `existing_function_id = precreate_function_id` to bypass the
    /// empty-serialized guard. Advertises `[PICKLE, CBOR]` formats so CBOR can be
    /// forced end-to-end.
    pub async fn function_create(
        &mut self,
        app_id: &str,
        precreate_function_id: &str,
        spec: &FunctionSpec,
    ) -> Result<CreatedFunction> {
        let function = Function {
            module_name: spec.module_name.clone(),
            function_name: spec.function_name.clone(),
            mount_ids: spec.mount_ids.clone(),
            image_id: spec.image_id.clone(),
            function_serialized: Vec::new(), // FILE mode: empty.
            definition_type: DefinitionType::File as i32,
            function_type: FunctionType::Function as i32,
            resources: Some(spec.resources.to_proto()), // fix #1: always set.
            timeout_secs: spec.timeout_secs,
            supported_input_formats: supported_formats(),
            supported_output_formats: supported_formats(),
            // Worker injects the client dep closure at container start (modern
            // builder), so the add_python image needs no `pip install modal` layer.
            mount_client_dependencies: spec.mount_client_dependencies,
            // Empty list ⇒ prost omits field 33 ⇒ byte-identical to pre-P6 for all
            // existing (no-volume) callers. P6 attaches the cargo-cache volume here;
            // user volumes (`#[function(volumes=..)]`) attach DISTINCT-path mounts.
            volume_mounts: spec.volume_mounts.iter().map(|m| m.to_proto()).collect(),
            // Empty list ⇒ prost omits field 10 ⇒ byte-identical for all existing
            // (no-secret) callers. The user `#[function(secrets=..)]` path pushes
            // resolved secret ids here; Modal injects their key/values as ENV VARS.
            secret_ids: spec.secret_ids.clone(),
            ..Default::default()
        };

        // Sent with existing_function_id = precreate id + a fixed definition; the
        // server reconciles by precreate id, so re-sending the same definition
        // after a dropped response is idempotent (mirrors Python, which retries
        // FunctionCreate).
        let req = FunctionCreateRequest {
            function: Some(function),
            app_id: app_id.to_string(),
            existing_function_id: precreate_function_id.to_string(),
            function_data: None, // fix #1: XOR — never both.
            ..Default::default()
        };
        let stub = self.stub();
        let resp = retry_unary("function_create", || {
            let mut stub = stub.clone();
            let req = req.clone();
            async move { Ok(stub.function_create(req).await?.into_inner()) }
        })
        .await?;

        if resp.function_id.is_empty() {
            return Err(Error::build(
                "FunctionCreate returned an empty function_id".to_string(),
            ));
        }

        let definition_id = resp
            .handle_metadata
            .as_ref()
            .map(|h| h.definition_id.clone())
            .unwrap_or_default();

        Ok(CreatedFunction {
            function_id: resp.function_id,
            definition_id,
            warnings: resp
                .server_warnings
                .iter()
                .map(|w| w.message.clone())
                .collect(),
        })
    }

    /// `FunctionGet` / `from_name` (api.proto:4242). Resolves a deployed function
    /// to its invokable `function_id`.
    ///
    /// `object_tag` is the function name (e.g. `"handler"`). `environment`
    /// defaults to the configured environment (or `"main"`).
    pub async fn function_from_name(
        &mut self,
        app_name: &str,
        function_name: &str,
        environment: Option<&str>,
    ) -> Result<String> {
        let environment_name = self.env_or_default(environment);
        // Pure read — idempotent, safe to retry.
        let req = FunctionGetRequest {
            app_name: app_name.to_string(),
            object_tag: function_name.to_string(),
            environment_name,
            app_version: 0,
        };
        let stub = self.stub();
        let resp = retry_unary("function_get", || {
            let mut stub = stub.clone();
            let req = req.clone();
            async move { Ok(stub.function_get(req).await?.into_inner()) }
        })
        .await?;

        if resp.function_id.is_empty() {
            return Err(Error::build(format!(
                "FunctionGet for '{app_name}/{function_name}' returned an empty function_id"
            )));
        }
        Ok(resp.function_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_defaults_and_builders() {
        let spec = FunctionSpec::new("spike_wrapper", "handler", "im-123")
            .with_mount_id("mo-client")
            .with_timeout_secs(120);
        assert_eq!(spec.module_name, "spike_wrapper");
        assert_eq!(spec.function_name, "handler");
        assert_eq!(spec.image_id, "im-123");
        assert_eq!(spec.mount_ids, vec!["mo-client".to_string()]);
        assert_eq!(spec.timeout_secs, 120);
        // add_python images rely on worker-injected client deps by default.
        assert!(spec.mount_client_dependencies);
        // No volume by default (wire-identical to pre-P6).
        assert!(spec.volume_mounts.is_empty());
        // No secrets by default (wire-identical to before).
        assert!(spec.secret_ids.is_empty());
    }

    #[test]
    fn volume_mounts_default_empty() {
        let spec = FunctionSpec::new("m", "handler", "im-1");
        assert!(
            spec.volume_mounts.is_empty(),
            "volume_mounts must default empty (wire-identical to pre-P6)"
        );
    }

    #[test]
    fn secret_ids_default_empty() {
        let spec = FunctionSpec::new("m", "handler", "im-1");
        assert!(
            spec.secret_ids.is_empty(),
            "secret_ids must default empty (wire-identical to before)"
        );
    }

    #[test]
    fn with_secret_ids_attaches_and_flows_to_proto() {
        // Builder appends; the resolved ids flow into Function.secret_ids (field 10).
        let spec = FunctionSpec::new("m", "handler", "im-1")
            .with_secret_id("se-1")
            .with_secret_id("se-2");
        assert_eq!(
            spec.secret_ids,
            vec!["se-1".to_string(), "se-2".to_string()]
        );
        // `with_secret_ids` replaces.
        let replaced = spec.with_secret_ids(vec!["se-3".to_string()]);
        assert_eq!(replaced.secret_ids, vec!["se-3".to_string()]);
    }

    #[test]
    fn user_volume_and_cache_volume_coexist() {
        // A user volume (e.g. /data) and the P6 cargo-cache volume (/cache) attach as
        // TWO DISTINCT mounts on the SAME function — they must coexist, not collide.
        let spec = FunctionSpec::new("m", "handler", "im-1")
            .with_volume_mount("vo-cache", "/cache") // P6 cargo cache
            .with_volume_mount("vo-data", "/data"); // user volume
        assert_eq!(spec.volume_mounts.len(), 2);
        let cache = spec.volume_mounts[0].to_proto();
        let data = spec.volume_mounts[1].to_proto();
        assert_eq!(cache.volume_id, "vo-cache");
        assert_eq!(cache.mount_path, "/cache");
        assert_eq!(data.volume_id, "vo-data");
        assert_eq!(data.mount_path, "/data");
        // Distinct mount paths => independent mounts.
        assert_ne!(cache.mount_path, data.mount_path);
    }

    #[test]
    fn with_volume_mount_appends_and_to_proto() {
        let spec = FunctionSpec::new("m", "handler", "im-1").with_volume_mount("vo-1", "/cache");
        assert_eq!(spec.volume_mounts.len(), 1);
        let m = spec.volume_mounts[0].to_proto();
        assert_eq!(m.volume_id, "vo-1");
        assert_eq!(m.mount_path, "/cache");
        // Cargo cache: writable + background commits, no sub_path.
        assert!(m.allow_background_commits, "bg-commits ON for cargo cache");
        assert!(!m.read_only, "cargo cache must be writable");
        assert!(m.sub_path.is_none(), "sub_path (field 5) unset");
    }

    #[test]
    fn mount_client_dependencies_defaults_true_and_is_overridable() {
        let spec = FunctionSpec::new("m", "handler", "im-1");
        assert!(spec.mount_client_dependencies);
        let off = spec.with_mount_client_dependencies(false);
        assert!(!off.mount_client_dependencies);
    }

    #[test]
    fn supported_formats_are_pickle_and_cbor() {
        assert_eq!(
            supported_formats(),
            vec![DataFormat::Pickle as i32, DataFormat::Cbor as i32]
        );
    }

    #[test]
    fn resources_default_is_zero() {
        let r = FunctionResources::default().to_proto();
        assert_eq!(r.memory_mb, 0);
        assert_eq!(r.milli_cpu, 0);
        // CPU-only default: gpu_config (proto field 4) stays UNSET — wire-identical
        // to before the GPU addition.
        assert!(
            r.gpu_config.is_none(),
            "CPU default must leave gpu_config unset"
        );
    }

    #[test]
    fn parse_gpu_config_mirrors_python() {
        // "TYPE" -> gpu_type uppercased, count 1, deprecated type field 0.
        let g = parse_gpu_config("T4").unwrap();
        assert_eq!(g.gpu_type, "T4");
        assert_eq!(g.count, 1);
        assert_eq!(g.r#type, 0);

        // Lowercase is uppercased (`.upper()`).
        assert_eq!(parse_gpu_config("t4").unwrap().gpu_type, "T4");

        // "TYPE:count" -> count parsed; default split on FIRST ':'.
        let h = parse_gpu_config("H100:4").unwrap();
        assert_eq!(h.gpu_type, "H100");
        assert_eq!(h.count, 4);

        // MEM suffix is NOT split — rides inside gpu_type verbatim (uppercased).
        let a = parse_gpu_config("A100-80GB").unwrap();
        assert_eq!(a.gpu_type, "A100-80GB");
        assert_eq!(a.count, 1);

        // MEM suffix + count.
        let a2 = parse_gpu_config("A100-80GB:2").unwrap();
        assert_eq!(a2.gpu_type, "A100-80GB");
        assert_eq!(a2.count, 2);

        // Non-integer count -> Err (mirrors Python InvalidError).
        assert!(parse_gpu_config("T4:x").is_err());
    }

    #[test]
    fn to_proto_populates_gpu_config_when_set() {
        let r = FunctionResources {
            gpu: Some("T4".to_string()),
            ..Default::default()
        }
        .to_proto();
        let g = r
            .gpu_config
            .expect("gpu_config must be set when gpu is Some");
        assert_eq!(g.gpu_type, "T4");
        assert_eq!(g.count, 1);
    }

    #[test]
    fn with_gpu_populates_field_4_and_validates() {
        // `with_gpu(Some("T4"))` populates the nested GPUConfig (proto field 4).
        let spec = FunctionSpec::new("m", "handler", "im-1")
            .with_gpu(Some("T4"))
            .unwrap();
        let g = spec
            .resources
            .to_proto()
            .gpu_config
            .expect("gpu_config must be set");
        assert_eq!(g.gpu_type, "T4");
        assert_eq!(g.count, 1);

        // `with_gpu(None)` is CPU (no gpu_config).
        let cpu = FunctionSpec::new("m", "handler", "im-1")
            .with_gpu(None::<String>)
            .unwrap();
        assert!(cpu.resources.to_proto().gpu_config.is_none());

        // A bad count is rejected UP FRONT at set time.
        assert!(FunctionSpec::new("m", "handler", "im-1")
            .with_gpu(Some("T4:nope"))
            .is_err());
    }
}
