//! The real scoring computation, kept out of the modal surface in `lib.rs`.
//!
//! `lib.rs` owns the input/output structs and the `#[modal_rust::function]`; this
//! module owns the arithmetic so the surface reads as nothing but "your structs
//! in, your struct out". The math is small, CPU-only, and fully deterministic.

/// Score a single match from raw `(hits, shots)` and return `(points, accuracy_pct)`.
///
/// - `points` is the headline score: one hit is worth 100 points.
/// - `accuracy_pct` is `hits / shots` as a whole-number percentage, rounded to the
///   nearest percent (the `+ shots / 2` before the division is the round-half-up
///   term). A player who took zero shots scores 0% rather than dividing by zero.
///
/// # Examples
///
/// ```
/// use example_custom_types::scoring::score_player;
/// assert_eq!(score_player(7, 10), (700, 70)); // 70.0% exactly
/// assert_eq!(score_player(3, 4), (300, 75)); //  75.0% exactly
/// assert_eq!(score_player(1, 3), (100, 33)); //  33.33.. rounds down to 33
/// assert_eq!(score_player(2, 3), (200, 67)); //  66.66.. rounds up to 67
/// assert_eq!(score_player(5, 0), (500, 0)); //  no shots -> 0%, no panic
/// ```
pub fn score_player(hits: u32, shots: u32) -> (u32, u32) {
    let points = hits * 100;
    let accuracy_pct = (hits * 100 + shots / 2).checked_div(shots).unwrap_or(0);
    (points, accuracy_pct)
}
