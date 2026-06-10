//! Facade-owned distributed registration for `#[modal_rust::function]`.
//!
//! The runtime crate owns only dispatch (`name` + `HandlerFn`). This module owns
//! the atomic macro-discovery record that pairs dispatch with Modal control-plane
//! metadata, so an inventory user submits one record or none.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{HandlerFn, Registry};
use modal_rust_runtime::{CheckFn, PrimeFn};

/// Per-function deploy/run CONFIG sourced from
/// `#[modal_rust::function(gpu=..., timeout=..., cache=...)]`.
///
/// This type is facade-owned control-plane metadata. The runner dispatch path
/// ignores it; only facade/CLI code reads it when creating Modal functions or
/// emitting the additive `--describe` manifest.
///
/// `gpu` is `Option<&'static str>` (not `String`) because
/// `inventory::submit!` builds a `static` initializer. The same const-initializer
/// constraint is why `secrets`/`volumes` are `&'static` slices.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FunctionConfig {
    /// GPU spec string, Modal-format (`"T4"`, `"A100"`, `"A100-80GB"`, `"H100:4"`).
    /// `None` => CPU.
    pub gpu: Option<&'static str>,
    /// Function timeout (seconds). `None` => facade default.
    pub timeout_secs: Option<u32>,
    /// Cache hint. `None` => default. Reserved/inert for P4 (no proto target yet).
    pub cache: Option<bool>,
    /// Requested CPU in MILLI-cores (`cpu = 2.0` ⇒ `2000`). `None` => server default
    /// (`milli_cpu = 0`). Already resolved to wire units by the macro
    /// (`int(1000 * cpu)`), so this stays a plain `Option<u32>` const-valid in the
    /// `static` `inventory::submit!` initializer.
    pub milli_cpu: Option<u32>,
    /// Requested memory in MEBIBYTES (`memory = 4096`). `None` => server default
    /// (`memory_mb = 0`).
    pub memory_mb: Option<u32>,
    /// Named Modal secrets to attach.
    pub secrets: &'static [&'static str],
    /// Asserted-present keys on the named `secrets` (`required_keys = ["K", ..]`). One
    /// flat list applied to ALL named secrets (v0); the facade passes it to
    /// `Secret.from_name`, and Modal errors if a key is missing. Empty (the default) =>
    /// no assertion, byte-identical to before.
    pub required_keys: &'static [&'static str],
    /// Inline secret key/values (`env = {"K" = "V", ..}`, mirroring Modal's
    /// `app.function(env=..)` → `Secret.from_dict`). Empty (the default) => no inline
    /// secret, byte-identical to before. When non-empty the facade resolves these via a
    /// deterministic per-entrypoint `Secret.from_dict` (CREATE_IF_MISSING) and attaches
    /// the resulting id to the SAME `secret_ids` list named `secrets` use (so `env` and
    /// `secrets` compose). `&'static` slice (const-valid in the `static` initializer,
    /// like `volumes`).
    pub env: &'static [(&'static str, &'static str)],
    /// User-volume mounts to attach as `(mount_path, volume_name)` pairs.
    pub volumes: &'static [(&'static str, &'static str)],
    /// Automatic retry COUNT (`retries = N`). `None` => no retry policy (server
    /// default: a failed call is not retried). A bare count maps to Modal's
    /// fixed-interval policy (`backoff = 1.0`, 1s initial / 60s max delay) when the
    /// facade builds the `FunctionCreate`. Plain `Option<u32>`, const-valid in the
    /// `static` `inventory::submit!` initializer (like `timeout`).
    pub retries: Option<u32>,
    /// Custom retry-policy SPEC string (`retries = Retries(max_retries = N, ..)`, the
    /// STRUCT form). `None` => use the bare-int `retries` shortcut (or no policy). The
    /// macro canonicalizes the `Retries(..)` form to a `&'static str` SPEC the SDK's
    /// `parse_retries_spec` reads (custom backoff/initial/max delay) when the facade
    /// builds the `FunctionCreate`; const-valid in the `static` initializer (like
    /// `schedule`). Mutually exclusive with `retries` (the macro emits at most one).
    pub retries_spec: Option<&'static str>,
    /// Run-SCHEDULE spec string (`schedule = Cron(..)/Period(..)`). `None` => no
    /// schedule (the function is invoked only by callers). The macro canonicalizes the
    /// `Cron`/`Period` form to a `&'static str` SPEC the SDK's `parse_schedule` reads
    /// when the facade builds the `FunctionCreate`; const-valid in the `static`
    /// `inventory::submit!` initializer (like `gpu`).
    pub schedule: Option<&'static str>,
    /// Autoscaler floor (`min_containers = N`): minimum containers to keep running, so
    /// requests never wait for a cold start. `None` => scale to zero. Plain
    /// `Option<u32>`, const-valid in the `static` `inventory::submit!` initializer.
    pub min_containers: Option<u32>,
    /// Autoscaler ceiling (`max_containers = N`): cap on concurrent containers. `None`
    /// => no client-set ceiling.
    pub max_containers: Option<u32>,
    /// Warm buffer (`buffer_containers = N`): extra idle containers kept beyond demand
    /// to absorb bursts. `None` => no buffer.
    pub buffer_containers: Option<u32>,
    /// Idle-before-scaledown window in seconds (`scaledown_window = N`): how long an
    /// idle container waits before being scaled down. `None` => server default.
    pub scaledown_window: Option<u32>,
    /// Per-function IMAGE spec string (`image = Image(base = .., apt = [..], ..)`, the
    /// struct form). `None` => the path's env-driven base image + no extra steps
    /// (byte-identical to before). The macro canonicalizes the `Image(..)` form to a
    /// compact JSON `&'static str` the facade parses via
    /// [`crate::remote::parse_image_spec`] (base image / install_rust / apt/pip/run
    /// `ImageStep`s), folded into the build config for THIS entrypoint's image. Lets a
    /// function declare its OWN image (e.g. a GPU function's CUDA base) instead of the
    /// env-only `MODAL_RUST_BASE_IMAGE`. Const-valid in the `static` initializer like
    /// `gpu`/`schedule`.
    pub image: Option<&'static str>,
    /// Memory-snapshot opt-in (`enable_memory_snapshot = true`), `#[cls]`-only in v0.
    /// `false` (the default) ⇒ inert ⇒ byte-identical to before. The facade only lets it
    /// take effect on the DEPLOY boundary (Modal snapshots deployed apps), so a RUN stays
    /// wire-identical even when the decorator opts in. A bare `bool`, const-valid in the
    /// `static` `inventory::submit!` initializer (like `cache`).
    pub enable_memory_snapshot: bool,
    /// Web-endpoint HTTP method (`#[endpoint(method = "POST")]`). `None` (the default)
    /// ⇒ not an endpoint ⇒ byte-identical to before web endpoints. The facade only lets
    /// it take effect on the DEPLOY boundary (the URL is deploy-only in v0), so a RUN
    /// stays wire-identical even when the decorator opts in — exactly like
    /// `enable_memory_snapshot`. `Option<&'static str>` (not `String`), const-valid in
    /// the `static` `inventory::submit!` initializer (like `gpu`/`schedule`).
    pub webhook_method: Option<&'static str>,
    /// Modal proxy-auth opt-in for the endpoint (`#[endpoint(.., requires_proxy_auth =
    /// true)]`). `false` (the default, matching Modal) = public URL; `true` = Modal
    /// rejects requests lacking the `Modal-Key`/`Modal-Secret` proxy-auth header pair
    /// BEFORE they reach the container. Inert unless `webhook_method` is set. A bare
    /// `bool`, const-valid in the `static` initializer (like `enable_memory_snapshot`).
    pub webhook_requires_proxy_auth: bool,
}

