//! The real per-document computation, kept off the modal surface in `lib.rs`.
//!
//! `summarize_text` does the actual work — counting words and characters and
//! estimating a read time — so `lib.rs` is just the input/output types plus the
//! `#[function]` that decodes the request and calls in here.

use crate::Summary;

/// Compute a small, deterministic digest of `text`: word count, character count,
/// and an estimated read time in minutes (rounded up at 200 words/minute, floored
/// at one minute so any non-empty document still reads as "1 min").
pub fn summarize_text(text: &str) -> Summary {
    let words = text.split_whitespace().count();
    let chars = text.chars().count();
    let read_minutes = words.div_ceil(200).max(1);
    Summary {
        words,
        chars,
        read_minutes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_words_chars_and_read_time() {
        let s = summarize_text("the quick brown fox");
        assert_eq!(s.words, 4);
        assert_eq!(s.chars, 19);
        assert_eq!(s.read_minutes, 1);
    }

    #[test]
    fn collapses_runs_of_whitespace_and_counts_unicode_chars() {
        // Multiple spaces/newlines collapse to one separator for WORD counting (4
        // words), but `chars()` counts every Unicode scalar value — including each
        // whitespace char — so the two spaces, tab, and newline all count: café(4) +
        // 2 spaces + is(2) + tab(1) + quite(5) + newline(1) + nice(4) = 19. The
        // accented `é` is a single scalar, so `café` is 4 chars, not 5.
        let s = summarize_text("café  is\tquite\nnice");
        assert_eq!(s.words, 4);
        assert_eq!(s.chars, 19);
    }

    #[test]
    fn empty_text_is_zero_words_but_floors_read_time_at_one() {
        let s = summarize_text("");
        assert_eq!(s.words, 0);
        assert_eq!(s.chars, 0);
        assert_eq!(s.read_minutes, 1);
    }

    #[test]
    fn read_time_rounds_up_past_two_hundred_words() {
        let text = "word ".repeat(201);
        let s = summarize_text(&text);
        assert_eq!(s.words, 201);
        assert_eq!(s.read_minutes, 2);
    }

    #[test]
    fn is_deterministic() {
        let a = summarize_text("repeatable input text");
        let b = summarize_text("repeatable input text");
        assert_eq!(a.words, b.words);
        assert_eq!(a.chars, b.chars);
        assert_eq!(a.read_minutes, b.read_minutes);
    }
}
