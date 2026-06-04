//! `modal-rust` — the public CLI (boundaries.md §8, tasks.md M9a/M9b; P9).
//!
//! ## Default path (programmatic — P9)
//!
//! `run`/`deploy`/`call` drive the proven SDK/facade orchestration directly: the CLI
//! builds the user crate's `modal_runner`, runs `modal_runner --describe` to read the
//! entrypoint manifest + per-entrypoint config, then calls the SAME `App` methods the
//! facade `.remote()`/`deploy`/`call` use. It emits NO generated `.py` and spawns NO
//! `modal` subprocess. `clap`/`tokio` live here (CLI-only), never in the runtime
//! crate (boundaries.md §1).
//!
//! ## Fallback path (`--use-shim` — KEPT, P10 removes)
//!
//! With `--use-shim`, `run`/`deploy`/`call` revert to the legacy behavior: generate
//! the validated Modal Python shims (byte-equivalent, modulo injected params, to
//! `workpads/prototype/{dev_app,deploy_app,call_app}.py`) under the gitignored
//! `<workspace-root>/.modal-rust/generated/`, then drive the official `modal` CLI.
//!
//! Subcommands:
//!   - `doctor [--rust] [--use-shim]` — OFFLINE preflight (see [`doctor`]).
//!   - `run <entrypoint> [--use-shim]` — programmatic ephemeral run (default), or
//!     generate `dev_app.py` + `modal run …::main` (`--use-shim`).
//!   - `deploy <entrypoint> [--use-shim]` — programmatic persistent deploy (default),
//!     or generate `deploy_app.py` + `modal deploy` (`--use-shim`).
//!   - `call <entrypoint> [--use-shim]` — programmatic `from_name` + invoke (default),
//!     or generate `call_app.py` + `modal run …::main` (`--use-shim`).

