//! `modal-rust` — the public CLI (boundaries.md §8, tasks.md M9a/M9b).
//!
//! A **pure wrapper** introducing no new Modal capability: it generates the
//! validated Modal Python shims (byte-equivalent, modulo injected params, to
//! `workpads/prototype/{dev_app,deploy_app,call_app}.py`) under the gitignored
//! `<workspace-root>/.modal-rust/generated/`, then drives the official `modal` CLI.
//! `clap` lives here (CLI-only), never in the runtime crate (boundaries.md §1).
//!
//! Subcommands:
//!   - `doctor [--rust]`  — OFFLINE preflight (see [`doctor`]).
//!   - `run <entrypoint>` — generate `dev_app.py`, then `modal run …::main`.
//!   - `deploy <entrypoint>` — generate `deploy_app.py`, then `modal deploy`.
//!   - `call <entrypoint>` — generate `call_app.py`, then `modal run …::main`.

mod doctor;
mod templates;
mod workspace;

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};

use templates::ShimParams;

/// The pinned Rust image tag component (`rust:{RUST_VER}-slim`), matching the
/// prototype shims (boundaries.md §11 default: `rust:1-slim`).
const RUST_VER: &str = "1";
/// The prototype dev/run app name (the injected param for `dev_app.py`).
const DEFAULT_DEV_APP: &str = "modal-rust-poc-dev";
/// The prototype local call-shim app name (the injected param for `call_app.py`).
const DEFAULT_CALL_APP: &str = "modal-rust-call";
/// The default persistent deploy app name (boundaries.md / tasks.md M7).
const DEFAULT_DEPLOY_APP: &str = "modal-rust-add-poc";

#[derive(Parser)]
#[command(
    name = "modal-rust",
    about = "Run, deploy, and call Rust functions on Modal — a pure wrapper over the official `modal` CLI.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// OFFLINE preflight: modal CLI + credentials (and with --rust, cargo/rustc +
    /// panic=abort detection).
    Doctor {
        /// Also check cargo/rustc and the release `panic = "abort"` profile.
        #[arg(long)]
        rust: bool,
        /// Project directory whose manifest chain is inspected (defaults to cwd).
        #[arg(long, default_value = "examples/add")]
        project: PathBuf,
    },
    /// Generate the dev shim and run the entrypoint with a RUNTIME build.
    Run {
        /// The registered entrypoint name (e.g. `add`).
        entrypoint: String,
        /// Project directory (the cargo workspace root is detected from here).
        #[arg(long, default_value = "examples/add")]
        project: PathBuf,
        #[command(flatten)]
        input: InputArg,
        /// Function timeout in seconds (informational; the shim pins timeout=1800).
        #[arg(long)]
        timeout: Option<u64>,
    },
    /// Generate the deploy shim and deploy with a BUILD-TIME build (baked binary).
    Deploy {
        /// The registered entrypoint name (informational; bound at call time).
        entrypoint: String,
        /// Project directory (the cargo workspace root is detected from here).
        #[arg(long, default_value = "examples/add")]
        project: PathBuf,
        /// The persistent Modal app name to deploy under.
        #[arg(long, default_value = DEFAULT_DEPLOY_APP)]
        app: String,
    },
    /// Generate the call shim and invoke the deployed Function (no build).
    Call {
        /// The registered entrypoint name (e.g. `add`).
        entrypoint: String,
        /// The persistent Modal app name to look up.
        #[arg(long, default_value = DEFAULT_DEPLOY_APP)]
        app: String,
        #[command(flatten)]
        input: InputArg,
    },
}

/// The public `--input <json|@file>` surface. Inline JSON or `@path` (read from
/// disk). Lowered to the shim's `--input-json <json>` (the runner-seam
/// `--input-file` split happens INSIDE the Python function body — tasks.md
/// Flag-mapping / Lowering rule).
#[derive(Args)]
struct InputArg {
    /// Inline JSON (`'{"a":40,"b":2}'`) or `@file` to read JSON from.
    #[arg(long)]
    input: Option<String>,
}

