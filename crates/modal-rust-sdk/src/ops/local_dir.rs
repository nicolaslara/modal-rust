//! Local-directory UPLOAD → ephemeral Modal mount (`add_local_dir(copy=False)`).
//!
//! This is the source upload for BOTH the RUN path (mount at `/src`, build in the
//! function body) and the DEPLOY path (image build context at `/app/src`). It walks
//! a set of local directories (pruning build artifacts and ignored paths), hashes
//! every surviving file, pushes the bytes to the control plane (inline for small
//! files, blob for large), and finalizes one ephemeral `Mount` whose `mount_id` is
//! attached to a `Function` via `Function.mount_ids`. Each file lands at
//! `<remote_path>/<rel-as-posix>` so the in-container `cargo build` sees the same
//! layout as the local crate.
//!
//! ## What gets uploaded (file selection)
//!
//! Two entrypoints, two selection strategies that share the same upload core:
//!
//! - [`ModalClient::mount_workspace_closure`] (PRIMARY): upload ONLY a caller-chosen
//!   set of crate directories (the cargo dependency closure of the target package)
//!   plus explicit extra files (the workspace `Cargo.toml`/`Cargo.lock`). The caller
//!   (the facade's `scope` module) computes the closure from `cargo metadata`. This
//!   is correct-by-construction: `cargo build` of the target needs exactly the
//!   closure, so nothing else is uploaded.
//! - [`ModalClient::mount_local_dir`] (FALLBACK): walk the whole source root. Used
//!   when `cargo metadata` is unavailable (no `Cargo.toml`, non-cargo project, etc.).
//!
//! ## Ignore-file resolution (pruning WITHIN the uploaded dirs)
//!
//! Both strategies prune via a real gitignore engine ([`ignore::gitignore`]), rooted
//! at the workspace root, layered by precedence (highest → lowest):
//!
//! 1. `.modalignore` at the workspace root (gitignore syntax incl. `!` negation),
//! 2. `.gitignore` at the workspace root,
//! 3. built-in defaults (`target/`, `.git/`, `**/*.rlib`).
//!
//! The workspace `Cargo.toml`/`Cargo.lock` are added explicitly and are EXEMPT from
//! ignore matching (never-ignorable build inputs).
//!
//! ## Non-source assets are NOT uploaded
//!
//! The source upload carries source only. Datasets, model weights, and other large
//! assets must be attached via **Modal Volumes**, not the source mount.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use sha2::{Digest, Sha256};
use tokio::time::{Duration, Instant};
use walkdir::WalkDir;

use crate::client::ModalClient;
use crate::error::{Error, Result};
use crate::ops::blob::LARGE_FILE_BLOB_THRESHOLD;
use crate::proto::api::mount_put_file_request::DataOneof;
use crate::proto::api::{
    DeploymentNamespace, MountFile, MountGetOrCreateRequest, MountPutFileRequest,
    ObjectCreationType,
};

/// Client-side completion deadline for a single file's `MountPutFile` upload.
/// After the upload `MountPutFile`, the server may still report `exists=false`
/// transiently; we re-probe until it flips, bounded by this 10-minute deadline
/// (matches Modal's `MOUNT_PUT_FILE_CLIENT_TIMEOUT` / modal-rs `mount.rs:16`).
const MOUNT_PUT_FILE_CLIENT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Built-in default ignore patterns — the FLOOR for a project with no
/// `.gitignore`/`.modalignore`. Gitignore syntax. Build artifacts, VCS, and cargo
/// integration-test dirs only; everything else is decided by the project's own ignore
/// files.
///
/// `tests/` (a cargo INTEGRATION-test directory) is excluded because the source upload
/// exists ONLY to feed the in-container `cargo build -p <pkg> --bin modal_runner` — and
/// `--bin` never compiles `tests/*.rs`. Uploading them is pure waste (~13 needless
/// `live_*.rs`/`mock_*.rs` files across this workspace). Rust UNIT tests are inline
/// `mod tests {}` in `.rs` files (never a directory), so they ride along untouched; a
/// crate's `src/**/tests/` MODULE directory (rare) would also match, but such a layout
/// is not a build input for the runner bin either. A project can re-include a needed
/// path via a `!tests/...` negation in `.modalignore` (higher precedence than this
/// floor).
pub const DEFAULT_IGNORE_PATTERNS: &[&str] = &["target/", ".git/", "**/*.rlib", "tests/"];

/// Default filename for the highest-precedence ignore file (gitignore syntax).
pub const DEFAULT_MODALIGNORE_NAME: &str = ".modalignore";

/// A file selected for upload: its on-disk bytes plus the POSIX path it will hold
/// inside the mount (`<remote_path>/<rel>`), and its Unix mode if available.
struct LocalFile {
    /// Mount-relative POSIX path, e.g. `/src/examples/add/Cargo.toml`.
    mount_filename: String,
    /// File contents.
    data: Vec<u8>,
    /// Unix permission bits (`st_mode & 0o7777`); `None` on non-Unix.
    mode: Option<u32>,
}

