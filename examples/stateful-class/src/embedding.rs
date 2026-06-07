//! The real (small, deterministic) embedding model the [`Embedder`] class loads ONCE.
//!
//! This is the expensive state the stateful-class example keeps warm: a [`Model`] that
//! `#[enter]` builds a single time per warm container, then every `#[method]` reuses. It
//! is intentionally small and dependency-free — a real text embedding done with the
//! standard library, not an echo and not a fixed constant. The shape mirrors a sentence
//! embedder: text in, a fixed-width unit-length vector out, where similar inputs land
//! near each other.
//!
//! How it works (a hashed character-trigram bag-of-features):
//!
//! 1. Slide a 3-character window over the input's `char`s to collect trigrams. Trigrams
//!    capture local letter structure, so two texts that share substrings share features.
//! 2. Hash each trigram with the std [`DefaultHasher`] and fold it into one of `dim`
//!    buckets, accumulating a count per bucket. This is the classic "hashing trick" — a
//!    fixed-width feature vector with no learned vocabulary.
//! 3. L2-normalize the vector to unit length, so the embedding measures direction
//!    (relative feature mix) rather than raw document length. The empty/whitespace-only
//!    case has no trigrams and stays the zero vector.
//!
//! It is fully deterministic: the same text always yields the same vector, on any run.
//!
//! [`Embedder`]: crate::Embedder

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The fixed embedding dimensionality this model produces.
pub const EMBED_DIMENSIONS: usize = 8;

/// Process-global count of how many times [`Model::load`] has run. The load-once test
/// reads it to PROVE `#[enter]` (which calls `Model::load`) fires exactly once across
/// many method calls — the whole point of `#[cls]`. Process-global because the generated
/// `#[enter]` singleton is too.
static LOAD_COUNT: AtomicUsize = AtomicUsize::new(0);

/// How many times [`Model::load`] has been called in this process. Read by
/// `tests/local.rs` to assert the load-once behavior.
pub fn load_count() -> usize {
    LOAD_COUNT.load(Ordering::SeqCst)
}

/// The (tiny, real) embedding model — the expensive state loaded once per warm
/// container. In a real app this would hold model weights read from disk or a GPU
/// handle; here it holds the fixed output dimensionality. Built via [`Model::load`].
pub struct Model {
    dim: usize,
}

impl Model {
    /// Build (load) the model. This is the expensive step the `#[enter]` body calls
    /// exactly once per warm container. There is NO model download and NO network — the
    /// "weights" are the deterministic hashing scheme below — so the example stays
    /// offline and CPU-only while still doing real compute.
    pub fn load() -> Model {
        // Bump the process-global load counter so the load-once test can prove `#[enter]`
        // ran exactly once across many method calls.
        LOAD_COUNT.fetch_add(1, Ordering::SeqCst);
        Model {
            dim: EMBED_DIMENSIONS,
        }
    }

    /// The output dimensionality of the vectors this model produces.
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Embed `text` into a fixed-width unit-length feature vector.
    ///
    /// Returns a `Vec<f32>` of length [`Model::dim`]. For any text with at least one
    /// character trigram the vector is L2-normalized to unit length (its squared
    /// components sum to ~1.0); text with no trigrams (empty, or fewer than 3 chars of
    /// content) maps to the zero vector. The mapping is deterministic across calls/runs.
    pub fn embed(&self, text: &str) -> Vec<f32> {
        let mut vector = vec![0.0f32; self.dim];

        // Character trigrams: slide a 3-char window so local letter structure becomes
        // features (we work over `char`s, not bytes, so multi-byte text is handled).
        let chars: Vec<char> = text.chars().collect();
        for window in chars.windows(3) {
            let mut hasher = DefaultHasher::new();
            window.hash(&mut hasher);
            // The hashing trick: fold the trigram's hash into one of the DIM buckets.
            let bucket = (hasher.finish() % self.dim as u64) as usize;
            vector[bucket] += 1.0;
        }

        // L2-normalize to unit length so the vector captures the feature MIX, not
        // document length. No trigrams -> zero vector, which we leave as-is.
        let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in vector.iter_mut() {
                *x /= norm;
            }
        }

        vector
    }
}
