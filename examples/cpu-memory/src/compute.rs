//! The actual CPU- and memory-bound work for [`crate::crunch`], kept out of the modal
//! surface in `src/lib.rs` so that file stays the clean input/output + `#[function]`.
//!
//! [`checksum`] folds an accumulating, order-dependent checksum over `0..records`. It is
//! real arithmetic — every record contributes — so the work cannot be optimized away and
//! the result is a deterministic function of `records` alone. `wrapping_*` keeps the fold
//! total over `u64` for any input, and the multiply by a large odd constant (Knuth's
//! multiplicative-hash constant, `2_654_435_761`) diffuses each step so the running value
//! depends on the whole sequence, not just the last term.

/// Fold a deterministic, order-dependent checksum over the records `0..records`.
///
/// Each step mixes the running value with the record index via a wrapping add followed by
/// a wrapping multiply by a large odd constant, so the output depends on every record and
/// is reproducible for a given `records`. `checksum(0) == 0`.
pub fn checksum(records: u64) -> u64 {
    let mut checksum: u64 = 0;
    for i in 0..records {
        checksum = checksum.wrapping_add(i).wrapping_mul(2_654_435_761);
    }
    checksum
}

#[cfg(test)]
mod tests {
    use super::checksum;

    #[test]
    fn empty_batch_is_zero() {
        assert_eq!(checksum(0), 0);
    }

    #[test]
    fn is_deterministic() {
        assert_eq!(checksum(1000), checksum(1000));
    }

    #[test]
    fn every_record_contributes() {
        // Growing the batch changes the running fold — the work is not elided.
        assert_ne!(checksum(10), checksum(11));
    }
}
