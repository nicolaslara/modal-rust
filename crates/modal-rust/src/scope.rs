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
///
/// `pub(crate)` so [`crate::runner_gen`] can reuse the SAME parsed metadata (one
/// `cargo metadata` call) for auto-detect + lib-name + crate-rel resolution.
#[derive(Debug, Deserialize)]
pub(crate) struct Metadata {
    /// Package-id strings of the workspace members.
    pub workspace_members: Vec<String>,
    /// Absolute workspace root (holds the workspace `Cargo.toml`/`Cargo.lock`).
    pub workspace_root: PathBuf,
    /// All packages in the graph (with `--no-deps`, just the workspace members).
    pub packages: Vec<Package>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Package {
    /// Opaque package-id (matches entries in `workspace_members`).
    pub id: String,
    /// Cargo package name (matched against the scoping target).
    pub name: String,
    /// Absolute path to this package's `Cargo.toml`.
    pub manifest_path: PathBuf,
    /// This package's declared dependencies.
    pub dependencies: Vec<Dependency>,
    /// Build targets of this package (kind + name). Used by [`crate::runner_gen`] to
    /// detect an existing `modal_runner` bin and to read the `[lib]` name. Defaults to
    /// empty if `cargo metadata` omits it (older cargo) — the closure algorithm here
    /// never reads it.
    #[serde(default)]
    pub targets: Vec<Target>,
}

/// A single `cargo metadata` build target (`targets[]` per package): the `kind`s
/// (`["lib"]`, `["bin"]`, `["cdylib"]`, …) and the target `name`.
#[derive(Debug, Deserialize)]
pub(crate) struct Target {
    /// Target kinds (e.g. `["bin"]`, `["lib"]`). Auto-detect matches a kind that
    /// CONTAINS `"bin"`.
    #[serde(default)]
    pub kind: Vec<String>,
    /// Target name (e.g. `"modal_runner"`, the crate's lib name).
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Dependency {
    /// The dependency package name (e.g. `"modal-rust"`). Used only for diagnostics —
    /// to name the offending crate when a normal path-dep points outside the upload.
    #[serde(default)]
    name: String,
    /// Dependency-kind: absent/`null` = normal, `"dev"`, or `"build"`. Only normal
    /// deps are followed (dev/build deps are not needed to build the runner binary
    /// and dev deps can form cycles — e.g. `modal-rust` dev-deps on `example-add`).
    #[serde(default)]
    kind: Option<String>,
    /// Filesystem path for a path dependency, ALWAYS absolute + canonicalized by cargo
    /// (`None` for a registry/git dep).
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
/// Returns:
/// - `Ok(Some(_))` — the scoped upload set;
/// - `Ok(None)` — soft fallback to the caller's whole-root upload (metadata
///   unavailable/unparseable, no workspace `Cargo.toml`, or the target package absent);
/// - `Err(msg)` — a HARD, actionable failure that the caller must surface (NOT fall
///   back on): a normal path-dep escapes the workspace, so its source can't be uploaded
///   and the remote `cargo build` would fail with a cryptic "No such file" error. The
///   whole-root fallback would fail the same way (the dep is outside `local_root` too),
///   so failing loudly here is strictly better. See [`out_of_workspace_error`].
///
/// Called only from the client `control_plane` (source-mount upload) + tests, so the
/// LIGHT build allows it dead.
#[cfg_attr(not(feature = "client"), allow(dead_code))]
pub(crate) fn workspace_closure(
    workspace_root: &Path,
    package: &str,
) -> Result<Option<ClosureUpload>, String> {
    let manifest = workspace_root.join("Cargo.toml");
    if !manifest.is_file() {
        eprintln!(
            "[modal-rust] cargo metadata unavailable (no Cargo.toml at {}); \
             uploading whole source root minus ignore files",
            workspace_root.display()
        );
        return Ok(None);
    }

    // run_cargo_metadata already logged the reason on failure; soft-fall-back.
    let Some(metadata) = run_cargo_metadata(&manifest) else {
        return Ok(None);
    };

    let Some(ClosureResult {
        dirs,
        out_of_workspace,
    }) = closure_from_metadata(&metadata, package)
    else {
        eprintln!(
            "[modal-rust] cargo metadata unavailable (package '{package}' not found in \
             workspace members); uploading whole source root minus ignore files"
        );
        return Ok(None);
    };

    // A normal path-dep escaping the workspace can't be uploaded — fail loudly with an
    // actionable message instead of letting the remote build fail cryptically.
    if !out_of_workspace.is_empty() {
        return Err(out_of_workspace_error(package, &out_of_workspace));
    }

    Ok(build_closure_upload(&metadata, package, dirs))
}

/// The cargo-metadata dependency-closure crate dirs of `package` rooted at
/// `workspace_root` — exactly the set of directories whose `.rs`/`Cargo.toml` source can
/// change what the runner's `--describe` emits (the registry + per-entrypoint configs).
///
/// This is the LENIENT closure (it tolerates out-of-workspace path-deps, like the
/// `--describe` shadow build), but it returns ONLY the `dirs` — no rewritten manifests or
/// `Cargo.lock` — because the sole consumer (the CLI's describe MANIFEST CACHE) hashes
/// source files itself. Returns `None` on the soft-fallback conditions (metadata
/// unavailable / target not a workspace member), in which case the CLI simply does not
/// cache (build-every-time, the prior behavior).
///
/// `pub` (re-exported by `lib.rs`) so the `modal-rust` CLI reuses the facade's ONE
/// closure resolution instead of re-shelling `cargo metadata`.
pub fn describe_cache_inputs(workspace_root: &Path, package: &str) -> Option<Vec<PathBuf>> {
    let manifest = workspace_root.join("Cargo.toml");
    if !manifest.is_file() {
        return None;
    }
    let metadata = run_cargo_metadata(&manifest)?;
    let ClosureResult { dirs, .. } = closure_from_metadata(&metadata, package)?;
    Some(dirs)
}

/// Like [`workspace_closure`] but LENIENT toward out-of-workspace path-deps: it returns
/// the closure even when a normal path-dep escapes the workspace, because the LOCAL
/// `--describe` SHADOW build (the sole caller) CAN resolve such deps against the user's
/// on-disk tree (the shadow rewrites them to absolute paths — see
/// [`crate::runner_gen::materialize_shadow`]). The remote upload, by contrast, cannot
/// carry that source, so [`workspace_closure`] hard-errors there. Returns `None` only on
/// the same soft-fallback conditions (metadata unavailable / target not a member).
pub(crate) fn workspace_closure_lenient(
    workspace_root: &Path,
    package: &str,
) -> Option<ClosureUpload> {
    let manifest = workspace_root.join("Cargo.toml");
    if !manifest.is_file() {
        return None;
    }
    let metadata = run_cargo_metadata(&manifest)?;
    let ClosureResult { dirs, .. } = closure_from_metadata(&metadata, package)?;
    build_closure_upload(&metadata, package, dirs)
}

/// Assemble the [`ClosureUpload`] (rewritten workspace manifest + verbatim `Cargo.lock` +
/// dev-dep-stripped member manifests + injected runner) from already-resolved closure
/// `dirs`. Shared by [`workspace_closure`] and [`workspace_closure_lenient`]. Returns
/// `None` on the soft-fallback conditions (unreadable / un-rewritable workspace
/// manifest), having logged the reason.
fn build_closure_upload(
    metadata: &Metadata,
    package: &str,
    dirs: Vec<PathBuf>,
) -> Option<ClosureUpload> {
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

    // The rewritten workspace manifest is the first inline override.
    let mut inline_files: Vec<(String, Vec<u8>)> =
        vec![("Cargo.toml".to_string(), rewritten.into_bytes())];

    // Strip `[dev-dependencies]` from each uploaded MEMBER manifest. The remote build
    // is `cargo build --bin modal_runner` (never tests/benches), so dev-deps are never
    // needed — and a member's dev-dep that path-points OUTSIDE the uploaded closure
    // (e.g. the facade `modal-rust` dev-deps on `examples/add`, which is NOT in the
    // closure) makes cargo ABORT loading the trimmed workspace with
    // "failed to read .../Cargo.toml: No such file or directory". Emitting a
    // dev-dep-stripped manifest as an inline override (which wins over the verbatim
    // on-disk one) keeps the uploaded workspace self-consistent. Build-only deps stay:
    // they are needed to build the runner.
    for dir in &dirs {
        let member_manifest = dir.join("Cargo.toml");
        let Ok(rel) = dir.strip_prefix(ws_root) else {
            continue; // a closure dir outside the ws root cannot be emitted inline
        };
        if rel.as_os_str().is_empty() {
            continue; // the root manifest is already handled by the workspace rewrite
        }
        let Ok(original) = std::fs::read_to_string(&member_manifest) else {
            continue; // unreadable -> fall back to the verbatim upload (no override)
        };
        match strip_dev_dependencies(&original) {
            Ok(Some(stripped)) => {
                let rel_posix = rel
                    .components()
                    .filter_map(|c| c.as_os_str().to_str())
                    .collect::<Vec<_>>()
                    .join("/");
                inline_files.push((format!("{rel_posix}/Cargo.toml"), stripped.into_bytes()));
            }
            // No dev-deps to strip (`None`) -> keep the verbatim upload.
            Ok(None) => {}
            Err(e) => {
                eprintln!(
                    "[modal-rust] could not strip dev-dependencies from {} ({e}); \
                     uploading it verbatim (a build may fail if it dev-deps an \
                     un-uploaded path)",
                    member_manifest.display()
                );
            }
        }
    }

    // Inject the tooling-generated `modal_runner` bin into the upload copy (design B)
    // unless the target already ships its own (auto-detect) or declares no `modal-rust`
    // facade dep (not generatable). Reuses the metadata already parsed above (no extra
    // `cargo metadata` call). Pushed onto `inline_files` so it rides the source mount at
    // `<remote_src>/<crate_rel>/src/bin/modal_runner.rs`, EXEMPT from ignore, overwrite-
    // precedent (it wins over an on-disk file at the same path — e.g. a crate that DOES
    // ship a bin still has `has_own_runner_bin == true` so we skip and keep its own).
    if let Some(runner) = crate::runner_gen::injected_runner_file_from_metadata(metadata, package) {
        inline_files.push(runner);
    }

    Some(ClosureUpload {
        dirs,
        extra_files,
        inline_files,
    })
}

/// Build the actionable upload-time error for one or more normal path-deps that escape
/// the workspace (so the upload can't carry their source). Names each offending crate +
/// path and tells the user how to fix it (git/version dep, or vendor into the workspace).
///
/// Reached only via [`workspace_closure`] (client) + tests, so light allows it dead.
#[cfg_attr(not(feature = "client"), allow(dead_code))]
fn out_of_workspace_error(package: &str, deps: &[OutOfWorkspaceDep]) -> String {
    use std::fmt::Write as _;
    let mut msg = format!(
        "package '{package}' depends on {} crate(s) by a path OUTSIDE the uploaded \
         workspace, whose source cannot be uploaded to Modal:\n",
        deps.len()
    );
    for dep in deps {
        let _ = writeln!(msg, "  - {} (path = {})", dep.name, dep.path.display());
    }
    msg.push_str(
        "Remote run/deploy uploads only the workspace dependency closure, so an \
         out-of-workspace path-dep would make the in-container `cargo build` fail to \
         read its Cargo.toml. Fix by depending on these crates via a git or version \
         (crates.io) spec, or by vendoring them into this workspace. (Local `--describe` \
         resolves out-of-workspace path-deps against your on-disk tree, so it is \
         unaffected.)",
    );
    msg
}

/// Remove `[dev-dependencies]` (and any `[target.<cfg>.dev-dependencies]`) tables from
/// a member `Cargo.toml`, preserving everything else (normal/build deps, `[lib]`,
/// `[[bin]]`, comments, formatting) byte-for-byte via `toml_edit`.
///
/// Returns `Ok(Some(rewritten))` when at least one dev-dependencies table was removed,
/// `Ok(None)` when the manifest declares none (so the caller keeps the verbatim
/// upload), or `Err` on a parse failure (the caller logs + falls back to verbatim).
///
/// Why: the remote in-body build runs only `cargo build --bin modal_runner`, so
/// dev-dependencies are never compiled — but a member whose dev-dep path-points
/// OUTSIDE the uploaded closure makes cargo fail to LOAD the workspace (it reads every
/// member manifest's dev-deps). Stripping them keeps the trimmed upload loadable.
fn strip_dev_dependencies(original: &str) -> Result<Option<String>, String> {
    use toml_edit::{DocumentMut, Item};

    let mut doc: DocumentMut = original
        .parse()
        .map_err(|e| format!("parse Cargo.toml: {e}"))?;
    let mut removed = doc.remove("dev-dependencies").is_some();

    // `[target.<cfg>.dev-dependencies]` lives under `target.<cfg>`; clear each.
    if let Some(target) = doc.get_mut("target").and_then(Item::as_table_like_mut) {
        // Collect cfg keys first to avoid borrowing `target` mutably while iterating.
        let cfgs: Vec<String> = target.iter().map(|(k, _)| k.to_string()).collect();
        for cfg in cfgs {
            if let Some(cfg_tbl) = target.get_mut(&cfg).and_then(Item::as_table_like_mut) {
                if cfg_tbl.remove("dev-dependencies").is_some() {
                    removed = true;
                }
            }
        }
    }

    Ok(removed.then(|| doc.to_string()))
}

/// Rewrite the workspace manifest's `[workspace] members` and `default-members`
/// arrays to exactly the closure `dirs` (as workspace-root-relative POSIX paths),
/// preserving everything else (`resolver`, `[profile.*]`, comments) byte-for-byte
/// via `toml_edit`. A `[workspace] exclude` array, if present, is cleared (the
/// excluded dirs aren't uploaded, so excluding them is moot and could reference a
/// missing path).
///
/// A STANDALONE crate (an external user's pure-library package with NO `[workspace]`
/// table — its own metadata makes it its own workspace root + sole member) needs no
/// member rewriting: there are no members to scope, and the single-package manifest is
/// already self-consistent. Return it unchanged so the closure path (and the local
/// `--describe` shadow build that depends on it) works for external users, not just the
/// in-workspace examples. In-workspace manifests always carry `[workspace]`, so this
/// branch is never taken for them — zero wire delta.
fn rewrite_workspace_members(
    original: &str,
    workspace_root: &Path,
    dirs: &[PathBuf],
) -> Result<String, String> {
    use toml_edit::{Array, DocumentMut, Item, Value};

    let mut doc: DocumentMut = original
        .parse()
        .map_err(|e| format!("parse Cargo.toml: {e}"))?;

    // Standalone (single-package) manifest: no `[workspace]` to rewrite. Leave it as-is.
    if doc.get("workspace").and_then(Item::as_table_like).is_none() {
        return Ok(original.to_string());
    }

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
///
/// `pub(crate)` so [`crate::runner_gen`] reuses the SAME invocation (auto-detect +
/// lib-name + crate-rel all read from one parsed `Metadata`).
pub(crate) fn run_cargo_metadata(manifest: &Path) -> Option<Metadata> {
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

/// A normal (non-dev, non-build) path-dependency whose target dir is NOT a workspace
/// member — so [`closure_from_metadata`] cannot upload its source. The remote
/// `cargo build` would then fail to read the dep's `Cargo.toml`. Captured so the I/O
/// layer can fail loudly with the offending crate's name + path (see
/// [`OutOfWorkspaceDep`] handling in [`workspace_closure`]).
#[derive(Debug, Clone, PartialEq, Eq)]
struct OutOfWorkspaceDep {
    /// The dependency package name (e.g. `"modal-rust"`).
    name: String,
    /// The absolute, canonicalized dir the path-dep points at (cargo-reported).
    path: PathBuf,
}

/// The result of the pure closure walk: the closure crate dirs PLUS any normal
/// path-deps that escape the workspace (which the upload cannot satisfy).
struct ClosureResult {
    dirs: Vec<PathBuf>,
    /// Read only by the client `workspace_closure` (hard-error path) + tests; the
    /// LIGHT `workspace_closure_lenient` ignores it, so light allows it dead.
    #[cfg_attr(not(feature = "client"), allow(dead_code))]
    out_of_workspace: Vec<OutOfWorkspaceDep>,
}

/// Pure closure algorithm over parsed metadata. Returns the workspace-member normal
/// path-dep closure crate dirs of `package` (plus any out-of-workspace normal path-deps
/// encountered along the way), or `None` if `package` is not a member.
///
/// Split out from I/O so it is unit-testable on fixtures (no cargo invocation).
fn closure_from_metadata(metadata: &Metadata, package: &str) -> Option<ClosureResult> {
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
    // De-duplicate out-of-workspace deps by (name, path) so each is reported once.
    let mut escaped: Vec<OutOfWorkspaceDep> = Vec::new();
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
            if member_dirs.contains(dep_path) {
                if !closure.contains(dep_path) {
                    stack.push(dep_path.clone());
                }
            } else {
                // A normal path-dep that escapes the workspace: the upload can't carry
                // its source, so the remote build would fail. Record it (deduped) for a
                // loud, actionable error at upload time.
                let entry = OutOfWorkspaceDep {
                    name: dep.name.clone(),
                    path: dep_path.clone(),
                };
                if !escaped.contains(&entry) {
                    escaped.push(entry);
                }
            }
        }
    }

    // Deterministic order (the upload sorts by mount path anyway, but stable output
    // keeps logs/tests predictable).
    let mut dirs: Vec<PathBuf> = closure.into_iter().collect();
    dirs.sort();
    escaped.sort_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
    Some(ClosureResult {
        dirs,
        out_of_workspace: escaped,
    })
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
                    targets: vec![],
                },
                Package {
                    id: "macros-id".into(),
                    name: "modal-rust-macros".into(),
                    manifest_path: "/ws/crates/macros/Cargo.toml".into(),
                    dependencies: vec![],
                    targets: vec![],
                },
                Package {
                    id: "sdk-id".into(),
                    name: "modal-rust-sdk".into(),
                    manifest_path: "/ws/crates/sdk/Cargo.toml".into(),
                    dependencies: vec![],
                    targets: vec![],
                },
                Package {
                    id: "addex-id".into(),
                    name: "example-add".into(),
                    manifest_path: "/ws/examples/add/Cargo.toml".into(),
                    dependencies: vec![
                        // normal path-dep on runtime
                        Dependency {
                            name: "modal-rust-runtime".into(),
                            kind: None,
                            path: Some("/ws/crates/runtime".into()),
                        },
                        // a registry dep (no path) — ignored
                        Dependency {
                            name: "serde".into(),
                            kind: None,
                            path: None,
                        },
                    ],
                    targets: vec![],
                },
                Package {
                    id: "facade-id".into(),
                    name: "modal-rust".into(),
                    manifest_path: "/ws/crates/facade/Cargo.toml".into(),
                    dependencies: vec![
                        Dependency {
                            name: "modal-rust-runtime".into(),
                            kind: None,
                            path: Some("/ws/crates/runtime".into()),
                        },
                        Dependency {
                            name: "modal-rust-macros".into(),
                            kind: None,
                            path: Some("/ws/crates/macros".into()),
                        },
                        Dependency {
                            name: "modal-rust-sdk".into(),
                            kind: None,
                            path: Some("/ws/crates/sdk".into()),
                        },
                        // DEV path-dep on example-add — MUST be excluded.
                        Dependency {
                            name: "example-add".into(),
                            kind: Some("dev".into()),
                            path: Some("/ws/examples/add".into()),
                        },
                    ],
                    targets: vec![],
                },
            ],
        }
    }

    #[test]
    fn closure_for_example_add_is_self_plus_runtime() {
        let m = fixture();
        let result = closure_from_metadata(&m, "example-add").unwrap();
        assert_eq!(
            result.dirs,
            vec![
                PathBuf::from("/ws/crates/runtime"),
                PathBuf::from("/ws/examples/add"),
            ],
            "example-add closure = {{examples/add, crates/runtime}} only"
        );
        assert!(
            result.out_of_workspace.is_empty(),
            "all path-deps are workspace members"
        );
    }

    #[test]
    fn closure_for_facade_excludes_dev_dep_on_example() {
        let m = fixture();
        let result = closure_from_metadata(&m, "modal-rust").unwrap();
        // The dev-dep on example-add must NOT appear; normal deps must.
        assert_eq!(
            result.dirs,
            vec![
                PathBuf::from("/ws/crates/facade"),
                PathBuf::from("/ws/crates/macros"),
                PathBuf::from("/ws/crates/runtime"),
                PathBuf::from("/ws/crates/sdk"),
            ],
        );
        assert!(
            !result.dirs.contains(&PathBuf::from("/ws/examples/add")),
            "the dev path-dep on example-add must be excluded"
        );
        assert!(result.out_of_workspace.is_empty());
    }

    #[test]
    fn closure_flags_out_of_workspace_normal_path_dep() {
        // An external standalone crate `myapp` (its own ws root + sole member) that deps
        // the facade `modal-rust` by an out-of-workspace path. The closure is {myapp}
        // only, and the escaping `modal-rust` dep is recorded so the upload fails loudly.
        let m = Metadata {
            workspace_root: PathBuf::from("/tmp/myapp"),
            workspace_members: vec!["myapp-id".into()],
            packages: vec![Package {
                id: "myapp-id".into(),
                name: "myapp".into(),
                manifest_path: "/tmp/myapp/Cargo.toml".into(),
                dependencies: vec![Dependency {
                    name: "modal-rust".into(),
                    kind: None,
                    // cargo canonicalizes the relative `../checkout/...` to absolute.
                    path: Some("/elsewhere/checkout/crates/modal-rust".into()),
                }],
                targets: vec![],
            }],
        };
        let result = closure_from_metadata(&m, "myapp").unwrap();
        assert_eq!(result.dirs, vec![PathBuf::from("/tmp/myapp")]);
        assert_eq!(
            result.out_of_workspace,
            vec![OutOfWorkspaceDep {
                name: "modal-rust".into(),
                path: PathBuf::from("/elsewhere/checkout/crates/modal-rust"),
            }],
            "the escaping facade path-dep is captured for a loud upload error"
        );
        // The actionable error names the crate, its path, and the git/version fix.
        let err = out_of_workspace_error("myapp", &result.out_of_workspace);
        assert!(err.contains("modal-rust"));
        assert!(err.contains("/elsewhere/checkout/crates/modal-rust"));
        assert!(err.contains("git or version"));
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
    fn rewrite_standalone_manifest_is_noop() {
        // An external user's pure-library crate: no `[workspace]` table. The rewrite is
        // a verbatim no-op (nothing to scope) so the closure path works for external
        // crates, not just the in-workspace examples.
        let original = "[package]\nname = \"modal-user-demo\"\nedition = \"2021\"\n\
                        [dependencies]\nmodal-rust = { path = \"/x/modal-rust\" }\n";
        let dirs = vec![PathBuf::from("/tmp/modal-user-demo")];
        let out =
            rewrite_workspace_members(original, &PathBuf::from("/tmp/modal-user-demo"), &dirs)
                .unwrap();
        assert_eq!(
            out, original,
            "standalone manifest is returned byte-identical"
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

    #[test]
    fn strip_dev_dependencies_removes_table_and_keeps_the_rest() {
        // Mirrors the facade `crates/modal-rust/Cargo.toml`: a `[dev-dependencies]`
        // with an OUT-OF-CLOSURE path-dep that would break the trimmed workspace load.
        let original = "\
[package]
name = \"modal-rust\"

[dependencies]
modal-rust-runtime = { path = \"../modal-rust-runtime\" }

[dev-dependencies]
example-add = { path = \"../../examples/add\" }
inventory = \"0.3\"

[build-dependencies]
some-build-dep = \"1\"
";
        let out = strip_dev_dependencies(original)
            .unwrap()
            .expect("dev-deps removed");
        let doc: toml_edit::DocumentMut = out.parse().unwrap();
        // dev-dependencies gone; the offending out-of-closure path is no longer present.
        assert!(doc.get("dev-dependencies").is_none());
        assert!(!out.contains("examples/add"));
        // Normal + build deps preserved (build deps ARE needed to build the runner).
        assert!(doc.get("dependencies").is_some());
        assert!(out.contains("modal-rust-runtime"));
        assert!(out.contains("[build-dependencies]"));
        assert!(out.contains("some-build-dep"));
    }

    #[test]
    fn strip_dev_dependencies_none_when_absent() {
        // A manifest with no dev-deps yields `None` so the caller keeps the verbatim
        // on-disk upload (no inline override emitted).
        let original = "[package]\nname = \"x\"\n\n[dependencies]\nserde = \"1\"\n";
        assert!(strip_dev_dependencies(original).unwrap().is_none());
    }

    #[test]
    fn strip_dev_dependencies_handles_target_cfg_table() {
        // `[target.'cfg(...)'.dev-dependencies]` must also be removed.
        let original = "\
[package]
name = \"x\"

[target.'cfg(unix)'.dev-dependencies]
nix = \"0.27\"
";
        let out = strip_dev_dependencies(original)
            .unwrap()
            .expect("target dev-deps removed");
        assert!(!out.contains("dev-dependencies"));
        assert!(!out.contains("nix"));
    }
}
