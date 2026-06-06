//! The [`App`] handle: owns a [`Registry`] and (optionally) a live remote handle.
//!
//! Every constructor and accessor here is sync, zero-Modal, and zero-network, so
//! `.local()` works without ever calling [`App::connect`]. `connect()` builds a
//! real `sdk::ModalClient` for the future remote path, but no unit/integration
//! test calls it, so the offline gates stay green.

use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::{Mutex, OnceCell};

use crate::deploy::{self, DeployConfig, DeployedApp};
use crate::remote::{self, RemoteConfig};
use crate::{Error, Function, Registry, Result};

/// One `map` input as the SDK's `map_cbor` expects it: `(args, kwargs)` where
/// `args = (entrypoint, input_json)` (the SAME 2-tuple `.remote()` sends) and
/// `kwargs` is the empty map. Aliased to keep [`App::remote_map`]'s annotation
/// readable (clippy `type_complexity`).
type MapInput<'a> = ((&'a str, String), std::collections::HashMap<String, ()>);

/// Memo key for created RUN-path Modal functions. Each ENTRYPOINT gets its OWN Modal
/// function (object tag = the entrypoint) carrying its OWN effective config, so the
/// key is the entrypoint name PLUS its effective gpu/timeout/cache/secrets/volumes.
/// Including the config means a (hypothetical) per-call config change re-creates
/// rather than silently reusing a stale wrapper.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RunFunctionKey {
    entrypoint: String,
    gpu: Option<String>,
    timeout_secs: u32,
    cache: bool,
    secrets: Vec<String>,
    volumes: Vec<(String, String)>,
}

/// The user-facing application handle.
///
/// Build an offline (in-process, no Modal) handle with [`App::local`] (the
/// `#[modal_rust::function]` path) or from an explicit [`Registry`] with
/// [`App::local_with_registry`]; build a remote handle with [`App::connect`].
/// Resolve a [`Function`] handle by entrypoint name with [`App::function`].
pub struct App {
    /// Owned registry; the ONLY field `.local()` needs.
    registry: Registry,
    /// Per-entrypoint config from `#[modal_rust::function(...)]`. EMPTY for the
    /// manual `App::local_with_registry(registry)` / `connect_with_registry` path
    /// (no decorator => facade defaults apply via [`App::config_for`]).
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
    /// Memoized invokable `function_id`s keyed by ENTRYPOINT + effective wrapper
    /// config. Each entrypoint is created as its OWN Modal function (object tag =
    /// the entrypoint), carrying its own gpu/timeout/cache/secrets/volumes, so
    /// divergent per-entrypoint configs COEXIST instead of clobbering one shared
    /// `"handler"`. Keying by entrypoint (not just config) keeps the memo 1:1 with
    /// the created function; the config rides in the key so a future config change to
    /// the same entrypoint would key distinctly. Each value is a `OnceCell` so
    /// concurrent first calls to the same key single-flight create.
    function_ids: Mutex<BTreeMap<RunFunctionKey, Arc<OnceCell<String>>>>,
    /// Cumulative published RUN functions for this ephemeral app. `AppPublish` is a
    /// SET-STATE publish, so each per-entrypoint create re-publishes the UNION of all
    /// functions created so far (else the prior entrypoint is de-invoked). Guarded by
    /// its own `Mutex` so the publish set stays consistent under concurrent creates.
    published: Mutex<remote::PublishedFunctions>,
    /// RUN-path knobs (source dir, package, image, timeout, ignore set).
    config: RemoteConfig,
}

impl App {
    /// Build an offline (in-process, no Modal) app from an explicit [`Registry`]
    /// (manual builder path — e.g. `example_add::modal_registry()`). Zero Modal,
    /// zero network.
    ///
    /// The manual path has NO decorator config: `configs` is empty, so
    /// [`App::config_for`] returns `FunctionConfig::default()` (all `None`) and the
    /// facade falls back to its path defaults — behavior preserved.
    pub fn local_with_registry(registry: Registry) -> Self {
        App {
            registry,
            configs: std::collections::BTreeMap::new(),
            remote: None,
        }
    }

