//! The real per-job computation, kept off the modal surface in `lib.rs`.
//!
//! `lib.rs` owns the input/output structs and the `#[modal_rust::function]`; this
//! module owns the work the job actually grinds through so the surface reads as nothing
//! but "your struct in, your struct out". The work is small, CPU-only, and fully
//! deterministic — the same `rounds` always folds to the same digest, which is what lets
//! the offline `.local()` run stand in for the value a live `.spawn()` would `.get()`.

/// Fold a deterministic digest over `rounds` iterations of a small xorshift-style mix.
///
/// Each round mixes the round index into the running digest with a multiply, a
/// rotate, and an add — cheap integer work with no allocations, but real work that
/// scales with `rounds` (the "longer job" knob). It is pure and deterministic: the same
/// `rounds` always returns the same `u64`.
///
/// # Examples
///
/// ```
/// use example_background_jobs::work::digest;
/// assert_eq!(digest(0), 0x9e37_79b9_7f4a_7c15); // zero rounds -> the untouched seed
/// assert_eq!(digest(1_000), digest(1_000)); //      deterministic: same in, same out
/// assert_ne!(digest(1_000), digest(1_001)); //      more rounds -> a different digest
/// ```
pub fn digest(rounds: u64) -> u64 {
    // A deterministic fold (a small xorshift mix per round) — same input, same digest.
    let mut digest: u64 = 0x9e37_79b9_7f4a_7c15;
    for round in 0..rounds {
        digest ^= round.wrapping_mul(0x2545_f491_4f6c_dd1d);
        digest = digest.rotate_left(13).wrapping_add(round);
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_rounds_is_the_untouched_seed() {
        assert_eq!(digest(0), 0x9e37_79b9_7f4a_7c15);
    }

    #[test]
    fn is_deterministic() {
        // The same number of rounds always folds to the same digest.
        assert_eq!(digest(250_000), digest(250_000));
        assert_eq!(digest(1_000), digest(1_000));
    }

    #[test]
    fn more_rounds_change_the_digest() {
        // The digest depends on the work done — one more round is a different result.
        assert_ne!(digest(1_000), digest(1_001));
        assert_ne!(digest(0), digest(1));
    }
}
