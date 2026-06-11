//! `modal_rust::Queue` — the typed facade handle over modal.Queue.
//!
//! A Queue is a named, server-side FIFO queue shared across functions and
//! clients (Python parity: `modal.Queue`). This module is the thin typed layer
//! over the SDK ops (`modal_rust_sdk::ops::queue`) + the restricted-pickle
//! codec (`modal_rust_sdk::pickle`, the Python interop boundary — design:
//! `docs/local/dict-queue-design.md`):
//!
//! - **Values are per-call generic** and round-trip with Python for *plain
//!   data* (str/int/float/bool/bytes/lists/dicts/structs-as-dicts); a
//!   Python-pickled custom class/function fails decode with a typed codec
//!   error, never a panic.
//! - **Blocking gets never ride one gRPC deadline**: [`Queue::get`] /
//!   [`Queue::get_many`] delegate to the SDK's `queue_get_blocking` poll loop,
//!   which caps every `QueueGet` RPC at Python's 50 s constant and loops
//!   client-side until the caller's deadline (or forever for `timeout = None`).
//! - **`_raw` byte methods** are the bring-your-own-codec escape hatch; the
//!   typed methods are implemented on top of them.
//!
//! Timeout convention (mirrors Python's `get(block=True, timeout=None)`
//! defaults without a boolean):
//!
//! - `None` — block until an item arrives (a long-running `get` holds this
//!   handle's client lock, so clones of the SAME handle queue behind it; use
//!   separate handles for concurrent consumers).
//! - `Some(d)` — wait ~`d`, then return `Ok(None)` / `Ok(vec![])` on timeout.
//! - `Some(Duration::ZERO)` — one non-blocking poll.
//!
//! Queues are app-independent in Modal, so the handle does NOT hang off
//! [`App`](crate::App): each `Queue` owns its own [`sdk::ModalClient`] behind
//! an `Arc<Mutex<…>>` (the same client-ownership shape as `App`'s
//! `RemoteHandle`); handles are `Clone` (clones share the client).
//!
//! Semantics + limits (from the Python client, restated for Rust users):
//! - Up to 100,000 partitions × 5,000 items; **1 MiB per item**.
//! - v0 always uses the **default partition**, with Python's explicit 24 h
//!   partition TTL on every put (partition parameters are deferred, additive).
//! - [`Queue::delete`] (by name) and [`Queue::clear`] are irreversible.

use std::sync::Arc;
use std::time::Duration;

use serde::{de::DeserializeOwned, Serialize};
use tokio::sync::Mutex;

use crate::error::Result;
use crate::sdk;
use sdk::modal::client::ObjectCreationType;
use sdk::pickle;

/// A handle to a named modal.Queue. See the [module docs](self) for the codec,
/// timeout convention, and lifecycle model. `Clone` is cheap (clones share the
/// underlying client).
#[derive(Clone)]
pub struct Queue {
    /// Owned control-plane client, the same `Arc<Mutex<…>>` shape as `App`'s
    /// `RemoteHandle`: interior mutability + single-flighting concurrent calls
    /// from clones.
    client: Arc<Mutex<sdk::ModalClient>>,
    /// Resolved server-side queue id (`QueueGetOrCreate`).
    queue_id: String,
    /// The deployment name the handle was resolved from (diagnostics only).
    name: String,
}

impl std::fmt::Debug for Queue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Queue")
            .field("name", &self.name)
            .field("queue_id", &self.queue_id)
            .finish_non_exhaustive()
    }
}

impl Queue {
    /// Resolve the named Queue, **creating it if missing** (`CREATE_IF_MISSING`
    /// — idempotent and retry-safe). Uses the configured default environment.
    /// This is the common path, mirroring Python's `modal.Queue.from_name`.
    pub async fn from_name(name: &str) -> Result<Self> {
        let client = sdk::ModalClient::connect().await?;
        Self::resolve_with(client, name, ObjectCreationType::CreateIfMissing, None).await
    }

    /// Pure lookup of an EXISTING named Queue (`UNSPECIFIED`): the server's
    /// not-found error surfaces as `Err` instead of creating the object.
    pub async fn lookup(name: &str) -> Result<Self> {
        let client = sdk::ModalClient::connect().await?;
        Self::resolve_with(client, name, ObjectCreationType::Unspecified, None).await
    }

