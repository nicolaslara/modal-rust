//! The example's honest computation: standard English Scrabble letter values.
//!
//! Kept in its own module so `lib.rs` stays the clean modal-rust surface (the
//! `#[function]` + the Dict wiring) and the math is testable on its own.

/// Score one word with the standard English Scrabble letter values.
/// Case-insensitive; characters outside `a..=z` score 0.
pub fn scrabble_score(word: &str) -> i64 {
    word.chars()
        .map(|c| letter_score(c.to_ascii_lowercase()))
        .sum()
}

/// The standard English Scrabble value of one (lowercase) letter.
fn letter_score(c: char) -> i64 {
    match c {
        'a' | 'e' | 'i' | 'o' | 'u' | 'l' | 'n' | 's' | 't' | 'r' => 1,
        'd' | 'g' => 2,
        'b' | 'c' | 'm' | 'p' => 3,
        'f' | 'h' | 'v' | 'w' | 'y' => 4,
        'k' => 5,
        'j' | 'x' => 8,
        'q' | 'z' => 10,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_scrabble_scores() {
        // j8 a1 z10 z10 = 29; q10 u1 a1 r1 t1 z10 = 24; r1 u1 s1 t1 = 4.
        assert_eq!(scrabble_score("jazz"), 29);
        assert_eq!(scrabble_score("quartz"), 24);
        assert_eq!(scrabble_score("rust"), 4);
    }

    #[test]
    fn case_insensitive_and_non_letters_score_zero() {
        assert_eq!(scrabble_score("Rust"), scrabble_score("rust"));
        assert_eq!(scrabble_score("a-z 9!"), 1 + 10);
        assert_eq!(scrabble_score(""), 0);
    }
}
