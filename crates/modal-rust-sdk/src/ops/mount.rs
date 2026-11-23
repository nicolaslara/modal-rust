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

/// Build the GLOBAL `MountGetOrCreate` lookup request — pure, no I/O. Looks up a
/// hosted mount by deployment name in the GLOBAL namespace with
/// `OBJECT_CREATION_TYPE_UNSPECIFIED` (pure lookup — Python's `from_name._load`
/// neither sets a creation type nor creates this mount).
///
/// Covers the hosted client mount and the python-standalone mount. Extracted from
/// [`ModalClient::global_mount_id`]; the method passes the resolved
/// `environment_name`.
pub(crate) fn build_mount_get_or_create_global_request(
    deployment_name: &str,
    environment_name: String,
) -> MountGetOrCreateRequest {
    MountGetOrCreateRequest {
        deployment_name: deployment_name.to_string(),
        namespace: DeploymentNamespace::Global as i32,
        environment_name,
        object_creation_type: ObjectCreationType::Unspecified as i32,
        ..Default::default()
    }
}

/// Hosted client-mount deployment name for a given Modal client version
/// (`client_mount_name()`, mount.py:62-69). The Python helper strips any
/// `+githash` suffix; our [`CLIENT_VERSION`] is already clean.
pub fn client_mount_name(version: &str) -> String {
    format!("modal-client-mount-{version}")
}

/// One python-build-standalone series → `(release, full_version)`.
type StandaloneVersions = &'static [(&'static str, (&'static str, &'static str))];

/// Series published for image builder versions up to and including `2025.06`,
/// mirroring `_PYTHON_STANDALONE_VERSIONS_2025_06` (mount.py).
const PYTHON_STANDALONE_VERSIONS_2025_06: StandaloneVersions = &[
    ("3.10", ("20230826", "3.10.13")),
    ("3.11", ("20230826", "3.11.5")),
    ("3.12", ("20240107", "3.12.1")),
    ("3.13", ("20241008", "3.13.0")),
    ("3.14", ("20251205", "3.14.2")),
    ("3.14t", ("20251209", "3.14.2t")),
    ("3.15", ("20261009", "3.15.0")),
    ("3.15t", ("20261009", "3.15.0t")),
];

/// Series published for image builder versions `2026.10` and `PREVIEW`, mirroring
/// `PYTHON_STANDALONE_VERSIONS["2026.10"]` (mount.py).
const PYTHON_STANDALONE_VERSIONS_2026_10: StandaloneVersions = &[
    ("3.11", ("20261021", "3.11.17")),
    ("3.12", ("20261021", "3.12.15")),
    ("3.13", ("20261021", "3.13.17")),
    ("3.14", ("20261021", "3.14.9")),
    ("3.14t", ("20261021", "3.14.9t")),
    ("3.15", ("20261021", "3.15.1")),
    ("3.15t", ("20261021", "3.15.1t")),
];

/// First python-build-standalone release whose tree ships a `python` binary;
/// earlier releases ship only `python3` (`_registry_setup_commands`, _image.py).
const FIRST_RELEASE_WITH_PYTHON_BINARY: &str = "20241008";

/// The standalone table for an image builder version, mirroring
/// `PYTHON_STANDALONE_VERSIONS[builder_version]` (mount.py). A version this SDK
/// does not name (including the empty "let the server choose") gets the `2025.06`
/// table, whose mounts every builder version can consume.
fn standalone_versions(builder_version: &str) -> StandaloneVersions {
    match builder_version {
        "2026.10" | "PREVIEW" => PYTHON_STANDALONE_VERSIONS_2026_10,
        _ => PYTHON_STANDALONE_VERSIONS_2025_06,
    }
}

/// The `(release, full_version)` python-build-standalone distribution that supplies
/// a python `series` under `builder_version`, or `None` for an unsupported series.
pub(crate) fn python_standalone_release(
    series: &str,
    builder_version: &str,
) -> Option<(&'static str, &'static str)> {
    standalone_versions(builder_version)
        .iter()
        .find(|(s, _)| *s == series)
        .map(|(_, pair)| *pair)
}

/// Whether the standalone tree for `series` under `builder_version` lacks a `python`
/// binary, so the image must symlink `python` → `python3` (the bare `python` the
/// container entrypoint execs). `false` for an unsupported series.
pub(crate) fn python_standalone_needs_python_symlink(series: &str, builder_version: &str) -> bool {
    python_standalone_release(series, builder_version)
        .is_some_and(|(release, _)| release < FIRST_RELEASE_WITH_PYTHON_BINARY)
}

