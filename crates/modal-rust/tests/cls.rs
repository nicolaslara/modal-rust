//! End-to-end proof of the `#[modal_rust::cls]` macro (Cls v0, Shape A / Shape 1).
//!
//! This is the real macro-expansion test: it AUTHORS a `#[cls]` class exactly as a
//! user would (a plain struct + `#[cls(..)] impl` with `#[enter]`/`#[method]`), then
//! proves the GENERATED code:
//!   1. compiles and produces the `<Class>Handle` + `<Class>Cls` extension trait,
//!   2. `.local()` dispatches through the frozen Registry and returns the typed output,
//!   3. `#[enter]` runs EXACTLY ONCE across many `.local()` calls (the load-once win),
//!   4. submits per-method `Registration`s under the dotted `"<Class>.<method>"` names
//!      with the merged (class-default + method-override) `FunctionConfig` (proven via
//!      the public `--describe` manifest).
//!
//! Offline, CPU-only, zero Modal — runs in the normal `cargo test`.

use std::sync::atomic::{AtomicUsize, Ordering};

use modal_rust::cls;
use modal_rust::App;
// The macro emits the `EmbedderCls` extension trait (which carries `app.embedder()`) at
// THIS module's scope, so it is already in scope for the call sites below. A real user
// authoring in a library crate brings it in with one glob: `use my_crate::*;`.

// A test-visible load counter: the `#[enter]` body bumps it, so we can assert it ran
// exactly once across calls (the load-once proof). Process-global because the generated
// singleton is process-global.
static LOAD_COUNT: AtomicUsize = AtomicUsize::new(0);

/// The expensive state — a plain struct, no macro on the type.
pub struct Embedder {
    dim: usize,
}

#[cls(gpu = "T4", timeout = 600)]
impl Embedder {
    /// Runs ONCE per process (here, per warm container): bump the counter and build.
    #[enter]
    fn load() -> anyhow::Result<Self> {
        LOAD_COUNT.fetch_add(1, Ordering::SeqCst);
        Ok(Embedder { dim: 8 })
    }

    /// A real (tiny) embedding: a deterministic char-sum vector of fixed dim.
    #[method(gpu = "A10G")]
    fn embed(&self, text: String) -> anyhow::Result<Vec<f32>> {
        let mut v = vec![0.0f32; self.dim];
        for (i, b) in text.bytes().enumerate() {
            v[i % self.dim] += b as f32;
        }
        Ok(v)
    }

    /// Inherits gpu="T4", timeout=600 from `#[cls]` (no override).
    #[method]
    fn dim(&self) -> anyhow::Result<usize> {
        Ok(self.dim)
    }
}

#[test]
fn cls_local_round_trip_and_load_once() {
    LOAD_COUNT.store(0, Ordering::SeqCst);
    let app = App::local();

    // The generated handle: app.embedder() -> EmbedderHandle, then .embed(..)/.dim().
    let d: usize = app.embedder().dim().local().unwrap();
    assert_eq!(d, 8);

    let v: Vec<f32> = app.embedder().embed("hi".into()).local().unwrap();
    assert_eq!(v.len(), 8);
    // "hi" = bytes [104, 105] -> v[0]=104, v[1]=105, rest 0.
    assert_eq!(v[0], 104.0);
    assert_eq!(v[1], 105.0);

    // A second embed call reuses the SAME singleton — #[enter] does NOT run again.
    let _ = app.embedder().embed("xyz".into()).local().unwrap();

    assert_eq!(
        LOAD_COUNT.load(Ordering::SeqCst),
        1,
        "#[enter] (the OnceLock init) ran exactly once across all .local() calls"
    );
}

#[test]
fn cls_registers_dotted_entrypoints_with_merged_config() {
    // Drive the public `--describe` manifest path (the same one the CLI uses): it lists
    // every inventory entrypoint + its resolved FunctionOptions. This is the public,
    // network-free way to inspect the macro's per-method registrations + merged config.
    let argv = vec!["--describe".to_string()];
    let mut buf = Vec::new();
    let code = modal_rust::run_cli_with_args_from_inventory(&argv, &mut buf);
    assert_eq!(code, 0);
    let manifest: serde_json::Value = serde_json::from_slice(&buf).expect("describe manifest");
    let eps = manifest["entrypoints"]
        .as_array()
        .expect("entrypoints array");

    let find = |name: &str| -> serde_json::Value {
        eps.iter()
            .find(|e| e["name"] == name)
            .unwrap_or_else(|| panic!("entrypoint {name:?} not found in {eps:?}"))
            .clone()
    };

    // Each #[method] is its OWN entrypoint under the dotted "<Class>.<method>" name.
    let embed = find("Embedder.embed");
    let dim = find("Embedder.dim");

    // `embed` overrode gpu to A10G; timeout is inherited from the class (#[cls]).
    assert_eq!(embed["config"]["gpu"], "A10G");
    assert_eq!(embed["config"]["timeout_secs"], 600);

    // `dim` inherits BOTH gpu=T4 and timeout=600 from the class.
    assert_eq!(dim["config"]["gpu"], "T4");
    assert_eq!(dim["config"]["timeout_secs"], 600);
}

#[test]
fn cls_check_input_validates_locally_through_the_macro() {
    // End-to-end proof of fix A through the REAL macro/inventory path: the `#[method]`
    // macro emits a `typed_check!` companion, so the runner's `--check-input` mode can
    // DECODE-ONLY validate `Embedder.embed`'s `{ text: String }` input WITHOUT running
    // the method body — exactly what the CLI invokes to fail fast before any Modal call.
    let run = |entrypoint: &str, input: &str| -> (serde_json::Value, i32) {
        let argv = vec![
            "--check-input".to_string(),
            "--entrypoint".to_string(),
            entrypoint.to_string(),
            "--input-json".to_string(),
            input.to_string(),
        ];
        let mut buf = Vec::new();
        let code = modal_rust::run_cli_with_args_from_inventory(&argv, &mut buf);
        let v: serde_json::Value = serde_json::from_slice(&buf).expect("check envelope");
        (v, code)
    };

    // Good input (the right shape) decodes → exit 0, ok:true.
    let (ok, code) = run("Embedder.embed", r#"{"text":"the quick brown fox"}"#);
    assert_eq!(code, 0);
    assert_eq!(ok["ok"], true);

    // Bad input (missing `text`) fails locally → exit 1, decode_error naming the field.
    let (bad, code) = run("Embedder.embed", r#"{"nope":1}"#);
    assert_eq!(code, 1);
    assert_eq!(bad["error"]["kind"], "decode_error");
    assert!(
        bad["error"]["message"].as_str().unwrap().contains("text"),
        "decode error names the missing field: {bad}"
    );

    // An unknown entrypoint fails fast locally too (a typo never reaches Modal).
    let (unknown, code) = run("Embedder.nope", r#"{"text":"x"}"#);
    assert_eq!(code, 1);
    assert_eq!(unknown["error"]["kind"], "unknown_entrypoint");
}

#[tokio::test]
async fn cls_remote_on_offline_app_is_not_connected() {
    // `.remote()` on a method needs a connected App, exactly like a free fn: an offline
    // App errors WITHOUT any network call (the client-gated surface).
    let app = App::local();
    let err = app
        .embedder()
        .dim()
        .remote()
        .await
        .expect_err("remote on offline app must error");
    // Light build: client_feature stub (NotImplemented). With `client`: NotConnected.
    let msg = err.to_string();
    assert!(
        msg.contains("client") || msg.contains("connect"),
        "unexpected error: {msg}"
    );
}
