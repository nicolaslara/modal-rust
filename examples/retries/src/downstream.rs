//! The flaky downstream call, kept off the modal surface in `lib.rs`.
//!
//! `lib.rs` owns the input/output structs and the `#[modal_rust::function]`; this
//! module owns the actual fetch — including the transient-failure behavior the retry
//! policy exists to absorb. The work is small, CPU-only, and fully deterministic:
//! the SAME `(resource, attempt)` always produces the SAME result, so the offline
//! demo can show both the failing and the healed call without any randomness.

use crate::{Payload, SETTLES_AT};

/// Fetch `resource` from a FLAKY downstream on a given `attempt` (1-based).
///
/// Early attempts model a transient outage and return `Err`; from [`SETTLES_AT`] on
/// the downstream has recovered and the fetch succeeds, returning the [`Payload`]. The
/// caller does NOT retry in code — that is exactly what the `#[function(retries = 5)]`
/// policy does for you. This function only decides, per attempt, whether the downstream
/// is up yet.
///
/// # Examples
///
/// ```
/// use example_retries::downstream::try_fetch;
/// use example_retries::SETTLES_AT;
/// assert!(try_fetch("weights.bin", 1).is_err()); // before SETTLES_AT -> transient fail
/// let ok = try_fetch("weights.bin", SETTLES_AT).unwrap();
/// assert_eq!(ok.resource, "weights.bin");
/// assert_eq!(ok.attempt, SETTLES_AT); // the attempt that finally succeeded
/// ```
pub fn try_fetch(resource: &str, attempt: u32) -> anyhow::Result<Payload> {
    if attempt < SETTLES_AT {
        // A transient failure — exactly what the retry policy exists to absorb.
        anyhow::bail!(
            "transient downstream failure fetching {:?} (attempt {})",
            resource,
            attempt
        );
    }
    Ok(Payload {
        resource: resource.to_string(),
        attempt,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn early_attempts_fail_transiently() {
        for attempt in 1..SETTLES_AT {
            assert!(
                try_fetch("weights.bin", attempt).is_err(),
                "attempt {attempt} is before SETTLES_AT and must fail",
            );
        }
    }

    #[test]
    fn settles_at_and_later_succeed() {
        let ok = try_fetch("weights.bin", SETTLES_AT).expect("settles at SETTLES_AT");
        assert_eq!(ok.resource, "weights.bin");
        assert_eq!(ok.attempt, SETTLES_AT);
        // A later attempt also succeeds.
        assert!(try_fetch("weights.bin", SETTLES_AT + 4).is_ok());
    }

    #[test]
    fn fetch_is_deterministic() {
        // Same (resource, attempt) -> same outcome, every time.
        assert!(try_fetch("a", 1).is_err());
        assert!(try_fetch("a", 1).is_err());
        assert_eq!(
            try_fetch("a", SETTLES_AT).unwrap().attempt,
            try_fetch("a", SETTLES_AT).unwrap().attempt,
        );
    }
}
