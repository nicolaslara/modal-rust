//! Facade-owned distributed registration for `#[modal_rust::function]`.
//!
//! The runtime crate owns only dispatch (`name` + `HandlerFn`). This module owns
//! the atomic macro-discovery record that pairs dispatch with Modal control-plane
//! metadata, so an inventory user submits one record or none.

use serde::Serialize;

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
    /// Named Modal secrets to attach.
    pub secrets: &'static [&'static str],
    /// User-volume mounts to attach as `(mount_path, volume_name)` pairs.
    pub volumes: &'static [(&'static str, &'static str)],
}

impl FunctionConfig {
    /// A `const` all-default config usable in a `static` `inventory::submit!`
    /// initializer.
    pub const fn new() -> Self {
        FunctionConfig {
            gpu: None,
            timeout_secs: None,
            cache: None,
            secrets: &[],
            volumes: &[],
        }
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
pub fn from_inventory_with_configs() -> (Registry, Vec<(&'static str, FunctionConfig)>) {
    let mut registry = Registry::new();
    let mut configs = Vec::new();
    for registration in inventory::iter::<Registration> {
        registry = registry.function(registration.name, registration.handler);
        configs.push((registration.name, registration.config.clone()));
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
    configs: &[(&'static str, FunctionConfig)],
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
    config: DescribeConfig,
}

#[derive(Serialize)]
struct DescribeConfig {
    gpu: Option<&'static str>,
    timeout_secs: Option<u32>,
    cache: Option<bool>,
    secrets: &'static [&'static str],
    volumes: &'static [(&'static str, &'static str)],
}

impl From<&FunctionConfig> for DescribeConfig {
    fn from(c: &FunctionConfig) -> Self {
        DescribeConfig {
            gpu: c.gpu,
            timeout_secs: c.timeout_secs,
            cache: c.cache,
            secrets: c.secrets,
            volumes: c.volumes,
        }
    }
}

fn emit_describe<W: std::io::Write>(
    registry: &Registry,
    configs: &[(&'static str, FunctionConfig)],
    out: &mut W,
) -> i32 {
    let default = FunctionConfig::default();
    let entrypoints: Vec<DescribeEntry<'_>> = registry
        .names()
        .map(|&name| {
            let config = configs
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, c)| c)
                .unwrap_or(&default);
            DescribeEntry {
                name,
                config: DescribeConfig::from(config),
            }
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
        let configs: &[(&'static str, FunctionConfig)] = &[(
            "add",
            FunctionConfig {
                gpu: Some("T4"),
                timeout_secs: Some(1800),
                cache: Some(false),
                secrets: &["my-secret"],
                volumes: &[("/data", "my-vol")],
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
        assert_eq!(
            eps[0]["config"]["secrets"],
            serde_json::json!(["my-secret"])
        );
        assert_eq!(
            eps[0]["config"]["volumes"],
            serde_json::json!([["/data", "my-vol"]])
        );
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
