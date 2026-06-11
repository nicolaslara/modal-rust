//! The mock servicer: `impl ModalClient` for [`MockServicer`].
//!
//! Hand-writes the ~18 RPCs the SDK actually calls on the deploy / call / remote
//! flow — each RECORDS its request into the shared [`RequestLog`], runs a per-test
//! override closure if one is configured, else returns a DETERMINISTIC happy-path
//! default. Every OTHER RPC (the ~183 the SDK never touches) is stubbed as
//! `Status::unimplemented` via [`mock_unimplemented!`](crate::macros).
//!
//! Determinism: all ids are fixed (`ap-1`, `im-1`, `fu-1`, …) — no `Date`, no
//! random. Mount/volume ids increment per call so a multi-mount manifest gets
//! distinct ids (`mo-1`, `mo-2`, …).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tonic::{Request, Response, Status};

use crate::macros::mock_unimplemented;
use crate::proto::api as gen;
use crate::proto::api::modal_client_server::ModalClient;
use crate::record::{RecordedRequest, RequestLog};
use crate::responder::Responses;
use crate::store::ObjectStore;

/// How often the mock's server-side-blocking `QueueGet` re-polls its store while
/// the request's `timeout` window is open (the real server blocks natively).
const QUEUE_GET_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// The in-process mock backend's gRPC servicer. Cheap to clone (everything shared
/// is behind an `Arc`), so the running server task and the test handle observe the
/// same [`RequestLog`].
#[derive(Clone)]
pub(crate) struct MockServicer {
    /// Shared, typed request log — the test queries this through [`crate::MockModal`].
    log: RequestLog,
    /// Per-test response config (happy-path defaults + override closures).
    responses: Arc<Responses>,
    /// Monotonic counter for incrementing ids (mounts, volumes) so a manifest with
    /// several mounts records distinct `mo-{n}` ids.
    counter: Arc<AtomicU64>,
    /// STATEFUL Dict/Queue backing store (real put→get round-trips offline).
    /// Locks are short and never held across an await.
    store: Arc<Mutex<ObjectStore>>,
}

impl MockServicer {
    pub(crate) fn new(log: RequestLog, responses: Responses) -> Self {
        MockServicer {
            log,
            responses: Arc::new(responses),
            counter: Arc::new(AtomicU64::new(0)),
            store: Arc::new(Mutex::new(ObjectStore::default())),
        }
    }

    /// Next monotonic id ordinal (1-based), e.g. for `mo-{n}` / `vo-{n}`.
    fn next_id(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Short-lived lock on the stateful Dict/Queue store (never held across an
    /// await; a poisoned lock is a test-infra bug worth a loud panic).
    fn store_lock(&self) -> std::sync::MutexGuard<'_, ObjectStore> {
        self.store.lock().expect("object store poisoned")
    }
}

/// The not-found `Status` an op on an unknown/deleted object id gets (the real
/// server's behavior for a stale id).
fn unknown_id(kind: &str, id: &str) -> Status {
    Status::not_found(format!("{kind} with id '{id}' not found"))
}

/// Pull the raw `input_json` string out of the inbound CBOR `(args, kwargs)` an
/// invoke sends. The facade encodes `args = (entrypoint, input_json)` (a 2-tuple)
/// and `kwargs = {}` (`app.rs` `remote_invoke`), so the decoded shape is
/// `((String, String), BTreeMap)`. We decode just `args` (a `(String, String)`)
/// and return its second element. On any decode mismatch, fall back to `"null"`
/// so the echo/body still produces a valid envelope.
fn decode_input_json(args_bytes: &[u8]) -> String {
    // kwargs is an arbitrary map; decode it as an ignored CBOR `Value` so the tuple
    // shape matches regardless of the (empty) kwargs contents. `args` is the
    // `(entrypoint, input_json)` 2-tuple — we want its second element.
    type Args = (String, String);
    match modal_rust_sdk::codec::decode::<(Args, serde_cbor::Value)>(args_bytes) {
        Ok(((_, input_json), _)) => input_json,
        Err(_) => "null".to_string(),
    }
}

#[tonic::async_trait]
impl ModalClient for MockServicer {
    // ---------- HAND-WRITTEN: the RPCs the SDK calls on deploy/call/remote ----------

    /// `from_config` issues a `ClientHello` on connect — must succeed for the dial
    /// to complete. Recorded as a presence marker.
    async fn client_hello(
        &self,
        _request: Request<()>,
    ) -> Result<Response<gen::ClientHelloResponse>, Status> {
        self.log.push(RecordedRequest::ClientHello);
        Ok(Response::new(gen::ClientHelloResponse::default()))
    }

    /// Resolve (or create) an app → deterministic `ap-1`.
    async fn app_get_or_create(
        &self,
        request: Request<gen::AppGetOrCreateRequest>,
    ) -> Result<Response<gen::AppGetOrCreateResponse>, Status> {
        let req = request.into_inner();
        self.log.push(RecordedRequest::AppGetOrCreate(req.clone()));
        if let Some(f) = &self.responses.on_app_get_or_create {
            return f(&req).map(Response::new);
        }
        Ok(Response::new(gen::AppGetOrCreateResponse {
            app_id: "ap-1".to_string(),
        }))
    }

    /// Create an EPHEMERAL app (the RUN path) → deterministic `ap-1`.
    async fn app_create(
        &self,
        request: Request<gen::AppCreateRequest>,
    ) -> Result<Response<gen::AppCreateResponse>, Status> {
        let req = request.into_inner();
        self.log.push(RecordedRequest::AppCreate(req.clone()));
        if let Some(f) = &self.responses.on_app_create {
            return f(&req).map(Response::new);
        }
        Ok(Response::new(gen::AppCreateResponse {
            app_id: "ap-1".to_string(),
            ..Default::default()
        }))
    }

