//! Function invocation: `FunctionMap` → `FunctionPutInputs` fallback → poll
//! `FunctionGetOutputs`, with the CBOR `(args, kwargs)` codec.
//!
//! ## Fix #3 — fall back to `FunctionPutInputs` when `FunctionMap` does not enqueue
//!
//! `FunctionMap` (with `pipelined_inputs`) usually enqueues the input directly and
//! echoes it back in `FunctionMapResponse.pipelined_inputs`. modal-rs assumed that
//! always happened and went straight to polling → "Function call not found". We
//! check: if `pipelined_inputs` comes back EMPTY, the input was NOT enqueued, so we
//! call `FunctionPutInputs` to actually enqueue it before polling outputs.
//!
//! ## Encoding
//!
//! The payload is the 2-tuple `(args, kwargs)` — `args` a positional sequence,
//! `kwargs` a map — CBOR-encoded ([`crate::codec`]) with
//! `FunctionInput.data_format = DATA_FORMAT_CBOR`. Outputs are decoded per
//! `FunctionGetOutputsItem.data_format` (CBOR → decode; PICKLE → opaque bytes).

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::client::ModalClient;
use crate::codec;
use crate::error::{Error, Result};
use crate::ops::{describe_failure, result_status, ResultState};
use crate::proto::api::function_input::ArgsOneof;
use crate::proto::api::generic_result::DataOneof;
use crate::proto::api::{
    DataFormat, FunctionCallInvocationType, FunctionCallType, FunctionGetOutputsRequest,
    FunctionInput, FunctionMapRequest, FunctionPutInputsItem, FunctionPutInputsRequest,
};

/// Per-poll timeout (seconds) for `FunctionGetOutputs` long-poll reconnects.
const OUTPUTS_TIMEOUT_SECS: f32 = 55.0;
/// Safety cap on total wall-clock time spent waiting for a function output.
const INVOKE_DEADLINE: Duration = Duration::from_secs(600);

/// A decoded function output plus the wire format it arrived in.
#[derive(Debug, Clone)]
pub struct Invocation {
    /// Raw result bytes (inline `GenericResult.data`). Empty if the function
    /// returned nothing inline (blob results are deferred — see below).
    pub data: Vec<u8>,
    /// The wire format of `data` (`DATA_FORMAT_CBOR` for our recipe).
    pub data_format: DataFormat,
}

impl Invocation {
    /// Decode the output as CBOR into `T`. Errors if the output was not CBOR.
    pub fn decode_cbor<T: DeserializeOwned>(&self) -> Result<T> {
        if self.data_format != DataFormat::Cbor {
            return Err(Error::codec(format!(
                "expected CBOR output, got {}",
                self.data_format.as_str_name()
            )));
        }
        codec::decode(&self.data)
    }
}

impl ModalClient {
    /// Invoke `function_id` with CBOR-encoded `args` (a serializable positional
    /// tuple) and `kwargs` (a serializable map), returning the decoded result `R`.
    ///
    /// Encodes `(args, kwargs)` as CBOR, drives the
    /// `FunctionMap` → `FunctionPutInputs` (fix #3) → `FunctionGetOutputs` path,
    /// and decodes the CBOR output. A terminal remote failure surfaces as
    /// [`Error::Build`] carrying the function's `exception`/`traceback`.
    pub async fn invoke_cbor<A, K, R>(
        &mut self,
        function_id: &str,
        args: &A,
        kwargs: &K,
    ) -> Result<R>
    where
        A: Serialize,
        K: Serialize,
        R: DeserializeOwned,
    {
        let payload = (args, kwargs);
        let encoded = codec::encode(&payload)?;
        let invocation = self.invoke_raw(function_id, encoded).await?;
        invocation.decode_cbor()
    }

