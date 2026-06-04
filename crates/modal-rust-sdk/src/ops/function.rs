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
    GpuConfig, Resources,
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
