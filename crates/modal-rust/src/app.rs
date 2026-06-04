//! The [`App`] handle: owns a [`Registry`] and (optionally) a live remote handle.
//!
//! Every constructor and accessor here is sync, zero-Modal, and zero-network, so
//! `.local()` works without ever calling [`App::connect`]. `connect()` builds a
//! real `sdk::ModalClient` for the future remote path, but no unit/integration
//! test calls it, so the offline gates stay green.

use crate::{Function, Registry, Result};

/// The user-facing application handle.
///
/// Build one from an explicit [`Registry`] ([`App::new`]) or from the
/// inventory-collected registry ([`App::from_inventory`], the
/// `#[modal_rust::function]` path). Resolve a [`Function`] handle by entrypoint
/// name with [`App::function`].
pub struct App {
    /// Owned registry; the ONLY field `.local()` needs.
    registry: Registry,
    /// `None` until [`App::connect`]; written by `connect_with_registry` and read
    /// by the next-milestone remote body (the stubbed `.remote()`/`.spawn()`/`.map()`
    /// surface does not touch it yet).
    #[allow(dead_code)]
    remote: Option<RemoteHandle>,
}

/// A live control-plane handle, built by [`App::connect`]. Private — the remote
/// surface that consumes it is stubbed this milestone.
#[allow(dead_code)] // fields consumed by the next-milestone remote body.
struct RemoteHandle {
    client: modal_rust_sdk::ModalClient,
    app_id: String,
}

impl App {
    /// Build from an explicit [`Registry`] (manual builder path — e.g.
    /// `example_add::modal_registry()`). Zero Modal, zero network.
    pub fn new(registry: Registry) -> Self {
        App {
            registry,
            remote: None,
        }
    }

    /// Build from the inventory-collected [`Registry`] (the
    /// `#[modal_rust::function]` macro path). Zero Modal, zero network.
    pub fn from_inventory() -> Self {
        App::new(Registry::from_inventory())
    }

    /// Connect to Modal's control plane for the remote path: build an
    /// `sdk::ModalClient` (reads `~/.modal.toml` / `MODAL_TOKEN_*`) and resolve an
    /// `app_id` via `AppGetOrCreate`. Uses the inventory [`Registry`].
    ///
    /// `.remote()`/`.spawn()`/`.map()` still return [`crate::Error::NotImplemented`]
    /// THIS milestone; `.local()` never needs this call. Wired for real so the next
    /// milestone is a pure addition.
    pub async fn connect(name: &str) -> Result<Self> {
        App::connect_with_registry(name, Registry::from_inventory()).await
    }

    /// As [`App::connect`], but combines an explicit [`Registry`] with a live
    /// remote handle. The `app_id` is resolved in the configured environment
    /// (defaults to `"main"`).
    pub async fn connect_with_registry(name: &str, registry: Registry) -> Result<Self> {
        let mut client = modal_rust_sdk::ModalClient::connect().await?; // From<sdk::Error>
        let app_id = client.app_get_or_create_id(name, None).await?;
        Ok(App {
            registry,
            remote: Some(RemoteHandle { client, app_id }),
        })
    }

    /// Get a [`Function`] handle by entrypoint name. Resolves the [`crate::HandlerFn`]
    /// from the [`Registry`] now (cheap, `Copy`) so an unknown name surfaces a clear
    /// error with the full known-names list when `.local()`/`.remote()` is actually
    /// called. Does NOT error eagerly — keeps the API fluent
    /// (`app.function("add").local(..)`).
    pub fn function(&self, name: &str) -> Function<'_> {
        Function {
            app: self,
            name: name.to_string(),
            handler: self.registry.get(name), // Option<HandlerFn>
        }
    }

    /// The registered entrypoint names, for diagnostics (e.g. the
    /// unknown-entrypoint error).
    pub(crate) fn known_names(&self) -> Vec<String> {
        self.registry.names().map(|n| n.to_string()).collect()
    }
}