    /// As [`from_name`](Queue::from_name), but in an explicit Modal environment.
    pub async fn from_name_in(name: &str, environment: &str) -> Result<Self> {
        let client = sdk::ModalClient::connect().await?;
        Self::resolve_with(
            client,
            name,
            ObjectCreationType::CreateIfMissing,
            Some(environment),
        )
        .await
    }

    /// Delete the named Queue — the OBJECT itself, not its items.
    /// **Irreversible.** Resolves the name without creating (`UNSPECIFIED`),
    /// then `QueueDelete`s it; a missing Queue surfaces the server's not-found
    /// error.
    pub async fn delete(name: &str) -> Result<()> {
        let client = sdk::ModalClient::connect().await?;
        let queue_id = client
            .queue_get_or_create(name, ObjectCreationType::Unspecified, None)
            .await?;
        client.queue_delete(&queue_id).await?;
        Ok(())
    }

    /// TEST-ONLY: resolve at an explicit `server_url` (e.g. an in-process mock)
    /// with dummy credentials, mirroring `App::connect_at`. Gated behind the
    /// `testkit` feature (enabled only by test targets); NOT shipped public API.
    #[cfg(any(test, feature = "testkit"))]
    pub async fn from_name_at(name: &str, server_url: String) -> Result<Self> {
        let client = crate::dict::mock_client(server_url).await?;
        Self::resolve_with(client, name, ObjectCreationType::CreateIfMissing, None).await
    }

    /// Shared resolve body: `QueueGetOrCreate` → id → handle.
    async fn resolve_with(
        client: sdk::ModalClient,
        name: &str,
        creation_type: ObjectCreationType,
        environment: Option<&str>,
    ) -> Result<Self> {
        let queue_id = client
            .queue_get_or_create(name, creation_type, environment)
            .await?;
        Ok(Queue {
            client: Arc::new(Mutex::new(client)),
            queue_id,
            name: name.to_string(),
        })
    }

    /// The deployment name this handle was resolved from.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The resolved server-side queue id.
    pub fn queue_id(&self) -> &str {
        &self.queue_id
    }

    // ---- typed surface (restricted-pickle codec; see module docs) ----

    /// Push one value (`QueuePut`, default partition). The value is
    /// pickle-encoded (Python-readable plain data). A queue-full error surfaces
    /// directly as the RPC error.
    pub async fn put<V: Serialize>(&self, value: &V) -> Result<()> {
        self.put_raw(vec![pickle::encode_value(value)?]).await
    }

    /// Push many values in order — the SAME `QueuePut` RPC (`values` is
    /// repeated on the wire). An empty slice is a no-op.
    pub async fn put_many<V: Serialize>(&self, values: &[V]) -> Result<()> {
        let encoded = values
            .iter()
            .map(pickle::encode_value)
            .collect::<sdk::Result<Vec<_>>>()?;
        self.put_raw(encoded).await
    }

    /// Pop the next value, decoded into `V`, honoring the module-level timeout
    /// convention: `None` blocks until an item arrives; `Some(d)` returns
    /// `Ok(None)` after ~`d` without one; `Some(Duration::ZERO)` is one
    /// non-blocking poll. A value that fails to decode into `V` is an `Err`,
    /// never a silent `None`.
    pub async fn get<V: DeserializeOwned>(&self, timeout: Option<Duration>) -> Result<Option<V>> {
        first_decoded(self.get_raw(1, timeout).await?)
    }

    /// Pop up to `n` values — **first-item-blocks** semantics (Python parity):
    /// the same timeout convention as [`get`](Queue::get) governs waiting for
    /// the FIRST item; once anything is available the server returns whatever
    /// batch it has (≤ `n`) without waiting for more. Timeout ⇒ `Ok(vec![])`.
    pub async fn get_many<V: DeserializeOwned>(
        &self,
        n: u32,
        timeout: Option<Duration>,
    ) -> Result<Vec<V>> {
        decode_values(self.get_raw(n, timeout).await?)
    }

