//! The [`App`] handle: owns a [`Registry`] and (optionally) a live remote handle.
//!
//! Every constructor and accessor here is sync, zero-Modal, and zero-network, so
//! `.local()` works without ever calling [`App::connect`]. `connect()` builds a
//! real `sdk::ModalClient` for the future remote path, but no unit/integration
//! test calls it, so the offline gates stay green.

#[cfg(feature = "client")]
use std::collections::BTreeMap;
#[cfg(feature = "client")]
use std::sync::Arc;

#[cfg(feature = "client")]
use tokio::sync::{Mutex, OnceCell};

#[cfg(feature = "client")]
use crate::deploy::{self, DeployConfig, DeployedApp};
#[cfg(feature = "client")]
use crate::remote::{self, RemoteConfig};
use crate::{Error, Function, FunctionOptions, Registry, Result};

/// One `map` input as the SDK's `map_cbor` expects it: `(args, kwargs)` where
/// `args = (entrypoint, input_json)` (the SAME 2-tuple `.remote()` sends) and
/// `kwargs` is the empty map. Aliased to keep [`App::remote_map`]'s annotation
/// readable (clippy `type_complexity`). Client-only.
#[cfg(feature = "client")]
type MapInput<'a> = ((&'a str, String), std::collections::HashMap<String, ()>);

/// Memo key for created RUN-path Modal functions. Each ENTRYPOINT gets its OWN Modal
/// function (object tag = the entrypoint) carrying its OWN effective config, so the
/// key is the entrypoint name PLUS its effective gpu/timeout/cache/secrets/volumes.
/// Including the config means a (hypothetical) per-call config change re-creates
/// rather than silently reusing a stale wrapper. Client-only.
#[cfg(feature = "client")]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RunFunctionKey {
    entrypoint: String,
    options: FunctionOptions,
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
    /// (no decorator => facade defaults apply via [`App::config_for`]). Read only by
    /// the client surface (`.remote()`/`deploy`/dump) + tests, so the LIGHT build
    /// allows it dead.
    #[cfg_attr(not(feature = "client"), allow(dead_code))]
    configs: std::collections::BTreeMap<String, FunctionOptions>,
    /// `None` until [`App::connect`]; the live control-plane handle `.remote()`
    /// consumes. `.local()` never touches it. Client-only field: the LIGHT build has
    /// no client, so the field is absent and the `local*`/`from_manifest` constructors
    /// omit it.
    #[cfg(feature = "client")]
    remote: Option<RemoteHandle>,
}

/// A live control-plane handle, built by [`App::connect`]. Private — `.remote()`
/// drives it through [`App::remote_invoke`]. Client-only.
#[cfg(feature = "client")]
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
    published: Mutex<crate::control_plane::Published>,
    /// RUN-path knobs (source dir, package, image, timeout, ignore set).
    config: RemoteConfig,
}

impl App {
    /// Build an offline (in-process, no Modal) app from an explicit [`Registry`]
    /// (manual builder path — e.g. `example_add::modal_registry()`). Zero Modal,
    /// zero network.
    ///
    /// The manual path has NO decorator config: `configs` is empty, so
    /// [`App::config_for`] returns `FunctionOptions::default()` (all `None`) and the
    /// facade falls back to its path defaults — behavior preserved.
    pub fn local_with_registry(registry: Registry) -> Self {
        App {
            registry,
            configs: std::collections::BTreeMap::new(),
            #[cfg(feature = "client")]
            remote: None,
        }
    }

