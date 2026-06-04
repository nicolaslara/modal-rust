//! `modal-rust doctor` — an OFFLINE preflight (boundaries.md §8, tasks.md M9b).
//!
//! Checks, in order:
//!   - Modal credentials are present (`~/.modal.toml` or `MODAL_TOKEN_*`).
//!   - with `--rust`: `cargo` and `rustc` are present, AND the resolved release
//!     profile is NOT `panic = "abort"` (which would silently degrade the frozen
//!     `panic` error kind into a raw process abort — boundaries.md §6).
//!
//! On a missing prerequisite the process exits non-zero and emits an actionable
//! **structured error reusing the runner's JSON envelope shape** (boundaries.md §2):
//! `{"ok":false,"error":{"kind":..,"message":..,"details":<json|null>,"backtrace":""}}`.
//! This is M9b's own boundary — it carries no wrapper/shim behavior (that is M9a).

use std::path::PathBuf;
use std::process::Command;

use serde_json::{json, Value};

/// Outcome of a single preflight check.
enum Check {
    /// The check passed; the string is a human-readable detail line.
    Ok(String),
    /// A fatal failure with an actionable structured error (the runner-envelope
    /// shape). The process will exit non-zero. Per boundaries.md §6 the
    /// `panic = "abort"` profile is a FAIL (the correctness gate for the `panic`
    /// kind), not a soft warning.
    Fail(Value),
}

/// Build the runner-shaped failure envelope (boundaries.md §2). `kind` is a
/// doctor-specific discriminant; `details` carries actionable remediation data.
fn fail_envelope(kind: &str, message: impl Into<String>, details: Value) -> Value {
    json!({
        "ok": false,
        "error": {
            "kind": kind,
            "message": message.into(),
            "details": details,
            "backtrace": "",
        }
    })
}

/// Locate an executable on `$PATH` (cross-platform-ish; we only target unix here).
fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(bin);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Run `<bin> <args...>` and capture trimmed stdout (falling back to stderr), or
/// `None` if the command could not be spawned or exited non-zero.
fn capture_version(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let mut s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        s = String::from_utf8_lossy(&out.stderr).trim().to_string();
    }
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Check: Modal credentials are present — `~/.modal.toml` OR `MODAL_TOKEN_ID` +
/// `MODAL_TOKEN_SECRET` environment variables.
fn check_modal_credentials() -> Check {
    let home_toml = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".modal.toml"));
    let toml_present = home_toml.as_ref().map(|p| p.is_file()).unwrap_or(false);
    let env_id = std::env::var("MODAL_TOKEN_ID").is_ok();
    let env_secret = std::env::var("MODAL_TOKEN_SECRET").is_ok();
    let env_present = env_id && env_secret;

    if toml_present {
        let p = home_toml.unwrap();
        Check::Ok(format!("Modal credentials: {}", p.display()))
    } else if env_present {
        Check::Ok("Modal credentials: MODAL_TOKEN_ID + MODAL_TOKEN_SECRET (env)".to_string())
    } else {
        Check::Fail(fail_envelope(
            "missing_prerequisite",
            "no Modal credentials found (~/.modal.toml or MODAL_TOKEN_ID/MODAL_TOKEN_SECRET)",
            json!({
                "prerequisite": "Modal credentials",
                "checked": {
                    "~/.modal.toml": toml_present,
                    "MODAL_TOKEN_ID": env_id,
                    "MODAL_TOKEN_SECRET": env_secret,
                },
                "remediation": "Run `modal token new` to create ~/.modal.toml, or export MODAL_TOKEN_ID and MODAL_TOKEN_SECRET."
            }),
        ))
    }
}

/// Check: `cargo` is on `$PATH` (`--rust`).
fn check_cargo() -> Check {
    match (which("cargo"), capture_version("cargo", &["--version"])) {
        (Some(path), Some(ver)) => Check::Ok(format!("cargo: {} ({})", ver, path.display())),
        _ => Check::Fail(fail_envelope(
            "missing_prerequisite",
            "`cargo` not found on $PATH (required for the run/deploy build stage)",
            json!({
                "prerequisite": "cargo",
                "remediation": "Install Rust via https://rustup.rs (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)."
            }),
        )),
    }
}

