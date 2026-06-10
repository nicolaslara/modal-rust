//! Frequency-based extractive summarization — the real computation behind the
//! endpoint, kept in its own module so `src/lib.rs` stays the clean modal-rust
//! surface.
//!
//! The classic word-frequency heuristic (Luhn-style): a sentence is representative
//! of a text in proportion to how often its content words recur across the WHOLE
//! text. So we (1) split the text into sentences, (2) count every content word
//! (stopwords excluded — "the"/"and"/… recur everywhere and signal nothing),
//! (3) score each sentence by the MEAN whole-text frequency of its content words,
//! and (4) keep the top scorers, re-emitted in their original order so the summary
//! still reads as prose. Deterministic, offline, dependency-free — but a real
//! group-by/score/rank computation, not a fixed string.

use std::collections::HashMap;

/// Words too common to signal what a sentence is about. Counting these would make
/// every sentence score alike (they recur everywhere), so the frequency model
/// excludes them from both the corpus counts and the per-sentence score.
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "if", "in", "into", "is", "it",
    "its", "of", "on", "or", "that", "the", "their", "then", "there", "this", "to", "was", "were",
    "will", "with", "you", "your",
];

/// Split `text` into sentences on terminal punctuation (`.` / `!` / `?`), trimmed,
/// keeping the terminator. Fragments with no alphanumeric content (stray "..." or
/// whitespace runs) are dropped; a trailing fragment without a terminator counts as
/// a sentence so no input text is silently lost.
pub fn split_sentences(text: &str) -> Vec<String> {
    text.split_inclusive(['.', '!', '?'])
        .map(str::trim)
        .filter(|s| s.chars().any(char::is_alphanumeric))
        .map(str::to_string)
        .collect()
}

/// Count every word token in `text` — the input-size statistic the endpoint reports
/// (stopwords included; this is a plain size measure, not the frequency model).
pub fn word_count(text: &str) -> usize {
    tokens(text).len()
}

/// Pick the `max_sentences` most representative members of `sentences`, returned in
/// their ORIGINAL order (a summary should still read as prose, not as a ranking).
///
/// Score = mean whole-text frequency of the sentence's content words, so sentences
/// about the text's dominant subject outrank one-off asides. The sort is stable and
/// descending, so tied scores keep the earlier sentence. Fewer sentences than
/// `max_sentences` ⇒ everything is kept.
pub fn pick_top(sentences: &[String], max_sentences: usize) -> Vec<String> {
    // Whole-text content-word frequencies (the corpus the per-sentence score reads).
    let mut freq: HashMap<String, usize> = HashMap::new();
    for sentence in sentences {
        for word in content_words(sentence) {
            *freq.entry(word).or_insert(0) += 1;
        }
    }

    // Score every sentence: the MEAN frequency of its content words (mean, not sum,
    // so a long sentence is not rewarded for length alone). All-stopword sentences
    // score 0.0 — nothing to be representative OF.
    let mut ranked: Vec<(usize, f64)> = sentences
        .iter()
        .enumerate()
        .map(|(index, sentence)| {
            let words = content_words(sentence);
            let score = if words.is_empty() {
                0.0
            } else {
                let total: usize = words.iter().map(|w| freq[w.as_str()]).sum();
                total as f64 / words.len() as f64
            };
            (index, score)
        })
        .collect();

    // Highest score first; `sort_by` is stable, so equal scores keep original order.
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));

    // Keep the winners, then restore ORIGINAL order for readable output.
    let mut keep: Vec<usize> = ranked
        .into_iter()
        .take(max_sentences)
        .map(|(index, _)| index)
        .collect();
    keep.sort_unstable();
    keep.into_iter().map(|i| sentences[i].clone()).collect()
}

/// Lowercased alphanumeric word tokens of `text` (every word, stopwords included).
fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// [`tokens`] minus [`STOPWORDS`] — the words the frequency model counts and scores.
fn content_words(text: &str) -> Vec<String> {
    tokens(text)
        .into_iter()
        .filter(|w| !STOPWORDS.contains(&w.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_sentences_keeps_terminators_and_drops_empty_fragments() {
        let sentences = split_sentences("One sentence. Another!  ...   Third? trailing tail");
        assert_eq!(
            sentences,
            vec!["One sentence.", "Another!", "Third?", "trailing tail"]
        );
    }

    #[test]
    fn word_count_counts_every_token() {
        assert_eq!(word_count("The cat, the hat."), 4);
        assert_eq!(word_count(""), 0);
    }

    #[test]
    fn pick_top_prefers_the_dominant_subject_and_keeps_original_order() {
        let sentences = split_sentences(
            "Compilers translate source code. My lunch was nice. \
             Source code feeds the compilers.",
        );
        // "compilers"/"source"/"code" each recur (freq 2); the lunch aside is all
        // freq-1 words — so top-2 is the FIRST and THIRD sentence, in original order.
        let picked = pick_top(&sentences, 2);
        assert_eq!(
            picked,
            vec![
                "Compilers translate source code.",
                "Source code feeds the compilers."
            ]
        );
    }

    #[test]
    fn pick_top_caps_at_the_available_sentences() {
        let sentences = split_sentences("Only one sentence here.");
        assert_eq!(pick_top(&sentences, 5), sentences);
        assert!(pick_top(&[], 3).is_empty());
    }
}