    /// Publish the app's functions (EPHEMERAL on RUN, DEPLOYED on deploy). Echoes
    /// an empty success.
    async fn app_publish(
        &self,
        request: Request<gen::AppPublishRequest>,
    ) -> Result<Response<gen::AppPublishResponse>, Status> {
        let req = request.into_inner();
        self.log.push(RecordedRequest::AppPublish(req));
        Ok(Response::new(gen::AppPublishResponse::default()))
    }

    /// Resolve the environment's settings. Defaults the image builder version to a
    /// MODERN value (`2025.06`) so the SDK's `mount_client_dependencies` gate stays
    /// consistent (the worker mounts the client dep closure for builder > 2024.10).
    async fn environment_get_or_create(
        &self,
        request: Request<gen::EnvironmentGetOrCreateRequest>,
    ) -> Result<Response<gen::EnvironmentGetOrCreateResponse>, Status> {
        let req = request.into_inner();
        self.log
            .push(RecordedRequest::EnvironmentGetOrCreate(req.clone()));
        if let Some(f) = &self.responses.on_environment_get_or_create {
            return f(&req).map(Response::new);
        }
        Ok(Response::new(gen::EnvironmentGetOrCreateResponse {
            environment_id: "en-1".to_string(),
            metadata: Some(gen::EnvironmentMetadata {
                name: req.deployment_name.clone(),
                settings: Some(gen::EnvironmentSettings {
                    image_builder_version: "2025.06".to_string(),
                    ..Default::default()
                }),
            }),
        }))
    }

    /// Single-part blob create → `bl-1`. (Small example mounts never reach here —
    /// `mount_put_file` returns `exists=true` — but it is wired for completeness.)
    async fn blob_create(
        &self,
        request: Request<gen::BlobCreateRequest>,
    ) -> Result<Response<gen::BlobCreateResponse>, Status> {
        let req = request.into_inner();
        self.log.push(RecordedRequest::BlobCreate(req));
        Ok(Response::new(gen::BlobCreateResponse {
            blob_id: "bl-1".to_string(),
            ..Default::default()
        }))
    }

    /// Resolve (or create) a mount → an incrementing `mo-{n}` id. Covers the hosted
    /// client mount, the python-standalone mount, and the uploaded source mount.
    async fn mount_get_or_create(
        &self,
        request: Request<gen::MountGetOrCreateRequest>,
    ) -> Result<Response<gen::MountGetOrCreateResponse>, Status> {
        let req = request.into_inner();
        self.log.push(RecordedRequest::MountGetOrCreate(req));
        Ok(Response::new(gen::MountGetOrCreateResponse {
            mount_id: format!("mo-{}", self.next_id()),
            ..Default::default()
        }))
    }

    /// Per-file upload probe/PUT. ALWAYS reports `exists=true` so the SDK's source
    /// upload short-circuits — no file bytes and no `blob_create` are needed for
    /// the example mounts (the v1 inline-only stance).
    async fn mount_put_file(
        &self,
        request: Request<gen::MountPutFileRequest>,
    ) -> Result<Response<gen::MountPutFileResponse>, Status> {
        let req = request.into_inner();
        self.log.push(RecordedRequest::MountPutFile(req));
        Ok(Response::new(gen::MountPutFileResponse { exists: true }))
    }

    /// Get-or-create an image → `im-{n}` with an INLINE `result.status = SUCCESS`,
    /// so the SDK short-circuits and never opens `ImageJoinStreaming`.
    async fn image_get_or_create(
        &self,
        request: Request<gen::ImageGetOrCreateRequest>,
    ) -> Result<Response<gen::ImageGetOrCreateResponse>, Status> {
        let req = request.into_inner();
        self.log
            .push(RecordedRequest::ImageGetOrCreate(req.clone()));
        if let Some(f) = &self.responses.on_image_get_or_create {
            return f(&req).map(Response::new);
        }
        Ok(Response::new(gen::ImageGetOrCreateResponse {
            image_id: format!("im-{}", self.next_id()),
            result: Some(gen::GenericResult {
                status: gen::generic_result::GenericStatus::Success as i32,
                ..Default::default()
            }),
            ..Default::default()
        }))
    }

    /// Precreate a function → a NON-EMPTY `fu-pre-1` (the SDK errors on empty).
    async fn function_precreate(
        &self,
        request: Request<gen::FunctionPrecreateRequest>,
    ) -> Result<Response<gen::FunctionPrecreateResponse>, Status> {
        let req = request.into_inner();
        self.log
            .push(RecordedRequest::FunctionPrecreate(req.clone()));
        if let Some(f) = &self.responses.on_function_precreate {
            return f(&req).map(Response::new);
        }
        Ok(Response::new(gen::FunctionPrecreateResponse {
            function_id: "fu-pre-1".to_string(),
            ..Default::default()
        }))
    }

    /// Create the FILE-mode function → `fu-1` + a non-empty `definition_id` (`de-1`)
    /// in `handle_metadata` (both required non-empty by the SDK).
    async fn function_create(
        &self,
        request: Request<gen::FunctionCreateRequest>,
    ) -> Result<Response<gen::FunctionCreateResponse>, Status> {
        let req = request.into_inner();
        self.log.push(RecordedRequest::FunctionCreate(req.clone()));
        if let Some(f) = &self.responses.on_function_create {
            return f(&req).map(Response::new);
        }
        Ok(Response::new(gen::FunctionCreateResponse {
            function_id: "fu-1".to_string(),
            handle_metadata: Some(gen::FunctionHandleMetadata {
                definition_id: "de-1".to_string(),
                function_name: req
                    .function
                    .as_ref()
                    .map(|f| f.function_name.clone())
                    .unwrap_or_default(),
                ..Default::default()
            }),
            ..Default::default()
        }))
    }

