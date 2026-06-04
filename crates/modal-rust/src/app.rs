//! The [`App`] handle: owns a [`Registry`] and (optionally) a live remote handle.
//!
//! Every constructor and accessor here is sync, zero-Modal, and zero-network, so
//! `.local()` works without ever calling [`App::connect`]. `connect()` builds a
//! real `sdk::ModalClient` for the future remote path, but no unit/integration
//! test calls it, so the offline gates stay green.

use tokio::sync::{Mutex, OnceCell};

use crate::deploy::{self, DeployConfig, DeployedApp};
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
    /// Per-entrypoint config from `#[modal_rust::function(...)]`. EMPTY for the
    /// manual `App::new(registry)` / `connect_with_registry` path (no decorator =>
    /// facade defaults apply via [`App::config_for`]).
    configs: std::collections::BTreeMap<String, modal_rust_runtime::FunctionConfig>,
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
    /// Resolved control-plane app id. For the RUN path this is an EPHEMERAL app
    /// (`AppCreate`, GC'd on disconnect) — so `.remote()` never leaves a lingering
    /// persistent deployment. The RUN path publishes the wrapper with
    /// `APP_STATE_EPHEMERAL` (publishing is needed to make the function invokable,
    /// but the ephemeral state keeps the app throwaway); persistent (DEPLOYED)
    /// `AppPublish` is DEPLOY-only.
    app_id: String,
    /// App name — needed for the EPHEMERAL `app_publish` + `from_name` resolution.
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
    ///
    /// The manual path has NO decorator config: `configs` is empty, so
    /// [`App::config_for`] returns `FunctionConfig::default()` (all `None`) and the
    /// facade falls back to its path defaults — behavior preserved.
    pub fn new(registry: Registry) -> Self {
        App {
            registry,
            configs: std::collections::BTreeMap::new(),
            remote: None,
        }
    }

    /// Build from the inventory-collected [`Registry`] (the
    /// `#[modal_rust::function]` macro path), ALSO capturing each entrypoint's
    /// decorator [`FunctionConfig`]. Zero Modal, zero network.
    pub fn from_inventory() -> Self {
        let (registry, configs) = modal_rust_runtime::from_inventory_with_configs();
        App {
            registry,
            configs: configs
                .into_iter()
                .map(|(n, c)| (n.to_string(), c))
                .collect(),
            remote: None,
        }
    }

    /// Connect to Modal's control plane for the remote path: build an
    /// `sdk::ModalClient` (reads `~/.modal.toml` / `MODAL_TOKEN_*`) and resolve an
    /// `app_id` via `AppGetOrCreate`. Uses the inventory [`Registry`] AND its
    /// per-entrypoint decorator config (so gpu/timeout survive into `.remote()`).
    ///
    /// Enables [`Function::remote`](crate::Function::remote); `.local()` never
    /// needs this call. `.spawn()`/`.map()` remain stubbed.
    pub async fn connect(name: &str) -> Result<Self> {
        let (registry, configs) = modal_rust_runtime::from_inventory_with_configs();
        let configs = configs
            .into_iter()
            .map(|(n, c)| (n.to_string(), c))
            .collect();
        App::connect_inner(name, registry, configs).await
    }

    /// As [`App::connect`], but combines an explicit [`Registry`] with a live
    /// remote handle. The manual path has NO decorator config (EMPTY `configs` =>
    /// facade defaults). The `app_id` is resolved in the configured environment
    /// (defaults to `"main"`).
    pub async fn connect_with_registry(name: &str, registry: Registry) -> Result<Self> {
        App::connect_inner(name, registry, std::collections::BTreeMap::new()).await
    }

    /// Shared connect body: build the ephemeral-app client handle and store the
    /// supplied per-entrypoint `configs` on the returned [`App`].
    async fn connect_inner(
        name: &str,
        registry: Registry,
        configs: std::collections::BTreeMap<String, modal_rust_runtime::FunctionConfig>,
    ) -> Result<Self> {
        let mut client = modal_rust_sdk::ModalClient::connect().await?; // From<sdk::Error>
                                                                        // RUN path = EPHEMERAL app: it is GC'd when this client disconnects, so
                                                                        // `.remote()` never leaves a lingering persistent deployment (the
                                                                        // crash-loop-clutter fix). `ensure_function` creates the wrapper in this
                                                                        // ephemeral app and invokes its `function_id` DIRECTLY — it does NOT
                                                                        // `app_publish` (which would set APP_STATE_DEPLOYED and promote this
                                                                        // throwaway app to a lingering persistent deploy). PERSISTENT publish is
                                                                        // DEPLOY-only (`App::deploy`).
        let app_id = client.app_create_ephemeral(name, None).await?;
        Ok(App {
            registry,
            configs,
            remote: Some(RemoteHandle {
                client: Mutex::new(client),
                app_id,
                app_name: name.to_string(),
                function_id: OnceCell::new(),
                config: RemoteConfig::default(),
            }),
        })
    }

    /// Resolve the decorator [`FunctionConfig`] for `name`. Returns
    /// `FunctionConfig::default()` (all `None`) for the manual path or an unknown
    /// name, so the facade's path defaults apply.
    pub(crate) fn config_for(&self, name: &str) -> modal_rust_runtime::FunctionConfig {
        self.configs.get(name).cloned().unwrap_or_default()
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

        // Resolve the invoked entrypoint's decorator config (gpu/timeout) and apply
        // it to a per-call clone of the path config BEFORE `ensure_function`. NOTE:
        // the wrapper `function_id` is memoized in a `OnceCell`, so the create (and
        // thus this config) is BOUND at the FIRST `.remote()` call on this App.
        // Acceptable for the single-GPU required path: one app typically targets one
        // GPU class. (A later GPU-list / per-entrypoint-function design would lift
        // this.)
        let cfg = self.config_for(entrypoint);
        let cfg_gpu: Option<String> = cfg.gpu.map(|s| s.to_string());
        let cfg_timeout: Option<u32> = cfg.timeout_secs;

        // Resolve (and memoize) the invokable function_id. `get_or_try_init`
        // single-flights the create sequence under concurrent `.remote()` calls.
        let function_id = handle
            .function_id
            .get_or_try_init(|| async {
                let mut run_config = handle.config.clone();
                run_config.gpu = cfg_gpu.clone();
                run_config.timeout_override_secs = cfg_timeout;
                let mut client = handle.client.lock().await;
                remote::ensure_function(&mut client, &handle.app_id, &handle.app_name, &run_config)
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
        // The function's effective timeout honors the decorator override (the same
        // value `ensure_function` set on the created function), so the poll deadline
        // tracks the actual container timeout.
        let effective_timeout = cfg_timeout.unwrap_or(handle.config.timeout_secs);
        let empty_kwargs: std::collections::HashMap<String, ()> = std::collections::HashMap::new();
        let deadline = std::time::Duration::from_secs(effective_timeout as u64 + 120);
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

    /// DEPLOY the wrapper function persistently under a STABLE app name (the
    /// PERSISTENT path — the ONLY one that uses `AppPublish` into a named app).
    ///
    /// Builds the deploy image (source COPYed into a layer; `cargo build --release`
    /// runs AT image-build time), creates the FILE-mode function with the client
    /// mount ONLY (the prebuilt `/app/modal_runner` is baked in the image — NO
    /// runtime source mount, NO cargo at call time), and publishes it. Re-deploys
    /// REPLACE the named app, so re-runs never accumulate.
    ///
    /// Requires a connected App ([`App::connect`](crate::App::connect)). The deploy
    /// app name comes from [`DeployConfig`] (default `"modal-rust-add-deploy"`,
    /// override `MODAL_RUST_DEPLOY_APP`); use [`App::deploy_with`] to pass an
    /// explicit config.
    pub async fn deploy(&self) -> Result<DeployedApp> {
        self.deploy_with(DeployConfig::default()).await
    }

    /// As [`App::deploy`], with an explicit [`DeployConfig`] (STABLE app name,
    /// source root, package, base image, ignore set).
    ///
    /// One Modal wrapper serves EVERY entrypoint, so the decorator gpu/timeout is
    /// resolved for the single decorated entrypoint (P4 deploy targets one app/one
    /// wrapper) and threaded onto the [`DeployConfig`]. The manual path (no
    /// decorator config) leaves the deploy defaults untouched.
    pub async fn deploy_with(&self, mut config: DeployConfig) -> Result<DeployedApp> {
        let handle = self.remote.as_ref().ok_or_else(Error::not_connected)?;
        // Resolve the decorated entrypoint's config. P4 deploy serves one wrapper,
        // so pick the single decorated entrypoint (the first registered name with a
        // non-default config; else the first name). Manual path => default (no-op).
        if let Some(cfg) = self.deploy_target_config() {
            config.gpu = cfg.gpu.map(|s| s.to_string());
            config.timeout_override_secs = cfg.timeout_secs;
        }
        let mut client = handle.client.lock().await;
        deploy::deploy_function(&mut client, &config).await
    }

    /// Pick the decorator config to apply at deploy time. Returns the first
    /// entrypoint config that sets gpu/timeout (the typical single-decorated-fn
    /// deploy); falls back to the first registered config, else `None` (manual
    /// path => deploy defaults).
    fn deploy_target_config(&self) -> Option<modal_rust_runtime::FunctionConfig> {
        self.configs
            .values()
            .find(|c| c.gpu.is_some() || c.timeout_secs.is_some())
            .or_else(|| self.configs.values().next())
            .cloned()
    }

    /// CALL a DEPLOYED function by app name + entrypoint, returning the typed
    /// output with the SAME semantics as [`Function::local`](crate::Function::local).
    ///
    /// NO upload, NO image build, NO `app_publish` — that absence IS the deploy
    /// invariant. The deployed function is resolved by name (`from_name`) and
    /// invoked directly; the prebuilt `/app/modal_runner` execs the handler.
    ///
    /// Requires a connected App ([`App::connect`](crate::App::connect)).
    pub async fn call<In, Out>(&self, app_name: &str, entrypoint: &str, input: In) -> Result<Out>
    where
        In: serde::Serialize,
        Out: serde::de::DeserializeOwned,
    {
        let handle = self.remote.as_ref().ok_or_else(Error::not_connected)?;
        let input_json = serde_json::to_string(&input).map_err(Error::Encode)?;
        let mut client = handle.client.lock().await;
        let envelope = deploy::call_function(&mut client, app_name, entrypoint, input_json).await?;
        crate::remote::parse_envelope::<Out>(&envelope)
    }

    /// CALL a [`DeployedApp`] returned by [`App::deploy`] directly (resolves by its
    /// stable name). Convenience wrapper over [`App::call`].
    pub async fn call_deployed<In, Out>(
        &self,
        deployed: &DeployedApp,
        entrypoint: &str,
        input: In,
    ) -> Result<Out>
    where
        In: serde::Serialize,
        Out: serde::de::DeserializeOwned,
    {
        let handle = self.remote.as_ref().ok_or_else(Error::not_connected)?;
        let mut client = handle.client.lock().await;
        deployed.call_with(&mut client, entrypoint, input).await
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

#[cfg(test)]
mod tests {
    use super::*;
    // Link example-add-macro's inventory submissions (incl. the decorated `add_gpu`
    // with `gpu="T4", timeout=1800, cache=false`) into this test binary so
    // `App::from_inventory()` surfaces their FunctionConfig.
    use example_add_macro as _;

    #[test]
    fn from_inventory_captures_decorator_config() {
        let app = App::from_inventory();
        // The decorated entrypoint's config flows through `config_for`.
        let gpu_cfg = app.config_for("add_gpu");
        assert_eq!(gpu_cfg.gpu, Some("T4"));
        assert_eq!(gpu_cfg.timeout_secs, Some(1800));
        assert_eq!(gpu_cfg.cache, Some(false));
        // The bare decorated entrypoint has the default (all-None) config.
        let bare = app.config_for("add");
        assert_eq!(bare, modal_rust_runtime::FunctionConfig::default());
    }

    #[test]
    fn manual_new_path_config_is_default() {
        // The manual `App::new(registry)` path has NO decorator config (empty
        // configs map), so `config_for` returns the default for any name.
        let app = App::new(Registry::new());
        assert_eq!(
            app.config_for("anything"),
            modal_rust_runtime::FunctionConfig::default()
        );
    }
}
