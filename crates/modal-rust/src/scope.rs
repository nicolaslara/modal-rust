//! cargo-metadata source SCOPING — pick exactly the crate directories the target
//! package's build needs, instead of uploading the whole workspace tree.
//!
//! The PRIMARY source-upload selector. Shelling out to `cargo metadata
//! --format-version 1 --no-deps` at the workspace root yields the workspace member
//! set and each member's normal (non-dev, non-build) path dependencies. From the
//! target package we compute its workspace-member path-dep CLOSURE — exactly the
//! crate directories `cargo build -p <target> --bin modal_runner` needs — plus the
//! workspace `Cargo.toml`/`Cargo.lock`. Everything else (sibling crates, vendored
//! reference clones, planning docs, datasets) is left out by construction.
//!
//! `--no-deps` is sufficient: the path-dep closure is computed from
//! `packages[].dependencies[].path` without resolving the crates.io graph (faster,
//! no network — crates.io deps are fetched by cargo on Modal at build time).
//!
//! Returns `None` (→ the caller's whole-root fallback) whenever scoping cannot be
//! trusted: `cargo metadata` missing / non-zero / unparseable, no workspace
//! `Cargo.toml`, or the target package absent from the metadata.
//!
//! ## Workspace-manifest rewrite (consistency)
//!
//! Uploading the workspace `Cargo.toml` VERBATIM alongside only a SUBSET of its
//! members is inconsistent: cargo loads EVERY `[workspace] members` entry and aborts
//! ("failed to load manifest for workspace member …") when a member dir is absent.
//! So we rewrite the uploaded `Cargo.toml`'s `members`/`default-members` arrays down
//! to exactly the uploaded closure crates (format-preserving via `toml_edit`, so
//! `[profile.release] panic = "unwind"` — the runner's panic-capture invariant — and
//! all comments survive). The rewrite is in-memory; the on-disk `Cargo.toml` is
//! untouched. See [`workspace_closure`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

/// Top-level `cargo metadata --format-version 1` shape (only the fields we read).
#[derive(Debug, Deserialize)]
struct Metadata {
    /// Package-id strings of the workspace members.
    workspace_members: Vec<String>,
    /// Absolute workspace root (holds the workspace `Cargo.toml`/`Cargo.lock`).
    workspace_root: PathBuf,
    /// All packages in the graph (with `--no-deps`, just the workspace members).
    packages: Vec<Package>,
}

#[derive(Debug, Deserialize)]
struct Package {
    /// Opaque package-id (matches entries in `workspace_members`).
    id: String,
    /// Cargo package name (matched against the scoping target).
    name: String,
    /// Absolute path to this package's `Cargo.toml`.
    manifest_path: PathBuf,
    /// This package's declared dependencies.
    dependencies: Vec<Dependency>,
}

#[derive(Debug, Deserialize)]
struct Dependency {
    /// Dependency-kind: absent/`null` = normal, `"dev"`, or `"build"`. Only normal
    /// deps are followed (dev/build deps are not needed to build the runner binary
    /// and dev deps can form cycles — e.g. `modal-rust` dev-deps on `example-add`).
    #[serde(default)]
    kind: Option<String>,
    /// Filesystem path for a path dependency (`None` for a registry/git dep).
    #[serde(default)]
    path: Option<PathBuf>,
}

/// The scoped source-upload set for a target package.
pub(crate) struct ClosureUpload {
    /// Closure crate directories to walk + upload (the target's workspace-member
    /// normal path-dep closure).
    pub dirs: Vec<PathBuf>,
    /// On-disk extra files uploaded verbatim (the workspace `Cargo.lock`).
    pub extra_files: Vec<PathBuf>,
    /// In-memory extra files uploaded by `(workspace-relative posix path, bytes)`:
    /// the REWRITTEN workspace `Cargo.toml` whose `members`/`default-members` are
    /// scoped to [`dirs`](Self::dirs). Kept separate so the SDK uploads these bytes
    /// rather than re-reading the verbatim on-disk manifest.
    pub inline_files: Vec<(String, Vec<u8>)>,
}