    /// Resolve a DEPLOYED function by name (the `call` path) → `fu-1`.
    async fn function_get(
        &self,
        request: Request<gen::FunctionGetRequest>,
    ) -> Result<Response<gen::FunctionGetResponse>, Status> {
        let req = request.into_inner();
        self.log.push(RecordedRequest::FunctionGet(req.clone()));
        if let Some(f) = &self.responses.on_function_get {
            return f(&req).map(Response::new);
        }
        Ok(Response::new(gen::FunctionGetResponse {
            function_id: "fu-1".to_string(),
            ..Default::default()
        }))
    }

    /// Invoke step 1: open a call → `fc-1`, ECHOING each pipelined input back so the
    /// SDK skips the fix-#3 `FunctionPutInputs` fallback and goes straight to poll.
    async fn function_map(
        &self,
        request: Request<gen::FunctionMapRequest>,
    ) -> Result<Response<gen::FunctionMapResponse>, Status> {
        let req = request.into_inner();
        if let Some(f) = &self.responses.on_function_map {
            let resp = f(&req)?;
            self.log.push(RecordedRequest::FunctionMap(req));
            return Ok(Response::new(resp));
        }
        let pipelined = req
            .pipelined_inputs
            .iter()
            .map(|item| gen::FunctionPutInputsResponseItem {
                idx: item.idx,
                input_id: format!("in-{}", item.idx),
                input_jwt: String::new(),
            })
            .collect();
        self.log.push(RecordedRequest::FunctionMap(req));
        Ok(Response::new(gen::FunctionMapResponse {
            function_call_id: "fc-1".to_string(),
            pipelined_inputs: pipelined,
            ..Default::default()
        }))
    }

    /// Invoke step 2 (fix-#3 fallback / map path): accept the inputs → echo one
    /// non-empty accepted item per input (the SDK errors on empty).
    async fn function_put_inputs(
        &self,
        request: Request<gen::FunctionPutInputsRequest>,
    ) -> Result<Response<gen::FunctionPutInputsResponse>, Status> {
        let req = request.into_inner();
        let inputs = req
            .inputs
            .iter()
            .map(|item| gen::FunctionPutInputsResponseItem {
                idx: item.idx,
                input_id: format!("in-{}", item.idx),
                input_jwt: String::new(),
            })
            .collect();
        self.log.push(RecordedRequest::FunctionPutInputs(req));
        Ok(Response::new(gen::FunctionPutInputsResponse { inputs }))
    }

    /// Invoke step 3: return ONE terminal SUCCESS output whose data is the CBOR of
    /// the runner ENVELOPE STRING (`R = String`) — exactly what `.remote()` decodes.
    /// The envelope content comes from the configured [`Responses`] (echo the input
    /// by default; or a canned value / closure-computed body / verbatim envelope).
    async fn function_get_outputs(
        &self,
        request: Request<gen::FunctionGetOutputsRequest>,
    ) -> Result<Response<gen::FunctionGetOutputsResponse>, Status> {
        let req = request.into_inner();
        self.log
            .push(RecordedRequest::FunctionGetOutputs(req.clone()));
        if let Some(f) = &self.responses.on_function_get_outputs {
            return f(&req).map(Response::new);
        }

        // The decoded input the test's body/echo sees comes from the FunctionMap
        // input pipeline. We don't have the original args here (this is the poll
        // RPC), so the default echo/value/body envelope is computed from the
        // recorded FunctionMap input if present, else "null".
        let input_json = self.last_invoked_input_json();
        let envelope = self.responses.envelope_for(&input_json);
        let cbor = modal_rust_sdk::codec::encode(&envelope)
            .map_err(|e| Status::internal(format!("mock: failed to encode envelope: {e}")))?;

        let item = gen::FunctionGetOutputsItem {
            idx: 0,
            data_format: gen::DataFormat::Cbor as i32,
            result: Some(gen::GenericResult {
                status: gen::generic_result::GenericStatus::Success as i32,
                data_oneof: Some(gen::generic_result::DataOneof::Data(cbor)),
                ..Default::default()
            }),
            ..Default::default()
        };
        Ok(Response::new(gen::FunctionGetOutputsResponse {
            outputs: vec![item],
            last_entry_id: "1-0".to_string(),
            num_unfinished_inputs: 0,
            ..Default::default()
        }))
    }

    /// Resolve (or create) a Secret by name → `sc-1`.
    async fn secret_get_or_create(
        &self,
        request: Request<gen::SecretGetOrCreateRequest>,
    ) -> Result<Response<gen::SecretGetOrCreateResponse>, Status> {
        let req = request.into_inner();
        self.log
            .push(RecordedRequest::SecretGetOrCreate(req.clone()));
        if let Some(f) = &self.responses.on_secret_get_or_create {
            return f(&req).map(Response::new);
        }
        Ok(Response::new(gen::SecretGetOrCreateResponse {
            secret_id: "sc-1".to_string(),
            ..Default::default()
        }))
    }

    /// Resolve (or create) a Volume by name → an incrementing `vo-{n}` id.
    async fn volume_get_or_create(
        &self,
        request: Request<gen::VolumeGetOrCreateRequest>,
    ) -> Result<Response<gen::VolumeGetOrCreateResponse>, Status> {
        let req = request.into_inner();
        self.log
            .push(RecordedRequest::VolumeGetOrCreate(req.clone()));
        if let Some(f) = &self.responses.on_volume_get_or_create {
            return f(&req).map(Response::new);
        }
        Ok(Response::new(gen::VolumeGetOrCreateResponse {
            volume_id: format!("vo-{}", self.next_id()),
            ..Default::default()
        }))
    }

    // ---------- HAND-WRITTEN + STATEFUL: the Dict/Queue v0 RPCs ----------
    //
    // These arms back real state transitions against the shared [`ObjectStore`]
    // (in-memory BTreeMap per dict / VecDeque per queue), so offline tests do
    // genuine put→get round-trips through the facade handles. Unknown ids and
    // pure-lookup misses surface as `Status::not_found`, mirroring the server.

