# Bank-the-testing-investment — build-ready spec

**Status:** design / build-ready. Date: 2026-06-05. Workpad: `shim-backend`.
**Goal:** turn the just-landed in-process mock backend (`crates/modal-rust-testkit`)
into real OFFLINE coverage of the gRPC/manifest + invoke surface, add the cheap
no-server pure-builder unit layer, and surface an additive dry-run/dump.
**Frozen:** additive + behavior-preserving. Builder extraction must be BYTE-IDENTICAL
on the wire (the mock records the SAME requests after the refactor). Do NOT touch
the macro crate, `modal-rust-runtime`, the runner CLI protocol, the 5 error kinds,
the FILE-mode wire, `typed!`, the Registry dispatch, or the facade public API
signatures. Do NOT commit `docs/testing-strategy.md`. The testkit stays a dev/test
concern (dev-dependency only, never in default-members).

All file:line citations verified against the working tree on 2026-06-05 (HEAD
`e993999`).

---

## 0. Where we are (baseline, offline, all green 2026-06-05)

- `cargo test -p modal-rust-sdk --lib` → **73** unit tests.
- `cargo test -p modal-rust --lib` → **35** unit tests.
- `crates/modal-rust/tests/local.rs` → **6** (in-process `.local()`).
- `crates/modal-rust/tests/mock_remote.rs` → **2** (facade `.remote()` + error envelope vs mock).
- `crates/modal-rust/tests/mock_table.rs` → **1** (table over RUN configs vs mock).
- `crates/modal-rust-sdk/tests/mock_ops.rs` → **2** (SDK ops vs mock).
- All `live_*` binaries compile to **0** tests without `--features live`.

The testkit is already landed and capable: `MockModal::start()` / `::builder()`,
`url()`, `modal_config()`, typed `requests::<T>()` / `last::<T>()` / `took::<T>()`
/ `all_requests()` / `request_count()`, `function_result_value` /
`function_result_envelope` / `function_body`, and `on_<rpc>` escape hatches for
every steerable RPC (`crates/modal-rust-testkit/src/{builder.rs,server.rs,
servicer.rs,record.rs,responder.rs}`). The facade seam is
`App::connect_at` / `connect_at_with` / `connect_at_with_configs`
(`crates/modal-rust/src/app.rs:216-266`, `#[cfg(any(test, feature = "testkit"))]`).

The three deliverables below are: (1) pure `build_*_request` extraction + unit
tests, (2) the mock test matrix, (3) the additive dump. **Build them
smallest-first; (1) is a prerequisite for (3) and de-risks (2).**

---

## 1. Deliverable 1 — pure `build_*_request` functions + unit tests

### 1.1 The rule (keeps the wire byte-identical)

Every outbound top-level request is currently BUILT-AND-SENT in one method body.
For each, extract ONLY the `let req = XRequest { … };` construction into a free
`pub(crate) fn build_*_request(...) -> XRequest` in the SAME module, and have the
existing method call it where the inline literal was. Nothing else moves — the
retry closure, the empty-id guards, and the response mapping stay put. Because the
method now sends the value the pure fn returns, the bytes on the wire are
unchanged; the mock-records-the-same-request proof is the two mock_* suites in §2
(they assert the recorded request fields are identical to today).

Signatures take already-resolved scalars/sub-messages (the method resolves
`environment_name`, `builder_version`, the `spec.to_proto()` etc. exactly as
today, then passes them in), so the pure fn does NO I/O and is trivially callable
from a `#[test]`.

### 1.2 Extraction table (each: current build+send site → pure-fn signature → call site)