    /// Build an offline (in-process, no Modal) app over the functions decorated
    /// with `#[modal_rust::function]`, ALSO capturing each entrypoint's decorator
    /// owned [`FunctionOptions`]. Zero Modal, zero network.
    pub fn local() -> Self {
        let (registry, configs) = crate::from_inventory_with_configs();
        App {
            registry,
            configs: FunctionOptions::by_name(configs),
            #[cfg(feature = "client")]
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
    #[cfg(feature = "client")]
    pub async fn connect(name: &str) -> Result<Self> {
        let (registry, configs) = crate::from_inventory_with_configs();
        let configs = FunctionOptions::by_name(configs);
        // PACKAGE AUTO-DETECT: the `#[modal_rust::function]` macro captured the
        // user's `env!("CARGO_PKG_NAME")` into each inventory `Registration`. Thread
        // it into the RUN config so `cargo build -p <pkg>` targets the user's crate
        // WITHOUT them setting `MODAL_RUST_PACKAGE`. `MODAL_RUST_PACKAGE` still
        // OVERRIDES (it is applied first inside `RemoteConfig::default()`), so this
        // only fills in the package when the env var is unset.
        let run_config = RemoteConfig::default().with_detected_package(
            std::env::var("MODAL_RUST_PACKAGE").ok().as_deref(),
            crate::package_from_inventory(),
        );
        App::connect_inner(name, registry, configs, run_config).await
    }

    /// As [`App::connect`], but combines an explicit [`Registry`] with a live
    /// remote handle. The manual path has NO decorator config (EMPTY `configs` =>
    /// facade defaults). The `app_id` is resolved in the configured environment
    /// (defaults to `"main"`).
    #[cfg(feature = "client")]
    pub async fn connect_with_registry(name: &str, registry: Registry) -> Result<Self> {
        App::connect_inner(
            name,
            registry,
            std::collections::BTreeMap::new(),
            RemoteConfig::default(),
        )
        .await
    }

    /// LIGHT-build stub for [`connect`](App::connect): without the `client` feature
    /// there is no gRPC client, so connecting to Modal returns a clear error. A
    /// function-only crate compiles either way; only an actual connect attempt fails.
    #[cfg(not(feature = "client"))]
    pub async fn connect(_name: &str) -> Result<Self> {
        Err(Error::client_feature("App::connect"))
    }

    /// LIGHT-build stub for [`connect_with_registry`](App::connect_with_registry).
    #[cfg(not(feature = "client"))]
    pub async fn connect_with_registry(_name: &str, _registry: Registry) -> Result<Self> {
        Err(Error::client_feature("App::connect_with_registry"))
    }

    /// Build a HEADLESS [`App`] from a `--describe` manifest: per-entrypoint config
    /// but NO handlers (empty [`Registry`]). `.local()` would fail (no handler), but
    /// `.remote()`/`deploy`/`call` never need handlers — they read only
    /// [`config_for`](App::config_for) + the SDK ops (P9 §B.1). Used by the
    /// `modal-rust` CLI, which cannot link the user crate.
    ///
    /// Zero Modal, zero network — pair with
    /// [`connect_from_manifest`](App::connect_from_manifest) for the live handle.
    pub fn from_manifest<I, O>(configs: I) -> Self
    where
        I: IntoIterator<Item = (String, O)>,
        O: Into<FunctionOptions>,
    {
        App {
            registry: Registry::new(),
            configs: FunctionOptions::by_name(configs),
            #[cfg(feature = "client")]
            remote: None,
        }
    }

    /// As [`App::connect`], but seeds an EMPTY [`Registry`] + the manifest configs +
    /// an EXPLICIT [`RemoteConfig`] (built by the CLI from the real workspace_root +
    /// package), instead of `connect_inner`'s hardcoded `RemoteConfig::default()`
    /// (which would (mis)discover `local_root`/`package` from the CLI's arbitrary
    /// CWD). Headless: no handlers, so only `.remote()`/`deploy`/`call` work (P9 §B).
    ///
    /// Client-only (it takes a [`RemoteConfig`], a client-gated type, and connects):
    /// no light stub — the only caller is the `modal-rust` CLI, which enables `client`.
    #[cfg(feature = "client")]
    pub async fn connect_from_manifest<I, O>(
        name: &str,
        configs: I,
        run_config: RemoteConfig,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = (String, O)>,
        O: Into<FunctionOptions>,
    {
        App::connect_inner(
            name,
            Registry::new(),
            FunctionOptions::by_name(configs),
            run_config,
        )
        .await
    }

    /// Shared connect body: build the ephemeral-app client handle, store the
    /// supplied per-entrypoint `configs`, and seed the EXPLICIT `run_config` (the
    /// only delta between `connect`/`connect_with_registry` — which pass
    /// `RemoteConfig::default()` — and the CLI's `connect_from_manifest`, which
    /// supplies a workspace-scoped config).
    #[cfg(feature = "client")]
    async fn connect_inner(
        name: &str,
        registry: Registry,
        configs: std::collections::BTreeMap<String, FunctionOptions>,
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
    #[cfg(feature = "client")]
    async fn connect_inner_with_client(
        name: &str,
        registry: Registry,
        configs: std::collections::BTreeMap<String, FunctionOptions>,
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
                published: Mutex::new(crate::control_plane::Published::default()),
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
    #[cfg(all(any(test, feature = "testkit"), feature = "client"))]
    pub async fn connect_at(name: &str, registry: Registry, server_url: String) -> Result<Self> {
        Self::connect_at_with(name, registry, server_url, RemoteConfig::default()).await
    }

    /// As [`connect_at`](App::connect_at), plus an explicit [`RemoteConfig`]
    /// (gpu/timeout/source dir/etc.) — the table-test entry point: each case builds
    /// its own mock + its own per-case `RemoteConfig` and asserts the captured
    /// `FunctionCreate` manifest.
    #[cfg(all(any(test, feature = "testkit"), feature = "client"))]
    pub async fn connect_at_with(
        name: &str,
        registry: Registry,
        server_url: String,
        run_config: RemoteConfig,
    ) -> Result<Self> {
        Self::connect_at_with_configs(
            name,
            registry,
            std::collections::BTreeMap::<String, FunctionOptions>::new(),
            server_url,
            run_config,
        )
        .await
    }

    /// As [`connect_at_with`](App::connect_at_with), but ALSO threads per-entrypoint
    /// decorator [`FunctionOptions`] — the gpu/timeout/secrets/volumes the RUN path
    /// resolves via [`config_for`](App::config_for). This is the table-test entry
    /// point that drives the manifest the SAME way a `#[function(gpu=.., timeout=..)]`
    /// decorator would (the RUN path re-derives gpu/timeout from the decorator config,
    /// not the bare `RemoteConfig`, so a faithful table must supply them here).
    #[cfg(all(any(test, feature = "testkit"), feature = "client"))]
    pub async fn connect_at_with_configs<I, O>(
        name: &str,
        registry: Registry,
        configs: I,
        server_url: String,
        run_config: RemoteConfig,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = (String, O)>,
        O: Into<FunctionOptions>,
    {
        let config = modal_rust_sdk::ModalConfig {
            profile: "mock".into(),
            server_url,
            token_id: "ak-mock".into(),
            token_secret: "as-mock".into(),
            environment: Some("main".into()),
            image_builder_version: None,
        };
        let client = modal_rust_sdk::ModalClient::from_config(config).await?;
        Self::connect_inner_with_client(
            name,
            registry,
            FunctionOptions::by_name(configs),
            run_config,
            client,
        )
        .await
    }

    /// Resolve the decorator [`FunctionOptions`] for `name`. Returns
    /// `FunctionOptions::default()` (all `None`) for the manual path or an unknown
    /// name, so the facade's path defaults apply.
    #[cfg_attr(not(feature = "client"), allow(dead_code))]
    pub(crate) fn config_for(&self, name: &str) -> FunctionOptions {
        self.configs.get(name).cloned().unwrap_or_default()
    }

    /// Drive the RUN path for one entrypoint: ensure a wrapper function exists on
    /// Modal for that entrypoint's effective config (single-flighted per config
    /// key), then invoke it with `(entrypoint, input_json)` and return the runner's
    /// one-line JSON envelope string. The caller ([`Function::remote`]) parses it.
    ///
    /// `cargo build` runs in the function body at invoke time (the RUN boundary);
    /// this method only orchestrates the control plane + the CBOR round-trip.
    #[cfg(feature = "client")]
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
    /// wrapper is memoized by entrypoint plus effective config, so each entrypoint
    /// has its own Modal object tag and a future config change cannot silently reuse
    /// a stale function.
    ///
    /// The deadline must cover the cold in-body `cargo build` (the RUN boundary):
    /// the first call to a fresh container compiles the whole dep tree, which can
    /// take many minutes — far past the SDK's 600s default. Match the function's own
    /// container timeout (honoring the decorator override, the same value
    /// `ensure_function` sets) plus a small queue/schedule buffer. spawn/map use
    /// the same keyed resolution path, so the deadline tracks the selected wrapper.
    #[cfg(feature = "client")]
    async fn resolve_function(
        &self,
        handle: &RemoteHandle,
        entrypoint: &str,
    ) -> Result<(String, std::time::Duration)> {
        let mut options = self.config_for(entrypoint);
        // P6 cache precedence: the decorator `#[function(cache=…)]` (explicit
        // `Some(_)`) OVERRIDES the env/default base; a bare `#[function]` (`None`)
        // defers to `run_config.cache` (folded from MODAL_RUST_NO_CACHE / default ON).
        // Matches the gpu/timeout override semantics.
        let effective_cache = options.cache.unwrap_or(handle.config.cache);
        let effective_timeout = options.timeout_secs.unwrap_or(handle.config.timeout_secs);
        options.cache = Some(effective_cache);
        options.timeout_secs = Some(effective_timeout);
        let key = RunFunctionKey {
            entrypoint: entrypoint.to_string(),
            options: options.clone(),
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
                run_config.options = options.clone();
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
    #[cfg(feature = "client")]
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
    #[cfg(feature = "client")]
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
    #[cfg(feature = "client")]
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

    /// Fire-and-forget fan-out RUN-path spawn_map: ensure the wrapper exists (same
    /// head as `.remote()`), enqueue N inputs under ONE async MAP call, and return
    /// the map call's `function_call_id` IMMEDIATELY (no output wait — results are
    /// not collected). [`Function::spawn_map`](crate::Function::spawn_map) wraps the
    /// id in a [`FunctionCall`](crate::FunctionCall).
    #[cfg(feature = "client")]
    pub(crate) async fn remote_spawn_map(
        &self,
        entrypoint: &str,
        inputs_json: Vec<String>,
    ) -> Result<String> {
        let handle = self.remote.as_ref().ok_or_else(Error::not_connected)?;
        let (function_id, _deadline) = self.resolve_function(handle, entrypoint).await?;
        // Same per-input args shape as `.remote()`/`.map()`: (entrypoint, json) with
        // empty kwargs, one per input.
        let inputs: Vec<MapInput<'_>> = inputs_json
            .into_iter()
            .map(|j| ((entrypoint, j), std::collections::HashMap::new()))
            .collect();
        let mut client = handle.client.lock().await;
        let (function_call_id, _n) = client.spawn_map_cbor(&function_id, &inputs).await?;
        Ok(function_call_id)
    }

    /// Run one entrypoint (the RUN path) and return the runner's one-line JSON
    /// envelope VERBATIM (P9 §B.3). A thin generic-free wrapper over the existing
    /// `pub(crate)` [`remote_invoke`](App::remote_invoke): the `modal-rust` CLI is
    /// generic over entrypoints (no typed `In`/`Out`), so it needs the raw envelope
    /// to print byte-for-byte and mirror `ok` → exit code. The typed
    /// [`Function::remote`](crate::Function::remote) path is unchanged.
    #[cfg(feature = "client")]
    pub async fn remote_envelope(&self, entrypoint: &str, input_json: String) -> Result<String> {
        self.remote_invoke(entrypoint, input_json).await
    }

    /// Call a DEPLOYED entrypoint by app name and return the runner's one-line JSON
    /// envelope VERBATIM (NO build, NO upload — the deploy-call invariant). Reuses
    /// [`deploy::call_function`] exactly as [`App::call`] does, but returns the raw
    /// string for the generic-over-entrypoints CLI (P9 §B.3).
    #[cfg(feature = "client")]
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
    #[cfg(feature = "client")]
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
    #[cfg(feature = "client")]
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
    /// Source precedence: the decorator [`FunctionOptions`] when present (the
    /// `#[function(...)]` path); else the registered handler NAMES with default config
    /// each (the manual `connect_with_registry` path — so each registered entrypoint
    /// is deployed under its own name). EMPTY only when there are NEITHER configs NOR
    /// handlers (the truly headless manifest path), where [`deploy::deploy_function`]
    /// falls back to a single default function under the wrapper callable. The
    /// run-only `cache` knob is irrelevant to deploy (image-build-time build).
    #[cfg(feature = "client")]
    fn deploy_entrypoints(&self) -> Vec<deploy::DeployEntrypoint> {
        if !self.configs.is_empty() {
            return self
                .configs
                .iter()
                .map(|(name, cfg)| deploy::DeployEntrypoint {
                    name: name.clone(),
                    options: cfg.clone(),
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
                options: FunctionOptions::default(),
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
    #[cfg(feature = "client")]
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
    #[cfg(feature = "client")]
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
    #[cfg(feature = "client")]
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
    #[cfg(feature = "client")]
    pub(crate) fn deploy_entrypoints_for_dump(
        &self,
        config: &DeployConfig,
    ) -> Vec<deploy::DeployEntrypoint> {
        let entrypoints = self.deploy_entrypoints();
        if entrypoints.is_empty() {
            vec![deploy::DeployEntrypoint {
                name: deploy::DEPLOY_WRAPPER_CALLABLE.to_string(),
                options: config.options.clone(),
            }]
        } else {
            entrypoints
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FunctionConfig, HandlerFn, Registration, RunnerError};

    // Test-only macro-style registrations for this crate's own inventory.
    //
    // `App::local()` collects `Registration` records from the current `modal-rust`
    // crate instance. In these unit tests, `example-add-macro` would depend on a
    // separate `modal-rust` crate instance, so importing that example would not give
    // this test module any records to collect. Submit the records here instead.
    //
    // The three records model the macro cases these tests need: bare config,
    // gpu/timeout/cache config, and secrets/volumes config. The handler body is
    // irrelevant; these tests assert config discovery, not dispatch behavior.
    fn test_handler(_input: &[u8]) -> std::result::Result<Vec<u8>, RunnerError> {
        Ok(b"null".to_vec())
    }

    inventory::submit! {
        Registration {
            name: "add",
            handler: test_handler as HandlerFn,
            check: None,
            config: FunctionConfig::new(),
            package: "modal-rust",
        }
    }

    inventory::submit! {
        Registration {
            name: "add_gpu",
            handler: test_handler as HandlerFn,
            check: None,
            config: FunctionConfig {
                gpu: Some("T4"),
                timeout_secs: Some(1800),
                cache: Some(false),
                milli_cpu: Some(2000),
                memory_mb: Some(4096),
                secrets: &[],
                volumes: &[],
                retries: Some(3),
                schedule: Some("cron:UTC:0 9 * * 1"),
                min_containers: None,
                max_containers: None,
                buffer_containers: None,
                scaledown_window: None,
            },
            package: "modal-rust",
        }
    }

    inventory::submit! {
        Registration {
            name: "add_extras",
            handler: test_handler as HandlerFn,
            check: None,
            config: FunctionConfig {
                gpu: None,
                timeout_secs: None,
                cache: None,
                milli_cpu: None,
                memory_mb: None,
                secrets: &["my-secret"],
                volumes: &[("/data", "my-vol")],
                retries: None,
                schedule: None,
                min_containers: None,
                max_containers: None,
                buffer_containers: None,
                scaledown_window: None,
            },
            package: "modal-rust",
        }
    }

    #[test]
    fn local_captures_decorator_config() {
        let app = App::local();
        // The decorated entrypoint's config flows through `config_for`.
        let gpu_cfg = app.config_for("add_gpu");
        assert_eq!(gpu_cfg.gpu.as_deref(), Some("T4"));
        assert_eq!(gpu_cfg.timeout_secs, Some(1800));
        assert_eq!(gpu_cfg.cache, Some(false));
        // cpu/memory ride through config_for exactly like gpu/timeout.
        assert_eq!(gpu_cfg.milli_cpu, Some(2000));
        assert_eq!(gpu_cfg.memory_mb, Some(4096));
        // retries ride through config_for exactly like gpu/timeout.
        assert_eq!(gpu_cfg.retries, Some(3));
        // schedule rides through config_for exactly like gpu/timeout.
        assert_eq!(gpu_cfg.schedule.as_deref(), Some("cron:UTC:0 9 * * 1"));
        // The bare decorated entrypoint has the default (all-None) config.
        let bare = app.config_for("add");
        assert_eq!(bare, FunctionOptions::default());
        assert_eq!(bare.retries, None);
        assert_eq!(bare.schedule, None);
    }

    #[test]
    fn local_captures_secrets_and_volumes() {
        // The decorated `add_extras` (`secrets=["my-secret"], volumes=["/data=my-vol"]`)
        // flows through `config_for` so the RUN/DEPLOY paths can resolve + attach them.
        let app = App::local();
        let cfg = app.config_for("add_extras");
        assert_eq!(cfg.secrets, vec!["my-secret".to_string()]);
        assert_eq!(
            cfg.volumes,
            vec![("/data".to_string(), "my-vol".to_string())]
        );
        // A bare entrypoint carries no extras (empty), so it stays wire-identical.
        let bare = app.config_for("add");
        assert!(bare.secrets.is_empty());
        assert!(bare.volumes.is_empty());
    }

    #[test]
    #[cfg(feature = "client")]
    fn deploy_publishes_distinct_function_per_divergent_entrypoint() {
        // NEW correct behavior (was `deploy_rejects_divergent_per_entrypoint_configs`):
        // deploy now publishes ONE Modal function PER ENTRYPOINT, each with its OWN
        // config, so divergent gpu/timeout are NO LONGER rejected — they coexist.
        let app = App::from_manifest([
            ("cpu".to_string(), FunctionConfig::default()),
            (
                "gpu".to_string(),
                FunctionConfig {
                    gpu: Some("T4"),
                    timeout_secs: Some(1800),
                    cache: None,
                    milli_cpu: None,
                    memory_mb: None,
                    secrets: &[],
                    volumes: &[],
                    retries: None,
                    schedule: None,
                    min_containers: None,
                    max_containers: None,
                    buffer_containers: None,
                    scaledown_window: None,
                },
            ),
        ]);
        let plan = app.deploy_entrypoints();
        assert_eq!(plan.len(), 2, "one deploy function per entrypoint");
        // BTreeMap orders by name: "cpu" then "gpu".
        let cpu = plan.iter().find(|e| e.name == "cpu").expect("cpu in plan");
        let gpu = plan.iter().find(|e| e.name == "gpu").expect("gpu in plan");
        // Each carries its OWN divergent config — no clobber, no rejection.
        assert_eq!(cpu.options.gpu, None);
        assert_eq!(cpu.options.timeout_secs, None);
        assert_eq!(gpu.options.gpu.as_deref(), Some("T4"));
        assert_eq!(gpu.options.timeout_secs, Some(1800));
    }

    #[test]
    #[cfg(feature = "client")]
    fn deploy_plan_carries_identical_per_entrypoint_configs() {
        let cfg = FunctionConfig {
            gpu: Some("T4"),
            timeout_secs: Some(1800),
            cache: Some(false), // deploy ignores run-cache config
            milli_cpu: None,
            memory_mb: None,
            secrets: &["my-secret"],
            volumes: &[("/data", "my-vol")],
            retries: None,
            schedule: None,
            min_containers: None,
            max_containers: None,
            buffer_containers: None,
            scaledown_window: None,
        };
        let app = App::from_manifest([
            ("train".to_string(), cfg.clone()),
            ("eval".to_string(), cfg.clone()),
        ]);
        let plan = app.deploy_entrypoints();
        assert_eq!(plan.len(), 2);
        for ep in &plan {
            assert_eq!(ep.options.gpu.as_deref(), Some("T4"));
            assert_eq!(ep.options.timeout_secs, Some(1800));
            assert_eq!(ep.options.secrets, &["my-secret".to_string()]);
            assert_eq!(
                ep.options.volumes,
                vec![("/data".to_string(), "my-vol".to_string())]
            );
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
        assert_eq!(app.config_for("anything"), FunctionOptions::default());
    }

    #[test]
    fn from_manifest_carries_config_but_is_headless() {
        // P9 §G.1: a headless App built from a manifest carries per-entrypoint
        // config but NO handlers (empty Registry). `config_for` surfaces the manifest
        // config; `known_names()` is empty (headless), so `.local()` would fail but
        // `.remote()`/`deploy`/`call` (which never touch handlers) work.
        let cfg = FunctionConfig {
            gpu: Some("A100"),
            timeout_secs: Some(900),
            cache: Some(true),
            milli_cpu: Some(4000),
            memory_mb: Some(8192),
            secrets: &["my-secret"],
            volumes: &[("/data", "my-vol")],
            retries: Some(5),
            schedule: Some("period:days=1"),
            min_containers: None,
            max_containers: None,
            buffer_containers: None,
            scaledown_window: None,
        };
        let app = App::from_manifest([("add".to_string(), cfg.clone())]);
        assert_eq!(app.config_for("add"), FunctionOptions::from(&cfg));
        assert!(app.known_names().is_empty(), "manifest App is headless");
        // An unknown name falls back to the default config.
        assert_eq!(app.config_for("missing"), FunctionOptions::default());
    }

    #[test]
    fn from_manifest_default_config_roundtrips() {
        // P9 §G.1: a default-config entry round-trips to the all-None config.
        let app = App::from_manifest([("add".to_string(), FunctionConfig::default())]);
        let c = app.config_for("add");
        assert_eq!(c.gpu, None);
        assert_eq!(c.timeout_secs, None);
        assert_eq!(c.cache, None);
        assert!(app.known_names().is_empty());
    }
}