/// Compute the scoped source-upload set for `package` rooted at `workspace_root`:
/// the dependency-closure crate dirs, the workspace `Cargo.lock` (verbatim), and the
/// REWRITTEN workspace `Cargo.toml` (members scoped to the closure — see module docs).
///
/// Returns `None` to signal the caller's whole-root fallback (see module docs).
pub(crate) fn workspace_closure(workspace_root: &Path, package: &str) -> Option<ClosureUpload> {
    let manifest = workspace_root.join("Cargo.toml");
    if !manifest.is_file() {
        eprintln!(
            "[modal-rust] cargo metadata unavailable (no Cargo.toml at {}); \
             uploading whole source root minus ignore files",
            workspace_root.display()
        );
        return None;
    }

    // `?` returns None on any failure; run_cargo_metadata already logged the reason.
    let metadata = run_cargo_metadata(&manifest)?;

    let Some(dirs) = closure_from_metadata(&metadata, package) else {
        eprintln!(
            "[modal-rust] cargo metadata unavailable (package '{package}' not found in \
             workspace members); uploading whole source root minus ignore files"
        );
        return None;
    };

    let ws_root = &metadata.workspace_root;
    let ws_manifest = ws_root.join("Cargo.toml");

    // Rewrite the workspace Cargo.toml's member arrays to the closure subset so the
    // uploaded workspace is self-consistent (cargo loads only present members).
    let original = match std::fs::read_to_string(&ws_manifest) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "[modal-rust] cargo metadata unavailable (cannot read {}: {e}); \
                 uploading whole source root minus ignore files",
                ws_manifest.display()
            );
            return None;
        }
    };
    let rewritten = match rewrite_workspace_members(&original, ws_root, &dirs) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "[modal-rust] cargo metadata unavailable (cannot rewrite {} members: {e}); \
                 uploading whole source root minus ignore files",
                ws_manifest.display()
            );
            return None;
        }
    };

    // Cargo.lock is uploaded verbatim (it lists ALL workspace + registry deps, which
    // is harmless — cargo ignores lock entries for crates not in the trimmed members).
    let extra_files = [ws_root.join("Cargo.lock")]
        .into_iter()
        .filter(|p| p.is_file())
        .collect();

    Some(ClosureUpload {
        dirs,
        extra_files,
        inline_files: vec![("Cargo.toml".to_string(), rewritten.into_bytes())],
    })
}

/// Rewrite the workspace manifest's `[workspace] members` and `default-members`
/// arrays to exactly the closure `dirs` (as workspace-root-relative POSIX paths),
/// preserving everything else (`resolver`, `[profile.*]`, comments) byte-for-byte
/// via `toml_edit`. A `[workspace] exclude` array, if present, is cleared (the
/// excluded dirs aren't uploaded, so excluding them is moot and could reference a
/// missing path).
fn rewrite_workspace_members(
    original: &str,
    workspace_root: &Path,
    dirs: &[PathBuf],
) -> Result<String, String> {
    use toml_edit::{Array, DocumentMut, Item, Value};

    let mut doc: DocumentMut = original
        .parse()
        .map_err(|e| format!("parse Cargo.toml: {e}"))?;

    // Closure dirs as workspace-relative POSIX strings, sorted + deduped.
    let mut rel: Vec<String> = dirs
        .iter()
        .filter_map(|d| d.strip_prefix(workspace_root).ok())
        .map(|r| {
            r.components()
                .filter_map(|c| c.as_os_str().to_str())
                .collect::<Vec<_>>()
                .join("/")
        })
        .filter(|s| !s.is_empty())
        .collect();
    rel.sort();
    rel.dedup();

    let ws = doc
        .get_mut("workspace")
        .and_then(Item::as_table_like_mut)
        .ok_or("no [workspace] table")?;

    // Build a fresh array of the closure-relative member paths.
    let make_array = || {
        let mut arr = Array::new();
        for m in &rel {
            arr.push(m.as_str());
        }
        arr
    };

    // `members` must exist; `default-members`/`exclude` only if originally present.
    ws.insert("members", Item::Value(Value::Array(make_array())));
    if ws.contains_key("default-members") {
        ws.insert("default-members", Item::Value(Value::Array(make_array())));
    }
    if ws.contains_key("exclude") {
        ws.insert("exclude", Item::Value(Value::Array(Array::new())));
    }

    Ok(doc.to_string())
}

