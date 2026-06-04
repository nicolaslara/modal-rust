//! Local-directory UPLOAD → ephemeral Modal mount (`add_local_dir(copy=False)`).
//!
//! This is the RUN-path source upload: walk a local directory (pruning build
//! artifacts), hash every surviving file, push the file bytes to the control
//! plane (inline for small files, blob for large), and finalize one ephemeral
//! `Mount` whose `mount_id` is attached to a `Function` via `Function.mount_ids`.
//! The mounted files land at `<remote_path>/<rel-as-posix>` so the in-container
//! `cargo build` in `/src` sees the same layout as the local crate.
//!
//! Ported from Modal's `_MountDir.get_files_to_upload` / `mount.py` and the
//! modal-rs `mount.rs` precedent, narrowed to the 4-pattern ignore subset the
//! proven `dev_app.py` recipe uses (no full gitignore engine — keeps CI lean).

use std::collections::HashSet;
use std::path::Path;

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

impl ModalClient {
    /// Upload a local directory as an EPHEMERAL Modal mount; return its `mount_id`.
    ///
    /// Files map to `"<remote_path>/<rel-as-posix>"`. `ignore` is the small pattern
    /// subset (bare segments pruned anywhere in the path; `*.<ext>` / `**/*.<ext>`
    /// by extension) matched against the path RELATIVE to `local_dir`. `target/`,
    /// `.git/`, etc. are pruned EARLY so we never descend into huge artifact trees.
    ///
    /// `environment` defaults to the configured environment (or `"main"`).
    pub async fn mount_local_dir(
        &mut self,
        local_dir: impl AsRef<Path>,
        remote_path: &str,
        ignore: &[&str],
        environment: Option<&str>,
    ) -> Result<String> {
        let local_dir = local_dir.as_ref();
        if !local_dir.is_dir() {
            return Err(Error::invalid(format!(
                "mount_local_dir: '{}' is not a directory",
                local_dir.display()
            )));
        }
        let environment_name = self.env_or_default(environment);
        let matcher = IgnoreMatcher::new(ignore);

        let files = collect_files(local_dir, remote_path, &matcher)?;
        if files.is_empty() {
            return Err(Error::invalid(format!(
                "mount_local_dir: no files to upload under '{}' after applying ignore patterns",
                local_dir.display()
            )));
        }

        let mount_files = self.upload_files(files).await?;

        let resp = self
            .inner_mut()
            .mount_get_or_create(MountGetOrCreateRequest {
                deployment_name: String::new(),
                namespace: DeploymentNamespace::Workspace as i32,
                environment_name,
                object_creation_type: ObjectCreationType::Ephemeral as i32,
                files: mount_files,
                // EPHEMERAL needs no app_id. TODO(fallback): if the server ever
                // rejects EPHEMERAL for use in Function.mount_ids, switch to
                // ANONYMOUS_OWNED_BY_APP (=4) with app_id set by the facade.
                app_id: String::new(),
            })
            .await?
            .into_inner();

        if resp.mount_id.is_empty() {
            return Err(Error::build(
                "MountGetOrCreate returned an empty mount_id for the uploaded local directory",
            ));
        }
        Ok(resp.mount_id)
    }

    /// Hash, dedup, and upload each [`LocalFile`], returning the assembled
    /// [`MountFile`] descriptors sorted by filename (deterministic mount build).
    async fn upload_files(&mut self, files: Vec<LocalFile>) -> Result<Vec<MountFile>> {
        let mut accounted_sha: HashSet<String> = HashSet::new();
        let mut seen_paths: HashSet<String> = HashSet::new();
        let mut mount_files: Vec<MountFile> = Vec::with_capacity(files.len());

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
                self.ensure_file_uploaded(&sha256_hex, &file.data).await?;
            }

