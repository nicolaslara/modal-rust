//! The wire side: pure `build_*` request builders, the [`CreatedFunction`]
//! response projection, and the `ModalClient` RPC methods. Split out of
//! `function.rs` mechanically (M1); the byte-for-byte FILE-mode invariants
//! (fix #1) live in [`build_function_create_request`]'s docs.

use super::spec::FunctionSpec;
use crate::client::ModalClient;
use crate::error::{Error, Result};
use crate::proto::api::function::{DefinitionType, FunctionType};
use crate::proto::api::{
    DataFormat, Function, FunctionCreateRequest, FunctionCreateResponse, FunctionGetRequest,
    FunctionPrecreateRequest, WebhookConfig, WebhookType,
};

/// Result of [`ModalClient::function_create`].
#[derive(Debug, Clone, Default)]
pub struct CreatedFunction {
    /// The created function id.
    pub function_id: String,
    /// `definition_id` from the create's `handle_metadata` (for `AppPublish`'s
    /// `definition_ids` map). Empty if the server did not return one.
    pub definition_id: String,
    /// Assigned web-endpoint URL from the create's `handle_metadata.web_url`
    /// (e.g. `https://{workspace}--{app}-{fn}.modal.run`). EMPTY for non-webhook
    /// functions — Modal only assigns a URL when `webhook_config` is set.
    pub web_url: String,
    /// Advisory server warnings (rendered text).
    pub warnings: Vec<String>,
}

/// The CBOR + PICKLE formats we advertise/support end-to-end.
pub(super) fn supported_formats() -> Vec<i32> {
    vec![DataFormat::Pickle as i32, DataFormat::Cbor as i32]
}

/// Build the `FunctionPrecreate` request (api.proto:4250) — pure, no I/O.
///
/// Extracted from [`ModalClient::function_precreate`]. Advertises `[PICKLE, CBOR]`
/// for both input and output formats; `function_type = FUNCTION`.
pub(crate) fn build_function_precreate_request(
    app_id: &str,
    function_name: &str,
) -> FunctionPrecreateRequest {
    FunctionPrecreateRequest {
        app_id: app_id.to_string(),
        function_name: function_name.to_string(),
        function_type: FunctionType::Function as i32,
        supported_input_formats: supported_formats(),
        supported_output_formats: supported_formats(),
        ..Default::default()
    }
}

/// Build the `FunctionCreate` request (api.proto:4240) in FILE mode — **fix #1**,
/// pure, no I/O.
///
/// Extracted from [`ModalClient::function_create`]; moves BOTH the inner [`Function`]
/// build AND the request wrapper. The byte-for-byte invariants this pins:
/// - the XOR — `function` is set, `function_data` is left `None` (NEVER both);
/// - `existing_function_id == precreate_function_id` (legalizes empty
///   `function_serialized` in FILE mode);
/// - `function_serialized` empty, `definition_type = FILE`, `function_type = FUNCTION`;
/// - `resources` ALWAYS set (fix #1);
/// - empty `volume_mounts` / `secret_ids` and a `None` `retry_policy` / `schedule` ⇒
///   prost omits those fields ⇒ wire-identical to before P6 / before secrets / before
///   retries / before schedule for existing callers.
pub(crate) fn build_function_create_request(
    app_id: &str,
    precreate_function_id: &str,
    spec: &FunctionSpec,
) -> FunctionCreateRequest {
    // Object tag vs in-container callable: `Function.function_name` is the
    // app-namespace object TAG (unique within the app). When the caller decoupled it
    // (set `app_function_name` to the entrypoint), the in-container `getattr` target
    // moves to `implementation_name` (Modal's `@app.function(name=..)` mechanism), so
    // every entrypoint shares ONE callable (`spec.function_name`, e.g. "handler") while
    // owning a DISTINCT tag + config. When NOT decoupled (single-callable shape), the
    // tag == the callable and `implementation_name` stays empty — byte-identical wire.
    let object_tag = spec.object_tag().to_string();
    let implementation_name = if object_tag == spec.function_name {
        String::new()
    } else {
        spec.function_name.clone()
    };
    // Autoscaler: the modern `autoscaler_settings` carries the knobs; when ANY knob is
    // set, Modal ALSO populates the deprecated mirror fields from the same values
    // (`_functions.py:1019-1022`) for server-side backward-compat. An all-`None`
    // autoscaler leaves `autoscaler_settings` unset AND every legacy field at `0` ⇒
    // prost omits them ⇒ byte-identical to before for non-autoscaling callers.
    let autoscaler_empty = spec.autoscaler.is_empty();
    let autoscaler_settings = if autoscaler_empty {
        None
    } else {
        Some(spec.autoscaler.to_settings())
    };
    let warm_pool_size = spec.autoscaler.min_containers.unwrap_or(0);
    let concurrency_limit = spec.autoscaler.max_containers.unwrap_or(0);
    let experimental_buffer_containers = spec.autoscaler.buffer_containers.unwrap_or(0);
    let task_idle_timeout_secs = spec.autoscaler.scaledown_window.unwrap_or(0);
    // Web endpoint: `None` ⇒ no `webhook_config` (prost omits field 15) AND the
    // advertised formats stay `[PICKLE, CBOR]` ⇒ byte-identical to before web
    // endpoints. `Some` ⇒ a FUNCTION-type webhook rides field 15 AND the formats
    // swap to the ASGI pair Modal's web layer requires (spike finding 3 — advertising
    // PICKLE on a webhook makes modal-http reject the ASGI response). `function_type`
    // stays FUNCTION either way (webhooks cannot be generators at the user level).
    let webhook_config = spec.webhook.as_ref().map(|w| WebhookConfig {
        r#type: WebhookType::Function as i32,
        method: w.method.clone(),
        requires_proxy_auth: w.requires_proxy_auth,
        ..Default::default()
    });
    let (supported_input_formats, supported_output_formats) = if spec.webhook.is_some() {
        (
            vec![DataFormat::Asgi as i32],
            vec![DataFormat::Asgi as i32, DataFormat::GeneratorDone as i32],
        )
    } else {
        (supported_formats(), supported_formats())
    };
    let function = Function {
        module_name: spec.module_name.clone(),
        function_name: object_tag,
        implementation_name,
        mount_ids: spec.mount_ids.clone(),
        image_id: spec.image_id.clone(),
        function_serialized: Vec::new(), // FILE mode: empty.
        definition_type: DefinitionType::File as i32,
        function_type: FunctionType::Function as i32,
        resources: Some(spec.resources.to_proto()), // fix #1: always set.
        timeout_secs: spec.timeout_secs,
        supported_input_formats,
        supported_output_formats,
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
        // `None` ⇒ prost omits field 18 ⇒ byte-identical for all existing
        // (no-retry) callers. The user `#[function(retries = N)]` path sets a
        // fixed-interval policy here so Modal auto-retries a failed call.
        retry_policy: spec.retry_policy,
        // `None` ⇒ prost omits field 72 ⇒ byte-identical for all existing
        // (no-schedule) callers. The user `#[function(schedule = Cron(..)/Period(..))]`
        // path sets a Cron/Period here so Modal runs the DEPLOYED function on a cadence.
        schedule: spec.schedule.clone(),
        // Autoscaler: modern `autoscaler_settings` (field 79) + the deprecated mirror
        // fields Modal still sets (`warm_pool_size`/`concurrency_limit`/
        // `_experimental_buffer_containers`/`task_idle_timeout_secs`). An all-`None`
        // autoscaler leaves `autoscaler_settings` = None AND every legacy field at `0`,
        // so prost omits all of them ⇒ byte-identical for non-autoscaling callers.
        autoscaler_settings,
        warm_pool_size,
        concurrency_limit,
        experimental_buffer_containers,
        task_idle_timeout_secs,
        // Memory snapshot (proto fields 40 + 41). `false` (the default) ⇒ prost omits
        // both ⇒ byte-identical for every non-snapshot function. The facade only sets
        // `spec.checkpointing_enabled` on the DEPLOY boundary, so RUN stays wire-identical.
        checkpointing_enabled: spec.checkpointing_enabled,
        is_checkpointing_function: spec.checkpointing_enabled,
        // `None` ⇒ prost omits field 15 ⇒ byte-identical for every non-endpoint
        // function. The facade only sets `spec.webhook` on the DEPLOY boundary,
        // so RUN stays wire-identical.
        webhook_config,
        ..Default::default()
    };

    FunctionCreateRequest {
        function: Some(function),
        app_id: app_id.to_string(),
        existing_function_id: precreate_function_id.to_string(),
        function_data: None, // fix #1: XOR — never both.
        ..Default::default()
    }
}