impl FunctionConfig {
    /// A `const` all-default config usable in a `static` `inventory::submit!`
    /// initializer.
    pub const fn new() -> Self {
        FunctionConfig {
            gpu: None,
            timeout_secs: None,
            cache: None,
            milli_cpu: None,
            memory_mb: None,
            secrets: &[],
            required_keys: &[],
            env: &[],
            volumes: &[],
            retries: None,
            retries_spec: None,
            schedule: None,
            min_containers: None,
            max_containers: None,
            buffer_containers: None,
            scaledown_window: None,
            image: None,
            enable_memory_snapshot: false,
            webhook_method: None,
            webhook_requires_proxy_auth: false,
        }
    }
}

/// Owned per-function deploy/run options after leaving the static inventory
/// boundary.
///
/// `FunctionConfig` exists only because `inventory::submit!` needs a
/// const-constructible static initializer. The facade converts that borrowed shape
/// into this owned domain type exactly once, then run/deploy/CLI code carries
/// `FunctionOptions` instead of re-declaring `gpu`/`timeout`/`cache`/`secrets`/
/// `volumes` in each layer.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FunctionOptions {
    /// GPU spec string, Modal-format (`"T4"`, `"A100"`, `"A100-80GB"`, `"H100:4"`).
    /// `None` => CPU.
    pub gpu: Option<String>,
    /// Function timeout (seconds). `None` => path default.
    pub timeout_secs: Option<u32>,
    /// Cache hint. `None` => path default.
    pub cache: Option<bool>,
    /// Requested CPU in MILLI-cores. `None` => server default.
    #[serde(default)]
    pub milli_cpu: Option<u32>,
    /// Requested memory in MEBIBYTES. `None` => server default.
    #[serde(default)]
    pub memory_mb: Option<u32>,
    /// Named Modal secrets to attach.
    #[serde(default)]
    pub secrets: Vec<String>,
    /// Asserted-present keys on the named `secrets` (`required_keys = [..]`). Empty =>
    /// no assertion.
    #[serde(default)]
    pub required_keys: Vec<String>,
    /// Inline secret key/values (`env = {"K" = "V", ..}`). Empty => no inline secret.
    /// Owned form of the `&'static` pairs; the facade resolves them via a deterministic
    /// per-entrypoint `Secret.from_dict`.
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// User-volume mounts to attach as `(mount_path, volume_name)` pairs.
    #[serde(default)]
    pub volumes: Vec<(String, String)>,
    /// Automatic retry COUNT (`retries = N`). `None` => no retry policy.
    #[serde(default)]
    pub retries: Option<u32>,
    /// Custom retry-policy SPEC string (`retries = Retries(..)`, struct form). `None` =>
    /// use the bare-int `retries` shortcut (or no policy). Owned form of the `&'static
    /// str` spec; the facade hands it to the SDK's `parse_retries_spec`.
    #[serde(default)]
    pub retries_spec: Option<String>,
    /// Run-SCHEDULE spec string (`schedule = Cron(..)/Period(..)`). `None` => no
    /// schedule. Owned form of the `&'static str` spec; the facade hands it to the SDK
    /// when building the `FunctionCreate`.
    #[serde(default)]
    pub schedule: Option<String>,
    /// Autoscaler floor (`min_containers`). `None` => scale to zero.
    #[serde(default)]
    pub min_containers: Option<u32>,
    /// Autoscaler ceiling (`max_containers`). `None` => no client-set ceiling.
    #[serde(default)]
    pub max_containers: Option<u32>,
    /// Warm buffer (`buffer_containers`). `None` => no buffer.
    #[serde(default)]
    pub buffer_containers: Option<u32>,
    /// Idle-before-scaledown window in seconds (`scaledown_window`). `None` => server
    /// default.
    #[serde(default)]
    pub scaledown_window: Option<u32>,
    /// Per-function IMAGE spec string (`image = Image(..)`). `None` => the path's
    /// env-driven base image. Owned form of the `&'static str` JSON spec; the facade
    /// parses it via [`crate::remote::parse_image_spec`] and folds the base image /
    /// install_rust / apt/pip/run steps into THIS entrypoint's build config (run +
    /// deploy). Rides the `--describe` manifest so the CLI path resolves it too.
    #[serde(default)]
    pub image: Option<String>,
    /// Memory-snapshot opt-in (`enable_memory_snapshot = true`), `#[cls]`-only in v0.
    /// `false` (the default) ⇒ inert ⇒ byte-identical to before. The facade only lets it
    /// take effect on the DEPLOY boundary (Modal snapshots deployed apps); RUN stays
    /// wire-identical. Owned form of the `bool`; rides the `--describe` manifest so the
    /// CLI path resolves it too.
    #[serde(default)]
    pub enable_memory_snapshot: bool,
    /// Web-endpoint HTTP method (`#[endpoint(method = "POST")]`). `None` (the default)
    /// ⇒ not an endpoint ⇒ byte-identical to before web endpoints. The facade only lets
    /// it take effect on the DEPLOY boundary (the URL is deploy-only in v0); RUN stays
    /// wire-identical even when the decorator opts in — exactly like
    /// `enable_memory_snapshot`. Owned form of the `&'static str`; rides the
    /// `--describe` manifest so the CLI path resolves it too.
    #[serde(default)]
    pub webhook_method: Option<String>,
    /// Modal proxy-auth opt-in for the endpoint (`requires_proxy_auth = true`).
    /// `false` (the default, matching Modal) = public URL. Inert unless
    /// `webhook_method` is set. Rides the `--describe` manifest like the rest.
    #[serde(default)]
    pub webhook_requires_proxy_auth: bool,
}

