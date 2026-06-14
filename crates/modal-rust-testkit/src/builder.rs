//! [`MockModalBuilder`] — the ergonomic per-test response config + `start()`.
//!
//! `MockModal::builder()` returns a builder whose default is the HAPPY PATH for a
//! deploy / call / remote flow. Override the canned function result and/or any
//! steerable RPC, then `.start()` to bind a loopback port and get the live handle.
//!
//! ```no_run
//! use modal_rust_testkit::prelude::*;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let mock = MockModal::builder()
//!     .function_result_value(serde_json::json!({ "sum": 42 }))
//!     .start()
//!     .await?;
//! # let _ = mock;
//! # Ok(())
//! # }
//! ```

use serde_json::Value;
use tonic::Status;

use crate::proto::api as gen;
use crate::responder::{FunctionBody, OverrideFn, Responses, ResultMode};
use crate::server::MockModal;

/// Builder for a [`MockModal`]. Configures the canned function result + per-RPC
/// override closures, then [`MockModalBuilder::start`]s the in-process server.
#[derive(Default)]
pub struct MockModalBuilder {
    responses: Responses,
}

impl MockModalBuilder {
    /// Pin the canned function output `value`: `function_get_outputs` returns the
    /// success envelope `{"ok":true,"value":<value>}` (CBOR-encoded), which the
    /// facade decodes back into the typed `Out`. The common case for a call test.
    pub fn function_result_value(mut self, value: Value) -> Self {
        self.responses.result_mode = ResultMode::Value(value);
        self
    }

    /// Return an EXACT, verbatim runner-envelope string — e.g. an ERROR envelope
    /// `{"ok":false,"error":{...}}` to drive the facade's `parse_envelope` error
    /// taxonomy offline.
    pub fn function_result_envelope(mut self, envelope: impl Into<String>) -> Self {
        self.responses.result_mode = ResultMode::Envelope(envelope.into());
        self
    }

    /// Compute the canned output `value` FROM the decoded input-JSON string (Python
    /// `function_body` parity), wrapped as `{"ok":true,"value":<f(input)>}`. Lets a
    /// table test compute a per-case output from the per-case input.
    ///
    /// The closure receives the raw `input_json` string the facade sent (`args.1`
    /// of the `(entrypoint, input_json)` tuple).
    pub fn function_body<F>(mut self, f: F) -> Self
    where
        F: Fn(&str) -> Value + Send + Sync + 'static,
    {
        let boxed: FunctionBody = Box::new(f);
        self.responses.result_mode = ResultMode::Body(boxed);
        self
    }

    /// ESCAPE HATCH: fully steer `function_get_outputs` (e.g. return a FAILURE
    /// `GenericResult`, a blob result, or a specific shape). Returning `Err(Status)`
    /// drives the SDK's transport-error paths.
    pub fn on_function_get_outputs<F>(mut self, f: F) -> Self
    where
        F: Fn(&gen::FunctionGetOutputsRequest) -> Result<gen::FunctionGetOutputsResponse, Status>
            + Send
            + Sync
            + 'static,
    {
        self.responses.on_function_get_outputs = Some(boxed(f));
        self
    }

    /// ESCAPE HATCH: steer `app_get_or_create` (e.g. a specific `app_id` or an error).
    pub fn on_app_get_or_create<F>(mut self, f: F) -> Self
    where
        F: Fn(&gen::AppGetOrCreateRequest) -> Result<gen::AppGetOrCreateResponse, Status>
            + Send
            + Sync
            + 'static,
    {
        self.responses.on_app_get_or_create = Some(boxed(f));
        self
    }

    /// ESCAPE HATCH: steer `app_create` (the ephemeral RUN app).
    pub fn on_app_create<F>(mut self, f: F) -> Self
    where
        F: Fn(&gen::AppCreateRequest) -> Result<gen::AppCreateResponse, Status>
            + Send
            + Sync
            + 'static,
    {
        self.responses.on_app_create = Some(boxed(f));
        self
    }

    /// ESCAPE HATCH: steer `environment_get_or_create` (e.g. pin an image builder
    /// version).
    pub fn on_environment_get_or_create<F>(mut self, f: F) -> Self
    where
        F: Fn(
                &gen::EnvironmentGetOrCreateRequest,
            ) -> Result<gen::EnvironmentGetOrCreateResponse, Status>
            + Send
            + Sync
            + 'static,
    {
        self.responses.on_environment_get_or_create = Some(boxed(f));
        self
    }

    /// ESCAPE HATCH: steer `image_get_or_create` (e.g. force a build FAILURE).
    pub fn on_image_get_or_create<F>(mut self, f: F) -> Self
    where
        F: Fn(&gen::ImageGetOrCreateRequest) -> Result<gen::ImageGetOrCreateResponse, Status>
            + Send
            + Sync
            + 'static,
    {
        self.responses.on_image_get_or_create = Some(boxed(f));
        self
    }