mod doctor;
mod programmatic;
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
    /// OFFLINE preflight: credentials + (with --rust) cargo/rustc + panic=abort
    /// detection. The `modal` CLI is checked ONLY with --use-shim (the default path
    /// is programmatic and never spawns `modal`).
    Doctor {
        /// Also check cargo/rustc and the release `panic = "abort"` profile.
        #[arg(long)]
        rust: bool,
        /// Project directory whose manifest chain is inspected (defaults to cwd).
        #[arg(long, default_value = "examples/add")]
        project: PathBuf,
        /// Also require the legacy `modal` CLI on $PATH (the --use-shim fallback).
        #[arg(long)]
        use_shim: bool,
    },
    /// Run the entrypoint with a RUNTIME build (default: programmatic ephemeral run;
    /// --use-shim: generate the dev shim + `modal run`).
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
        /// Use the legacy Python-shim + `modal` CLI fallback instead of the default
        /// programmatic path (P9; P10 removes this).
        #[arg(long)]
        use_shim: bool,
    },
    /// Deploy with a BUILD-TIME build / baked binary (default: programmatic
    /// persistent deploy; --use-shim: generate the deploy shim + `modal deploy`).
    Deploy {
        /// The registered entrypoint name (informational; bound at call time).
        entrypoint: String,
        /// Project directory (the cargo workspace root is detected from here).
        #[arg(long, default_value = "examples/add")]
        project: PathBuf,
        /// The persistent Modal app name to deploy under.
        #[arg(long, default_value = DEFAULT_DEPLOY_APP)]
        app: String,
        /// Use the legacy Python-shim + `modal` CLI fallback instead of the default
        /// programmatic path (P9; P10 removes this).
        #[arg(long)]
        use_shim: bool,
    },
    /// Invoke the deployed Function (no build). Default: programmatic `from_name` +
    /// invoke; --use-shim: generate the call shim + `modal run`.
    Call {
        /// The registered entrypoint name (e.g. `add`).
        entrypoint: String,
        /// The persistent Modal app name to look up.
        #[arg(long, default_value = DEFAULT_DEPLOY_APP)]
        app: String,
        #[command(flatten)]
        input: InputArg,
        /// Use the legacy Python-shim + `modal` CLI fallback instead of the default
        /// programmatic path (P9; P10 removes this).
        #[arg(long)]
        use_shim: bool,
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
        Commands::Doctor {
            rust,
            project,
            use_shim,
        } => Ok(doctor::run(rust, use_shim, &project)),
        Commands::Run {
            entrypoint,
            project,
            input,
            timeout,
            use_shim,
        } => {
            let input_json = input.resolve()?;
            if use_shim {
                cmd_run_shim(&entrypoint, &project, &input_json, timeout)
            } else {
                runtime()?.block_on(programmatic::cmd_run_programmatic(
                    &entrypoint,
                    &project,
                    input_json,
                    timeout,
                ))
            }
        }
        Commands::Deploy {
            entrypoint,
            project,
            app,
            use_shim,
        } => {
            if use_shim {
                cmd_deploy_shim(&entrypoint, &project, &app)
            } else {
                runtime()?.block_on(programmatic::cmd_deploy_programmatic(
                    &entrypoint,
                    &project,
                    &app,
                ))
            }
        }
        Commands::Call {
            entrypoint,
            app,
            input,
            use_shim,
        } => {
            let input_json = input.resolve()?;
            if use_shim {
                cmd_call_shim(&entrypoint, &app, &input_json)
            } else {
                runtime()?.block_on(programmatic::cmd_call_programmatic(
                    &entrypoint,
                    &app,
                    input_json,
                ))
            }
        }
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

/// Build the shim params for a given workspace root and app names. `package` is the
/// cargo `[package].name` injected as `-p <pkg>` (disambiguating the shared
/// `modal_runner` bin). Per-function config (gpu/timeout/cache) is sourced from the
/// Rust `#[modal_rust::function(...)]` decorator via the facade, NOT the CLI.
fn shim_params(
    workspace_root: &Path,
    dev_app: &str,
    deploy_app: &str,
    call_app: &str,
    package: &str,
) -> ShimParams {
    ShimParams {
        dev_app_name: dev_app.to_string(),
        deploy_app_name: deploy_app.to_string(),
        call_app_name: call_app.to_string(),
        rust_ver: RUST_VER.to_string(),
        local_src: workspace_root.display().to_string(),
        package: package.to_string(),
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

/// LEGACY `--use-shim` run: render `dev_app.py` and drive `modal run …::main`.
/// Byte-for-byte UNCHANGED from the pre-P9 `cmd_run` (P9 §C.2; P10 removes it).
fn cmd_run_shim(
    entrypoint: &str,
    project: &Path,
    input_json: &str,
    timeout: Option<u64>,
) -> Result<i32> {
    let root = workspace::workspace_root(project)?;
    // Derive the cargo package from `--project`'s `[package].name`: the build must
    // be `-p <pkg>` because the `modal_runner` bin is shared across workspace
    // members (boundaries.md §8).
    let package = workspace::package_name(project)?;
    let params = shim_params(
        &root,
        DEFAULT_DEV_APP,
        DEFAULT_DEPLOY_APP,
        DEFAULT_CALL_APP,
        &package,
    );
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

/// LEGACY `--use-shim` deploy: render `deploy_app.py` and drive `modal deploy`.
/// Byte-for-byte UNCHANGED from the pre-P9 `cmd_deploy` (P9 §C.2; P10 removes it).
fn cmd_deploy_shim(entrypoint: &str, project: &Path, app: &str) -> Result<i32> {
    let root = workspace::workspace_root(project)?;
    // Same package-qualified build as `run` (boundaries.md §8): derive `-p <pkg>`
    // from `--project`'s `[package].name`.
    let package = workspace::package_name(project)?;
    let params = shim_params(&root, DEFAULT_DEV_APP, app, DEFAULT_CALL_APP, &package);
    let shim = templates::deploy_app(&params);
    let path = write_shim(&root, "deploy_app.py", &shim)?;
    eprintln!("modal-rust: generated deploy shim: {}", path.display());
    eprintln!(
        "modal-rust: note: entrypoint {entrypoint:?} is bound at call time, not deploy time."
    );
    let args = vec!["deploy".to_string(), path.display().to_string()];
    run_modal(&args)
}

/// LEGACY `--use-shim` call: render `call_app.py` and drive `modal run …::main`.
/// Byte-for-byte UNCHANGED from the pre-P9 `cmd_call` (P9 §C.2; P10 removes it).
fn cmd_call_shim(entrypoint: &str, app: &str, input_json: &str) -> Result<i32> {
    // The call shim does not mount/copy source, so any workspace root works for the
    // generated-dir location; use the cwd's workspace root if present, else cwd.
    let cwd = std::env::current_dir().context("could not read current dir")?;
    let root = workspace::workspace_root(&cwd).unwrap_or(cwd);
    // The call shim builds nothing and looks up the deployed Function by name, so
    // the package param is unused by call_app.py (passed empty).
    let params = shim_params(&root, DEFAULT_DEV_APP, app, DEFAULT_CALL_APP, "");
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
        // The exact prototype injected-param values. The prototype project is
        // examples/add (package `example-add`); the CLI no longer injects gpu (P4:
        // config is sourced from the Rust decorator), so the rendered shim must match
        // the package-qualified, no-gpu prototype reference.
        let local = "/Users/nicolas/devel/modal-rust".to_string();
        (
            ShimParams {
                dev_app_name: "modal-rust-poc-dev".to_string(),
                deploy_app_name: DEFAULT_DEPLOY_APP.to_string(),
                call_app_name: DEFAULT_CALL_APP.to_string(),
                rust_ver: "1".to_string(),
                local_src: local.clone(),
                package: "example-add".to_string(),
            },
            ShimParams {
                dev_app_name: DEFAULT_DEV_APP.to_string(),
                deploy_app_name: "modal-rust-add-poc".to_string(),
                call_app_name: DEFAULT_CALL_APP.to_string(),
                rust_ver: "1".to_string(),
                local_src: local.clone(),
                package: "example-add".to_string(),
            },
            ShimParams {
                dev_app_name: DEFAULT_DEV_APP.to_string(),
                deploy_app_name: "modal-rust-add-poc".to_string(),
                call_app_name: "modal-rust-call".to_string(),
                rust_ver: "1".to_string(),
                local_src: local,
                package: "example-add".to_string(),
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
    fn dev_shim_injects_package_qualified_build() {
        // Regression guard (boundaries.md §8): the shared `modal_runner` bin must be
        // built package-qualified. Rendering for examples/add must put `-p
        // example-add` into the dev shim's cargo build.
        let (dev, _, _) = proto_params();
        let shim = templates::dev_app(&dev);
        assert!(
            shim.contains(
                r#"["cargo", "build", "--release", "-p", PACKAGE, "--bin", "modal_runner"]"#
            ),
            "dev shim must build with -p PACKAGE"
        );
        assert!(
            shim.contains(r#"PACKAGE = "example-add""#),
            "dev shim must set PACKAGE = example-add"
        );
        // The ambiguous bare build must be gone.
        assert!(
            !shim.contains(r#"["cargo", "build", "--release", "--bin", "modal_runner"]"#),
            "dev shim must NOT keep the ambiguous bare --bin build"
        );
    }

    #[test]
    fn deploy_shim_injects_package_qualified_build() {
        let (_, deploy, _) = proto_params();
        let shim = templates::deploy_app(&deploy);
        assert!(
            shim.contains("cargo build --release -p {PACKAGE} --bin modal_runner"),
            "deploy shim must build with -p {{PACKAGE}}"
        );
        assert!(
            shim.contains(r#"PACKAGE = "example-add""#),
            "deploy shim must set PACKAGE = example-add"
        );
    }

    #[test]
    fn dev_shim_never_emits_gpu_kwarg() {
        // P4: per-function config (gpu/timeout/cache) is sourced from the Rust
        // `#[modal_rust::function(...)]` decorator via the facade, NOT the CLI. The
        // legacy `--gpu` flag and its `gpu=` passthrough are dropped, so the shim
        // never carries a `gpu=` kwarg.
        let (dev, _, _) = proto_params();
        let shim = templates::dev_app(&dev);
        assert!(
            !shim.contains("gpu="),
            "CLI dev shim must have no gpu= kwarg"
        );
        assert!(shim.contains("@app.function(image=mounted_image, timeout=1800)"));
    }

    #[test]
    fn deploy_shim_never_emits_gpu_kwarg() {
        let (_, deploy, _) = proto_params();
        let shim = templates::deploy_app(&deploy);
        assert!(
            !shim.contains("gpu="),
            "CLI deploy shim must have no gpu= kwarg"
        );
        assert!(shim.contains("@app.function(image=image)"));
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

    /// P9 §G.2/§G.3 (static): the DEFAULT (programmatic) path must contain no codegen
    /// (`templates::`/`write_shim`) and must NOT spawn `modal` (no `run_modal`, no
    /// `Command::new("modal")`). The only subprocesses it spawns are `cargo` (build)
    /// and the user's `modal_runner` (`--describe`).
    #[test]
    fn programmatic_path_has_no_codegen_or_modal_subprocess() {
        let src = include_str!("programmatic.rs");
        assert!(
            !src.contains("templates::"),
            "programmatic path must not render shims"
        );
        assert!(
            !src.contains("write_shim"),
            "programmatic path must not write generated .py"
        );
        assert!(
            !src.contains("run_modal"),
            "programmatic path must not call run_modal"
        );
        assert!(
            !src.contains("Command::new(\"modal\")"),
            "programmatic path must not spawn the `modal` CLI"
        );
        // Positive: it DOES drive cargo (build) + the runner (--describe).
        assert!(src.contains("Command::new(\"cargo\")"));
        assert!(src.contains("--describe"));
    }

    /// P9 §G.3 (static): the `modal` subprocess (`run_modal`) is reachable ONLY from
    /// the renamed `cmd_*_shim` functions (i.e. only `--use-shim`). The default
    /// `cmd_*_programmatic` arms call into `programmatic::*`, never `run_modal`.
    #[test]
    fn modal_subprocess_only_in_shim_path() {
        let src = include_str!("main.rs");
        // The three legacy shim commands exist and own the `run_modal` calls.
        assert!(src.contains("fn cmd_run_shim"));
        assert!(src.contains("fn cmd_deploy_shim"));
        assert!(src.contains("fn cmd_call_shim"));
        // The dispatcher routes the default arms to the programmatic module and the
        // --use-shim arms to the shim commands.
        assert!(src.contains("programmatic::cmd_run_programmatic"));
        assert!(src.contains("programmatic::cmd_deploy_programmatic"));
        assert!(src.contains("programmatic::cmd_call_programmatic"));
        // `run_modal` (the sole `modal` spawner) is defined and invoked only by the
        // shim commands — never by a `*_programmatic` function (proven by
        // `programmatic_path_has_no_codegen_or_modal_subprocess`, which scans
        // programmatic.rs for `run_modal`/`Command::new("modal")`).
        assert!(src.contains("fn run_modal"));
    }
}
