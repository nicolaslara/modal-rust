//! Client-mount resolution — the Modal-native way to make `modal` importable in
//! a FILE-mode container.
//!
//! FILE-mode containers boot via `python -m modal._container_entrypoint`, so the
//! `modal` client package MUST be importable inside the container. Normal Modal
//! users never `pip install modal` into their image: the Python SDK attaches a
//! hosted, version-keyed **client mount** to every function automatically
//! (`_functions.py:730-734` prepends `_get_client_mount()`; `mount.py` resolves
//! `_Mount.from_name("modal-client-mount-{version}", namespace=GLOBAL)`).
//!
//! We do the same: resolve that hosted mount with `MountGetOrCreate` (lookup-only,
//! GLOBAL namespace) → `mount_id`, then attach it via `Function.mount_ids`
//! ([`crate::ops::function`]). This REPLACES the spike's `pip install modal`
//! shortcut (kept only as a documented fallback in `ops/image.rs`).

use crate::client::ModalClient;
use crate::error::{Error, Result};
use crate::ops::CLIENT_VERSION;
use crate::proto::api::{DeploymentNamespace, MountGetOrCreateRequest, ObjectCreationType};

/// Hosted client-mount deployment name for a given Modal client version
/// (`client_mount_name()`, mount.py:62-69). The Python helper strips any
/// `+githash` suffix; our [`CLIENT_VERSION`] is already clean.
pub fn client_mount_name(version: &str) -> String {
    format!("modal-client-mount-{version}")
}

impl ModalClient {
    /// Resolve the hosted Modal client mount for the configured client version and
    /// return its `mount_id`, for attaching via `Function.mount_ids`.
    ///
    /// Looks up `modal-client-mount-{version}` in the **GLOBAL** deployment
    /// namespace with `OBJECT_CREATION_TYPE_UNSPECIFIED` (pure lookup — Python's
    /// `from_name._load` sets no creation type and never creates this mount).
    ///
    /// `environment` defaults to the configured environment (or `"main"`).
    pub async fn client_mount_id(&mut self, environment: Option<&str>) -> Result<String> {
        self.mount_id_for_version(CLIENT_VERSION, environment).await
    }

    /// Resolve a hosted client mount for an explicit client `version`. Prefer
    /// [`ModalClient::client_mount_id`]; this exists to pin a specific worker
    /// image's `modal` version when it differs from [`CLIENT_VERSION`].
    pub async fn mount_id_for_version(
        &mut self,
        version: &str,
        environment: Option<&str>,
    ) -> Result<String> {
        let environment_name = self.env_or_default(environment);
        let deployment_name = client_mount_name(version);

        let resp = self
            .inner_mut()
            .mount_get_or_create(MountGetOrCreateRequest {
                deployment_name: deployment_name.clone(),
                namespace: DeploymentNamespace::Global as i32,
                environment_name,
                object_creation_type: ObjectCreationType::Unspecified as i32,
                ..Default::default()
            })
            .await?
            .into_inner();

        if resp.mount_id.is_empty() {
            return Err(Error::build(format!(
                "hosted client mount '{deployment_name}' resolved to an empty mount_id; \
                 the worker image's modal version may differ from {CLIENT_VERSION} \
                 (fall back to pip install modal via run_commands if needed)"
            )));
        }
        Ok(resp.mount_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_name_is_version_keyed() {
        assert_eq!(client_mount_name("1.3.2"), "modal-client-mount-1.3.2");
    }
}
