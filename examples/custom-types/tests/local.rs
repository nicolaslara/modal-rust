//! Offline proof (zero Modal, zero network) that a `#[modal_rust::function]` takes
//! and returns YOUR OWN structs: the input struct goes in, the output struct comes
//! back — the macro inferred the wire I/O from the `fn score(p: Player) -> Scored`
//! signature. The single user-struct param is the explicit-input path, so the call
//! site names the entrypoint and hands it your input struct directly.
//!
//! The assertions check the REAL scoring math (`points = hits * 100`, `accuracy_pct
//! = round(hits / shots * 100)`): exact percentages, rounding both ways, the
//! zero-shots guard, and determinism.

use example_custom_types::scoring::score_player;
use example_custom_types::{Player, Scored};
use modal_rust::App;

#[test]
fn score_round_trips_through_user_structs() {
    let app = App::local();
    // Your input struct in; your output struct back — no I/O type but your own.
    let scored: Scored = app
        .function("score")
        .local(Player {
            name: "Ada".to_string(),
            hits: 7,
            shots: 10,
        })
        .unwrap();

    assert_eq!(scored.name, "Ada");
    assert_eq!(scored.points, 700); // 7 hits * 100
    assert_eq!(scored.accuracy_pct, 70); // 7 / 10 = 70% exactly
}

#[test]
fn plain_fn_is_directly_callable() {
    // The macro emits the user fn verbatim, so it stays a plain Rust fn over your
    // structs.
    let scored = example_custom_types::score(Player {
        name: "Lin".to_string(),
        hits: 3,
        shots: 4,
    })
    .unwrap();
    assert_eq!(scored.points, 300); // 3 hits * 100
    assert_eq!(scored.accuracy_pct, 75); // 3 / 4 = 75% exactly
}

#[test]
fn accuracy_rounds_to_the_nearest_percent() {
    // 1/3 = 33.33% rounds DOWN to 33; 2/3 = 66.66% rounds UP to 67.
    assert_eq!(score_player(1, 3), (100, 33));
    assert_eq!(score_player(2, 3), (200, 67));
}

#[test]
fn zero_shots_is_zero_accuracy_not_a_panic() {
    // A player who never fired scores 0% accuracy (no divide-by-zero).
    assert_eq!(score_player(5, 0), (500, 0));
}

#[test]
fn scoring_is_deterministic() {
    // Pure arithmetic: same input -> same output, every time.
    assert_eq!(score_player(7, 10), score_player(7, 10));
}
