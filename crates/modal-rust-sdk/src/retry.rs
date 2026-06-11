//! Transient-retry helper for control-plane unary RPCs.
//!
//! Modal's own clients wrap EVERY unary RPC in a transient-retry layer
//! (`grpc_utils.py::retry_transient_errors`): a single mid-stream transport reset
//! (`h2 protocol error` / connection reset / `UNAVAILABLE`) on any one RPC should
//! retry just that RPC with exponential backoff, NOT discard all progress made so
//! far. Without this, the run-path `.remote()` sequence (~7 unary RPCs plus many
//! per-file upload RPCs) fails the whole round-trip on the first blip and the
//! outer retry restarts the entire upload+build.
//!
//! [`retry_transient`] retries ONLY when the error is transient
//! ([`Error::is_transient`]). Real errors — auth, invalid argument, in-band build
//! or function failure — propagate on the FIRST occurrence; they are never masked
//! and never retried into a timeout. The RPCs we wrap are idempotent for our use
//! (Modal dedups images by content hash and mount files by sha; precreate/create
//! reconcile by id; get/from_name are reads) so re-sending a request after a
//! dropped response is safe — mirroring the Python client's assumptions.

use std::future::Future;
use std::time::{Duration, Instant};

use crate::error::Result;

/// Tunables for [`retry_transient`].
///
/// [`RetryPolicy::default`] is the control-plane unary default: 8 attempts,
/// 100ms→5s exponential backoff with full jitter, 120s wall-clock deadline.
/// Values mirror Python's `Retry` defaults (`grpc_utils.py:299-309`,
/// `base_delay=0.1`, `delay_factor=2`) with a higher attempt count and 5s cap
/// (Python uses 5s for `connect_channel`) tuned for the run path's longer
/// per-RPC work and common build-window resets.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RetryPolicy {
    /// Initial backoff before the first retry.
    pub base_delay: Duration,
    /// Cap on the (pre-jitter) backoff between retries.
    pub max_delay: Duration,
    /// Multiplier applied to the backoff after each retry.
    pub delay_factor: f64,
    /// Total tries (1 initial + N-1 retries).
    pub max_attempts: u32,
    /// Hard wall-clock cap for the whole retry loop of a single RPC.
    pub total_deadline: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(5),
            delay_factor: 2.0,
            max_attempts: 8,
            total_deadline: Duration::from_secs(120),
        }
    }
}

/// Retry `op` while it fails with a TRANSIENT error ([`Error::is_transient`]).
///
/// Non-transient errors propagate on the first occurrence (auth, invalid
/// argument, in-band build/function failure) — they are NEVER retried or masked.
/// On exhaustion (attempt cap or deadline) the LAST error is returned unchanged
/// so the caller keeps the original `Status`/`Transport` for diagnostics.
///
/// `op` is an `FnMut` returning a fresh `Future` each call so the wrapped RPC can
/// rebuild its owned tonic request per attempt (tonic requests are consumed by
/// value). `name` is used only for the per-retry log line.
pub(crate) async fn retry_transient<T, F, Fut>(
    name: &str,
    policy: RetryPolicy,
    mut op: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let start = Instant::now();
    let mut delay = policy.base_delay;
    let mut attempt = 1u32;
    loop {
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                let last_attempt = attempt >= policy.max_attempts;
                let over_deadline = start.elapsed() + delay >= policy.total_deadline;
                if !e.is_transient() || last_attempt || over_deadline {
                    return Err(e);
                }
                // A SINGLE transient blip (long-poll connection resets are routine —
                // Modal's own client retries them silently) is not worth alarming
                // the user; log only when trouble REPEATS.
                if attempt >= 2 {
                    eprintln!(
                        "[retry] {name} attempt {attempt}/{} after transient: {e}",
                        policy.max_attempts
                    );
                }
                let jittered = jitter(delay);
                tokio::time::sleep(jittered).await;
                delay = delay.mul_f64(policy.delay_factor).min(policy.max_delay);
                attempt += 1;
            }
        }
    }
}

/// Convenience for the common case: [`retry_transient`] with the default
/// control-plane unary policy.
pub(crate) async fn retry_unary<T, F, Fut>(name: &str, op: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    retry_transient(name, RetryPolicy::default(), op).await
}

/// Full jitter: a uniformly random delay in `[0, delay]`. Spreads out a burst of
/// per-file upload retries so they do not all reconnect in lockstep (thundering
/// herd). Uses a tiny clock-seeded LCG so we pull no extra crate. Shared with the
/// blob-PUT retry in [`crate::ops::blob`].
pub(crate) fn jitter(delay: Duration) -> Duration {
    let nanos = delay.as_nanos();
    if nanos == 0 {
        return delay;
    }
    // Cap at u64 so the modulo math stays in range; control-plane delays are <=5s.
    let max = nanos.min(u64::MAX as u128) as u64;
    let r = next_rand_u64() % (max + 1);
    Duration::from_nanos(r)
}

/// A small, dependency-free PRNG: SplitMix64 seeded per call from the wall clock
/// nanos XOR a process-wide counter. Quality is irrelevant — this only de-syncs
/// retry timing, it is NOT used for anything security- or correctness-sensitive.
fn next_rand_u64() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        ^ COUNTER.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);

    // SplitMix64 finalizer.
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tonic::{Code, Status};

    fn transient_status() -> Error {
        Error::Status(Status::new(Code::Unavailable, "connection reset"))
    }

    #[tokio::test(start_paused = true)]
    async fn transient_then_ok_returns_ok() {
        let calls = AtomicUsize::new(0);
        let out: Result<u32> = retry_unary("test", || {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < 2 {
                    Err(transient_status())
                } else {
                    Ok(42)
                }
            }
        })
        .await;
        assert_eq!(out.unwrap(), 42);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "two transient + one success"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn non_transient_build_error_returns_immediately() {
        let calls = AtomicUsize::new(0);
        let out: Result<u32> = retry_unary("test", || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err(Error::build("real build failure")) }
        })
        .await;
        assert!(matches!(out, Err(Error::Build(_))));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "a non-transient error must NOT be retried"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn non_transient_invalid_argument_returns_immediately() {
        let calls = AtomicUsize::new(0);
        let out: Result<u32> = retry_unary("test", || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err(Error::Status(Status::new(Code::InvalidArgument, "bad arg"))) }
        })
        .await;
        assert!(matches!(out, Err(Error::Status(_))));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "InvalidArgument is terminal — surface on first occurrence"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn all_transient_exhausts_attempts_and_returns_last_error() {
        let calls = AtomicUsize::new(0);
        let out: Result<u32> = retry_unary("test", || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err::<u32, _>(transient_status()) }
        })
        .await;
        assert!(matches!(out, Err(Error::Status(_))));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            8,
            "default max_attempts is 8 total tries"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn deadline_cap_stops_early() {
        // A tiny deadline with a long base delay: the very first backoff already
        // exceeds the deadline, so we stop after the first failure.
        let policy = RetryPolicy {
            base_delay: Duration::from_secs(10),
            total_deadline: Duration::from_secs(1),
            ..RetryPolicy::default()
        };
        let calls = AtomicUsize::new(0);
        let out: Result<u32> = retry_transient("test", policy, || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err::<u32, _>(transient_status()) }
        })
        .await;
        assert!(out.is_err());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "deadline reached before the first backoff completes"
        );
    }
}
