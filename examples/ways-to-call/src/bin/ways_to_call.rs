//! One function, four ways to call it.
//!
//! The single `#[modal_rust::function] fn square(n)` from this crate's `lib.rs` is
//! invoked through the SAME typed `app.square(n)` method four ways:
//!
//! 1. `.local()`                 — in-process dispatch, ZERO Modal, ZERO network.
//! 2. `.remote().await`          — one call on Modal (upload + in-body build + run).
//! 3. `.spawn().await` + `.get()` — fire-and-forget, then poll for the result.
//! 4. `.map([..]).await`         — fan-out over many inputs, results in input order.
//!
//! Only shape 1 runs by default (`cargo run -p example-ways-to-call --bin
//! ways_to_call`): it needs nothing — no credentials, no network. Shapes 2–4 hit
//! real Modal, so they are compiled always (they are the genuine API) but run only
//! when `RUN_REMOTE=1` is set with Modal credentials configured.
//!
//! The typed `app.square(n)` method comes from the macro (one `use` brings it in:
//! `use example_ways_to_call::SquareCall;`); no input/output type is ever named.

use example_ways_to_call::SquareCall;
use modal_rust::App;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ----- 1. .local() — the primary, zero-Modal path -----------------------------
    //
    // `App::local()` builds an in-process app from the `#[modal_rust::function]`
    // inventory. `app.square(6).local()?` runs the real handler in-process and
    // decodes to the return type — no credentials, no network, nothing to install.
    let app = App::local();
    let squared: i64 = app.square(6).local()?;
    println!("local:  square(6) -> {squared}");
    assert_eq!(squared, 36, "the offline .local() path must compute 36");

    // ----- 2, 3 & 4. the live shapes (credential-gated) ---------------------------
    //
    // These hit real Modal, so they only run when explicitly enabled. The code is
    // always compiled (it is the genuine API), it is just not executed by default.
    if std::env::var("RUN_REMOTE").as_deref() == Ok("1") {
        run_live_shapes().await?;
    } else {
        println!(
            "(skipping live .remote()/.spawn()/.map() — set RUN_REMOTE=1 with Modal \
             credentials to run them)"
        );
    }

    Ok(())
}

/// Shapes 2–4 against a connected App. `App::connect("name").await` builds a live
/// control-plane client (reading `~/.modal.toml` / `MODAL_TOKEN_*`) and uses the
/// inventory registry, so the SAME typed `app.square(n)` method drives every shape.
async fn run_live_shapes() -> Result<(), Box<dyn std::error::Error>> {
    let app = App::connect("modal-rust-ways-to-call").await?;

    // 2. `.remote().await` — one call: upload the crate, build it IN the Modal
    //    function body at invoke time, run `square`, and return the typed result.
    let one: i64 = app.square(6).remote().await?;
    println!("remote: square(6) -> {one}");
    assert_eq!(one, 36);

    // 3. `.spawn().await` + `.get(..)` — fire-and-forget: `.spawn()` returns a handle
    //    immediately; `.get(None)` awaits that handle's single result later.
    let call = app.square(7).spawn().await?;
    let spawned: i64 = call.get(None).await?;
    println!("spawn:  square(7) -> {spawned}");
    assert_eq!(spawned, 49);

    // 4. `.map([..]).await` — fan-out: the leading arg only fixes the entrypoint +
    //    types; `.map(..)` runs the supplied inputs and returns `Vec<Out>` in input
    //    order. Each item is the function's named input (`square::Input { n }`, the
    //    one field per parameter the macro generates); for a one-arg fn that is just
    //    the value wrapped in `n`.
    use example_ways_to_call::square;
    let inputs = [2, 3, 4].map(|n| square::Input { n });
    let many: Vec<i64> = app.square(0).map(inputs).await?;
    println!("map:    square([2, 3, 4]) -> {many:?}");
    assert_eq!(many, vec![4, 9, 16]);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The offline `.local()` shape is the contract this example guarantees: it runs
    /// the real `square` handler in-process and yields 36 with no Modal, no network,
    /// no credentials — through the typed `app.square(n)` method, no I/O type named.
    #[test]
    fn local_square_returns_36() {
        let app = App::local();
        let squared: i64 = app
            .square(6)
            .local()
            .expect("the typed .local() path should run in-process");
        assert_eq!(squared, 36);
    }
}