impl FunctionOptions {
    pub(crate) fn by_name<I, N, O>(configs: I) -> BTreeMap<String, Self>
    where
        I: IntoIterator<Item = (N, O)>,
        N: Into<String>,
        O: Into<Self>,
    {
        configs
            .into_iter()
            .map(|(name, options)| (name.into(), options.into()))
            .collect()
    }
}

impl From<&FunctionConfig> for FunctionOptions {
    fn from(config: &FunctionConfig) -> Self {
        FunctionOptions {
            gpu: config.gpu.map(str::to_string),
            timeout_secs: config.timeout_secs,
            cache: config.cache,
            milli_cpu: config.milli_cpu,
            memory_mb: config.memory_mb,
            secrets: config.secrets.iter().map(|s| s.to_string()).collect(),
            required_keys: config.required_keys.iter().map(|s| s.to_string()).collect(),
            env: config
                .env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            volumes: config
                .volumes
                .iter()
                .map(|(mount_path, name)| (mount_path.to_string(), name.to_string()))
                .collect(),
            retries: config.retries,
            retries_spec: config.retries_spec.map(str::to_string),
            schedule: config.schedule.map(str::to_string),
            min_containers: config.min_containers,
            max_containers: config.max_containers,
            buffer_containers: config.buffer_containers,
            scaledown_window: config.scaledown_window,
            image: config.image.map(str::to_string),
            enable_memory_snapshot: config.enable_memory_snapshot,
            webhook_method: config.webhook_method.map(str::to_string),
            webhook_requires_proxy_auth: config.webhook_requires_proxy_auth,
        }
    }
}