/// The file-selection inputs for [`ModalClient::mount_workspace_closure`], bundled
/// so the upload call site stays readable (and within clippy's arg budget).
///
/// All paths are anchored at [`workspace_root`](Self::workspace_root): the matcher
/// is rooted there, and every uploaded file preserves its path relative to it under
/// `remote_path`, so the in-container layout matches the local workspace.
pub struct WorkspaceClosureSpec<'a> {
    /// Workspace root (the cargo-metadata root + the ignore-matcher anchor).
    pub workspace_root: &'a Path,
    /// Cargo dependency-closure crate directories of the target package (computed by
    /// the facade from `cargo metadata`); each is walked and pruned by the matcher.
    pub crate_dirs: &'a [PathBuf],
    /// Extra files uploaded verbatim FROM DISK (e.g. the workspace `Cargo.lock`),
    /// EXEMPT from ignore matching.
    pub extra_files: &'a [PathBuf],
    /// Extra files uploaded from IN-MEMORY bytes as `(workspace-relative POSIX path,
    /// bytes)` — the REWRITTEN workspace `Cargo.toml` whose members are scoped to the
    /// closure (the on-disk manifest lists ALL members, which would not load against a
    /// subset upload). Takes precedence over a same-path disk/walked file.
    pub extra_inline_files: &'a [(String, Vec<u8>)],
    /// Highest-precedence ignore filename (default [`DEFAULT_MODALIGNORE_NAME`]).
    pub modalignore_name: &'a str,
}

/// Build the EPHEMERAL source `MountGetOrCreate` request — pure, no I/O.
///
/// The WORKSPACE-namespace, EPHEMERAL-creation mount that carries the uploaded
/// source/build-context `files` (the SHA-addressed [`MountFile`] descriptors).
/// Extracted from [`ModalClient::finalize_mount`]; the method passes the resolved
/// `environment_name` and the already-uploaded `files`.
pub(crate) fn build_mount_get_or_create_source_request(
    environment_name: String,
    files: Vec<MountFile>,
) -> MountGetOrCreateRequest {
    MountGetOrCreateRequest {
        deployment_name: String::new(),
        namespace: DeploymentNamespace::Workspace as i32,
        environment_name,
        object_creation_type: ObjectCreationType::Ephemeral as i32,
        files,
        // EPHEMERAL needs no app_id. TODO(fallback): if the server ever
        // rejects EPHEMERAL for use in Function.mount_ids, switch to
        // ANONYMOUS_OWNED_BY_APP (=4) with app_id set by the facade.
        app_id: String::new(),
    }
}

/// Build a `MountPutFile` request — pure, no I/O. ONE builder for BOTH shapes the
/// upload tail relies on:
/// - the existence PROBE (`data == None` ⇒ "do you already have it?"),
/// - the inline/blob UPLOAD (`data == Some(..)`).
///
/// Extracted from [`ModalClient::ensure_file_uploaded`] /
/// [`ModalClient::mount_put_file_probe`].
pub(crate) fn build_mount_put_file_request(
    sha256_hex: &str,
    data: Option<DataOneof>,
) -> MountPutFileRequest {
    MountPutFileRequest {
        sha256_hex: sha256_hex.to_string(),
        data_oneof: data,
    }
}

impl ModalClient {
    /// Upload ONLY the closure crate directories (plus the explicit extra files) as
    /// an EPHEMERAL Modal mount under `remote_path`; return its `mount_id`. PRIMARY
    /// upload path. See [`WorkspaceClosureSpec`] for the selection inputs.
    ///
    /// `environment` defaults to the configured environment (or `"main"`).
    pub async fn mount_workspace_closure(
        &mut self,
        spec: &WorkspaceClosureSpec<'_>,
        remote_path: &str,
        environment: Option<&str>,
    ) -> Result<String> {
        if !spec.workspace_root.is_dir() {
            return Err(Error::invalid(format!(
                "mount_workspace_closure: workspace root '{}' is not a directory",
                spec.workspace_root.display()
            )));
        }
        let matcher = build_matcher(spec.workspace_root, spec.modalignore_name)?;
        let files = collect_files_for_dirs(
            spec.workspace_root,
            spec.crate_dirs,
            spec.extra_files,
            spec.extra_inline_files,
            remote_path,
            &matcher,
        )?;
        self.finalize_mount(files, spec.workspace_root, environment)
            .await
    }

    /// Upload a whole local directory as an EPHEMERAL Modal mount; return its
    /// `mount_id`. FALLBACK upload path (used when `cargo metadata` is unavailable).
    ///
    /// Walks `local_dir` and prunes via the resolved ignore matcher, with precedence
    /// `.modalignore` then `.gitignore` then built-in defaults, rooted at `local_dir`.
    /// Files map to `"<remote_path>/<rel-as-posix>"`. `modalignore_name` is the
    /// highest-precedence ignore filename. `environment` defaults to the configured
    /// environment.
    pub async fn mount_local_dir(
        &mut self,
        local_dir: impl AsRef<Path>,
        remote_path: &str,
        modalignore_name: &str,
        environment: Option<&str>,
    ) -> Result<String> {
        self.mount_local_dir_with_inline(local_dir, remote_path, modalignore_name, environment, &[])
            .await
    }

    /// [`mount_local_dir`](Self::mount_local_dir) plus `extra_inline_files`: in-memory
    /// `(workspace-relative POSIX path, bytes)` appended verbatim, EXEMPT from ignore
    /// matching and OVERWRITING any same-path walked file. The facade uses this on the
    /// FALLBACK source-mount arm to inject the tooling-generated `modal_runner.rs` even
    /// when cargo-metadata scoping is unavailable.
    pub async fn mount_local_dir_with_inline(
        &mut self,
        local_dir: impl AsRef<Path>,
        remote_path: &str,
        modalignore_name: &str,
        environment: Option<&str>,
        extra_inline_files: &[(String, Vec<u8>)],
    ) -> Result<String> {
        let local_dir = local_dir.as_ref();
        if !local_dir.is_dir() {
            return Err(Error::invalid(format!(
                "mount_local_dir: '{}' is not a directory",
                local_dir.display()
            )));
        }
        let matcher = build_matcher(local_dir, modalignore_name)?;
        let files = collect_files(local_dir, remote_path, &matcher, extra_inline_files)?;
        self.finalize_mount(files, local_dir, environment).await
    }

