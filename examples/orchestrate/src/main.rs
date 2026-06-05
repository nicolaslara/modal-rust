//! A runnable tour of the `modal-rust` facade — the real user-facing API.
//!
//! It DEFINES nothing of its own: it reuses the registered `add` handler from
//! `example-add` (in your own project this would be your library crate of
//! `#[modal_rust::function]`s). It then drives that function three ways through the
//! [`modal_rust::App`] / [`modal_rust::Function`] handles:
//!
//! 1. **`.local(..)`** — in-process dispatch through the SAME frozen registry the
//!    runner uses. ZERO Modal, ZERO network. This is the path that runs in
//!    `cargo run -p example-orchestrate` and in the test below; it prints `{sum:42}`.
//!    Shown through BOTH registries: the manual `App::new(modal_registry())` (the
//!    no-macro teaching path) and the macro `App::from_inventory()` — including the
//!    typed positional ergonomics `app.add(2, 3).local()` from the
//!    `#[modal_rust::function]` auto-I/O twin, where no input/output type is named.
//! 2. **`.remote(..).await`** — the RUN path: the crate is uploaded and
//!    `cargo build`-ed IN the Modal function body at invoke time, then the result
//!    comes back typed. Requires Modal credentials.
//! 3. **`App::deploy_with(..)` + `App::call(..)`** — the DEPLOY path: build once at
//!    image-build time into a persistent app, then invoke with no rebuild. Requires
//!    Modal credentials.
//!
//! Auth for the live paths reads `~/.modal.toml` or the `MODAL_TOKEN_ID` /
//! `MODAL_TOKEN_SECRET` env vars directly — there is NO dependency on the `modal`
//! CLI for any of this.
//!
//! By default only the offline `.local()` path runs. Set `RUN_REMOTE=1` (with
//! Modal credentials configured) to also run the live `.remote()` + deploy/call
//! round-trips.

use example_add::{modal_registry, AddInput, AddOutput};
use modal_rust::{App, DeployConfig, Registry};

/// The persistent app name used by the deploy/call demo.
const DEPLOY_APP: &str = "modal-rust-orchestrate-demo";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ----- 1. OFFLINE: `.local()` — the primary, zero-Modal path -----------------
    //
    // Build an App from an explicit Registry (here `example_add::modal_registry()`;
    // with the `#[modal_rust::function]` macro you would call `App::from_inventory()`
    // instead). No network, no credentials, nothing to install.
    let app = App::new(modal_registry());

    // Resolve a Function handle by entrypoint name and run it in-process. The input
    // and output are your real Rust types — `.local()` is `serde_json` in / handler /
    // `serde_json` out, identical to what the remote runner does minus the network.
    let out: AddOutput = app.function("add").local(AddInput { a: 40, b: 2 })?;
    println!("local: add(40, 2) -> {{sum: {}}}", out.sum);
    assert_eq!(out.sum, 42, "the offline .local() path must compute 42");

    // ----- 1b. OFFLINE (MACRO PATH): App::from_inventory() + typed app.fn() -------
    //
    // The SAME offline `.local()` dispatch, but the registry comes from the
    // `#[modal_rust::function]` inventory instead of a hand-written builder. The
    // `add` entrypoint is registered the same way (string-keyed `.function("add")`
    // still works against this registry); the ergonomic surface is the typed positional
    // method generated for the plain-signature `add` — no input/output type is
    // ever named, the args are typed from the signature, and the result decodes to the
    // return type. (The macro generates `add::{Input, Output}` deriving both serde
    // directions, which is what the facade `.local()` callers use.)
    use example_add_macro::AddCall; // the generated typed-method trait
    let macro_app = App::from_inventory();

    // The `#[modal_rust::function]` inventory registers `add` into the
    // SAME `Registry` shape the manual builder produced — proven via the macro crate's
    // re-exported `Registry::from_inventory()` lookup.
    let macro_registry = Registry::from_inventory();
    assert!(
        macro_registry.get("add").is_some(),
        "the #[modal_rust::function] inventory must register `add`"
    );
    println!("local (macro/inventory): registry resolves `add` by name");

    // Auto-I/O ergonomics: typed positional method, result decodes to the return type.
    let plain_sum: i64 = macro_app.add(2, 3).local()?;
    println!("local (macro auto-I/O):  add(2, 3) -> {plain_sum}");
    assert_eq!(
        plain_sum, 5,
        "the typed app.add(2,3).local() path must compute 5"
    );

    // ----- 2 & 3. LIVE: `.remote()` and deploy/call (credential-gated) -----------
    //
    // These hit real Modal, so they only run when explicitly enabled. The code is
    // always compiled (it is the genuine API), it is just not executed by default.
    if std::env::var("RUN_REMOTE").as_deref() == Ok("1") {
        run_remote().await?;
        run_deploy_and_call().await?;
    } else {
        println!(
            "(skipping live .remote()/deploy/call — set RUN_REMOTE=1 with Modal \
             credentials to run them)"
        );
    }

    Ok(())
}