impl InputArg {
    /// Resolve the public `--input` into the JSON string passed to the shim's
    /// `--input-json`. Defaults to `{"a":40,"b":2}` (the prototype default) when
    /// omitted, matching the shims' own `main` defaults.
    fn resolve(&self) -> Result<String> {
        match &self.input {
            None => Ok(r#"{"a":40,"b":2}"#.to_string()),
            Some(s) => {
                if let Some(path) = s.strip_prefix('@') {
                    let body = std::fs::read_to_string(path)
                        .with_context(|| format!("failed to read --input file: {path}"))?;
                    Ok(body.trim().to_string())
                } else {
                    Ok(s.clone())
                }
            }
        }
    }
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let code = match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("modal-rust: error: {e:#}");
            1
        }
    };
    std::process::ExitCode::from(code as u8)
}

fn run(cli: Cli) -> Result<i32> {
    match cli.command {
        Commands::Doctor { rust, project } => Ok(doctor::run(rust, &project)),
        Commands::Run {
            entrypoint,
            project,
            input,
            timeout,
        } => cmd_run(&entrypoint, &project, &input.resolve()?, timeout),
        Commands::Deploy {
            entrypoint,
            project,
            app,
        } => cmd_deploy(&entrypoint, &project, &app),
        Commands::Call {
            entrypoint,
            app,
            input,
        } => cmd_call(&entrypoint, &app, &input.resolve()?),
    }
}

/// The directory under the workspace root where generated shims live (gitignored).
fn generated_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".modal-rust").join("generated")
}

/// Write `contents` to `<workspace-root>/.modal-rust/generated/<name>`, creating
/// the directory. Returns the path.
fn write_shim(workspace_root: &Path, name: &str, contents: &str) -> Result<PathBuf> {
    let dir = generated_dir(workspace_root);
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let path = dir.join(name);
    std::fs::write(&path, contents)
        .with_context(|| format!("failed to write shim {}", path.display()))?;
    Ok(path)
}

/// Build the shim params for a given workspace root and app names.
fn shim_params(
    workspace_root: &Path,
    dev_app: &str,
    deploy_app: &str,
    call_app: &str,
) -> ShimParams {
    ShimParams {
        dev_app_name: dev_app.to_string(),
        deploy_app_name: deploy_app.to_string(),
        call_app_name: call_app.to_string(),
        rust_ver: RUST_VER.to_string(),
        local_src: workspace_root.display().to_string(),
    }
}

/// Spawn a `modal` subcommand, inheriting stdio so the build log + envelope stream
/// straight through. Returns its exit code. Errors only if `modal` cannot spawn.
fn run_modal(args: &[String]) -> Result<i32> {
    eprintln!("modal-rust: exec: modal {}", args.join(" "));
    let status = Command::new("modal")
        .args(args)
        .status()
        .with_context(|| "failed to spawn `modal` (is it on $PATH? run `modal-rust doctor`)")?;
    Ok(status.code().unwrap_or(1))
}

fn cmd_run(
    entrypoint: &str,
    project: &Path,
    input_json: &str,
    timeout: Option<u64>,
) -> Result<i32> {
    let root = workspace::workspace_root(project)?;
    let params = shim_params(&root, DEFAULT_DEV_APP, DEFAULT_DEPLOY_APP, DEFAULT_CALL_APP);
    let shim = templates::dev_app(&params);
    let path = write_shim(&root, "dev_app.py", &shim)?;
    eprintln!("modal-rust: generated run shim: {}", path.display());
    if let Some(t) = timeout {
        eprintln!(
            "modal-rust: note: --timeout {t}s is informational; the run shim pins timeout=1800 (boundaries.md §4)."
        );
    }
    // Lowering rule (tasks.md): public `run <e> --input <json>` lowers to the shim
    // invocation `modal run <shim>::main --entrypoint <e> --input-json <json>`.
    let target = format!("{}::main", path.display());
    let args = vec![
        "run".to_string(),
        target,
        "--entrypoint".to_string(),
        entrypoint.to_string(),
        "--input-json".to_string(),
        input_json.to_string(),
    ];
    run_modal(&args)
}

fn cmd_deploy(entrypoint: &str, project: &Path, app: &str) -> Result<i32> {
    let root = workspace::workspace_root(project)?;
    let params = shim_params(&root, DEFAULT_DEV_APP, app, DEFAULT_CALL_APP);
    let shim = templates::deploy_app(&params);
    let path = write_shim(&root, "deploy_app.py", &shim)?;
    eprintln!("modal-rust: generated deploy shim: {}", path.display());
    eprintln!(
        "modal-rust: note: entrypoint {entrypoint:?} is bound at call time, not deploy time."
    );
    let args = vec!["deploy".to_string(), path.display().to_string()];
    run_modal(&args)
}

