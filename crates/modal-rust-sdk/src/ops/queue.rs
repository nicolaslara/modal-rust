//! modal.Queue control-plane ops — the v0 unary RPC surface.
//!
//! Mirrors `modal/queue.py` over the Queue RPCs (api.proto ~4284; messages
//! ~2848–2950). Values are **opaque bytes at this layer** — the server never
//! interprets them (no wire-level format field). Encoding is the CALLER's
//! concern: the facade encodes via [`crate::pickle`]; raw-byte users ride these
//! methods directly.
//!
//! v0 surface (all unary, all through `retry_rpc`):
//!
//! - `QueueGetOrCreate` — named resolve (`CREATE_IF_MISSING`) or pure lookup
//!   (`UNSPECIFIED`).
//! - `QueuePut` — `values` is `repeated bytes`; put / put_many are the SAME RPC
//!   (there is no QueuePutMany). v0 always sends the default partition (empty
//!   `partition_key`) and Python's explicit `partition_ttl` default.
//! - `QueueGet` — server-side blocking `timeout` + `n_values` batching are
//!   native to the wire, but a long block never rides one gRPC deadline:
//!   [`ModalClient::queue_get_blocking`] caps each RPC at
//!   [`QUEUE_GET_PER_RPC_TIMEOUT_SECS`] and loops client-side, mirroring
//!   `Queue._get_blocking` (design doc §3.4).
//! - `QueueLen` / `QueueClear` / `QueueDelete`.
//!
//! Deferred (design doc §4): `QueueNextItems` (non-destructive iteration
//! cursor), `QueueHeartbeat` (ephemeral lifecycle), partition parameters on the
//! public surface (the builders carry them internally from day one so v1 is
//! additive), `QueueGetById`/`QueueList`.
//!
//! Limits to surface in facade rustdoc: 100,000 partitions × 5,000 items;
//! 1 MiB per item; `QueueLen` is `int32` on the wire (widened to `u64`).

use std::time::{Duration, Instant};

use crate::client::ModalClient;
use crate::error::{Error, Result};
use crate::proto::api::{
    ObjectCreationType, QueueClearRequest, QueueDeleteRequest, QueueGetOrCreateRequest,
    QueueGetRequest, QueueLenRequest, QueuePutRequest,
};

/// Default partition TTL sent on every `QueuePut`, in seconds. Python sends an
/// EXPLICIT default — `partition_ttl: int = 24 * 3600` (queue.py:549, pinned
/// 2026-06-11) — so we must send the same explicit value, never 0.
pub(crate) const DEFAULT_PARTITION_TTL_SECONDS: i32 = 24 * 3600;

/// Per-RPC cap on the server-side blocking `QueueGet` timeout, in seconds.
/// Python: `request_timeout = 50.0  # We prevent longer ones in order to keep
/// the connection alive` (queue.py:448, pinned 2026-06-11). The blocking-get
/// poll loop caps every RPC at this and loops until the caller's deadline.
pub(crate) const QUEUE_GET_PER_RPC_TIMEOUT_SECS: f32 = 50.0;

/// Build the `QueueGetOrCreate` request — pure, no I/O. Mirrors
/// `Queue.from_name` (`CREATE_IF_MISSING`, idempotent + retry-safe) and
/// `Queue.lookup` (`UNSPECIFIED` — pure lookup; the server errors if missing).
pub(crate) fn build_queue_get_or_create_request(
    name: &str,
    environment_name: String,
    creation_type: ObjectCreationType,
) -> QueueGetOrCreateRequest {
    QueueGetOrCreateRequest {
        deployment_name: name.to_string(),
        environment_name,
        object_creation_type: creation_type as i32,
    }
}

/// Build the `QueuePut` request — pure, no I/O. `values` are caller-encoded
/// byte items (`repeated bytes` — put_many is the same RPC). v0 callers pass
/// the default partition (`partition_key = b""`) and
/// [`DEFAULT_PARTITION_TTL_SECONDS`]; the parameters exist so exposing
/// partitions later is purely additive.
pub(crate) fn build_queue_put_request(
    queue_id: &str,
    values: Vec<Vec<u8>>,
    partition_key: &[u8],
    partition_ttl_seconds: i32,
) -> QueuePutRequest {
    QueuePutRequest {
        queue_id: queue_id.to_string(),
        values,
        partition_key: partition_key.to_vec(),
        partition_ttl_seconds,
    }
}

/// Build one `QueueGet` request — pure, no I/O. `timeout_secs` is the
/// SERVER-side blocking window for this single RPC (callers cap it at
/// [`QUEUE_GET_PER_RPC_TIMEOUT_SECS`]; `0.0` = non-blocking poll). `n_values`
/// is clamped to ≥ 1 (the wire batching: `get_many` = `n_values > 1`).
pub(crate) fn build_queue_get_request(
    queue_id: &str,
    n_values: u32,
    timeout_secs: f32,
    partition_key: &[u8],
) -> QueueGetRequest {
    QueueGetRequest {
        queue_id: queue_id.to_string(),
        timeout: timeout_secs,
        n_values: i32::try_from(n_values.max(1)).unwrap_or(i32::MAX),
        partition_key: partition_key.to_vec(),
    }
}