impl From<FunctionConfig> for FunctionOptions {
    fn from(config: FunctionConfig) -> Self {
        FunctionOptions::from(&config)
    }
}

/// The single macro-discovery record submitted to facade inventory.
///
/// Keeping `handler` and the control-plane metadata in one record makes the
/// advanced/manual inventory path atomic: users cannot accidentally submit
/// dispatch without the metadata companion, or metadata without dispatch. The
/// facade splits this record internally when it builds a runtime [`Registry`] and
/// a per-entrypoint config map.
pub struct Registration {
    /// The entrypoint name (registry key).
    pub name: &'static str,
    /// The monomorphized `typed!` wrapper `fn` pointer.
    pub handler: HandlerFn,
    /// The monomorphized `typed_check!` DECODE-ONLY companion, powering the runner's
    /// `--check-input` LOCAL input validation (fail fast before any Modal call).
    /// `None` for hand-built records that predate the checker; such entrypoints skip
    /// local validation and degrade to the remote decode check rather than
    /// false-reject. The `#[modal_rust::function]`/`#[cls]` macros always populate it.
    pub check: Option<CheckFn>,
    /// The SNAPSHOT-PRIME hook: forces this entrypoint's `#[cls]` singleton (running
    /// its `#[enter]` inside Modal's memory-snapshot freeze window) on a `prime` serve
    /// frame. Populated ONLY for snapshot-enabled `#[cls]` methods (the `#[cls]` macro
    /// sets it when `enable_memory_snapshot = true`); `None` for plain `#[function]`s
    /// and non-snapshot classes ⇒ inert, byte-identical to before. The facade threads
    /// it into the runtime [`Registry`] so the serve loop can fire it best-effort.
    pub snapshot_prime: Option<PrimeFn>,
    /// Per-function deploy/run config sourced from the decorator.
    pub config: FunctionConfig,
    /// Cargo package captured at the user crate's macro expansion site.
    pub package: &'static str,
}