fn cmd_call(entrypoint: &str, app: &str, input_json: &str) -> Result<i32> {
    // The call shim does not mount/copy source, so any workspace root works for the
    // generated-dir location; use the cwd's workspace root if present, else cwd.
    let cwd = std::env::current_dir().context("could not read current dir")?;
    let root = workspace::workspace_root(&cwd).unwrap_or(cwd);
    let params = shim_params(&root, DEFAULT_DEV_APP, app, DEFAULT_CALL_APP);
    let shim = templates::call_app(&params);
    let path = write_shim(&root, "call_app.py", &shim)?;
    eprintln!("modal-rust: generated call shim: {}", path.display());
    let target = format!("{}::main", path.display());
    let args = vec![
        "run".to_string(),
        target,
        "--entrypoint".to_string(),
        entrypoint.to_string(),
        "--input-json".to_string(),
        input_json.to_string(),
    ];
    run_modal(&args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proto_params() -> (ShimParams, ShimParams, ShimParams) {
        // The exact prototype injected-param values.
        let local = "/Users/nicolas/devel/modal-rust".to_string();
        (
            ShimParams {
                dev_app_name: "modal-rust-poc-dev".to_string(),
                deploy_app_name: DEFAULT_DEPLOY_APP.to_string(),
                call_app_name: DEFAULT_CALL_APP.to_string(),
                rust_ver: "1".to_string(),
                local_src: local.clone(),
            },
            ShimParams {
                dev_app_name: DEFAULT_DEV_APP.to_string(),
                deploy_app_name: "modal-rust-add-poc".to_string(),
                call_app_name: DEFAULT_CALL_APP.to_string(),
                rust_ver: "1".to_string(),
                local_src: local.clone(),
            },
            ShimParams {
                dev_app_name: DEFAULT_DEV_APP.to_string(),
                deploy_app_name: "modal-rust-add-poc".to_string(),
                call_app_name: "modal-rust-call".to_string(),
                rust_ver: "1".to_string(),
                local_src: local,
            },
        )
    }

    /// M9a byte-equivalence: rendering each template with the prototype params must
    /// reproduce the prototype shim byte-for-byte.
    #[test]
    fn dev_shim_byte_equivalent_to_prototype() {
        let (dev, _, _) = proto_params();
        let want = include_str!("../../../workpads/prototype/dev_app.py");
        assert_eq!(templates::dev_app(&dev), want);
    }

    #[test]
    fn deploy_shim_byte_equivalent_to_prototype() {
        let (_, deploy, _) = proto_params();
        let want = include_str!("../../../workpads/prototype/deploy_app.py");
        assert_eq!(templates::deploy_app(&deploy), want);
    }

    #[test]
    fn call_shim_byte_equivalent_to_prototype() {
        let (_, _, call) = proto_params();
        let want = include_str!("../../../workpads/prototype/call_app.py");
        assert_eq!(templates::call_app(&call), want);
    }

    #[test]
    fn input_defaults_to_prototype_default() {
        let arg = InputArg { input: None };
        assert_eq!(arg.resolve().unwrap(), r#"{"a":40,"b":2}"#);
    }

    #[test]
    fn input_inline_passthrough() {
        let arg = InputArg {
            input: Some(r#"{"a":1,"b":2}"#.to_string()),
        };
        assert_eq!(arg.resolve().unwrap(), r#"{"a":1,"b":2}"#);
    }

    #[test]
    fn input_at_file_read() {
        let f = std::env::temp_dir().join(format!("mr-input-{}.json", std::process::id()));
        std::fs::write(&f, "{\"a\":5,\"b\":6}\n").unwrap();
        let arg = InputArg {
            input: Some(format!("@{}", f.display())),
        };
        assert_eq!(arg.resolve().unwrap(), r#"{"a":5,"b":6}"#);
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn generated_dir_is_under_modal_rust() {
        let d = generated_dir(Path::new("/tmp/ws"));
        assert!(d.ends_with(".modal-rust/generated"));
    }
}
