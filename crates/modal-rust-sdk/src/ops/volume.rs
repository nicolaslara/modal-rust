//! Volume resolution — `VolumeGetOrCreate`.
//!
//! Used by the P6 cargo build cache: resolve a persistent V2 volume by name
//! (create-if-missing) and return its `volume_id`, for attaching to a function
//! via `FunctionSpec.volume_mounts` -> `Function.volume_mounts`.
//!
//! Mirrors `Volume.from_name(..., create_if_missing, version)` (volume.py):
//! sets ONLY deployment_name, environment_name, object_creation_type, version.
//! Leaves the reserved `namespace` (not generated) and `app_id` UNSET.

use crate::client::ModalClient;
use crate::error::{Error, Result};
use crate::proto::api::{ObjectCreationType, VolumeFsVersion, VolumeGetOrCreateRequest};
use crate::retry::retry_unary;

/// Build the `VolumeGetOrCreate` request — pure, no I/O. Mirrors
/// `Volume.from_name(..., create_if_missing, version)`: sets ONLY `deployment_name`,
/// `environment_name`, `object_creation_type`, and `version`.
///
/// - `v2 = true` ⇒ `VolumeFsVersion::V2` (concurrent writes — the cargo cache);
///   `false` ⇒ `Unspecified` (server default == V1; the user-volume path).
/// - `create_if_missing = true` ⇒ `CreateIfMissing` (idempotent); `false` ⇒
///   `Unspecified` (pure lookup).
///
/// Extracted from [`ModalClient::volume_get_or_create`]; the method passes the
/// resolved `environment_name`.
pub fn build_volume_get_or_create_request(
    name: &str,
    v2: bool,
    create_if_missing: bool,
    environment_name: String,
) -> VolumeGetOrCreateRequest {
    let version = if v2 {
        VolumeFsVersion::V2 as i32
    } else {
        VolumeFsVersion::Unspecified as i32 // == Python version=None
    };
    let object_creation_type = if create_if_missing {
        ObjectCreationType::CreateIfMissing as i32
    } else {
        ObjectCreationType::Unspecified as i32 // pure lookup
    };
    VolumeGetOrCreateRequest {
        deployment_name: name.to_string(),
        environment_name,
        object_creation_type,
        version,
        ..Default::default() // app_id empty; reserved namespace never set
    }
}

impl ModalClient {
    /// Resolve a persistent Volume by deployment name, creating it if missing,
    /// and return its `volume_id`.
    ///
    /// - `name`: deployment name (e.g. `"modal-rust-cargo-cache"`).
    /// - `v2`: `true` => `VolumeFsVersion::V2` (concurrent writes — required for
    ///   the cargo cache); `false` => `VolumeFsVersion::Unspecified` (server
    ///   default == V1; matches Python `version=None`).
    /// - `create_if_missing`: `true` => `ObjectCreationType::CreateIfMissing`
    ///   (idempotent, retry-safe); `false` => `Unspecified` (pure lookup).
    /// - `environment`: defaults to the configured environment (or `"main"`).
    pub async fn volume_get_or_create(
        &mut self,
        name: &str,
        v2: bool,
        create_if_missing: bool,
        environment: Option<&str>,
    ) -> Result<String> {
        let environment_name = self.env_or_default(environment);
        let req = build_volume_get_or_create_request(name, v2, create_if_missing, environment_name);
        let stub = self.stub();
        // CREATE_IF_MISSING is idempotent server-side, so a retry after a dropped
        // response re-resolves the same volume_id (mirrors mount_get_or_create).
        let resp = retry_unary("volume_get_or_create", || {
            let mut stub = stub.clone();
            let req = req.clone();
            async move { Ok(stub.volume_get_or_create(req).await?.into_inner()) }
        })
        .await?;

        if resp.volume_id.is_empty() {
            return Err(Error::build(format!(
                "VolumeGetOrCreate for '{name}' returned an empty volume_id"
            )));
        }
        Ok(resp.volume_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::api::{ObjectCreationType, VolumeFsVersion};

    #[test]
    fn version_flag_maps_to_fs_version() {
        assert_eq!(VolumeFsVersion::V2 as i32, 2);
        assert_eq!(VolumeFsVersion::Unspecified as i32, 0);
    }

    #[test]
    fn create_flag_maps_to_creation_type() {
        assert_eq!(ObjectCreationType::CreateIfMissing as i32, 1);
        assert_eq!(ObjectCreationType::Unspecified as i32, 0);
    }

    #[test]
    fn build_volume_get_or_create_v2_create() {
        // The cargo-cache path: V2 + create-if-missing.
        let req = build_volume_get_or_create_request(
            "modal-rust-cargo-cache",
            true,
            true,
            "main".to_string(),
        );
        assert_eq!(req.deployment_name, "modal-rust-cargo-cache");
        assert_eq!(req.environment_name, "main");
        assert_eq!(req.version, VolumeFsVersion::V2 as i32);
        assert_eq!(
            req.object_creation_type,
            ObjectCreationType::CreateIfMissing as i32
        );
    }

    #[test]
    fn build_volume_get_or_create_v1_create() {
        // The user-volume path: V1 (Unspecified == server default) + create.
        let req = build_volume_get_or_create_request("my-vol", false, true, "main".to_string());
        assert_eq!(req.version, VolumeFsVersion::Unspecified as i32);
        assert_eq!(
            req.object_creation_type,
            ObjectCreationType::CreateIfMissing as i32
        );
    }
}
