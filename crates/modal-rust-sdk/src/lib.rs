//! # modal-rust-sdk (`modal_rust::sdk`)
//!
//! A lean, first-party Rust gRPC client that talks to Modal's control plane
//! directly. This crate is the durable foundation for the higher-level
//! `modal_rust` orchestration (programmatic `FunctionCreate`, deploy, and local
//! `.remote()` / `.local()` dispatch) and grows toward full Modal client-SDK
//! compatibility over later milestones.
//!
//! The authenticated transport is built from credential/endpoint resolution
//! ([`config`]), the `x-modal-*` auth interceptor ([`auth`]), the hardened TLS
//! [`channel`], and the CBOR [`codec`]. A typed [`ModalClient`] performs a
//! `ClientHello` handshake on connect and exposes a cheap, safe
//! [`ModalClient::app_get_or_create`] round-trip to prove auth end-to-end.
//!
//! On top of that, the [`ops`] module is a native Rust port of the proven
//! FILE-mode recipe (the "shim-backend" spike), with the three modal-rs bug
//! fixes baked in. The full path is method calls on [`ModalClient`]:
//! [`app_get_or_create_id`](ModalClient::app_get_or_create_id) /
//! [`app_create_ephemeral`](ModalClient::app_create_ephemeral) →
//! [`client_mount_id`](ModalClient::client_mount_id) →
//! [`image_get_or_create`](ModalClient::image_get_or_create) →
//! [`function_precreate`](ModalClient::function_precreate) →
//! [`function_create`](ModalClient::function_create) →
//! [`app_publish`](ModalClient::app_publish) →
//! [`function_from_name`](ModalClient::function_from_name) →
//! [`invoke_cbor`](ModalClient::invoke_cbor). The [`ImageSpec`] / [`FunctionSpec`]
//! builders describe the image and function; [`ModalClient::inner_mut`] remains
//! the escape hatch for any other control-plane RPC.
//!
//! ## Attribution
//!
//! The vendored proto (`proto/api.proto`) is copied verbatim from the official
//! Modal Python SDK, **modal-client**
//! (<https://github.com/modal-labs/modal-client>, Apache-2.0 / MIT). The
//! build-script recipe (tonic-prost-build + protoc-bin-vendored), the
//! auth/channel/interceptor structure, and the CBOR codec follow the unofficial
//! Rust SDK, **modal-rs** (<https://github.com/thehumanworks/modal-rs>, MIT).
//!
//! This crate does **not** depend on either project at build or run time — both
//! are read-only references. See `NOTICE` for the full attribution.

mod proto;

pub mod auth;
pub mod channel;
pub mod client;
pub mod codec;
pub mod config;
pub mod error;
pub mod ops;
pub(crate) mod retry;

/// Generated Modal control-plane protobuf types and the `ModalClient` gRPC stub
/// (`package modal.client`). Re-exported so consumers and later phases can build
/// requests against the canonical Modal API.
pub use proto::modal;

pub use auth::{AuthInterceptor, CLIENT_TYPE_CLIENT, CLIENT_VERSION};
pub use client::{ModalClient, ModalClientStub};
pub use config::{
    read_modal_config, ModalConfig, ModalProfile, DEFAULT_ENVIRONMENT, DEFAULT_SERVER_URL,
};
pub use error::{Error, Result};

// Typed control-plane operation surface (the FILE-mode recipe).
pub use ops::app::PublishedApp;
pub use ops::function::{CreatedFunction, FunctionResources, FunctionSpec, FunctionVolumeMount};
pub use ops::image::ImageSpec;
pub use ops::invoke::Invocation;
pub use ops::local_dir::{WorkspaceClosureSpec, DEFAULT_IGNORE_PATTERNS, DEFAULT_MODALIGNORE_NAME};
pub use ops::mount::{client_mount_name, python_standalone_mount_name};
pub use ops::DEFAULT_BASE_IMAGE;

/// Internal: the pure `build_*_request` functions, re-exported so the facade's
/// offline dry-run/dump (`modal_rust::dump`) can assemble the SAME control-plane
/// requests the live path sends — built ON these identical builders, so the dumped
/// manifest can never drift from the wire. NOT a stable public API; the
/// `build_*_request` functions are the seam the run/deploy ops already call.
#[doc(hidden)]
pub mod planning {
    pub use crate::ops::app::{
        build_app_create_request, build_app_get_or_create_request, build_app_publish_request,
    };
    pub use crate::ops::blob::build_blob_create_request;
    pub use crate::ops::function::{
        build_function_create_request, build_function_get_request, build_function_precreate_request,
    };
    pub use crate::ops::image::build_image_get_or_create_request;
    pub use crate::ops::invoke::{
        build_function_get_outputs_request, build_function_map_request,
        build_function_put_inputs_request,
    };
    pub use crate::ops::local_dir::{
        build_mount_get_or_create_source_request, build_mount_put_file_request,
    };
    pub use crate::ops::mount::build_mount_get_or_create_global_request;
    pub use crate::ops::secret::{build_secret_from_dict_request, build_secret_from_name_request};
    pub use crate::ops::volume::build_volume_get_or_create_request;
}
