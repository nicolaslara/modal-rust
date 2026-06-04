//! The [`App`] handle: owns a [`Registry`] and (optionally) a live remote handle.
//!
//! Every constructor and accessor here is sync, zero-Modal, and zero-network, so
//! `.local()` works without ever calling [`App::connect`]. `connect()` builds a
//! real `sdk::ModalClient` for the future remote path, but no unit/integration
//! test calls it, so the offline gates stay green.

use tokio::sync::{Mutex, OnceCell};

use crate::remote::{self, RemoteConfig};
use crate::{Error, Function, Registry, Result};

/// The user-facing application handle.
///
/// Build one from an explicit [`Registry`] ([`App::new`]) or from the
/// inventory-collected registry ([`App::from_inventory`], the
/// `#[modal_rust::function]` path). Resolve a [`Function`] handle by entrypoint
/// name with [`App::function`].
pub struct App {
    /// Owned registry; the ONLY field `.local()` needs.
    registry: Registry,
    /// `None` until [`App::connect`]; the live control-plane handle `.remote()`
    /// consumes. `.local()` never touches it.
    remote: Option<RemoteHandle>,
}

/// A live control-plane handle, built by [`App::connect`]. Private — `.remote()`
/// drives it through [`App::remote_invoke`].
struct RemoteHandle {
    /// Interior mutability: `App::function` hands out `Function<'_>` borrowing
    /// `&App`, but `invoke_cbor`/the ensure sequence need `&mut ModalClient`. The
    /// `Mutex` also single-flights concurrent `.remote()` calls cleanly.
    client: Mutex<modal_rust_sdk::ModalClient>,
    /// Resolved control-plane app id (`AppGetOrCreate`).
    app_id: String,
    /// App name — needed for `app_publish` / `from_name` resolution.
    app_name: String,
    /// Memoized invokable `function_id` for the single wrapper function that serves
    /// every entrypoint. `get_or_try_init` gives correct single-flight create.
    function_id: OnceCell<String>,
    /// RUN-path knobs (source dir, package, image, timeout, ignore set).
    config: RemoteConfig,
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
    /// Enables [`Function::remote`](crate::Function::remote); `.local()` never
    /// needs this call. `.spawn()`/`.map()` remain stubbed.
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
            remote: Some(RemoteHandle {
                client: Mutex::new(client),
                app_id,
                app_name: name.to_string(),
                function_id: OnceCell::new(),
                config: RemoteConfig::default(),
            }),
        })
    }

    /// Drive the RUN path for one entrypoint: ensure the wrapper function exists on
    /// Modal (once per App, single-flighted via the `function_id` [`OnceCell`]),
    /// then invoke it with `(entrypoint, input_json)` and return the runner's
    /// one-line JSON envelope string. The caller ([`Function::remote`]) parses it.
    ///
    /// `cargo build` runs in the function body at invoke time (the RUN boundary);
    /// this method only orchestrates the control plane + the CBOR round-trip.
    pub(crate) async fn remote_invoke(
        &self,
        entrypoint: &str,
        input_json: String,
    ) -> Result<String> {
        let handle = self.remote.as_ref().ok_or_else(Error::not_connected)?;

        // Resolve (and memoize) the invokable function_id. `get_or_try_init`
        // single-flights the create sequence under concurrent `.remote()` calls.
        let function_id = handle
            .function_id
            .get_or_try_init(|| async {
                let mut client = handle.client.lock().await;
                remote::ensure_function(
                    &mut client,
                    &handle.app_id,
                    &handle.app_name,
                    &handle.config,
                )
                .await
            })
            .await?;

        // Invoke: two positional args (entrypoint, input_json), no kwargs. R=String
        // (the wrapper returns the runner stdout envelope verbatim).
        //
        // The output-poll deadline must cover the cold in-body `cargo build` (the
        // RUN boundary): the first call to a fresh container compiles the whole
        // dep tree, which can take many minutes — far past the SDK's 600s default.
        // Match the function's own container timeout (plus a small queue/schedule
        // buffer) so the client keeps polling for as long as the function may run.
        let empty_kwargs: std::collections::HashMap<String, ()> = std::collections::HashMap::new();
        let deadline = std::time::Duration::from_secs(handle.config.timeout_secs as u64 + 120);
        let mut client = handle.client.lock().await;
        let envelope: String = client
            .invoke_cbor_with_deadline(
                function_id,
                &(entrypoint, input_json),
                &empty_kwargs,
                deadline,
            )
            .await?;
        Ok(envelope)
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
