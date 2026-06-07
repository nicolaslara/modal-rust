//! The real (small, deterministic) embedding model behind the [`embed`] entrypoint.
//!
//! This is the compute the autoscaling example keeps warm. It is intentionally small
//! and dependency-free — a real text embedding done with the standard library, not an
//! echo and not a fixed constant. The shape mirrors a sentence embedder: text in, a
//! fixed-width unit-length vector out, where similar inputs land near each other.
//!
//! How it works (a hashed character-trigram bag-of-features):
//!
//! 1. Slide a 3-character window over the input's `char`s to collect trigrams. Trigrams
//!    capture local letter structure, so two texts that share substrings share features.
//! 2. Hash each trigram with the std [`DefaultHasher`] and fold it into one of
//!    [`EMBED_DIMENSIONS`] buckets, accumulating a count per bucket. This is the
//!    classic "hashing trick" — a fixed-width feature vector with no learned vocabulary.
//! 3. L2-normalize the vector to unit length, so the embedding measures direction
//!    (relative feature mix) rather than raw document length. The empty/whitespace-only
//!    case has no trigrams and stays the zero vector.
//!
//! It is fully deterministic: the same text always yields the same vector, on any run.
//!
//! [`embed`]: crate::embed

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// The fixed embedding dimensionality the model produces.
pub const EMBED_DIMENSIONS: usize = 8;

/// Embed `text` into a fixed-width unit-length feature vector.
///
/// Returns a `Vec<f32>` of length [`EMBED_DIMENSIONS`]. For any text with at least one
/// character trigram the vector is L2-normalized to unit length (its squared components
/// sum to ~1.0); text with no trigrams (empty, or fewer than 3 chars of content) maps
/// to the zero vector. The mapping is deterministic across calls and runs.
pub fn embed_text(text: &str) -> Vec<f32> {
    let mut vector = vec![0.0f32; EMBED_DIMENSIONS];

    // Character trigrams: slide a 3-char window so local letter structure becomes
    // features (we work over `char`s, not bytes, so multi-byte text is handled).
    let chars: Vec<char> = text.chars().collect();
    for window in chars.windows(3) {
        let mut hasher = DefaultHasher::new();
        window.hash(&mut hasher);
        // The hashing trick: fold the trigram's hash into one of the DIM buckets.
        let bucket = (hasher.finish() % EMBED_DIMENSIONS as u64) as usize;
        vector[bucket] += 1.0;
    }

    // L2-normalize to unit length so the vector captures the feature MIX, not document
    // length. No trigrams -> zero vector, which we leave as-is (nothing to normalize).
    let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in vector.iter_mut() {
            *x /= norm;
        }
    }

    vector
}