    /// Shared upload + `MountGetOrCreate` finalize for both selection strategies.
    async fn finalize_mount(
        &mut self,
        files: Vec<LocalFile>,
        source_root: &Path,
        environment: Option<&str>,
    ) -> Result<String> {
        if files.is_empty() {
            return Err(Error::invalid(format!(
                "no files to upload under '{}' after applying ignore files",
                source_root.display()
            )));
        }
        let environment_name = self.env_or_default(environment);

        // Log the exact uploaded file set (paths + total bytes) so the scoped
        // upload is observable: which crate dirs the cargo closure selected and
        // which files the ignore files (`.modalignore` > `.gitignore` > defaults)
        // pruned. Cheap, durable evidence; goes to stderr (never stdout).
        let total_bytes: u64 = files.iter().map(|f| f.data.len() as u64).sum();
        eprintln!(
            "[modal-rust] source upload: {} files, {} bytes from '{}'",
            files.len(),
            total_bytes,
            source_root.display()
        );
        for f in &files {
            eprintln!("[modal-rust]   upload {}", f.mount_filename);
        }

        let mount_files = self.upload_files(files).await?;

        // Keyed by the sha-addressed `files` set: re-sending yields the same mount,
        // so retrying on a transient reset is safe.
        let req = build_mount_get_or_create_source_request(environment_name, mount_files);
        let resp = self
            .retry_rpc(
                "mount_get_or_create(source)",
                req,
                |mut stub, req| async move { stub.mount_get_or_create(req).await },
            )
            .await?;

        if resp.mount_id.is_empty() {
            return Err(Error::build(
                "MountGetOrCreate returned an empty mount_id for the uploaded local directory",
            ));
        }
        Ok(resp.mount_id)
    }

    /// Hash, dedup, and upload each [`LocalFile`], returning the assembled
    /// [`MountFile`] descriptors sorted by filename (deterministic mount build).
    ///
    /// Uploads run with BOUNDED CONCURRENCY (16-way, matching the Python client):
    /// for an unchanged source tree every file resolves as a sha256 probe hit
    /// ("server already has it"), so the cost is pure round-trip latency — done
    /// sequentially that was ~N×RTT (5-10s for a typical crate), concurrently it
    /// is ~N/16×RTT. Clones of [`ModalClient`] share one multiplexed h2 channel.
    async fn upload_files(&mut self, files: Vec<LocalFile>) -> Result<Vec<MountFile>> {
        const UPLOAD_CONCURRENCY: usize = 16;

        let mut accounted_sha: HashSet<String> = HashSet::new();
        let mut seen_paths: HashSet<String> = HashSet::new();
        let mut mount_files: Vec<MountFile> = Vec::with_capacity(files.len());
        let mut to_upload: Vec<(String, Vec<u8>)> = Vec::new();

        for file in files {
            if !seen_paths.insert(file.mount_filename.clone()) {
                return Err(Error::invalid(format!(
                    "duplicate mount path '{}'",
                    file.mount_filename
                )));
            }
            let sha256_hex = sha256_hex(&file.data);

            // Skip RPCs for identical content already accounted for this run
            // (in-run dedup); the server also dedups by sha across runs/users.
            if accounted_sha.insert(sha256_hex.clone()) {
                to_upload.push((sha256_hex.clone(), file.data.clone()));
            }

            mount_files.push(MountFile {
                filename: file.mount_filename,
                sha256_hex,
                size: Some(file.data.len() as u64),
                mode: file.mode,
            });
        }

        let mut join = tokio::task::JoinSet::new();
        let mut queue = to_upload.into_iter();
        let mut spawn_next = |join: &mut tokio::task::JoinSet<Result<()>>,
                              item: (String, Vec<u8>)| {
            let mut client = self.clone();
            join.spawn(async move { client.ensure_file_uploaded(&item.0, &item.1).await });
        };
        for item in queue.by_ref().take(UPLOAD_CONCURRENCY) {
            spawn_next(&mut join, item);
        }
        while let Some(res) = join.join_next().await {
            res.map_err(|e| Error::build(format!("mount upload task panicked: {e}")))??;
            if let Some(item) = queue.next() {
                spawn_next(&mut join, item);
            }
        }

        mount_files.sort_by(|a, b| a.filename.cmp(&b.filename));
        Ok(mount_files)
    }

