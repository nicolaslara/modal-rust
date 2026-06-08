//! `examples/stateful-class` — load expensive state ONCE, serve it many times.
//!
//! Teaching ONE concept: the load-once stateful class, `#[modal_rust::cls]`. It mirrors
//! Python's `@app.cls` + `@enter` + `@method` line for line, but every token is
//! idiomatic Rust — a struct holds the state, an impl holds the behavior, attributes
//! carry the config.
//!
//! Some workloads pay a large fixed cost before they can do any work: loading model
//! weights, opening a connection pool, warming a cache. A plain `#[function]` would pay
//! that cost on EVERY call. A `#[cls]` pays it ONCE per warm container and reuses the
//! result:
//!
//! - `#[enter] fn load() -> Result<Self>` runs ONCE per warm container (mirrors
//!   `@modal.enter()`). It builds the expensive state and the macro moves it into a
//!   process-lifetime singleton. Here it loads the embedding [`Model`].
//! - each `#[method] fn ..(&self, ..)` (mirrors `@modal.method()`) reads that loaded
//!   state by `&self`. The FIRST method call in a warm container triggers `#[enter]`;
//!   every later call — same or different method — reuses the same in-memory singleton,
//!   so the model is loaded once and served many.
//!
//! The decorator IS the config. Class-level config on `#[cls(gpu = "T4", timeout =
//! 600)]` is the default every method inherits; a per-method `#[method(gpu = "A10G")]`
//! overrides the class default field-by-field. Each method becomes its OWN entrypoint
//! under the dotted `"<Class>.<method>"` name (`Embedder.embed` / `Embedder.dim`) in the
//! `FunctionCreate` manifest, with its fully-resolved (class + method-override) config —
//! proven OFFLINE by `tests/manifest.rs`.
//!
//! Config divergence caveat: two methods with DIFFERENT effective config become
//! DIFFERENT Modal functions, hence different containers. So warm load-once reuse holds
//! across methods with the SAME effective config (the common all-inherit case). Here
//! `embed` (`A10G`) and `dim` (`T4`) do not share a container; `dim` and any other
//! `T4`-inherited method would.
//!
//! The model lives in its own module ([`embedding`]) so this file stays the clean Modal
//! surface — a plain `Embedder` struct plus the `#[cls]` impl that loads and serves it.
//! `src/bin/modal_runner.rs` is the one-line runner; `tests/local.rs` proves OFFLINE
//! that `#[enter]` runs exactly once across many `.local()` calls and that the embedding
//! is real (right width, deterministic, unit-norm); `tests/manifest.rs` proves the
//! dotted per-method entrypoints ride into the planned `FunctionCreate` with the merged
//! config.

mod embedding;

use modal_rust::cls;

use embedding::Model;
pub use embedding::EMBED_DIMENSIONS;
// Re-exported for `tests/local.rs`: lets the offline test PROVE `#[enter]` (which calls
// `Model::load`) ran exactly once across many method calls — the load-once win.
pub use embedding::load_count;

/// The expensive state, loaded ONCE per warm container. A PLAIN struct — no macro on the
/// type. It just holds the loaded embedding [`Model`]; all class-level config rides on
/// the `#[cls(..)]` attribute on the impl block below.
//
// README drift-guard: the region between the begin/end markers below is kept
// byte-identical to the ```rust cls block in README.md (see tests/readme_drift.rs).
// cls:begin
pub struct Embedder {
    model: Model,
}

#[cls(gpu = "T4", timeout = 600)] // CLASS-LEVEL default config -> inherited by every #[method].
impl Embedder {
    /// Runs ONCE per warm container (mirrors `@modal.enter()`). Loads the embedding
    /// model and returns the built value; the macro moves it into a process-lifetime
    /// singleton, so this expensive step happens a single time no matter how many method
    /// calls a warm container serves. (`Model::load` is offline + CPU-only here, but it
    /// stands in for the real "read weights from disk / warm a GPU" cost.)
    #[enter]
    fn load() -> anyhow::Result<Self> {
        Ok(Embedder {
            model: Model::load(),
        })
    }

    /// Embed `text` into a fixed-width unit-length vector, reusing the already-loaded
    /// model by `&self`. `#[method(gpu = "A10G")]` OVERRIDES the class default gpu
    /// (`T4`) for this method only; `timeout = 600` is still inherited from `#[cls]`.
    #[method(gpu = "A10G")]
    fn embed(&self, text: String) -> anyhow::Result<Vec<f32>> {
        Ok(self.model.embed(&text))
    }

    /// Report the model's output dimensionality, reusing the loaded model by `&self`.
    /// Bare `#[method]` — inherits BOTH `gpu = "T4"` and `timeout = 600` from `#[cls]`.
    #[method]
    fn dim(&self) -> anyhow::Result<usize> {
        Ok(self.model.dim())
    }
}
// cls:end
