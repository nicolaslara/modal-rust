//! `modal_rust::Volume` — the typed facade handle over modal.Volume (put path).
//!
//! A Volume is a named, server-side persistent filesystem shared across
//! functions and clients (Python parity: `modal.Volume`). Unlike
//! [`Dict`](crate::Dict) / [`Queue`](crate::Queue), a Volume is typically
//! write-once / read-many: this v0 facade exposes the client-side **upload**
//! surface (`modal volume put`), which is the part a Rust program drives
//! programmatically. Reading happens by mounting the Volume into a function.
//!
//! Like Dict/Queue, the handle owns its own [`sdk::ModalClient`] behind an
//! `Arc<Mutex<…>>` (the same client-ownership shape as `App`'s `RemoteHandle`),
//! and clones share that client.
//!
//! The actual byte transfer uses the V2 block-based protocol
//! (`VolumePutFiles2`): files are split into 8 MiB content-addressed blocks
//! streamed straight to object storage, so multi-GB weights never need to be
//! held in memory at once. The Volume is therefore resolved as a **V2** volume.

use std::path::Path;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::error::Result;
use crate::sdk;
use sdk::ops::volume::{plan_upload, VolumePutStats};

/// A handle to a named modal.Volume. `Clone` is cheap (clones share the
/// underlying client). See the [module docs](self) for the upload model.
#[derive(Clone)]
pub struct Volume {
    /// Owned control-plane client, the same `Arc<Mutex<…>>` shape as `App`'s
    /// `RemoteHandle`: interior mutability + single-flighting concurrent calls
    /// from clones.
    client: Arc<Mutex<sdk::ModalClient>>,
    /// Resolved server-side volume id (`VolumeGetOrCreate`).
    volume_id: String,
    /// The deployment name the handle was resolved from (diagnostics only).
    name: String,
}

impl std::fmt::Debug for Volume {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Volume")
            .field("name", &self.name)
            .field("volume_id", &self.volume_id)
            .finish_non_exhaustive()
    }
}

impl Volume {
    /// Resolve the named Volume, **creating it if missing** (`CREATE_IF_MISSING`
    /// — idempotent and retry-safe), as a V2 volume. Uses the configured default
    /// environment. Mirrors Python's `modal.Volume.from_name(create_if_missing=True)`.
    pub async fn from_name(name: &str) -> Result<Self> {
        let mut client = sdk::ModalClient::connect().await?;
        let volume_id = client.volume_get_or_create(name, true, true, None).await?;
        Ok(Self::wrap(client, name, volume_id))
    }

    /// Pure lookup of an EXISTING named Volume: the server's not-found error
    /// surfaces as `Err` instead of creating the object.
    pub async fn lookup(name: &str) -> Result<Self> {
        let mut client = sdk::ModalClient::connect().await?;
        let volume_id = client.volume_get_or_create(name, true, false, None).await?;
        Ok(Self::wrap(client, name, volume_id))
    }

    /// As [`from_name`](Volume::from_name), but in an explicit Modal environment.
    pub async fn from_name_in(name: &str, environment: &str) -> Result<Self> {
        let mut client = sdk::ModalClient::connect().await?;
        let volume_id = client
            .volume_get_or_create(name, true, true, Some(environment))
            .await?;
        Ok(Self::wrap(client, name, volume_id))
    }

    /// TEST-ONLY: resolve at an explicit `server_url` (an in-process mock) with
    /// dummy credentials, mirroring [`Dict::from_name_at`](crate::Dict). Gated
    /// behind the `testkit` feature; NOT shipped public API.
    #[cfg(any(test, feature = "testkit"))]
    pub async fn from_name_at(name: &str, server_url: String) -> Result<Self> {
        let mut client = crate::dict::mock_client(server_url).await?;
        let volume_id = client.volume_get_or_create(name, true, true, None).await?;
        Ok(Self::wrap(client, name, volume_id))
    }

    fn wrap(client: sdk::ModalClient, name: &str, volume_id: String) -> Self {
        Volume {
            client: Arc::new(Mutex::new(client)),
            volume_id,
            name: name.to_string(),
        }
    }

    /// The deployment name this handle was resolved from.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The resolved server-side volume id.
    pub fn volume_id(&self) -> &str {
        &self.volume_id
    }

    /// Upload a LOCAL file or directory into the Volume.
    ///
    /// - `local_path`: a file or a directory (uploaded recursively).
    /// - `remote_path`: the destination inside the Volume. For a file, a trailing
    ///   `/` (or an empty string) means "into this directory by basename"; for a
    ///   directory it is the destination prefix. See
    ///   [`plan_upload`](sdk::ops::volume::plan_upload) for the exact mapping.
    /// - `force`: when `false`, the server rejects overwriting an existing remote
    ///   file (matching `modal volume put`); `true` overwrites.
    ///
    /// Returns the upload [`VolumePutStats`] (files, bytes, blocks uploaded).
    pub async fn put(
        &self,
        local_path: &Path,
        remote_path: &str,
        force: bool,
    ) -> Result<VolumePutStats> {
        let files = plan_upload(local_path, remote_path)?;
        let stats = self
            .client
            .lock()
            .await
            .volume_put(&self.volume_id, &files, force, |_, _| {})
            .await?;
        Ok(stats)
    }
}