/// The RUN path: `App::connect(..)` then `function(..).remote(input).await`.
///
/// `connect_with_registry` builds a live control-plane client (reading
/// `~/.modal.toml` / `MODAL_TOKEN_*`) and an ephemeral app. The first `.remote()`
/// uploads the crate, builds it IN the function body, runs the real Rust `add`, and
/// returns the typed `AddOutput` — same semantics as `.local()`.
async fn run_remote() -> Result<(), Box<dyn std::error::Error>> {
    // `App::connect("name").await` uses the inventory registry; here we pass an
    // explicit one so the example is self-contained.
    let app = App::connect_with_registry("modal-rust-orchestrate-run", modal_registry()).await?;
    let out: AddOutput = app.function("add").remote(AddInput { a: 40, b: 2 }).await?;
    println!("remote: add(40, 2) -> {{sum: {}}}", out.sum);
    assert_eq!(out.sum, 42);
    Ok(())
}

/// The DEPLOY path: `App::deploy_with(..)` (build at image-build time, persistent)
/// then `App::call(app_name, entrypoint, input).await` (invoke with no rebuild).
async fn run_deploy_and_call() -> Result<(), Box<dyn std::error::Error>> {
    // A connected App is required for both deploy and call. The connection's own app
    // name is throwaway; the deploy publishes persistently under DEPLOY_APP.
    let app = App::connect_with_registry("modal-rust-orchestrate-deploy-driver", modal_registry())
        .await?;

    // DEPLOY: cargo build runs AT image-build time; the deployed runtime execs only
    // the prebuilt /app/modal_runner. Re-deploys REPLACE the named app.
    let deployed = app.deploy_with(DeployConfig::for_app(DEPLOY_APP)).await?;
    println!(
        "deployed app '{}' (image {})",
        deployed.name, deployed.image_id
    );

    // CALL: resolve by name + invoke — no upload, no build.
    let out: AddOutput = app
        .call(DEPLOY_APP, "add", AddInput { a: 40, b: 2 })
        .await?;
    println!("call: add(40, 2) -> {{sum: {}}}", out.sum);
    assert_eq!(out.sum, 42);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The offline `.local()` path is the contract this example guarantees: it runs
    /// the real registered `add` handler in-process and yields `{sum:42}` with no
    /// Modal, no network, no credentials.
    #[test]
    fn local_add_returns_42() {
        let app = App::new(modal_registry());
        let out: AddOutput = app
            .function("add")
            .local(AddInput { a: 40, b: 2 })
            .expect(".local() should run the in-process handler");
        assert_eq!(out.sum, 42);
    }

    /// The MACRO path guarantees the same offline contract via the inventory registry
    /// and the typed positional method — no input/output type named. Guards the
    /// ergonomic surface (`App::from_inventory()` + `app.add(2, 3).local()`).
    #[test]
    fn local_macro_add_returns_5() {
        use example_add_macro::AddCall;
        let app = App::from_inventory();
        let sum: i64 = app
            .add(2, 3)
            .local()
            .expect("the typed macro .local() path should run in-process");
        assert_eq!(sum, 5);
    }
}