    /// Ensure the file with `sha256` exists on the control plane, uploading it if
    /// needed and waiting (bounded) for the server to confirm it.
    async fn ensure_file_uploaded(&mut self, sha256: &str, data: &[u8]) -> Result<()> {
        // Existence probe: empty `data_oneof` asks "do you already have it?"
        if self.mount_put_file_probe(sha256).await? {
            return Ok(());
        }

        // Upload: inline for small files, blob for large.
        let data_oneof = if data.len() >= LARGE_FILE_BLOB_THRESHOLD {
            DataOneof::DataBlobId(self.blob_create_and_put(data).await?)
        } else {
            DataOneof::Data(data.to_vec())
        };
        // The server dedups by sha256_hex, so re-PUT of the same bytes is a no-op:
        // safe to retry on a transient reset.
        let req = build_mount_put_file_request(sha256, Some(data_oneof));
        self.retry_rpc("mount_put_file(upload)", req, |mut stub, req| async move {
            stub.mount_put_file(req).await
        })
        .await?;

        // Completion gate: re-probe until the server confirms (bounded).
        let deadline = Instant::now() + MOUNT_PUT_FILE_CLIENT_TIMEOUT;
        loop {
            if self.mount_put_file_probe(sha256).await? {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Error::build(format!(
                    "uploading mount file with sha256 {sha256} timed out after 10 minutes"
                )));
            }
        }
    }

    /// Issue a probe-shape `MountPutFile` (no data) and return `exists`.
    async fn mount_put_file_probe(&mut self, sha256: &str) -> Result<bool> {
        // Pure existence read — idempotent, safe to retry on a transient reset.
        let req = build_mount_put_file_request(sha256, None);
        Ok(self
            .retry_rpc("mount_put_file(probe)", req, |mut stub, req| async move {
                stub.mount_put_file(req).await
            })
            .await?
            .exists)
    }
}

/// Lowercase hex SHA-256 of `data`.
pub(crate) fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

/// Build the layered ignore matcher rooted at `workspace_root`, with precedence
/// (highest → lowest): `<modalignore_name>` > `.gitignore` > built-in defaults.
///
/// The matcher is rooted at `workspace_root`, so callers query it with paths
/// RELATIVE to `workspace_root` (the in-container layout root). Layers are added in
/// LOWEST→HIGHEST order because `ignore`'s matcher resolves "last match wins": the
/// defaults go in first, then `.gitignore`, then `<modalignore_name>` LAST so its
/// rules (and its `!` negations) win over both.
///
/// Only the workspace-root ignore files are consulted (no per-directory `.gitignore`
/// discovery, no `~/.config/git/ignore`, no parent-dir walk) — a single, predictable,
/// documented set of sources.
fn build_matcher(workspace_root: &Path, modalignore_name: &str) -> Result<Gitignore> {
    let mut builder = GitignoreBuilder::new(workspace_root);

    // 1. Built-in defaults (lowest precedence — the floor).
    for pat in DEFAULT_IGNORE_PATTERNS {
        builder
            .add_line(None, pat)
            .map_err(|e| Error::build(format!("invalid default ignore pattern '{pat}': {e}")))?;
    }
    // 2. .gitignore (if present). `add` returns Some(err) only on a real problem;
    //    a missing file surfaces as an Io error we treat as "absent" (skip).
    let gitignore = workspace_root.join(".gitignore");
    if gitignore.is_file() {
        if let Some(err) = builder.add(&gitignore) {
            return Err(Error::build(format!(
                "failed to parse '{}': {err}",
                gitignore.display()
            )));
        }
    }
    // 3. .modalignore (highest precedence — added LAST so it wins).
    let modalignore = workspace_root.join(modalignore_name);
    if modalignore.is_file() {
        if let Some(err) = builder.add(&modalignore) {
            return Err(Error::build(format!(
                "failed to parse '{}': {err}",
                modalignore.display()
            )));
        }
    }

    builder
        .build()
        .map_err(|e| Error::build(format!("failed to build ignore matcher: {e}")))
}

/// Is the path (RELATIVE to the matcher's `workspace_root`) ignored? Honors `!`
/// negations (a whitelist match re-includes). Directory pruning uses `is_dir=true`.
fn is_ignored(matcher: &Gitignore, rel: &Path, is_dir: bool) -> bool {
    // Empty rel = the workspace root itself; never ignore it.
    if rel.as_os_str().is_empty() {
        return false;
    }
    matcher.matched_path_or_any_parents(rel, is_dir).is_ignore()
}

/// Normalize the remote mount prefix: strip a trailing `/` so joins never produce
/// a `//`. A bare `/` (mount at root) normalizes to the empty string.
fn normalize_remote_prefix(remote_path: &str) -> String {
    remote_path.trim_end_matches('/').to_string()
}

/// Join a normalized `remote_prefix` with a POSIX relative path into a mount path.
fn mount_path(remote_prefix: &str, rel_posix: &str) -> String {
    if remote_prefix.is_empty() {
        format!("/{rel_posix}")
    } else {
        format!("{remote_prefix}/{rel_posix}")
    }
}

