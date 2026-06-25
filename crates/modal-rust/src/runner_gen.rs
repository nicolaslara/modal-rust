//! Tooling-generated `modal_runner` — the INJECT-BIN mechanism (design B).
//!
//! Today a runnable crate had to ship its OWN `src/bin/modal_runner.rs`
//! (`modal_rust::modal_runner!(<crate>);`). This module lets the TOOLING own/generate
//! that runner so a user's crate is a PURE LIBRARY (`modal-rust` dep + `#[function]`
//! fns, no runner bin). The mechanism materializes ONE file —
//! `<target-crate-rel>/src/bin/modal_runner.rs` whose body is
//! `<facade-extern>::modal_runner!(<lib_ident>);` — into the UPLOAD/BUILD COPY of the
//! target crate ONLY (never the user's on-disk tree). Cargo's autobins discovers
//! `src/bin/*.rs` as a bin target, so `cargo build -p <pkg> --bin modal_runner`
//! resolves with NO `Cargo.toml` edit. The build PACKAGE never flips: it stays
//! `<target_pkg>` everywhere, so the run-wrapper config env, the deploy
//! `cargo build -p <pkg>` line, and the `--describe` cargo invocation are all
//! BYTE-IDENTICAL to today for crates that already ship a bin.
//!
//! ## Why this works for an EXTERNAL crate
//!
//! The injected file is `<facade-extern>::modal_runner!(<lib_ident>);`. Because the
//! file compiles INSIDE the target crate, `modal-rust` resolves to whatever the
//! target crate's manifest declares (path, git, or version) — identically to the
//! user's own `#[function]` code. We only need to spell the facade with the SAME
//! extern name the target crate gives the `modal-rust` package, which we read from the
//! target's `Cargo.toml` `[dependencies]` (the key, honoring `package = "modal-rust"`).
//! No dependency-spec mirroring, no Cargo.toml synthesis.
//!
//! ## Auto-detect (backward-compatible)
//!
//! If the target package ALREADY ships a target with `kind` containing `"bin"` AND
//! `name == "modal_runner"`, it BRINGS ITS OWN bin: we inject NOTHING (today's path).
//! Otherwise we GENERATE (inject the file).

use std::path::{Path, PathBuf};

use crate::scope;

/// The injected/auto-detected runner-bin name (the build line is `--bin modal_runner`).
pub(crate) const RUNNER_BIN_NAME: &str = "modal_runner";

/// The facade package whose extern name the injected runner spells.
const FACADE_PACKAGE: &str = "modal-rust";

/// What the tooling needs to inject (or skip injecting) a `modal_runner` bin for a
/// target package, resolved purely from `cargo metadata` + the target manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerTarget {
    /// The `-p <pkg>` arg — UNCHANGED across describe/run/deploy.
    pub package: String,
    /// The library crate ident the macro must link (`[lib].name`, else the package
    /// name with `-` → `_`). The injected file is `modal_runner!(<lib_ident>);`.
    pub lib_ident: String,
    /// The extern name the target crate gives the `modal-rust` facade (the
    /// `[dependencies]` key with `-` → `_`, honoring `package = "modal-rust"`). `None`
    /// when the target declares no `modal-rust` dep (e.g. a pure `modal-rust-runtime`
    /// crate) — then the runner CANNOT be generated.
    pub facade_extern: Option<String>,
    /// The crate dir RELATIVE to the workspace root, POSIX-joined
    /// (`"examples/quickstart"`; `""` when the crate IS the workspace root).
    pub crate_rel: String,
    /// Auto-detect: the target already ships a `modal_runner` bin ⇒ skip generation.
    pub has_own_runner_bin: bool,
    /// The names of THIS package's own `bin` targets (e.g. `["add-runner"]`), read from
    /// `cargo metadata`. A SIBLING crate's `modal_runner` bin is NOT in this list — it is
    /// scoped to the resolved package only. Surfaced so the CLI can name the crate's real
    /// bin(s) in the "cannot run this crate" hint instead of cargo's confusing
    /// "available bin in <unrelated-package>" help.
    pub bin_targets: Vec<String>,
}

impl RunnerTarget {
    /// Can the tooling generate a runner for this target? Only when it does NOT bring
    /// its own bin AND we can spell the facade extern name.
    pub fn is_generatable(&self) -> bool {
        !self.has_own_runner_bin && self.facade_extern.is_some()
    }

    /// Is this target usable by `modal-rust run`/`deploy` at all? It is when the CLI can
    /// either GENERATE a runner (an `#[modal_rust::function]`/inventory library) OR the
    /// crate already ships its OWN `modal_runner` bin. When NEITHER holds (a manual-
    /// registry crate that ships a differently-named bin, like `examples/add`'s
    /// `add-runner`), the `cargo build -p <pkg> --bin modal_runner` line is doomed, so the
    /// CLI must short-circuit with a clear hint BEFORE shelling out.
    pub fn is_runnable(&self) -> bool {
        self.is_generatable() || self.has_own_runner_bin
    }
}

// Public-API surface (re-exported by the facade `lib.rs`):
//   - `RunnerTarget` + `is_generatable` (above),
//   - `resolve_runner_target` + `materialize_shadow` — the `modal-rust` CLI's
//     `--describe` shadow build,
//   - `render_runner_main` + `injected_runner_rel_path` — the exact injected bytes +
//     path (used by the wire-delta mock test to assert the +1 MountFile).
// Everything else stays `pub(crate)`.

