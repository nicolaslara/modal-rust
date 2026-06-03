//! Cargo workspace-root detection.
//!
//! The `run` shim mounts the cargo WORKSPACE ROOT as `/src` (and `deploy` copies it
//! to `/app/src`) so the runner's path-dependency runtime crate resolves — this
//! matches `workpads/prototype/dev_app.py`, which mounts the repo root (the dir
//! whose `Cargo.toml` carries `[workspace]`), not the `examples/add` project dir.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

/// Walk up from `start` (inclusive) to find the cargo WORKSPACE ROOT: the nearest
/// enclosing directory whose `Cargo.toml` declares a `[workspace]` table.
///
/// Falls back to `start` itself if no `[workspace]` manifest is found but `start`
/// has a `Cargo.toml` (a standalone crate is its own "workspace root" for mount
/// purposes). Errors if no `Cargo.toml` is found anywhere up the chain.
pub fn workspace_root(start: &Path) -> Result<PathBuf> {
    let start = start
        .canonicalize()
        .with_context(|| format!("project dir does not exist: {}", start.display()))?;

    let mut nearest_with_manifest: Option<PathBuf> = None;
    let mut dir: Option<&Path> = Some(&start);
    while let Some(d) = dir {
        let manifest = d.join("Cargo.toml");
        if manifest.is_file() {
            if nearest_with_manifest.is_none() {
                nearest_with_manifest = Some(d.to_path_buf());
            }
            let text = std::fs::read_to_string(&manifest)
                .with_context(|| format!("could not read {}", manifest.display()))?;
            if declares_workspace(&text) {
                return Ok(d.to_path_buf());
            }
        }
        dir = d.parent();
    }

    nearest_with_manifest.ok_or_else(|| {
        anyhow!(
            "no Cargo.toml found in {} or any parent directory",
            start.display()
        )
    })
}

/// Does this manifest text declare a `[workspace]` table?
fn declares_workspace(text: &str) -> bool {
    text.lines()
        .map(str::trim)
        .any(|l| l == "[workspace]" || l.starts_with("[workspace."))
}

/// Read the cargo PACKAGE name (`[package].name`) from `<project>/Cargo.toml`.
///
/// This is the name passed to `cargo build -p <name>` in the generated shims:
/// multiple workspace members share the `modal_runner` bin name, so a bare
/// `--bin modal_runner` is ambiguous and the build must be package-qualified
/// (boundaries.md §8). So `--project examples/add` (package `example-add`) builds
/// `-p example-add --bin modal_runner`.
///
/// Errors if `<project>/Cargo.toml` is missing, unreadable, or carries no
/// `[package].name` (e.g. a virtual-manifest / `[workspace]`-only directory — the
/// user must point `--project` at a concrete package).
pub fn package_name(project: &Path) -> Result<String> {
    let project = project
        .canonicalize()
        .with_context(|| format!("project dir does not exist: {}", project.display()))?;
    let manifest = project.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest)
        .with_context(|| format!("could not read {}", manifest.display()))?;
    package_name_in_manifest(&text).ok_or_else(|| {
        anyhow!(
            "no [package].name in {} — point --project at a concrete package directory \
             (e.g. examples/add), not a workspace/virtual manifest",
            manifest.display()
        )
    })
}

/// Extract `[package].name` from a manifest's text. A tolerant line-scan that
/// tracks the active TOML table header (no TOML-parser dependency, keeping the
/// CLI's dep surface minimal — matching the doctor's hand-rolled scanner). Only
/// the `[package]` table's `name = "..."` is read; `[workspace.package]` etc. are
/// ignored.
fn package_name_in_manifest(text: &str) -> Option<String> {
    let mut in_package = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_package = line == "[package]";
            continue;
        }
        if in_package {
            if let Some(rest) = line.strip_prefix("name") {
                let rest = rest.trim_start();
                if let Some(rest) = rest.strip_prefix('=') {
                    let v = rest.trim().trim_matches(|c| c == '"' || c == '\'');
                    if !v.is_empty() {
                        return Some(v.to_string());
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_table_detected() {
        assert!(declares_workspace("[workspace]\nmembers = []\n"));
        assert!(declares_workspace(
            "[workspace.package]\nedition = \"2021\"\n"
        ));
        assert!(!declares_workspace(
            "[package]\nname = \"x\"\n[dependencies]\n"
        ));
    }

    #[test]
    fn package_name_read_from_package_table() {
        let text = "[package]\nname = \"example-add\"\nedition = \"2021\"\n";
        assert_eq!(
            package_name_in_manifest(text).as_deref(),
            Some("example-add")
        );
    }

    #[test]
    fn package_name_ignores_workspace_package_table() {
        // A virtual/workspace manifest with [workspace.package] but no [package]
        // must NOT yield a package name (the user must target a concrete crate).
        let text = "[workspace]\nmembers = []\n[workspace.package]\nname = \"nope\"\n";
        assert_eq!(package_name_in_manifest(text), None);
    }

    #[test]
    fn package_name_reads_only_package_table_name() {
        // `name` appears in multiple tables; only [package].name counts.
        let text =
            "[package]\nname = \"example-cuda-vector-add\"\n[[bin]]\nname = \"modal_runner\"\n";
        assert_eq!(
            package_name_in_manifest(text).as_deref(),
            Some("example-cuda-vector-add")
        );
    }

    #[test]
    fn package_name_from_project_dir() {
        let dir = std::env::temp_dir().join(format!("mr-pkg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"example-burn-add\"\n",
        )
        .unwrap();
        assert_eq!(package_name(&dir).unwrap(), "example-burn-add");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finds_root_from_member_dir() {
        let base = std::env::temp_dir().join(format!("mr-ws-{}", std::process::id()));
        let member = base.join("examples").join("add");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(base.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        std::fs::write(member.join("Cargo.toml"), "[package]\nname = \"add\"\n").unwrap();

        let root = workspace_root(&member).unwrap();
        assert_eq!(root, base.canonicalize().unwrap());

        let _ = std::fs::remove_dir_all(&base);
    }
}
