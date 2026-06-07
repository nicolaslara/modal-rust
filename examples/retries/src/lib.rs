//! `examples/retries` — make a flaky function self-heal with an automatic retry policy.
//!
//! Teaching ONE concept: one operational knob the decorator sets directly,
//! `#[modal_rust::function(retries = 5)]`.
//!
//! Some work is FLAKY: a call to a downstream API times out, a spot instance is
//! reclaimed, a network blip drops a connection. The fix is not to scatter retry
//! loops through your code — it is to tell Modal "if this call fails, just run it
//! again". `retries = 5` does exactly that: the facade builds Modal's fixed-interval
//! retry policy (`backoff_coefficient = 1.0`, `initial_delay = 1s`, `max_delay = 60s`,
//! 5 retries — mirroring Modal's bare-int `retries` semantics) and rides it into the
//! `FunctionCreate` manifest's `retry_policy`. When the function returns `Err(_)`,
//! Modal automatically re-runs the WHOLE call up to 5 more times before giving up.
//!
//! The decorator IS the config. The body is a plain Rust fn that just does its work
//! and returns `Err` when the flaky step fails — it contains NO retry loop, NO sleep,
//! NO Modal. Retrying is operational metadata the facade reads when CREATING the Modal
//! function; it does not change what the function COMPUTES, only how many times Modal
//! will run it. `retries` defaults to unset (no policy), so a bare `#[function]` is
//! wire-identical to before.
//!
//! `src/bin/modal_runner.rs` is the one-line runner; `tests/manifest.rs` proves
//! OFFLINE (no live Modal) that the knob rides into the planned `FunctionCreate`
//! manifest — `retry_policy.retries == 5`.

use modal_rust::function;
use serde::{Deserialize, Serialize};

/// Input for [`fetch`] — the resource to fetch and which attempt this is. `attempt`
/// stands in for the real world: the flaky downstream fails on the first tries and
/// settles by attempt 3. On Modal you would NOT pass `attempt` yourself — the retry
/// policy supplies successive attempts automatically; it is a field here only so the
/// offline demo can show both the failing and the healed call deterministically.
#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    /// The resource id being fetched (echoed back on success).
    pub resource: String,
    /// Which attempt this is (1-based). Attempts before [`SETTLES_AT`] fail.
    pub attempt: u32,
}

/// The payload a successful fetch returns.
#[derive(Debug, Serialize, Deserialize)]
pub struct Payload {
    /// The resource that was fetched.
    pub resource: String,
    /// The attempt that finally succeeded.
    pub attempt: u32,
}

/// The attempt at which the flaky downstream stops failing — earlier attempts return
/// `Err`, this attempt and later succeed. A retry policy of `retries >= SETTLES_AT - 1`
/// therefore heals the call.
pub const SETTLES_AT: u32 = 3;

/// Fetch `resource` from a FLAKY downstream. Early attempts fail with a transient
/// error; from attempt [`SETTLES_AT`] on it succeeds. The body is plain Rust — it just
/// returns `Err` on a transient failure. The `#[function(retries = 5)]` decorator
/// makes Modal re-run the whole call automatically until it succeeds (or 5 retries are
/// exhausted), so a flaky operation SELF-HEALS without any retry loop in your code.
///
/// Run `modal_runner --describe` to see `"retries":5` ride on this entrypoint's config.
#[function(retries = 5)]
pub fn fetch(req: Request) -> anyhow::Result<Payload> {
    if req.attempt < SETTLES_AT {
        // A transient failure — exactly what the retry policy exists to absorb.
        anyhow::bail!(
            "transient downstream failure fetching {:?} (attempt {})",
            req.resource,
            req.attempt
        );
    }
    Ok(Payload {
        resource: req.resource,
        attempt: req.attempt,
    })
}
