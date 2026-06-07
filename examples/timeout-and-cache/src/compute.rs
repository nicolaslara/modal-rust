//! The real computation behind [`crate::spin`], kept OUT of `lib.rs` so the library
//! surface stays a clean modal-rust example (input/output types + the `#[function]`).
//!
//! [`checksum`] is a small, CPU-only, fully deterministic fold: it walks
//! `0..iterations`, mixing each index into a running accumulator with a Knuth
//! multiplicative hash constant. It is cheap (a tight integer loop, no allocation, no
//! I/O), order-dependent, and avalanche-mixing, so the result genuinely depends on
//! every iteration and cannot be optimized away. `wrapping_*` keeps it total over
//! `u64` for any iteration count.

/// The Knuth multiplicative-hash constant (`2^32 / phi`, the golden ratio). Mixing the
/// running accumulator by this on every step spreads each index across all output bits.
const MIX: u64 = 2_654_435_761;

/// Fold `0..iterations` into a deterministic 64-bit checksum.
///
/// Each step folds the current index into the accumulator and re-mixes by [`MIX`], so
/// the output depends on the whole sequence in order. `wrapping_*` arithmetic makes it
/// total (never panics on overflow) regardless of how large `iterations` is.
///
/// Properties the tests rely on: `checksum(0) == 0` (empty fold), it is a pure
/// function (same input -> same output), and a longer run produces a different value
/// (the work is observable, not a constant).
pub fn checksum(iterations: u64) -> u64 {
    let mut acc: u64 = 0;
    for i in 0..iterations {
        acc = acc.wrapping_add(i).wrapping_mul(MIX);
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::checksum;

    #[test]
    fn empty_fold_is_zero() {
        assert_eq!(checksum(0), 0);
    }

    #[test]
    fn is_deterministic() {
        assert_eq!(checksum(1_000), checksum(1_000));
    }

    #[test]
    fn longer_runs_change_the_result() {
        // The loop actually runs: more iterations -> a different accumulator.
        assert_ne!(checksum(10), checksum(11));
    }
}