/// Project a `FunctionCreateResponse` into [`CreatedFunction`] — pure, no I/O.
///
/// Extracted from [`ModalClient::function_create`]. Surfaces `definition_id` (for
/// `AppPublish`) and `web_url` (the assigned endpoint URL; empty for non-webhooks)
/// from the create's `handle_metadata`, and renders the advisory server warnings.
/// An empty `function_id` maps to [`Error::build`].
pub(crate) fn created_function_from_response(
    resp: FunctionCreateResponse,
) -> Result<CreatedFunction> {
    if resp.function_id.is_empty() {
        return Err(Error::build(
            "FunctionCreate returned an empty function_id".to_string(),
        ));
    }
    let (definition_id, web_url) = resp
        .handle_metadata
        .as_ref()
        .map(|h| (h.definition_id.clone(), h.web_url.clone()))
        .unwrap_or_default();
    Ok(CreatedFunction {
        function_id: resp.function_id,
        definition_id,
        web_url,
        warnings: resp
            .server_warnings
            .iter()
            .map(|w| w.message.clone())
            .collect(),
    })
}

/// Build the `FunctionGet` / `from_name` request (api.proto:4242) — pure, no I/O.
///
/// Extracted from [`ModalClient::function_from_name`]; the method passes the
/// resolved `environment_name`. `object_tag` is the function name; `app_version`
/// stays `0` (latest).
pub(crate) fn build_function_get_request(
    app_name: &str,
    function_name: &str,
    environment_name: String,
) -> FunctionGetRequest {
    FunctionGetRequest {
        app_name: app_name.to_string(),
        object_tag: function_name.to_string(),
        environment_name,
        app_version: 0,
    }
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
        let req = build_function_precreate_request(app_id, function_name);
        let resp = self
            .retry_rpc("function_precreate", req, |mut stub, req| async move {
                stub.function_precreate(req).await
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
        // Sent with existing_function_id = precreate id + a fixed definition; the
        // server reconciles by precreate id, so re-sending the same definition
        // after a dropped response is idempotent (mirrors Python, which retries
        // FunctionCreate).
        let req = build_function_create_request(app_id, precreate_function_id, spec);
        let resp = self
            .retry_rpc("function_create", req, |mut stub, req| async move {
                stub.function_create(req).await
            })
            .await?;

        created_function_from_response(resp)
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
        let req = build_function_get_request(app_name, function_name, environment_name);
        let resp = self
            .retry_rpc("function_get", req, |mut stub, req| async move {
                stub.function_get(req).await
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
