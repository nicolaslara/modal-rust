//! The rest of the map family: side-effect maps with `.for_each()` and
//! fire-and-forget fan-out with `.spawn_map()`.
//!
//! The single `#[modal_rust::function] fn notify(r)` from this crate's `lib.rs` is run
//! over a BATCH of recipients. The point of the call is the SIDE EFFECT (notify each
//! one), not the return values — so unlike `.map()` you do not collect results:
//!
//! - OFFLINE (default): the local mirror of `.for_each(..)`. `app.function("notify").
//!   local(r)?` runs the real handler in-process for each recipient, performing every
//!   side effect and discarding the receipt — zero Modal, zero network.
//! - LIVE (`RUN_REMOTE=1` + Modal credentials):
//!   - `app.function("notify").for_each(recipients).await?` runs all N across
//!     containers, WAITS for them to finish, and returns `()` (results discarded).
//!   - `app.function("notify").spawn_map(recipients).await?` enqueues all N and returns
//!     a handle IMMEDIATELY (fire-and-forget) — it does not wait for the fan-out.
//!
//! `.for_each` is the contrast with `.map()`: `.map()` collects `Vec<Out>` in input
//! order, `.for_each()` drives the same work but throws the outputs away. `.spawn_map`
//! is the contrast with `.for_each`: `.for_each` blocks until done, `.spawn_map`
//! returns at once. Both fan the SAME handler out over the SAME inputs.
//!
//! Because each input is one of your own structs (`Recipient`), the call site names
//! the entrypoint and hands it that struct directly — the same string-keyed
//! `app.function("notify")` handle drives every shape.

use example_spawn_map_foreach::{Receipt, Recipient};
use modal_rust::App;

/// The batch to fan out over — three recipients on different channels.
fn recipients() -> Vec<Recipient> {
    [("ada", "email"), ("babbage", "sms"), ("turing", "push")]
        .into_iter()
        .map(|(name, channel)| Recipient {
            name: name.to_string(),
            channel: channel.to_string(),
        })
        .collect()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let recipients = recipients();

    // ----- OFFLINE: the local mirror of .for_each — run every side effect ------------
    //
    // `App::local()` builds an in-process app from the `#[modal_rust::function]`
    // inventory. Running `app.function("notify").local(r)?` for each recipient performs
    // every notification side effect, in order, and discards the receipt — the local
    // mirror of `.for_each(..)`: same handler, same per-recipient work, with zero Modal,
    // zero network, nothing to install.
    let app = App::local();
    let mut sent = 0_usize;
    for r in &recipients {
        let receipt: Receipt = app.function("notify").local(r.clone())?;
        // The side effect: print the confirmation, with the stable receipt id the
        // function computed from this recipient (same input -> same id).
        println!("  {} (receipt {})", receipt.sent, receipt.receipt_id);
        sent += 1;
    }
    println!("for_each (local mirror): notified {sent} recipients, results discarded");
    assert_eq!(sent, recipients.len(), "every recipient must be notified");

    // ----- LIVE: .for_each([..]) then .spawn_map([..]) (credential-gated) ------------
    //
    // This hits real Modal, so it only runs when explicitly enabled. The code is always
    // compiled (it is the genuine API), it is just not executed by default.
    if std::env::var("RUN_REMOTE").as_deref() == Ok("1") {
        run_live(recipients).await?;
    } else {
        println!(
            "(skipping live .for_each([..]) + .spawn_map([..]) — set RUN_REMOTE=1 with \
             Modal credentials to fan out across containers)"
        );
    }

    Ok(())
}

/// The live fan-out against a connected App. `App::connect("name").await` builds a
/// live control-plane client (reading `~/.modal.toml` / `MODAL_TOKEN_*`) and uses the
/// inventory registry, so the SAME `app.function("notify")` handle drives both shapes.
async fn run_live(recipients: Vec<Recipient>) -> Result<(), Box<dyn std::error::Error>> {
    let app = App::connect("modal-rust-spawn-map-foreach").await?;

    // `.for_each(recipients)` runs all N across containers, WAITS for them to finish,
    // and returns `()` — the outputs are discarded. Ok(()) means every input ran.
    app.function("notify").for_each(recipients.clone()).await?;
    println!(
        "for_each (live): notified {} recipients across containers, results discarded",
        recipients.len()
    );

    // `.spawn_map(recipients)` enqueues all N and returns a handle IMMEDIATELY, without
    // waiting for the fan-out to finish (fire-and-forget). You can carry on while the
    // work runs in the background.
    let handle = app.function("notify").spawn_map(recipients).await?;
    println!(
        "spawn_map (live): fired fan-out, handle {} (not waiting for results)",
        handle.function_call_id()
    );

    Ok(())
}
