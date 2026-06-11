//! High-level authenticated Modal client.
//!
//! [`ModalClient`] wraps the tonic-generated `ModalClientClient` stub behind an
//! [`AuthInterceptor`] (every call carries the `x-modal-*` headers) over a
//! hardened TLS channel. `connect()` resolves credentials from the environment /
//! `~/.modal.toml`, builds the channel, and performs a `ClientHello` handshake to
//! fail fast on bad credentials before any real work.

use tonic::codegen::InterceptedService;
use tonic::transport::Channel;

use crate::auth::AuthInterceptor;
use crate::channel::build_channel;
use crate::config::{read_modal_config, ModalConfig};
use crate::error::Result;
use crate::proto::api::modal_client_client::ModalClientClient;
use crate::proto::api::{AppGetOrCreateRequest, ObjectCreationType};
use crate::retry::retry_unary;

/// The inner stub type: the generated gRPC client over a TLS channel with the
/// auth interceptor applied. Exposed via [`ModalClient::inner_mut`] so the `ops`
/// surface (later phases) can issue any control-plane RPC.
pub type ModalClientStub = ModalClientClient<InterceptedService<Channel, AuthInterceptor>>;

/// Authenticated client for Modal's control plane.
///
/// # Example
///
/// ```rust,no_run
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// use modal_rust_sdk::ModalClient;
///
/// let mut client = ModalClient::connect().await?;
/// let app_id = client.app_get_or_create("my-app", None).await?;
/// println!("app_id = {app_id}");
/// # Ok(())
/// # }
/// ```
/// `Clone` is cheap and shares the underlying HTTP/2 channel (tonic multiplexes
/// concurrent requests over one connection) — used by the mount upload to probe
/// many files concurrently.
#[derive(Clone)]
pub struct ModalClient {
    inner: ModalClientStub,
    config: ModalConfig,
    /// Lazily-resolved image builder version (see
    /// [`ModalClient::resolved_image_builder_version`]). Cached after the first
    /// `EnvironmentGetOrCreate` so repeated image builds don't re-fetch it.
    resolved_builder_version: Option<String>,
}

impl ModalClient {
    /// Connect using config resolved from the environment / `~/.modal.toml`
    /// (see [`read_modal_config`]). Performs a `ClientHello` handshake.
    pub async fn connect() -> Result<Self> {
        Self::from_config(read_modal_config()?).await
    }

    /// Connect using explicit credentials against the default endpoint,
    /// bypassing any config file. Performs a `ClientHello` handshake.
    pub async fn connect_with_credentials(token_id: &str, token_secret: &str) -> Result<Self> {
        Self::from_config(ModalConfig::from_credentials(token_id, token_secret)?).await
    }

    /// Connect using a fully-resolved [`ModalConfig`]. Builds the channel +
    /// interceptor, then issues `ClientHello` to surface auth failures early.
    pub async fn from_config(config: ModalConfig) -> Result<Self> {
        let channel = build_channel(&config.server_url).await?;
        // IN-CONTAINER (MODAL_IS_REMOTE=1): CLIENT_TYPE_CONTAINER with NO token
        // headers — the worker-provided connection is the credential (Python
        // parity, client.py:230-231). Everywhere else: token-authenticated CLIENT.
        let interceptor = if config.is_container {
            AuthInterceptor::container()?
        } else {
            AuthInterceptor::new(&config.token_id, &config.token_secret)?
        };
        let inner = ModalClientClient::with_interceptor(channel, interceptor);
        let mut client = Self {
            inner,
            config,
            resolved_builder_version: None,
        };
        client.client_hello().await?;
        Ok(client)
    }

    /// The resolved configuration this client connected with.
    pub fn config(&self) -> &ModalConfig {
        &self.config
    }

    /// Resolve the environment name for an op: the explicit override, else the
    /// configured environment, else [`crate::config::DEFAULT_ENVIRONMENT`]. Shared
    /// by the `ops` surface so every RPC scopes to a consistent environment.
    pub(crate) fn env_or_default(&self, environment: Option<&str>) -> String {
        environment
            .map(str::to_string)
            .unwrap_or_else(|| self.config.environment_or_default().to_string())
    }