    /// Low-level invoke: enqueue already-CBOR-encoded `(args, kwargs)` bytes and
    /// return the raw [`Invocation`] (bytes + format) without decoding.
    ///
    /// Drives the full fix-#3 sequence. Inline args only (small payloads); blob
    /// upload for oversized args is deferred to a later milestone.
    pub async fn invoke_raw(
        &mut self,
        function_id: &str,
        args_serialized: Vec<u8>,
    ) -> Result<Invocation> {
        let item = FunctionPutInputsItem {
            idx: 0,
            input: Some(FunctionInput {
                data_format: DataFormat::Cbor as i32,
                final_input: false,
                method_name: None,
                args_oneof: Some(ArgsOneof::Args(args_serialized)),
            }),
            ..Default::default()
        };

        // Step 1 — FunctionMap with the input pipelined.
        let map = self
            .inner_mut()
            .function_map(FunctionMapRequest {
                function_id: function_id.to_string(),
                function_call_type: FunctionCallType::Unary as i32,
                function_call_invocation_type: FunctionCallInvocationType::Sync as i32,
                pipelined_inputs: vec![item.clone()],
                ..Default::default()
            })
            .await?
            .into_inner();

        let function_call_id = map.function_call_id;
        if function_call_id.is_empty() {
            return Err(Error::build(
                "FunctionMap returned an empty function_call_id".to_string(),
            ));
        }

        // Step 2 — fix #3: if the input was NOT pipelined (echoed back), enqueue it.
        if map.pipelined_inputs.is_empty() {
            let put = self
                .inner_mut()
                .function_put_inputs(FunctionPutInputsRequest {
                    function_id: function_id.to_string(),
                    function_call_id: function_call_id.clone(),
                    inputs: vec![item],
                })
                .await?
                .into_inner();
            if put.inputs.is_empty() {
                return Err(Error::build(
                    "FunctionPutInputs accepted no inputs (input queue full?)".to_string(),
                ));
            }
        }

        // Step 3 — poll FunctionGetOutputs until the result arrives.
        self.poll_outputs(&function_call_id).await
    }

    /// Long-poll `FunctionGetOutputs` (api.proto:4247), advancing `last_entry_id`,
    /// until an output for the call arrives; decode its terminal `GenericResult`.
    async fn poll_outputs(&mut self, function_call_id: &str) -> Result<Invocation> {
        let started = std::time::Instant::now();
        let mut last_entry_id = String::new();

        loop {
            if started.elapsed() > INVOKE_DEADLINE {
                return Err(Error::build(format!(
                    "function call {function_call_id} produced no output within {}s",
                    INVOKE_DEADLINE.as_secs()
                )));
            }

            let resp = self
                .inner_mut()
                .function_get_outputs(FunctionGetOutputsRequest {
                    function_call_id: function_call_id.to_string(),
                    max_values: 1,
                    timeout: OUTPUTS_TIMEOUT_SECS,
                    last_entry_id: last_entry_id.clone(),
                    clear_on_success: true,
                    ..Default::default()
                })
                .await?
                .into_inner();

            if !resp.last_entry_id.is_empty() {
                last_entry_id = resp.last_entry_id;
            }

            if let Some(item) = resp.outputs.into_iter().next() {
                let data_format =
                    DataFormat::try_from(item.data_format).unwrap_or(DataFormat::Unspecified);
                let result = item.result.ok_or_else(|| {
                    Error::build("FunctionGetOutputs item had no result".to_string())
                })?;

                match result_status(Some(&result)) {
                    ResultState::Success => {
                        let data = match result.data_oneof {
                            Some(DataOneof::Data(bytes)) => bytes,
                            Some(DataOneof::DataBlobId(_)) => {
                                return Err(Error::build(
                                    "function returned a blob result; blob fetch is not yet \
                                     implemented (inline results only for now)"
                                        .to_string(),
                                ));
                            }
                            None => Vec::new(),
                        };
                        return Ok(Invocation { data, data_format });
                    }
                    ResultState::Failure(status) => {
                        return Err(Error::build(describe_failure(
                            "function call",
                            status,
                            &result,
                        )));
                    }
                    // A finished output with UNSPECIFIED status is unexpected; treat
                    // as pending and keep polling rather than mis-decoding.
                    ResultState::Pending => {}
                }
            }
            // No output this window (and num_unfinished_inputs > 0) — keep polling.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn invocation_decode_rejects_non_cbor() {
        let inv = Invocation {
            data: vec![1, 2, 3],
            data_format: DataFormat::Pickle,
        };
        let decoded: Result<i64> = inv.decode_cbor();
        assert!(decoded.is_err());
    }

    #[test]
    fn invocation_round_trips_cbor_payload() {
        // Mirror an echo handler returning the args tuple.
        let mut payload = HashMap::new();
        payload.insert("hi".to_string(), 1_i64);
        let encoded = codec::encode(&payload).expect("encode");
        let inv = Invocation {
            data: encoded,
            data_format: DataFormat::Cbor,
        };
        let decoded: HashMap<String, i64> = inv.decode_cbor().expect("decode");
        assert_eq!(decoded.get("hi"), Some(&1));
    }
}
