//! Shared transport plumbing for the typed control-plane ops.
//!
//! Every unary control-plane RPC in [`crate::ops`] follows the SAME shape — clone
//! the (cheap, `Arc`-backed) stub per attempt, clone the owned tonic request, drive
//! it through [`crate::retry::retry_unary`], and unwrap `.into_inner()`. And the
//! invoke path repeats two higher-level shapes on top of that: the fix-#3
//! `FunctionMap` → `FunctionPutInputs` enqueue fallback, and the
//! `FunctionGetOutputs` long-poll window (deadline + `last_entry_id` cursor +
//! per-window transient retry). This module extracts each into ONE helper so the
//! call sites stop copy-pasting the boilerplate.
//!
//! These helpers change NO wire bytes: [`retry_rpc`](ModalClient::retry_rpc) issues
//! the identical request through the identical [`retry_unary`], and
//! [`enqueue_pipelined`](ModalClient::enqueue_pipelined) /
//! [`poll_one_output`](ModalClient::poll_one_output) build the SAME requests via the
//! SAME `build_*_request` builders the inlined code did.

use std::future::Future;

use crate::client::{ModalClient, ModalClientStub};
use crate::error::{Error, Result};
use crate::ops::invoke::{
    build_function_get_outputs_request, build_function_map_request,
    build_function_put_inputs_request,
};
use crate::proto::api::{
    FunctionCallInvocationType, FunctionCallType, FunctionGetOutputsResponse, FunctionPutInputsItem,
};
use crate::retry::retry_unary;

impl ModalClient {
    /// Issue ONE unary control-plane RPC with the standard transient-retry wrapping.
    ///
    /// Collapses the boilerplate every op repeated by hand: clone the stub per
    /// attempt (the tonic stub is `Arc`-backed, so cheap), clone the owned `req`
    /// (tonic requests are consumed by value), call `rpc(stub, req)`, and unwrap the
    /// response with `.into_inner()`. `name` is the per-retry log label.
    ///
    /// `rpc` is the one-line stub method, e.g.
    /// `|mut stub, req| async move { stub.app_create(req).await }`.
    pub(crate) async fn retry_rpc<Req, Resp, F, Fut>(
        &self,
        name: &str,
        req: Req,
        rpc: F,
    ) -> Result<Resp>
    where
        Req: Clone,
        F: Fn(ModalClientStub, Req) -> Fut,
        Fut: Future<Output = std::result::Result<tonic::Response<Resp>, tonic::Status>>,
    {
        let stub = self.stub();
        retry_unary(name, || {
            let stub = stub.clone();
            let req = req.clone();
            let fut = rpc(stub, req);
            async move { Ok(fut.await?.into_inner()) }
        })
        .await
    }

    /// Open a UNARY `FunctionMap` with `item` pipelined, then — fix #3 — fall back to
    /// `FunctionPutInputs` when the server did NOT echo the input back (it was not
    /// enqueued). Returns the call's `function_call_id`. Shared by `.remote()`
    /// ([`invoke_raw`](ModalClient::invoke_raw)) and `.spawn()`
    /// ([`spawn_raw`](ModalClient::spawn_raw)); `invocation_type` distinguishes them
    /// (`Sync` vs `Async`).
    ///
    /// CARE: both `FunctionMap` and the fallback `FunctionPutInputs` enqueue, so a
    /// retry could double-enqueue. For v0 the run path only invokes the idempotent
    /// `add` and reads exactly ONE output, so a duplicate enqueue is observably
    /// harmless — we retry on transient like Python (which relies on server-side
    /// input dedup). A non-idempotent user function would need a server idempotency
    /// token before enabling this generally.
    pub(crate) async fn enqueue_pipelined(
        &self,
        function_id: &str,
        invocation_type: FunctionCallInvocationType,
        item: FunctionPutInputsItem,
    ) -> Result<String> {
        // Step 1 — FunctionMap (UNARY) with the input pipelined.
        let map_req = build_function_map_request(
            function_id,
            FunctionCallType::Unary,
            invocation_type,
            vec![item.clone()],
        );
        let map = self
            .retry_rpc("function_map", map_req, |mut stub, req| async move {
                stub.function_map(req).await
            })
            .await?;

        let function_call_id = map.function_call_id;
        if function_call_id.is_empty() {
            return Err(Error::build(
                "FunctionMap returned an empty function_call_id".to_string(),
            ));
        }

        // Step 2 — fix #3: if the input was NOT pipelined (echoed back), enqueue it.
        // The item carries idx/function_call_id so the server can dedup within a call.
        if map.pipelined_inputs.is_empty() {
            let put_req =
                build_function_put_inputs_request(function_id, &function_call_id, vec![item]);
            let put = self
                .retry_rpc("function_put_inputs", put_req, |mut stub, req| async move {
                    stub.function_put_inputs(req).await
                })
                .await?;
            if put.inputs.is_empty() {
                return Err(Error::build(
                    "FunctionPutInputs accepted no inputs (input queue full?)".to_string(),
                ));
            }
        }

        Ok(function_call_id)
    }

    /// Long-poll ONE `FunctionGetOutputs` window for `function_call_id`, advancing
    /// the `last_entry_id` cursor in place and returning the window's response. A
    /// transient transport reset retries just this poll (via
    /// [`retry_rpc`](ModalClient::retry_rpc)) rather than failing the whole invoke.
    ///
    /// `max_values` bounds the batch (1 for a single `.remote()`/indexed get, N for a
    /// `map` collect); `index` filters to a specific output ordinal
    /// (`start_idx == end_idx == index`) or `None` for the next unfiltered output.
    /// The caller owns the deadline loop and the per-output decode; this isolates the
    /// shared cursor + window + retry mechanics common to
    /// [`poll_outputs_indexed`](ModalClient::poll_outputs_indexed) and
    /// [`map_cbor`](ModalClient::map_cbor).
    pub(crate) async fn poll_one_output(
        &self,
        function_call_id: &str,
        max_values: i32,
        last_entry_id: &mut String,
        index: Option<i32>,
    ) -> Result<FunctionGetOutputsResponse> {
        let req = build_function_get_outputs_request(
            function_call_id,
            max_values,
            last_entry_id.clone(),
            index,
        );
        let resp = self
            .retry_rpc("function_get_outputs", req, |mut stub, req| async move {
                stub.function_get_outputs(req).await
            })
            .await?;
        if !resp.last_entry_id.is_empty() {
            last_entry_id.clone_from(&resp.last_entry_id);
        }
        Ok(resp)
    }
}