/// Resolve the runner target for `package` rooted at `workspace_root`: run
/// `cargo metadata` (via [`scope`]) to find the package's lib name, its crate dir
/// (for `crate_rel`), and whether it already ships a `modal_runner` bin; then parse
/// the target `Cargo.toml` for the facade extern name. Returns `None` only when
/// metadata is unavailable or the package is not a workspace member.
pub fn resolve_runner_target(workspace_root: &Path, package: &str) -> Option<RunnerTarget> {
    let manifest = workspace_root.join("Cargo.toml");
    if !manifest.is_file() {
        return None;
    }
    let metadata = scope::run_cargo_metadata(&manifest)?;
    resolve_from_metadata(&metadata, package)
}

/// Pure resolution over already-parsed metadata (no cargo invocation) — unit-testable.
/// Reads the target manifest from disk ONLY for the facade-extern name.
fn resolve_from_metadata(metadata: &scope::Metadata, package: &str) -> Option<RunnerTarget> {
    let member_ids: std::collections::HashSet<&str> = metadata
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect();
    let pkg = metadata
        .packages
        .iter()
        .find(|p| p.name == package && member_ids.contains(p.id.as_str()))?;

    let lib_ident = lib_ident_of(pkg);
    let has_own_runner_bin = has_modal_runner_bin(pkg);
    let bin_targets = bin_target_names(pkg);
    let crate_rel = crate_rel_of(&metadata.workspace_root, pkg);

    // The facade-extern name is read from the target's OWN Cargo.toml; a parse failure
    // or absent dep yields `None` (NOT generatable — must bring its own runner).
    let facade_extern = std::fs::read_to_string(&pkg.manifest_path)
        .ok()
        .and_then(|s| facade_extern_name(&s));

    Some(RunnerTarget {
        package: package.to_string(),
        lib_ident,
        facade_extern,
        crate_rel,
        has_own_runner_bin,
        bin_targets,
    })
}

/// The library ident the runner links: the `[lib].name` target if present, else the
/// package name with `-` → `_` (cargo's default lib name).
fn lib_ident_of(pkg: &scope::Package) -> String {
    for t in &pkg.targets {
        if t.kind.iter().any(|k| k == "lib") {
            return t.name.clone();
        }
    }
    pkg.name.replace('-', "_")
}

/// Does this package already ship a target with `kind` CONTAINING `"bin"` AND
/// `name == "modal_runner"`? (Auto-detect: USE it verbatim, inject nothing.)
fn has_modal_runner_bin(pkg: &scope::Package) -> bool {
    pkg.targets
        .iter()
        .any(|t| t.name == RUNNER_BIN_NAME && t.kind.iter().any(|k| k == "bin"))
}

/// The names of THIS package's own `bin` targets (`kind` CONTAINS `"bin"`). Used to name
/// the crate's real bin(s) in the "cannot run this crate" hint — a sibling crate's bin is
/// never included (this reads only `pkg.targets`).
fn bin_target_names(pkg: &scope::Package) -> Vec<String> {
    pkg.targets
        .iter()
        .filter(|t| t.kind.iter().any(|k| k == "bin"))
        .map(|t| t.name.clone())
        .collect()
}

/// The crate dir RELATIVE to the workspace root (POSIX), `""` when the crate IS the
/// root. Derived from `manifest_path` (drop the trailing `Cargo.toml`).
fn crate_rel_of(workspace_root: &Path, pkg: &scope::Package) -> String {
    let crate_dir = pkg.manifest_path.parent().unwrap_or(workspace_root);
    match crate_dir.strip_prefix(workspace_root) {
        Ok(rel) => rel
            .components()
            .filter_map(|c| c.as_os_str().to_str())
            .collect::<Vec<_>>()
            .join("/"),
        // Crate dir outside the workspace root (an unusual path-dep layout): fall back
        // to the crate dir's file name so the injected file still has a stable path.
        Err(_) => crate_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string(),
    }
}

/// Find the extern name the target crate gives the `modal-rust` facade. Scans
/// `[dependencies]` for the entry whose EFFECTIVE package is `modal-rust`: the key
/// `modal-rust` itself, OR any key with `package = "modal-rust"`. The extern name cargo
/// uses is the KEY with `-` → `_` (`modal-rust` → `modal_rust`; a `modal_rust_facade`
/// alias stays `modal_rust_facade`). Returns `None` when no such dep exists.
fn facade_extern_name(manifest: &str) -> Option<String> {
    use toml_edit::{DocumentMut, Item, Value};

    let doc: DocumentMut = manifest.parse().ok()?;
    let deps = doc.get("dependencies").and_then(Item::as_table_like)?;

    for (key, item) in deps.iter() {
        // Effective package name: a `package = "..."` rename overrides the key.
        let effective = match item {
            // `dep = { package = "modal-rust", .. }`
            Item::Value(Value::InlineTable(t)) => t
                .get("package")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| key.to_string()),
            // `[dependencies.dep]` with `package = "..."`
            Item::Table(t) => t
                .get("package")
                .and_then(|p| p.as_value())
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| key.to_string()),
            // `dep = "1.0"` — a bare version string: package == key.
            _ => key.to_string(),
        };
        if effective == FACADE_PACKAGE {
            return Some(key.replace('-', "_"));
        }
    }
    None
}

