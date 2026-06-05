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

use std::time::Instant;

use base64::Engine;
use sha2::{Digest, Sha256};

use crate::client::ModalClient;
use crate::error::{Error, Result};
use crate::proto::api::blob_create_response::UploadTypeOneof;
use crate::proto::api::BlobCreateRequest;
use crate::retry::{jitter, retry_unary, RetryPolicy};

/// Files at or above this size go through the blob path instead of inline
/// `MountPutFile.data`. Matches Modal's `LARGE_FILE_LIMIT` (`blob_utils.py`) and
/// modal-rs `LARGE_FILE_BLOB_THRESHOLD_BYTES` (`blob_transfer.rs:8`).
pub const LARGE_FILE_BLOB_THRESHOLD: usize = 4 * 1024 * 1024;

/// Build the `BlobCreate` request (api.proto) — pure, no I/O.
///
/// Computes the content-addressed base64 SHA-256 + content length INTERNALLY (the
/// side-effecting presigned-URL `PUT` stays in [`ModalClient::blob_create_and_put`]).
/// `content_md5` is omitted (optional for the single-part sizes we use), matching
/// the inline literal it replaces.
pub fn build_blob_create_request(data: &[u8]) -> BlobCreateRequest {
    let content_sha256_base64 =
        base64::engine::general_purpose::STANDARD.encode(Sha256::digest(data));
    BlobCreateRequest {
        content_md5: String::new(),
        content_sha256_base64,
        content_length: data.len() as i64,
    }
}

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
        // BlobCreate returns a presigned URL for the content-addressed sha;
        // re-requesting is safe (a new URL for the same content), so retry on a
        // transient reset.
        let req = build_blob_create_request(data);
        let stub = self.stub();
        let resp = retry_unary("blob_create", || {
            let mut stub = stub.clone();
            let req = req.clone();
            async move { Ok(stub.blob_create(req).await?.into_inner()) }
        })
        .await?;

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
///
/// This is a plain object-store PUT (not gRPC), so `Error::is_transient` does not
/// classify it. We add our own small retry with the same backoff shape as the
/// control-plane unary policy: a reqwest timeout/connect error OR a `5xx`/`429`
/// response is transient (the PUT is an idempotent same-key/same-bytes upload);
/// any other `4xx` is terminal and surfaces immediately.
async fn put_blob_bytes(url: &str, data: &[u8]) -> Result<()> {
    let policy = RetryPolicy::default();
    let http = reqwest::Client::new();
    let start = Instant::now();
    let mut delay = policy.base_delay;
    let mut attempt = 1u32;

    loop {
        let outcome = http
            .put(url)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(data.to_vec())
            .send()
            .await;

        // Decide: Ok(()) on 2xx, Err(terminal) on a non-retryable failure, or
        // Err(transient) flagged for retry.
        let (err, transient): (Error, bool) = match outcome {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    return Ok(());
                }
                let retryable =
                    status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
                (
                    Error::build(format!(
                        "blob upload PUT returned non-success status {status}"
                    )),
                    retryable,
                )
            }
            Err(e) => {
                let retryable = e.is_timeout() || e.is_connect();
                (
                    Error::build(format!("blob upload PUT failed: {e}")),
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
            "[retry] blob_put attempt {attempt}/{} after transient: {err}",
            policy.max_attempts
        );
        // Full jitter over [0, delay] (shared with the gRPC retry helper).
        tokio::time::sleep(jitter(delay)).await;
        delay = delay.mul_f64(policy.delay_factor).min(policy.max_delay);
        attempt += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_blob_create_carries_length_and_b64_sha() {
        let data = b"hello blob";
        let req = build_blob_create_request(data);
        // content_length == data.len().
        assert_eq!(req.content_length, data.len() as i64);
        // content_sha256_base64 is the base64 SHA-256 of the bytes.
        let want = base64::engine::general_purpose::STANDARD.encode(Sha256::digest(data));
        assert_eq!(req.content_sha256_base64, want);
        // md5 is omitted for the single-part path.
        assert!(req.content_md5.is_empty(), "content_md5 stays empty");
    }

    #[test]
    fn build_blob_create_empty_bytes() {
        let req = build_blob_create_request(&[]);
        assert_eq!(req.content_length, 0);
        // SHA-256 of the empty string, base64-encoded.
        assert_eq!(
            req.content_sha256_base64,
            "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
        );
    }
}