            mount_files.push(MountFile {
                filename: file.mount_filename,
                sha256_hex,
                size: Some(file.data.len() as u64),
                mode: file.mode,
            });
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
        self.inner_mut()
            .mount_put_file(MountPutFileRequest {
                sha256_hex: sha256.to_string(),
                data_oneof: Some(data_oneof),
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
        Ok(self
            .inner_mut()
            .mount_put_file(MountPutFileRequest {
                sha256_hex: sha256.to_string(),
                data_oneof: None,
            })
            .await?
            .into_inner()
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

/// Normalize the remote mount prefix: strip a trailing `/` so joins never produce
/// a `//`. A bare `/` (mount at root) normalizes to the empty string.
fn normalize_remote_prefix(remote_path: &str) -> String {
    let trimmed = remote_path.trim_end_matches('/');
    trimmed.to_string()
}

/// Walk `local_dir`, prune ignored directories early, and collect the surviving
/// files as [`LocalFile`] with `<remote_prefix>/<rel-as-posix>` paths.
///
/// `remote_prefix` is normalized defensively (trailing slashes stripped) so a
/// root mount passed as `"/"` yields `/<rel>` rather than `//<rel>`, regardless
/// of whether the caller pre-normalized.
fn collect_files(
    local_dir: &Path,
    remote_prefix: &str,
    matcher: &IgnoreMatcher,
) -> Result<Vec<LocalFile>> {
    let remote_prefix = normalize_remote_prefix(remote_prefix);
    let remote_prefix = remote_prefix.as_str();
    let mut files = Vec::new();
    let walker = WalkDir::new(local_dir).into_iter().filter_entry(|entry| {
        // Always keep the root itself; prune any descendant dir/file whose
        // RELATIVE path is ignored (directory pruning stops descent — critical
        // for `target/`, which can hold tens of thousands of files).
        match entry.path().strip_prefix(local_dir) {
            Ok(rel) if rel.as_os_str().is_empty() => true,
            Ok(rel) => !matcher.is_ignored(rel),
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
        let mount_filename = if remote_prefix.is_empty() {
            format!("/{rel_posix}")
        } else {
            format!("{remote_prefix}/{rel_posix}")
        };

        let data = std::fs::read(entry.path()).map_err(|e| {
            Error::build(format!("reading '{}' failed: {e}", entry.path().display()))
        })?;
        let mode = file_mode(entry.path());

        files.push(LocalFile {
            mount_filename,
            data,
            mode,
        });
    }

    Ok(files)
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

/// The narrow ignore-pattern matcher: bare path segments (pruned anywhere in the
/// path) and extension globs (`*.rlib`, `**/*.rlib`). NOT a full gitignore engine.
struct IgnoreMatcher {
    /// Bare segments like `target`, `.git`, `.modal-rust` — ignore any path whose
    /// components contain one of these.
    segments: Vec<String>,
    /// Extensions like `rlib` (from `*.rlib` / `**/*.rlib`) — ignore any file
    /// whose name ends in `.<ext>`.
    extensions: Vec<String>,
}

impl IgnoreMatcher {
    fn new(patterns: &[&str]) -> Self {
        let mut segments = Vec::new();
        let mut extensions = Vec::new();
        for &pat in patterns {
            if let Some(ext) = pat.strip_prefix("**/*.").or_else(|| pat.strip_prefix("*.")) {
                extensions.push(ext.to_string());
            } else {
                // Bare segment (possibly with surrounding slashes trimmed).
                let seg = pat.trim_matches('/');
                if !seg.is_empty() {
                    segments.push(seg.to_string());
                }
            }
        }
        Self {
            segments,
            extensions,
        }
    }

    /// Is the path RELATIVE to the mount root ignored?
    fn is_ignored(&self, rel: &Path) -> bool {
        // Bare-segment match: any component equal to a configured segment.
        for component in rel.components() {
            if let Some(name) = component.as_os_str().to_str() {
                if self.segments.iter().any(|s| s == name) {
                    return true;
                }
            }
        }
        // Extension match against the final component (the file name).
        if let Some(name) = rel.file_name().and_then(|n| n.to_str()) {
            for ext in &self.extensions {
                if name.len() > ext.len() + 1
                    && name.ends_with(ext.as_str())
                    && name.as_bytes()[name.len() - ext.len() - 1] == b'.'
                {
                    return true;
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn sha256_hex_is_lowercase_digest() {
        assert_eq!(
            sha256_hex(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn ignore_matcher_prunes_bare_segments() {
        let m = IgnoreMatcher::new(&["target", ".git", ".modal-rust", "**/*.rlib"]);
        assert!(m.is_ignored(&p("target")));
        assert!(m.is_ignored(&p("target/release/foo")));
        assert!(m.is_ignored(&p("crates/x/target/debug")));
        assert!(m.is_ignored(&p(".git/config")));
        assert!(m.is_ignored(&p(".modal-rust/cache")));
    }

    #[test]
    fn ignore_matcher_drops_extension_globs() {
        let m = IgnoreMatcher::new(&["**/*.rlib"]);
        assert!(m.is_ignored(&p("deps/libfoo.rlib")));
        assert!(m.is_ignored(&p("libfoo.rlib")));
        // Bare `*.ext` form also works.
        let m2 = IgnoreMatcher::new(&["*.tmp"]);
        assert!(m2.is_ignored(&p("scratch/a.tmp")));
    }

    #[test]
    fn ignore_matcher_keeps_sources() {
        let m = IgnoreMatcher::new(&["target", ".git", ".modal-rust", "**/*.rlib"]);
        assert!(!m.is_ignored(&p("src/lib.rs")));
        assert!(!m.is_ignored(&p("Cargo.toml")));
        assert!(!m.is_ignored(&p("examples/add/src/bin/modal_runner.rs")));
        // A file literally named like a partial extension must not false-match.
        assert!(!m.is_ignored(&p("rlib")));
        assert!(!m.is_ignored(&p(".rlib")));
    }

    #[test]
    fn remote_prefix_normalizes_trailing_slash() {
        assert_eq!(normalize_remote_prefix("/src"), "/src");
        assert_eq!(normalize_remote_prefix("/src/"), "/src");
        assert_eq!(normalize_remote_prefix("/"), "");
    }

    #[test]
    fn collect_files_maps_to_posix_remote_paths() {
        let dir = tempdir_with_files();
        let m = IgnoreMatcher::new(&["target", ".git", "**/*.rlib"]);
        let mut files = collect_files(dir.path(), "/src", &m).unwrap();
        files.sort_by(|a, b| a.mount_filename.cmp(&b.mount_filename));
        let names: Vec<&str> = files.iter().map(|f| f.mount_filename.as_str()).collect();
        assert_eq!(
            names,
            vec!["/src/Cargo.toml", "/src/sub/main.rs"],
            "ignored target/ and *.rlib must be excluded; survivors use /src POSIX paths"
        );
    }

    #[test]
    fn collect_files_handles_root_mount_prefix() {
        let dir = tempdir_with_files();
        let m = IgnoreMatcher::new(&["target", "**/*.rlib"]);
        let files = collect_files(dir.path(), "/", &m).unwrap();
        assert!(files.iter().any(|f| f.mount_filename == "/Cargo.toml"));
        assert!(files.iter().all(|f| !f.mount_filename.starts_with("//")));
    }

    /// Build a small temp tree: Cargo.toml, sub/main.rs, target/junk (ignored),
    /// deps/foo.rlib (ignored). Returned guard removes it on drop. The name is
    /// unique per call (PID + atomic counter) so concurrently-run tests never
    /// share a directory and clobber each other's contents on drop.
    fn tempdir_with_files() -> TempTree {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!(
            "modal_rust_local_dir_test_{}_{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("sub")).unwrap();
        std::fs::create_dir_all(base.join("target/release")).unwrap();
        std::fs::create_dir_all(base.join("deps")).unwrap();
        std::fs::write(base.join("Cargo.toml"), b"[package]\n").unwrap();
        std::fs::write(base.join("sub/main.rs"), b"fn main() {}\n").unwrap();
        std::fs::write(base.join("target/release/junk"), b"junk").unwrap();
        std::fs::write(base.join("deps/foo.rlib"), b"rlib").unwrap();
        TempTree { path: base }
    }

    struct TempTree {
        path: PathBuf,
    }
    impl TempTree {
        fn path(&self) -> &Path {
            &self.path
        }
    }
    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
