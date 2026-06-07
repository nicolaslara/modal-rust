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
//! let app = App::local_with_registry(modal_registry()); // or App::local()
//! let out: AddOutput = app
//!     .function("add")
//!     .local(AddInput { a: 40, b: 2 })
//!     .unwrap();
//! assert_eq!(out.sum, 42);
//! ```
//!
//! # Using `#[modal_rust::function]` (a single-dep story)
//!
//! The [`function`] attribute is re-exported here so it is spellable as
//! `#[modal_rust::function]` without the `extern crate ... as modal_rust;` alias
//! hack. Its expansion routes every runtime / `inventory` path THROUGH this facade —
//! `::<facade>::__private::runtime::...` and `::<facade>::__private::inventory::...`
//! (the macro resolves `<facade>` via `proc-macro-crate`, honoring a rename) — so a
//! crate using `#[modal_rust::function]` needs ONLY `modal-rust` as its modal
//! dependency (plus `serde`/`anyhow` for the handler types). The macro submits one
//! facade-owned [`Registration`] record that atomically carries both dispatch and
//! control-plane metadata:
//!
//! ```toml
//! modal-rust = { path = "..." }  # facade: App/Function/sdk + `function` + macro deps
//! serde      = { version = "1", features = ["derive"] }
//! anyhow     = "1"
//! ```
//!
//! This mirrors how `serde_derive` routes `::serde::...` through the `serde` facade
//! and `clap_derive` through `clap`, so the user carries one dependency, not three.
//! See [`__private`] for the re-exports the macro names.

// (1) Control-plane SDK, namespaced as `modal_rust::sdk`.
pub use modal_rust_sdk as sdk;

// (2) Runtime essentials that appear in the facade's public API / error paths.
//     Selective — NOT a glob — so `__macro_support`/`codec` stay out of the
//     facade's stable surface.
pub use modal_rust_runtime::{HandlerFn, Registry, RunnerError};
// `typed!` is #[macro_export] at the runtime crate root; re-export it for users who
// build a Registry by hand through the facade.
pub use modal_rust_runtime::typed;

// (3) Proc-macro re-exports. Make `#[modal_rust::function]` and
//     `modal_rust::modal_runner!()` spellable without the alias hack (see the crate
//     docs above for the downstream-dep caveat). `function` is the handler
//     attribute; `modal_runner!()` expands to the runner `main()` (the whole
//     `src/bin/modal_runner.rs` in one line, with NO `__private` in user code).
//     There is NO `app` macro — `modal_rust::App` is a struct.
pub use modal_rust_macros::{function, modal_runner};

/// Macro-support re-exports — NOT a stable public API (hidden from docs).
///
/// `#[modal_rust::function]` expands to facade-routed paths
/// (`::<facade>::__private::runtime::…`, `::<facade>::__private::inventory::…`) so a
/// crate using the macro needs ONLY the `modal-rust` dependency — no direct
/// `modal-rust-runtime` / `inventory`. This mirrors how `serde_derive` routes
/// `::serde::…` through the `serde` facade and `clap_derive` through `clap`. The
/// macro resolves THIS crate's import name via `proc-macro-crate`, so the re-exports
/// resolve even when the facade is renamed (e.g. the `modal_rust_facade` alias the
/// canonical example uses to dodge the `extern crate modal_rust_macros as modal_rust`
/// shadow).
///
/// Items here are an internal contract between the macro and the facade; do not
/// depend on them directly.
#[doc(hidden)]
pub mod __private {
    /// Run the facade-owned inventory path; used by `modal_runner!()`.
    pub use crate::registration::run_cli_from_inventory;
    /// `::inventory`, re-exported so the macro's `inventory::submit!{…}` resolves
    /// through the facade. `submit!` builds a `static` from a path to the facade
    /// [`crate::Registration`] type — both edition-2018+ macro-path resolution and
    /// the type path go through this re-export, so no direct `inventory` dep is
    /// needed.
    pub use inventory;
    /// The frozen runner crate, re-exported as `runtime` so the macro can name
    /// `::<facade>::__private::runtime::typed!`.
    pub use modal_rust_runtime as runtime;
    /// `typed!` is `#[macro_export]`ed at the runtime crate root; re-export it here so
    /// the macro can invoke it through the facade as
    /// `::<facade>::__private::runtime::typed!`.
    pub use modal_rust_runtime::typed;
}

mod app;
mod control_plane;
mod deploy;
mod dump;
mod error;
mod function;
mod registration;
mod remote;
mod runner_gen;
mod scope;

pub use app::App;
pub use deploy::{DeployConfig, DeployedApp, DEFAULT_DEPLOY_APP};
// The additive, network-free dry-run / dump (the deferred P8). New types/methods —
// the facade's run/deploy/call public signatures are unchanged.
pub use dump::{Manifest, MountRole, PlannedRequest, RunMode};
pub use error::{Error, Result};
pub use function::{Function, FunctionCall, TypedCall, TypedFunctionCall};
pub use registration::{
    from_inventory_with_configs, package_from_inventory, registry_from_inventory,
    run_cli_from_inventory, run_cli_with_args_and_configs, run_cli_with_args_from_inventory,
    FunctionConfig, FunctionOptions, Registration,
};
// The RUN-path config the `modal-rust` CLI builds from `--describe` + workspace root
// (P9). `App::connect_from_manifest` takes it explicitly.
pub use remote::{ImageStep, RemoteConfig};
// Tooling-generated `modal_runner` (inject-bin, design B). The `modal-rust` CLI needs
// this for the `--describe` LOCAL build: auto-detect whether the target ships its own
// `modal_runner` bin, and if not, materialize a temp SHADOW copy with the generated
// runner and build there (never mutating the user's tree). Run/deploy inject inside
// the source upload (no CLI involvement). Single source of truth — no second
// `cargo metadata` call, no logic drift.
pub use runner_gen::{
    injected_runner_rel_path, materialize_shadow, render_runner_main, resolve_runner_target,
    RunnerTarget,
};

/// Shared lock serializing the unit tests that mutate the SHARED process env
/// (`MODAL_RUST_*`). `cargo test` runs a binary's tests in parallel threads, so two
/// tests touching `MODAL_RUST_INSTALL_RUST` / `MODAL_RUST_BASE_IMAGE` (a writer in
/// `remote::tests` and a reader in `deploy::tests`) would otherwise race. Each such
/// test takes this lock for the duration of its env reads/writes. `std::sync::Mutex`
/// (no extra dep); poisoning is fine — a panicked test already failed.
#[cfg(test)]
pub(crate) static ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