/// Check: `rustc` is on `$PATH` (`--rust`).
fn check_rustc() -> Check {
    match (which("rustc"), capture_version("rustc", &["--version"])) {
        (Some(path), Some(ver)) => Check::Ok(format!("rustc: {} ({})", ver, path.display())),
        _ => Check::Fail(fail_envelope(
            "missing_prerequisite",
            "`rustc` not found on $PATH (required for the run/deploy build stage)",
            json!({
                "prerequisite": "rustc",
                "remediation": "Install Rust via https://rustup.rs."
            }),
        )),
    }
}

/// Detect `panic = "abort"` in the resolved release profile (`--rust`). The
/// release profile is resolved by parsing `[profile.release]` from the nearest
/// enclosing cargo manifest chain: a project `Cargo.toml`, then walking up to the
/// workspace-root `Cargo.toml` (the one with `[workspace]`). `panic = "abort"` in
/// the resolved release profile would silently degrade the frozen `panic` error
/// kind into a raw process abort, so it is FAILED here (boundaries.md §6).
///
/// `manifest_dir` is the project directory the user is operating on (the `run`/
/// `deploy` `--project`, defaulting to the current working dir for `doctor`).
fn check_panic_profile(manifest_dir: &std::path::Path) -> Check {
    // Resolve the release-profile `panic` setting across the manifest chain.
    // Cargo resolves `[profile.*]` only from the workspace root, but a non-root
    // project manifest may also carry one (cargo errors on that in a workspace,
    // but we are conservative and inspect both, preferring the workspace root).
    let mut found: Option<(String, PathBuf)> = None;

    let mut dir = Some(manifest_dir.to_path_buf());
    let mut is_root = false;
    while let Some(d) = dir {
        let manifest = d.join("Cargo.toml");
        if manifest.is_file() {
            // A manifest we cannot read is skipped (non-fatal): the abort check is a
            // best-effort guard, and an unreadable manifest is not itself an abort.
            if let Ok(text) = std::fs::read_to_string(&manifest) {
                let is_workspace_root = manifest_declares_workspace(&text);
                if let Some(panic_val) = release_profile_panic(&text) {
                    // Workspace-root setting wins; record the first one found walking
                    // up, but a root setting overrides a member one.
                    if is_workspace_root || found.is_none() {
                        found = Some((panic_val, manifest.clone()));
                    }
                }
                if is_workspace_root {
                    is_root = true;
                    break;
                }
            }
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }

    match found {
        Some((val, manifest)) if val == "abort" => Check::Fail(fail_envelope(
            "panic_abort_profile",
            "release profile is `panic = \"abort\"` — this breaks the `panic` error envelope",
            json!({
                "manifest": manifest.display().to_string(),
                "resolved_release_panic": val,
                "why": "catch_unwind requires the unwind strategy; abort degrades the structured `panic` kind into a raw process abort (boundaries.md §6).",
                "remediation": "Set `[profile.release] panic = \"unwind\"` in the workspace-root Cargo.toml (or remove the abort override)."
            }),
        )),
        Some((val, manifest)) => Check::Ok(format!(
            "release profile panic = \"{val}\" ({})",
            manifest.display()
        )),
        None => {
            let where_ = if is_root {
                "workspace root"
            } else {
                "manifest chain"
            };
            // No explicit setting => cargo default is `unwind` for the release
            // profile, which is correct for the `panic` envelope.
            Check::Ok(format!(
                "release profile panic = \"unwind\" (cargo default; no override in {where_})"
            ))
        }
    }
}

/// Does this manifest text declare a `[workspace]` table (i.e. is it a workspace
/// root)? A tolerant line-scan (no TOML parser dependency).
fn manifest_declares_workspace(text: &str) -> bool {
    text.lines()
        .map(str::trim)
        .any(|l| l == "[workspace]" || l.starts_with("[workspace.") || l.starts_with("[workspace]"))
}

/// Extract the `panic = "..."` value from a `[profile.release]` table, if present.
/// A tolerant line-scan that tracks the active TOML table header (no TOML parser
/// dependency, keeping the CLI's dep surface minimal).
fn release_profile_panic(text: &str) -> Option<String> {
    let mut in_release = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_release = line == "[profile.release]";
            continue;
        }
        if in_release {
            if let Some(rest) = line.strip_prefix("panic") {
                let rest = rest.trim_start();
                if let Some(rest) = rest.strip_prefix('=') {
                    let v = rest.trim().trim_matches(|c| c == '"' || c == '\'');
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// Run the full `doctor` preflight. `with_rust` enables the `--rust` checks.
/// `project_dir` is the directory whose manifest chain is inspected for the
/// `panic = "abort"` profile check (the cwd for a bare `doctor`).
///
/// The path is programmatic (P9/P10): it connects to Modal directly and never
/// spawns `modal`, so the `modal` CLI is NOT a requirement. Only auth (always)
/// and cargo/rustc (under `--rust`, load-bearing because the programmatic path
/// runs a local `cargo build` for `--describe`) are required.
///
/// Prints a clear report to stdout. Returns the process exit code: `0` if every
/// check passed; `1` if any check FAILED (the first failure's structured error is
/// printed to stderr as a single JSON envelope line, reusing the runner shape).
pub fn run(with_rust: bool, project_dir: &std::path::Path) -> i32 {
    println!("modal-rust doctor — preflight (OFFLINE)");
    println!("(programmatic path; the modal CLI is not required — auth + --rust cargo/rustc only)");
    if with_rust {
        println!("(--rust: also checking cargo/rustc and the release panic profile)");
    }
    println!();

    // Auth is ALWAYS a hard requirement (the programmatic path's `App::connect_*`
    // reads ~/.modal.toml / MODAL_TOKEN_*). cargo/rustc/panic under --rust.
    let mut checks: Vec<Check> = vec![check_modal_credentials()];
    if with_rust {
        checks.push(check_cargo());
        checks.push(check_rustc());
        checks.push(check_panic_profile(project_dir));
    }

    let mut first_failure: Option<Value> = None;
    for check in &checks {
        match check {
            Check::Ok(msg) => println!("  [ok]   {msg}"),
            Check::Fail(env) => {
                let msg = env["error"]["message"]
                    .as_str()
                    .unwrap_or("preflight failure");
                println!("  [FAIL] {msg}");
                if first_failure.is_none() {
                    first_failure = Some(env.clone());
                }
            }
        }
    }

    println!();
    match first_failure {
        None => {
            println!("All preflight checks passed.");
            0
        }
        Some(env) => {
            println!("Preflight FAILED. Structured error (runner-envelope shape) on stderr.");
            // Emit exactly one JSON envelope line to stderr (the actionable error).
            eprintln!("{}", serde_json::to_string(&env).unwrap_or_default());
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_panic_abort_in_release_profile() {
        let text = "[package]\nname=\"x\"\n[profile.release]\npanic = \"abort\"\n";
        assert_eq!(release_profile_panic(text).as_deref(), Some("abort"));
    }

    #[test]
    fn detects_panic_unwind_in_release_profile() {
        let text = "[profile.release]\npanic = \"unwind\"\n[profile.dev]\npanic = \"abort\"\n";
        // Must read the release table, not the dev table.
        assert_eq!(release_profile_panic(text).as_deref(), Some("unwind"));
    }

    #[test]
    fn no_release_panic_setting_is_none() {
        let text = "[package]\nname=\"x\"\n[profile.dev]\npanic = \"abort\"\n";
        assert_eq!(release_profile_panic(text), None);
    }

    #[test]
    fn workspace_root_detected() {
        assert!(manifest_declares_workspace("[workspace]\nmembers = []\n"));
        assert!(manifest_declares_workspace(
            "[workspace.package]\nedition = \"2021\"\n"
        ));
        assert!(!manifest_declares_workspace("[package]\nname = \"x\"\n"));
    }

    #[test]
    fn fail_envelope_has_runner_shape() {
        let env = fail_envelope("missing_prerequisite", "boom", json!({"k": "v"}));
        assert_eq!(env["ok"], false);
        assert_eq!(env["error"]["kind"], "missing_prerequisite");
        assert_eq!(env["error"]["message"], "boom");
        assert_eq!(env["error"]["details"]["k"], "v");
        assert_eq!(env["error"]["backtrace"], "");
    }

    #[test]
    fn abort_profile_check_fails_against_abort_manifest() {
        let dir = std::env::temp_dir().join(format!("mr-doctor-abort-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[workspace]\nmembers = []\n[profile.release]\npanic = \"abort\"\n",
        )
        .unwrap();
        match check_panic_profile(&dir) {
            Check::Fail(env) => {
                assert_eq!(env["error"]["kind"], "panic_abort_profile");
                assert_eq!(env["error"]["details"]["resolved_release_panic"], "abort");
            }
            _ => panic!("expected Fail for panic = abort"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unwind_profile_check_passes() {
        let dir = std::env::temp_dir().join(format!("mr-doctor-unwind-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[workspace]\nmembers = []\n[profile.release]\npanic = \"unwind\"\n",
        )
        .unwrap();
        assert!(matches!(check_panic_profile(&dir), Check::Ok(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