    /// `DictGetOrCreate`: named resolve. CREATE_IF_MISSING is idempotent (same
    /// name → same `di-{n}` id); UNSPECIFIED ("just lookup") not-founds on a miss.
    async fn dict_get_or_create(
        &self,
        request: Request<gen::DictGetOrCreateRequest>,
    ) -> Result<Response<gen::DictGetOrCreateResponse>, Status> {
        let req = request.into_inner();
        self.log.push(RecordedRequest::DictGetOrCreate(req.clone()));
        if let Some(f) = &self.responses.on_dict_get_or_create {
            return f(&req).map(Response::new);
        }
        let create = req.object_creation_type != gen::ObjectCreationType::Unspecified as i32;
        let candidate = format!("di-{}", self.next_id());
        let resolved = self
            .store_lock()
            .resolve_dict(&req.deployment_name, create, candidate);
        match resolved {
            Some(dict_id) => Ok(Response::new(gen::DictGetOrCreateResponse {
                dict_id,
                ..Default::default()
            })),
            None => Err(Status::not_found(format!(
                "Dict '{}' not found",
                req.deployment_name
            ))),
        }
    }

    /// `DictGet`: byte-equality lookup → `{found, value}`.
    async fn dict_get(
        &self,
        request: Request<gen::DictGetRequest>,
    ) -> Result<Response<gen::DictGetResponse>, Status> {
        let req = request.into_inner();
        self.log.push(RecordedRequest::DictGet(req.clone()));
        let value = self
            .store_lock()
            .dict_get(&req.dict_id, &req.key)
            .map_err(|()| unknown_id("Dict", &req.dict_id))?;
        Ok(Response::new(gen::DictGetResponse {
            found: value.is_some(),
            value,
        }))
    }

    /// `DictUpdate`: put / put-if-absent / batch are all this RPC. `created`
    /// reports whether the entry was actually inserted (the `put_if_absent` flag).
    async fn dict_update(
        &self,
        request: Request<gen::DictUpdateRequest>,
    ) -> Result<Response<gen::DictUpdateResponse>, Status> {
        let req = request.into_inner();
        self.log.push(RecordedRequest::DictUpdate(req.clone()));
        let entries = req.updates.iter().map(|e| (e.key.clone(), e.value.clone()));
        let created = self
            .store_lock()
            .dict_update(&req.dict_id, entries, req.if_not_exists)
            .map_err(|()| unknown_id("Dict", &req.dict_id))?;
        Ok(Response::new(gen::DictUpdateResponse { created }))
    }

    /// `DictPop`: remove + return → `{found, value}`.
    async fn dict_pop(
        &self,
        request: Request<gen::DictPopRequest>,
    ) -> Result<Response<gen::DictPopResponse>, Status> {
        let req = request.into_inner();
        self.log.push(RecordedRequest::DictPop(req.clone()));
        let value = self
            .store_lock()
            .dict_pop(&req.dict_id, &req.key)
            .map_err(|()| unknown_id("Dict", &req.dict_id))?;
        Ok(Response::new(gen::DictPopResponse {
            found: value.is_some(),
            value,
        }))
    }

    /// `DictContains`: byte-equality presence.
    async fn dict_contains(
        &self,
        request: Request<gen::DictContainsRequest>,
    ) -> Result<Response<gen::DictContainsResponse>, Status> {
        let req = request.into_inner();
        self.log.push(RecordedRequest::DictContains(req.clone()));
        let found = self
            .store_lock()
            .dict_contains(&req.dict_id, &req.key)
            .map_err(|()| unknown_id("Dict", &req.dict_id))?;
        Ok(Response::new(gen::DictContainsResponse { found }))
    }

    /// `DictLen`: entry count (`int32` on the wire, like the real server).
    async fn dict_len(
        &self,
        request: Request<gen::DictLenRequest>,
    ) -> Result<Response<gen::DictLenResponse>, Status> {
        let req = request.into_inner();
        self.log.push(RecordedRequest::DictLen(req.clone()));
        let len = self
            .store_lock()
            .dict_len(&req.dict_id)
            .map_err(|()| unknown_id("Dict", &req.dict_id))?;
        Ok(Response::new(gen::DictLenResponse {
            len: i32::try_from(len).unwrap_or(i32::MAX),
        }))
    }

    /// `DictClear`: drop all entries (the object survives).
    async fn dict_clear(
        &self,
        request: Request<gen::DictClearRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        self.log.push(RecordedRequest::DictClear(req.clone()));
        self.store_lock()
            .dict_clear(&req.dict_id)
            .map_err(|()| unknown_id("Dict", &req.dict_id))?;
        Ok(Response::new(()))
    }

    /// `DictDelete`: delete the Dict OBJECT (id + name mapping).
    async fn dict_delete(
        &self,
        request: Request<gen::DictDeleteRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        self.log.push(RecordedRequest::DictDelete(req.clone()));
        if !self.store_lock().delete_dict(&req.dict_id) {
            return Err(unknown_id("Dict", &req.dict_id));
        }
        Ok(Response::new(()))
    }

    /// `QueueGetOrCreate`: named resolve — same lifecycle contract as the dict arm.
    async fn queue_get_or_create(
        &self,
        request: Request<gen::QueueGetOrCreateRequest>,
    ) -> Result<Response<gen::QueueGetOrCreateResponse>, Status> {
        let req = request.into_inner();
        self.log
            .push(RecordedRequest::QueueGetOrCreate(req.clone()));
        if let Some(f) = &self.responses.on_queue_get_or_create {
            return f(&req).map(Response::new);
        }
        let create = req.object_creation_type != gen::ObjectCreationType::Unspecified as i32;
        let candidate = format!("qu-{}", self.next_id());
        let resolved = self
            .store_lock()
            .resolve_queue(&req.deployment_name, create, candidate);
        match resolved {
            Some(queue_id) => Ok(Response::new(gen::QueueGetOrCreateResponse {
                queue_id,
                ..Default::default()
            })),
            None => Err(Status::not_found(format!(
                "Queue '{}' not found",
                req.deployment_name
            ))),
        }
    }

