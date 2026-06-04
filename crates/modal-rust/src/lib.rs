//! modal-rust: the user-facing facade. One dependency for App/Function + sdk.
//!
//! - [`Function::local`] runs the registered handler IN-PROCESS via the frozen
//!   [`Registry`] (zero Modal, zero network) and returns the typed output. This
//!   mirrors Modal Python's `Function.local()` = `raw_f(*args)`.
//! - [`Function::remote`] runs the handler REMOTELY on Modal (the RUN path): the
//!   source crate is uploaded as a mount, `cargo build` runs IN THE FUNCTION BODY
//!   at invoke time, and the freshly built `modal_runner` execs the handler —
//!   returning the SAME typed `Result` as `.local()`. Requires
//!   [`App::connect`](crate::App::connect).
//! - [`Function::spawn`] fire-and-forget enqueues one input on Modal and returns a
//!   [`FunctionCall`] handle immediately; [`FunctionCall::get`] fetches the result
//!   later. [`Function::map`] fans out N inputs and collects the outputs in INPUT
//!   ORDER. Both drive the SAME RUN path as `.remote()` and require
//!   [`App::connect`](crate::App::connect).
//!
//! # Quick start (single-dep App/Function path)
//!
//! ```rust
//! use modal_rust::App;
//! use example_add::{modal_registry, AddInput, AddOutput};
//!
//! let app = App::new(modal_registry()); // or App::from_inventory()
//! let out: AddOutput = app
//!     .function("add")
//!     .local(AddInput { a: 40, b: 2 })
//!     .unwrap();
//! assert_eq!(out.sum, 42);
//! ```
//!
//! # Using `#[modal_rust::function]` (NOT a single-dep story)
//!
//! The [`function`] attribute is re-exported here so it is spellable as
//! `#[modal_rust::function]` without the `extern crate ... as modal_rust;` alias
//! hack. BUT its expansion emits absolute `::modal_rust_runtime::...` and
//! `::inventory::submit!` paths, which Rust resolves against the *downstream
//! crate's own* extern prelude — a `pub use` from this facade does not inject those
//! crates. So a crate using `#[modal_rust::function]` must still add three direct
//! deps of its own:
//!
//! ```toml
//! modal-rust         = { path = "..." }  # facade: App/Function/sdk + `function`
//! modal-rust-runtime = { path = "..." }  # macro expands to ::modal_rust_runtime
//! inventory          = "0.3"             # macro expands to ::inventory::submit!
//! ```
//!
//! The frozen macro is intentionally left unchanged; making this zero-extra-dep
//! would require editing the macro's expansion and would break `examples/add-macro`.

// (1) Control-plane SDK, namespaced as `modal_rust::sdk`.
pub use modal_rust_sdk as sdk;

// (2) Runtime essentials that appear in the facade's public API / error paths.
//     Selective — NOT a glob — so `__macro_support`/`codec` stay out of the
//     facade's stable surface.
pub use modal_rust_runtime::{FunctionConfig, HandlerFn, Registration, Registry, RunnerError};
// `typed!` is #[macro_export] at the runtime crate root; re-export it for users who
// build a Registry by hand through the facade.
pub use modal_rust_runtime::typed;

// (3) Proc-macro re-export. Makes `#[modal_rust::function]` spellable without the
//     alias hack (see the crate docs above for the downstream-dep caveat). Only
//     `function` exists; there is NO `app` macro — `modal_rust::App` is a struct.
pub use modal_rust_macros::function;

mod app;
mod deploy;
mod error;
mod function;
mod remote;
mod scope;

pub use app::App;
pub use deploy::{DeployConfig, DeployedApp};
pub use error::{Error, Result};
pub use function::{Function, FunctionCall};
// The RUN-path config the `modal-rust` CLI builds from `--describe` + workspace root
// (P9). `App::connect_from_manifest` takes it explicitly.
pub use remote::RemoteConfig;

/// Shared lock serializing the unit tests that mutate the SHARED process env
/// (`MODAL_RUST_*`). `cargo test` runs a binary's tests in parallel threads, so two
/// tests touching `MODAL_RUST_INSTALL_RUST` / `MODAL_RUST_BASE_IMAGE` (a writer in
/// `remote::tests` and a reader in `deploy::tests`) would otherwise race. Each such
/// test takes this lock for the duration of its env reads/writes. `std::sync::Mutex`
/// (no extra dep); poisoning is fine — a panicked test already failed.
#[cfg(test)]
pub(crate) static ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
