//! Client-driven retries of a SYNC call, through the public invoke interface against
//! the mock control plane.
//!
//! The server delegates retries by answering `FunctionMap` with
//! `sync_client_retries_enabled` and the function's retry policy. A failed attempt
//! then arrives as an ordinary output, and the client asks for another attempt with
//! `FunctionRetryInputs`. Expected behavior is the official client's
//! (`_Invocation.run_function`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use modal_rust_sdk::{Error, ModalClient};
use modal_rust_testkit::prelude::generic_result::{DataOneof, GenericStatus};
use modal_rust_testkit::prelude::*;

const CALL_JWT: &str = "fc-jwt";
const INPUT_JWT: &str = "in-jwt";

/// A `FunctionMap` answer that accepts the pipelined input and reports a retry
/// policy with `retries`, delegating them to the client or not.
fn map_response(delegated: bool, retries: u32) -> FunctionMapResponse {
    FunctionMapResponse {
        function_call_id: "fc-1".to_string(),
        pipelined_inputs: vec![FunctionPutInputsResponseItem {
            idx: 0,
            input_id: "in-0".to_string(),
            input_jwt: INPUT_JWT.to_string(),
        }],
        retry_policy: Some(FunctionRetryPolicy {
            retries,
            // Below the client's one-second floor, so each policy retry waits 1 s.
            initial_delay_ms: 1,
            backoff_coefficient: 1.0,
            ..Default::default()
        }),
        function_call_jwt: CALL_JWT.to_string(),
        sync_client_retries_enabled: delegated,
        ..Default::default()
    }
}

/// One output with `status`; a success carries the runner envelope for `42`.
fn output(status: GenericStatus) -> FunctionGetOutputsResponse {
    let data_oneof = (status == GenericStatus::Success).then(|| {
        let envelope = serde_json::json!({ "ok": true, "value": 42 }).to_string();
        DataOneof::Data(modal_rust_sdk::codec::encode(&envelope).unwrap())
    });
    FunctionGetOutputsResponse {
        outputs: vec![FunctionGetOutputsItem {
            idx: 0,
            data_format: DataFormat::Cbor as i32,
            result: Some(GenericResult {
                status: status as i32,
                exception: "boom".to_string(),
                data_oneof,
                ..Default::default()
            }),
            ..Default::default()
        }],
        last_entry_id: "1-0".to_string(),
        ..Default::default()
    }
}

/// A mock whose call fails with each of `attempts` in turn (the last one repeats).
async fn mock_with(delegated: bool, retries: u32, attempts: &[GenericStatus]) -> MockModal {
    let attempts = attempts.to_vec();
    let polled = Arc::new(AtomicUsize::new(0));
    MockModal::builder()
        .on_function_map(move |_req| Ok(map_response(delegated, retries)))
        .on_function_get_outputs(move |_req| {
            let n = polled.fetch_add(1, Ordering::SeqCst);
            Ok(output(attempts[n.min(attempts.len() - 1)]))
        })
        .start()
        .await
        .expect("mock up")
}

async fn invoke(mock: &MockModal) -> Result<String, Error> {
    let mut client = ModalClient::from_config(mock.modal_config())
        .await
        .expect("connect");
    let no_kwargs: HashMap<String, ()> = HashMap::new();
    client
        .invoke_cbor(
            "fu-1",
            &("add", r#"{"a":40,"b":2}"#.to_string()),
            &no_kwargs,
        )
        .await
}

#[tokio::test]
async fn internal_failures_are_retried_at_once_without_using_the_policys_retries() {
    use GenericStatus::{InternalFailure, Success};
    // Three lost attempts against a policy that allows ONE retry.
    let mock = mock_with(
        true,
        1,
        &[InternalFailure, InternalFailure, InternalFailure, Success],
    )
    .await;

    let envelope = invoke(&mock).await.expect("the fourth attempt succeeds");
    assert!(envelope.contains("42"), "{envelope}");

    let retried = mock.requests::<FunctionRetryInputsRequest>();
    assert_eq!(retried.len(), 3, "one retry per lost attempt");
    let sent = mock.last::<FunctionMapRequest>().expect("map");
    for req in &retried {
        assert_eq!(req.function_call_jwt, CALL_JWT);
        assert_eq!(req.inputs.len(), 1);
        assert_eq!(req.inputs[0].input_jwt, INPUT_JWT);
        assert_eq!(
            req.inputs[0].input, sent.pipelined_inputs[0].input,
            "the retry resends the original input"
        );
    }
    // Every poll names the input the client expects the server to be running.
    for poll in mock.requests::<FunctionGetOutputsRequest>() {
        assert_eq!(poll.input_jwts, vec![INPUT_JWT.to_string()]);
        assert_eq!((poll.start_idx, poll.end_idx), (Some(0), Some(0)));
    }
}

#[tokio::test]
async fn a_failed_attempt_is_retried_under_the_policy_and_can_succeed() {
    use GenericStatus::{Failure, Success};
    let mock = mock_with(true, 1, &[Failure, Success]).await;

    let started = std::time::Instant::now();
    let envelope = invoke(&mock).await.expect("the retry succeeds");
    assert!(envelope.contains("42"), "{envelope}");
    assert_eq!(mock.took::<FunctionRetryInputsRequest>(), 1);
    assert!(
        started.elapsed() >= std::time::Duration::from_secs(1),
        "a policy retry waits out its delay first"
    );
}

#[tokio::test]
async fn the_failure_is_returned_once_the_policys_retries_are_used_up() {
    let mock = mock_with(true, 1, &[GenericStatus::Failure]).await;

    let err = invoke(&mock).await.expect_err("every attempt fails");
    let msg = err.to_string();
    assert!(msg.contains("GENERIC_STATUS_FAILURE"), "{msg}");
    assert!(msg.contains("boom"), "{msg}");
    assert_eq!(
        mock.took::<FunctionRetryInputsRequest>(),
        1,
        "a policy of one retry asks for exactly one more attempt"
    );
}

#[tokio::test]
async fn a_terminated_call_is_not_retried() {
    let mock = mock_with(true, 3, &[GenericStatus::Terminated]).await;

    let err = invoke(&mock).await.expect_err("terminated");
    assert!(
        err.to_string().contains("GENERIC_STATUS_TERMINATED"),
        "{err}"
    );
    assert_eq!(mock.took::<FunctionRetryInputsRequest>(), 0);
}

#[tokio::test]
async fn the_client_does_not_retry_a_call_the_server_retries_itself() {
    // Same policy, but the server keeps the retries.
    let mock = mock_with(false, 3, &[GenericStatus::InternalFailure]).await;

    let err = invoke(&mock).await.expect_err("the failure is the result");
    assert!(
        err.to_string().contains("GENERIC_STATUS_INTERNAL_FAILURE"),
        "{err}"
    );
    assert_eq!(mock.took::<FunctionRetryInputsRequest>(), 0);
    for poll in mock.requests::<FunctionGetOutputsRequest>() {
        assert!(poll.input_jwts.is_empty());
        assert_eq!((poll.start_idx, poll.end_idx), (None, None));
    }
}