    /// `QueuePut`: append `values` in order (put / put_many are the same RPC;
    /// default partition only — v0 always sends an empty `partition_key`).
    async fn queue_put(
        &self,
        request: Request<gen::QueuePutRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        self.log.push(RecordedRequest::QueuePut(req.clone()));
        self.store_lock()
            .queue_put(&req.queue_id, req.values.clone())
            .map_err(|()| unknown_id("Queue", &req.queue_id))?;
        Ok(Response::new(()))
    }

    /// `QueueGet`: pop up to `n_values` FIFO, honoring the SERVER-side blocking
    /// `timeout` window like the real backend — poll the store every
    /// [`QUEUE_GET_POLL_INTERVAL`] until something is available or the window
    /// closes (empty response = timed out). The lock is re-taken per tick, so a
    /// concurrent `QueuePut` from another connection gets through mid-wait.
    async fn queue_get(
        &self,
        request: Request<gen::QueueGetRequest>,
    ) -> Result<Response<gen::QueueGetResponse>, Status> {
        let req = request.into_inner();
        self.log.push(RecordedRequest::QueueGet(req.clone()));
        let n = usize::try_from(req.n_values.max(1)).unwrap_or(1);
        let deadline = Instant::now() + Duration::from_secs_f32(req.timeout.max(0.0));
        loop {
            let values = self
                .store_lock()
                .queue_pop(&req.queue_id, n)
                .map_err(|()| unknown_id("Queue", &req.queue_id))?;
            if !values.is_empty() || Instant::now() >= deadline {
                return Ok(Response::new(gen::QueueGetResponse { values }));
            }
            tokio::time::sleep(QUEUE_GET_POLL_INTERVAL).await;
        }
    }

    /// `QueueLen`: item count (single default partition, so `total` is moot).
    async fn queue_len(
        &self,
        request: Request<gen::QueueLenRequest>,
    ) -> Result<Response<gen::QueueLenResponse>, Status> {
        let req = request.into_inner();
        self.log.push(RecordedRequest::QueueLen(req.clone()));
        let len = self
            .store_lock()
            .queue_len(&req.queue_id)
            .map_err(|()| unknown_id("Queue", &req.queue_id))?;
        Ok(Response::new(gen::QueueLenResponse {
            len: i32::try_from(len).unwrap_or(i32::MAX),
        }))
    }

    /// `QueueClear`: drop all items (the object survives).
    async fn queue_clear(
        &self,
        request: Request<gen::QueueClearRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        self.log.push(RecordedRequest::QueueClear(req.clone()));
        self.store_lock()
            .queue_clear(&req.queue_id)
            .map_err(|()| unknown_id("Queue", &req.queue_id))?;
        Ok(Response::new(()))
    }

    /// `QueueDelete`: delete the Queue OBJECT (id + name mapping).
    async fn queue_delete(
        &self,
        request: Request<gen::QueueDeleteRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        self.log.push(RecordedRequest::QueueDelete(req.clone()));
        if !self.store_lock().delete_queue(&req.queue_id) {
            return Err(unknown_id("Queue", &req.queue_id));
        }
        Ok(Response::new(()))
    }

    /// The one server-streaming RPC our flow can touch. The happy path never reaches
    /// it (image get-or-create short-circuits on inline success), but it is wired as
    /// a concrete boxed stream yielding a single terminal SUCCESS so a streaming RPC
    /// is implementable, not just stubbable.
    type ImageJoinStreamingStream = std::pin::Pin<
        Box<
            dyn tokio_stream::Stream<Item = Result<gen::ImageJoinStreamingResponse, Status>>
                + Send
                + 'static,
        >,
    >;
    async fn image_join_streaming(
        &self,
        _request: Request<gen::ImageJoinStreamingRequest>,
    ) -> Result<Response<Self::ImageJoinStreamingStream>, Status> {
        let terminal = gen::ImageJoinStreamingResponse {
            result: Some(gen::GenericResult {
                status: gen::generic_result::GenericStatus::Success as i32,
                ..Default::default()
            }),
            ..Default::default()
        };
        let s = tokio_stream::once(Ok(terminal));
        Ok(Response::new(Box::pin(s)))
    }

