//! modal.Dict control-plane ops — the v0 unary RPC surface.
//!
//! Mirrors `modal/dict.py` over the Dict RPCs (api.proto ~4191; messages
//! ~1252–1366). Keys and values are **opaque bytes at this layer** — the server
//! never interprets them (there is no wire-level format field, unlike function
//! IO's `DataFormat`). Encoding is the CALLER's concern: the facade encodes via
//! [`crate::pickle`] (restricted-pickle, Python-interoperable); raw-byte users
//! ride these methods directly.
//!
//! v0 surface (all unary, all through `retry_rpc`):
//!
//! - `DictGetOrCreate` — named resolve (`CREATE_IF_MISSING`) or pure lookup
//!   (`UNSPECIFIED`); the `data` field is "Unused after 1.4.0" and never sent.
//! - `DictGet` / `DictPop` — `{dict_id, key}` → `{found, optional value}`.
//! - `DictUpdate` — put / put-if-absent / batch update are ALL this one RPC
//!   (`repeated DictEntry updates` + `if_not_exists`); there is no DictPut.
//! - `DictContains` / `DictLen` / `DictClear` / `DictDelete`.
//!
//! Deferred (design doc §4): `DictContents` (the only streaming RPC),
//! `DictHeartbeat` (ephemeral lifecycle), `DictGetById`/`DictList`.
//!
//! Semantics to surface in facade rustdoc: entries expire after 7 days of
//! inactivity; `DictLen` is expensive and caps at 100,000 (the wire `len` is
//! `int32`; we expose `u64`).

use crate::client::ModalClient;
use crate::error::{Error, Result};
use crate::proto::api::{
    DictClearRequest, DictContainsRequest, DictDeleteRequest, DictEntry, DictGetOrCreateRequest,
    DictGetRequest, DictLenRequest, DictPopRequest, DictUpdateRequest, ObjectCreationType,
};

/// Build the `DictGetOrCreate` request — pure, no I/O. Mirrors
/// `Dict.from_name` (`CREATE_IF_MISSING`, idempotent + retry-safe) and
/// `Dict.lookup` (`UNSPECIFIED` — pure lookup; the server errors if missing).
/// Never sends `data` (proto: "Unused after 1.4.0").
pub(crate) fn build_dict_get_or_create_request(
    name: &str,
    environment_name: String,
    creation_type: ObjectCreationType,
) -> DictGetOrCreateRequest {
    DictGetOrCreateRequest {
        deployment_name: name.to_string(),
        environment_name,
        object_creation_type: creation_type as i32,
        ..Default::default() // data: never sent (unused after 1.4.0)
    }
}

/// Build the `DictGet` request — pure, no I/O. `key` is the caller-encoded bytes
/// (byte-equality is how the server matches keys).
pub(crate) fn build_dict_get_request(dict_id: &str, key: &[u8]) -> DictGetRequest {
    DictGetRequest {
        dict_id: dict_id.to_string(),
        key: key.to_vec(),
    }
}

/// Build the `DictUpdate` request — pure, no I/O. There is no DictPut on the
/// wire: single put = one entry; `put_if_absent` = `if_not_exists = true`;
/// batch update = many entries. Entries are caller-encoded `(key, value)` bytes.
pub(crate) fn build_dict_update_request(
    dict_id: &str,
    entries: Vec<(Vec<u8>, Vec<u8>)>,
    if_not_exists: bool,
) -> DictUpdateRequest {
    DictUpdateRequest {
        dict_id: dict_id.to_string(),
        updates: entries
            .into_iter()
            .map(|(key, value)| DictEntry { key, value })
            .collect(),
        if_not_exists,
    }
}

/// Build the `DictPop` request — pure, no I/O. Same shape as `DictGet`.
pub(crate) fn build_dict_pop_request(dict_id: &str, key: &[u8]) -> DictPopRequest {
    DictPopRequest {
        dict_id: dict_id.to_string(),
        key: key.to_vec(),
    }
}

/// Build the `DictContains` request — pure, no I/O.
pub(crate) fn build_dict_contains_request(dict_id: &str, key: &[u8]) -> DictContainsRequest {
    DictContainsRequest {
        dict_id: dict_id.to_string(),
        key: key.to_vec(),
    }
}

/// Build the `DictLen` request — pure, no I/O.
pub(crate) fn build_dict_len_request(dict_id: &str) -> DictLenRequest {
    DictLenRequest {
        dict_id: dict_id.to_string(),
    }
}

