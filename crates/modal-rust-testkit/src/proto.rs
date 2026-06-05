//! The testkit's OWN tonic/prost-generated Modal types + the `ModalClient`
//! *server* trait (`build_server(true)`).
//!
//! These message types are wire-compatible with the SDK's client types: tests
//! construct mock responses with the testkit types and assert on the
//! testkit-recorded request types, while the SDK serializes/deserializes the
//! identical bytes over the loopback channel. Included behind `#![allow(..)]` so
//! `cargo clippy -- -D warnings` stays green on the machine-generated module
//! (mirrors `crates/modal-rust-sdk/src/proto.rs`).

pub mod modal {
    pub mod client {
        #![allow(clippy::all, clippy::pedantic, dead_code)]
        tonic::include_proto!("modal.client");
    }
}

/// Short alias for the generated `modal.client` module: `api::FunctionCreateRequest`,
/// `api::modal_client_server::ModalClient`, etc.
pub use modal::client as api;