    /// ESCAPE HATCH: steer `function_precreate`.
    pub fn on_function_precreate<F>(mut self, f: F) -> Self
    where
        F: Fn(&gen::FunctionPrecreateRequest) -> Result<gen::FunctionPrecreateResponse, Status>
            + Send
            + Sync
            + 'static,
    {
        self.responses.on_function_precreate = Some(boxed(f));
        self
    }

    /// ESCAPE HATCH: steer `function_create` (e.g. a specific `function_id` /
    /// `definition_id`, or an error to drive the SDK's empty-id guard).
    pub fn on_function_create<F>(mut self, f: F) -> Self
    where
        F: Fn(&gen::FunctionCreateRequest) -> Result<gen::FunctionCreateResponse, Status>
            + Send
            + Sync
            + 'static,
    {
        self.responses.on_function_create = Some(boxed(f));
        self
    }

    /// ESCAPE HATCH: steer `function_get` (the DEPLOYED `call` lookup).
    pub fn on_function_get<F>(mut self, f: F) -> Self
    where
        F: Fn(&gen::FunctionGetRequest) -> Result<gen::FunctionGetResponse, Status>
            + Send
            + Sync
            + 'static,
    {
        self.responses.on_function_get = Some(boxed(f));
        self
    }

    /// ESCAPE HATCH: steer `function_map` (e.g. return EMPTY `pipelined_inputs` to
    /// force the SDK's fix-#3 `FunctionPutInputs` fallback, exercising the MAP path).
    pub fn on_function_map<F>(mut self, f: F) -> Self
    where
        F: Fn(&gen::FunctionMapRequest) -> Result<gen::FunctionMapResponse, Status>
            + Send
            + Sync
            + 'static,
    {
        self.responses.on_function_map = Some(boxed(f));
        self
    }

    /// ESCAPE HATCH: steer `secret_get_or_create`.
    pub fn on_secret_get_or_create<F>(mut self, f: F) -> Self
    where
        F: Fn(&gen::SecretGetOrCreateRequest) -> Result<gen::SecretGetOrCreateResponse, Status>
            + Send
            + Sync
            + 'static,
    {
        self.responses.on_secret_get_or_create = Some(boxed(f));
        self
    }

    /// ESCAPE HATCH: steer `volume_get_or_create`.
    pub fn on_volume_get_or_create<F>(mut self, f: F) -> Self
    where
        F: Fn(&gen::VolumeGetOrCreateRequest) -> Result<gen::VolumeGetOrCreateResponse, Status>
            + Send
            + Sync
            + 'static,
    {
        self.responses.on_volume_get_or_create = Some(boxed(f));
        self
    }

    /// ESCAPE HATCH: steer `volume_put_files2` (e.g. force a `missing_blocks`
    /// list to exercise the upload loop, or an overwrite error). The default
    /// returns an EMPTY `missing_blocks` (the upload converges in one round).
    pub fn on_volume_put_files2<F>(mut self, f: F) -> Self
    where
        F: Fn(&gen::VolumePutFiles2Request) -> Result<gen::VolumePutFiles2Response, Status>
            + Send
            + Sync
            + 'static,
    {
        self.responses.on_volume_put_files2 = Some(boxed(f));
        self
    }

    /// ESCAPE HATCH: steer `dict_get_or_create` (e.g. a specific `dict_id` or a
    /// resolve-time error `Status`). NOTE: an override BYPASSES the stateful
    /// mock store for this RPC, so the returned id won't be backed by store
    /// state — pair it with error-path tests, not round-trip tests.
    pub fn on_dict_get_or_create<F>(mut self, f: F) -> Self
    where
        F: Fn(&gen::DictGetOrCreateRequest) -> Result<gen::DictGetOrCreateResponse, Status>
            + Send
            + Sync
            + 'static,
    {
        self.responses.on_dict_get_or_create = Some(boxed(f));
        self
    }

    /// ESCAPE HATCH: steer `queue_get_or_create` — same bypass caveat as
    /// [`on_dict_get_or_create`](Self::on_dict_get_or_create).
    pub fn on_queue_get_or_create<F>(mut self, f: F) -> Self
    where
        F: Fn(&gen::QueueGetOrCreateRequest) -> Result<gen::QueueGetOrCreateResponse, Status>
            + Send
            + Sync
            + 'static,
    {
        self.responses.on_queue_get_or_create = Some(boxed(f));
        self
    }

    /// Bind a loopback port, start the in-process server task, and return the live
    /// [`MockModal`] handle. The handle owns the task (aborted on `Drop`).
    pub async fn start(self) -> std::io::Result<MockModal> {
        MockModal::start_with_responses(self.responses).await
    }
}

/// Box a per-RPC override closure into the type-erased [`OverrideFn`].
fn boxed<Req, Resp, F>(f: F) -> OverrideFn<Req, Resp>
where
    F: Fn(&Req) -> Result<Resp, Status> + Send + Sync + 'static,
{
    Box::new(f)
}