/// The whole body of the generated `src/bin/modal_runner.rs`: one line,
/// `<facade-extern>::modal_runner!(<lib_ident>);`. The macro (resolved from the
/// crate the file lives in) emits `use <lib> as _;` to force-link the lib's inventory,
/// then the frozen runner `main`. A panic is impossible: callers guard on
/// [`RunnerTarget::is_generatable`], so `facade_extern` is `Some`.
pub fn render_runner_main(t: &RunnerTarget) -> String {
    let facade = t
        .facade_extern
        .as_deref()
        .expect("render_runner_main on a non-generatable target (facade_extern is None)");
    format!(
        "// AUTO-GENERATED by modal-rust: the tooling injects this runner bin into the\n\
         // build/upload copy of the crate so `cargo build -p {pkg} --bin {bin}` resolves\n\
         // with NO Cargo.toml edit. Do not commit; your crate stays a pure library.\n\
         {facade}::modal_runner!({lib});\n",
        pkg = t.package,
        bin = RUNNER_BIN_NAME,
        facade = facade,
        lib = t.lib_ident,
    )
}

/// The workspace-relative POSIX path the injected runner lands at:
/// `<crate_rel>/src/bin/modal_runner.rs` (or `src/bin/modal_runner.rs` when the crate
/// IS the workspace root). This is the `(rel, bytes)` key for the source upload's
/// `extra_inline_files` and the in-container build-context path.
pub fn injected_runner_rel_path(t: &RunnerTarget) -> String {
    if t.crate_rel.is_empty() {
        format!("src/bin/{RUNNER_BIN_NAME}.rs")
    } else {
        format!("{}/src/bin/{RUNNER_BIN_NAME}.rs", t.crate_rel)
    }
}

/// Compute the injected runner file `(workspace-relative POSIX path, bytes)` for the
/// source upload when generation applies, or `None` (auto-detect hit, no facade dep,
/// or unresolvable metadata) so the caller injects nothing. Runs `cargo metadata`
/// itself — used by the FALLBACK source-mount arm (which has no parsed metadata).
///
/// Called only from the client `control_plane` fallback arm, so light allows it dead.
#[cfg_attr(not(feature = "client"), allow(dead_code))]
pub(crate) fn injected_runner_file(
    workspace_root: &Path,
    package: &str,
) -> Option<(String, Vec<u8>)> {
    let target = resolve_runner_target(workspace_root, package)?;
    runner_file_for(&target)
}

/// Like [`injected_runner_file`] but over ALREADY-parsed metadata (no extra cargo
/// call) — used by the SCOPED source-mount arm, which already ran `cargo metadata`.
pub(crate) fn injected_runner_file_from_metadata(
    metadata: &scope::Metadata,
    package: &str,
) -> Option<(String, Vec<u8>)> {
    let target = resolve_from_metadata(metadata, package)?;
    runner_file_for(&target)
}

/// The injected `(rel, bytes)` for a resolved target, or `None` when not generatable.
fn runner_file_for(target: &RunnerTarget) -> Option<(String, Vec<u8>)> {
    if !target.is_generatable() {
        return None;
    }
    Some((
        injected_runner_rel_path(target),
        render_runner_main(target).into_bytes(),
    ))
}

