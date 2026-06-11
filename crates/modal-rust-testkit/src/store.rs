//! In-memory Dict/Queue object store — the STATEFUL part of the mock backend.
//!
//! Unlike the canned-response arms (deploy/call/remote), the Dict/Queue arms do
//! real state transitions so offline tests exercise genuine put→get round-trips
//! through the facade handles. The store is deliberately tiny and v0-shaped:
//!
//! - One `BTreeMap<Vec<u8>, Vec<u8>>` per dict id — keys are matched by
//!   **byte-equality on the serialized key**, exactly the real server's contract
//!   (design doc §1.1), so a facade-pickled key only hits if the bytes match.
//! - One `VecDeque<Vec<u8>>` per queue id — the DEFAULT partition only (v0
//!   always sends an empty `partition_key`; partition routing is deferred with
//!   the v1 surface).
//! - Name → id maps give `GetOrCreate` real lifecycle semantics:
//!   `CREATE_IF_MISSING` is idempotent (same name → same id), `UNSPECIFIED`
//!   ("just lookup") misses with `None` (the servicer maps it to
//!   `Status::not_found`, mirroring the real server).
//!
//! Ids are deterministic (`di-{n}` / `qu-{n}` off the servicer's shared
//! counter). The store lives behind an `Arc<Mutex<…>>` on [`crate::servicer::MockServicer`];
//! every lock is short (never held across an await — the mock's blocking
//! `QueueGet` re-locks per poll tick).

use std::collections::{BTreeMap, HashMap, VecDeque};

/// The shared mutable Dict/Queue state. See the [module docs](self).
#[derive(Default)]
pub(crate) struct ObjectStore {
    /// Named-dict lifecycle: deployment name → resolved `di-{n}` id.
    dict_ids: HashMap<String, String>,
    /// Per-dict entries, keyed by the RAW serialized key bytes (byte-equality).
    dicts: HashMap<String, BTreeMap<Vec<u8>, Vec<u8>>>,
    /// Named-queue lifecycle: deployment name → resolved `qu-{n}` id.
    queue_ids: HashMap<String, String>,
    /// Per-queue items (default partition only), FIFO.
    queues: HashMap<String, VecDeque<Vec<u8>>>,
}

impl ObjectStore {
    // ---- lifecycle (GetOrCreate / Delete) ----

    /// Resolve a named dict. `create = true` (CREATE_IF_MISSING) creates it under
    /// `candidate_id` when absent; `create = false` (UNSPECIFIED pure lookup)
    /// returns `None` on a miss (the servicer maps that to not-found).
    pub(crate) fn resolve_dict(
        &mut self,
        name: &str,
        create: bool,
        candidate_id: String,
    ) -> Option<String> {
        if let Some(id) = self.dict_ids.get(name) {
            return Some(id.clone());
        }
        if !create {
            return None;
        }
        self.dict_ids.insert(name.to_string(), candidate_id.clone());
        self.dicts.insert(candidate_id.clone(), BTreeMap::new());
        Some(candidate_id)
    }

    /// Resolve a named queue — same lifecycle contract as [`resolve_dict`](Self::resolve_dict).
    pub(crate) fn resolve_queue(
        &mut self,
        name: &str,
        create: bool,
        candidate_id: String,
    ) -> Option<String> {
        if let Some(id) = self.queue_ids.get(name) {
            return Some(id.clone());
        }
        if !create {
            return None;
        }
        self.queue_ids
            .insert(name.to_string(), candidate_id.clone());
        self.queues.insert(candidate_id.clone(), VecDeque::new());
        Some(candidate_id)
    }

    /// Delete the dict OBJECT (id + its name mapping). `false` = unknown id.
    pub(crate) fn delete_dict(&mut self, dict_id: &str) -> bool {
        if self.dicts.remove(dict_id).is_none() {
            return false;
        }
        self.dict_ids.retain(|_, id| id != dict_id);
        true
    }

