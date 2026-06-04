//! Blob upload for large mount files: `BlobCreate` → HTTP `PUT`.
//!
//! Mount files at or above [`LARGE_FILE_BLOB_THRESHOLD`] (4 MiB) are not sent
//! inline in a `MountPutFile`; instead the client asks the control plane for a
//! presigned upload URL (`BlobCreate`), `PUT`s the bytes to object storage over
//! plain HTTPS (reqwest, rustls), then references the returned `blob_id` from the
//! `MountPutFile.data_blob_id` oneof. This mirrors Modal's
//! `_utils/blob_utils.py` and the modal-rs `blob_transfer.rs` precedent.
//!
//! Single-part only: multipart (`> ~1 GiB`) is rejected. The run path's source
//! files are tiny, so the blob branch is rarely hit, but it is correct for the
//! occasional large vendored asset.

use base64::Engine;
use sha2::{Digest, Sha256};

use crate::client::ModalClient;
use crate::error::{Error, Result};
use crate::proto::api::blob_create_response::UploadTypeOneof;
use crate::proto::api::BlobCreateRequest;

/// Files at or above this size go through the blob path instead of inline
/// `MountPutFile.data`. Matches Modal's `LARGE_FILE_LIMIT` (`blob_utils.py`) and
/// modal-rs `LARGE_FILE_BLOB_THRESHOLD_BYTES` (`blob_transfer.rs:8`).
pub const LARGE_FILE_BLOB_THRESHOLD: usize = 4 * 1024 * 1024;

impl ModalClient {
    /// Upload `data` as a single-part blob and return its `blob_id`.
    ///
    /// Steps (ported from modal-rs `upload_blob`, `blob_transfer.rs:20-66`):
    /// 1. `BlobCreate` with the base64 SHA-256 + content length (md5 omitted —
    ///    optional for the single-part sizes we use).
    /// 2. Read the **singular** `upload_type_oneof`: a presigned URL → `PUT` the
    ///    bytes (require a 2xx); a multipart plan → error (unsupported here).
    /// 3. Return `resp.blob_id`.
    pub(crate) async fn blob_create_and_put(&mut self, data: &[u8]) -> Result<String> {
        let sha256_b64 = base64::engine::general_purpose::STANDARD.encode(Sha256::digest(data));

        let resp = self
            .inner_mut()
            .blob_create(BlobCreateRequest {
                content_md5: String::new(),
                content_sha256_base64: sha256_b64,
                content_length: data.len() as i64,
            })
            .await?
            .into_inner();

        let blob_id = resp.blob_id.clone();
        match resp.upload_type_oneof {
            Some(UploadTypeOneof::UploadUrl(url)) => {
                put_blob_bytes(&url, data).await?;
                Ok(blob_id)
            }
            Some(UploadTypeOneof::Multipart(_)) => Err(Error::invalid(
                "multipart blob uploads not yet supported (file exceeds the single-part limit)",
            )),
            None => Err(Error::build(
                "BlobCreate response did not include an upload URL",
            )),
        }
    }
}

/// `PUT` `data` to a presigned URL with the octet-stream content type; require a
/// 2xx response. Uses a fresh reqwest client (rustls; no system OpenSSL).
async fn put_blob_bytes(url: &str, data: &[u8]) -> Result<()> {
    let http = reqwest::Client::new();
    let resp = http
        .put(url)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .body(data.to_vec())
        .send()
        .await
        .map_err(|e| Error::build(format!("blob upload PUT failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(Error::build(format!(
            "blob upload PUT returned non-success status {}",
            resp.status()
        )));
    }
    Ok(())
}
