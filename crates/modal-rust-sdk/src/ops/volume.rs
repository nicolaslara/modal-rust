//! Volume resolution — `VolumeGetOrCreate`.
//!
//! Used by the P6 cargo build cache: resolve a persistent V2 volume by name
//! (create-if-missing) and return its `volume_id`, for attaching to a function
//! via `FunctionSpec.volume_mounts` -> `Function.volume_mounts`.
//!
//! Mirrors `Volume.from_name(..., create_if_missing, version)` (volume.py):
//! sets ONLY deployment_name, environment_name, object_creation_type, version.
//! Leaves the reserved `namespace` (not generated) and `app_id` UNSET.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Instant;

use sha2::{Digest, Sha256};

use crate::client::ModalClient;
use crate::error::{Error, Result};
use crate::proto::api::volume_put_files2_request::{Block, File as PutFile};
use crate::proto::api::{
    ObjectCreationType, VolumeFsVersion, VolumeGetOrCreateRequest, VolumePutFiles2Request,
};
use crate::retry::{jitter, RetryPolicy};

/// V2 block size: files are split into 8 MiB content-addressed blocks. Matches
/// Modal's `BLOCK_SIZE` (`_utils/blob_utils.py`). Block-based upload streams each
/// chunk independently, so multi-GB files never need to be held in memory at once.
pub const VOLUME_BLOCK_SIZE: u64 = 8 * 1024 * 1024;

/// Build the `VolumeGetOrCreate` request — pure, no I/O. Mirrors
/// `Volume.from_name(..., create_if_missing, version)`: sets ONLY `deployment_name`,
/// `environment_name`, `object_creation_type`, and `version`.
///
/// - `v2 = true` ⇒ `VolumeFsVersion::V2` (concurrent writes — the cargo cache);
///   `false` ⇒ `Unspecified` (server default == V1; the user-volume path).
/// - `create_if_missing = true` ⇒ `CreateIfMissing` (idempotent); `false` ⇒
///   `Unspecified` (pure lookup).
///
/// Extracted from [`ModalClient::volume_get_or_create`]; the method passes the
/// resolved `environment_name`.
pub(crate) fn build_volume_get_or_create_request(
    name: &str,
    v2: bool,
    create_if_missing: bool,
    environment_name: String,
) -> VolumeGetOrCreateRequest {
    let version = if v2 {
        VolumeFsVersion::V2 as i32
    } else {
        VolumeFsVersion::Unspecified as i32 // == Python version=None
    };
    let object_creation_type = if create_if_missing {
        ObjectCreationType::CreateIfMissing as i32
    } else {
        ObjectCreationType::Unspecified as i32 // pure lookup
    };
    VolumeGetOrCreateRequest {
        deployment_name: name.to_string(),
        environment_name,
        object_creation_type,
        version,
        ..Default::default() // app_id empty; reserved namespace never set
    }
}

impl ModalClient {
    /// Resolve a persistent Volume by deployment name, creating it if missing,
    /// and return its `volume_id`.
    ///
    /// - `name`: deployment name (e.g. `"modal-rust-cargo-cache"`).
    /// - `v2`: `true` => `VolumeFsVersion::V2` (concurrent writes — required for
    ///   the cargo cache); `false` => `VolumeFsVersion::Unspecified` (server
    ///   default == V1; matches Python `version=None`).
    /// - `create_if_missing`: `true` => `ObjectCreationType::CreateIfMissing`
    ///   (idempotent, retry-safe); `false` => `Unspecified` (pure lookup).
    /// - `environment`: defaults to the configured environment (or `"main"`).
    pub async fn volume_get_or_create(
        &mut self,
        name: &str,
        v2: bool,
        create_if_missing: bool,
        environment: Option<&str>,
    ) -> Result<String> {
        let environment_name = self.env_or_default(environment);
        let req = build_volume_get_or_create_request(name, v2, create_if_missing, environment_name);
        // CREATE_IF_MISSING is idempotent server-side, so a retry after a dropped
        // response re-resolves the same volume_id (mirrors mount_get_or_create).
        let resp = self
            .retry_rpc("volume_get_or_create", req, |mut stub, req| async move {
                stub.volume_get_or_create(req).await
            })
            .await?;

        if resp.volume_id.is_empty() {
            return Err(Error::build(format!(
                "VolumeGetOrCreate for '{name}' returned an empty volume_id"
            )));
        }
        Ok(resp.volume_id)
    }
}

