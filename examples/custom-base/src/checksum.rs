//! The real (tiny) computation, kept off the modal surface in `lib.rs`.
//!
//! `lib.rs` owns the input/output structs and the `#[modal_rust::function]`; this
//! module owns a small, deterministic checksum so the body is a genuine transform of
//! its input rather than a pure echo. The work is deliberately trivial — this example
//! teaches the BUILD-path base-image knobs (see `lib.rs`), not the computation — but it
//! is a real, well-known algorithm, not a fixed constant.
//!
//! The algorithm is FNV-1a (64-bit), the canonical Fowler–Noll–Vo hash: start from the
//! offset basis, then for each input byte XOR it in and multiply by the FNV prime. It
//! is CPU-only, allocation-free, and fully deterministic, so the same `value` always
//! yields the same checksum on every base image.

/// The 64-bit FNV-1a offset basis (the standard starting state for the hash).
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
/// The 64-bit FNV prime (the standard multiplier applied after each byte).
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Compute the FNV-1a 64-bit checksum of `value`'s big-endian byte representation.
///
/// This is a real hash, not an echo: distinct inputs almost always map to distinct
/// outputs, and the result is fully determined by the input (so it is identical across
/// base images and runs). Hashing the fixed-width big-endian bytes keeps the mapping
/// stable regardless of host endianness.
///
/// # Examples
///
/// ```
/// use example_custom_base::checksum::fnv1a_checksum;
/// // Deterministic: same input -> same checksum, every time.
/// assert_eq!(fnv1a_checksum(7), fnv1a_checksum(7));
/// // Not an echo, and not a constant: distinct inputs give distinct checksums.
/// assert_ne!(fnv1a_checksum(7), 7);
/// assert_ne!(fnv1a_checksum(7), fnv1a_checksum(8));
/// // Zero still hashes (it is not a special-cased no-op).
/// assert_ne!(fnv1a_checksum(0), 0);
/// ```
pub fn fnv1a_checksum(value: u64) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in value.to_be_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_deterministic() {
        assert_eq!(fnv1a_checksum(7), fnv1a_checksum(7));
        assert_eq!(fnv1a_checksum(0), fnv1a_checksum(0));
    }

    #[test]
    fn is_not_an_echo_or_constant() {
        // A real transform: the output is not the input, not a fixed value, and
        // distinct inputs map to distinct outputs.
        assert_ne!(fnv1a_checksum(7), 7);
        assert_ne!(fnv1a_checksum(0), 0);
        assert_ne!(fnv1a_checksum(7), fnv1a_checksum(8));
    }
}
