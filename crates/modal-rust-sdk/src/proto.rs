//! tonic/prost-generated Modal control-plane types and the `ModalClient` gRPC stub.
//!
//! The generated code is produced at build time from the vendored canonical
//! `proto/api.proto` (`package modal.client`) by `build.rs`. It is included here
//! behind `#![allow(...)]` so `cargo clippy -- -D warnings` stays green on the
//! machine-generated module (verified pattern, mirrors modal-rs `lib.rs`).

pub mod modal {
    pub mod client {
        #![allow(clippy::large_enum_variant, clippy::enum_variant_names, dead_code)]
        tonic::include_proto!("modal.client");
    }
}

/// Short alias for the generated `modal.client` module, used throughout the
/// crate (client, ops) to build requests against the canonical Modal API.
pub(crate) use modal::client as api;