inventory::collect!(Registration);

/// Assemble the runtime registry from facade-owned macro registrations.
pub fn registry_from_inventory() -> Registry {
    let mut registry = Registry::new();
    for registration in inventory::iter::<Registration> {
        registry = register_one(registry, registration);
    }
    registry
}

/// Insert one inventory [`Registration`] into `registry`, recording its
/// `--check-input` [`CheckFn`] when present (via `function_checked`) and falling back
/// to the handler-only `function` otherwise. Shared by both inventory collectors so
/// the checker wiring cannot drift between them.
fn register_one(registry: Registry, registration: &Registration) -> Registry {
    let registry = match registration.check {
        Some(check) => registry.function_checked(registration.name, registration.handler, check),
        None => registry.function(registration.name, registration.handler),
    };
    // Carry the SNAPSHOT-PRIME hook (if any) into the runtime registry so the serve
    // loop fires it on a `prime` frame. `None` (plain functions / non-snapshot classes)
    // adds nothing ⇒ a registry with no snapshot `#[cls]` has no primes, byte-identical.
    match registration.snapshot_prime {
        Some(prime) => registry.snapshot_prime(prime),
        None => registry,
    }
}

/// Build the runtime registry and the per-name control-plane configs from one
/// pass over the same facade-owned inventory records.
pub fn from_inventory_with_configs() -> (Registry, Vec<(&'static str, FunctionOptions)>) {
    let mut registry = Registry::new();
    let mut configs = Vec::new();
    for registration in inventory::iter::<Registration> {
        registry = register_one(registry, registration);
        configs.push((
            registration.name,
            FunctionOptions::from(&registration.config),
        ));
    }
    (registry, configs)
}

/// The cargo package name captured by the macro from the user's
/// `env!("CARGO_PKG_NAME")` expansion site.
pub fn package_from_inventory() -> Option<&'static str> {
    inventory::iter::<Registration>
        .into_iter()
        .map(|r| r.package)
        .find(|p| !p.is_empty())
}

/// Run the macro-backed runner CLI from facade inventory.
pub fn run_cli_from_inventory() -> i32 {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    run_cli_with_args_from_inventory(&argv, &mut std::io::stdout())
}

/// Testable core of [`run_cli_from_inventory`].
pub fn run_cli_with_args_from_inventory<W: std::io::Write>(argv: &[String], out: &mut W) -> i32 {
    let (registry, configs) = from_inventory_with_configs();
    run_cli_with_args_and_configs(registry, &configs, argv, out)
}

/// Runtime dispatch plus the facade-owned additive `--describe` AND `--serve`
/// branches.
///
/// The `--serve` branch is the ADDITIVE warm-reuse path (cls-design.md §2.1): it hands
/// off to the long-lived [`modal_rust_runtime::run_serve`] loop (framed
/// `(entrypoint, input)` requests in, one frozen envelope per response out), keeping
/// the process — and so any generated `Cls` `OnceLock` singleton — alive across calls.
/// The one-shot `--entrypoint/--input-*` path below is byte-identical to before; only
/// a caller that explicitly passes `--serve` (the Python wrapper, for warm `Cls`
/// reuse) enters the serve loop.
pub fn run_cli_with_args_and_configs<W: std::io::Write>(
    registry: Registry,
    configs: &[(&'static str, FunctionOptions)],
    argv: &[String],
    out: &mut W,
) -> i32 {
    if argv.first().map(String::as_str) == Some("--describe") {
        return emit_describe(&registry, configs, out);
    }
    if argv.first().map(String::as_str) == Some("--serve") {
        return modal_rust_runtime::run_serve(registry);
    }
    modal_rust_runtime::run_cli_with_args(registry, argv, out)
}

const DESCRIBE_SCHEMA: &str = "modal-rust/describe@1";

#[derive(Serialize)]
struct DescribeManifest<'a> {
    schema: &'a str,
    entrypoints: Vec<DescribeEntry<'a>>,
}

#[derive(Serialize)]
struct DescribeEntry<'a> {
    name: &'a str,
    config: &'a FunctionOptions,
}