    /// Build an offline (in-process, no Modal) app over the functions decorated
    /// with `#[modal_rust::function]`, ALSO capturing each entrypoint's decorator
    /// [`FunctionConfig`]. Zero Modal, zero network.
    pub fn local() -> Self {
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
        // PACKAGE AUTO-DETECT: the `#[modal_rust::function]` macro captured the
        // user's `env!("CARGO_PKG_NAME")` into each inventory `Registration`. Thread
        // it into the RUN config so `cargo build -p <pkg>` targets the user's crate
        // WITHOUT them setting `MODAL_RUST_PACKAGE`. `MODAL_RUST_PACKAGE` still
        // OVERRIDES (it is applied first inside `RemoteConfig::default()`), so this
        // only fills in the package when the env var is unset.
        let run_config = RemoteConfig::default().with_detected_package(
            std::env::var("MODAL_RUST_PACKAGE").ok().as_deref(),
            modal_rust_runtime::package_from_inventory(),
        );
        App::connect_inner(name, registry, configs, run_config).await
    }

    /// As [`App::connect`], but combines an explicit [`Registry`] with a live
    /// remote handle. The manual path has NO decorator config (EMPTY `configs` =>
    /// facade defaults). The `app_id` is resolved in the configured environment
    /// (defaults to `"main"`).
    pub async fn connect_with_registry(name: &str, registry: Registry) -> Result<Self> {
        App::connect_inner(
            name,
            registry,
            std::collections::BTreeMap::new(),
            RemoteConfig::default(),
        )
        .await
    }

    /// Build a HEADLESS [`App`] from a `--describe` manifest: per-entrypoint config
    /// but NO handlers (empty [`Registry`]). `.local()` would fail (no handler), but
    /// `.remote()`/`deploy`/`call` never need handlers — they read only
    /// [`config_for`](App::config_for) + the SDK ops (P9 §B.1). Used by the
    /// `modal-rust` CLI, which cannot link the user crate.
    ///
    /// Zero Modal, zero network — pair with
    /// [`connect_from_manifest`](App::connect_from_manifest) for the live handle.
    pub fn from_manifest(
        configs: impl IntoIterator<Item = (String, modal_rust_runtime::FunctionConfig)>,
    ) -> Self {
        App {
            registry: Registry::new(),
            configs: configs.into_iter().collect(),
            remote: None,
        }
    }

    /// As [`App::connect`], but seeds an EMPTY [`Registry`] + the manifest configs +
    /// an EXPLICIT [`RemoteConfig`] (built by the CLI from the real workspace_root +
    /// package), instead of `connect_inner`'s hardcoded `RemoteConfig::default()`
    /// (which would (mis)discover `local_root`/`package` from the CLI's arbitrary
    /// CWD). Headless: no handlers, so only `.remote()`/`deploy`/`call` work (P9 §B).
    pub async fn connect_from_manifest(
        name: &str,
        configs: impl IntoIterator<Item = (String, modal_rust_runtime::FunctionConfig)>,
        run_config: RemoteConfig,
    ) -> Result<Self> {
        App::connect_inner(
            name,
            Registry::new(),
            configs.into_iter().collect(),
            run_config,
        )
        .await
    }

    /// Shared connect body: build the ephemeral-app client handle, store the
    /// supplied per-entrypoint `configs`, and seed the EXPLICIT `run_config` (the
    /// only delta between `connect`/`connect_with_registry` — which pass
    /// `RemoteConfig::default()` — and the CLI's `connect_from_manifest`, which
    /// supplies a workspace-scoped config).
    async fn connect_inner(
        name: &str,
        registry: Registry,
        configs: std::collections::BTreeMap<String, modal_rust_runtime::FunctionConfig>,
        run_config: RemoteConfig,
    ) -> Result<Self> {
        let client = modal_rust_sdk::ModalClient::connect().await?; // From<sdk::Error>
        Self::connect_inner_with_client(name, registry, configs, run_config, client).await
    }

