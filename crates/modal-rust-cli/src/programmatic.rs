//! The DEFAULT (programmatic) `run`/`deploy`/`call` path (P9 §C).
//!
//! Instead of rendering a Python shim and shelling out to the official `modal` CLI,
//! the CLI drives the proven SDK/facade orchestration directly:
//!
//! 1. `cargo build --release -p <package> --bin modal_runner` (cwd = workspace root)
//!    — the SAME `-p <pkg>` the shims used. This LOCAL build is for the manifest
//!    ONLY; the REMOTE build still happens per the frozen build boundary (in-body for
//!    `run`, at image-build for `deploy`). The CLI does NOT upload this local binary.
//! 2. Run `<workspace_root>/target/release/modal_runner --describe`, parse the
//!    `modal-rust/describe@1` manifest (entrypoints + per-entrypoint `FunctionOptions`).
//! 3. Drive the facade `App`: `run` = ephemeral app (`remote_envelope`), `deploy` =
//!    persistent (`deploy_with`), `call` = `from_name` + invoke (`call_envelope`).
//!
//! It emits NO generated `.py` and spawns NO `modal` subprocess. The only
//! subprocesses are `cargo` (build) and the user's `modal_runner` (`--describe`).

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use modal_rust::{App, DeployConfig, FunctionOptions, RemoteConfig};

use crate::workspace;

/// The `--describe` manifest schema this CLI understands. The CLI warns-and-proceeds
/// on an unknown MINOR; HARD-errors on an unknown MAJOR (P9 §A.3 / §C.3).
const DESCRIBE_SCHEMA_FAMILY: &str = "modal-rust/describe@";
const DESCRIBE_SCHEMA_MAJOR: u32 = 1;

/// One entrypoint in the parsed manifest.
#[derive(Debug, Clone, Deserialize)]
struct ManifestEntry {
    name: String,
    config: FunctionOptions,
}

/// The parsed `--describe` manifest.
#[derive(Debug, Deserialize)]
struct Manifest {
    schema: String,
    entrypoints: Vec<ManifestEntry>,
}

impl Manifest {
    /// Look up an entrypoint by name, returning a clear error (listing the known
    /// names) on a miss — parity with `run_cli`'s unknown-entrypoint diagnostic.
    fn entry(&self, name: &str) -> Result<&ManifestEntry> {
        self.entrypoints
            .iter()
            .find(|e| e.name == name)
            .with_context(|| {
                let known: Vec<String> = self
                    .entrypoints
                    .iter()
                    .map(|e| format!("{:?}", e.name))
                    .collect();
                format!(
                    "unknown entrypoint {name:?}; known entrypoints: [{}]",
                    known.join(", ")
                )
            })
    }
}

/// Build the manifest configs (`name -> FunctionOptions`) the facade `App` carries.
fn manifest_configs(manifest: &Manifest) -> Vec<(String, FunctionOptions)> {
    manifest
        .entrypoints
        .iter()
        .map(|e| (e.name.clone(), e.config.clone()))
        .collect()
}

/// Validate the manifest `schema` tag: warn-and-proceed on an unknown minor;
/// HARD-error on an unknown major (P9 §A.3 / §C.3).
fn check_schema(schema: &str) -> Result<()> {
    let version = schema
        .strip_prefix(DESCRIBE_SCHEMA_FAMILY)
        .with_context(|| format!("unrecognized --describe schema {schema:?}"))?;
    // Versions are bare major ("1") for describe@1; tolerate "major.minor".
    let major: u32 = version
        .split('.')
        .next()
        .unwrap_or("")
        .parse()
        .with_context(|| format!("unparseable --describe schema version in {schema:?}"))?;
    if major != DESCRIBE_SCHEMA_MAJOR {
        bail!(
            "incompatible --describe schema major {major} (this modal-rust expects \
             {DESCRIBE_SCHEMA_FAMILY}{DESCRIBE_SCHEMA_MAJOR}); rebuild your crate against \
             a matching modal-rust-runtime"
        );
    }
    Ok(())
}