/// Materialize a gitignored SHADOW copy of the target crate's cargo dependency closure
/// under `dest`, with the generated `src/bin/modal_runner.rs` written into the shadow
/// target crate, and return the shadow workspace root. The CLI `--describe` path
/// builds + runs the runner here so it NEVER mutates the user's on-disk tree, while
/// resolving `modal-rust` identically to the real upload (the SAME closure dirs +
/// rewritten workspace manifest + verbatim `Cargo.lock`; git/registry deps are fetched
/// by cargo at build time).
///
/// Layout under `dest`: each closure crate at its `crate_rel`, the rewritten workspace
/// `Cargo.toml` + dev-dep-stripped member manifests (from [`scope::workspace_closure`])
/// at their relative paths, the verbatim `Cargo.lock`, and the injected runner.
/// Returns the shadow root (`= dest`) on success.
pub fn materialize_shadow(
    workspace_root: &Path,
    target: &RunnerTarget,
    dest: &Path,
) -> std::io::Result<PathBuf> {
    // LENIENT closure: unlike the remote upload, the local shadow build CAN resolve a
    // path-dep that escapes the workspace (e.g. an external standalone crate's
    // `modal-rust = { path = "../checkout/..." }`) by rewriting it to an absolute path
    // anchored at the user's on-disk tree (step 5 below). So we must NOT hard-error on
    // out-of-workspace deps here.
    let closure =
        scope::workspace_closure_lenient(workspace_root, &target.package).ok_or_else(|| {
            std::io::Error::other(format!(
                "cargo metadata unavailable for shadow build of package '{}'",
                target.package
            ))
        })?;

    std::fs::create_dir_all(dest)?;

    // The shadow dirs that are present IN the shadow (so an in-closure relative path-dep
    // resolves locally and must NOT be rewritten). Real on-disk dirs, canonicalized so
    // the path-dep comparison (also canonicalized) is exact.
    let shadow_closure_dirs: std::collections::HashSet<PathBuf> = closure
        .dirs
        .iter()
        .map(|d| std::fs::canonicalize(d).unwrap_or_else(|_| d.clone()))
        .collect();

    // 1. Copy each closure crate dir under its workspace-relative path (POSIX → native).
    for dir in &closure.dirs {
        let Ok(rel) = dir.strip_prefix(workspace_root) else {
            continue; // a closure dir outside the ws root cannot be placed relatively
        };
        let shadow_dir = if rel.as_os_str().is_empty() {
            dest.to_path_buf()
        } else {
            dest.join(rel)
        };
        copy_dir_recursive(dir, &shadow_dir)?;
    }

    // 2. Verbatim extra files (the workspace Cargo.lock).
    for path in &closure.extra_files {
        let Ok(rel) = path.strip_prefix(workspace_root) else {
            continue;
        };
        let shadow_path = dest.join(rel);
        if let Some(parent) = shadow_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(path, &shadow_path)?;
    }

    // 3. Inline overrides (rewritten workspace Cargo.toml + dev-dep-stripped member
    //    manifests) — these WIN over any same-path verbatim file copied above.
    for (rel_posix, bytes) in &closure.inline_files {
        write_inline(dest, rel_posix, bytes)?;
    }

    // 4. The generated runner bin into the shadow target crate.
    let runner_rel = injected_runner_rel_path(target);
    write_inline(dest, &runner_rel, render_runner_main(target).as_bytes())?;

    // 5. Rewrite every shadow manifest's OUT-OF-CLOSURE relative path-deps to absolute
    //    paths anchored at the REAL on-disk crate dir. Without this, cargo resolves a
    //    relative `../checkout/...` path-dep against the TEMP shadow dir (where it does
    //    not exist) and the build fails. In-closure path-deps (present in the shadow at
    //    the same relative layout) are left untouched so they resolve within the shadow.
    for dir in &closure.dirs {
        let Ok(rel) = dir.strip_prefix(workspace_root) else {
            continue;
        };
        let shadow_manifest = if rel.as_os_str().is_empty() {
            dest.join("Cargo.toml")
        } else {
            dest.join(rel).join("Cargo.toml")
        };
        rewrite_shadow_manifest_path_deps(&shadow_manifest, dir, &shadow_closure_dirs)?;
    }

    Ok(dest.to_path_buf())
}

/// Rewrite, in the shadow manifest at `shadow_manifest`, every relative path-dependency
/// that resolves OUTSIDE the shadow closure to an absolute path anchored at the REAL
/// on-disk crate dir (`real_crate_dir`). In-closure path-deps and deps already absolute
/// are left as-is. A no-op when the manifest has no relative out-of-closure path-deps,
/// or when it is missing/unparseable (best-effort; cargo then reports the real error).
fn rewrite_shadow_manifest_path_deps(
    shadow_manifest: &Path,
    real_crate_dir: &Path,
    shadow_closure_dirs: &std::collections::HashSet<PathBuf>,
) -> std::io::Result<()> {
    let Ok(original) = std::fs::read_to_string(shadow_manifest) else {
        return Ok(()); // no manifest at this dir (shouldn't happen) — let cargo report it
    };
    // Canonicalize the real crate dir FIRST so any symlink in it (e.g. macOS
    // `/tmp` -> `/private/tmp`) is resolved before `..` segments in a relative dep are
    // applied — otherwise the rewritten absolute path can dangle.
    let base =
        std::fs::canonicalize(real_crate_dir).unwrap_or_else(|_| real_crate_dir.to_path_buf());
    if let Some(rewritten) = rewrite_path_deps_to_absolute(&original, &base, shadow_closure_dirs) {
        std::fs::write(shadow_manifest, rewritten)?;
    }
    Ok(())
}

/// Pure manifest rewrite: in `[dependencies]`, `[build-dependencies]`,
/// `[dev-dependencies]`, and their `[target.<cfg>.*]` variants, rewrite each path-dep
/// whose `path` is RELATIVE and resolves (against `real_crate_dir`) to a dir NOT in
/// `shadow_closure_dirs` into an ABSOLUTE path (canonicalized when possible). Returns
/// `Some(rewritten)` if anything changed, else `None`. Format-preserving via `toml_edit`.
///
/// Anchoring relative deps at the real crate dir is what lets the shadow resolve an
/// external standalone crate's `modal-rust = { path = "../checkout/..." }`: the temp
/// shadow has no such sibling, but the absolute path always points at the real source.
fn rewrite_path_deps_to_absolute(
    manifest: &str,
    real_crate_dir: &Path,
    shadow_closure_dirs: &std::collections::HashSet<PathBuf>,
) -> Option<String> {
    use toml_edit::{DocumentMut, Item};

    let mut doc: DocumentMut = manifest.parse().ok()?;
    let mut changed = false;

    // The four flavours of top-level dependency tables.
    for section in ["dependencies", "build-dependencies", "dev-dependencies"] {
        if let Some(tbl) = doc.get_mut(section).and_then(Item::as_table_like_mut) {
            changed |= rewrite_dep_table(tbl, real_crate_dir, shadow_closure_dirs);
        }
    }
    // `[target.<cfg>.<dependencies|build-dependencies|dev-dependencies>]`.
    if let Some(target) = doc.get_mut("target").and_then(Item::as_table_like_mut) {
        let cfgs: Vec<String> = target.iter().map(|(k, _)| k.to_string()).collect();
        for cfg in cfgs {
            let Some(cfg_tbl) = target.get_mut(&cfg).and_then(Item::as_table_like_mut) else {
                continue;
            };
            for section in ["dependencies", "build-dependencies", "dev-dependencies"] {
                if let Some(tbl) = cfg_tbl.get_mut(section).and_then(Item::as_table_like_mut) {
                    changed |= rewrite_dep_table(tbl, real_crate_dir, shadow_closure_dirs);
                }
            }
        }
    }

    changed.then(|| doc.to_string())
}

