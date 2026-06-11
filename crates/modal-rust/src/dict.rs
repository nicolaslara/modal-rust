//! `modal_rust::Dict` — the typed facade handle over modal.Dict.
//!
//! A Dict is a named, server-side key-value store shared across functions and
//! clients (Python parity: `modal.Dict`). This module is the thin typed layer
//! over the SDK ops (`modal_rust_sdk::ops::dict`) + the restricted-pickle codec
//! (`modal_rust_sdk::pickle`, the Python interop boundary — design:
//! `docs/local/dict-queue-design.md`):
//!
//! - **Keys are `&str` in v0**, encoded as byte-exact CPython protocol-4 pickle
//!   so Rust-written keys are loadable from Python and vice versa (the server
//!   matches Dict keys by byte-equality on the serialized key).
//! - **Values are per-call generic** (`d.get::<i64>("k")`) — Modal dicts are
//!   heterogeneous, like Python. Values round-trip with Python for *plain data*
//!   (str/int/float/bool/bytes/lists/dicts/structs-as-dicts); a Python-pickled
//!   custom class/function fails decode with a typed codec error, never a
//!   panic or a silent `None`.
//! - **`_raw` byte methods** are the bring-your-own-codec escape hatch; the
//!   typed methods are implemented on top of them.
//!
//! Dicts are app-independent in Modal, so the handle does NOT hang off
//! [`App`](crate::App): each `Dict` owns its own [`sdk::ModalClient`] behind an
//! `Arc<Mutex<…>>` (the same client-ownership shape as `App`'s `RemoteHandle`);
//! handles are `Clone` (clones share the client).
//!
//! Semantics (from the Python client, restated for Rust users):
//! - Entries expire after **7 days of inactivity** (no reads or writes).
//! - [`Dict::len`] is expensive server-side and caps at **100,000**.
//! - [`Dict::delete`] (by name) and [`Dict::clear`] are irreversible.

use std::sync::Arc;

use serde::{de::DeserializeOwned, Serialize};
use tokio::sync::Mutex;

use crate::error::Result;
use crate::sdk;
use sdk::modal::client::ObjectCreationType;
use sdk::pickle;

/// A handle to a named modal.Dict. See the [module docs](self) for the codec
/// and lifecycle model. `Clone` is cheap (clones share the underlying client).
#[derive(Clone)]
pub struct Dict {
    /// Owned control-plane client, the same `Arc<Mutex<…>>` shape as `App`'s
    /// `RemoteHandle`: interior mutability + single-flighting concurrent calls
    /// from clones.
    client: Arc<Mutex<sdk::ModalClient>>,
    /// Resolved server-side dict id (`DictGetOrCreate`).
    dict_id: String,
    /// The deployment name the handle was resolved from (diagnostics only).
    name: String,
}

impl std::fmt::Debug for Dict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dict")
            .field("name", &self.name)
            .field("dict_id", &self.dict_id)
            .finish_non_exhaustive()
    }
}

/// TEST-ONLY: build a [`sdk::ModalClient`] pointed at an explicit `server_url`
/// (an in-process mock) with dummy credentials — the same seam as
/// `App::connect_at`. Shared by [`Dict::from_name_at`] and
/// [`Queue::from_name_at`](crate::Queue::from_name_at).
#[cfg(any(test, feature = "testkit"))]
pub(crate) async fn mock_client(server_url: String) -> Result<sdk::ModalClient> {
    let config = sdk::ModalConfig {
        profile: "mock".into(),
        server_url,
        token_id: "ak-mock".into(),
        token_secret: "as-mock".into(),
        environment: Some("main".into()),
        image_builder_version: None,
    };
    Ok(sdk::ModalClient::from_config(config).await?)
}

impl Dict {
    /// Resolve the named Dict, **creating it if missing** (`CREATE_IF_MISSING` —
    /// idempotent and retry-safe). Uses the configured default environment.
    /// This is the common path, mirroring Python's `modal.Dict.from_name`.
    pub async fn from_name(name: &str) -> Result<Self> {
        let client = sdk::ModalClient::connect().await?;
        Self::resolve_with(client, name, ObjectCreationType::CreateIfMissing, None).await
    }

    /// Pure lookup of an EXISTING named Dict (`UNSPECIFIED`): the server's
    /// not-found error surfaces as `Err` instead of creating the object.
    pub async fn lookup(name: &str) -> Result<Self> {
        let client = sdk::ModalClient::connect().await?;
        Self::resolve_with(client, name, ObjectCreationType::Unspecified, None).await
    }

    /// As [`from_name`](Dict::from_name), but in an explicit Modal environment.
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

    /// Delete the named Dict — the OBJECT itself, not an entry. **Irreversible.**
    /// Resolves the name without creating (`UNSPECIFIED`), then `DictDelete`s it;
    /// a missing Dict surfaces the server's not-found error.
    pub async fn delete(name: &str) -> Result<()> {
        let client = sdk::ModalClient::connect().await?;
        let dict_id = client
            .dict_get_or_create(name, ObjectCreationType::Unspecified, None)
            .await?;
        client.dict_delete(&dict_id).await?;
        Ok(())
    }

    /// TEST-ONLY: resolve at an explicit `server_url` (e.g. an in-process mock)
    /// with dummy credentials, mirroring `App::connect_at`. Gated behind the
    /// `testkit` feature (enabled only by test targets); NOT shipped public API.
    #[cfg(any(test, feature = "testkit"))]
    pub async fn from_name_at(name: &str, server_url: String) -> Result<Self> {
        let client = mock_client(server_url).await?;
        Self::resolve_with(client, name, ObjectCreationType::CreateIfMissing, None).await
    }