    /// Resolve the image builder version to send with `ImageGetOrCreate`, mirroring
    /// the official client's `_get_image_builder_version` (`_image.py:247`): an
    /// explicit config / `MODAL_IMAGE_BUILDER_VERSION` value wins; otherwise it comes
    /// from the ENVIRONMENT's settings (`EnvironmentGetOrCreate` →
    /// `EnvironmentSettings.image_builder_version`, e.g. `"2025.06"`).
    ///
    /// This matters beyond image rendering: the worker mounts the modal client's dep
    /// closure at container start ONLY for a builder version `> "2024.10"` (the
    /// `mount_client_dependencies` gate, `_functions.py:936-939`). Sending an empty
    /// builder version built an image whose `python -m modal._container_entrypoint`
    /// had no deps and was TERMINATED at boot (live-observed 2026-06-04). Resolving the
    /// environment's modern version makes our `add_python` image + the
    /// `mount_client_dependencies = true` claim mutually consistent.
    ///
    /// The result is cached on `self` after the first lookup. A lookup failure (or an
    /// empty server value) caches the EMPTY string, falling back to letting the server
    /// pick — the prior behavior — rather than failing the build.
    pub(crate) async fn resolved_image_builder_version(&mut self) -> String {
        if let Some(v) = &self.resolved_builder_version {
            return v.clone();
        }
        // 1. Explicit config / env override wins (matches the Python client).
        if let Some(v) = self.config.image_builder_version.as_deref() {
            if !v.is_empty() {
                self.resolved_builder_version = Some(v.to_string());
                return v.to_string();
            }
        }
        // 2. Otherwise resolve from the environment's settings.
        let resolved = self
            .fetch_environment_builder_version()
            .await
            .unwrap_or_else(|e| {
                eprintln!(
                    "[modal-rust] could not resolve image builder version from the environment \
                 ({e}); letting the server choose (set MODAL_IMAGE_BUILDER_VERSION to pin)"
                );
                String::new()
            });
        self.resolved_builder_version = Some(resolved.clone());
        resolved
    }

    /// Fetch `EnvironmentSettings.image_builder_version` for the configured
    /// environment via `EnvironmentGetOrCreate` (idempotent lookup).
    async fn fetch_environment_builder_version(&mut self) -> Result<String> {
        use crate::proto::api::EnvironmentGetOrCreateRequest;
        let req = EnvironmentGetOrCreateRequest {
            deployment_name: self.config.environment_or_default().to_string(),
            object_creation_type: ObjectCreationType::Unspecified as i32,
        };
        let stub = self.stub();
        let resp = retry_unary("environment_get_or_create", || {
            let mut stub = stub.clone();
            let req = req.clone();
            async move { Ok(stub.environment_get_or_create(req).await?.into_inner()) }
        })
        .await?;
        Ok(resp
            .metadata
            .and_then(|m| m.settings)
            .map(|s| s.image_builder_version)
            .unwrap_or_default())
    }

    /// Low-level escape hatch: the underlying generated gRPC stub. Used by the
    /// `ops` surface (later phases) to issue arbitrary control-plane RPCs.
    pub fn inner_mut(&mut self) -> &mut ModalClientStub {
        &mut self.inner
    }

    /// A fresh clone of the underlying gRPC stub. The tonic stub is cheap to
    /// clone (the channel is an `Arc`-backed multiplexed handle), so each
    /// transient-retry attempt clones its own owned stub — letting the retried
    /// future own its borrow (`retry_unary`'s `FnMut` cannot hold `&mut self`).
    pub(crate) fn stub(&self) -> ModalClientStub {
        self.inner.clone()
    }

    /// Connect-time handshake (`ClientHello`, api.proto:4171). Free, no GPU/cost.
    /// Acts as the post-connect auth probe: a bad token id/secret surfaces here
    /// as an [`crate::Error::Status`] (Unauthenticated). The response's
    /// `warning` / `server_warnings` are advisory; the deprecated
    /// `image_builder_version` field is ignored (resolved from config instead).
    pub async fn client_hello(&mut self) -> Result<()> {
        // Clone the (cheap, Arc-backed) stub per attempt so the retried future
        // owns its borrow — `retry_unary`'s `FnMut` cannot hold `&mut self.inner`.
        let stub = &self.inner;
        let _resp = retry_unary("client_hello", || {
            let mut stub = stub.clone();
            async move { Ok(stub.client_hello(()).await?.into_inner()) }
        })
        .await?;
        Ok(())
    }

    /// Cheap, safe live RPC proving auth end-to-end:
    /// `AppGetOrCreate` (api.proto:4142). No cost, no GPU. Returns the `app_id`.
    ///
    /// `environment` defaults to the configured environment (or `"main"`).
    /// Pass [`ObjectCreationType::CreateIfMissing`] semantics by default so this
    /// is idempotent and resume-friendly (the first real step of the recipe).
    pub async fn app_get_or_create(
        &mut self,
        app_name: &str,
        environment: Option<&str>,
    ) -> Result<String> {
        let environment_name = environment
            .map(str::to_string)
            .unwrap_or_else(|| self.config.environment_or_default().to_string());

        let req = AppGetOrCreateRequest {
            app_name: app_name.to_string(),
            environment_name,
            object_creation_type: ObjectCreationType::CreateIfMissing as i32,
        };
        let stub = &self.inner;
        let resp = retry_unary("app_get_or_create", || {
            let mut stub = stub.clone();
            let req = req.clone();
            async move { Ok(stub.app_get_or_create(req).await?.into_inner()) }
        })
        .await?;

        Ok(resp.app_id)
    }
}
