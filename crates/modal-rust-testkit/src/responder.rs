//! Ergonomic response configuration: happy-path DEFAULTS + per-test OVERRIDES.
//!
//! Mirrors the Python mock's `add_response` / `set_responder` / `function_body`.
//! Two override surfaces (both wired through [`crate::builder::MockModalBuilder`]):
//!
//! - The "fake function body" ([`FunctionBody`]): given the decoded input JSON
//!   string, produce the runner-envelope the `FunctionGetOutputs` poll returns.
//!   The common case (`.function_result_value`) just pins a canned output; the
//!   `.function_body` form computes it from the input (Python parity).
//! - Per-RPC escape hatches (`.on_<rpc>(|req| Ok(resp))`) to force a specific id,
//!   a server warning, or an error `Status` on any steerable RPC.
//!
//! All ids are DETERMINISTIC (`ap-1`, `im-1`, `fu-1`, …): no `Date`, no random.

use serde_json::Value;
use tonic::Status;

use crate::proto::api as gen;

/// A per-RPC override closure: `Fn(&Req) -> Result<Resp, Status>`. Returning
/// `Err(Status)` lets a test drive the SDK's error paths offline.
pub(crate) type OverrideFn<Req, Resp> =
    Box<dyn Fn(&Req) -> Result<Resp, Status> + Send + Sync + 'static>;

/// The fake function body: maps the decoded input-JSON string (the second element
/// of the `(entrypoint, input_json)` args tuple `.remote()` sends) to the output
/// `value` that gets wrapped in a success envelope.
pub(crate) type FunctionBody = Box<dyn Fn(&str) -> Value + Send + Sync + 'static>;

/// How `function_get_outputs` should build the runner ENVELOPE it returns.
#[derive(Default)]
pub(crate) enum ResultMode {
    /// Echo the decoded input back as `{"ok":true,"value":<input>}` — a useful
    /// default for round-trip / identity tests.
    #[default]
    EchoInput,
    /// A fixed output `value`, wrapped as `{"ok":true,"value":<value>}`.
    Value(Value),
    /// An exact, verbatim envelope string (for error-envelope cases).
    Envelope(String),
    /// Compute the output `value` from the decoded input JSON, wrapped as
    /// `{"ok":true,"value":<body(input)>}` (Python's `function_body`).
    Body(FunctionBody),
}

/// Per-test response config. `Default` = the happy path for a deploy / call /
/// remote flow. Built by [`crate::builder::MockModalBuilder`] and consumed by the
/// servicer.
#[derive(Default)]
pub(crate) struct Responses {
    /// How the canned `FunctionGetOutputs` result is produced.
    pub(crate) result_mode: ResultMode,
    // Per-RPC escape hatches. One optional hook per RPC a test may want to steer.
    pub(crate) on_app_get_or_create:
        Option<OverrideFn<gen::AppGetOrCreateRequest, gen::AppGetOrCreateResponse>>,
    pub(crate) on_app_create: Option<OverrideFn<gen::AppCreateRequest, gen::AppCreateResponse>>,
    pub(crate) on_environment_get_or_create:
        Option<OverrideFn<gen::EnvironmentGetOrCreateRequest, gen::EnvironmentGetOrCreateResponse>>,
    pub(crate) on_image_get_or_create:
        Option<OverrideFn<gen::ImageGetOrCreateRequest, gen::ImageGetOrCreateResponse>>,
    pub(crate) on_function_precreate:
        Option<OverrideFn<gen::FunctionPrecreateRequest, gen::FunctionPrecreateResponse>>,
    pub(crate) on_function_create:
        Option<OverrideFn<gen::FunctionCreateRequest, gen::FunctionCreateResponse>>,
    pub(crate) on_function_get:
        Option<OverrideFn<gen::FunctionGetRequest, gen::FunctionGetResponse>>,
    pub(crate) on_function_map:
        Option<OverrideFn<gen::FunctionMapRequest, gen::FunctionMapResponse>>,
    pub(crate) on_function_get_outputs:
        Option<OverrideFn<gen::FunctionGetOutputsRequest, gen::FunctionGetOutputsResponse>>,
    pub(crate) on_secret_get_or_create:
        Option<OverrideFn<gen::SecretGetOrCreateRequest, gen::SecretGetOrCreateResponse>>,
    pub(crate) on_volume_get_or_create:
        Option<OverrideFn<gen::VolumeGetOrCreateRequest, gen::VolumeGetOrCreateResponse>>,
}

impl Responses {
    /// Build the runner ENVELOPE STRING for a decoded `input_json` per the
    /// configured [`ResultMode`]. The servicer CBOR-encodes this so the SDK's
    /// `invoke_cbor::<_, _, String>` decodes it back byte-identically.
    ///
    /// `input_json` is the raw input-JSON string the facade sends as `args.1` of
    /// the `(entrypoint, input_json)` args tuple (see [`crate::servicer`]).
    pub(crate) fn envelope_for(&self, input_json: &str) -> String {
        match &self.result_mode {
            ResultMode::EchoInput => {
                let value: Value = serde_json::from_str(input_json).unwrap_or(Value::Null);
                wrap_success(value)
            }
            ResultMode::Value(v) => wrap_success(v.clone()),
            ResultMode::Envelope(s) => s.clone(),
            // The body sees the RAW input-JSON string (Python parity), and returns
            // the output `value` to wrap in a success envelope.
            ResultMode::Body(f) => wrap_success(f(input_json)),
        }
    }
}

/// Wrap an output `value` as the runner success envelope `{"ok":true,"value":..}`.
fn wrap_success(value: Value) -> String {
    serde_json::json!({ "ok": true, "value": value }).to_string()
}