/// Build the `DictClear` request — pure, no I/O.
pub(crate) fn build_dict_clear_request(dict_id: &str) -> DictClearRequest {
    DictClearRequest {
        dict_id: dict_id.to_string(),
    }
}

/// Build the `DictDelete` request — pure, no I/O. Irreversible server-side.
pub(crate) fn build_dict_delete_request(dict_id: &str) -> DictDeleteRequest {
    DictDeleteRequest {
        dict_id: dict_id.to_string(),
    }
}

impl ModalClient {
    /// Resolve a named Dict and return its `dict_id`.
    ///
    /// - `creation_type = CreateIfMissing` mirrors `Dict.from_name` —
    ///   idempotent and retry-safe (a retry after a dropped response re-resolves
    ///   the same id, same justification as `secret_get_or_create`).
    /// - `creation_type = Unspecified` is the pure-lookup `Dict.lookup` path —
    ///   the server's not-found error surfaces as the RPC error.
    /// - `environment`: defaults to the configured environment (or `"main"`).
    pub async fn dict_get_or_create(
        &self,
        name: &str,
        creation_type: ObjectCreationType,
        environment: Option<&str>,
    ) -> Result<String> {
        let environment_name = self.env_or_default(environment);
        let req = build_dict_get_or_create_request(name, environment_name, creation_type);
        let resp = self
            .retry_rpc("dict_get_or_create", req, |mut stub, req| async move {
                stub.dict_get_or_create(req).await
            })
            .await?;
        if resp.dict_id.is_empty() {
            return Err(Error::build(format!(
                "DictGetOrCreate for '{name}' returned an empty dict_id"
            )));
        }
        Ok(resp.dict_id)
    }

    /// `DictGet` — fetch the value bytes for an encoded key. `Ok(None)` means the
    /// key is absent (`found = false`); a present key always yields `Some` (an
    /// empty value byte-string stays distinguishable from absence).
    pub async fn dict_get(&self, dict_id: &str, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let req = build_dict_get_request(dict_id, key);
        let resp = self
            .retry_rpc("dict_get", req, |mut stub, req| async move {
                stub.dict_get(req).await
            })
            .await?;
        if resp.found {
            Ok(Some(resp.value.unwrap_or_default()))
        } else {
            Ok(None)
        }
    }

    /// `DictUpdate` — write one or more `(key, value)` byte entries. Returns the
    /// server's `created` flag: with `if_not_exists = true` it reports whether
    /// the entry was actually inserted (`false` = key already existed). Put,
    /// put-if-absent and batch update are all this RPC (there is no DictPut).
    pub async fn dict_update(
        &self,
        dict_id: &str,
        entries: Vec<(Vec<u8>, Vec<u8>)>,
        if_not_exists: bool,
    ) -> Result<bool> {
        let req = build_dict_update_request(dict_id, entries, if_not_exists);
        let resp = self
            .retry_rpc("dict_update", req, |mut stub, req| async move {
                stub.dict_update(req).await
            })
            .await?;
        Ok(resp.created)
    }

    /// `DictPop` — remove and return the value bytes for an encoded key.
    /// `Ok(None)` means the key was absent.
    pub async fn dict_pop(&self, dict_id: &str, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let req = build_dict_pop_request(dict_id, key);
        let resp = self
            .retry_rpc("dict_pop", req, |mut stub, req| async move {
                stub.dict_pop(req).await
            })
            .await?;
        if resp.found {
            Ok(Some(resp.value.unwrap_or_default()))
        } else {
            Ok(None)
        }
    }

    /// `DictContains` — whether an encoded key is present.
    pub async fn dict_contains(&self, dict_id: &str, key: &[u8]) -> Result<bool> {
        let req = build_dict_contains_request(dict_id, key);
        let resp = self
            .retry_rpc("dict_contains", req, |mut stub, req| async move {
                stub.dict_contains(req).await
            })
            .await?;
        Ok(resp.found)
    }

    /// `DictLen` — number of entries. Expensive server-side; the wire caps the
    /// answer at 100,000 (`int32` on the wire, widened to `u64` here).
    pub async fn dict_len(&self, dict_id: &str) -> Result<u64> {
        let req = build_dict_len_request(dict_id);
        let resp = self
            .retry_rpc("dict_len", req, |mut stub, req| async move {
                stub.dict_len(req).await
            })
            .await?;
        Ok(u64::try_from(resp.len).unwrap_or(0))
    }

