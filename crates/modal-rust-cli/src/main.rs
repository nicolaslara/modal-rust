//! `modal-rust` — the public CLI (boundaries.md §8, tasks.md M9a/M9b; P9/P10).
//!
//! ## The programmatic path (the only path)
//!
//! `run`/`deploy`/`call` drive the proven SDK/facade orchestration directly: the CLI
//! builds the user crate's `modal_runner`, runs `modal_runner --describe` to read the
//! entrypoint manifest + per-entrypoint config, then calls the SAME `App` methods the
//! facade `.remote()`/`deploy`/`call` use. It emits NO generated `.py` and spawns NO
//! `modal` subprocess. `clap`/`tokio` live here (CLI-only), never in the runtime
//! crate (boundaries.md §1).
//!
//! Subcommands:
//!   - `doctor [--rust]` — OFFLINE preflight (see [`doctor`]).
//!   - `run <entrypoint>` — programmatic ephemeral run.
//!   - `deploy <entrypoint>` — programmatic persistent deploy.
//!   - `call <entrypoint>` — programmatic `from_name` + invoke.

mod describe_cache;
mod doctor;
mod programmatic;
mod workspace;

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};

/// The default persistent deploy app name (boundaries.md / tasks.md M7).
///
/// Re-exported from the facade so the CLI and `DeployConfig::default` share ONE
/// source of truth and cannot disagree (previously this was a divergent local
/// `"modal-rust-add-poc"` string).
use modal_rust::DEFAULT_DEPLOY_APP;

#[derive(Parser)]
#[command(
    name = "modal-rust",
    about = "Run, deploy, and call Rust functions on Modal via the first-party SDK — no codegen, no `modal` CLI.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// OFFLINE preflight: credentials (always) + (with --rust) cargo/rustc +
    /// panic=abort detection. The programmatic path connects to Modal directly and
    /// never spawns `modal`, so the `modal` CLI is not a prerequisite.
    Doctor {
        /// Also check cargo/rustc and the release `panic = "abort"` profile.
        #[arg(long)]
        rust: bool,
        /// Project directory whose manifest chain is inspected (defaults to the
        /// current directory).
        #[arg(long, default_value = ".")]
        project: PathBuf,
    },
    /// Run the entrypoint with a RUNTIME build (programmatic ephemeral run).
    Run {
        /// The registered entrypoint name (e.g. `add`).
        entrypoint: String,
        /// Project directory, the crate to run (the cargo workspace root is detected
        /// from here). Defaults to the current directory.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        #[command(flatten)]
        input: InputArg,
        /// Skip the local describe build; take config from --manifest or
        /// --gpu/--timeout/... instead. Use when the crate compiles on Modal but not
        /// locally, or to avoid the describe build overhead.
        #[arg(long)]
        no_local_build: bool,
        /// Path to a describe@1 manifest JSON file (entrypoints + config). Implies
        /// --no-local-build. The entrypoint must be present in this file.
        #[arg(long)]
        manifest: Option<PathBuf>,
        #[command(flatten)]
        inline_config: InlineConfig,
    },
    /// Deploy with a BUILD-TIME build / baked binary (programmatic persistent deploy).
    Deploy {
        /// The registered entrypoint name (informational; bound at call time).
        entrypoint: String,
        /// Project directory, the crate to deploy (the cargo workspace root is
        /// detected from here). Defaults to the current directory.
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// The persistent Modal app name to deploy under.
        #[arg(long, default_value = DEFAULT_DEPLOY_APP)]
        app: String,
        /// Skip the local describe build; take config from --manifest or
        /// --gpu/--timeout/... instead. Use when the crate compiles on Modal but not
        /// locally, or to avoid the describe build overhead.
        #[arg(long)]
        no_local_build: bool,
        /// Path to a describe@1 manifest JSON file (entrypoints + config). Implies
        /// --no-local-build. The entrypoint must be present in this file.
        #[arg(long)]
        manifest: Option<PathBuf>,
        #[command(flatten)]
        inline_config: InlineConfig,
    },
    /// Invoke the deployed Function (no build): programmatic `from_name` + invoke.
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

