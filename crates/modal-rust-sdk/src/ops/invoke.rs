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
use crate::retry::retry_unary;

/// Per-poll timeout (seconds) for `FunctionGetOutputs` long-poll reconnects.
const OUTPUTS_TIMEOUT_SECS: f32 = 55.0;
/// Initial `last_entry_id` cursor for `FunctionGetOutputs` — the "from the
/// beginning" sentinel. Modal's server REQUIRES a non-empty `last_entry_id` on a
/// per-index get (`start_idx`/`end_idx` set), rejecting an empty string with
/// `INVALID_ARGUMENT: "No last_entry_id provided."`. The Python client always
/// seeds `"0-0"` (`_functions.py` `pop_function_call_outputs`, `parallel_map.py`
/// `get_all_outputs`); we match it for every poll so the indexed `get` path works
/// and the unfiltered `.remote()`/`map` polls stay byte-compatible with Python.
const LAST_ENTRY_ID_INITIAL: &str = "0-0";
/// Default safety cap on total wall-clock time spent waiting for a function
/// output. Used by [`ModalClient::invoke_cbor`]/[`invoke_raw`] (ordinary calls).
/// The RUN path overrides this with [`invoke_cbor_with_deadline`] because its
/// first invocation triggers a cold in-body `cargo build` that can run for many
/// minutes — the client must poll at least as long as the function's own
/// container timeout, not give up at the default.
const DEFAULT_INVOKE_DEADLINE: Duration = Duration::from_secs(600);

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
        self.invoke_cbor_with_deadline(function_id, args, kwargs, DEFAULT_INVOKE_DEADLINE)
            .await
    }

    /// Like [`invoke_cbor`](Self::invoke_cbor) but with an explicit wall-clock
    /// `deadline` for the output poll. The RUN path passes its function's
    /// container timeout here so the client keeps polling while the cold in-body
    /// `cargo build` runs (the default 600s is far too short for a first build).
    pub async fn invoke_cbor_with_deadline<A, K, R>(
        &mut self,
        function_id: &str,
        args: &A,
        kwargs: &K,
        deadline: Duration,
    ) -> Result<R>
    where
        A: Serialize,
        K: Serialize,
        R: DeserializeOwned,
    {
        let payload = (args, kwargs);
        let encoded = codec::encode(&payload)?;
        let invocation = self
            .invoke_raw_with_deadline(function_id, encoded, deadline)
            .await?;
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
        self.invoke_raw_with_deadline(function_id, args_serialized, DEFAULT_INVOKE_DEADLINE)
            .await
    }

    /// Like [`invoke_raw`](Self::invoke_raw) but with an explicit output-poll
    /// `deadline` (see [`invoke_cbor_with_deadline`](Self::invoke_cbor_with_deadline)).
    pub async fn invoke_raw_with_deadline(
        &mut self,
        function_id: &str,
        args_serialized: Vec<u8>,
        deadline: Duration,
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
        //
        // CARE: FunctionMap enqueues an input, so a retry could double-enqueue. For
        // v0 the run path only invokes the pure `add` (idempotent) and poll_outputs
        // reads exactly ONE output, so a duplicate enqueue is observably harmless —
        // we retry on transient like Python (which relies on server-side input
        // dedup). A non-idempotent user function would need a server idempotency
        // token before enabling this generally.
        let map_req = FunctionMapRequest {
            function_id: function_id.to_string(),
            function_call_type: FunctionCallType::Unary as i32,
            function_call_invocation_type: FunctionCallInvocationType::Sync as i32,
            pipelined_inputs: vec![item.clone()],
            ..Default::default()
        };
        let stub = self.stub();
        let map = retry_unary("function_map", || {
            let mut stub = stub.clone();
            let req = map_req.clone();
            async move { Ok(stub.function_map(req).await?.into_inner()) }
        })
        .await?;

        let function_call_id = map.function_call_id;
        if function_call_id.is_empty() {
            return Err(Error::build(
                "FunctionMap returned an empty function_call_id".to_string(),
            ));
        }

        // Step 2 — fix #3: if the input was NOT pipelined (echoed back), enqueue it.
        if map.pipelined_inputs.is_empty() {
            // Same double-enqueue caveat as FunctionMap; same v0 stance (retry —
            // harmless for the pure `add`). The item carries idx/function_call_id
            // so the server can dedup within a call.
            let put_req = FunctionPutInputsRequest {
                function_id: function_id.to_string(),
                function_call_id: function_call_id.clone(),
                inputs: vec![item],
            };
            let stub = self.stub();
            let put = retry_unary("function_put_inputs", || {
                let mut stub = stub.clone();
                let req = put_req.clone();
                async move { Ok(stub.function_put_inputs(req).await?.into_inner()) }
            })
            .await?;
            if put.inputs.is_empty() {
                return Err(Error::build(
                    "FunctionPutInputs accepted no inputs (input queue full?)".to_string(),
                ));
            }
        }

        // Step 3 — poll FunctionGetOutputs until the result arrives.
        self.poll_outputs_indexed(&function_call_id, None, deadline)
            .await
    }

    /// Long-poll `FunctionGetOutputs` (api.proto:4247), advancing `last_entry_id`,
    /// until an output for the call arrives; decode its terminal `GenericResult`.
    /// Gives up after `deadline` wall-clock with a clear "no output" build error.
    ///
    /// `index` selects WHICH output:
    /// - `None` — read the next available output, unfiltered (the single-input
    ///   `.remote()` case: one input at `idx 0`, exactly as before this refactor).
    /// - `Some(i)` — request ONLY index `i` (`start_idx == end_idx == i`, per the
    ///   Python per-index pop) and assert the returned `item.idx == i` before
    ///   decoding (the `FunctionCall::get`-by-call case).
    async fn poll_outputs_indexed(
        &mut self,
        function_call_id: &str,
        index: Option<i32>,
        deadline: Duration,
    ) -> Result<Invocation> {
        let started = std::time::Instant::now();
        let mut last_entry_id = LAST_ENTRY_ID_INITIAL.to_string();

        loop {
            if started.elapsed() > deadline {
                return Err(Error::build(format!(
                    "function call {function_call_id} produced no output within {}s",
                    deadline.as_secs()
                )));
            }

            // Pure read with a last_entry_id cursor — a transient reset retries the
            // single poll window rather than failing the whole invoke (analogous to
            // the image-build poll reconnect).
            let req = FunctionGetOutputsRequest {
                function_call_id: function_call_id.to_string(),
                max_values: 1,
                timeout: OUTPUTS_TIMEOUT_SECS,
                last_entry_id: last_entry_id.clone(),
                clear_on_success: true,
                // Per-index get (Python `pop_function_call_outputs` sets
                // start_idx=end_idx=index). `None` => unfiltered next output,
                // preserving the byte-for-byte single-input `.remote()` behavior.
                start_idx: index,
                end_idx: index,
                ..Default::default()
            };
            let stub = self.stub();
            let resp = retry_unary("function_get_outputs", || {
                let mut stub = stub.clone();
                let req = req.clone();
                async move { Ok(stub.function_get_outputs(req).await?.into_inner()) }
            })
            .await?;

            if !resp.last_entry_id.is_empty() {
                last_entry_id = resp.last_entry_id;
            }

            if let Some(item) = resp.outputs.into_iter().next() {
                // When a specific index was requested, defensively ignore any output
                // for a different index (the server should already filter via
                // start_idx/end_idx, but keep the cursor advancing and re-poll).
                if let Some(want) = index {
                    if item.idx != want {
                        continue;
                    }
                }
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

    /// Enqueue ONE input (fire-and-forget) and return its `function_call_id`
    /// IMMEDIATELY, WITHOUT polling for the output. CBOR-encodes `(args, kwargs)`,
    /// then defers to [`spawn_raw`](Self::spawn_raw).
    ///
    /// Mirrors Python `Function.spawn` (_functions.py:1860) →
    /// `_Invocation.create(..., ASYNC)` (_functions.py:134): a UNARY `FunctionMap`
    /// with `function_call_invocation_type = ASYNC` and the input pipelined, then
    /// `_FunctionCall._new_hydrated(invocation.function_call_id, ...)` — no wait.
    pub async fn spawn_cbor<A, K>(
        &mut self,
        function_id: &str,
        args: &A,
        kwargs: &K,
    ) -> Result<String>
    where
        A: Serialize,
        K: Serialize,
    {
        let encoded = codec::encode(&(args, kwargs))?;
        self.spawn_raw(function_id, encoded).await
    }

    /// Low-level fire-and-forget enqueue: send already-CBOR-encoded
    /// `(args, kwargs)` bytes as ONE input and return the `function_call_id` without
    /// polling. Reuses the `.remote()` step-1 (`FunctionMap`) + step-2 (fix-#3
    /// `FunctionPutInputs` fallback) enqueue, but DROPS the output poll.
    ///
    /// Unlike `.remote()` it sends `FunctionCallInvocationType::Async` (Python
    /// `spawn` does the same, _functions.py:1878) — the client will not hold a sync
    /// poll open; the result is fetched later via
    /// [`get_by_call_cbor`](Self::get_by_call_cbor).
    pub async fn spawn_raw(
        &mut self,
        function_id: &str,
        args_serialized: Vec<u8>,
    ) -> Result<String> {
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

        // Step 1 — FunctionMap (UNARY + ASYNC) with the input pipelined. Same
        // double-enqueue caveat as `.remote()` (idempotent `add`; `get` reads
        // idx 0).
        let map_req = FunctionMapRequest {
            function_id: function_id.to_string(),
            function_call_type: FunctionCallType::Unary as i32,
            function_call_invocation_type: FunctionCallInvocationType::Async as i32,
            pipelined_inputs: vec![item.clone()],
            ..Default::default()
        };
        let stub = self.stub();
        let map = retry_unary("function_map", || {
            let mut stub = stub.clone();
            let req = map_req.clone();
            async move { Ok(stub.function_map(req).await?.into_inner()) }
        })
        .await?;

        let function_call_id = map.function_call_id;
        if function_call_id.is_empty() {
            return Err(Error::build(
                "FunctionMap returned an empty function_call_id".to_string(),
            ));
        }

        // Step 2 — fix #3: if the input was NOT pipelined back, enqueue it.
        if map.pipelined_inputs.is_empty() {
            let put_req = FunctionPutInputsRequest {
                function_id: function_id.to_string(),
                function_call_id: function_call_id.clone(),
                inputs: vec![item],
            };
            let stub = self.stub();
            let put = retry_unary("function_put_inputs", || {
                let mut stub = stub.clone();
                let req = put_req.clone();
                async move { Ok(stub.function_put_inputs(req).await?.into_inner()) }
            })
            .await?;
            if put.inputs.is_empty() {
                return Err(Error::build(
                    "FunctionPutInputs accepted no inputs (input queue full?)".to_string(),
                ));
            }
        }

        // Step 3 — return the call id. Do NOT poll (fire-and-forget).
        Ok(function_call_id)
    }

    /// Poll `FunctionGetOutputs` for an already-known `function_call_id`, returning
    /// the output at `index` decoded into `R` (CBOR). Mirrors Python
    /// `FunctionCall.get` (_functions.py:1959) → `poll_function(timeout, index)`.
    pub async fn get_by_call_cbor<R: DeserializeOwned>(
        &mut self,
        function_call_id: &str,
        index: i32,
        deadline: Duration,
    ) -> Result<R> {
        self.get_by_call_raw(function_call_id, index, deadline)
            .await?
            .decode_cbor()
    }

    /// Like [`get_by_call_cbor`](Self::get_by_call_cbor) but returns the raw
    /// [`Invocation`] (bytes + format) without decoding. Delegates to
    /// [`poll_outputs_indexed`](Self::poll_outputs_indexed) with `Some(index)` so
    /// the request carries `start_idx == end_idx == index` (the per-index get).
    pub async fn get_by_call_raw(
        &mut self,
        function_call_id: &str,
        index: i32,
        deadline: Duration,
    ) -> Result<Invocation> {
        self.poll_outputs_indexed(function_call_id, Some(index), deadline)
            .await
    }

    /// Fan-out N inputs under ONE map call and return their decoded envelopes in
    /// INPUT ORDER. Each `inputs[i]` is the `(args, kwargs)` for input ordinal `i`;
    /// the ordinal becomes the `FunctionPutInputsItem.idx`, which is the ordering
    /// key the server tags each output with.
    ///
    /// Mirrors Python `_map_invocation` (parallel_map.py:361): a `FunctionMap`
    /// (`function_call_type = MAP`) opens the call EMPTY, then `FunctionPutInputs`
    /// enqueues the inputs each carrying its `idx`, then `FunctionGetOutputs` is
    /// polled and outputs are reordered by `item.idx`. This is the synchronous
    /// fail-fast collect form (`return_exceptions = false`): the first remote
    /// failure surfaces immediately.
    pub async fn map_cbor<A, K, R>(
        &mut self,
        function_id: &str,
        inputs: &[(A, K)],
        deadline: Duration,
    ) -> Result<Vec<R>>
    where
        A: Serialize,
        K: Serialize,
        R: DeserializeOwned,
    {
        let n = inputs.len();
        if n == 0 {
            return Ok(Vec::new());
        }

        // Step 1 — open the MAP call EMPTY (Python opens with no pipelined inputs,
        // parallel_map.py:371-378), SYNC collect.
        let map_req = FunctionMapRequest {
            function_id: function_id.to_string(),
            function_call_type: FunctionCallType::Map as i32,
            function_call_invocation_type: FunctionCallInvocationType::Sync as i32,
            pipelined_inputs: vec![],
            ..Default::default()
        };
        let stub = self.stub();
        let map = retry_unary("function_map", || {
            let mut stub = stub.clone();
            let req = map_req.clone();
            async move { Ok(stub.function_map(req).await?.into_inner()) }
        })
        .await?;
        let function_call_id = map.function_call_id;
        if function_call_id.is_empty() {
            return Err(Error::build(
                "FunctionMap (MAP) returned an empty function_call_id".to_string(),
            ));
        }

        // Step 2 — build N items with sequential idx (the input ordinal IS the
        // ordering key). Step 3 — enqueue all N under the open call.
        let mut items = Vec::with_capacity(n);
        for (i, (args, kwargs)) in inputs.iter().enumerate() {
            let encoded = codec::encode(&(args, kwargs))?;
            items.push(FunctionPutInputsItem {
                idx: i as i32,
                input: Some(FunctionInput {
                    data_format: DataFormat::Cbor as i32,
                    final_input: false,
                    method_name: None,
                    args_oneof: Some(ArgsOneof::Args(encoded)),
                }),
                ..Default::default()
            });
        }
        let put_req = FunctionPutInputsRequest {
            function_id: function_id.to_string(),
            function_call_id: function_call_id.clone(),
            inputs: items,
        };
        let stub = self.stub();
        let put = retry_unary("function_put_inputs", || {
            let mut stub = stub.clone();
            let req = put_req.clone();
            async move { Ok(stub.function_put_inputs(req).await?.into_inner()) }
        })
        .await?;
        if put.inputs.len() < n {
            return Err(Error::build(format!(
                "FunctionPutInputs accepted {} of {n} inputs (input queue full?)",
                put.inputs.len()
            )));
        }

        // Step 4 — collect N outputs, reorder by idx (BTreeMap keyed on the input
        // ordinal). Same long-poll loop / cursor / retry as `poll_outputs_indexed`,
        // but unfiltered and batched (max_values = N), fail-fast on any failure.
        let mut got: std::collections::BTreeMap<i32, R> = std::collections::BTreeMap::new();
        let started = std::time::Instant::now();
        let mut last_entry_id = LAST_ENTRY_ID_INITIAL.to_string();
        while got.len() < n {
            if started.elapsed() > deadline {
                return Err(Error::build(format!(
                    "map call {function_call_id} produced {} of {n} outputs within {}s",
                    got.len(),
                    deadline.as_secs()
                )));
            }
            let req = FunctionGetOutputsRequest {
                function_call_id: function_call_id.clone(),
                max_values: n as i32,
                timeout: OUTPUTS_TIMEOUT_SECS,
                last_entry_id: last_entry_id.clone(),
                clear_on_success: true,
                ..Default::default()
            };
            let stub = self.stub();
            let resp = retry_unary("function_get_outputs", || {
                let mut stub = stub.clone();
                let req = req.clone();
                async move { Ok(stub.function_get_outputs(req).await?.into_inner()) }
            })
            .await?;

            if !resp.last_entry_id.is_empty() {
                last_entry_id = resp.last_entry_id;
            }

            for item in resp.outputs {
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
                        let inv = Invocation { data, data_format };
                        // Reorder by the input ordinal. Duplicate idxs (e.g. a
                        // retried enqueue) are idempotent on the BTreeMap key.
                        got.insert(item.idx, inv.decode_cbor()?);
                    }
                    ResultState::Failure(status) => {
                        return Err(Error::build(describe_failure(
                            "map function call",
                            status,
                            &result,
                        )));
                    }
                    // Finished output with UNSPECIFIED status is unexpected; skip and
                    // keep polling rather than mis-decoding.
                    ResultState::Pending => {}
                }
            }
        }

        // Step 5 — reassemble in input order (idx 0..N).
        reassemble_in_order(got, n)
    }
}

