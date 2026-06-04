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
    Resources,
};
use crate::retry::retry_unary;

/// CPU/memory request for a created function. FILE-mode CPU functions can use the
/// zero default (`Resources::default()`); set modest values to be explicit.
#[derive(Debug, Clone, Default)]
pub struct FunctionResources {
    /// Requested memory (MiB). `0` = server default.
    pub memory_mb: u32,
    /// Requested CPU (milli-cores). `0` = server default.
    pub milli_cpu: u32,
}

impl FunctionResources {
    fn to_proto(&self) -> Resources {
        Resources {
            memory_mb: self.memory_mb,
            milli_cpu: self.milli_cpu,
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
    }
}