/// Rewrite, IN PLACE, every relative out-of-closure path-dep in one dependency table to
/// an absolute path anchored at `real_crate_dir`. Returns `true` if anything changed.
fn rewrite_dep_table(
    tbl: &mut dyn toml_edit::TableLike,
    real_crate_dir: &Path,
    shadow_closure_dirs: &std::collections::HashSet<PathBuf>,
) -> bool {
    use toml_edit::{Item, Value};

    let mut changed = false;
    let dep_keys: Vec<String> = tbl.iter().map(|(k, _)| k.to_string()).collect();
    for key in dep_keys {
        let Some(item) = tbl.get_mut(&key) else {
            continue;
        };
        // Find the `path` value (inline `{ path = ".." }` or `[dep]` sub-table).
        let path_str = match item {
            Item::Value(Value::InlineTable(t)) => {
                t.get("path").and_then(Value::as_str).map(str::to_string)
            }
            Item::Table(t) => t
                .get("path")
                .and_then(Item::as_value)
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => None,
        };
        let Some(path_str) = path_str else {
            continue; // not a path-dep (version/git) — leave it.
        };
        // Already absolute → resolves anywhere; leave it.
        if Path::new(&path_str).is_absolute() {
            continue;
        }
        // Resolve against the (already-canonicalized) real crate dir; canonicalize the
        // join so the in-closure membership test matches the canonicalized
        // shadow_closure_dirs. If the target does not exist on disk, fall back to a
        // LEXICAL normalization (collapsing `.`/`..`) so the emitted absolute path is
        // still clean — never a `..`-laden join that can dangle past a symlink.
        let joined = real_crate_dir.join(&path_str);
        let resolved =
            std::fs::canonicalize(&joined).unwrap_or_else(|_| lexically_normalize(&joined));
        if shadow_closure_dirs.contains(&resolved) {
            continue; // in-closure: present in the shadow, resolves relatively.
        }
        // Out-of-closure relative path-dep → rewrite to the absolute real path.
        let abs = resolved.to_string_lossy().to_string();
        match item {
            Item::Value(Value::InlineTable(t)) => {
                t.insert("path", abs.into());
                changed = true;
            }
            Item::Table(t) => {
                t.insert("path", toml_edit::value(abs));
                changed = true;
            }
            _ => {}
        }
    }
    changed
}

/// Collapse `.` and `..` components lexically (no filesystem access). A pure fallback
/// for [`rewrite_dep_table`] when a path-dep target does not exist on disk (so
/// `canonicalize` fails) — keeps the emitted absolute path clean. Leading `..` past the
/// root are dropped (cannot ascend above root).
fn lexically_normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    // Nothing to pop (at/above a prefix/root) — keep `..` only if the
                    // accumulated path is purely relative; an absolute base never is.
                    if !out.has_root() {
                        out.push("..");
                    }
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Write `bytes` to `<root>/<rel_posix>` (POSIX path → native join), creating parents.
fn write_inline(root: &Path, rel_posix: &str, bytes: &[u8]) -> std::io::Result<()> {
    let rel_posix = rel_posix.trim_start_matches('/');
    let mut path = root.to_path_buf();
    for comp in rel_posix.split('/') {
        path.push(comp);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)
}

