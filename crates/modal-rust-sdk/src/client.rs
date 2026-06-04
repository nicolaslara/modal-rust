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
pub struct ModalClient {
    inner: ModalClientStub,
    config: ModalConfig,
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
        let interceptor = AuthInterceptor::new(&config.token_id, &config.token_secret)?;
        let inner = ModalClientClient::with_interceptor(channel, interceptor);
        let mut client = Self { inner, config };
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

    /// The configured image builder version, if any (used by `ImageGetOrCreate`).
    pub(crate) fn image_builder_version(&self) -> Option<&str> {
        self.config.image_builder_version.as_deref()
    }

    /// Low-level escape hatch: the underlying generated gRPC stub. Used by the
    /// `ops` surface (later phases) to issue arbitrary control-plane RPCs.
    pub fn inner_mut(&mut self) -> &mut ModalClientStub {
        &mut self.inner
    }

    /// Connect-time handshake (`ClientHello`, api.proto:4171). Free, no GPU/cost.
    /// Acts as the post-connect auth probe: a bad token id/secret surfaces here
    /// as an [`crate::Error::Status`] (Unauthenticated). The response's
    /// `warning` / `server_warnings` are advisory; the deprecated
    /// `image_builder_version` field is ignored (resolved from config instead).
    pub async fn client_hello(&mut self) -> Result<()> {
        let _resp = self.inner.client_hello(()).await?.into_inner();
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

        let resp = self
            .inner
            .app_get_or_create(AppGetOrCreateRequest {
                app_name: app_name.to_string(),
                environment_name,
                object_creation_type: ObjectCreationType::CreateIfMissing as i32,
            })
            .await?
            .into_inner();

        Ok(resp.app_id)
    }
}