/// Build the user crate's `modal_runner` and read its `--describe` manifest (P9 §C.3).
///
/// Returns the parsed [`Manifest`] plus the resolved `(workspace_root, package)` the
/// caller threads into `RemoteConfig`/`DeployConfig`.
fn build_and_describe(project: &Path) -> Result<(Manifest, std::path::PathBuf, String)> {
    let root = workspace::workspace_root(project)?;
    // `-p <pkg>` disambiguates the shared `modal_runner` bin across workspace members
    // (boundaries.md §8) — the SAME package the shims built.
    let package = workspace::package_name(project)?;

    // 1. LOCAL build (manifest-only; NOT uploaded). cwd = workspace root, inheriting
    //    stderr so the compile log streams. `Command::new("cargo")` — NOT `modal`.
    eprintln!("modal-rust: building {package} modal_runner (cargo) for --describe …");
    let status = Command::new("cargo")
        .args([
            "build",
            "--release",
            "-p",
            &package,
            "--bin",
            "modal_runner",
        ])
        .current_dir(&root)
        .status()
        .context("failed to spawn `cargo` (is it on $PATH? run `modal-rust doctor --rust`)")?;
    if !status.success() {
        bail!(
            "cargo build of `{package}` modal_runner failed (exit {})",
            status.code().unwrap_or(-1)
        );
    }

    // 2. Run `modal_runner --describe`, capture stdout, parse the manifest.
    let runner_bin = root.join("target").join("release").join("modal_runner");
    let out = Command::new(&runner_bin)
        .arg("--describe")
        .output()
        .with_context(|| format!("failed to run {} --describe", runner_bin.display()))?;
    if !out.status.success() {
        bail!(
            "{} --describe exited {} (stderr: {})",
            runner_bin.display(),
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let manifest: Manifest = serde_json::from_slice(&out.stdout)
        .context("failed to parse `modal_runner --describe` manifest JSON")?;
    check_schema(&manifest.schema)?;

    Ok((manifest, root, package))
}

/// Print the runner's one-line JSON envelope VERBATIM to stdout and mirror its `ok`
/// field into the process exit code (0 success / 1 failure) — the SAME contract the
/// shim path produced (P9 §G.1).
fn print_envelope_and_exit_code(envelope: &str) -> i32 {
    println!("{envelope}");
    match serde_json::from_str::<serde_json::Value>(envelope) {
        Ok(v) if v.get("ok") == Some(&serde_json::Value::Bool(true)) => 0,
        _ => 1,
    }
}

/// DEFAULT `run`: build + describe, then drive an EPHEMERAL app via the facade's
/// `App::connect_from_manifest` + `remote_envelope` (mirrors `.remote()`). NO
/// generated `.py`, NO `modal` subprocess (P9 §C.4).
pub async fn cmd_run_programmatic(
    entrypoint: &str,
    project: &Path,
    input_json: String,
    timeout: Option<u64>,
) -> Result<i32> {
    let (manifest, root, package) = build_and_describe(project)?;
    let _ = manifest.entry(entrypoint)?;
    if let Some(t) = timeout {
        eprintln!(
            "modal-rust: note: --timeout {t}s is informational; the entrypoint's decorator \
             timeout (or the run-path default) applies."
        );
    }

    // Build the EXPLICIT RemoteConfig from the real workspace root + package + the
    // resolved per-entrypoint config (the SAME fields `App::remote_invoke` applies).
    let run_config = RemoteConfig {
        local_root: root,
        package,
        ..RemoteConfig::default()
    };

    let configs = manifest_configs(&manifest);
    let app = App::connect_from_manifest("modal-rust-cli-run", configs, run_config)
        .await
        .context("failed to connect to Modal for the run path")?;
    let envelope = app
        .remote_envelope(entrypoint, input_json)
        .await
        .context("remote run failed")?;
    Ok(print_envelope_and_exit_code(&envelope))
}

/// DEFAULT `deploy`: build + describe, then drive a PERSISTENT deploy via the
/// facade's `App::deploy_with` (mirrors `App::deploy`). The decorated gpu/timeout is
/// resolved INSIDE `deploy_with` from the manifest configs (P9 §C.4).
pub async fn cmd_deploy_programmatic(
    entrypoint: &str,
    project: &Path,
    app_name: &str,
) -> Result<i32> {
    let (manifest, root, package) = build_and_describe(project)?;
    // Deploy publishes every manifest entrypoint as its own Modal function over one
    // shared image. The selected `entrypoint` is still not the only deployed
    // function, but validate it exists so a typo fails fast — parity with run.
    let _ = manifest.entry(entrypoint)?;
    eprintln!(
        "modal-rust: note: entrypoint {entrypoint:?} is bound at call time, not deploy time."
    );

    let deploy_config = DeployConfig {
        local_root: root,
        package,
        ..DeployConfig::for_app(app_name)
    };

    let configs = manifest_configs(&manifest);
    // `connect_from_manifest`'s RemoteConfig is unused by the deploy path (deploy
    // reads DeployConfig), so the default is fine here.
    let app = App::connect_from_manifest(app_name, configs, RemoteConfig::default())
        .await
        .context("failed to connect to Modal for the deploy path")?;
    let deployed = app
        .deploy_with(deploy_config)
        .await
        .context("deploy failed")?;
    eprintln!(
        "modal-rust: deployed app {:?} (function_id={}, image_id={}, url={:?})",
        deployed.name, deployed.function_id, deployed.image_id, deployed.url
    );
    println!("deployed: {}", deployed.name);
    Ok(0)
}

/// DEFAULT `call`: `from_name` + invoke via the facade's `App::call_envelope`. NO
/// build, NO describe, NO upload — the deployed wrapper already carries its config
/// (P9 §C.4). Builds a headless app with empty configs + a default RemoteConfig.
pub async fn cmd_call_programmatic(
    entrypoint: &str,
    app_name: &str,
    input_json: String,
) -> Result<i32> {
    let app = App::connect_from_manifest(
        "modal-rust-cli-call",
        std::iter::empty::<(String, FunctionOptions)>(),
        RemoteConfig::default(),
    )
    .await
    .context("failed to connect to Modal for the call path")?;
    let envelope = app
        .call_envelope(app_name, entrypoint, input_json)
        .await
        .context("call failed")?;
    Ok(print_envelope_and_exit_code(&envelope))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_accepts_describe_v1() {
        assert!(check_schema("modal-rust/describe@1").is_ok());
    }

    #[test]
    fn schema_accepts_unknown_minor() {
        // Unknown minor warns-and-proceeds (still major 1).
        assert!(check_schema("modal-rust/describe@1.5").is_ok());
    }

    #[test]
    fn schema_rejects_unknown_major() {
        let err = check_schema("modal-rust/describe@2").unwrap_err();
        assert!(err.to_string().contains("incompatible"));
    }

    #[test]
    fn schema_rejects_foreign_family() {
        assert!(check_schema("other/thing@1").is_err());
    }

    #[test]
    fn manifest_parses_and_resolves_entry() {
        let json = r#"{
            "schema": "modal-rust/describe@1",
            "entrypoints": [
                {"name": "add", "config": {"gpu": null, "timeout_secs": null, "cache": null}},
                {"name": "vector_add", "config": {"gpu": "T4", "timeout_secs": 1800, "cache": false}}
            ]
        }"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        check_schema(&m.schema).unwrap();
        let add = m.entry("add").unwrap();
        assert_eq!(add.config.gpu, None);
        let va = m.entry("vector_add").unwrap();
        assert_eq!(va.config.gpu.as_deref(), Some("T4"));
        assert_eq!(va.config.timeout_secs, Some(1800));
        assert_eq!(va.config.cache, Some(false));
    }

    #[test]
    fn manifest_unknown_entrypoint_lists_known() {
        let json = r#"{"schema":"modal-rust/describe@1","entrypoints":[
            {"name":"add","config":{"gpu":null,"timeout_secs":null,"cache":null}}]}"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        let err = m.entry("nope").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown entrypoint \"nope\""));
        assert!(msg.contains("\"add\""));
    }

    #[test]
    fn manifest_configs_preserve_owned_options() {
        let json = r#"{"schema":"modal-rust/describe@1","entrypoints":[
            {"name":"add","config":{
                "gpu":"A100",
                "timeout_secs":900,
                "cache":true,
                "secrets":["my-secret"],
                "volumes":[["/data","my-vol"]]
            }}]}"#;
        let manifest: Manifest = serde_json::from_str(json).unwrap();
        let configs = manifest_configs(&manifest);
        let c = &configs[0].1;
        assert_eq!(configs[0].0, "add");
        assert_eq!(c.gpu.as_deref(), Some("A100"));
        assert_eq!(c.timeout_secs, Some(900));
        assert_eq!(c.cache, Some(true));
        assert_eq!(c.secrets, vec!["my-secret".to_string()]);
        assert_eq!(c.volumes, vec![("/data".to_string(), "my-vol".to_string())]);
    }

    #[test]
    fn print_envelope_exit_code_mirrors_ok() {
        assert_eq!(
            print_envelope_and_exit_code(r#"{"ok":true,"value":{"sum":42}}"#),
            0
        );
        assert_eq!(
            print_envelope_and_exit_code(r#"{"ok":false,"error":{"kind":"panic"}}"#),
            1
        );
        assert_eq!(print_envelope_and_exit_code("garbage"), 1);
    }
}
