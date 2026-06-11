//! The example's honest computation: Collatz stopping times.
//!
//! Kept in its own module so `lib.rs` stays the clean modal-rust surface (the
//! `#[function]` + the Queue wiring) and the math is testable on its own.

/// The Collatz total stopping time of `n`: how many `n -> n/2` (even) /
/// `n -> 3n+1` (odd) steps until `n == 1`. `collatz_steps(0)` and
/// `collatz_steps(1)` are 0. Uses checked arithmetic so a pathological input
/// saturates the count instead of overflowing (no input below 2^64 is known to
/// overflow u64 mid-sequence, but the math stays honest).
pub fn collatz_steps(n: u64) -> u64 {
    let mut n = n.max(1);
    let mut steps = 0u64;
    while n != 1 {
        n = if n.is_multiple_of(2) {
            n / 2
        } else {
            match n.checked_mul(3).and_then(|m| m.checked_add(1)) {
                Some(m) => m,
                None => break, // saturate: stop counting rather than overflow
            }
        };
        steps += 1;
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_stopping_times() {
        // The classics: 27 takes 111 steps; 9 takes 19; 97 takes 118.
        assert_eq!(collatz_steps(1), 0);
        assert_eq!(collatz_steps(2), 1);
        assert_eq!(collatz_steps(9), 19);
        assert_eq!(collatz_steps(27), 111);
        assert_eq!(collatz_steps(97), 118);
    }

    #[test]
    fn degenerate_inputs_are_zero_steps() {
        assert_eq!(collatz_steps(0), 0);
    }
}