| RPC request | Current build+send site (file:line) | Proposed pure fn (same module) | Called from | Offline coverage TODAY |
| --- | --- | --- | --- | --- |
| `AppGetOrCreateRequest` | `ops/app.rs:47-51` (built), sent `:53-58` | `build_app_get_or_create_request(app_name:&str, environment_name:String) -> AppGetOrCreateRequest` | `app_get_or_create_id` (`ops/app.rs:41`) | **none** (`app.rs` = 0 tests) |
| `AppCreateRequest` | `ops/app.rs:77-83`, sent `:85-90` | `build_app_create_request(description:&str, environment_name:String) -> AppCreateRequest` | `app_create_ephemeral` (`ops/app.rs:67`) | **none** |
| `AppPublishRequest` | `ops/app.rs:125-132`, sent `:134-139` | `build_app_publish_request(app_id:&str, app_name:&str, app_state:AppState, function_ids:HashMap<…>, definition_ids:HashMap<…>) -> AppPublishRequest` | `app_publish` (`ops/app.rs:114`) | **none** |
| `ImageGetOrCreateRequest` | `ops/image.rs:479-484`, sent `:486-491` | `build_image_get_or_create_request(image:Image, app_id:&str, builder_version:String) -> ImageGetOrCreateRequest` | `image_get_or_create` (`ops/image.rs:468`) | sub-msg `to_proto`/`dockerfile_commands` covered (`image.rs:606-920`); **wrapper not** |
| `FunctionPrecreateRequest` | `ops/function.rs:295-302`, sent `:304-308` | `build_function_precreate_request(app_id:&str, function_name:&str) -> FunctionPrecreateRequest` | `function_precreate` (`ops/function.rs:287`) | **none** |
| `FunctionCreateRequest` | `ops/function.rs:332-368` (`Function{…}` then `FunctionCreateRequest{…}`), sent `:370-374` | `build_function_create_request(app_id:&str, precreate_function_id:&str, spec:&FunctionSpec) -> FunctionCreateRequest` (moves both the `Function` build + the wrapper) | `function_create` (`ops/function.rs:326`) | `FunctionSpec::to_proto` sub-msg covered (`function.rs:441-610`); **the XOR `function`/`function_data` + `existing_function_id` wrapper invariant is live-only** |
| `FunctionGetRequest` | `ops/function.rs:413-418`, sent `:420-424` | `build_function_get_request(app_name:&str, function_name:&str, environment_name:String) -> FunctionGetRequest` | `function_from_name` (`ops/function.rs:405`) | **none** |
| `FunctionMapRequest` (unary/remote) | `ops/invoke.rs:165-171`, sent `:173-177` | `build_function_map_request(function_id:&str, call_type:FunctionCallType, invocation_type:FunctionCallInvocationType, pipelined:Vec<FunctionPutInputsItem>) -> FunctionMapRequest` | `invoke_raw_with_deadline`, `spawn_raw`, `map_cbor` (three sites: `invoke.rs:165`, `:367`, `:471`) — ALL three collapse to this one builder | **none** |
| `FunctionPutInputsRequest` | `ops/invoke.rs:192-196` / `:391-395` / `:508-512` | `build_function_put_inputs_request(function_id:&str, function_call_id:&str, inputs:Vec<FunctionPutInputsItem>) -> FunctionPutInputsRequest` | same three invoke sites | **none** |
| `FunctionGetOutputsRequest` | `ops/invoke.rs:246-258` / `:541-548` | `build_function_get_outputs_request(function_call_id:&str, max_values:i32, last_entry_id:String, index:Option<i32>) -> FunctionGetOutputsRequest` | `poll_outputs_indexed` (`invoke.rs:226`) + `map_cbor` loop (`:533`) | **none** |
| `SecretGetOrCreateRequest` (from_name) | `ops/secret.rs:49-55` | `build_secret_from_name_request(name:&str, required_keys:&[String], environment_name:String) -> SecretGetOrCreateRequest` | `secret_get_or_create` (`ops/secret.rs:40`) | sub-msg field asserts EXIST (`secret.rs:122-160`) but on a hand-built literal, not the production builder |
| `SecretGetOrCreateRequest` (from_dict) | `ops/secret.rs:79-85` | `build_secret_from_dict_request(name:&str, env:&HashMap<…>, environment_name:String) -> SecretGetOrCreateRequest` | `secret_from_dict` (`ops/secret.rs:72`) | as above |
| `VolumeGetOrCreateRequest` | `ops/volume.rs:45-51` | `build_volume_get_or_create_request(name:&str, v2:bool, create_if_missing:bool, environment_name:String) -> VolumeGetOrCreateRequest` | `volume_get_or_create` (`ops/volume.rs:27`) | only enum-constant asserts (`volume.rs:71-86`), not the request |
| `MountGetOrCreateRequest` (global) | `ops/mount.rs:135-141`, sent `:143-148` | `build_mount_get_or_create_global_request(deployment_name:&str, environment_name:String) -> MountGetOrCreateRequest` | `global_mount_id` (`ops/mount.rs:129`) | name builders covered (`mount.rs:157+`); **request not** |
| `MountGetOrCreateRequest` (source/ephemeral) | `ops/local_dir.rs:201-211` | `build_mount_get_or_create_source_request(environment_name:String, files:Vec<MountFile>) -> MountGetOrCreateRequest` | `mount_local_dir`/`mount_workspace_closure` upload tail (`local_dir.rs:201`) | file-collection covered (11 tests `local_dir.rs:581+`); **request envelope not** |
| `MountPutFileRequest` (probe + upload) | `ops/local_dir.rs:278-281` (upload) / `:307-310` (probe) | `build_mount_put_file_request(sha256_hex:&str, data:Option<DataOneof>) -> MountPutFileRequest` (one fn; probe passes `None`) | `ensure_file_uploaded` + `mount_put_file_probe` (`local_dir.rs:264,305`) | **none** (`mount_put_file` shape live-only) |
| `BlobCreateRequest` | `ops/blob.rs:45-49`, sent `:51-56` | `build_blob_create_request(data:&[u8]) -> BlobCreateRequest` (computes the sha256-b64 + length internally; the side-effecting PUT stays in `blob_create_and_put`) | `blob_create_and_put` (`ops/blob.rs:39`) | **none** (`blob.rs` = 0 tests) |