/// One local file selected for upload, with its resolved remote (POSIX) path and
/// Unix mode bits. Produced by [`plan_upload`] from a local file or directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    /// Absolute (or cwd-relative) local source path.
    pub local: PathBuf,
    /// Destination path inside the volume — always forward-slash POSIX, never
    /// leading `/`, never `.`/`..` components.
    pub remote: String,
    /// Unix permission bits (`st_mode & 0o7777`).
    pub mode: u32,
}

/// Result summary of a [`ModalClient::volume_put`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumePutStats {
    /// Number of files uploaded (the full set declared, whether or not their
    /// blocks already existed server-side).
    pub files: usize,
    /// Total bytes across all declared files.
    pub bytes: u64,
    /// Number of distinct blocks actually PUT to object storage (deduplicated by
    /// content sha; pre-existing blocks are not re-uploaded).
    pub blocks_uploaded: usize,
}

/// Normalise a remote destination into a clean POSIX path component string.
///
/// Strips a leading `/`, rejects `.`/`..` traversal, and rejects empty/`/`-only
/// paths. Mirrors the Python client's `PurePosixPath(remote_path).as_posix()`
/// handling plus the "must refer to a file" guard.
fn normalize_remote(remote: &str) -> Result<String> {
    let trimmed = remote.trim_start_matches('/');
    if trimmed.is_empty() || trimmed.ends_with('/') {
        return Err(Error::invalid(format!(
            "remote path '{remote}' must refer to a file (cannot be empty or end with '/')"
        )));
    }
    if trimmed
        .split('/')
        .any(|c| c.is_empty() || c == "." || c == "..")
    {
        return Err(Error::invalid(format!(
            "remote path '{remote}' must not contain empty, '.' or '..' components"
        )));
    }
    Ok(trimmed.to_string())
}

/// Read the Unix mode bits of `path` (`st_mode & 0o7777`); falls back to `0o644`
/// on platforms / metadata where the mode is unavailable.
fn file_mode(meta: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o7777
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        0o644
    }
}

/// Plan an upload: expand a local file OR directory into the concrete set of
/// [`PlannedFile`]s with their remote paths.
///
/// - LOCAL FILE: the single file maps to `remote_path` (a trailing `/` on the
///   remote means "into this dir under the local basename", matching the CLI).
/// - LOCAL DIR: every regular file under it (recursively) maps to
///   `remote_path/<relpath>`. Directories and non-regular files are skipped.
///
/// Pure except for the filesystem walk (no network). Sorted for deterministic
/// request ordering / testability.
pub fn plan_upload(local_path: &Path, remote_path: &str) -> Result<Vec<PlannedFile>> {
    let meta = std::fs::metadata(local_path)
        .map_err(|e| Error::invalid(format!("cannot stat '{}': {e}", local_path.display())))?;

    if meta.is_file() {
        // A trailing-slash remote means "place under this directory by basename".
        let remote = if remote_path.is_empty() || remote_path.ends_with('/') {
            let base = local_path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| {
                    Error::invalid(format!("local file '{}' has no name", local_path.display()))
                })?;
            format!("{}{base}", remote_path)
        } else {
            remote_path.to_string()
        };
        return Ok(vec![PlannedFile {
            local: local_path.to_path_buf(),
            remote: normalize_remote(&remote)?,
            mode: file_mode(&meta),
        }]);
    }

    if !meta.is_dir() {
        return Err(Error::invalid(format!(
            "'{}' is neither a regular file nor a directory",
            local_path.display()
        )));
    }

    // Directory: the remote prefix is normalised once; trailing slash is optional.
    let prefix = remote_path.trim_start_matches('/').trim_end_matches('/');
    let mut out = Vec::new();
    walk_dir(local_path, local_path, prefix, &mut out)?;
    out.sort_by(|a, b| a.remote.cmp(&b.remote));
    Ok(out)
}

/// Recursive directory walk used by [`plan_upload`]. Appends one [`PlannedFile`]
/// per regular file; skips symlinked dirs implicitly (only descends real dirs).
fn walk_dir(root: &Path, dir: &Path, prefix: &str, out: &mut Vec<PlannedFile>) -> Result<()> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| Error::invalid(format!("cannot read dir '{}': {e}", dir.display())))?;
    for entry in entries {
        let entry =
            entry.map_err(|e| Error::invalid(format!("dir entry in '{}': {e}", dir.display())))?;
        let path = entry.path();
        let meta = entry
            .metadata()
            .map_err(|e| Error::invalid(format!("cannot stat '{}': {e}", path.display())))?;
        if meta.is_dir() {
            walk_dir(root, &path, prefix, out)?;
        } else if meta.is_file() {
            let rel = path.strip_prefix(root).map_err(|_| {
                Error::invalid(format!("'{}' escaped the upload root", path.display()))
            })?;
            // POSIX-ify the relative path on every platform.
            let rel_posix = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let remote = if prefix.is_empty() {
                rel_posix
            } else {
                format!("{prefix}/{rel_posix}")
            };
            out.push(PlannedFile {
                local: path.clone(),
                remote: normalize_remote(&remote)?,
                mode: file_mode(&meta),
            });
        }
        // else: non-regular (fifo/socket/device) — skip, like the Python client.
    }
    Ok(())
}

