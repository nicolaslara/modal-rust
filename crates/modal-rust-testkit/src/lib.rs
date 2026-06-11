//! `modal-rust-testkit` — an in-process Modal gRPC mock backend for OFFLINE tests.
//!
//! This crate stands up a real [`tonic`] server implementing the full
//! `ModalClient` gRPC service on a loopback port, so a real
//! [`modal_rust_sdk::ModalClient`] (or the `modal-rust` facade) can be pointed at
//! `http://127.0.0.1:<port>` with **zero transport change** and driven entirely
//! offline — no Modal credentials, no network beyond loopback, no Python.
//!
//! It mirrors the Python `MockClientServicer` pattern natively in Rust: the mock
//! (1) RECORDS every request it receives (typed + queryable from the test via
//! [`MockModal::last`] / [`MockModal::requests`] / [`MockModal::took`]) and
//! (2) returns RESPONSES that tests configure ergonomically — sensible happy-path
//! DEFAULTS for the RPCs a basic deploy/call/remote flow needs, plus per-test
//! OVERRIDES via the [`MockModal::builder`] (canned function results + per-RPC
//! closures).
//!
//! # The 201-method problem (Option A)
//!
//! The `ModalClient` service has 201 RPCs but the SDK only calls ~18. The testkit
//! owns its OWN `build_server(true)` codegen on the same proto and hand-writes the
//! handful the SDK calls; the rest are stubbed as `Status::unimplemented` via the
//! [`mock_unimplemented!`](crate::macros) declarative macro. The testkit's generated
//! message types are wire-compatible with the SDK's client types, so tests construct
//! responses with the testkit types and assert on the testkit-recorded request types.
//!
//! # Quick start
//!
//! ```no_run
//! use modal_rust_testkit::prelude::*;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! // Zero-config happy-path mock on a loopback port.
//! let mock = MockModal::start().await?;
//!
//! // Point a real SDK client at it — no transport change.
//! let mut client = modal_rust_sdk::ModalClient::from_config(mock.modal_config()).await?;
//! let resp = client.app_get_or_create("my-app", None).await?;
//! assert_eq!(resp, "ap-1"); // deterministic canned id
//!
//! // Query what the mock recorded, typed.
//! let app = mock.last::<AppGetOrCreateRequest>().expect("AppGetOrCreate recorded");
//! assert_eq!(app.app_name, "my-app");
//! # Ok(())
//! # }
//! ```
//!
//! Both a plain `#[tokio::test]` and a TABLE test (build a `MockModal` per case in
//! a loop) are supported — each `MockModal` binds its OWN loopback port and owns
//! its OWN request log, so there is no shared global state.

#![forbid(unsafe_code)]

pub(crate) mod macros;
pub(crate) mod proto;

mod builder;
mod record;
mod responder;
mod server;
mod servicer;
mod store;

pub use builder::MockModalBuilder;
pub use record::{FromRecorded, RecordedRequest, RequestLog};
pub use server::MockModal;

/// A tiny prelude: `use modal_rust_testkit::prelude::*;` brings the handle, the
/// builder, the recorded-request enum, and ALL generated Modal message types
/// (e.g. `FunctionCreateRequest`, `FunctionMapRequest`) into scope so tests can
/// name them directly.
pub mod prelude {
    pub use crate::proto::api::*;
    pub use crate::{FromRecorded, MockModal, MockModalBuilder, RecordedRequest, RequestLog};
}