### 1.3 Zero-coverage files this closes (the doc's named gaps)

`ops/app.rs` (0 tests) and `ops/blob.rs` (0 tests) get their first unit tests.
The `FunctionCreateRequest` wrapper invariant — `function` set / `function_data`
`None` (the FILE-mode XOR) + `existing_function_id == precreate_id` + `app_id` —
is asserted offline for the first time (today only `live_remote.rs`/`live_deploy.rs`
prove it).

### 1.4 Unit tests to add (no server, no mock; in each op's `#[cfg(test)] mod tests`)

Add `#[test]`s asserting the builder output. Representative (not exhaustive):

- **`build_function_create_request`** (the headline): given a `FunctionSpec` with
  two mount ids + a T4 gpu + a cache volume mount, assert `req.function.is_some()`,
  `req.function_data.is_none()` (XOR), `req.existing_function_id == "fu-pre-1"`,
  `req.app_id == "ap-1"`, `function.definition_type == File`, `function_type ==
  Function`, `function.function_serialized.is_empty()`, `function.mount_ids ==
  [client, source]`, `resources.gpu_config.gpu_type == "T4"`, `volume_mounts.len()
  == 1`, `secret_ids` round-trips. A SECOND test with a bare CPU `FunctionSpec`
  proves `gpu_config.is_none()`, `volume_mounts.is_empty()`, `secret_ids.is_empty()`
  — the byte-identical-to-pre-P6 path.
- **`build_app_publish_request`**: ephemeral vs deployed `app_state` projects the
  right enum int; `function_ids`/`definition_ids` maps round-trip.
- **`build_image_get_or_create_request`**: `image.is_some()`, `app_id`,
  `builder_version` carried; pairs with the existing `to_proto`/`dockerfile_commands`
  sub-message tests.
- **`build_function_map_request`**: the three call-shapes — unary+sync (remote),
  unary+async (spawn), map+sync (map) — each set the right
  `function_call_type`/`function_call_invocation_type` and (for remote/spawn) one
  pipelined input, (for map) empty `pipelined_inputs`.
- **`build_function_get_outputs_request`**: `index=None` ⇒ `start_idx`/`end_idx`
  both `None` (the byte-for-byte single-input `.remote()` shape); `index=Some(i)`
  ⇒ both `Some(i)`; `last_entry_id` carried; `clear_on_success == true`.
- **`build_secret_from_name_request`** vs **`build_secret_from_dict_request`**:
  re-express the existing `secret.rs:122-160` asserts THROUGH the builders (pure
  lookup UNSPECIFIED + no env_dict; vs CREATE_IF_MISSING + env_dict, no
  required_keys).
- **`build_volume_get_or_create_request`**: `(v2=true,create=true)` ⇒ `version==V2`
  + `CreateIfMissing`; `(v2=false,create=true)` ⇒ `version==Unspecified` + V1
  semantics (the user-volume path).
- **`build_mount_get_or_create_source_request`**: `namespace==Workspace`,
  `object_creation_type==Ephemeral`, empty `deployment_name`/`app_id`, files
  passed through.
- **`build_mount_put_file_request`**: probe (`data_oneof==None`) vs inline upload
  (`Some(Data(..))`) — proves the probe/upload shape distinction the live path
  relies on.
- **`build_blob_create_request`**: `content_length == data.len()`,
  `content_sha256_base64` is the base64 SHA-256 of the bytes, `content_md5` empty.

Expected new unit-test count: **~16-20** across the op modules (≥1 per builder,
2 for `function_create` and `function_map`). These run with `cargo test -p
modal-rust-sdk --lib`.

### 1.5 Guardrail that proves the refactor is byte-identical

The extraction is "behavior-preserving" iff the §2 mock suites still record the
SAME `FunctionCreate`/`AppPublish`/etc. after the change. So: land the §2
`mock_ops.rs`/`mock_remote.rs` assertions FIRST or alongside, run them before and
after the extraction commit, and confirm the recorded fields are unchanged. (The
existing `mock_ops.rs::function_create_round_trips_and_is_recorded` already pins
the `FunctionCreate` fields; extend it per §2.)

