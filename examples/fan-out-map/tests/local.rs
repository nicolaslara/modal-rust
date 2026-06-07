//! Offline proof (zero Modal, zero network) of the fan-out concept: the SAME
//! `app.function("analyze")` handle that drives the live `.map([..])` shape also runs
//! the real handler in-process per input. Mapping `.local()` over a batch is the
//! local mirror of `.map(..)` — same per-record result, results in INPUT ORDER. The
//! live `.map([..])` fan-out across containers is compiled by the binary and proven
//! against real Modal in the credential-gated tour (`RUN_REMOTE=1 cargo run -p
//! example-fan-out-map --bin fan_out_map`).

use example_fan_out_map::reading::analyze_body;
use example_fan_out_map::{Document, Reading};
use modal_rust::App;

fn docs() -> Vec<Document> {
    [("a", "one two three"), ("b", "four five"), ("c", "six")]
        .into_iter()
        .map(|(title, body)| Document {
            title: title.to_string(),
            body: body.to_string(),
        })
        .collect()
}

#[test]
fn local_fan_out_returns_results_in_input_order() {
    // The local fan-out: run `.local()` per input, in order. This is the offline
    // mirror of `.map(..)` — no Modal, no network, no credentials.
    let app = App::local();
    let out: Vec<Reading> = docs()
        .into_iter()
        .map(|doc| app.function("analyze").local(doc))
        .collect::<Result<_, _>>()
        .expect("the .local() path should run in-process for every input");

    // Results aligned to input order: item k is the analysis of input k.
    assert_eq!(
        out.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
        ["a", "b", "c"],
    );
    assert_eq!(out.iter().map(|r| r.words).collect::<Vec<_>>(), [3, 2, 1]);
    // Every short doc floors to one minute at 200 wpm.
    assert!(out.iter().all(|r| r.minutes == 1));
}

#[test]
fn analyze_body_counts_words_and_reading_minutes() {
    // The extracted computation is real and deterministic: whitespace-separated
    // word count plus reading minutes at 200 wpm (rounded up, floored at one).
    assert_eq!(analyze_body("one two three"), (3, 1)); // short doc floors to 1 min
    assert_eq!(analyze_body(""), (0, 1)); //              empty still reads as 1 min
    assert_eq!(analyze_body("a  b\tc\nd"), (4, 1)); //    whitespace runs are 1 split
    assert_eq!(analyze_body(&"w ".repeat(450)), (450, 3)); // ceil(450 / 200) = 3 min
                                                           // Deterministic: the same input always yields the same output.
    assert_eq!(
        analyze_body("repeatable input"),
        analyze_body("repeatable input")
    );
}

#[test]
fn plain_fn_is_directly_callable() {
    // The macro emits the user fn verbatim, so it stays a plain Rust fn over your
    // structs.
    let r = example_fan_out_map::analyze(Document {
        title: "doc".to_string(),
        body: "w ".repeat(450),
    })
    .unwrap();
    assert_eq!(r.words, 450);
    assert_eq!(r.minutes, 3); // ceil(450 / 200) = 3
}