/// Reassemble a by-`idx` output map into a `Vec` in INPUT ORDER (idx `0..n`).
///
/// The ordering guarantee of [`ModalClient::map_cbor`] lives here: outputs arrive
/// tagged with their input ordinal (`FunctionGetOutputsItem.idx`) in COMPLETION
/// order, are accumulated into a `BTreeMap<idx, R>`, then popped `0,1,..,n-1`. This
/// is the Rust-`Vec` form of Python's reorder buffer (parallel_map.py:556-577).
///
/// Returns an error (rather than panicking) if any index in `0..n` is missing — a
/// defensive guard; `map_cbor` only calls this once `got.len() == n`.
fn reassemble_in_order<R>(mut got: std::collections::BTreeMap<i32, R>, n: usize) -> Result<Vec<R>> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        match got.remove(&(i as i32)) {
            Some(v) => out.push(v),
            None => {
                return Err(Error::build(format!(
                    "map output reassembly is missing index {i} of {n}"
                )))
            }
        }
    }
    Ok(out)
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

    #[test]
    fn reassemble_orders_by_idx_regardless_of_completion_order() {
        // Outputs arrive in COMPLETION order (here: 2, 0, 3, 1) tagged with their
        // input ordinal; reassembly must yield INPUT order 0,1,2,3. This is the
        // core `map` ordering invariant.
        let mut got = std::collections::BTreeMap::new();
        got.insert(2_i32, "c");
        got.insert(0_i32, "a");
        got.insert(3_i32, "d");
        got.insert(1_i32, "b");
        let ordered = reassemble_in_order(got, 4).expect("complete");
        assert_eq!(ordered, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn reassemble_single_input() {
        let mut got = std::collections::BTreeMap::new();
        got.insert(0_i32, 42_i64);
        assert_eq!(reassemble_in_order(got, 1).expect("complete"), vec![42]);
    }

    #[test]
    fn reassemble_empty_is_empty() {
        let got: std::collections::BTreeMap<i32, i64> = std::collections::BTreeMap::new();
        assert!(reassemble_in_order(got, 0).expect("complete").is_empty());
    }

    #[test]
    fn reassemble_missing_index_errors_not_panics() {
        // idx 1 missing out of 3 — defensive error, never a panic.
        let mut got = std::collections::BTreeMap::new();
        got.insert(0_i32, "a");
        got.insert(2_i32, "c");
        let err = reassemble_in_order(got, 3).unwrap_err();
        assert!(format!("{err}").contains("missing index 1 of 3"));
    }
}
