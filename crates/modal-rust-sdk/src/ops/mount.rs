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
use crate::retry::retry_unary;

/// Hosted client-mount deployment name for a given Modal client version
/// (`client_mount_name()`, mount.py:62-69). The Python helper strips any
/// `+githash` suffix; our [`CLIENT_VERSION`] is already clean.
pub fn client_mount_name(version: &str) -> String {
    format!("modal-client-mount-{version}")
}

/// Supported python-build-standalone series → `(release, full_version)`, mirroring
/// `PYTHON_STANDALONE_VERSIONS` (mount.py:44-52). These are the HOSTED, name-resolved
/// python-standalone mounts Modal publishes for `add_python`; the pair encodes which
/// python-build-standalone distribution release supplies the interpreter.
const PYTHON_STANDALONE_VERSIONS: &[(&str, (&str, &str))] = &[
    ("3.10", ("20230826", "3.10.13")),
    ("3.11", ("20230826", "3.11.5")),
    ("3.12", ("20240107", "3.12.1")),
    ("3.13", ("20241008", "3.13.0")),
];

/// Hosted python-standalone mount deployment name for a python `series`
/// (`python_standalone_mount_name()`, mount.py:72-86). Only the glibc (`gnu`) libc
/// is supported (matching the Python client). Returns
/// `python-build-standalone.{release}.{full_version}-gnu`, e.g.
/// `python-build-standalone.20240107.3.12.1-gnu` for `"3.12"`.
///
/// Returns `None` for an unsupported series (mirrors the client's `InvalidError`).
pub fn python_standalone_mount_name(series: &str) -> Option<String> {
    let (release, full) = PYTHON_STANDALONE_VERSIONS
        .iter()
        .find(|(s, _)| *s == series)
        .map(|(_, pair)| *pair)?;
    Some(format!("python-build-standalone.{release}.{full}-gnu"))
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
        let deployment_name = client_mount_name(version);
        self.global_mount_id(&deployment_name, environment)
            .await?
            .ok_or_else(|| {
                Error::build(format!(
                    "hosted client mount '{deployment_name}' resolved to an empty mount_id; \
                     the worker image's modal version may differ from {CLIENT_VERSION} \
                     (fall back to pip install modal via run_commands if needed)"
                ))
            })
    }

    /// Resolve the HOSTED python-build-standalone mount for a python `series` (e.g.
    /// `"3.12"`) and return its `mount_id`, for use as an image build CONTEXT
    /// (`Image.context_mount_id`) so the image's `COPY /python/. /usr/local` drops a
    /// relocatable, NON-PEP-668 `python`/`python3`/`pip` onto `PATH`.
    ///
    /// This is the byte-for-byte analogue of [`ModalClient::client_mount_id`]: a pure
    /// GLOBAL `MountGetOrCreate` lookup (`OBJECT_CREATION_TYPE_UNSPECIFIED`), only the
    /// deployment name differs ([`python_standalone_mount_name`]). It is how the
    /// official Python client provisions Python for `add_python` — no apt, no build
    /// step (mount.py:72-86; `_registry_setup_commands` add_python branch,
    /// _image.py:2041-2059).
    ///
    /// `environment` defaults to the configured environment (or `"main"`).
    pub async fn python_standalone_mount_id(
        &mut self,
        series: &str,
        environment: Option<&str>,
    ) -> Result<String> {
        let deployment_name = python_standalone_mount_name(series).ok_or_else(|| {
            Error::build(format!(
                "unsupported standalone python series {series:?}; supported series: \
                 {:?}",
                PYTHON_STANDALONE_VERSIONS
                    .iter()
                    .map(|(s, _)| *s)
                    .collect::<Vec<_>>()
            ))
        })?;
        self.global_mount_id(&deployment_name, environment)
            .await?
            .ok_or_else(|| {
                Error::build(format!(
                    "hosted python-standalone mount '{deployment_name}' resolved to an \
                     empty mount_id (is the series supported on this server?)"
                ))
            })
    }

    /// Pure GLOBAL `MountGetOrCreate` lookup for a hosted mount by deployment name.
    /// `Ok(None)` when the server returns an empty `mount_id` (the caller maps that
    /// to a descriptive error). UNSPECIFIED creation type ⇒ idempotent, retry-safe.
    async fn global_mount_id(
        &mut self,
        deployment_name: &str,
        environment: Option<&str>,
    ) -> Result<Option<String>> {
        let environment_name = self.env_or_default(environment);
        let req = MountGetOrCreateRequest {
            deployment_name: deployment_name.to_string(),
            namespace: DeploymentNamespace::Global as i32,
            environment_name,
            object_creation_type: ObjectCreationType::Unspecified as i32,
            ..Default::default()
        };
        let stub = self.stub();
        let resp = retry_unary("mount_get_or_create(global)", || {
            let mut stub = stub.clone();
            let req = req.clone();
            async move { Ok(stub.mount_get_or_create(req).await?.into_inner()) }
        })
        .await?;
        if resp.mount_id.is_empty() {
            Ok(None)
        } else {
            Ok(Some(resp.mount_id))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_name_is_version_keyed() {
        assert_eq!(client_mount_name("1.3.2"), "modal-client-mount-1.3.2");
    }

    #[test]
    fn python_standalone_mount_name_matches_client() {
        // mount.py:72-86 → python-build-standalone.{release}.{full}-gnu.
        assert_eq!(
            python_standalone_mount_name("3.12").as_deref(),
            Some("python-build-standalone.20240107.3.12.1-gnu")
        );
        assert_eq!(
            python_standalone_mount_name("3.13").as_deref(),
            Some("python-build-standalone.20241008.3.13.0-gnu")
        );
    }

    #[test]
    fn python_standalone_mount_name_rejects_unsupported_series() {
        assert_eq!(python_standalone_mount_name("3.99"), None);
    }
}