    /// Shared connect body taking an ALREADY-BUILT [`modal_rust_sdk::ModalClient`]:
    /// create the ephemeral RUN app and assemble the [`App`]. Factored out of
    /// [`connect_inner`](App::connect_inner) so the test-only `connect_at*`
    /// constructors can supply a client built with
    /// [`from_config`](modal_rust_sdk::ModalClient::from_config) (pointed at an
    /// in-process mock) instead of the real [`connect`](modal_rust_sdk::ModalClient::connect).
    ///
    /// RUN path = EPHEMERAL app: it is GC'd when this client disconnects, so
    /// `.remote()` never leaves a lingering persistent deployment. `ensure_function`
    /// creates the wrapper in this ephemeral app and invokes its `function_id`
    /// DIRECTLY — PERSISTENT publish is DEPLOY-only (`App::deploy`).
    async fn connect_inner_with_client(
        name: &str,
        registry: Registry,
        configs: std::collections::BTreeMap<String, modal_rust_runtime::FunctionConfig>,
        run_config: RemoteConfig,
        mut client: modal_rust_sdk::ModalClient,
    ) -> Result<Self> {
        let app_id = client.app_create_ephemeral(name, None).await?;
        Ok(App {
            registry,
            configs,
            remote: Some(RemoteHandle {
                client: Mutex::new(client),
                app_id,
                app_name: name.to_string(),
                function_ids: Mutex::new(BTreeMap::new()),
                published: Mutex::new(remote::PublishedFunctions::default()),
                config: run_config,
            }),
        })
    }

    /// TEST-ONLY: connect at an explicit `server_url` (e.g. an in-process mock)
    /// using the given [`Registry`] and DUMMY credentials, instead of resolving
    /// real Modal config. Additive — does NOT change [`connect`](App::connect) or
    /// any other constructor; the public deploy/call/remote behavior is unchanged.
    ///
    /// Gated behind the `testkit` feature (enabled only by the facade's test
    /// targets via `[dev-dependencies]`), so it is NOT part of the shipped public
    /// API. The env-var path (`MODAL_SERVER_URL`) is process-global and unsuitable
    /// for parallel / table tests that each need their OWN mock port — hence this
    /// per-`App` seam.
    #[cfg(any(test, feature = "testkit"))]
    pub async fn connect_at(name: &str, registry: Registry, server_url: String) -> Result<Self> {
        Self::connect_at_with(name, registry, server_url, RemoteConfig::default()).await
    }

    /// As [`connect_at`](App::connect_at), plus an explicit [`RemoteConfig`]
    /// (gpu/timeout/source dir/etc.) — the table-test entry point: each case builds
    /// its own mock + its own per-case `RemoteConfig` and asserts the captured
    /// `FunctionCreate` manifest.
    #[cfg(any(test, feature = "testkit"))]
    pub async fn connect_at_with(
        name: &str,
        registry: Registry,
        server_url: String,
        run_config: RemoteConfig,
    ) -> Result<Self> {
        Self::connect_at_with_configs(
            name,
            registry,
            std::collections::BTreeMap::new(),
            server_url,
            run_config,
        )
        .await
    }

    /// As [`connect_at_with`](App::connect_at_with), but ALSO threads per-entrypoint
    /// decorator [`FunctionConfig`]s — the gpu/timeout/secrets/volumes the RUN path
    /// resolves via [`config_for`](App::config_for). This is the table-test entry
    /// point that drives the manifest the SAME way a `#[function(gpu=.., timeout=..)]`
    /// decorator would (the RUN path re-derives gpu/timeout from the decorator config,
    /// not the bare `RemoteConfig`, so a faithful table must supply them here).
    #[cfg(any(test, feature = "testkit"))]
    pub async fn connect_at_with_configs(
        name: &str,
        registry: Registry,
        configs: std::collections::BTreeMap<String, modal_rust_runtime::FunctionConfig>,
        server_url: String,
        run_config: RemoteConfig,
    ) -> Result<Self> {
        let config = modal_rust_sdk::ModalConfig {
            profile: "mock".into(),
            server_url,
            token_id: "ak-mock".into(),
            token_secret: "as-mock".into(),
            environment: Some("main".into()),
            image_builder_version: None,
        };
        let client = modal_rust_sdk::ModalClient::from_config(config).await?;
        Self::connect_inner_with_client(name, registry, configs, run_config, client).await
    }