    /// Items in the default partition. The wire is `int32` (widened to `u64`).
    pub async fn len(&self) -> Result<u64> {
        let client = self.client.lock().await;
        client.queue_len(&self.queue_id).await.map_err(Into::into)
    }

    /// Remove ALL items across ALL partitions. **Irreversible.**
    pub async fn clear(&self) -> Result<()> {
        let client = self.client.lock().await;
        client.queue_clear(&self.queue_id).await.map_err(Into::into)
    }

    // ---- raw escape hatch (bring your own codec; exact bytes on the wire) ----

    /// Push exact byte items in order (one `QueuePut`). Empty = no-op.
    pub async fn put_raw(&self, values: Vec<Vec<u8>>) -> Result<()> {
        if values.is_empty() {
            return Ok(());
        }
        let client = self.client.lock().await;
        client.queue_put(&self.queue_id, values).await?;
        Ok(())
    }

    /// Pop up to `n` exact byte items via the SDK's blocking poll loop (each
    /// RPC capped at Python's 50 s constant). Same timeout convention as
    /// [`get`](Queue::get); timeout ⇒ `Ok(vec![])`.
    pub async fn get_raw(&self, n: u32, timeout: Option<Duration>) -> Result<Vec<Vec<u8>>> {
        let client = self.client.lock().await;
        client
            .queue_get_blocking(&self.queue_id, n, timeout)
            .await
            .map_err(Into::into)
    }
}

/// Decode the first item of a raw batch: empty (timed out) passes through as
/// `None`; a present item that fails to decode is an `Err` (typed codec
/// error), never a silent `None`.
fn first_decoded<V: DeserializeOwned>(values: Vec<Vec<u8>>) -> Result<Option<V>> {
    match values.into_iter().next() {
        None => Ok(None),
        Some(b) => Ok(Some(pickle::decode_value(&b)?)),
    }
}

/// Decode every item of a raw batch, preserving order; the first decode
/// failure aborts with the typed codec error.
fn decode_values<V: DeserializeOwned>(values: Vec<Vec<u8>>) -> Result<Vec<V>> {
    values
        .iter()
        .map(|b| pickle::decode_value(b))
        .collect::<sdk::Result<Vec<V>>>()
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    #[test]
    fn handle_is_clone_send_sync() {
        // The handle must be shareable across tasks (clones share the client).
        fn assert_impls<T: Clone + Send + Sync>() {}
        assert_impls::<Queue>();
    }

    #[test]
    fn first_decoded_empty_batch_is_none() {
        // queue_get_blocking returns Ok(vec![]) on timeout — maps to Ok(None).
        let got: Option<i64> = first_decoded(Vec::new()).unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn first_decoded_takes_the_first_item() {
        let batch = vec![
            pickle::encode_value(&7_i64).unwrap(),
            pickle::encode_value(&8_i64).unwrap(),
        ];
        let got: Option<i64> = first_decoded(batch).unwrap();
        assert_eq!(got, Some(7));
    }

    #[test]
    fn first_decoded_wrong_type_is_err_not_none() {
        let batch = vec![pickle::encode_value(&"not an int").unwrap()];
        let got: Result<Option<i64>> = first_decoded(batch);
        assert!(
            matches!(got, Err(Error::Sdk(sdk::Error::Codec(_)))),
            "got: {got:?}"
        );
    }

    #[test]
    fn decode_values_preserves_order() {
        let batch = vec![
            pickle::encode_value(&1_i64).unwrap(),
            pickle::encode_value(&2_i64).unwrap(),
            pickle::encode_value(&3_i64).unwrap(),
        ];
        let got: Vec<i64> = decode_values(batch).unwrap();
        assert_eq!(got, vec![1, 2, 3]);
    }

    #[test]
    fn decode_values_empty_is_empty() {
        let got: Vec<i64> = decode_values(Vec::new()).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn decode_values_bad_item_aborts_with_codec_error() {
        let batch = vec![
            pickle::encode_value(&1_i64).unwrap(),
            pickle::encode_value(&"oops").unwrap(),
        ];
        let got: Result<Vec<i64>> = decode_values(batch);
        assert!(
            matches!(got, Err(Error::Sdk(sdk::Error::Codec(_)))),
            "got: {got:?}"
        );
    }
}
