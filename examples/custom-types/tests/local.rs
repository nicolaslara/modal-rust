//! Offline proof (zero Modal, zero network) that a `#[modal_rust::function]` takes
//! and returns YOUR OWN structs: the input struct goes in, the output struct comes
//! back — the macro inferred the wire I/O from the `fn score(p: Player) -> Scored`
//! signature. The single user-struct param is the explicit-input path, so the call
//! site names the entrypoint and hands it your input struct directly.

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
    assert_eq!(scored.points, 700);
    assert_eq!(scored.accuracy_pct, 70);
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
    assert_eq!(scored.points, 300);
    assert_eq!(scored.accuracy_pct, 75);
}