/// Inline entrypoint configuration flags (override decorator config when
/// --no-local-build or --manifest is used).
#[derive(Args)]
struct InlineConfig {
    /// GPU type (Modal format: "T4", "A100", "A100-80GB", "H100:4", etc.).
    /// Overrides the decorator's setting. Only valid with --no-local-build/--manifest.
    #[arg(long)]
    gpu: Option<String>,

    /// Function timeout in seconds. Overrides the decorator's setting.
    /// Only valid with --no-local-build/--manifest.
    #[arg(long)]
    timeout: Option<u32>,

    /// Requested CPU in millicores. Overrides the decorator's setting.
    /// Only valid with --no-local-build/--manifest.
    #[arg(long)]
    cpu: Option<u32>,

    /// Requested memory in mebibytes. Overrides the decorator's setting.
    /// Only valid with --no-local-build/--manifest.
    #[arg(long)]
    memory: Option<u32>,
}

/// The public `--input <json|@file>` surface. Inline JSON or `@path` (read from
/// disk). The resolved JSON string is handed to the programmatic invoke path.
#[derive(Args)]
struct InputArg {
    /// Inline JSON (`'{"a":40,"b":2}'`) or `@file` to read JSON from.
    #[arg(long)]
    input: Option<String>,
}

impl InlineConfig {
    /// True when ANY inline config flag was supplied.
    fn any_set(&self) -> bool {
        self.gpu.is_some() || self.timeout.is_some() || self.cpu.is_some() || self.memory.is_some()
    }
}

/// P4 doctrine: inline config flags are an ESCAPE HATCH, only valid together with
/// --no-local-build or --manifest. On the normal build path the decorator is the
/// source of truth, so reject inline flags with a clear, actionable error.
fn reject_inline_without_escape_hatch(inline: &InlineConfig, no_local_build: bool) -> Result<()> {
    if inline.any_set() && !no_local_build {
        bail!(
            "inline config flags (--gpu, --timeout, --cpu, --memory) are only valid with \
             --no-local-build or --manifest.\n\n\
             P4 doctrine: decorator settings are the source of truth. To override them:\n  \
             1. Use --manifest <file> with a custom describe@1 manifest, OR\n  \
             2. Use --no-local-build --gpu <T4> --timeout 600 ... (inline flags only skip the build)\n\n\
             For per-entrypoint config, edit the decorator: \
             #[modal_rust::function(gpu = \"T4\", timeout = 600)]"
        );
    }
    Ok(())
}

impl InputArg {
    /// Resolve the public `--input` into the JSON string passed to the invoke path.
    /// Defaults to `{"a":40,"b":2}` (the prototype default) when omitted.
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

/// Build a current-thread-free multi-thread tokio runtime for the async facade ops.
/// `main()` stays `i32`-returning; the programmatic arms `block_on` here.
fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")
}

fn run(cli: Cli) -> Result<i32> {
    match cli.command {
        Commands::Doctor { rust, project } => Ok(doctor::run(rust, &project)),
        Commands::Run {
            entrypoint,
            project,
            input,
            mut no_local_build,
            manifest,
            inline_config,
        } => {
            // --manifest implies --no-local-build (manifest supplies config; skip build).
            if manifest.is_some() {
                no_local_build = true;
            }
            // P4: inline flags only with the escape hatch.
            reject_inline_without_escape_hatch(&inline_config, no_local_build)?;
            let input_json = input.resolve()?;
            runtime()?.block_on(programmatic::cmd_run_programmatic(
                &entrypoint,
                &project,
                input_json,
                no_local_build,
                manifest,
                &inline_config,
            ))
        }
        Commands::Deploy {
            entrypoint,
            project,
            app,
            mut no_local_build,
            manifest,
            inline_config,
        } => {
            if manifest.is_some() {
                no_local_build = true;
            }
            reject_inline_without_escape_hatch(&inline_config, no_local_build)?;
            runtime()?.block_on(programmatic::cmd_deploy_programmatic(
                &entrypoint,
                &project,
                &app,
                no_local_build,
                manifest,
                &inline_config,
            ))
        }
        Commands::Call {
            entrypoint,
            app,
            input,
        } => {
            let input_json = input.resolve()?;
            runtime()?.block_on(programmatic::cmd_call_programmatic(
                &entrypoint,
                &app,
                input_json,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