/// SHA-256 a single 8 MiB block read from `file` at `[start, start+len)`.
/// Returns the raw 32-byte digest (the wire form for `Block.contents_sha256`).
fn hash_block(file: &mut std::fs::File, start: u64, len: usize) -> Result<(Vec<u8>, Vec<u8>)> {
    file.seek(SeekFrom::Start(start))
        .map_err(|e| Error::build(format!("seek failed: {e}")))?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf)
        .map_err(|e| Error::build(format!("read failed: {e}")))?;
    let digest = Sha256::digest(&buf).to_vec();
    Ok((digest, buf))
}

impl ModalClient {
    /// Upload a LOCAL file or directory into a resolved Volume, using the V2
    /// block-based protocol (`VolumePutFiles2`).
    ///
    /// Chosen over the V1 `VolumePutFiles` + blob path because correctly handling
    /// multi-GB weights on V1 requires MULTIPART blob upload (files > 1 GiB), which
    /// our [`blob`](crate::ops::blob) module deliberately does not support. The V2
    /// protocol streams 8 MiB content-addressed blocks straight to object storage
    /// via HTTP `PUT` with a server-driven missing-block recovery loop — no
    /// multipart, no whole-file buffering, idempotent on retry. This mirrors what
    /// the official client does for large files (`_VolumeUploadContextManager2`).
    ///
    /// - `volume_id`: a resolved volume id (see [`volume_get_or_create`]).
    /// - `files`: the planned set (from [`plan_upload`]).
    /// - `force`: when `false`, the server rejects overwriting an existing remote
    ///   file with `ALREADY_EXISTS` (surfaced as a terminal [`Error::Status`]);
    ///   `true` sets `disallow_overwrite_existing_files = false`.
    /// - `on_file`: optional per-file progress callback `(remote_path, size)`,
    ///   invoked once per declared file as its metadata is hashed.
    ///
    /// [`volume_get_or_create`]: ModalClient::volume_get_or_create
    pub async fn volume_put(
        &mut self,
        volume_id: &str,
        files: &[PlannedFile],
        force: bool,
        mut on_file: impl FnMut(&str, u64),
    ) -> Result<VolumePutStats> {
        if files.is_empty() {
            return Err(Error::invalid("no files to upload"));
        }

        // (1) Hash every file into its 8 MiB blocks. We keep an open File handle per
        // file (re-seeked for missing-block reads) but never hold all bytes in RAM.
        struct LocalFile {
            handle: std::fs::File,
            size: u64,
            remote: String,
            mode: u32,
            // (block_start, block_len, raw_sha256) per block, in order.
            blocks: Vec<(u64, usize, Vec<u8>)>,
        }
        let mut locals: Vec<LocalFile> = Vec::with_capacity(files.len());
        let mut total_bytes = 0u64;
        for pf in files {
            let mut handle = std::fs::File::open(&pf.local)
                .map_err(|e| Error::build(format!("cannot open '{}': {e}", pf.local.display())))?;
            let size = handle
                .metadata()
                .map_err(|e| Error::build(format!("cannot stat '{}': {e}", pf.local.display())))?
                .len();
            total_bytes += size;
            on_file(&pf.remote, size);

            let mut blocks = Vec::new();
            let mut start = 0u64;
            while start < size {
                let len = std::cmp::min(VOLUME_BLOCK_SIZE, size - start) as usize;
                let (digest, _) = hash_block(&mut handle, start, len)?;
                blocks.push((start, len, digest));
                start += len as u64;
            }
            // A zero-byte file has no blocks — the server materialises it from `size`.
            locals.push(LocalFile {
                handle,
                size,
                remote: pf.remote.clone(),
                mode: pf.mode,
                blocks,
            });
        }

        // (2) Up to two VolumePutFiles2 rounds: round 1 declares all blocks; the
        // server returns missing blocks; round 2 re-declares them with the captured
        // HTTP PUT response bytes. A second non-empty missing set is an error.
        // `put_responses` is keyed by raw block sha (content-addressed dedup).
        let mut put_responses: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();
        let mut blocks_uploaded = 0usize;

        for round in 0..2 {
            let request_files: Vec<PutFile> = locals
                .iter()
                .map(|lf| PutFile {
                    path: lf.remote.clone(),
                    size: lf.size,
                    mode: Some(lf.mode),
                    blocks: lf
                        .blocks
                        .iter()
                        .map(|(_, _, sha)| Block {
                            contents_sha256: sha.clone(),
                            put_response: put_responses.get(sha).cloned(),
                        })
                        .collect(),
                })
                .collect();

            let req = VolumePutFiles2Request {
                volume_id: volume_id.to_string(),
                files: request_files,
                disallow_overwrite_existing_files: !force,
            };

            let resp = self
                .retry_rpc("volume_put_files2", req, |mut stub, req| async move {
                    stub.volume_put_files2(req).await
                })
                .await?;

            if resp.missing_blocks.is_empty() {
                return Ok(VolumePutStats {
                    files: files.len(),
                    bytes: total_bytes,
                    blocks_uploaded,
                });
            }
            if round == 1 {
                return Err(Error::build(
                    "volume put did not converge: server still reports missing blocks after \
                     re-uploading them",
                ));
            }

            // (3) PUT each missing block's bytes to its presigned URL; capture the
            // response body to echo back as `put_response` next round.
            for mb in &resp.missing_blocks {
                let lf = locals.get(mb.file_index as usize).ok_or_else(|| {
                    Error::build(format!(
                        "server returned out-of-range file_index {}",
                        mb.file_index
                    ))
                })?;
                let (start, len, sha) = lf
                    .blocks
                    .get(mb.block_index as usize)
                    .ok_or_else(|| {
                        Error::build(format!(
                            "server returned out-of-range block_index {} for file {}",
                            mb.block_index, mb.file_index
                        ))
                    })?
                    .clone();
                if put_responses.contains_key(&sha) {
                    continue; // same content already PUT in this round (dedup).
                }
                // Read the block fresh (independent handle to avoid cursor races).
                let mut h = lf
                    .handle
                    .try_clone()
                    .map_err(|e| Error::build(format!("clone handle: {e}")))?;
                let (_, bytes) = hash_block(&mut h, start, len)?;
                let resp_body = put_block_bytes(&mb.put_url, bytes).await?;
                put_responses.insert(sha, resp_body);
                blocks_uploaded += 1;
            }
        }

        unreachable!("volume put loop always returns within two rounds")
    }
}

