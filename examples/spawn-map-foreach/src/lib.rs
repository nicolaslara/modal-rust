//! `examples/spawn-map-foreach` — the rest of the map family: fire-and-forget
//! fan-out and side-effect maps.
//!
//! Teaching ONE concept: `.map()` collects N RESULTS in order — but sometimes you
//! fan out for the SIDE EFFECT and do not need the results back. Two members of the
//! map family cover that:
//!
//! ```text
//! app.notify(..).for_each([r0, r1, r2, ..]).await    // run all N, WAIT, discard results -> ()
//! app.notify(..).spawn_map([r0, r1, r2, ..]).await   // fire all N, return at once -> a handle
//! ```
//!
//! - `.for_each([..])` runs the function over every input across containers, WAITS
//!   for them all to finish, and discards the outputs (returns `()`). Use it to drive
//!   work whose result you do not need — here, sending a notification to each
//!   recipient. It is `.map()` with the results thrown away.
//! - `.spawn_map([..])` enqueues every input and returns a handle IMMEDIATELY,
//!   without waiting for the fan-out to finish (fire-and-forget). It is `.spawn()`
//!   (one input) generalized to N inputs.
//!
//! (`.starmap([..])` rounds out the family: a `.map()` whose each input is a
//! tuple/sequence; with modal-rust's single named-object input it shares `.map()`'s
//! wire path, so this example focuses on the two that genuinely differ from `.map()`.)
//!
//! The companion `src/bin/spawn_map_foreach.rs` is the runnable tour: the OFFLINE
//! default runs the real handler in-process for every recipient (the local mirror of
//! `.for_each(..)` — every side effect happens, results discarded); the live
//! `.for_each([..])` / `.spawn_map([..])` shapes compile always and run only with
//! Modal credentials. `src/bin/modal_runner.rs` is the one-line runner.

use modal_rust::function;
use serde::{Deserialize, Serialize};

/// One recipient to notify — the per-input unit of work. Plain user structs you own;
/// the macro uses them AS the wire input/output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipient {
    /// Who to notify; echoed into the receipt so a side effect is traceable.
    pub name: String,
    /// The channel to send on (e.g. "email", "sms").
    pub channel: String,
}

/// The per-recipient outcome — proof the notification was "sent". With `.for_each`
/// and `.spawn_map` you do NOT collect these, but the function still returns one (so
/// the SAME handler also works with `.map()` / `.remote()`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    /// The recipient this receipt is for (carried through from the input).
    pub name: String,
    /// A short, deterministic confirmation line describing the side effect.
    pub sent: String,
}

/// Notify ONE recipient — the whole per-input side effect. In a real app this would
/// hit an email/SMS provider; here it formats a deterministic confirmation so the
/// example is offline-runnable and assertable. Each call depends only on its own
/// input, so a batch fans out cleanly with `.for_each` / `.spawn_map`.
///
/// Because the single parameter is one of your own structs, the macro uses
/// `Recipient` AS the wire input and `Receipt` AS the wire output; the call site
/// names the entrypoint and hands it your struct directly:
/// `app.function("notify").for_each(recipients).await?`.
#[function]
pub fn notify(r: Recipient) -> anyhow::Result<Receipt> {
    Ok(Receipt {
        sent: format!("notified {} via {}", r.name, r.channel),
        name: r.name,
    })
}
