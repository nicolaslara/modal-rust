//! The real per-document computation, kept off the modal surface in `lib.rs`.
//!
//! `lib.rs` owns the input/output structs and the `#[modal_rust::function]`; this
//! module owns the reading-time analysis so the surface reads as nothing but "your
//! struct in, your struct out". The work is small, CPU-only, and fully deterministic.

/// Analyze one document body and return `(words, minutes)`.
///
/// - `words` is the count of whitespace-separated tokens (runs of whitespace are a
///   single separator, so this collapses double spaces, tabs, and newlines).
/// - `minutes` is the estimated reading time at 200 words/minute, rounded up, with a
///   floor of one minute so any non-empty document still reads as "1 min".
///
/// # Examples
///
/// ```
/// use example_fan_out_map::reading::analyze_body;
/// assert_eq!(analyze_body("one two three"), (3, 1)); // short -> floors to 1 min
/// assert_eq!(analyze_body(""), (0, 1)); //               empty still reads as 1 min
/// assert_eq!(analyze_body(&"w ".repeat(450)), (450, 3)); // ceil(450 / 200) = 3 min
/// assert_eq!(analyze_body("a  b\tc\nd"), (4, 1)); // whitespace runs are one split
/// ```
pub fn analyze_body(body: &str) -> (u32, u32) {
    let words = body.split_whitespace().count() as u32;
    // 200 wpm, rounded up, with a floor of one minute for any non-empty document.
    let minutes = words.div_ceil(200).max(1);
    (words, minutes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_words_and_floors_read_time() {
        assert_eq!(analyze_body("one two three"), (3, 1));
        assert_eq!(analyze_body(""), (0, 1));
    }

    #[test]
    fn rounds_read_time_up_at_200_wpm() {
        assert_eq!(analyze_body(&"w ".repeat(450)), (450, 3)); // ceil(450 / 200) = 3
        assert_eq!(analyze_body(&"w ".repeat(200)), (200, 1)); // exactly one minute
        assert_eq!(analyze_body(&"w ".repeat(201)), (201, 2)); // one over -> 2 minutes
    }

    #[test]
    fn collapses_runs_of_whitespace() {
        // Multiple spaces, tabs, and newlines are one separator (4 words).
        assert_eq!(analyze_body("a  b\tc\nd"), (4, 1));
    }
}