/// `PUT` a block's bytes to its presigned `put_url`; return the response body
/// bytes (echoed back to the server as `Block.put_response`). Same transient
/// retry shape as [`crate::ops::blob`]'s blob PUT: a 5xx/429 or a
/// timeout/connect error retries the idempotent same-bytes upload; any other
/// 4xx is terminal.
async fn put_block_bytes(url: &str, data: Vec<u8>) -> Result<Vec<u8>> {
    let policy = RetryPolicy::default();
    let http = reqwest::Client::new();
    let start = Instant::now();
    let mut delay = policy.base_delay;
    let mut attempt = 1u32;

    loop {
        let outcome = http
            .put(url)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(data.clone())
            .send()
            .await;

        let (err, transient): (Error, bool) = match outcome {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    let body = resp.bytes().await.map_err(|e| {
                        Error::build(format!("block PUT response read failed: {e}"))
                    })?;
                    return Ok(body.to_vec());
                }
                let retryable =
                    status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
                (
                    Error::build(format!(
                        "block upload PUT returned non-success status {status}"
                    )),
                    retryable,
                )
            }
            Err(e) => {
                let retryable = e.is_timeout() || e.is_connect();
                (
                    Error::build(format!("block upload PUT failed: {e}")),
                    retryable,
                )
            }
        };

        let last_attempt = attempt >= policy.max_attempts;
        let over_deadline = start.elapsed() + delay >= policy.total_deadline;
        if !transient || last_attempt || over_deadline {
            return Err(err);
        }
        eprintln!(
            "[retry] volume_block_put attempt {attempt}/{} after transient: {err}",
            policy.max_attempts
        );
        tokio::time::sleep(jitter(delay)).await;
        delay = delay.mul_f64(policy.delay_factor).min(policy.max_delay);
        attempt += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::api::{ObjectCreationType, VolumeFsVersion};

    #[test]
    fn version_flag_maps_to_fs_version() {
        assert_eq!(VolumeFsVersion::V2 as i32, 2);
        assert_eq!(VolumeFsVersion::Unspecified as i32, 0);
    }

    #[test]
    fn create_flag_maps_to_creation_type() {
        assert_eq!(ObjectCreationType::CreateIfMissing as i32, 1);
        assert_eq!(ObjectCreationType::Unspecified as i32, 0);
    }

    #[test]
    fn build_volume_get_or_create_v2_create() {
        // The cargo-cache path: V2 + create-if-missing.
        let req = build_volume_get_or_create_request(
            "modal-rust-cargo-cache",
            true,
            true,
            "main".to_string(),
        );
        assert_eq!(req.deployment_name, "modal-rust-cargo-cache");
        assert_eq!(req.environment_name, "main");
        assert_eq!(req.version, VolumeFsVersion::V2 as i32);
        assert_eq!(
            req.object_creation_type,
            ObjectCreationType::CreateIfMissing as i32
        );
    }

    #[test]
    fn build_volume_get_or_create_v1_create() {
        // The user-volume path: V1 (Unspecified == server default) + create.
        let req = build_volume_get_or_create_request("my-vol", false, true, "main".to_string());
        assert_eq!(req.version, VolumeFsVersion::Unspecified as i32);
        assert_eq!(
            req.object_creation_type,
            ObjectCreationType::CreateIfMissing as i32
        );
    }

    use std::io::Write;

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "mr-vol-put-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_file(path: &Path, bytes: &[u8]) {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).unwrap();
        }
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(bytes).unwrap();
    }

    #[test]
    fn normalize_remote_strips_leading_slash() {
        assert_eq!(normalize_remote("/a/b.txt").unwrap(), "a/b.txt");
        assert_eq!(normalize_remote("a/b.txt").unwrap(), "a/b.txt");
    }

    #[test]
    fn normalize_remote_rejects_dir_and_traversal() {
        assert!(normalize_remote("").is_err());
        assert!(normalize_remote("/").is_err());
        assert!(normalize_remote("a/").is_err());
        assert!(normalize_remote("a/../b").is_err());
        assert!(normalize_remote("./b").is_err());
    }

    #[test]
    fn plan_single_file_explicit_remote() {
        let d = tmpdir();
        let f = d.join("weights.bin");
        write_file(&f, b"hello");
        let plan = plan_upload(&f, "models/w.bin").unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].remote, "models/w.bin");
        assert_eq!(plan[0].local, f);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn plan_single_file_trailing_slash_uses_basename() {
        let d = tmpdir();
        let f = d.join("weights.bin");
        write_file(&f, b"hello");
        // Trailing slash => place under the dir by basename (matches `modal volume put`).
        let plan = plan_upload(&f, "models/").unwrap();
        assert_eq!(plan[0].remote, "models/weights.bin");
        // Empty remote => root by basename.
        let plan2 = plan_upload(&f, "").unwrap();
        assert_eq!(plan2[0].remote, "weights.bin");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn plan_directory_maps_recursively_sorted() {
        let d = tmpdir();
        write_file(&d.join("a.txt"), b"a");
        write_file(&d.join("sub/b.txt"), b"bb");
        write_file(&d.join("sub/deep/c.txt"), b"ccc");
        let plan = plan_upload(&d, "dst").unwrap();
        let remotes: Vec<&str> = plan.iter().map(|p| p.remote.as_str()).collect();
        assert_eq!(
            remotes,
            vec!["dst/a.txt", "dst/sub/b.txt", "dst/sub/deep/c.txt"]
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn plan_directory_empty_prefix_is_root() {
        let d = tmpdir();
        write_file(&d.join("a.txt"), b"a");
        let plan = plan_upload(&d, "/").unwrap();
        assert_eq!(plan[0].remote, "a.txt");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn hash_block_matches_known_sha256() {
        let d = tmpdir();
        let f = d.join("x");
        write_file(&f, b"hello");
        let mut fh = std::fs::File::open(&f).unwrap();
        let (digest, bytes) = hash_block(&mut fh, 0, 5).unwrap();
        assert_eq!(bytes, b"hello");
        // Known SHA-256("hello").
        assert_eq!(
            hex_lower(&digest),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn block_count_for_multi_block_file() {
        // 8 MiB block size: a 20 MiB file => 3 blocks (8 + 8 + 4).
        let size: u64 = 20 * 1024 * 1024;
        let mut n = 0u64;
        let mut start = 0u64;
        while start < size {
            let len = std::cmp::min(VOLUME_BLOCK_SIZE, size - start);
            n += 1;
            start += len;
        }
        assert_eq!(n, 3);
        assert_eq!(VOLUME_BLOCK_SIZE, 8 * 1024 * 1024);
    }

    fn hex_lower(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }
}