/// Hosted python-standalone mount deployment name for a python `series` under an
/// image `builder_version` (`python_standalone_mount_name()`, mount.py). Only the
/// glibc (`gnu`) libc on x86_64 is supported. Returns
/// `python-build-standalone.{release}.{full_version}-gnu`, e.g.
/// `python-build-standalone.20261021.3.12.15-gnu` for `"3.12"` under `"2026.10"`.
///
/// Returns `None` for an unsupported series (mirrors the client's `InvalidError`).
pub fn python_standalone_mount_name(series: &str, builder_version: &str) -> Option<String> {
    let (release, full) = python_standalone_release(series, builder_version)?;
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
    /// The distribution is the one the resolved image builder version uses (the same
    /// version [`ModalClient::image_get_or_create`] sends), so the mount and the
    /// image's dockerfile commands agree on which standalone release is installed.
    ///
    /// `environment` defaults to the configured environment (or `"main"`).
    pub async fn python_standalone_mount_id(
        &mut self,
        series: &str,
        environment: Option<&str>,
    ) -> Result<String> {
        let builder_version = self.resolved_image_builder_version().await;
        let deployment_name =
            python_standalone_mount_name(series, &builder_version).ok_or_else(|| {
                Error::build(format!(
                    "unsupported standalone python series {series:?} for image builder \
                     version {builder_version:?}; supported series: {:?}",
                    standalone_versions(&builder_version)
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
        let req = build_mount_get_or_create_global_request(deployment_name, environment_name);
        let resp = self
            .retry_rpc(
                "mount_get_or_create(global)",
                req,
                |mut stub, req| async move { stub.mount_get_or_create(req).await },
            )
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
        assert_eq!(client_mount_name("1.6.4"), "modal-client-mount-1.6.4");
    }

    #[test]
    fn python_standalone_mount_name_matches_client() {
        // mount.py → python-build-standalone.{release}.{full}-gnu, per builder version.
        for legacy in ["2023.12", "2024.04", "2024.10", "2025.06", ""] {
            assert_eq!(
                python_standalone_mount_name("3.12", legacy).as_deref(),
                Some("python-build-standalone.20240107.3.12.1-gnu"),
                "builder {legacy:?}"
            );
        }
        assert_eq!(
            python_standalone_mount_name("3.13", "2025.06").as_deref(),
            Some("python-build-standalone.20241008.3.13.0-gnu")
        );
        for modern in ["2026.10", "PREVIEW"] {
            assert_eq!(
                python_standalone_mount_name("3.12", modern).as_deref(),
                Some("python-build-standalone.20261021.3.12.15-gnu"),
                "builder {modern:?}"
            );
        }
    }

    #[test]
    fn python_standalone_mount_name_rejects_unsupported_series() {
        assert_eq!(python_standalone_mount_name("3.99", "2025.06"), None);
        // 3.10 left the table with builder 2026.10.
        assert!(python_standalone_mount_name("3.10", "2025.06").is_some());
        assert_eq!(python_standalone_mount_name("3.10", "2026.10"), None);
    }

    #[test]
    fn python_symlink_is_needed_only_for_releases_without_a_python_binary() {
        assert!(python_standalone_needs_python_symlink("3.12", "2025.06"));
        assert!(!python_standalone_needs_python_symlink("3.13", "2025.06"));
        assert!(!python_standalone_needs_python_symlink("3.12", "2026.10"));
        assert!(!python_standalone_needs_python_symlink("3.99", "2025.06"));
    }

    #[test]
    fn build_mount_get_or_create_global_is_pure_lookup() {
        let req = build_mount_get_or_create_global_request(
            "modal-client-mount-1.6.4",
            "main".to_string(),
        );
        assert_eq!(req.deployment_name, "modal-client-mount-1.6.4");
        assert_eq!(req.environment_name, "main");
        // GLOBAL namespace, UNSPECIFIED creation (pure lookup, never creates).
        assert_eq!(req.namespace, DeploymentNamespace::Global as i32);
        assert_eq!(
            req.object_creation_type,
            ObjectCreationType::Unspecified as i32
        );
        // No files / app_id on a lookup.
        assert!(req.files.is_empty());
        assert!(req.app_id.is_empty());
    }
}