/// PRIMARY collector: walk each crate dir in `crate_dirs`, emitting files at
/// `<remote_prefix>/<rel-to-workspace_root>` and pruning via `matcher` (queried with
/// the path relative to `workspace_root`). Then append `extra_files` (read from disk)
/// and `extra_inline_files` (in-memory bytes) verbatim — both EXEMPT from ignore
/// matching (these are never-ignorable workspace manifests).
fn collect_files_for_dirs(
    workspace_root: &Path,
    crate_dirs: &[PathBuf],
    extra_files: &[PathBuf],
    extra_inline_files: &[(String, Vec<u8>)],
    remote_prefix: &str,
    matcher: &Gitignore,
) -> Result<Vec<LocalFile>> {
    let remote_prefix = normalize_remote_prefix(remote_prefix);
    let remote_prefix = remote_prefix.as_str();
    let mut files = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for dir in crate_dirs {
        if !dir.is_dir() {
            return Err(Error::build(format!(
                "closure crate dir '{}' is not a directory",
                dir.display()
            )));
        }
        // Walk the crate dir; prune ignored subtrees early. We compute the path
        // RELATIVE to workspace_root so the matcher (rooted there) and the mount
        // layout both see the same in-workspace path.
        let walker = WalkDir::new(dir).into_iter().filter_entry(|entry| {
            match entry.path().strip_prefix(workspace_root) {
                Ok(rel) => !is_ignored(matcher, rel, entry.file_type().is_dir()),
                // The crate dir might be a sibling outside workspace_root (a path
                // dep elsewhere); keep it (matcher can't anchor it anyway).
                Err(_) => true,
            }
        });

        for entry in walker {
            let entry = entry
                .map_err(|e| Error::build(format!("walking closure crate dir failed: {e}")))?;
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = entry.path().strip_prefix(workspace_root).map_err(|e| {
                Error::build(format!(
                    "closure crate dir '{}' is outside the workspace root '{}': {e}",
                    entry.path().display(),
                    workspace_root.display()
                ))
            })?;
            let rel_posix = to_posix(rel);
            let mount_filename = mount_path(remote_prefix, &rel_posix);
            if !seen.insert(mount_filename.clone()) {
                // Overlapping crate dirs (one nested in another) — keep first.
                continue;
            }
            files.push(read_local_file(entry.path(), mount_filename)?);
        }
    }

    // Extra files (workspace Cargo.toml/Cargo.lock): added verbatim, no ignore check.
    for path in extra_files {
        if !path.is_file() {
            // A workspace may legitimately lack Cargo.lock (e.g. a lib-only ws); skip.
            continue;
        }
        let rel = path.strip_prefix(workspace_root).map_err(|e| {
            Error::build(format!(
                "extra file '{}' is outside the workspace root '{}': {e}",
                path.display(),
                workspace_root.display()
            ))
        })?;
        let rel_posix = to_posix(rel);
        let mount_filename = mount_path(remote_prefix, &rel_posix);
        if !seen.insert(mount_filename.clone()) {
            continue;
        }
        files.push(read_local_file(path, mount_filename)?);
    }

    append_inline_files(&mut files, &mut seen, remote_prefix, extra_inline_files);
    Ok(files)
}

/// Append `extra_inline_files` (in-memory `(workspace-relative POSIX path, bytes)`)
/// to `files`, mounted at `<remote_prefix>/<rel-posix>`, EXEMPT from ignore matching.
/// Each takes PRECEDENCE over a same-path walked/extra file (e.g. the rewritten
/// workspace `Cargo.toml` over the verbatim one, or the injected `modal_runner.rs`):
/// inserted last with an explicit overwrite of any prior same-path entry. Shared by
/// BOTH the closure collector and the fallback whole-dir collector so their inline
/// semantics cannot drift.
fn append_inline_files(
    files: &mut Vec<LocalFile>,
    seen: &mut HashSet<String>,
    remote_prefix: &str,
    extra_inline_files: &[(String, Vec<u8>)],
) {
    for (rel_posix, data) in extra_inline_files {
        let rel_posix = rel_posix.trim_start_matches('/');
        let mount_filename = mount_path(remote_prefix, rel_posix);
        // Overwrite any prior entry at this mount path (the inline bytes win over a
        // verbatim file a walk might have emitted).
        files.retain(|f| f.mount_filename != mount_filename);
        seen.insert(mount_filename.clone());
        files.push(LocalFile {
            mount_filename,
            data: data.clone(),
            mode: Some(0o644),
        });
    }
}

/// FALLBACK collector: walk the whole `local_dir`, prune ignored directories early
/// (directory pruning stops descent — critical for `target/`), and collect the
/// surviving files as [`LocalFile`] with `<remote_prefix>/<rel-as-posix>` paths. Then
/// append `extra_inline_files` (the injected `modal_runner.rs`) with the SAME
/// overwrite-precedence + ignore-exemption as the closure collector.
fn collect_files(
    local_dir: &Path,
    remote_prefix: &str,
    matcher: &Gitignore,
    extra_inline_files: &[(String, Vec<u8>)],
) -> Result<Vec<LocalFile>> {
    let remote_prefix = normalize_remote_prefix(remote_prefix);
    let remote_prefix = remote_prefix.as_str();
    let mut files = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let walker = WalkDir::new(local_dir).into_iter().filter_entry(|entry| {
        // Always keep the root itself; prune any descendant dir/file whose RELATIVE
        // path is ignored (directory pruning stops descent into huge trees).
        match entry.path().strip_prefix(local_dir) {
            Ok(rel) => !is_ignored(matcher, rel, entry.file_type().is_dir()),
            Err(_) => true,
        }
    });

    for entry in walker {
        let entry =
            entry.map_err(|e| Error::build(format!("walking local source dir failed: {e}")))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(local_dir)
            .map_err(|e| Error::build(format!("path prefix error during walk: {e}")))?;
        let rel_posix = to_posix(rel);
        let mount_filename = mount_path(remote_prefix, &rel_posix);
        seen.insert(mount_filename.clone());
        files.push(read_local_file(entry.path(), mount_filename)?);
    }

    append_inline_files(&mut files, &mut seen, remote_prefix, extra_inline_files);
    Ok(files)
}

/// Read a file's bytes + mode into a [`LocalFile`] bound for `mount_filename`.
fn read_local_file(path: &Path, mount_filename: String) -> Result<LocalFile> {
    let data = std::fs::read(path)
        .map_err(|e| Error::build(format!("reading '{}' failed: {e}", path.display())))?;
    let mode = file_mode(path);
    Ok(LocalFile {
        mount_filename,
        data,
        mode,
    })
}