    /// `DictClear` — remove ALL entries. Irreversible.
    pub async fn dict_clear(&self, dict_id: &str) -> Result<()> {
        let req = build_dict_clear_request(dict_id);
        self.retry_rpc("dict_clear", req, |mut stub, req| async move {
            stub.dict_clear(req).await
        })
        .await?;
        Ok(())
    }

    /// `DictDelete` — delete the Dict object itself (by id). Irreversible.
    pub async fn dict_delete(&self, dict_id: &str) -> Result<()> {
        let req = build_dict_delete_request(dict_id);
        self.retry_rpc("dict_delete", req, |mut stub, req| async move {
            stub.dict_delete(req).await
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
        // Pin the proto enum values the dict lifecycle relies on (api.proto ~207).
        assert_eq!(ObjectCreationType::Unspecified as i32, 0);
        assert_eq!(ObjectCreationType::CreateIfMissing as i32, 1);
        assert_eq!(ObjectCreationType::Ephemeral as i32, 5);
    }

    #[test]
    fn get_or_create_from_name_is_create_if_missing() {
        // The `from_name` path: CREATE_IF_MISSING, never sends the dead `data` field.
        let req = build_dict_get_or_create_request(
            "scores",
            "main".to_string(),
            ObjectCreationType::CreateIfMissing,
        );
        assert_eq!(req.deployment_name, "scores");
        assert_eq!(req.environment_name, "main");
        assert_eq!(
            req.object_creation_type,
            ObjectCreationType::CreateIfMissing as i32
        );
        assert!(
            req.data.is_empty(),
            "`data` is unused after 1.4.0 — never sent"
        );
    }

    #[test]
    fn get_or_create_lookup_is_unspecified() {
        // The `lookup` path: UNSPECIFIED == "just lookup"; server errors if missing.
        let req = build_dict_get_or_create_request(
            "scores",
            "dev".to_string(),
            ObjectCreationType::Unspecified,
        );
        assert_eq!(req.object_creation_type, 0);
        assert_eq!(req.environment_name, "dev");
    }

    #[test]
    fn get_request_carries_raw_key_bytes() {
        let req = build_dict_get_request("di-123", b"\x80\x04key");
        assert_eq!(req.dict_id, "di-123");
        assert_eq!(req.key, b"\x80\x04key".to_vec());
    }

    #[test]
    fn update_request_single_put() {
        // Single put = one entry, if_not_exists = false.
        let req = build_dict_update_request("di-123", vec![(b"k".to_vec(), b"v".to_vec())], false);
        assert_eq!(req.dict_id, "di-123");
        assert_eq!(req.updates.len(), 1);
        assert_eq!(req.updates[0].key, b"k".to_vec());
        assert_eq!(req.updates[0].value, b"v".to_vec());
        assert!(!req.if_not_exists);
    }

    #[test]
    fn update_request_put_if_absent_sets_flag() {
        let req = build_dict_update_request("di-123", vec![(b"k".to_vec(), b"v".to_vec())], true);
        assert!(req.if_not_exists);
    }

    #[test]
    fn update_request_batch_preserves_order() {
        let entries = vec![
            (b"a".to_vec(), b"1".to_vec()),
            (b"b".to_vec(), b"2".to_vec()),
        ];
        let req = build_dict_update_request("di-123", entries, false);
        assert_eq!(req.updates.len(), 2);
        assert_eq!(req.updates[0].key, b"a".to_vec());
        assert_eq!(req.updates[1].key, b"b".to_vec());
    }

    #[test]
    fn pop_and_contains_requests_carry_key() {
        let pop = build_dict_pop_request("di-1", b"k");
        assert_eq!(pop.dict_id, "di-1");
        assert_eq!(pop.key, b"k".to_vec());
        let contains = build_dict_contains_request("di-1", b"k");
        assert_eq!(contains.dict_id, "di-1");
        assert_eq!(contains.key, b"k".to_vec());
    }

    #[test]
    fn id_only_requests_carry_dict_id() {
        assert_eq!(build_dict_len_request("di-9").dict_id, "di-9");
        assert_eq!(build_dict_clear_request("di-9").dict_id, "di-9");
        assert_eq!(build_dict_delete_request("di-9").dict_id, "di-9");
    }
}
