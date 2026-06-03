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