    /// Resolve the decorator [`FunctionConfig`] for `name`. Returns
    /// `FunctionConfig::default()` (all `None`) for the manual path or an unknown
    /// name, so the facade's path defaults apply.
    pub(crate) fn config_for(&self, name: &str) -> modal_rust_runtime::FunctionConfig {
        self.configs.get(name).cloned().unwrap_or_default()
    }

    /// Drive the RUN path for one entrypoint: ensure a wrapper function exists on
    /// Modal for that entrypoint's effective config (single-flighted per config
    /// key), then invoke it with `(entrypoint, input_json)` and return the runner's
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
        let (function_id, deadline) = self.resolve_function(handle, entrypoint).await?;

        // Invoke: two positional args (entrypoint, input_json), no kwargs. R=String
        // (the wrapper returns the runner stdout envelope verbatim).
        let empty_kwargs: std::collections::HashMap<String, ()> = std::collections::HashMap::new();
        let mut client = handle.client.lock().await;
        let envelope: String = client
            .invoke_cbor_with_deadline(
                &function_id,
                &(entrypoint, input_json),
                &empty_kwargs,
                deadline,
            )
            .await?;
        Ok(envelope)
    }

    /// Shared RUN-path head reused by `.remote()`/`.spawn()`/`.map()`: resolve (and
    /// memoize) the invokable wrapper `function_id` for `entrypoint`, applying its
    /// decorator gpu/timeout, and compute the output-poll `deadline`.
    ///
    /// Resolves the invoked entrypoint's decorator config and applies it to a
    /// per-call clone of the path config BEFORE `ensure_function`. The created
    /// wrapper is memoized by effective config, so entrypoints with identical
    /// gpu/timeout/cache/secrets/volumes share a Modal function while divergent
    /// entrypoints get separate functions.
    ///
    /// The deadline must cover the cold in-body `cargo build` (the RUN boundary):
    /// the first call to a fresh container compiles the whole dep tree, which can
    /// take many minutes — far past the SDK's 600s default. Match the function's own
    /// container timeout (honoring the decorator override, the same value
    /// `ensure_function` sets) plus a small queue/schedule buffer. spawn/map use
    /// the same keyed resolution path, so the deadline tracks the selected wrapper.
    async fn resolve_function(
        &self,
        handle: &RemoteHandle,
        entrypoint: &str,
    ) -> Result<(String, std::time::Duration)> {
        let cfg = self.config_for(entrypoint);
        let cfg_gpu: Option<String> = cfg.gpu.map(|s| s.to_string());
        let cfg_timeout: Option<u32> = cfg.timeout_secs;
        // P6 cache precedence: the decorator `#[function(cache=…)]` (explicit
        // `Some(_)`) OVERRIDES the env/default base; a bare `#[function]` (`None`)
        // defers to `run_config.cache` (folded from MODAL_RUST_NO_CACHE / default ON).
        // Matches the gpu/timeout override semantics.
        let cfg_cache: Option<bool> = cfg.cache;
        // USER secrets/volumes from the decorator: owned copies for the create.
        // Empty ⇒ no extras ⇒ wire-identical to before.
        let cfg_secrets: Vec<String> = cfg.secrets.iter().map(|s| s.to_string()).collect();
        let cfg_volumes: Vec<(String, String)> = cfg
            .volumes
            .iter()
            .map(|(m, n)| (m.to_string(), n.to_string()))
            .collect();

        let effective_cache = cfg_cache.unwrap_or(handle.config.cache);
        let effective_timeout = cfg_timeout.unwrap_or(handle.config.timeout_secs);
        let key = RunFunctionKey {
            entrypoint: entrypoint.to_string(),
            gpu: cfg_gpu.clone(),
            timeout_secs: effective_timeout,
            cache: effective_cache,
            secrets: cfg_secrets.clone(),
            volumes: cfg_volumes.clone(),
        };
        // Resolve (and memoize) the invokable function_id for this effective
        // config. The map lock is held only long enough to fetch/create the cell;
        // `get_or_try_init` then single-flights the Modal create for this key.
        let cell = {
            let mut function_ids = handle.function_ids.lock().await;
            function_ids
                .entry(key)
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };
        let function_id = cell
            .get_or_try_init(|| async {
                let mut run_config = handle.config.clone();
                run_config.gpu = cfg_gpu.clone();
                run_config.timeout_override_secs = cfg_timeout;
                run_config.cache = effective_cache;
                run_config.secrets = cfg_secrets.clone();
                run_config.volumes = cfg_volumes.clone();
                let mut client = handle.client.lock().await;
                // The publish set is the cumulative union across entrypoints (AppPublish
                // REPLACES the set), so lock it across the create so each per-entrypoint
                // create re-publishes every prior one too.
                let mut published = handle.published.lock().await;
                remote::ensure_function(
                    &mut client,
                    &handle.app_id,
                    &handle.app_name,
                    entrypoint,
                    &run_config,
                    &mut published,
                )
                .await
            })
            .await?
            .clone();

        let deadline = std::time::Duration::from_secs(effective_timeout as u64 + 120);
        Ok((function_id, deadline))
    }

    /// Fire-and-forget RUN-path spawn: ensure the wrapper exists (same head as
    /// `.remote()`), enqueue ONE input, and return its `function_call_id`
    /// IMMEDIATELY (no output wait). [`Function::spawn`](crate::Function::spawn)
    /// wraps the id in a [`FunctionCall`](crate::FunctionCall).
    pub(crate) async fn remote_spawn(
        &self,
        entrypoint: &str,
        input_json: String,
    ) -> Result<String> {
        let handle = self.remote.as_ref().ok_or_else(Error::not_connected)?;
        let (function_id, _deadline) = self.resolve_function(handle, entrypoint).await?;
        let empty_kwargs: std::collections::HashMap<String, ()> = std::collections::HashMap::new();
        let mut client = handle.client.lock().await;
        client
            .spawn_cbor(&function_id, &(entrypoint, input_json), &empty_kwargs)
            .await
            .map_err(Into::into)
    }

    /// Poll ONE output of a previously-spawned call by `function_call_id` + `index`,
    /// returning the runner's one-line JSON envelope VERBATIM (the caller parses it,
    /// exactly as `.remote()` does). The call id is self-describing, so no
    /// `function_id`/config resolution is needed — but the deadline must still cover
    /// the cold in-body `cargo build` the first spawned input pays, so it tracks the
    /// path timeout + buffer.
    pub(crate) async fn remote_get(
        &self,
        function_call_id: &str,
        index: i32,
        timeout: Option<std::time::Duration>,
    ) -> Result<String> {
        let handle = self.remote.as_ref().ok_or_else(Error::not_connected)?;
        let deadline = timeout.unwrap_or_else(|| {
            std::time::Duration::from_secs(handle.config.timeout_secs as u64 + 120)
        });
        let mut client = handle.client.lock().await;
        client
            .get_by_call_cbor::<String>(function_call_id, index, deadline)
            .await
            .map_err(Into::into)
    }

    /// Fan-out RUN-path map: ensure the wrapper exists (same head as `.remote()`),
    /// enqueue N inputs under one map call, and return the runner envelopes in INPUT
    /// ORDER (the SDK reorders by input ordinal). [`Function::map`](crate::Function::map)
    /// parses each envelope via the SAME taxonomy as `.local()`/`.remote()`.
    pub(crate) async fn remote_map(
        &self,
        entrypoint: &str,
        inputs_json: Vec<String>,
    ) -> Result<Vec<String>> {
        let handle = self.remote.as_ref().ok_or_else(Error::not_connected)?;
        let (function_id, deadline) = self.resolve_function(handle, entrypoint).await?;
        // Each input's args = (entrypoint, input_json_i), kwargs = {} — the SAME
        // shape `.remote()` sends, one per input. The element type matches
        // `map_cbor`'s `&[(A, K)]` slice (A = (entrypoint, json), K = empty map).
        let inputs: Vec<MapInput<'_>> = inputs_json
            .into_iter()
            .map(|j| ((entrypoint, j), std::collections::HashMap::new()))
            .collect();
        let mut client = handle.client.lock().await;
        client
            .map_cbor::<_, _, String>(&function_id, &inputs, deadline)
            .await
            .map_err(Into::into)
    }

    /// Run one entrypoint (the RUN path) and return the runner's one-line JSON
    /// envelope VERBATIM (P9 §B.3). A thin generic-free wrapper over the existing
    /// `pub(crate)` [`remote_invoke`](App::remote_invoke): the `modal-rust` CLI is
    /// generic over entrypoints (no typed `In`/`Out`), so it needs the raw envelope
    /// to print byte-for-byte and mirror `ok` → exit code. The typed
    /// [`Function::remote`](crate::Function::remote) path is unchanged.
    pub async fn remote_envelope(&self, entrypoint: &str, input_json: String) -> Result<String> {
        self.remote_invoke(entrypoint, input_json).await
    }

    /// Call a DEPLOYED entrypoint by app name and return the runner's one-line JSON
    /// envelope VERBATIM (NO build, NO upload — the deploy-call invariant). Reuses
    /// [`deploy::call_function`] exactly as [`App::call`] does, but returns the raw
    /// string for the generic-over-entrypoints CLI (P9 §B.3).
    pub async fn call_envelope(
        &self,
        app_name: &str,
        entrypoint: &str,
        input_json: String,
    ) -> Result<String> {
        let handle = self.remote.as_ref().ok_or_else(Error::not_connected)?;
        let mut client = handle.client.lock().await;
        deploy::call_function(&mut client, app_name, entrypoint, input_json).await
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
    /// Deploy now publishes ONE Modal function PER ENTRYPOINT (each named by its
    /// entrypoint, carrying its OWN gpu/timeout/secrets/volumes), over a single shared
    /// image that bakes the one `modal_runner` handling all entrypoints. Divergent
    /// per-entrypoint configs COEXIST — they are no longer rejected. The manual path
    /// (no decorator config) deploys a single default function, unchanged.
    pub async fn deploy_with(&self, config: DeployConfig) -> Result<DeployedApp> {
        let handle = self.remote.as_ref().ok_or_else(Error::not_connected)?;
        // Each decorated entrypoint becomes its OWN deployed function (object tag = the
        // entrypoint) with its OWN effective config. No divergence rejection.
        let entrypoints = self.deploy_entrypoints();
        let mut client = handle.client.lock().await;
        deploy::deploy_function(&mut client, &config, &entrypoints).await
    }

    /// Build the per-entrypoint deploy plan: one [`deploy::DeployEntrypoint`] per
    /// entrypoint (object tag = the entrypoint), carrying its effective
    /// gpu/timeout/secrets/volumes so `call(app, entrypoint)` resolves the right one.
    ///
    /// Source precedence: the decorator [`FunctionConfig`]s when present (the
    /// `#[function(...)]` path); else the registered handler NAMES with default config
    /// each (the manual `connect_with_registry` path — so each registered entrypoint
    /// is deployed under its own name). EMPTY only when there are NEITHER configs NOR
    /// handlers (the truly headless manifest path), where [`deploy::deploy_function`]
    /// falls back to a single default function under the wrapper callable. The
    /// run-only `cache` knob is irrelevant to deploy (image-build-time build).
    fn deploy_entrypoints(&self) -> Vec<deploy::DeployEntrypoint> {
        if !self.configs.is_empty() {
            return self
                .configs
                .iter()
                .map(|(name, cfg)| deploy::DeployEntrypoint {
                    name: name.clone(),
                    gpu: cfg.gpu.map(|s| s.to_string()),
                    timeout_secs: cfg.timeout_secs,
                    secrets: cfg.secrets.iter().map(|s| s.to_string()).collect(),
                    volumes: cfg
                        .volumes
                        .iter()
                        .map(|(m, n)| (m.to_string(), n.to_string()))
                        .collect(),
                })
                .collect();
        }
        // Manual path (registry handlers, no decorator config): deploy each registered
        // entrypoint under its own name with default config, so `call(app, name)`
        // resolves it.
        self.known_names()
            .into_iter()
            .map(|name| deploy::DeployEntrypoint {
                name,
                gpu: None,
                timeout_secs: None,
                secrets: Vec::new(),
                volumes: Vec::new(),
            })
            .collect()
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

    /// The app name the offline dump ([`App::dry_run`]) renders for the RUN path:
    /// the connected ephemeral app's name if this App is connected, else `fallback`
    /// (the config package — a bare, unconnected App has no app name). NO network.
    pub(crate) fn dump_app_name(&self, fallback: &str) -> String {
        self.remote
            .as_ref()
            .map(|h| h.app_name.clone())
            .unwrap_or_else(|| fallback.to_string())
    }

    /// The per-entrypoint deploy plan the offline DEPLOY dump
    /// ([`App::dump_deploy_manifest`]) renders — the SAME plan [`App::deploy_with`]
    /// passes to [`deploy::deploy_function`], with the empty-fallback applied so the
    /// dump shows the concrete functions (the manual path renders ONE default
    /// function under the wrapper callable). NO network.
    pub(crate) fn deploy_entrypoints_for_dump(
        &self,
        config: &DeployConfig,
    ) -> Vec<deploy::DeployEntrypoint> {
        let entrypoints = self.deploy_entrypoints();
        if entrypoints.is_empty() {
            vec![deploy::DeployEntrypoint {
                name: deploy::DEPLOY_WRAPPER_CALLABLE.to_string(),
                gpu: config.gpu.clone(),
                timeout_secs: config.timeout_override_secs,
                secrets: config.secrets.clone(),
                volumes: config.volumes.clone(),
            }]
        } else {
            entrypoints
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Link example-add-macro's inventory submissions (incl. the decorated `add_gpu`
    // with `gpu="T4", timeout=1800, cache=false`) into this test binary so
    // `App::local()` surfaces their FunctionConfig.
    use example_add_macro as _;

    #[test]
    fn local_captures_decorator_config() {
        let app = App::local();
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
    fn local_captures_secrets_and_volumes() {
        // The decorated `add_extras` (`secrets=["my-secret"], volumes=["/data=my-vol"]`)
        // flows through `config_for` so the RUN/DEPLOY paths can resolve + attach them.
        let app = App::local();
        let cfg = app.config_for("add_extras");
        assert_eq!(cfg.secrets, &["my-secret"]);
        assert_eq!(cfg.volumes, &[("/data", "my-vol")]);
        // A bare entrypoint carries no extras (empty), so it stays wire-identical.
        let bare = app.config_for("add");
        assert!(bare.secrets.is_empty());
        assert!(bare.volumes.is_empty());
    }

    #[test]
    fn deploy_publishes_distinct_function_per_divergent_entrypoint() {
        // NEW correct behavior (was `deploy_rejects_divergent_per_entrypoint_configs`):
        // deploy now publishes ONE Modal function PER ENTRYPOINT, each with its OWN
        // config, so divergent gpu/timeout are NO LONGER rejected — they coexist.
        let app = App::from_manifest([
            (
                "cpu".to_string(),
                modal_rust_runtime::FunctionConfig::default(),
            ),
            (
                "gpu".to_string(),
                modal_rust_runtime::FunctionConfig {
                    gpu: Some("T4"),
                    timeout_secs: Some(1800),
                    cache: None,
                    secrets: &[],
                    volumes: &[],
                },
            ),
        ]);
        let plan = app.deploy_entrypoints();
        assert_eq!(plan.len(), 2, "one deploy function per entrypoint");
        // BTreeMap orders by name: "cpu" then "gpu".
        let cpu = plan.iter().find(|e| e.name == "cpu").expect("cpu in plan");
        let gpu = plan.iter().find(|e| e.name == "gpu").expect("gpu in plan");
        // Each carries its OWN divergent config — no clobber, no rejection.
        assert_eq!(cpu.gpu, None);
        assert_eq!(cpu.timeout_secs, None);
        assert_eq!(gpu.gpu.as_deref(), Some("T4"));
        assert_eq!(gpu.timeout_secs, Some(1800));
    }

    #[test]
    fn deploy_plan_carries_identical_per_entrypoint_configs() {
        let cfg = modal_rust_runtime::FunctionConfig {
            gpu: Some("T4"),
            timeout_secs: Some(1800),
            cache: Some(false), // deploy ignores run-cache config
            secrets: &["my-secret"],
            volumes: &[("/data", "my-vol")],
        };
        let app = App::from_manifest([
            ("train".to_string(), cfg.clone()),
            ("eval".to_string(), cfg.clone()),
        ]);
        let plan = app.deploy_entrypoints();
        assert_eq!(plan.len(), 2);
        for ep in &plan {
            assert_eq!(ep.gpu.as_deref(), Some("T4"));
            assert_eq!(ep.timeout_secs, Some(1800));
            assert_eq!(ep.secrets, &["my-secret".to_string()]);
            assert_eq!(ep.volumes, &[("/data".to_string(), "my-vol".to_string())]);
        }
    }

    #[test]
    fn decorator_cache_override_precedence() {
        // Mirror the `resolve_function` precedence: `cfg_cache.unwrap_or(base)`.
        // `Some(false)` (an explicit `#[function(cache=false)]`) wins over either
        // base; `None` (bare `#[function]`) defers to the env/default base.
        let apply = |cfg_cache: Option<bool>, base: bool| cfg_cache.unwrap_or(base);

        // Decorator cache=false beats a default-ON base AND an OFF base.
        assert!(!apply(Some(false), true), "Some(false) overrides base ON");
        assert!(!apply(Some(false), false));
        // Decorator cache=true beats an OFF (env MODAL_RUST_NO_CACHE) base.
        assert!(apply(Some(true), false), "Some(true) overrides base OFF");
        // Bare decorator (None) defers to whatever the base is.
        assert!(apply(None, true), "None defers to base ON");
        assert!(!apply(None, false), "None defers to base OFF");

        // The decorated `add_gpu` entrypoint carries cache=Some(false) end-to-end, so
        // the RUN path will force cache off for it regardless of the env base.
        let app = App::local();
        assert_eq!(app.config_for("add_gpu").cache, Some(false));
        assert!(!apply(app.config_for("add_gpu").cache, true));
        // The bare `add` entrypoint defers (cache=None).
        assert_eq!(app.config_for("add").cache, None);
        assert!(apply(app.config_for("add").cache, true));
    }

    #[test]
    fn manual_local_with_registry_path_config_is_default() {
        // The manual `App::local_with_registry(registry)` path has NO decorator config
        // (empty configs map), so `config_for` returns the default for any name.
        let app = App::local_with_registry(Registry::new());
        assert_eq!(
            app.config_for("anything"),
            modal_rust_runtime::FunctionConfig::default()
        );
    }

    #[test]
    fn from_manifest_carries_config_but_is_headless() {
        // P9 §G.1: a headless App built from a manifest carries per-entrypoint
        // config but NO handlers (empty Registry). `config_for` surfaces the manifest
        // config; `known_names()` is empty (headless), so `.local()` would fail but
        // `.remote()`/`deploy`/`call` (which never touch handlers) work.
        let cfg = modal_rust_runtime::FunctionConfig {
            gpu: Some("A100"),
            timeout_secs: Some(900),
            cache: Some(true),
            secrets: &["my-secret"],
            volumes: &[("/data", "my-vol")],
        };
        let app = App::from_manifest([("add".to_string(), cfg.clone())]);
        assert_eq!(app.config_for("add"), cfg);
        assert!(app.known_names().is_empty(), "manifest App is headless");
        // An unknown name falls back to the default config.
        assert_eq!(
            app.config_for("missing"),
            modal_rust_runtime::FunctionConfig::default()
        );
    }

    #[test]
    fn from_manifest_default_config_roundtrips() {
        // P9 §G.1: a default-config entry round-trips to the all-None config.
        let app = App::from_manifest([(
            "add".to_string(),
            modal_rust_runtime::FunctionConfig::default(),
        )]);
        let c = app.config_for("add");
        assert_eq!(c.gpu, None);
        assert_eq!(c.timeout_secs, None);
        assert_eq!(c.cache, None);
        assert!(app.known_names().is_empty());
    }
}
