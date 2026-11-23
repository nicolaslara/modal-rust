//! Client-driven retries of a SYNC call's input.
//!
//! For a SYNC call the server can delegate retries to the client: `FunctionMap`
//! answers with `sync_client_retries_enabled`, the function's retry policy and the
//! JWTs that name the call and its input. A failed attempt then comes back as an
//! ordinary output and stays failed until the client asks for another attempt with
//! `FunctionRetryInputs`. This mirrors the official client's
//! `_Invocation.run_function` (_functions.py) and `RetryManager` (retries.py).

use std::time::Duration;

use crate::proto::api::{
    FunctionInput, FunctionRetryInputsItem, FunctionRetryInputsRequest, FunctionRetryPolicy,
};

/// Shortest and longest wait before retrying an input (`MIN_INPUT_RETRY_DELAY_MS`,
/// `MAX_INPUT_RETRY_DELAY_MS`, retries.py).
const MIN_RETRY_DELAY_MS: f64 = 1_000.0;
const MAX_RETRY_DELAY_MS: f64 = 24.0 * 60.0 * 60.0 * 1_000.0;

/// What the client needs to retry one input of a SYNC call it was told to retry
/// itself.
#[derive(Debug, Clone)]
pub(crate) struct ClientRetries {
    /// The function's retry policy, as the server reported it.
    pub policy: FunctionRetryPolicy,
    /// Names the function call in `FunctionRetryInputs`.
    pub function_call_jwt: String,
    /// Names the input, both in `FunctionRetryInputs` and in the output poll's
    /// `input_jwts` (the inputs the client expects the server to be running).
    pub input_jwt: String,
    /// The input to run again.
    pub input: FunctionInput,
}

impl ClientRetries {
    /// The retry context for a call, or `None` when the server keeps retries to
    /// itself. The official client retries only a call that has a policy with at
    /// least one retry AND was opened with `sync_client_retries_enabled`.
    pub(crate) fn for_call(
        sync_client_retries_enabled: bool,
        policy: Option<FunctionRetryPolicy>,
        function_call_jwt: String,
        input_jwt: String,
        input: Option<FunctionInput>,
    ) -> Option<Self> {
        let policy = policy.filter(|p| p.retries > 0)?;
        if !sync_client_retries_enabled {
            return None;
        }
        Some(Self {
            policy,
            function_call_jwt,
            input_jwt,
            input: input?,
        })
    }
}

/// Build the `FunctionRetryInputs` request for one input — pure, no I/O.
pub(crate) fn build_function_retry_inputs_request(
    retries: &ClientRetries,
) -> FunctionRetryInputsRequest {
    FunctionRetryInputsRequest {
        function_call_jwt: retries.function_call_jwt.clone(),
        inputs: vec![FunctionRetryInputsItem {
            input_jwt: retries.input_jwt.clone(),
            input: Some(retries.input.clone()),
            ..Default::default()
        }],
    }
}

/// Hands out the wait before each retry the policy still allows.
#[derive(Debug)]
pub(crate) struct RetryDelays {
    policy: FunctionRetryPolicy,
    retry_count: u32,
}

impl RetryDelays {
    pub(crate) fn new(policy: FunctionRetryPolicy) -> Self {
        Self {
            policy,
            retry_count: 0,
        }
    }

    /// The wait before the next retry, or `None` once the policy's retries are
    /// used up. The n-th wait is `initial_delay * backoff^(n-1)`, kept between one
    /// second and one day.
    pub(crate) fn next_delay(&mut self) -> Option<Duration> {
        self.retry_count += 1;
        if self.retry_count > self.policy.retries {
            return None;
        }
        let exponent = (self.retry_count - 1) as i32;
        let delay_ms = f64::from(self.policy.initial_delay_ms)
            * f64::from(self.policy.backoff_coefficient).powi(exponent);
        let delay_ms = delay_ms.clamp(MIN_RETRY_DELAY_MS, MAX_RETRY_DELAY_MS);
        Some(Duration::from_millis(delay_ms as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(
        retries: u32,
        initial_delay_ms: u32,
        backoff_coefficient: f32,
    ) -> FunctionRetryPolicy {
        FunctionRetryPolicy {
            retries,
            initial_delay_ms,
            backoff_coefficient,
            ..Default::default()
        }
    }

    #[test]
    fn delays_follow_the_backoff_and_stop_after_the_policys_retries() {
        let mut delays = RetryDelays::new(policy(3, 2_000, 2.0));
        assert_eq!(delays.next_delay(), Some(Duration::from_secs(2)));
        assert_eq!(delays.next_delay(), Some(Duration::from_secs(4)));
        assert_eq!(delays.next_delay(), Some(Duration::from_secs(8)));
        assert_eq!(delays.next_delay(), None);
        assert_eq!(delays.next_delay(), None);
    }

    #[test]
    fn delays_stay_between_one_second_and_one_day() {
        let mut short = RetryDelays::new(policy(1, 10, 1.0));
        assert_eq!(short.next_delay(), Some(Duration::from_secs(1)));
        let mut long = RetryDelays::new(policy(40, 60_000, 10.0));
        let last = std::iter::from_fn(|| long.next_delay()).last();
        assert_eq!(last, Some(Duration::from_secs(24 * 60 * 60)));
    }

    #[test]
    fn the_client_retries_only_when_the_server_delegates_and_a_retry_is_allowed() {
        let input = Some(FunctionInput::default());
        let ctx = |enabled, p: Option<FunctionRetryPolicy>| {
            ClientRetries::for_call(enabled, p, "fc-jwt".into(), "in-jwt".into(), input.clone())
        };
        assert!(ctx(true, Some(policy(2, 1_000, 1.0))).is_some());
        assert!(ctx(false, Some(policy(2, 1_000, 1.0))).is_none());
        assert!(ctx(true, Some(policy(0, 1_000, 1.0))).is_none());
        assert!(ctx(true, None).is_none());
    }

    #[test]
    fn the_retry_request_names_the_call_and_resends_the_input() {
        let retries = ClientRetries {
            policy: policy(1, 1_000, 1.0),
            function_call_jwt: "fc-jwt".into(),
            input_jwt: "in-jwt".into(),
            input: FunctionInput {
                final_input: true,
                ..Default::default()
            },
        };
        let req = build_function_retry_inputs_request(&retries);
        assert_eq!(req.function_call_jwt, "fc-jwt");
        assert_eq!(req.inputs.len(), 1);
        assert_eq!(req.inputs[0].input_jwt, "in-jwt");
        assert_eq!(req.inputs[0].input, Some(retries.input.clone()));
    }
}