/// Convert a relative path to a POSIX (`/`-separated) string. On Windows the
/// component separator is `\`; we always emit `/` for mount paths.
fn to_posix(rel: &Path) -> String {
    rel.components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

/// Read the Unix permission bits (`st_mode & 0o7777`); `None` on non-Unix.
#[cfg(unix)]
fn file_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.mode() & 0o7777)
}

/// Permission bits are not portable on non-Unix; emit `None`.
#[cfg(not(unix))]
fn file_mode(_path: &Path) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn build_mount_get_or_create_source_request_is_ephemeral_workspace() {
        let files = vec![MountFile {
            filename: "/src/app/src/lib.rs".to_string(),
            sha256_hex: "abc".to_string(),
            size: Some(7),
            mode: Some(0o644),
        }];
        let req = build_mount_get_or_create_source_request("main".to_string(), files);
        // EPHEMERAL + WORKSPACE namespace; empty deployment_name/app_id.
        assert_eq!(req.namespace, DeploymentNamespace::Workspace as i32);
        assert_eq!(
            req.object_creation_type,
            ObjectCreationType::Ephemeral as i32
        );
        assert!(req.deployment_name.is_empty());
        assert!(req.app_id.is_empty());
        assert_eq!(req.environment_name, "main");
        // The uploaded files pass through.
        assert_eq!(req.files.len(), 1);
        assert_eq!(req.files[0].filename, "/src/app/src/lib.rs");
    }

    #[test]
    fn build_mount_put_file_request_probe_vs_upload() {
        // PROBE: data_oneof None ("do you have it?").
        let probe = build_mount_put_file_request("deadbeef", None);
        assert_eq!(probe.sha256_hex, "deadbeef");
        assert!(probe.data_oneof.is_none(), "probe sends no data");

        // INLINE UPLOAD: data_oneof Some(Data(..)).
        let upload = build_mount_put_file_request("deadbeef", Some(DataOneof::Data(vec![1, 2, 3])));
        match upload.data_oneof {
            Some(DataOneof::Data(bytes)) => assert_eq!(bytes, vec![1, 2, 3]),
            other => panic!("expected inline Data, got {other:?}"),
        }
    }

    #[test]
    fn sha256_hex_is_lowercase_digest() {
        assert_eq!(
            sha256_hex(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn remote_prefix_normalizes_trailing_slash() {
        assert_eq!(normalize_remote_prefix("/src"), "/src");
        assert_eq!(normalize_remote_prefix("/src/"), "/src");
        assert_eq!(normalize_remote_prefix("/"), "");
    }

    #[test]
    fn defaults_prune_target_git_and_rlib() {
        // No .gitignore / .modalignore: just the built-in defaults floor.
        let dir = TempTree::new();
        let m = build_matcher(dir.path(), DEFAULT_MODALIGNORE_NAME).unwrap();
        assert!(is_ignored(&m, &p("target"), true));
        assert!(is_ignored(&m, &p("target/release/foo"), false));
        assert!(is_ignored(&m, &p("crates/x/target"), true));
        assert!(is_ignored(&m, &p(".git/config"), false));
        assert!(is_ignored(&m, &p("deps/libfoo.rlib"), false));
        // Kept: real source.
        assert!(!is_ignored(&m, &p("src/lib.rs"), false));
        assert!(!is_ignored(&m, &p("Cargo.toml"), false));
        assert!(!is_ignored(
            &m,
            &p("examples/add/src/bin/modal_runner.rs"),
            false
        ));
    }

    #[test]
    fn defaults_prune_integration_tests_dir() {
        // `tests/` (cargo integration tests) is excluded by the built-in floor: the
        // source upload feeds `cargo build --bin modal_runner`, which never compiles
        // `tests/*.rs`. Unit tests are inline `mod tests {}` in `.rs` files (kept).
        let dir = TempTree::new();
        let m = build_matcher(dir.path(), DEFAULT_MODALIGNORE_NAME).unwrap();
        // A crate-root tests dir and its files are ignored…
        assert!(is_ignored(&m, &p("tests"), true));
        assert!(is_ignored(&m, &p("tests/live_run.rs"), false));
        // …at any depth (a workspace member's tests dir).
        assert!(is_ignored(&m, &p("crates/x/tests/mock_wire.rs"), false));
        assert!(is_ignored(&m, &p("examples/quickstart/tests/it.rs"), false));
        // Real source under src/ is kept (including a FILE literally named tests.rs).
        assert!(!is_ignored(&m, &p("src/lib.rs"), false));
        assert!(!is_ignored(&m, &p("src/tests.rs"), false));
    }

    #[test]
    fn collect_files_for_dirs_excludes_tests_dir() {
        // The PRIMARY (closure) collector drops a crate's `tests/` dir but keeps src/.
        let dir = TempTree::new();
        dir.write("Cargo.toml", "[workspace]\n");
        dir.write("Cargo.lock", "# lock\n");
        dir.write("a/Cargo.toml", "[package]\n");
        dir.write("a/src/lib.rs", "fn a() {}\n");
        dir.write(
            "a/tests/live_a.rs",
            "// integration test, not a build input\n",
        );
        let m = build_matcher(dir.path(), DEFAULT_MODALIGNORE_NAME).unwrap();
        let crate_dirs = vec![dir.path().join("a")];
        let extras = vec![dir.path().join("Cargo.toml"), dir.path().join("Cargo.lock")];
        let files =
            collect_files_for_dirs(dir.path(), &crate_dirs, &extras, &[], "/src", &m).unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.mount_filename.as_str()).collect();
        assert!(
            names.contains(&"/src/a/src/lib.rs"),
            "crate source is uploaded: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n.contains("/tests/")),
            "no tests/ files uploaded: {names:?}"
        );
    }

    #[test]
    fn collect_files_fallback_excludes_tests_dir() {
        // The FALLBACK whole-dir walk also prunes `tests/`.
        let dir = TempTree::new();
        dir.write("Cargo.toml", "[package]\n");
        dir.write("src/lib.rs", "fn a() {}\n");
        dir.write("tests/mock_x.rs", "// not a build input\n");
        let m = build_matcher(dir.path(), DEFAULT_MODALIGNORE_NAME).unwrap();
        let files = collect_files(dir.path(), "/src", &m, &[]).unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.mount_filename.as_str()).collect();
        assert!(names.contains(&"/src/src/lib.rs"), "{names:?}");
        assert!(
            !names.iter().any(|n| n.contains("/tests/")),
            "tests/ pruned on the fallback walk: {names:?}"
        );
    }

    #[test]
    fn gitignore_layer_prunes_its_entries() {
        // .gitignore adds `references/` and `workpads/` on top of the defaults —
        // the exact scenario that previously required the hardcoded list.
        let dir = TempTree::new();
        dir.write(".gitignore", "references/\nworkpads/\n");
        let m = build_matcher(dir.path(), DEFAULT_MODALIGNORE_NAME).unwrap();
        // From .gitignore:
        assert!(is_ignored(&m, &p("references"), true));
        assert!(is_ignored(&m, &p("references/modal-rs/Cargo.toml"), false));
        assert!(is_ignored(
            &m,
            &p("workpads/shim-backend/knowledge.md"),
            false
        ));
        // Defaults still apply.
        assert!(is_ignored(&m, &p("target/debug/x"), false));
        // Kept.
        assert!(!is_ignored(&m, &p("Cargo.toml"), false));
        assert!(!is_ignored(
            &m,
            &p("crates/modal-rust-sdk/src/lib.rs"),
            false
        ));
    }

    #[test]
    fn modalignore_overrides_gitignore_with_negation() {
        // .modalignore (highest) re-includes a path .gitignore excluded, and adds a
        // new exclusion. Precedence: .modalignore > .gitignore > defaults.
        let dir = TempTree::new();
        dir.write(".gitignore", "secret/\nkeep_me/\n");
        // Re-include keep_me/ (negation wins), and newly exclude scratch/.
        dir.write(".modalignore", "!keep_me/\nscratch/\n");
        let m = build_matcher(dir.path(), DEFAULT_MODALIGNORE_NAME).unwrap();
        // .gitignore still hides secret/.
        assert!(is_ignored(&m, &p("secret/data"), false));
        // .modalignore negation re-includes keep_me/.
        assert!(!is_ignored(&m, &p("keep_me/data.txt"), false));
        // .modalignore newly excludes scratch/.
        assert!(is_ignored(&m, &p("scratch/tmp"), false));
        // Defaults still apply under .modalignore.
        assert!(is_ignored(&m, &p("target/x"), false));
    }

    #[test]
    fn custom_modalignore_name_is_honored() {
        let dir = TempTree::new();
        dir.write(".myignore", "blocked/\n");
        let m = build_matcher(dir.path(), ".myignore").unwrap();
        assert!(is_ignored(&m, &p("blocked/x"), false));
        assert!(!is_ignored(&m, &p("kept/x"), false));
    }

    #[test]
    fn collect_files_for_dirs_scopes_to_closure() {
        // Workspace layout: two crate dirs in the closure (a, b), one NOT (c), and a
        // target/ inside a crate. extra = root Cargo.toml/Cargo.lock.
        let dir = TempTree::new();
        dir.write("Cargo.toml", "[workspace]\n");
        dir.write("Cargo.lock", "# lock\n");
        dir.write("a/Cargo.toml", "[package]\n");
        dir.write("a/src/lib.rs", "fn a() {}\n");
        dir.write("a/target/junk", "junk"); // pruned by defaults
        dir.write("b/Cargo.toml", "[package]\n");
        dir.write("b/src/lib.rs", "fn b() {}\n");
        dir.write("c/Cargo.toml", "[package]\n"); // NOT in closure → not uploaded
        dir.write("c/src/lib.rs", "fn c() {}\n");

        let m = build_matcher(dir.path(), DEFAULT_MODALIGNORE_NAME).unwrap();
        let crate_dirs = vec![dir.path().join("a"), dir.path().join("b")];
        let extras = vec![dir.path().join("Cargo.toml"), dir.path().join("Cargo.lock")];
        let mut files =
            collect_files_for_dirs(dir.path(), &crate_dirs, &extras, &[], "/src", &m).unwrap();
        files.sort_by(|x, y| x.mount_filename.cmp(&y.mount_filename));
        let names: Vec<&str> = files.iter().map(|f| f.mount_filename.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "/src/Cargo.lock",
                "/src/Cargo.toml",
                "/src/a/Cargo.toml",
                "/src/a/src/lib.rs",
                "/src/b/Cargo.toml",
                "/src/b/src/lib.rs",
            ],
            "only closure crate dirs (a, b) + root manifests; c/ and target/ excluded"
        );
    }

    #[test]
    fn collect_files_for_dirs_extras_exempt_from_ignore() {
        // Even if a .gitignore would match Cargo.lock, the explicit extra is kept.
        let dir = TempTree::new();
        dir.write("Cargo.toml", "[workspace]\n");
        dir.write("Cargo.lock", "# lock\n");
        dir.write(".gitignore", "Cargo.lock\n"); // would normally hide it
        dir.write("a/Cargo.toml", "[package]\n");
        dir.write("a/src/lib.rs", "fn a() {}\n");
        let m = build_matcher(dir.path(), DEFAULT_MODALIGNORE_NAME).unwrap();
        let crate_dirs = vec![dir.path().join("a")];
        let extras = vec![dir.path().join("Cargo.toml"), dir.path().join("Cargo.lock")];
        let files =
            collect_files_for_dirs(dir.path(), &crate_dirs, &extras, &[], "/src", &m).unwrap();
        assert!(
            files.iter().any(|f| f.mount_filename == "/src/Cargo.lock"),
            "explicit extra files are exempt from ignore matching"
        );
    }

    #[test]
    fn collect_files_for_dirs_inline_manifest_overrides_disk() {
        // The rewritten workspace Cargo.toml (inline bytes) must WIN over the verbatim
        // on-disk one a crate-dir walk might emit at the same mount path.
        let dir = TempTree::new();
        dir.write("Cargo.toml", "[workspace]\nmembers=[\"a\",\"b\",\"c\"]\n");
        dir.write("a/Cargo.toml", "[package]\n");
        dir.write("a/src/lib.rs", "fn a() {}\n");
        let m = build_matcher(dir.path(), DEFAULT_MODALIGNORE_NAME).unwrap();
        let crate_dirs = vec![dir.path().join("a")];
        // No on-disk extra; the rewritten manifest is inline.
        let inline = vec![(
            "Cargo.toml".to_string(),
            b"[workspace]\nmembers=[\"a\"]\n".to_vec(),
        )];
        let files =
            collect_files_for_dirs(dir.path(), &crate_dirs, &[], &inline, "/src", &m).unwrap();
        let manifest = files
            .iter()
            .find(|f| f.mount_filename == "/src/Cargo.toml")
            .expect("workspace Cargo.toml present");
        assert_eq!(
            manifest.data, b"[workspace]\nmembers=[\"a\"]\n",
            "inline rewritten manifest must win over the on-disk verbatim one"
        );
        // Exactly one entry at that path (no duplicate).
        assert_eq!(
            files
                .iter()
                .filter(|f| f.mount_filename == "/src/Cargo.toml")
                .count(),
            1
        );
    }

    #[test]
    fn collect_files_fallback_prunes_and_maps_posix() {
        let dir = tempdir_with_files();
        let m = build_matcher(dir.path(), DEFAULT_MODALIGNORE_NAME).unwrap();
        let mut files = collect_files(dir.path(), "/src", &m, &[]).unwrap();
        files.sort_by(|a, b| a.mount_filename.cmp(&b.mount_filename));
        let names: Vec<&str> = files.iter().map(|f| f.mount_filename.as_str()).collect();
        assert_eq!(
            names,
            vec!["/src/Cargo.toml", "/src/sub/main.rs"],
            "ignored target/ and *.rlib must be excluded; survivors use /src POSIX paths"
        );
    }

    #[test]
    fn collect_files_fallback_handles_root_mount_prefix() {
        let dir = tempdir_with_files();
        let m = build_matcher(dir.path(), DEFAULT_MODALIGNORE_NAME).unwrap();
        let files = collect_files(dir.path(), "/", &m, &[]).unwrap();
        assert!(files.iter().any(|f| f.mount_filename == "/Cargo.toml"));
        assert!(files.iter().all(|f| !f.mount_filename.starts_with("//")));
    }

    #[test]
    fn collect_files_fallback_appends_inline_runner() {
        // The fallback whole-dir walk also carries injected inline files (the generated
        // `modal_runner.rs`), EXEMPT from ignore and at the right mount path.
        let dir = tempdir_with_files();
        let m = build_matcher(dir.path(), DEFAULT_MODALIGNORE_NAME).unwrap();
        let inline = vec![(
            "app/src/bin/modal_runner.rs".to_string(),
            b"modal_rust::modal_runner!(app);\n".to_vec(),
        )];
        let files = collect_files(dir.path(), "/src", &m, &inline).unwrap();
        let runner = files
            .iter()
            .find(|f| f.mount_filename == "/src/app/src/bin/modal_runner.rs")
            .expect("injected runner present on the fallback path");
        assert_eq!(runner.data, b"modal_rust::modal_runner!(app);\n");
    }

    /// Build a small temp tree: Cargo.toml, sub/main.rs, target/junk (ignored),
    /// deps/foo.rlib (ignored).
    fn tempdir_with_files() -> TempTree {
        let t = TempTree::new();
        t.write("Cargo.toml", "[package]\n");
        t.write("sub/main.rs", "fn main() {}\n");
        t.write("target/release/junk", "junk");
        t.write("deps/foo.rlib", "rlib");
        t
    }

    /// A unique temp dir (PID + atomic counter) removed on drop. Unique per call so
    /// concurrently-run tests never share a directory.
    struct TempTree {
        path: PathBuf,
    }
    impl TempTree {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let base = std::env::temp_dir().join(format!(
                "modal_rust_local_dir_test_{}_{n}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&base).unwrap();
            TempTree { path: base }
        }
        fn path(&self) -> &Path {
            &self.path
        }
        fn write(&self, rel: &str, contents: &str) {
            let full = self.path.join(rel);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(full, contents).unwrap();
        }
    }
    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