/// Recursively copy `src` → `dst`, pruning build artifacts (`target/`, `.git/`) so the
/// shadow stays small. Mirrors the upload's default ignore floor closely enough for a
/// local build (the shadow is only ever fed to cargo, never uploaded).
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if name_str == "target" || name_str == ".git" {
                continue; // never copy build artifacts / VCS into the shadow
            }
            copy_dir_recursive(&entry.path(), &dst.join(&name))?;
        } else if file_type.is_file() {
            std::fs::copy(entry.path(), dst.join(&name))?;
        }
        // Symlinks and other entries are skipped (source trees rarely need them).
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::{Metadata, Package, Target};

    fn pkg(name: &str, manifest: &str, targets: Vec<Target>) -> Package {
        Package {
            id: format!("{name}-id"),
            name: name.to_string(),
            manifest_path: manifest.into(),
            dependencies: vec![],
            targets,
        }
    }

    fn lib_target(name: &str) -> Target {
        Target {
            kind: vec!["lib".to_string()],
            name: name.to_string(),
            src_path: PathBuf::new(),
        }
    }

    fn bin_target(name: &str) -> Target {
        Target {
            kind: vec!["bin".to_string()],
            name: name.to_string(),
            src_path: PathBuf::new(),
        }
    }

    #[test]
    fn facade_extern_default_key() {
        // The default `modal-rust = { path = ... }` → extern `modal_rust`.
        let m = "[package]\nname = \"q\"\n[dependencies]\nmodal-rust = { path = \"../x\" }\n";
        assert_eq!(facade_extern_name(m).as_deref(), Some("modal_rust"));
    }

    #[test]
    fn facade_extern_bare_version_string() {
        // `modal-rust = "0.1"` (registry version) → extern `modal_rust`.
        let m = "[package]\nname = \"q\"\n[dependencies]\nmodal-rust = \"0.1\"\n";
        assert_eq!(facade_extern_name(m).as_deref(), Some("modal_rust"));
    }

    #[test]
    fn facade_extern_honors_package_rename_inline() {
        // `modal_rust_facade = { package = "modal-rust" }` → extern `modal_rust_facade`.
        let m = "[package]\nname = \"q\"\n[dependencies]\n\
                 modal_rust_facade = { package = \"modal-rust\", path = \"../x\" }\n";
        assert_eq!(facade_extern_name(m).as_deref(), Some("modal_rust_facade"));
    }

    #[test]
    fn facade_extern_honors_package_rename_dotted_table() {
        // `[dependencies.alias]` with `package = "modal-rust"`.
        let m = "[package]\nname = \"q\"\n[dependencies.my_modal]\n\
                 package = \"modal-rust\"\ngit = \"https://example/x\"\n";
        assert_eq!(facade_extern_name(m).as_deref(), Some("my_modal"));
    }

    #[test]
    fn facade_extern_none_for_runtime_only_crate() {
        // A pure `modal-rust-runtime` crate (the `examples/add` case) → None.
        let m = "[package]\nname = \"q\"\n[dependencies]\n\
                 modal-rust-runtime = { path = \"../rt\" }\nserde = \"1\"\n";
        assert_eq!(facade_extern_name(m), None);
    }

    #[test]
    fn facade_extern_none_when_no_deps_table() {
        assert_eq!(facade_extern_name("[package]\nname = \"q\"\n"), None);
    }

    #[test]
    fn lib_ident_prefers_lib_name_then_falls_back() {
        let with_lib = pkg(
            "example-own",
            "/ws/examples/own/Cargo.toml",
            vec![lib_target("example_own_lib")],
        );
        assert_eq!(lib_ident_of(&with_lib), "example_own_lib");
        // No [lib] target → package name with '-' -> '_'.
        let no_lib = pkg("my-crate", "/ws/my-crate/Cargo.toml", vec![]);
        assert_eq!(lib_ident_of(&no_lib), "my_crate");
    }

    #[test]
    fn auto_detect_finds_modal_runner_bin() {
        let owns = pkg(
            "own",
            "/ws/own/Cargo.toml",
            vec![lib_target("own"), bin_target("modal_runner")],
        );
        assert!(has_modal_runner_bin(&owns));
        // A differently-named bin does NOT count.
        let other = pkg(
            "other",
            "/ws/other/Cargo.toml",
            vec![lib_target("other"), bin_target("add-runner")],
        );
        assert!(!has_modal_runner_bin(&other));
        // No bin at all → false.
        let lib_only = pkg(
            "libonly",
            "/ws/libonly/Cargo.toml",
            vec![lib_target("libonly")],
        );
        assert!(!has_modal_runner_bin(&lib_only));
    }

    #[test]
    fn crate_rel_is_workspace_relative_posix() {
        let p = pkg("q", "/ws/examples/quickstart/Cargo.toml", vec![]);
        assert_eq!(crate_rel_of(Path::new("/ws"), &p), "examples/quickstart");
        // Crate IS the workspace root → empty rel.
        let root = pkg("r", "/ws/Cargo.toml", vec![]);
        assert_eq!(crate_rel_of(Path::new("/ws"), &root), "");
    }

    #[test]
    fn render_and_path_for_generatable_target() {
        let t = RunnerTarget {
            package: "quickstart".to_string(),
            lib_ident: "quickstart".to_string(),
            facade_extern: Some("modal_rust".to_string()),
            crate_rel: "examples/quickstart".to_string(),
            has_own_runner_bin: false,
            bin_targets: vec![],
        };
        assert!(t.is_generatable());
        let body = render_runner_main(&t);
        assert!(
            body.contains("modal_rust::modal_runner!(quickstart);"),
            "body spells the facade extern + lib ident: {body}"
        );
        assert_eq!(
            injected_runner_rel_path(&t),
            "examples/quickstart/src/bin/modal_runner.rs"
        );
    }

    #[test]
    fn render_honors_facade_rename() {
        let t = RunnerTarget {
            package: "add-macro".to_string(),
            lib_ident: "example_add_macro".to_string(),
            facade_extern: Some("modal_rust_facade".to_string()),
            crate_rel: "examples/add-macro".to_string(),
            has_own_runner_bin: false,
            bin_targets: vec![],
        };
        assert!(
            render_runner_main(&t).contains("modal_rust_facade::modal_runner!(example_add_macro);")
        );
    }

    #[test]
    fn injected_path_for_root_crate() {
        let t = RunnerTarget {
            package: "standalone".to_string(),
            lib_ident: "standalone".to_string(),
            facade_extern: Some("modal_rust".to_string()),
            crate_rel: String::new(),
            has_own_runner_bin: false,
            bin_targets: vec![],
        };
        assert_eq!(injected_runner_rel_path(&t), "src/bin/modal_runner.rs");
    }

    #[test]
    fn is_generatable_false_when_own_bin_or_no_facade() {
        // Own bin → skip (auto-detect).
        let own_bin = RunnerTarget {
            package: "own".to_string(),
            lib_ident: "own".to_string(),
            facade_extern: Some("modal_rust".to_string()),
            crate_rel: "examples/own".to_string(),
            has_own_runner_bin: true,
            bin_targets: vec!["modal_runner".to_string()],
        };
        assert!(!own_bin.is_generatable());
        // No facade dep → not generatable (must bring its own runner, the add case).
        let no_facade = RunnerTarget {
            package: "add".to_string(),
            lib_ident: "example_add".to_string(),
            facade_extern: None,
            crate_rel: "examples/add".to_string(),
            has_own_runner_bin: false,
            bin_targets: vec!["add-runner".to_string()],
        };
        assert!(!no_facade.is_generatable());
    }

    #[test]
    fn is_runnable_matrix() {
        // Generatable library (inventory + facade dep) → runnable (CLI synthesizes a runner).
        let generatable = RunnerTarget {
            package: "quickstart".to_string(),
            lib_ident: "quickstart".to_string(),
            facade_extern: Some("modal_rust".to_string()),
            crate_rel: "examples/quickstart".to_string(),
            has_own_runner_bin: false,
            bin_targets: vec![],
        };
        assert!(generatable.is_runnable());

        // Ships its own `modal_runner` bin → runnable (today's own-bin path).
        let own_bin = RunnerTarget {
            package: "own-runner-bin".to_string(),
            lib_ident: "own_runner_bin".to_string(),
            facade_extern: Some("modal_rust".to_string()),
            crate_rel: "examples/own-runner-bin".to_string(),
            has_own_runner_bin: true,
            bin_targets: vec!["modal_runner".to_string()],
        };
        assert!(own_bin.is_runnable());

        // Manual-registry crate: no facade dep (not generatable) AND its only bin is
        // `add-runner` (not `modal_runner`) → NOT runnable (the examples/add bug).
        let manual = RunnerTarget {
            package: "example-add".to_string(),
            lib_ident: "example_add".to_string(),
            facade_extern: None,
            crate_rel: "examples/add".to_string(),
            has_own_runner_bin: false,
            bin_targets: vec!["add-runner".to_string()],
        };
        assert!(!manual.is_runnable());
    }

    #[test]
    fn resolve_from_metadata_builds_target_for_member() {
        // A member whose manifest exists on disk (a temp file) so the facade-extern is
        // read; lib name + crate_rel come from the parsed metadata.
        let dir = std::env::temp_dir().join(format!("mr-runnergen-{}", std::process::id()));
        let crate_dir = dir.join("examples/quickstart");
        std::fs::create_dir_all(&crate_dir).unwrap();
        let manifest = crate_dir.join("Cargo.toml");
        std::fs::write(
            &manifest,
            "[package]\nname = \"quickstart\"\n[dependencies]\nmodal-rust = { path = \"../../x\" }\n",
        )
        .unwrap();

        let metadata = Metadata {
            workspace_root: dir.clone(),
            workspace_members: vec!["quickstart-id".to_string()],
            packages: vec![Package {
                id: "quickstart-id".to_string(),
                name: "quickstart".to_string(),
                manifest_path: manifest.clone(),
                dependencies: vec![],
                targets: vec![lib_target("quickstart")],
            }],
        };

        let t = resolve_from_metadata(&metadata, "quickstart").expect("resolved");
        assert_eq!(t.package, "quickstart");
        assert_eq!(t.lib_ident, "quickstart");
        assert_eq!(t.facade_extern.as_deref(), Some("modal_rust"));
        assert_eq!(t.crate_rel, "examples/quickstart");
        assert!(!t.has_own_runner_bin);
        assert!(t.bin_targets.is_empty(), "pure library has no bin targets");
        assert!(t.is_generatable());
        assert!(t.is_runnable());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_from_metadata_none_for_non_member() {
        let metadata = Metadata {
            workspace_root: PathBuf::from("/ws"),
            workspace_members: vec![],
            packages: vec![],
        };
        assert!(resolve_from_metadata(&metadata, "nope").is_none());
    }

    #[test]
    fn bin_target_names_lists_only_this_package_bins() {
        // A manual-registry crate like examples/add: a lib + a differently-named bin.
        let p = pkg(
            "example-add",
            "/ws/examples/add/Cargo.toml",
            vec![lib_target("example_add"), bin_target("add-runner")],
        );
        assert_eq!(bin_target_names(&p), vec!["add-runner".to_string()]);
        // Lib-only crate → no bins.
        let lib_only = pkg("q", "/ws/q/Cargo.toml", vec![lib_target("q")]);
        assert!(bin_target_names(&lib_only).is_empty());
    }

    #[test]
    fn resolve_from_metadata_manual_crate_is_not_runnable() {
        // Mirrors the examples/add bug: a MANUAL-registry crate (no `modal-rust` facade
        // dep, so NOT generatable) whose only bin is `add-runner` (NOT `modal_runner`).
        // `modal-rust run` must short-circuit, and the hint names the REAL bin.
        let dir = std::env::temp_dir().join(format!("mr-runnergen-manual-{}", std::process::id()));
        let crate_dir = dir.join("examples/add");
        std::fs::create_dir_all(&crate_dir).unwrap();
        let manifest = crate_dir.join("Cargo.toml");
        std::fs::write(
            &manifest,
            "[package]\nname = \"example-add\"\n[dependencies]\n\
             modal-rust-runtime = { path = \"../../crates/modal-rust-runtime\" }\n",
        )
        .unwrap();

        let metadata = Metadata {
            workspace_root: dir.clone(),
            workspace_members: vec!["example-add-id".to_string()],
            packages: vec![Package {
                id: "example-add-id".to_string(),
                name: "example-add".to_string(),
                manifest_path: manifest.clone(),
                dependencies: vec![],
                targets: vec![lib_target("example_add"), bin_target("add-runner")],
            }],
        };

        let t = resolve_from_metadata(&metadata, "example-add").expect("resolved");
        assert_eq!(t.facade_extern, None, "no `modal-rust` facade dep");
        assert!(
            !t.has_own_runner_bin,
            "ships `add-runner`, not `modal_runner`"
        );
        assert!(!t.is_generatable());
        assert!(
            !t.is_runnable(),
            "neither generatable nor a modal_runner bin → not runnable"
        );
        assert_eq!(
            t.bin_targets,
            vec!["add-runner".to_string()],
            "hint should name the crate's REAL bin, not an unrelated package's"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rewrite_out_of_closure_relative_path_dep_to_absolute() {
        // The headline external case: a standalone crate `myapp` deps `modal-rust` by a
        // RELATIVE out-of-workspace path. In the shadow, that relative path resolves to a
        // sibling that does not exist → cargo fails. The rewrite makes it absolute,
        // anchored at the REAL on-disk crate dir, so it resolves regardless of where the
        // manifest lives. Use a real temp tree so canonicalize() works.
        let tmp = std::env::temp_dir().join(format!("mr-rewrite-{}", std::process::id()));
        let app_dir = tmp.join("myapp");
        let facade_dir = tmp.join("checkout/crates/modal-rust");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::create_dir_all(&facade_dir).unwrap();

        let manifest = "[package]\nname = \"myapp\"\n\n[dependencies]\n\
                        modal-rust = { path = \"../checkout/crates/modal-rust\" }\n\
                        serde = \"1\"\n";
        // The shadow closure is {myapp} only — the facade is OUT of closure.
        let mut closure = std::collections::HashSet::new();
        closure.insert(std::fs::canonicalize(&app_dir).unwrap());

        let out = rewrite_path_deps_to_absolute(manifest, &app_dir, &closure)
            .expect("the out-of-closure path-dep was rewritten");
        let facade_abs = std::fs::canonicalize(&facade_dir).unwrap();
        assert!(
            out.contains(&facade_abs.to_string_lossy().to_string()),
            "modal-rust path is now the absolute real path: {out}"
        );
        // The relative spec is gone; serde (a version dep) is untouched.
        assert!(
            !out.contains("../checkout"),
            "relative spec replaced: {out}"
        );
        assert!(
            out.contains("serde = \"1\""),
            "version dep untouched: {out}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn rewrite_leaves_in_closure_path_dep_relative() {
        // An IN-closure path-dep (present in the shadow at the same relative layout) must
        // stay relative so it resolves WITHIN the shadow. Mirrors an in-workspace example
        // depending on a sibling crate that IS uploaded.
        let tmp = std::env::temp_dir().join(format!("mr-rewrite-in-{}", std::process::id()));
        let app_dir = tmp.join("examples/quickstart");
        let dep_dir = tmp.join("crates/modal-rust");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::create_dir_all(&dep_dir).unwrap();

        let manifest = "[package]\nname = \"quickstart\"\n\n[dependencies]\n\
                        modal-rust = { path = \"../../crates/modal-rust\" }\n";
        // Both dirs are IN the closure (uploaded), so the dep resolves in the shadow.
        let mut closure = std::collections::HashSet::new();
        closure.insert(std::fs::canonicalize(&app_dir).unwrap());
        closure.insert(std::fs::canonicalize(&dep_dir).unwrap());

        assert!(
            rewrite_path_deps_to_absolute(manifest, &app_dir, &closure).is_none(),
            "an in-closure path-dep is left untouched (no rewrite => None)"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn rewrite_leaves_absolute_path_dep_and_no_path_deps_alone() {
        // An already-absolute path-dep and a pure version dep both yield None (no change).
        let tmp = std::env::temp_dir().join(format!("mr-rewrite-abs-{}", std::process::id()));
        let app_dir = tmp.join("app");
        std::fs::create_dir_all(&app_dir).unwrap();
        let abs = std::fs::canonicalize(&app_dir).unwrap();
        let manifest = format!(
            "[package]\nname = \"x\"\n\n[dependencies]\n\
             modal-rust = {{ path = {abs:?} }}\nserde = \"1\"\n",
            abs = abs.display().to_string()
        );
        let closure = std::collections::HashSet::new();
        assert!(
            rewrite_path_deps_to_absolute(&manifest, &app_dir, &closure).is_none(),
            "absolute path-dep + version dep => no change"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