---

## 2. Deliverable 2 — the mock test matrix

Mirror the established pattern: SDK-level flows in
`crates/modal-rust-sdk/tests/mock_ops.rs` (point a real `ModalClient` at
`mock.modal_config()`); facade end-to-end flows in
`crates/modal-rust/tests/mock_remote.rs` / `mock_table.rs` (drive a real `App`
via `App::connect_at*`). Every row is a plain `#[tokio::test]`, offline (loopback
only, no creds, no Python, no `#[ignore]`). Each row names the RPCs it exercises,
what the mock returns, and the exact assertion.

The facade RUN flow needs a tiny deterministic source dir (the
`tiny_source_config` helper already in `mock_remote.rs:29` / `mock_table.rs:30`) so
the source upload is small + deterministic. The DEPLOY rows need the analogous
`DeployConfig { local_root: <tiny dir>, use_cargo_scoping:false, .. }`.

### Matrix

| # | Flow | Where (test file) | RPCs exercised | Mock config | Exact assertion |
| --- | --- | --- | --- | --- | --- |
| 1 | **deploy + call** | `mock_remote.rs` (facade `App::deploy_with` + `App::call`) | `MountGetOrCreate` (client + source), `AppGetOrCreate` (persistent), `ImageGetOrCreate` ×2 (base + top layer), `FunctionPrecreate`, `FunctionCreate`, `AppPublish`(DEPLOYED), `FunctionGet`(from_name), then `call`: `FunctionGet` + `FunctionMap` + `FunctionGetOutputs` | `builder().function_result_value(json!({"sum":42}))` | `App::connect_at` + `deploy_with(DeployConfig{local_root:tiny,..})`. Assert `mock.took::<ImageGetOrCreateRequest>() == 2` (two layers); the `FunctionCreate` `function.mount_ids.len() == 1` (**CLIENT mount ONLY — the deploy build-boundary invariant: NO source mount**, vs RUN's 2); `AppPublishRequest.app_state == Deployed`; the top-layer `ImageGetOrCreate` image carries the cargo `RUN` (`get_all_dockerfile_commands` analog: its `dockerfile_commands` contains `cargo build --release`) while the RUN image does NOT. Then `App::call(name,"add",{40,2}) == AddOutput{sum:42}`; assert `took::<FunctionGetRequest>() >= 1` and that **no** `MountGetOrCreate`/`ImageGetOrCreate`/`AppPublish` fired during `call` (snapshot `request_count()` before/after `call`, or assert counts unchanged) — proving call does NO upload/build/publish. |
| 2 | **map** (fan-out, input order) | `mock_remote.rs` (facade `Function::map` over 4 inputs) **or** `mock_ops.rs` (`map_cbor` directly) | `FunctionMap`(MAP/SYNC, empty pipelined), `FunctionPutInputs` (N items, each its `idx`), `FunctionGetOutputs` (loop) | `builder().function_body(|input| { let v:Value = parse(input); json!(v["a"].as_i64()+v["b"].as_i64()) })` (compute per-input) so each output is distinct; OR pin via `on_function_get_outputs` returning N items with shuffled `idx` to prove reorder | Drive `app.function("add").map([{a:1,b:1},{a:2,b:2},{a:3,b:3},{a:20,b:22}])`. Assert the returned `Vec<AddOutput>` is **in input order** `[2,4,6,42]`. Assert `last::<FunctionMapRequest>().function_call_type == Map` and `pipelined_inputs.is_empty()` (MAP opens empty); `last::<FunctionPutInputsRequest>().inputs.len() == 4` with `idx` 0..3. (The SDK-level `reassemble_in_order` unit tests at `invoke.rs:657-692` already cover reorder logic; this row proves it end-to-end through the mock.) |
| 3 | **spawn → get** | `mock_ops.rs` (`spawn_cbor` + `get_by_call_cbor`) and/or `mock_remote.rs` (`Function::spawn` → `FunctionCall::get`) | `FunctionMap`(UNARY/**ASYNC**, 1 pipelined), then later `FunctionGetOutputs`(`start_idx==end_idx==index`) | `function_result_value(json!({"sum":42}))` | Assert `spawn` returns a non-empty `function_call_id == "fc-1"`; assert `last::<FunctionMapRequest>().function_call_invocation_type == Async` (vs remote's `Sync` — the spawn invariant). Then `get(idx 0)` decodes to `{sum:42}`; assert the polled `FunctionGetOutputsRequest.start_idx == Some(0)` and `end_idx == Some(0)`. |
| 4 | **secrets** | `mock_remote.rs` (facade RUN with `RemoteConfig{secrets:vec!["api-creds"],..}`) | adds `SecretGetOrCreate` before `FunctionCreate` | default happy path; optionally `on_secret_get_or_create` to pin `sc-1` | Assert `took::<SecretGetOrCreateRequest>() == 1`; the recorded `deployment_name == "api-creds"`, `object_creation_type == Unspecified` (from_name pure lookup), `env_dict.is_empty()`. Assert the resulting `FunctionCreate.function.secret_ids == ["sc-1"]` (the id rode into FunctionCreate). A second case with `secrets:vec![]` ⇒ `took::<SecretGetOrCreateRequest>() == 0` and `function.secret_ids.is_empty()` (wire-identical to before). |
| 5 | **volumes (user)** | `mock_remote.rs` (facade RUN with `RemoteConfig{volumes:vec![("/data".into(),"my-vol".into())], cache:false,..}`) | adds `VolumeGetOrCreate`(V1) before `FunctionCreate` | default; `vo-{n}` canned | Assert `took::<VolumeGetOrCreateRequest>() == 1`; recorded `deployment_name=="my-vol"`, `version==Unspecified` (V1), `object_creation_type==CreateIfMissing`. Assert `FunctionCreate.function.volume_mounts` has one entry with `mount_path == "/data"` and `volume_id == "vo-1"` (the id rode into FunctionCreate). |
| 6 | **cache on/off** (P6) | `mock_table.rs` (2 rows) or `mock_remote.rs` (2 tests) | cache ON adds `VolumeGetOrCreate`(V2) | row A `RemoteConfig{cache:true,..}`; row B `{cache:false,..}` | **ON:** `took::<VolumeGetOrCreateRequest>() == 1`; recorded `deployment_name == "modal-rust-cargo-cache"`, `version == V2`, `CreateIfMissing`; `FunctionCreate.function.volume_mounts` contains a mount at `/cache` with `allow_background_commits==true`. **OFF:** `took::<VolumeGetOrCreateRequest>() == 0` and `function.volume_mounts.is_empty()` — the byte-identical-to-pre-P6 path. (`mock_remote.rs` already sets `cache:true`; add the OFF counterpart + the `/cache` mount-path assertion.) |
| 7 | **image build (get-or-create + streaming)** | `mock_ops.rs` (`image_get_or_create` directly) | `ImageGetOrCreate`; + `ImageJoinStreaming` when forced pending | (a) default inline `result.status==SUCCESS` → no streaming; (b) `on_image_get_or_create` returning `result=Pending` (or omit result) → SDK opens `ImageJoinStreaming`, which the mock already serves a terminal SUCCESS | (a) `image_get_or_create("ap-1",&spec)` returns `im-{n}` with `took::<ImageGetOrCreateRequest>()==1` and the recorded image's `dockerfile_commands` contain the expected layers (FROM, add_python COPY, baked wrapper RUN). (b) Same but proves the streaming poll path completes (no hang) — the one server-streaming RPC the flow can touch (`servicer.rs:408`). Assert the recorded `ImageGetOrCreateRequest.image.dockerfile_commands` for the RUN spec contains the add_python `COPY /python/. /usr/local` and does NOT contain `cargo build` (RUN builds in-body), whereas row 1's deploy top-layer DOES. |
| 8a | **error: decode_error** | `mock_remote.rs` | invoke RPCs (`FunctionMap`/`FunctionGetOutputs`) | `function_result_envelope(r#"{"ok":false,"error":{"kind":"decode_error","message":"bad in","details":null}}"#)` | `.remote()` returns `Err`; assert `matches!(err, Error::Runner(RunnerError::Decode(m)))` with `m=="bad in"`. |
| 8b | **error: unknown_entrypoint** | `mock_remote.rs` | same | envelope `kind:"unknown_entrypoint","message":"no fn"` | `Err(Error::Runner(RunnerError::UnknownEntrypoint("no fn")))`. |
| 8c | **error: function_error (+details)** | `mock_remote.rs` (already partly present `mock_remote.rs:133`) | same | envelope `kind:"function_error","message":"boom","details":{"code":7}` | `Err(Error::Runner(RunnerError::Function{message:"boom", details:Some(json!({"code":7}))}))`. Extend the existing test to assert the typed details, not just the substring. |
| 8d | **error: encode_error** | `mock_remote.rs` | same | envelope `kind:"encode_error","message":"enc"` | `Err(Error::Runner(RunnerError::Encode("enc")))`. |
| 8e | **error: panic (+backtrace)** | `mock_remote.rs` | same | envelope `kind:"panic","message":"oops","backtrace":"f0\nf1"` | `Err(Error::Runner(RunnerError::Panic{message:"oops",backtrace:"f0\nf1"}))`. |

Notes on the 5-kind taxonomy rows: the SDK-level `parse_envelope` unit tests
(`remote.rs:854-934`) already cover the string→enum mapping in isolation. Rows
8a-8e prove the SAME mapping fires through the FULL `.remote()` mock path
(`function_result_envelope` → CBOR → `invoke_cbor::<_,_,String>` →
`parse_envelope`), which is the part that was live-only. A small table test
(one `#[tokio::test]` with a `[(kind, expect)]` slice, fresh mock per case) is the
ergonomic form and proves the per-case-port independence the env-var path could
not (`mock_table.rs` is the template).

### 2.1 Anything the current mock CANNOT drive (flag + minimal extension)

The mock covers the full create/call/invoke surface needed by rows 1-8 **as is**.
The two small gaps, both OPTIONAL hardening — do them only if a row needs them:

1. **`AppGetOrCreate` is NOT recorded in `RecordedRequest`/`FromRecorded`.** The
   servicer implements `app_get_or_create` and pushes
   `RecordedRequest::AppGetOrCreate` (`servicer.rs:90`), and the enum/`FromRecorded`
   variant exist (`record.rs:29,127`) — so it IS queryable. ✓ (No gap; the DEPLOY
   row 1 can assert `took::<AppGetOrCreateRequest>() == 1`.)
2. **Per-`idx` distinct outputs for `map` (row 2).** The default
   `function_get_outputs` echoes ONE output computed from the *last* recorded input
   (`servicer.rs:340` `last_invoked_input_json`), so a multi-input MAP poll returns
   the same value for every idx unless steered. For a faithful input-order proof,
   use `.on_function_get_outputs(|req| …)` to return N `FunctionGetOutputsItem`s
   with per-`idx` data (the escape hatch already exists, `builder.rs:69`). If that
   is too verbose, a MINIMAL testkit extension is a `ResultMode::PerIndexBody`
   that maps each recorded `FunctionPutInputs` item's decoded input → its own
   envelope and returns all N at once. **Recommendation:** start with the existing
   `on_function_get_outputs` escape hatch (no testkit change); add the
   `PerIndexBody` convenience only if more than one map test needs it. This is the
   single most likely small testkit addition; keep it additive and behind the
   existing `MockModalBuilder`.

Everything else (secrets, volumes, cache volume, two-layer deploy image, the
streaming image build, all 5 error kinds) is drivable with the shipped testkit
API. No `build_server`/codegen changes, no proto edits.

Expected new mock-test count: **~12-15** (rows 1-7 + the 5 error kinds folded into
1-2 table tests), split across `mock_ops.rs` (image, map, spawn at the SDK level)
and `mock_remote.rs`/`mock_table.rs` (facade deploy+call, secrets, volumes, cache,
errors).

---

## 3. Deliverable 3 — the dry-run / dump (the deferred P8)

### 3.1 Shape: additive facade method, reuses the §1 pure builders, NO network

Add a NEW additive method on `App` (and a small companion struct) that assembles
the FULL request set a RUN (and a DEPLOY) WOULD send and returns it as structured
data + a readable text render — with **zero** network. It does NOT change
`remote`/`deploy`/`call`; it routes the SAME ordering logic
(`ensure_function`/`deploy_function`) through the §1 pure builders against a local
"planning sink" instead of the live stub. This makes the dump and the real path
share the builder code (no drift), satisfying the "built ON the pure builders"
requirement.

**Public API (additive — does NOT touch the frozen facade signatures):**

```rust
// crates/modal-rust/src/dump.rs (new module; re-exported from lib.rs)

/// The assembled control-plane manifest a run/deploy WOULD send, with NO network.
pub struct Manifest {
    pub mode: RunMode,                 // Run | Deploy
    pub app_name: String,
    pub requests: Vec<PlannedRequest>, // in send order
}

/// One planned outbound request, typed enough to assert + render.
pub enum PlannedRequest {
    AppCreate { description: String },
    AppGetOrCreate { app_name: String },
    VolumeGetOrCreate { name: String, v2: bool },
    SecretGetOrCreate { name: String },
    MountGetOrCreate { role: MountRole },          // Client | Source | PythonStandalone
    ImageGetOrCreate { dockerfile_commands: Vec<String>, layer: u8 },
    FunctionPrecreate { function_name: String },
    FunctionCreate { module: String, function: String, mount_ids_count: usize,
                     gpu: Option<String>, timeout_secs: u32,
                     volume_mounts: Vec<(String,String)>, secret_count: usize,
                     function_data_is_none: bool },
    AppPublish { app_state: &'static str },        // "ephemeral" | "deployed"
}

impl App {
    /// Render the RUN manifest for `entrypoint` (cargo cache vol, secrets,
    /// volumes, client+source+python mounts, image, precreate, FunctionCreate FILE,
    /// ephemeral AppPublish) WITHOUT any network. Additive; does not change `.remote()`.
    pub fn dry_run(&self, entrypoint: &str, config: &RemoteConfig) -> Result<Manifest>;

    /// Render the DEPLOY manifest (two image layers, client-mount-only FunctionCreate,
    /// persistent AppPublish) WITHOUT any network. Additive; does not change `deploy`.
    pub fn dump_deploy_manifest(&self, config: &DeployConfig) -> Result<Manifest>;
}

impl Manifest {
    /// A readable, deterministic text render (one line per planned request).
    pub fn render(&self) -> String;
}
```

`dry_run` is sync + offline: it does NOT need a connected `App` (no `connect`),
because it never sends. It resolves the decorator config via the existing
`config_for(entrypoint)` (`app.rs:271`) so the dumped gpu/timeout/secrets/volumes
match what `.remote()` would send.

### 3.2 How it reuses the §1 pure builders without drift

Two viable implementations — choose the lighter one that keeps the real path
unchanged:

- **(A, recommended) A "planning" assembler that mirrors the ordering and feeds the
  §1 builders.** Factor the ORDERING of `ensure_function`
  (`remote.rs:458-664`) / `deploy_function` (`deploy.rs:267-395`) so the same
  sequence can run against a sink that, instead of calling the stub, (i) records
  the request the §1 `build_*_request` fn produced and (ii) returns a canned id
  (`ap-1`, `mo-1`, `im-1`, `fu-1`, …) so the next step has an id to thread. Because
  the request VALUES come from the identical `build_*_request` fns the live path
  calls, the dumped manifest is exactly what would be sent. The image
  `dockerfile_commands` come from the existing `ImageSpec::dockerfile_commands()`
  (`image.rs`, already public-for-test), so the dump shows the real layers
  including the deploy `cargo build --release` RUN.

  Mechanically, this is the doc's `RequestSink` trait (testing-strategy.md §4-D)
  scoped to the ~12 RPCs the two orchestrators use: a `LiveSink` (today's
  stub+retry calls, behavior-unchanged) and a `PlanningSink` (record + canned id).
  Keep it additive: introduce the sink, make `ensure_function`/`deploy_function`
  generic over it, and the existing call sites pass `LiveSink` — the live wire is
  unchanged (proven by the §2 mock suites still recording the same requests).

- **(B, lighter, acceptable v1) A parallel `*_plan` fn** that re-runs the ordering
  but pushes `PlannedRequest`s built from the §1 builders into a `Vec` (no sink
  trait). Risk: ordering drift between `_plan` and the real fn. Mitigate by keeping
  both in the same module next to each other and having a §2 mock test assert the
  RUN mock's recorded-request ORDER equals `dry_run(...).requests` order. Pick (A)
  if the orchestrators factor cleanly; (B) only if (A) proves invasive.

Either way: NO change to `remote`/`deploy`/`call` semantics or signatures; the
dump is a NEW method/struct/module.

### 3.3 What the dump includes — RUN vs DEPLOY

- **RUN** (`dry_run`): `AppCreate(ephemeral)` → `VolumeGetOrCreate(cargo-cache,V2)`
  *if cache* → `SecretGetOrCreate × secrets` → `VolumeGetOrCreate(user,V1) ×
  volumes` → `MountGetOrCreate(client)` → `MountGetOrCreate(source)` →
  `MountGetOrCreate(python-standalone)` → `ImageGetOrCreate(layer 0; add_python +
  baked wrapper; NO cargo)` → `FunctionPrecreate("handler")` →
  `FunctionCreate(FILE; module=modal_rust_run_wrapper; mount_ids_count=2;
  function_data_is_none=true; gpu/timeout/volume_mounts/secret_count from config)`
  → `AppPublish(ephemeral)`. (Source order/conditionals exactly per
  `remote.rs:464-655`.)
- **DEPLOY** (`dump_deploy_manifest`): `MountGetOrCreate(client)` →
  `MountGetOrCreate(source as build context)` →
  `MountGetOrCreate(python-standalone)` → `AppGetOrCreate(persistent)` →
  `ImageGetOrCreate(base layer; add_python)` → `ImageGetOrCreate(top layer; COPY
  source + cargo build --release)` → `FunctionPrecreate` →
  `FunctionCreate(FILE; module=modal_rust_deploy_wrapper; **mount_ids_count=1 —
  client only, NO source mount**; secrets/volumes from config)` →
  `AppPublish(deployed)`. The `mount_ids_count==1` + the top-layer `cargo build`
  ARE the deploy build-boundary, now inspectable offline. (Exactly per
  `deploy.rs:271-382`.)

### 3.4 Sample rendered manifest (target shape for `Manifest::render`, RUN, cache on, T4)

```
RUN manifest for app "mock-app" (entrypoint "add")
  1. AppCreate              description="mock-app" (ephemeral)
  2. VolumeGetOrCreate      name="modal-rust-cargo-cache" v2=true
  3. MountGetOrCreate       role=Client
  4. MountGetOrCreate       role=Source
  5. MountGetOrCreate       role=PythonStandalone
  6. ImageGetOrCreate       layer=0  [FROM rust:1-slim; COPY /python/. /usr/local; RUN <baked wrapper>; ENV RUST_BACKTRACE=1; ENTRYPOINT []]
  7. FunctionPrecreate      function="handler"
  8. FunctionCreate         module="modal_rust_run_wrapper" function="handler" mount_ids=2 gpu=Some("T4") timeout=1800s volumes=[("/cache",vol)] secrets=0 function_data=None
  9. AppPublish             state=ephemeral
```

DEPLOY render differs at the lines that matter: two `ImageGetOrCreate` (the top
layer's command list contains `RUN cargo build --release …`), `FunctionCreate …
mount_ids=1` (client only), `AppPublish state=deployed`.

### 3.5 Tests for the dump (offline, no server even needed)

- A `#[test]` (sync, no tokio, no mock) asserting `app.dry_run("add",&cfg)?.requests`
  has the expected variant sequence and the `FunctionCreate` fields
  (`mount_ids_count==2`, `function_data_is_none`, gpu/timeout from config).
- A `#[test]` for `dump_deploy_manifest` asserting `mount_ids_count==1` (NO source
  mount) and that the top-layer image commands contain `cargo build --release` —
  the deploy build-boundary, now an offline unit test.
- (Optional, strongest) a `#[tokio::test]` cross-check: drive the SAME config
  through `App::connect_at` + `.remote()` against the mock and assert the mock's
  recorded request types/order equal `dry_run(...).requests` mapped to types —
  proving the dump did NOT drift from the real path.

---

## 4. Verification (offline = HARD gate; NO Modal/Python)

Run on default-members + testkit:

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo build
cargo test
cargo test -p modal-rust -p modal-rust-sdk     # the facade + sdk mock targets explicitly
```

All must be green. The builder refactor (§1) must keep EVERY existing test green —
especially: the `live_*` binaries still COMPILE (no `--features live` in CI), and
the existing mock tests
(`mock_ops.rs::function_create_round_trips_and_is_recorded`,
`mock_remote.rs::remote_add_against_mock_records_manifest_and_decodes_output`)
still record the SAME `FunctionCreate`/`AppPublish` requests as before (the
byte-identical proof). Paste exact cargo output + the new test names + (for the
dump) a quoted sample of `Manifest::render()`.

Expected totals after build: SDK lib **~73 → ~90** (+16-20 builder unit tests),
facade tests **~44 → ~60** (+12-15 mock matrix rows + ~3 dump tests). Counts are
estimates; the build phase reports exact names/numbers.

---

## 5. File map (what changes, all additive)

- `crates/modal-rust-sdk/src/ops/{app,image,function,invoke,secret,volume,mount,
  local_dir,blob}.rs` — extract `build_*_request` fns + add `#[cfg(test)]` unit
  tests (§1). Behavior-preserving.
- `crates/modal-rust-sdk/tests/mock_ops.rs` — add image/map/spawn SDK-level rows (§2).
- `crates/modal-rust/tests/mock_remote.rs` — add deploy+call, secrets, volumes,
  cache-off, the 5 error kinds (§2).
- `crates/modal-rust/tests/mock_table.rs` — optionally fold the cache on/off + the
  error taxonomy into table form (§2).
- `crates/modal-rust/src/dump.rs` (new) + `lib.rs` re-export — the `Manifest` /
  `dry_run` / `dump_deploy_manifest` additive API (§3); plus the `RequestSink`
  refactor of `ensure_function`/`deploy_function` (option A) if chosen.
- `crates/modal-rust-testkit/src/{responder.rs,builder.rs}` — ONLY if a map test
  needs per-idx outputs: an additive `ResultMode::PerIndexBody` (§2.1). Avoid if the
  `on_function_get_outputs` escape hatch suffices.

NO changes to: the macro crate, `modal-rust-runtime`, the runner protocol, the
proto, `build.rs`, the facade public signatures (`local`/`local_with_registry`/
`connect`/`connect_with_registry`/`deploy`/`call`), or `docs/testing-strategy.md`.
