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
//! Modal credentials. Run/deploy is driven by the modal-rust CLI (no runner bin needed).

use modal_rust::function;
use serde::{Deserialize, Serialize};

mod delivery;

/// One recipient to notify — the per-input unit of work. Plain user structs you own;
/// the macro uses them AS the wire input/output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipient {
    /// Who to notify; carried into the receipt so a send is traceable.
    pub name: String,
    /// The channel to send on (e.g. "email", "sms").
    pub channel: String,
}

/// The per-recipient outcome — a receipt for the notification that was sent. With
/// `.for_each` and `.spawn_map` you do NOT collect these, but the function still
/// returns one (so the SAME handler also works with `.map()` / `.remote()`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    /// The recipient this receipt is for (carried through from the input).
    pub name: String,
    /// The channel the notification was sent on (carried through from the input).
    pub channel: String,
    /// A stable id for this send, computed from `(name, channel)`. Same recipient on
    /// the same channel always gets the same id; different inputs get different ids.
    pub receipt_id: String,
    /// A human-readable confirmation line describing the send.
    pub sent: String,
}

/// Notify ONE recipient — the whole per-input side effect. Delivers the notification
/// and returns its receipt; the real per-send work (a stable id derived from the
/// recipient plus a channel-resolved confirmation) lives in [`delivery::deliver`], so
/// this file stays the clean modal surface. Each call depends only on its own input,
/// so a batch fans out cleanly with `.for_each` / `.spawn_map`.
///
/// Because the single parameter is one of your own structs, the macro uses
/// `Recipient` AS the wire input and `Receipt` AS the wire output; the call site
/// names the entrypoint and hands it your struct directly:
/// `app.function("notify").for_each(recipients).await?`.
#[function]
pub fn notify(r: Recipient) -> anyhow::Result<Receipt> {
    Ok(delivery::deliver(&r))
}