/// Run `cargo metadata --format-version 1 --no-deps --manifest-path <manifest>` and
/// parse stdout. Returns `None` (with a stderr note) on any failure.
fn run_cargo_metadata(manifest: &Path) -> Option<Metadata> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .arg("--manifest-path")
        .arg(manifest)
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            eprintln!(
                "[modal-rust] cargo metadata unavailable (failed to spawn cargo: {e}); \
                 uploading whole source root minus ignore files"
            );
            return None;
        }
    };
    if !output.status.success() {
        eprintln!(
            "[modal-rust] cargo metadata unavailable (cargo exited {}); \
             uploading whole source root minus ignore files",
            output.status
        );
        return None;
    }
    match serde_json::from_slice::<Metadata>(&output.stdout) {
        Ok(m) => Some(m),
        Err(e) => {
            eprintln!(
                "[modal-rust] cargo metadata unavailable (unparseable JSON: {e}); \
                 uploading whole source root minus ignore files"
            );
            None
        }
    }
}

/// Pure closure algorithm over parsed metadata. Returns the workspace-member normal
/// path-dep closure crate dirs of `package`, or `None` if `package` is not a member.
///
/// Split out from I/O so it is unit-testable on fixtures (no cargo invocation).
fn closure_from_metadata(metadata: &Metadata, package: &str) -> Option<Vec<PathBuf>> {
    let member_ids: HashSet<&str> = metadata
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect();

    // dir(p) = manifest_path without the trailing "/Cargo.toml".
    let dir_of = |p: &Package| -> PathBuf {
        p.manifest_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
    };

    // Member crate dirs (the only dirs the closure may include).
    let member_dirs: HashSet<PathBuf> = metadata
        .packages
        .iter()
        .filter(|p| member_ids.contains(p.id.as_str()))
        .map(&dir_of)
        .collect();

    // Index members by their crate dir so we can hop dep.path -> package.
    let by_dir: HashMap<PathBuf, &Package> = metadata
        .packages
        .iter()
        .filter(|p| member_ids.contains(p.id.as_str()))
        .map(|p| (dir_of(p), p))
        .collect();

    // The scoping target must itself be a workspace member.
    let target = metadata
        .packages
        .iter()
        .find(|p| p.name == package && member_ids.contains(p.id.as_str()))?;

    let mut closure: HashSet<PathBuf> = HashSet::new();
    let mut stack: Vec<PathBuf> = vec![dir_of(target)];
    while let Some(cur) = stack.pop() {
        if !closure.insert(cur.clone()) {
            continue;
        }
        let Some(pkg) = by_dir.get(&cur) else {
            continue;
        };
        for dep in &pkg.dependencies {
            // Follow ONLY normal (kind == null) path deps that are workspace members.
            if dep.kind.is_some() {
                continue; // dev / build dep — not needed to build the runner binary.
            }
            let Some(dep_path) = &dep.path else {
                continue; // registry / git dep — fetched by cargo on Modal.
            };
            if member_dirs.contains(dep_path) && !closure.contains(dep_path) {
                stack.push(dep_path.clone());
            }
        }
    }

    // Deterministic order (the upload sorts by mount path anyway, but stable output
    // keeps logs/tests predictable).
    let mut dirs: Vec<PathBuf> = closure.into_iter().collect();
    dirs.sort();
    Some(dirs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `Metadata` fixture mirroring the real workspace shape: `example-add`
    /// has a normal path-dep on `runtime`; `modal-rust` has normal path-deps on
    /// `runtime`/`macros`/`sdk` and a DEV path-dep on `example-add`.
    fn fixture() -> Metadata {
        Metadata {
            workspace_root: PathBuf::from("/ws"),
            workspace_members: vec![
                "sdk-id".into(),
                "rt-id".into(),
                "macros-id".into(),
                "facade-id".into(),
                "addex-id".into(),
            ],
            packages: vec![
                Package {
                    id: "rt-id".into(),
                    name: "modal-rust-runtime".into(),
                    manifest_path: "/ws/crates/runtime/Cargo.toml".into(),
                    dependencies: vec![],
                },
                Package {
                    id: "macros-id".into(),
                    name: "modal-rust-macros".into(),
                    manifest_path: "/ws/crates/macros/Cargo.toml".into(),
                    dependencies: vec![],
                },
                Package {
                    id: "sdk-id".into(),
                    name: "modal-rust-sdk".into(),
                    manifest_path: "/ws/crates/sdk/Cargo.toml".into(),
                    dependencies: vec![],
                },
                Package {
                    id: "addex-id".into(),
                    name: "example-add".into(),
                    manifest_path: "/ws/examples/add/Cargo.toml".into(),
                    dependencies: vec![
                        // normal path-dep on runtime
                        Dependency {
                            kind: None,
                            path: Some("/ws/crates/runtime".into()),
                        },
                        // a registry dep (no path) — ignored
                        Dependency {
                            kind: None,
                            path: None,
                        },
                    ],
                },
                Package {
                    id: "facade-id".into(),
                    name: "modal-rust".into(),
                    manifest_path: "/ws/crates/facade/Cargo.toml".into(),
                    dependencies: vec![
                        Dependency {
                            kind: None,
                            path: Some("/ws/crates/runtime".into()),
                        },
                        Dependency {
                            kind: None,
                            path: Some("/ws/crates/macros".into()),
                        },
                        Dependency {
                            kind: None,
                            path: Some("/ws/crates/sdk".into()),
                        },
                        // DEV path-dep on example-add — MUST be excluded.
                        Dependency {
                            kind: Some("dev".into()),
                            path: Some("/ws/examples/add".into()),
                        },
                    ],
                },
            ],
        }
    }

    #[test]
    fn closure_for_example_add_is_self_plus_runtime() {
        let m = fixture();
        let dirs = closure_from_metadata(&m, "example-add").unwrap();
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/ws/crates/runtime"),
                PathBuf::from("/ws/examples/add"),
            ],
            "example-add closure = {{examples/add, crates/runtime}} only"
        );
    }

    #[test]
    fn closure_for_facade_excludes_dev_dep_on_example() {
        let m = fixture();
        let dirs = closure_from_metadata(&m, "modal-rust").unwrap();
        // The dev-dep on example-add must NOT appear; normal deps must.
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/ws/crates/facade"),
                PathBuf::from("/ws/crates/macros"),
                PathBuf::from("/ws/crates/runtime"),
                PathBuf::from("/ws/crates/sdk"),
            ],
        );
        assert!(
            !dirs.contains(&PathBuf::from("/ws/examples/add")),
            "the dev path-dep on example-add must be excluded"
        );
    }

    #[test]
    fn unknown_package_returns_none() {
        let m = fixture();
        assert!(closure_from_metadata(&m, "not-a-member").is_none());
    }

    #[test]
    fn rewrite_scopes_members_and_preserves_profile() {
        // The real-shaped workspace manifest: 9 members, default-members (minus the
        // CUDA one), and the load-bearing [profile.release] panic = "unwind".
        let original = r#"[workspace]
resolver = "2"
members = [
    "crates/modal-rust-sdk",
    "crates/modal-rust-runtime",
    "examples/add",
    "examples/burn-add",
]
default-members = [
    "crates/modal-rust-sdk",
    "crates/modal-rust-runtime",
    "examples/add",
]

# panic-capture invariant: keep unwind.
[profile.release]
panic = "unwind"
"#;
        let ws_root = PathBuf::from("/ws");
        // Closure = example-add + its runtime dep only.
        let dirs = vec![
            PathBuf::from("/ws/crates/modal-rust-runtime"),
            PathBuf::from("/ws/examples/add"),
        ];
        let out = rewrite_workspace_members(original, &ws_root, &dirs).unwrap();

        // It still parses, and members/default-members are the closure subset.
        let doc: toml_edit::DocumentMut = out.parse().unwrap();
        let members: Vec<String> = doc["workspace"]["members"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            members,
            vec!["crates/modal-rust-runtime", "examples/add"],
            "members scoped to the closure"
        );
        let dm: Vec<String> = doc["workspace"]["default-members"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(dm, vec!["crates/modal-rust-runtime", "examples/add"]);
        // The other 2 members (sdk, burn-add) are GONE (not uploaded → would not load).
        assert!(!members.iter().any(|m| m.contains("burn-add")));
        assert!(!members.iter().any(|m| m.contains("modal-rust-sdk")));
        // The panic-capture profile invariant survives the rewrite.
        assert_eq!(
            doc["profile"]["release"]["panic"].as_str(),
            Some("unwind"),
            "[profile.release] panic = unwind must be preserved"
        );
    }

    #[test]
    fn rewrite_handles_manifest_without_default_members() {
        let original = "[workspace]\nmembers = [\"a\", \"b\"]\n";
        let dirs = vec![PathBuf::from("/ws/a")];
        let out = rewrite_workspace_members(original, &PathBuf::from("/ws"), &dirs).unwrap();
        let doc: toml_edit::DocumentMut = out.parse().unwrap();
        let members: Vec<String> = doc["workspace"]["members"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(members, vec!["a"]);
        // No default-members originally → none added.
        assert!(doc["workspace"].get("default-members").is_none());
    }
}