    /// Delete the queue OBJECT (id + its name mapping). `false` = unknown id.
    pub(crate) fn delete_queue(&mut self, queue_id: &str) -> bool {
        if self.queues.remove(queue_id).is_none() {
            return false;
        }
        self.queue_ids.retain(|_, id| id != queue_id);
        true
    }

    // ---- dict data ops (all keyed by raw bytes; Err(()) = unknown dict id) ----

    /// `DictGet`: the stored value bytes, or `None` for an absent key.
    pub(crate) fn dict_get(&self, dict_id: &str, key: &[u8]) -> Result<Option<Vec<u8>>, ()> {
        Ok(self.dicts.get(dict_id).ok_or(())?.get(key).cloned())
    }

    /// `DictUpdate`: write the entries (in order). With `if_not_exists`, existing
    /// keys are left untouched. Returns the `created` flag: whether EVERY entry
    /// inserted a new key (the flag the facade's `put_if_absent` reads — v0 sends
    /// a single entry, where this is exactly "was it inserted").
    pub(crate) fn dict_update(
        &mut self,
        dict_id: &str,
        entries: impl IntoIterator<Item = (Vec<u8>, Vec<u8>)>,
        if_not_exists: bool,
    ) -> Result<bool, ()> {
        let dict = self.dicts.get_mut(dict_id).ok_or(())?;
        let mut created = true;
        for (key, value) in entries {
            if dict.contains_key(&key) {
                created = false;
                if if_not_exists {
                    continue; // leave the stored value untouched
                }
            }
            dict.insert(key, value);
        }
        Ok(created)
    }

    /// `DictPop`: remove + return the value bytes (`None` = key was absent).
    pub(crate) fn dict_pop(&mut self, dict_id: &str, key: &[u8]) -> Result<Option<Vec<u8>>, ()> {
        Ok(self.dicts.get_mut(dict_id).ok_or(())?.remove(key))
    }

    /// `DictContains`: byte-equality key presence.
    pub(crate) fn dict_contains(&self, dict_id: &str, key: &[u8]) -> Result<bool, ()> {
        Ok(self.dicts.get(dict_id).ok_or(())?.contains_key(key))
    }

    /// `DictLen`: entry count.
    pub(crate) fn dict_len(&self, dict_id: &str) -> Result<usize, ()> {
        Ok(self.dicts.get(dict_id).ok_or(())?.len())
    }

    /// `DictClear`: remove all entries (the dict object survives).
    pub(crate) fn dict_clear(&mut self, dict_id: &str) -> Result<(), ()> {
        self.dicts.get_mut(dict_id).ok_or(())?.clear();
        Ok(())
    }

    // ---- queue data ops (default partition; Err(()) = unknown queue id) ----

    /// `QueuePut`: append the items in order (put / put_many are the same RPC).
    pub(crate) fn queue_put(
        &mut self,
        queue_id: &str,
        values: impl IntoIterator<Item = Vec<u8>>,
    ) -> Result<(), ()> {
        self.queues.get_mut(queue_id).ok_or(())?.extend(values);
        Ok(())
    }

    /// One non-blocking `QueueGet` poll: pop up to `n` items FIFO (empty =
    /// nothing available right now — the servicer's poll loop handles the
    /// server-side blocking window).
    pub(crate) fn queue_pop(&mut self, queue_id: &str, n: usize) -> Result<Vec<Vec<u8>>, ()> {
        let queue = self.queues.get_mut(queue_id).ok_or(())?;
        let take = n.min(queue.len());
        Ok(queue.drain(..take).collect())
    }

    /// `QueueLen`: items in the (single, default) partition.
    pub(crate) fn queue_len(&self, queue_id: &str) -> Result<usize, ()> {
        Ok(self.queues.get(queue_id).ok_or(())?.len())
    }

    /// `QueueClear`: drop all items (the queue object survives).
    pub(crate) fn queue_clear(&mut self, queue_id: &str) -> Result<(), ()> {
        self.queues.get_mut(queue_id).ok_or(())?.clear();
        Ok(())
    }
}
