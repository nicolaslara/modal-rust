//! The real (small, deterministic) work the snapshot [`Concordance`] class loads ONCE.
//!
//! This is the expensive state the snapshot-class example keeps warm: a sorted
//! word-concordance [`Index`] over an embedded text corpus. `#[enter]` builds it a single
//! time per warm container, then every `#[method]` query reuses it by `&self`. When the
//! app is DEPLOYED with `enable_memory_snapshot = true`, Modal also snapshots the loaded
//! process so even cold containers restore this already-built index instead of re-running
//! the build — the "pay the load once, EVER" win.
//!
//! The build is intentionally small and dependency-free, but it is real index
//! construction, not an echo or a fixed constant: it tokenizes the corpus, counts every
//! word's occurrences, and produces a vector SORTED by word so queries can binary-search
//! it. A concordance like this is exactly the kind of precomputed lookup structure you
//! would not want to rebuild on every cold start.
//!
//! How it works:
//!
//! 1. Tokenize the corpus into lowercase alphabetic words (split on any non-letter).
//! 2. Accumulate a count per distinct word in a hash map.
//! 3. Flatten to a `Vec<(word, count)>` SORTED by word — the sort is the precompute that
//!    makes [`Index::search`] a binary search (`O(log n)`) instead of a linear scan.
//!
//! It is fully deterministic: the same corpus always yields the same index, on any run.
//!
//! [`Concordance`]: crate::Concordance

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The embedded corpus the index is built from. A small, fixed, public-domain text
/// (opening of *Pride and Prejudice*). Real enough to make the sorted index non-trivial,
/// small enough to keep the build offline and near-instant.
const CORPUS: &str = "\
It is a truth universally acknowledged, that a single man in possession of a good \
fortune, must be in want of a wife. However little known the feelings or views of \
such a man may be on his first entering a neighbourhood, this truth is so well fixed \
in the minds of the surrounding families, that he is considered the rightful property \
of some one or other of their daughters. My dear Mr Bennet, said his lady to him one \
day, have you heard that Netherfield Park is let at last? Mr Bennet replied that he \
had not. But it is, returned she; for Mrs Long has just been here, and she told me all \
about it. Mr Bennet made no answer. Do you not want to know who has taken it? cried his \
wife impatiently. You want to tell me, and I have no objection to hearing it. This was \
invitation enough.";

/// Process-global count of how many times [`Index::build`] has run. The load-once test
/// reads it to PROVE `#[enter]` (which calls `Index::build`) fires exactly once across
/// many method calls — the whole point of a `#[cls]`. Process-global because the
/// generated `#[enter]` singleton is too.
static BUILD_COUNT: AtomicUsize = AtomicUsize::new(0);

/// How many times [`Index::build`] has been called in this process. Read by
/// `tests/local.rs` to assert the load-once behavior.
pub fn build_count() -> usize {
    BUILD_COUNT.load(Ordering::SeqCst)
}

/// One concordance entry: a distinct corpus word and how many times it occurs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    /// The word, lowercased.
    pub word: String,
    /// How many times the word appears in the corpus.
    pub count: usize,
}

/// The precomputed concordance index — the expensive state loaded once per warm
/// container. It holds the corpus's distinct words and their occurrence counts in a
/// vector SORTED by word, so a prefix query is a binary search. In a real app this would
/// be a large index read from disk or a model; here it is built from the embedded
/// [`CORPUS`] via [`Index::build`].
pub struct Index {
    /// Distinct words with their counts, SORTED ascending by `word`. The sort is the
    /// precompute [`Index::search`] relies on.
    entries: Vec<Entry>,
}

impl Index {
    /// Build the index from the embedded corpus. This is the expensive step the
    /// `#[enter]` body calls exactly once per warm container. There is NO download and NO
    /// network — the corpus is embedded and the work is plain tokenize/count/sort — so the
    /// example stays offline and CPU-only while still doing real index construction.
    pub fn build() -> Index {
        // Bump the process-global build counter so the load-once test can prove `#[enter]`
        // ran exactly once across many method calls.
        BUILD_COUNT.fetch_add(1, Ordering::SeqCst);

        // 1 + 2: tokenize into lowercase alphabetic words and count each distinct word.
        let mut counts: HashMap<String, usize> = HashMap::new();
        for word in CORPUS
            .split(|c: char| !c.is_alphabetic())
            .filter(|w| !w.is_empty())
        {
            *counts.entry(word.to_lowercase()).or_insert(0) += 1;
        }

        // 3: flatten to a vector SORTED by word — the precompute that turns every later
        // query into a binary search instead of a linear scan.
        let mut entries: Vec<Entry> = counts
            .into_iter()
            .map(|(word, count)| Entry { word, count })
            .collect();
        entries.sort_by(|a, b| a.word.cmp(&b.word));

        Index { entries }
    }

    /// The number of distinct words in the index.
    pub fn distinct_words(&self) -> usize {
        self.entries.len()
    }

    /// Return every index entry whose word starts with `prefix` (case-insensitive),
    /// in sorted order. Uses the sorted index: a binary search finds the first candidate,
    /// then a short forward scan collects the contiguous run of matches — `O(log n + k)`
    /// for `k` matches, the payoff of the precomputed sort. An empty prefix returns the
    /// whole (sorted) index.
    pub fn search(&self, prefix: &str) -> Vec<Entry> {
        let prefix = prefix.to_lowercase();
        // Binary search for the first entry >= the prefix; the run of prefix matches is
        // contiguous from there because the index is sorted.
        let start = self
            .entries
            .partition_point(|e| e.word.as_str() < prefix.as_str());
        self.entries[start..]
            .iter()
            .take_while(|e| e.word.starts_with(&prefix))
            .cloned()
            .collect()
    }
}