    /// Shared resolve body: `DictGetOrCreate` → id → handle.
    async fn resolve_with(
        client: sdk::ModalClient,
        name: &str,
        creation_type: ObjectCreationType,
        environment: Option<&str>,
    ) -> Result<Self> {
        let dict_id = client
            .dict_get_or_create(name, creation_type, environment)
            .await?;
        Ok(Dict {
            client: Arc::new(Mutex::new(client)),
            dict_id,
            name: name.to_string(),
        })
    }

    /// The deployment name this handle was resolved from.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The resolved server-side dict id.
    pub fn dict_id(&self) -> &str {
        &self.dict_id
    }

    // ---- typed surface (restricted-pickle codec; see module docs) ----

    /// Fetch the value for `key`, decoded into `V`. `Ok(None)` = key absent.
    /// A present value that fails to decode into `V` (wrong type, or a pickled
    /// Python object) is an `Err`, never a silent `None`.
    pub async fn get<V: DeserializeOwned>(&self, key: &str) -> Result<Option<V>> {
        decode_opt(self.get_raw(&pickle::encode_str_key(key)).await?)
    }

    /// Write `value` under `key` (`DictUpdate`, one entry), overwriting any
    /// existing entry. The value is pickle-encoded (Python-readable plain data).
    pub async fn put<V: Serialize>(&self, key: &str, value: &V) -> Result<()> {
        self.put_raw(&pickle::encode_str_key(key), &pickle::encode_value(value)?)
            .await
    }

    /// Write `value` under `key` only if the key is absent (`DictUpdate` with
    /// `if_not_exists`). Returns whether the entry was actually inserted
    /// (`false` = the key already existed; the stored value is unchanged).
    pub async fn put_if_absent<V: Serialize>(&self, key: &str, value: &V) -> Result<bool> {
        self.put_if_absent_raw(&pickle::encode_str_key(key), &pickle::encode_value(value)?)
            .await
    }

    /// Remove `key` and return its decoded value. `Ok(None)` = key was absent.
    pub async fn pop<V: DeserializeOwned>(&self, key: &str) -> Result<Option<V>> {
        decode_opt(self.pop_raw(&pickle::encode_str_key(key)).await?)
    }

    /// Whether `key` is present.
    pub async fn contains(&self, key: &str) -> Result<bool> {
        self.contains_raw(&pickle::encode_str_key(key)).await
    }

    /// Number of entries. Expensive server-side; the answer caps at 100,000.
    pub async fn len(&self) -> Result<u64> {
        let client = self.client.lock().await;
        client.dict_len(&self.dict_id).await.map_err(Into::into)
    }

    /// Remove ALL entries. **Irreversible.**
    pub async fn clear(&self) -> Result<()> {
        let client = self.client.lock().await;
        client.dict_clear(&self.dict_id).await.map_err(Into::into)
    }

    // ---- raw escape hatch (bring your own codec; exact bytes on the wire) ----

    /// Fetch the exact value bytes stored under the exact `key` bytes.
    /// `Ok(None)` = key absent (an empty stored value stays `Some(vec![])`).
    pub async fn get_raw(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let client = self.client.lock().await;
        client
            .dict_get(&self.dict_id, key)
            .await
            .map_err(Into::into)
    }

    /// Write exact `value` bytes under exact `key` bytes, overwriting.
    pub async fn put_raw(&self, key: &[u8], value: &[u8]) -> Result<()> {
        let client = self.client.lock().await;
        client
            .dict_update(&self.dict_id, vec![(key.to_vec(), value.to_vec())], false)
            .await?;
        Ok(())
    }

    /// Write exact bytes only if the key is absent; returns whether inserted.
    pub async fn put_if_absent_raw(&self, key: &[u8], value: &[u8]) -> Result<bool> {
        let client = self.client.lock().await;
        client
            .dict_update(&self.dict_id, vec![(key.to_vec(), value.to_vec())], true)
            .await
            .map_err(Into::into)
    }

    /// Remove the exact `key` bytes and return the stored value bytes.
    pub async fn pop_raw(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let client = self.client.lock().await;
        client
            .dict_pop(&self.dict_id, key)
            .await
            .map_err(Into::into)
    }

    /// Whether the exact `key` bytes are present.
    pub async fn contains_raw(&self, key: &[u8]) -> Result<bool> {
        let client = self.client.lock().await;
        client
            .dict_contains(&self.dict_id, key)
            .await
            .map_err(Into::into)
    }
}

/// Decode an optional raw value: absence passes through; a present value that
/// fails to decode is an `Err` (typed codec error), never a silent `None`.
fn decode_opt<V: DeserializeOwned>(bytes: Option<Vec<u8>>) -> Result<Option<V>> {
    match bytes {
        None => Ok(None),
        Some(b) => Ok(Some(pickle::decode_value(&b)?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    #[test]
    fn handle_is_clone_send_sync() {
        // The handle must be shareable across tasks (clones share the client).
        fn assert_impls<T: Clone + Send + Sync>() {}
        assert_impls::<Dict>();
    }

    #[test]
    fn decode_opt_absent_is_none() {
        let got: Option<i64> = decode_opt(None).unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn decode_opt_present_decodes() {
        let bytes = pickle::encode_value(&42_i64).unwrap();
        let got: Option<i64> = decode_opt(Some(bytes)).unwrap();
        assert_eq!(got, Some(42));
    }

    #[test]
    fn decode_opt_wrong_type_is_err_not_none() {
        // The contract: a PRESENT key that fails to decode is an Err — a silent
        // None would be indistinguishable from absence.
        let bytes = pickle::encode_value(&"not an int").unwrap();
        let got: Result<Option<i64>> = decode_opt(Some(bytes));
        assert!(
            matches!(got, Err(Error::Sdk(sdk::Error::Codec(_)))),
            "got: {got:?}"
        );
    }
}
