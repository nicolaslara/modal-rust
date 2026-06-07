//! Facade-owned distributed registration for `#[modal_rust::function]`.
//!
//! The runtime crate owns only dispatch (`name` + `HandlerFn`). This module owns
//! the atomic macro-discovery record that pairs dispatch with Modal control-plane
//! metadata, so an inventory user submits one record or none.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{HandlerFn, Registry};

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
    /// User-volume mounts to attach as `(mount_path, volume_name)` pairs.
    pub volumes: &'static [(&'static str, &'static str)],
    /// Automatic retry COUNT (`retries = N`). `None` => no retry policy (server
    /// default: a failed call is not retried). A bare count maps to Modal's
    /// fixed-interval policy (`backoff = 1.0`, 1s initial / 60s max delay) when the
    /// facade builds the `FunctionCreate`. Plain `Option<u32>`, const-valid in the
    /// `static` `inventory::submit!` initializer (like `timeout`).
    pub retries: Option<u32>,
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
            volumes: &[],
            retries: None,
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
    /// User-volume mounts to attach as `(mount_path, volume_name)` pairs.
    #[serde(default)]
    pub volumes: Vec<(String, String)>,
    /// Automatic retry COUNT (`retries = N`). `None` => no retry policy.
    #[serde(default)]
    pub retries: Option<u32>,
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
            volumes: config
                .volumes
                .iter()
                .map(|(mount_path, name)| (mount_path.to_string(), name.to_string()))
                .collect(),
            retries: config.retries,
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
        registry = registry.function(registration.name, registration.handler);
    }
    registry
}

/// Build the runtime registry and the per-name control-plane configs from one
/// pass over the same facade-owned inventory records.
pub fn from_inventory_with_configs() -> (Registry, Vec<(&'static str, FunctionOptions)>) {
    let mut registry = Registry::new();
    let mut configs = Vec::new();
    for registration in inventory::iter::<Registration> {
        registry = registry.function(registration.name, registration.handler);
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

/// Runtime dispatch plus the facade-owned additive `--describe` branch.
pub fn run_cli_with_args_and_configs<W: std::io::Write>(
    registry: Registry,
    configs: &[(&'static str, FunctionOptions)],
    argv: &[String],
    out: &mut W,
) -> i32 {
    if argv.first().map(String::as_str) == Some("--describe") {
        return emit_describe(&registry, configs, out);
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
                volumes: vec![("/data".to_string(), "my-vol".to_string())],
                retries: Some(3),
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
            eps[0]["config"]["volumes"],
            serde_json::json!([["/data", "my-vol"]])
        );
        assert_eq!(eps[0]["config"]["retries"], 3);
        assert_eq!(eps[1]["name"], "other");
        assert_eq!(eps[1]["config"]["gpu"], serde_json::Value::Null);
        assert_eq!(eps[1]["config"]["secrets"], serde_json::json!([]));
    }

    #[test]
    fn function_config_default_has_empty_secrets_and_volumes() {
        let d = FunctionConfig::default();
        assert!(d.secrets.is_empty());
        assert!(d.volumes.is_empty());
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
            volumes: &[("/data", "my-vol")],
            retries: Some(3),
        };
        let options = FunctionOptions::from(&config);
        assert_eq!(options.gpu.as_deref(), Some("T4"));
        assert_eq!(options.timeout_secs, Some(1800));
        assert_eq!(options.cache, Some(false));
        assert_eq!(options.milli_cpu, Some(2000));
        assert_eq!(options.memory_mb, Some(4096));
        assert_eq!(options.secrets, vec!["my-secret".to_string()]);
        assert_eq!(
            options.volumes,
            vec![("/data".to_string(), "my-vol".to_string())]
        );
        assert_eq!(options.retries, Some(3));
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