/// Build the `QueueLen` request — pure, no I/O. v0 callers pass the default
/// partition and `total = false` (Python `Queue.len()` defaults).
pub(crate) fn build_queue_len_request(
    queue_id: &str,
    partition_key: &[u8],
    total: bool,
) -> QueueLenRequest {
    QueueLenRequest {
        queue_id: queue_id.to_string(),
        partition_key: partition_key.to_vec(),
        total,
    }
}

/// Build the `QueueClear` request — pure, no I/O. v0 callers pass the default
/// partition with `all_partitions = true` (clear everything, mirroring the
/// facade's whole-queue `clear()`).
pub(crate) fn build_queue_clear_request(
    queue_id: &str,
    partition_key: &[u8],
    all_partitions: bool,
) -> QueueClearRequest {
    QueueClearRequest {
        queue_id: queue_id.to_string(),
        partition_key: partition_key.to_vec(),
        all_partitions,
    }
}

/// Build the `QueueDelete` request — pure, no I/O. Irreversible server-side.
pub(crate) fn build_queue_delete_request(queue_id: &str) -> QueueDeleteRequest {
    QueueDeleteRequest {
        queue_id: queue_id.to_string(),
    }
}

impl ModalClient {
    /// Resolve a named Queue and return its `queue_id`.
    ///
    /// - `creation_type = CreateIfMissing` mirrors `Queue.from_name` —
    ///   idempotent + retry-safe.
    /// - `creation_type = Unspecified` is the pure-lookup `Queue.lookup` path.
    /// - `environment`: defaults to the configured environment (or `"main"`).
    pub async fn queue_get_or_create(
        &self,
        name: &str,
        creation_type: ObjectCreationType,
        environment: Option<&str>,
    ) -> Result<String> {
        let environment_name = self.env_or_default(environment);
        let req = build_queue_get_or_create_request(name, environment_name, creation_type);
        let resp = self
            .retry_rpc("queue_get_or_create", req, |mut stub, req| async move {
                stub.queue_get_or_create(req).await
            })
            .await?;
        if resp.queue_id.is_empty() {
            return Err(Error::build(format!(
                "QueueGetOrCreate for '{name}' returned an empty queue_id"
            )));
        }
        Ok(resp.queue_id)
    }

    /// `QueuePut` — push one or more encoded byte items onto the default
    /// partition (put and put_many are the same RPC). Sends Python's explicit
    /// partition-TTL default. A queue-full error surfaces directly as the RPC
    /// error (block-on-full retry is deferred, design doc §4).
    pub async fn queue_put(&self, queue_id: &str, values: Vec<Vec<u8>>) -> Result<()> {
        let req = build_queue_put_request(queue_id, values, b"", DEFAULT_PARTITION_TTL_SECONDS);
        self.retry_rpc("queue_put", req, |mut stub, req| async move {
            stub.queue_put(req).await
        })
        .await?;
        Ok(())
    }

    /// One `QueueGet` RPC — up to `n_values` items from the default partition,
    /// blocking server-side up to `timeout_secs` (`0.0` = non-blocking poll).
    /// Empty result = nothing arrived within the window. Most callers want
    /// [`queue_get_blocking`](ModalClient::queue_get_blocking) instead, which
    /// caps each RPC and loops.
    pub async fn queue_get(
        &self,
        queue_id: &str,
        n_values: u32,
        timeout_secs: f32,
    ) -> Result<Vec<Vec<u8>>> {
        let req = build_queue_get_request(queue_id, n_values, timeout_secs, b"");
        let resp = self
            .retry_rpc("queue_get", req, |mut stub, req| async move {
                stub.queue_get(req).await
            })
            .await?;
        Ok(resp.values)
    }

    /// Blocking get with the client-side poll loop (design doc §3.4) — mirrors
    /// `Queue._get_blocking` so a long wait never rides one gRPC deadline.
    ///
    /// - `timeout = None` — block until at least one item arrives (loop
    ///   forever; each RPC blocks ≤ [`QUEUE_GET_PER_RPC_TIMEOUT_SECS`]).
    /// - `timeout = Some(d)` — return `Ok(vec![])` once ~`d` elapses with no
    ///   item; per-RPC window = `min(remaining, cap)`.
    /// - `timeout = Some(Duration::ZERO)` — one non-blocking poll.
    ///
    /// Returns up to `n_values` items — first-item-blocks semantics: whatever
    /// the server hands back once something is available (Python parity).
    pub async fn queue_get_blocking(
        &self,
        queue_id: &str,
        n_values: u32,
        timeout: Option<Duration>,
    ) -> Result<Vec<Vec<u8>>> {
        let deadline = timeout.map(|d| Instant::now() + d);
        let cap = Duration::from_secs_f32(QUEUE_GET_PER_RPC_TIMEOUT_SECS);
        loop {
            let per_rpc = match deadline {
                None => cap,
                Some(dl) => dl.saturating_duration_since(Instant::now()).min(cap),
            };
            let values = self
                .queue_get(queue_id, n_values, per_rpc.as_secs_f32())
                .await?;
            if !values.is_empty() {
                return Ok(values);
            }
            if let Some(dl) = deadline {
                if Instant::now() >= dl {
                    return Ok(Vec::new()); // timed out empty — caller maps to None
                }
            }
        }
    }