fn emit_describe<W: std::io::Write>(
    registry: &Registry,
    configs: &[(&'static str, FunctionOptions)],
    out: &mut W,
) -> i32 {
    let default = FunctionOptions::default();
    let entrypoints: Vec<DescribeEntry<'_>> = registry
        .names()
        .map(|&name| {
            let config = configs
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, c)| c)
                .unwrap_or(&default);
            DescribeEntry { name, config }
        })
        .collect();
    let manifest = DescribeManifest {
        schema: DESCRIBE_SCHEMA,
        entrypoints,
    };
    match serde_json::to_string(&manifest) {
        Ok(s) => {
            if let Err(e) = writeln!(out, "{s}") {
                eprintln!("modal_runner: failed to write describe manifest: {e}");
                return 1;
            }
            0
        }
        Err(e) => {
            eprintln!("modal_runner: failed to serialize describe manifest: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{typed, RunnerError};

    #[derive(serde::Deserialize)]
    struct In {
        a: i64,
        b: i64,
    }

    #[derive(serde::Serialize)]
    struct Out {
        sum: i64,
    }

    fn add(input: In) -> Result<Out, std::convert::Infallible> {
        Ok(Out {
            sum: input.a + input.b,
        })
    }

    fn registry() -> Registry {
        Registry::new()
            .function("add", typed!(add))
            .function("other", typed!(add))
    }

    #[test]
    fn describe_emits_manifest_with_configs() {
        let configs: &[(&'static str, FunctionOptions)] = &[(
            "add",
            FunctionOptions {
                gpu: Some("T4".to_string()),
                timeout_secs: Some(1800),
                cache: Some(false),
                milli_cpu: Some(2000),
                memory_mb: Some(4096),
                secrets: vec!["my-secret".to_string()],
                required_keys: vec!["API_KEY".to_string()],
                env: vec![("REGION".to_string(), "us".to_string())],
                volumes: vec![("/data".to_string(), "my-vol".to_string())],
                retries: Some(3),
                retries_spec: None,
                schedule: Some("cron:UTC:0 9 * * 1".to_string()),
                min_containers: Some(1),
                max_containers: Some(5),
                buffer_containers: Some(2),
                scaledown_window: Some(120),
                image: Some(
                    r#"{"base":"nvidia/cuda:12.6.3-devel","install_rust":true}"#.to_string(),
                ),
                enable_memory_snapshot: false,
                webhook_method: None,
                webhook_requires_proxy_auth: false,
            },
        )];
        let argv = vec!["--describe".to_string()];
        let mut buf = Vec::new();
        let code = run_cli_with_args_and_configs(registry(), configs, &argv, &mut buf);
        assert_eq!(code, 0);
        let v: serde_json::Value = serde_json::from_slice(&buf).expect("one JSON manifest");
        assert_eq!(v["schema"], "modal-rust/describe@1");
        let eps = v["entrypoints"].as_array().expect("entrypoints array");
        assert_eq!(eps[0]["name"], "add");
        assert_eq!(eps[0]["config"]["gpu"], "T4");
        assert_eq!(eps[0]["config"]["timeout_secs"], 1800);
        assert_eq!(eps[0]["config"]["cache"], false);
        assert_eq!(eps[0]["config"]["milli_cpu"], 2000);
        assert_eq!(eps[0]["config"]["memory_mb"], 4096);
        assert_eq!(
            eps[0]["config"]["secrets"],
            serde_json::json!(["my-secret"])
        );
        assert_eq!(
            eps[0]["config"]["required_keys"],
            serde_json::json!(["API_KEY"])
        );
        assert_eq!(
            eps[0]["config"]["env"],
            serde_json::json!([["REGION", "us"]])
        );
        assert_eq!(
            eps[0]["config"]["volumes"],
            serde_json::json!([["/data", "my-vol"]])
        );
        assert_eq!(eps[0]["config"]["retries"], 3);
        assert_eq!(eps[0]["config"]["schedule"], "cron:UTC:0 9 * * 1");
        assert_eq!(eps[0]["config"]["min_containers"], 1);
        assert_eq!(eps[0]["config"]["max_containers"], 5);
        assert_eq!(eps[0]["config"]["buffer_containers"], 2);
        assert_eq!(eps[0]["config"]["scaledown_window"], 120);
        assert_eq!(
            eps[0]["config"]["image"],
            r#"{"base":"nvidia/cuda:12.6.3-devel","install_rust":true}"#
        );
        assert_eq!(eps[1]["name"], "other");
        assert_eq!(eps[1]["config"]["gpu"], serde_json::Value::Null);
        assert_eq!(eps[1]["config"]["secrets"], serde_json::json!([]));
        assert_eq!(eps[1]["config"]["schedule"], serde_json::Value::Null);
        assert_eq!(eps[1]["config"]["min_containers"], serde_json::Value::Null);
        assert_eq!(
            eps[1]["config"]["scaledown_window"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn function_config_default_has_empty_secrets_and_volumes() {
        let d = FunctionConfig::default();
        assert!(d.secrets.is_empty());
        assert!(d.required_keys.is_empty());
        assert!(d.env.is_empty());
        assert!(d.volumes.is_empty());
        assert!(d.retries_spec.is_none());
        assert!(d.image.is_none());
        assert!(!d.enable_memory_snapshot);
        assert!(d.webhook_method.is_none());
        assert!(!d.webhook_requires_proxy_auth);
        let c = FunctionConfig::new();
        assert_eq!(d, c);
    }

    #[test]
    fn function_config_converts_to_owned_options() {
        let config = FunctionConfig {
            gpu: Some("T4"),
            timeout_secs: Some(1800),
            cache: Some(false),
            milli_cpu: Some(2000),
            memory_mb: Some(4096),
            secrets: &["my-secret"],
            required_keys: &["API_KEY"],
            env: &[("REGION", "us")],
            volumes: &[("/data", "my-vol")],
            retries: Some(3),
            retries_spec: None,
            schedule: Some("cron:UTC:0 9 * * 1"),
            min_containers: Some(1),
            max_containers: Some(5),
            buffer_containers: Some(2),
            scaledown_window: Some(120),
            image: Some(r#"{"base":"rust:1-slim"}"#),
            enable_memory_snapshot: true,
            webhook_method: Some("POST"),
            webhook_requires_proxy_auth: true,
        };
        let options = FunctionOptions::from(&config);
        assert_eq!(options.gpu.as_deref(), Some("T4"));
        assert_eq!(options.timeout_secs, Some(1800));
        assert_eq!(options.cache, Some(false));
        assert_eq!(options.milli_cpu, Some(2000));
        assert_eq!(options.memory_mb, Some(4096));
        assert_eq!(options.secrets, vec!["my-secret".to_string()]);
        assert_eq!(options.required_keys, vec!["API_KEY".to_string()]);
        assert_eq!(options.env, vec![("REGION".to_string(), "us".to_string())]);
        assert_eq!(
            options.volumes,
            vec![("/data".to_string(), "my-vol".to_string())]
        );
        assert_eq!(options.retries, Some(3));
        assert_eq!(options.retries_spec, None);
        assert_eq!(options.schedule.as_deref(), Some("cron:UTC:0 9 * * 1"));
        assert_eq!(options.min_containers, Some(1));
        assert_eq!(options.max_containers, Some(5));
        assert_eq!(options.buffer_containers, Some(2));
        assert_eq!(options.scaledown_window, Some(120));
        assert_eq!(options.image.as_deref(), Some(r#"{"base":"rust:1-slim"}"#));
        assert!(options.enable_memory_snapshot);
        assert_eq!(options.webhook_method.as_deref(), Some("POST"));
        assert!(options.webhook_requires_proxy_auth);
    }

    #[test]
    fn non_describe_delegates_to_runtime_dispatch() {
        let argv: Vec<String> = ["--entrypoint", "add", "--input-json", r#"{"a":40,"b":2}"#]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut buf = Vec::new();
        let code = run_cli_with_args_and_configs(registry(), &[], &argv, &mut buf);
        assert_eq!(code, 0);
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "{\"ok\":true,\"value\":{\"sum\":42}}\n"
        );
    }

    #[test]
    fn runner_error_still_reexported_for_handler_fn_shape() {
        let err = RunnerError::Decode("x".to_string());
        assert_eq!(err.kind(), "decode_error");
    }
}