    // ---------- EVERYTHING ELSE: the unused RPCs, via the macro stub ----------
    mock_unimplemented! {
    // ---- unary RPCs ----
    unary app_client_disconnect(gen::AppClientDisconnectRequest) -> ();
    unary app_count_logs(gen::AppCountLogsRequest) -> gen::AppCountLogsResponse;
    unary app_deploy(gen::AppDeployRequest) -> gen::AppDeployResponse;
    unary app_deployment_history(gen::AppDeploymentHistoryRequest) -> gen::AppDeploymentHistoryResponse;
    unary app_fetch_logs(gen::AppFetchLogsRequest) -> gen::AppFetchLogsResponse;
    unary app_get_by_deployment_name(gen::AppGetByDeploymentNameRequest) -> gen::AppGetByDeploymentNameResponse;
    unary app_get_layout(gen::AppGetLayoutRequest) -> gen::AppGetLayoutResponse;
    unary app_get_lifecycle(gen::AppGetLifecycleRequest) -> gen::AppGetLifecycleResponse;
    unary app_get_objects(gen::AppGetObjectsRequest) -> gen::AppGetObjectsResponse;
    unary app_get_tags(gen::AppGetTagsRequest) -> gen::AppGetTagsResponse;
    unary app_heartbeat(gen::AppHeartbeatRequest) -> ();
    unary app_list(gen::AppListRequest) -> gen::AppListResponse;
    unary app_lookup(gen::AppLookupRequest) -> gen::AppLookupResponse;
    unary app_rollback(gen::AppRollbackRequest) -> ();
    unary app_rollover(gen::AppRolloverRequest) -> gen::AppRolloverResponse;
    unary app_set_objects(gen::AppSetObjectsRequest) -> ();
    unary app_set_tags(gen::AppSetTagsRequest) -> ();
    unary app_stop(gen::AppStopRequest) -> ();
    unary attempt_await(gen::AttemptAwaitRequest) -> gen::AttemptAwaitResponse;
    unary attempt_retry(gen::AttemptRetryRequest) -> gen::AttemptRetryResponse;
    unary attempt_start(gen::AttemptStartRequest) -> gen::AttemptStartResponse;
    unary auth_token_get(gen::AuthTokenGetRequest) -> gen::AuthTokenGetResponse;
    unary blob_get(gen::BlobGetRequest) -> gen::BlobGetResponse;
    unary class_create(gen::ClassCreateRequest) -> gen::ClassCreateResponse;
    unary class_get(gen::ClassGetRequest) -> gen::ClassGetResponse;
    unary cluster_get(gen::ClusterGetRequest) -> gen::ClusterGetResponse;
    unary cluster_list(gen::ClusterListRequest) -> gen::ClusterListResponse;
    unary container_checkpoint(gen::ContainerCheckpointRequest) -> ();
    unary container_exec(gen::ContainerExecRequest) -> gen::ContainerExecResponse;
    unary container_exec_put_input(gen::ContainerExecPutInputRequest) -> ();
    unary container_exec_wait(gen::ContainerExecWaitRequest) -> gen::ContainerExecWaitResponse;
    unary container_filesystem_exec(gen::ContainerFilesystemExecRequest) -> gen::ContainerFilesystemExecResponse;
    unary container_heartbeat(gen::ContainerHeartbeatRequest) -> gen::ContainerHeartbeatResponse;
    unary container_hello(()) -> ();
    unary container_log(gen::ContainerLogRequest) -> ();
    unary container_reload_volumes(gen::ContainerReloadVolumesRequest) -> gen::ContainerReloadVolumesResponse;
    unary container_stop(gen::ContainerStopRequest) -> gen::ContainerStopResponse;
    unary dict_get_by_id(gen::DictGetByIdRequest) -> gen::DictGetByIdResponse;
    unary dict_heartbeat(gen::DictHeartbeatRequest) -> ();
    unary dict_list(gen::DictListRequest) -> gen::DictListResponse;
    unary domain_certificate_verify(gen::DomainCertificateVerifyRequest) -> gen::DomainCertificateVerifyResponse;
    unary domain_create(gen::DomainCreateRequest) -> gen::DomainCreateResponse;
    unary domain_list(gen::DomainListRequest) -> gen::DomainListResponse;
    unary endpoint_create(gen::EndpointCreateRequest) -> gen::EndpointCreateResponse;
    unary endpoint_list(gen::EndpointListRequest) -> gen::EndpointListResponse;
    unary endpoint_stop(gen::EndpointStopRequest) -> gen::EndpointStopResponse;
    unary environment_create(gen::EnvironmentCreateRequest) -> ();
    unary environment_delete(gen::EnvironmentDeleteRequest) -> ();
    unary environment_get_managed(gen::EnvironmentGetManagedRequest) -> gen::EnvironmentGetManagedResponse;
    unary environment_list(()) -> gen::EnvironmentListResponse;
    unary environment_role_set(gen::EnvironmentRoleSetRequest) -> ();
    unary environment_set_managed(gen::EnvironmentSetManagedRequest) -> ();
    unary environment_update(gen::EnvironmentUpdateRequest) -> gen::EnvironmentListItem;
    unary flash_container_deregister(gen::FlashContainerDeregisterRequest) -> ();
    unary flash_container_list(gen::FlashContainerListRequest) -> gen::FlashContainerListResponse;
    unary flash_container_register(gen::FlashContainerRegisterRequest) -> gen::FlashContainerRegisterResponse;
    unary flash_set_target_slots_metrics(gen::FlashSetTargetSlotsMetricsRequest) -> gen::FlashSetTargetSlotsMetricsResponse;
    unary function_async_invoke(gen::FunctionAsyncInvokeRequest) -> gen::FunctionAsyncInvokeResponse;
    unary function_bind_params(gen::FunctionBindParamsRequest) -> gen::FunctionBindParamsResponse;
    unary function_call_cancel(gen::FunctionCallCancelRequest) -> ();
    unary function_call_from_id(gen::FunctionCallFromIdRequest) -> gen::FunctionCallFromIdResponse;
    unary function_call_list(gen::FunctionCallListRequest) -> gen::FunctionCallListResponse;
    unary function_call_put_data_out(gen::FunctionCallPutDataRequest) -> ();
    unary function_finish_inputs(gen::FunctionFinishInputsRequest) -> ();
    unary function_get_call_graph(gen::FunctionGetCallGraphRequest) -> gen::FunctionGetCallGraphResponse;
    unary function_get_current_stats(gen::FunctionGetCurrentStatsRequest) -> gen::FunctionStats;
    unary function_get_dynamic_concurrency(gen::FunctionGetDynamicConcurrencyRequest) -> gen::FunctionGetDynamicConcurrencyResponse;
    unary function_get_inputs(gen::FunctionGetInputsRequest) -> gen::FunctionGetInputsResponse;
    unary function_get_serialized(gen::FunctionGetSerializedRequest) -> gen::FunctionGetSerializedResponse;
    unary function_put_outputs(gen::FunctionPutOutputsRequest) -> ();
    unary function_retry_inputs(gen::FunctionRetryInputsRequest) -> gen::FunctionRetryInputsResponse;
    unary function_start_pty_shell(()) -> ();
    unary function_update_scheduling_params(gen::FunctionUpdateSchedulingParamsRequest) -> gen::FunctionUpdateSchedulingParamsResponse;
    unary image_delete(gen::ImageDeleteRequest) -> ();
    unary image_from_id(gen::ImageFromIdRequest) -> gen::ImageFromIdResponse;
    unary map_await(gen::MapAwaitRequest) -> gen::MapAwaitResponse;
    unary map_check_inputs(gen::MapCheckInputsRequest) -> gen::MapCheckInputsResponse;
    unary map_start_or_continue(gen::MapStartOrContinueRequest) -> gen::MapStartOrContinueResponse;
    unary notebook_kernel_publish_results(gen::NotebookKernelPublishResultsRequest) -> ();
    unary proxy_add_ip(gen::ProxyAddIpRequest) -> gen::ProxyAddIpResponse;
    unary proxy_create(gen::ProxyCreateRequest) -> gen::ProxyCreateResponse;
    unary proxy_delete(gen::ProxyDeleteRequest) -> ();
    unary proxy_get(gen::ProxyGetRequest) -> gen::ProxyGetResponse;
    unary proxy_get_or_create(gen::ProxyGetOrCreateRequest) -> gen::ProxyGetOrCreateResponse;
    unary proxy_list(()) -> gen::ProxyListResponse;
    unary proxy_remove_ip(gen::ProxyRemoveIpRequest) -> ();
    unary queue_get_by_id(gen::QueueGetByIdRequest) -> gen::QueueGetByIdResponse;
    unary queue_heartbeat(gen::QueueHeartbeatRequest) -> ();
    unary queue_list(gen::QueueListRequest) -> gen::QueueListResponse;
    unary queue_next_items(gen::QueueNextItemsRequest) -> gen::QueueNextItemsResponse;
    unary sandbox_create(gen::SandboxCreateRequest) -> gen::SandboxCreateResponse;
    unary sandbox_create_connect_token(gen::SandboxCreateConnectTokenRequest) -> gen::SandboxCreateConnectTokenResponse;
    unary sandbox_create_v2(gen::SandboxCreateV2Request) -> gen::SandboxCreateV2Response;
    unary sandbox_get_command_router_access(gen::SandboxGetCommandRouterAccessRequest) -> gen::SandboxGetCommandRouterAccessResponse;
    unary sandbox_get_from_name(gen::SandboxGetFromNameRequest) -> gen::SandboxGetFromNameResponse;
    unary sandbox_get_resource_usage(gen::SandboxGetResourceUsageRequest) -> gen::SandboxGetResourceUsageResponse;
    unary sandbox_get_task_id(gen::SandboxGetTaskIdRequest) -> gen::SandboxGetTaskIdResponse;
    unary sandbox_get_task_id_v2(gen::SandboxGetTaskIdRequest) -> gen::SandboxGetTaskIdResponse;
    unary sandbox_get_tunnels(gen::SandboxGetTunnelsRequest) -> gen::SandboxGetTunnelsResponse;
    unary sandbox_get_tunnels_v2(gen::SandboxGetTunnelsRequest) -> gen::SandboxGetTunnelsResponse;
    unary sandbox_list(gen::SandboxListRequest) -> gen::SandboxListResponse;
    unary sandbox_restore(gen::SandboxRestoreRequest) -> gen::SandboxRestoreResponse;
    unary sandbox_snapshot(gen::SandboxSnapshotRequest) -> gen::SandboxSnapshotResponse;
    unary sandbox_snapshot_fs(gen::SandboxSnapshotFsRequest) -> gen::SandboxSnapshotFsResponse;
    unary sandbox_snapshot_fs_async(gen::SandboxSnapshotFsAsyncRequest) -> gen::SandboxSnapshotFsAsyncResponse;
    unary sandbox_snapshot_fs_async_get(gen::SandboxSnapshotFsAsyncGetRequest) -> gen::SandboxSnapshotFsResponse;
    unary sandbox_snapshot_get(gen::SandboxSnapshotGetRequest) -> gen::SandboxSnapshotGetResponse;
    unary sandbox_snapshot_wait(gen::SandboxSnapshotWaitRequest) -> gen::SandboxSnapshotWaitResponse;
    unary sandbox_stdin_write(gen::SandboxStdinWriteRequest) -> gen::SandboxStdinWriteResponse;
    unary sandbox_tags_get(gen::SandboxTagsGetRequest) -> gen::SandboxTagsGetResponse;
    unary sandbox_tags_set(gen::SandboxTagsSetRequest) -> ();
    unary sandbox_terminate(gen::SandboxTerminateRequest) -> gen::SandboxTerminateResponse;
    unary sandbox_terminate_v2(gen::SandboxTerminateRequest) -> gen::SandboxTerminateResponse;
    unary sandbox_wait(gen::SandboxWaitRequest) -> gen::SandboxWaitResponse;
    unary sandbox_wait_until_ready(gen::SandboxWaitUntilReadyRequest) -> gen::SandboxWaitUntilReadyResponse;
    unary sandbox_wait_v2(gen::SandboxWaitRequest) -> gen::SandboxWaitResponse;
    unary secret_delete(gen::SecretDeleteRequest) -> ();
    unary secret_list(gen::SecretListRequest) -> gen::SecretListResponse;
    unary secret_update(gen::SecretUpdateRequest) -> ();
    unary service_user_list(()) -> gen::ServiceUserListResponse;
    unary shared_volume_delete(gen::SharedVolumeDeleteRequest) -> ();
    unary shared_volume_get_file(gen::SharedVolumeGetFileRequest) -> gen::SharedVolumeGetFileResponse;
    unary shared_volume_get_or_create(gen::SharedVolumeGetOrCreateRequest) -> gen::SharedVolumeGetOrCreateResponse;
    unary shared_volume_heartbeat(gen::SharedVolumeHeartbeatRequest) -> ();
    unary shared_volume_list(gen::SharedVolumeListRequest) -> gen::SharedVolumeListResponse;
    unary shared_volume_list_files(gen::SharedVolumeListFilesRequest) -> gen::SharedVolumeListFilesResponse;
    unary shared_volume_put_file(gen::SharedVolumePutFileRequest) -> gen::SharedVolumePutFileResponse;
    unary shared_volume_remove_file(gen::SharedVolumeRemoveFileRequest) -> ();
    unary task_cluster_hello(gen::TaskClusterHelloRequest) -> gen::TaskClusterHelloResponse;
    unary task_current_inputs(()) -> gen::TaskCurrentInputsResponse;
    unary task_get_command_router_access(gen::TaskGetCommandRouterAccessRequest) -> gen::TaskGetCommandRouterAccessResponse;
    unary task_get_info(gen::TaskGetInfoRequest) -> gen::TaskGetInfoResponse;
    unary task_list(gen::TaskListRequest) -> gen::TaskListResponse;
    unary task_result(gen::TaskResultRequest) -> ();
    unary template_list(gen::TemplateListRequest) -> gen::TemplateListResponse;
    unary token_flow_create(gen::TokenFlowCreateRequest) -> gen::TokenFlowCreateResponse;
    unary token_flow_wait(gen::TokenFlowWaitRequest) -> gen::TokenFlowWaitResponse;
    unary token_info_get(gen::TokenInfoGetRequest) -> gen::TokenInfoGetResponse;
    unary tunnel_start(gen::TunnelStartRequest) -> gen::TunnelStartResponse;
    unary tunnel_stop(gen::TunnelStopRequest) -> gen::TunnelStopResponse;
    unary volume_commit(gen::VolumeCommitRequest) -> gen::VolumeCommitResponse;
    unary volume_copy_files(gen::VolumeCopyFilesRequest) -> ();
    unary volume_copy_files2(gen::VolumeCopyFiles2Request) -> ();
    unary volume_delete(gen::VolumeDeleteRequest) -> ();
    unary volume_get_by_id(gen::VolumeGetByIdRequest) -> gen::VolumeGetByIdResponse;
    unary volume_get_file(gen::VolumeGetFileRequest) -> gen::VolumeGetFileResponse;
    unary volume_get_file2(gen::VolumeGetFile2Request) -> gen::VolumeGetFile2Response;
    unary volume_heartbeat(gen::VolumeHeartbeatRequest) -> ();
    unary volume_list(gen::VolumeListRequest) -> gen::VolumeListResponse;
    unary volume_put_files(gen::VolumePutFilesRequest) -> ();
    unary volume_put_files2(gen::VolumePutFiles2Request) -> gen::VolumePutFiles2Response;
    unary volume_reload(gen::VolumeReloadRequest) -> ();
    unary volume_remove_file(gen::VolumeRemoveFileRequest) -> ();
    unary volume_remove_file2(gen::VolumeRemoveFile2Request) -> ();
    unary volume_rename(gen::VolumeRenameRequest) -> ();
    unary workspace_dashboard_url_get(gen::WorkspaceDashboardUrlRequest) -> gen::WorkspaceDashboardUrlResponse;
    unary workspace_members_list(()) -> gen::WorkspaceMembersListResponse;
    unary workspace_name_lookup(()) -> gen::WorkspaceNameLookupResponse;
    // ---- server-streaming RPCs ----
    stream app_get_logs[AppGetLogsStream](gen::AppGetLogsRequest) -> gen::TaskLogsBatch;
    stream container_exec_get_output[ContainerExecGetOutputStream](gen::ContainerExecGetOutputRequest) -> gen::RuntimeOutputBatch;
    stream container_filesystem_exec_get_output[ContainerFilesystemExecGetOutputStream](gen::ContainerFilesystemExecGetOutputRequest) -> gen::FilesystemRuntimeOutputBatch;
    stream dict_contents[DictContentsStream](gen::DictContentsRequest) -> gen::DictEntry;
    stream function_call_get_data_in[FunctionCallGetDataInStream](gen::FunctionCallGetDataRequest) -> gen::DataChunk;
    stream function_call_get_data_out[FunctionCallGetDataOutStream](gen::FunctionCallGetDataRequest) -> gen::DataChunk;
    stream sandbox_get_logs[SandboxGetLogsStream](gen::SandboxGetLogsRequest) -> gen::TaskLogsBatch;
    stream shared_volume_list_files_stream[SharedVolumeListFilesStreamStream](gen::SharedVolumeListFilesRequest) -> gen::SharedVolumeListFilesResponse;
    stream volume_list_files[VolumeListFilesStream](gen::VolumeListFilesRequest) -> gen::VolumeListFilesResponse;
    stream volume_list_files2[VolumeListFiles2Stream](gen::VolumeListFiles2Request) -> gen::VolumeListFiles2Response;
    stream workspace_billing_report[WorkspaceBillingReportStream](gen::WorkspaceBillingReportRequest) -> gen::WorkspaceBillingReportItem;
    }
}

impl MockServicer {
    /// Best-effort decode of the most-recently-pipelined invoke input back into its
    /// raw `input_json` string, so the default `function_get_outputs` envelope can
    /// echo / transform the actual input. Reads the last recorded `FunctionMap`
    /// (or `FunctionPutInputs`) request's pipelined args. Returns `"null"` if none.
    fn last_invoked_input_json(&self) -> String {
        for r in self.log.all().into_iter().rev() {
            let args: Option<Vec<u8>> = match r {
                RecordedRequest::FunctionMap(m) => m
                    .pipelined_inputs
                    .into_iter()
                    .find_map(|i| i.input.and_then(invoke_args_bytes)),
                RecordedRequest::FunctionPutInputs(p) => p
                    .inputs
                    .into_iter()
                    .find_map(|i| i.input.and_then(invoke_args_bytes)),
                _ => None,
            };
            if let Some(bytes) = args {
                return decode_input_json(&bytes);
            }
        }
        "null".to_string()
    }
}

/// Extract the inline CBOR `(args, kwargs)` bytes from a [`gen::FunctionInput`]
/// (the `Args` arm of `args_oneof`; blob args are not used by the example flow).
fn invoke_args_bytes(input: gen::FunctionInput) -> Option<Vec<u8>> {
    match input.args_oneof {
        Some(gen::function_input::ArgsOneof::Args(bytes)) => Some(bytes),
        _ => None,
    }
}