    /// `QueueLen` — items in the default partition (`total = false`, the
    /// Python `Queue.len()` default). `int32` on the wire, widened to `u64`.
    pub async fn queue_len(&self, queue_id: &str) -> Result<u64> {
        let req = build_queue_len_request(queue_id, b"", false);
        let resp = self
            .retry_rpc("queue_len", req, |mut stub, req| async move {
                stub.queue_len(req).await
            })
            .await?;
        Ok(u64::try_from(resp.len).unwrap_or(0))
    }

    /// `QueueClear` — remove ALL items across ALL partitions. Irreversible.
    pub async fn queue_clear(&self, queue_id: &str) -> Result<()> {
        let req = build_queue_clear_request(queue_id, b"", true);
        self.retry_rpc("queue_clear", req, |mut stub, req| async move {
            stub.queue_clear(req).await
        })
        .await?;
        Ok(())
    }

    /// `QueueDelete` — delete the Queue object itself (by id). Irreversible.
    pub async fn queue_delete(&self, queue_id: &str) -> Result<()> {
        let req = build_queue_delete_request(queue_id);
        self.retry_rpc("queue_delete", req, |mut stub, req| async move {
            stub.queue_delete(req).await
        })
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_type_constants_match_proto() {
        assert_eq!(ObjectCreationType::Unspecified as i32, 0);
        assert_eq!(ObjectCreationType::CreateIfMissing as i32, 1);
        assert_eq!(ObjectCreationType::Ephemeral as i32, 5);
    }

    #[test]
    fn get_or_create_from_name_is_create_if_missing() {
        let req = build_queue_get_or_create_request(
            "jobs",
            "main".to_string(),
            ObjectCreationType::CreateIfMissing,
        );
        assert_eq!(req.deployment_name, "jobs");
        assert_eq!(req.environment_name, "main");
        assert_eq!(
            req.object_creation_type,
            ObjectCreationType::CreateIfMissing as i32
        );
    }

    #[test]
    fn get_or_create_lookup_is_unspecified() {
        let req = build_queue_get_or_create_request(
            "jobs",
            "dev".to_string(),
            ObjectCreationType::Unspecified,
        );
        assert_eq!(req.object_creation_type, 0);
        assert_eq!(req.environment_name, "dev");
    }

    #[test]
    fn put_request_default_partition_and_explicit_ttl() {
        // The v0 wire pins: EMPTY partition_key (default partition) + Python's
        // EXPLICIT 24h TTL default (never 0) + repeated values in order.
        let req = build_queue_put_request(
            "qu-1",
            vec![b"a".to_vec(), b"b".to_vec()],
            b"",
            DEFAULT_PARTITION_TTL_SECONDS,
        );
        assert_eq!(req.queue_id, "qu-1");
        assert_eq!(req.values, vec![b"a".to_vec(), b"b".to_vec()]);
        assert!(
            req.partition_key.is_empty(),
            "default partition = empty key"
        );
        assert_eq!(req.partition_ttl_seconds, 86_400, "queue.py:549 default");
    }

    #[test]
    fn get_request_clamps_n_values_to_at_least_one() {
        let req = build_queue_get_request("qu-1", 0, 5.0, b"");
        assert_eq!(req.n_values, 1, "n_values is clamped to >= 1");
        let req = build_queue_get_request("qu-1", 10, 5.0, b"");
        assert_eq!(req.n_values, 10);
    }

    #[test]
    fn get_request_carries_timeout_and_default_partition() {
        let req = build_queue_get_request("qu-1", 1, 12.5, b"");
        assert_eq!(req.queue_id, "qu-1");
        assert_eq!(req.timeout, 12.5);
        assert!(req.partition_key.is_empty());
    }

    #[test]
    fn per_rpc_timeout_cap_matches_python() {
        // queue.py:448 — `request_timeout = 50.0` keeps the connection alive.
        assert_eq!(QUEUE_GET_PER_RPC_TIMEOUT_SECS, 50.0);
    }

    #[test]
    fn len_request_default_partition_not_total() {
        let req = build_queue_len_request("qu-1", b"", false);
        assert_eq!(req.queue_id, "qu-1");
        assert!(req.partition_key.is_empty());
        assert!(!req.total);
    }

    #[test]
    fn clear_request_all_partitions() {
        let req = build_queue_clear_request("qu-1", b"", true);
        assert_eq!(req.queue_id, "qu-1");
        assert!(req.all_partitions);
    }

    #[test]
    fn delete_request_carries_queue_id() {
        assert_eq!(build_queue_delete_request("qu-9").queue_id, "qu-9");
    }
}
