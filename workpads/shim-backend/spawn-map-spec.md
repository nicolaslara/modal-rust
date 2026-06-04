# spawn / map / get — build-ready spec

Implements `Function::spawn` / `FunctionCall::get` / `Function::map` (and the SDK
ops they need) by EXTENDING the proven invoke path. No new RPCs, no new control
plane. The single wrapper function already serves every entrypoint; spawn/map/get
reuse `ensure_function` + the same `(entrypoint, input_json)` CBOR args tuple that
`.remote()` sends.

## 0. What is already proven (reuse verbatim)

- `.remote()` → `App::remote_invoke` (`crates/modal-rust/src/app.rs:199`) resolves
  the wrapper `function_id` once via `ensure_function`
  (`crates/modal-rust/src/remote.rs:280`), memoized in a `OnceCell`
  (`app.rs:50`), then calls
  `invoke_cbor_with_deadline(function_id, &(entrypoint, input_json), &empty_kwargs, deadline)`
  (`app.rs:246-253`). The wrapper returns the runner's one-line JSON envelope as
  an `R=String`.
- The SDK invoke core is `invoke_raw_with_deadline`
  (`crates/modal-rust-sdk/src/ops/invoke.rs:132`): build a `FunctionPutInputsItem`
  with `idx:0` + a CBOR `FunctionInput` (`invoke.rs:138-147`), `FunctionMap`
  (`FUNCTION_CALL_TYPE_UNARY`, pipelined) → fix-#3 `FunctionPutInputs` fallback if
  `pipelined_inputs` came back empty → `poll_outputs` (`invoke.rs:210`).
- CBOR codec: `codec::encode((args, kwargs))` / `Invocation::decode_cbor`
  (`invoke.rs:108-113`, `codec.rs:17`). Payload is the 2-tuple `(args, kwargs)`,
  `DATA_FORMAT_CBOR` (`api.proto:115`).
- Envelope decode: `crate::remote::parse_envelope::<Out>` (`remote.rs:411`) — the
  SAME function `.remote()`/`call` use, mapping the runner's five-kind taxonomy.
- All RPCs go through `retry_unary` (`retry.rs:104`); `self.stub()` is
  `pub(crate)` (`client.rs:165`) so new ops in `ops/invoke.rs` reach the gRPC stub
  directly.

Key insight: spawn/map differ from `.remote()` ONLY in (a) `function_call_type`,
(b) number of inputs + their `idx`, and (c) whether the client waits. The decode
layer (`parse_envelope::<Out>` over the `R=String` envelope) is IDENTICAL.

---

## 1. SDK layer (`crates/modal-rust-sdk/src/ops/invoke.rs`)

Three new `impl ModalClient` methods, all `String`-typed at the envelope level
(generic decode happens in the facade via `parse_envelope`, exactly like
`.remote()`). Each enqueues already-knowing `args = (entrypoint, input_json)` and
`kwargs = {}` — but the SDK stays generic over `A: Serialize`/`K: Serialize` like
`invoke_cbor`, so the facade passes the tuple.

### 1a. `spawn_raw` — fire-and-forget, returns the call id

```rust
/// Enqueue ONE input (fire-and-forget) and return its `function_call_id`
/// immediately, WITHOUT polling. Mirrors Python `Function.spawn`
/// (_functions.py:1860) → `_Invocation.create(..., ASYNC)` (_functions.py:134),
/// which sends a UNARY FunctionMap with the input pipelined, then returns
/// `_FunctionCall._new_hydrated(invocation.function_call_id, ...)`
/// (_functions.py:1880) — no output wait.
pub async fn spawn_cbor<A, K>(
    &mut self,
    function_id: &str,
    args: &A,
    kwargs: &K,
) -> Result<String>           // returns function_call_id
where A: Serialize, K: Serialize
{
    let encoded = codec::encode(&(args, kwargs))?;
    self.spawn_raw(function_id, encoded).await
}

pub async fn spawn_raw(
    &mut self,
    function_id: &str,
    args_serialized: Vec<u8>,
) -> Result<String>           // function_call_id
```

Body (reuse the `.remote()` step-1/step-2 enqueue, DROP step-3 poll):
1. Build the `FunctionPutInputsItem { idx: 0, input: Some(FunctionInput{
   data_format: Cbor, args_oneof: Args(bytes), .. }), .. }` exactly as
   `invoke_raw_with_deadline` (`invoke.rs:138-147`).
2. `FunctionMapRequest { function_id, function_call_type: FunctionCallType::Unary
   as i32, function_call_invocation_type: FunctionCallInvocationType::Async as i32,
   pipelined_inputs: vec![item.clone()], ..Default::default() }`. **Note vs
   `.remote()`**: Python `spawn` uses `function_call_type=UNARY`
   (single input, _functions.py:159) but `function_call_invocation_type=ASYNC`
   (_functions.py:1878) — ASYNC = "don't expect the client to hold a sync poll
   open" — whereas `.remote()` uses SYNC. Use `Async` here.
3. `retry_unary("function_map", ...)` (same pattern as `invoke.rs:165-170`). Error
   if `function_call_id` is empty (`invoke.rs:173-177`).
4. Fix-#3 fallback: if `map.pipelined_inputs.is_empty()`, send
   `FunctionPutInputsRequest { function_id, function_call_id, inputs: vec![item] }`
   via `retry_unary("function_put_inputs", ...)` (`invoke.rs:184-200`); error if
   the response `inputs` is empty.
5. **Return `function_call_id`. Do NOT poll.**

Idempotency caveat is identical to `.remote()` (`invoke.rs:150-156`): a retried
FunctionMap could double-enqueue; the `add` test fn is idempotent and `get`
reads `idx=0`, so harmless for v0.

### 1b. `get_by_call_raw` — poll ONE output by call id + index

```rust
/// Poll `FunctionGetOutputs` for `function_call_id`, return the output at
/// `index` (decoded `Invocation`). Mirrors Python `FunctionCall.get`
/// (_functions.py:1959) → `_Invocation.poll_function(timeout, index)`
/// (_functions.py:311) → `pop_function_call_outputs(index, ...)`
/// (_functions.py:217), which sets `start_idx=index, end_idx=index`
/// (_functions.py:240-241).
pub async fn get_by_call_raw(
    &mut self,
    function_call_id: &str,
    index: i32,
    deadline: Duration,
) -> Result<Invocation>

pub async fn get_by_call_cbor<R: DeserializeOwned>(
    &mut self,
    function_call_id: &str,
    index: i32,
    deadline: Duration,
) -> Result<R>   // = get_by_call_raw(...).await?.decode_cbor()
```

Body: this is `poll_outputs` (`invoke.rs:210-285`) GENERALIZED to take a
`function_call_id` the caller already owns AND filter by `index`:
- Same long-poll loop, same `last_entry_id` cursor, same `OUTPUTS_TIMEOUT_SECS`
  (`invoke.rs:37`), same `retry_unary("function_get_outputs", ...)`, same
  terminal-vs-pending handling, same blob-result rejection (`invoke.rs:258-268`).
- ADD to `FunctionGetOutputsRequest`: `start_idx: Some(index)`,
  `end_idx: Some(index)` (proto `api.proto:2103-2104`), matching Python's
  per-index pop (_functions.py:240-241). Keep `max_values: 1`,
  `clear_on_success: true`.
- When an output item arrives, it carries `item.idx` (`api.proto:2084`). With
  `start_idx==end_idx==index` the server returns only that index, but assert /
  filter `item.idx == index` defensively before decoding.
- **Refactor, don't fork**: extract the current `poll_outputs` body into
  `poll_outputs_indexed(function_call_id, index: Option<i32>, deadline)`.
  `invoke_raw_with_deadline` calls it with `index = None` (single-input
  `.remote()` reads `idx 0`, unfiltered, exactly as today — behavior preserved).
  `get_by_call_raw` calls it with `index = Some(index)`. This keeps `.remote()`
  byte-for-byte unchanged.

### 1c. `map_cbor` — fan-out N inputs, collect outputs in input order

```rust
/// Enqueue N inputs under ONE map call and return their decoded envelopes in
/// INPUT ORDER. Mirrors Python `_map_invocation` (parallel_map.py:361):
/// FunctionMap(function_call_type=MAP) (parallel_map.py:374) to OPEN the call,
/// then FunctionPutInputs the inputs each carrying its `idx`
/// (parallel_map.py:122-132 via _create_input idx; function_utils.py:613/624),
/// then poll FunctionGetOutputs and reorder by `item.idx`
/// (parallel_map.py:541-577).
pub async fn map_cbor<A, K, R>(
    &mut self,
    function_id: &str,
    inputs: &[(A, K)],       // each element = (args, kwargs) for one input
    deadline: Duration,
) -> Result<Vec<R>>
where A: Serialize, K: Serialize, R: DeserializeOwned
```

The facade passes `inputs` where each element's `args = (entrypoint, input_json_i)`
and `kwargs = {}`. Body:

1. **Open the map call.** `FunctionMapRequest { function_id,
   function_call_type: FunctionCallType::Map as i32,
   function_call_invocation_type: FunctionCallInvocationType::Sync as i32,
   pipelined_inputs: vec![], ..Default::default() }` (NO pipelined inputs — Python
   opens the MAP call empty, parallel_map.py:371-378). `retry_unary` →
   `function_call_id` (error if empty).

2. **Build N items with sequential `idx`.** For `i in 0..N`:
   `FunctionPutInputsItem { idx: i as i32, input: Some(FunctionInput {
   data_format: Cbor, args_oneof: Args(codec::encode(&(args_i, kwargs_i))?), .. }),
   .. }`. The `idx` IS the input ordinal — this is the ordering key
   (Python: `_create_input(..., idx=idx)` with `idx = self.inputs_created`
   incrementing, parallel_map.py:122-132; written into `FunctionPutInputsItem.idx`,
   function_utils.py:624).

3. **Enqueue all N.** `FunctionPutInputsRequest { function_id, function_call_id,
   inputs: items }` via `retry_unary("function_put_inputs", ...)`. (Python batches
   via the input pumper, parallel_map.py:179-189; for small N a single request is
   correct — chunk into batches of e.g. 1000 only if `N` is large, not required
   for the test plan.) Error if the response accepts fewer than N inputs.

4. **Collect N outputs, reorder by idx.** Long-poll `FunctionGetOutputs` (same
   loop/cursor/retry as `poll_outputs`) WITHOUT `start_idx/end_idx`, with
   `max_values` set to a batch (e.g. `N as i32` or a cap like 100) and
   `clear_on_success: true`. Accumulate into `let mut got: BTreeMap<i32, R>`:
   for each returned `FunctionGetOutputsItem`, on `ResultState::Success` decode
   `Invocation{data, data_format}.decode_cbor::<R>()` and insert at `item.idx`
   (`api.proto:2084`); on `ResultState::Failure` return the `describe_failure`
   error immediately (fail-fast; `return_exceptions=false` — Python default is
   ordered fail). Stop when `got.len() == N`. Honor `deadline` (same elapsed-check
   as `invoke.rs:219-224`).
   - Track `last_entry_id` across windows (`invoke.rs:245-247`).
   - Ignore duplicate idxs (BTreeMap insert is idempotent on key — matches
     Python's duplicate handling at a coarse grain; fine for idempotent `add`).
5. **Reassemble in input order**: `Ok((0..N).map(|i| got.remove(&(i as i32))
   .expect("all idx present")).collect())`. Equivalent to Python's reorder buffer
   (parallel_map.py:556-577) which yields `output_idx` 0,1,2,... popping from
   `received_outputs[idx]`. Returning a `Vec` indexed by `idx` is the simpler,
   correct form of the same ordering invariant.

Note: this is the SYNCHRONOUS-collect form of map (no streaming generator, no
client-side retry policy, no input-plane path). That is the minimal correct subset
of `_map_invocation`; the FROZEN `.remote()` path is untouched.

---

## 2. Facade layer (`crates/modal-rust/src/function.rs` + `app.rs`)

### 2a. `FunctionCall` carries the client handle + call id

Replace the placeholder `FunctionCall { _private: () }` (`function.rs:120`). It
needs to outlive the `Function<'a>` borrow (the user holds it and calls `.get()`
later), so it borrows the `&App` like `Function` does — OR carries an
`&'a App` + the call id:

```rust
pub struct FunctionCall<'a> {
    app: &'a crate::App,
    function_call_id: String,
}
```

`spawn` returns `FunctionCall<'a>` borrowing the same `&App` (lifetime ties the
handle to the App, which owns the `Mutex<ModalClient>`). This mirrors Python's
`_FunctionCall` carrying `(function_call_id, client)` (_functions.py:1880-1882).

### 2b. App plumbing (`app.rs`) — three thin `pub(crate)` methods

Reuse the EXACT `function_id` resolution + config logic from `remote_invoke`
(`app.rs:199-255`). Factor the shared head (resolve cfg, `get_or_try_init` the
`function_id`, compute `effective_timeout`/`deadline`) so spawn/map/get respect
the decorator gpu/timeout the same way `.remote()` does
(`app.rs:213-242`). The deadline (`effective_timeout + 120`) covers the cold
in-body `cargo build` on first call — spawn/map hit the SAME wrapper, so the SAME
deadline applies.

```rust
// fire-and-forget: ensure_function, enqueue, return call_id (no wait)
pub(crate) async fn remote_spawn(&self, entrypoint: &str, input_json: String)
    -> Result<String>;  // function_call_id

// poll one output by call id + index, return raw envelope String
pub(crate) async fn remote_get(&self, function_call_id: &str, index: i32)
    -> Result<String>;

// fan-out N, return envelopes in input order
pub(crate) async fn remote_map(&self, entrypoint: &str, inputs_json: Vec<String>)
    -> Result<Vec<String>>;
```

- `remote_spawn`: resolve `function_id` (the `get_or_try_init` block,
  `app.rs:219-229`), then
  `client.spawn_cbor(function_id, &(entrypoint, input_json), &empty_kwargs)`.
- `remote_get`: needs only the client (no `function_id` — the call id is
  self-describing). `let deadline = Duration::from_secs(handle.config.timeout_secs
  + 120)` (or a `get`-specific deadline; for the test, the input is already
  enqueued so a few minutes covers the cold build). Then
  `client.get_by_call_cbor::<String>(function_call_id, index, deadline)`.
- `remote_map`: resolve `function_id` (same block), build
  `let inputs: Vec<((&str, String), HashMap<String,()>)> = inputs_json.into_iter()
  .map(|j| ((entrypoint, j), HashMap::new())).collect()`, then
  `client.map_cbor::<_,_,String>(function_id, &inputs, deadline)`.

`remote_get`/`remote_spawn`/`remote_map` each do
`let handle = self.remote.as_ref().ok_or_else(Error::not_connected)?` and
`let mut client = handle.client.lock().await` (same as `remote_invoke`,
`app.rs:204`, `245`).

### 2c. `Function::spawn` / `FunctionCall::get` / `Function::map`

```rust
pub async fn spawn<In>(&self, input: In) -> Result<FunctionCall<'_>>
where In: serde::Serialize
{
    let input_json = serde_json::to_string(&input).map_err(Error::Encode)?;
    let function_call_id = self.app.remote_spawn(&self.name, input_json).await?;
    Ok(FunctionCall { app: self.app, function_call_id })
}

pub async fn map<In, Out, I>(&self, inputs: I) -> Result<Vec<Out>>
where In: serde::Serialize, Out: serde::de::DeserializeOwned, I: IntoIterator<Item = In>
{
    let inputs_json = inputs.into_iter()
        .map(|i| serde_json::to_string(&i).map_err(Error::Encode))
        .collect::<Result<Vec<_>>>()?;
    let envelopes = self.app.remote_map(&self.name, inputs_json).await?;
    envelopes.iter().map(|e| crate::remote::parse_envelope::<Out>(e)).collect()
}
```

```rust
impl FunctionCall<'_> {
    pub async fn get<Out>(&self, timeout: Option<Duration>) -> Result<Out>
    where Out: serde::de::DeserializeOwned
    {
        // `timeout` maps onto the get deadline; None => the wrapper timeout+buffer.
        let envelope = self.app.remote_get(&self.function_call_id, 0).await?;
        crate::remote::parse_envelope::<Out>(&envelope)
    }
}
```

- `spawn` keeps the SAME signature (`function.rs:86`), now returning a hydrated
  handle instead of `Error::NotImplemented`.
- `map` keeps the SAME signature (`function.rs:98`); ordering is guaranteed by the
  SDK `map_cbor` idx-reassembly, then each envelope decodes via `parse_envelope`
  (the same five-kind taxonomy as `.local()`/`.remote()`).
- `get` keeps the SAME signature (`function.rs:128`); `index` is `0` (spawn = one
  output, _functions.py:1963). `timeout` plumbs into the SDK `deadline` (thread it
  into `remote_get` if a caller-supplied bound is wanted; default = wrapper
  timeout + 120s).

**Codec consistency note:** the wrapper returns the envelope as a CBOR `String`
(`R=String`), so `spawn_cbor`/`map_cbor`/`get_by_call_cbor` decode `R=String`,
then `parse_envelope::<Out>` parses the JSON inside. This is the EXACT two-step
`.remote()` uses (`app.rs:246` returns `String`; `function.rs:78-79` parses).

---

## 3. Proto fields cited (no proto changes)

- `FunctionMapRequest` `api.proto:2174`: `function_id`(1), `function_call_type`(4),
  `pipelined_inputs`(5), `function_call_invocation_type`(6).
- `FunctionMapResponse` `api.proto:2184`: `function_call_id`(1),
  `pipelined_inputs`(2).
- `FunctionPutInputsItem` `api.proto:2238`: `idx`(1) ← the ordering key,
  `input`(2).
- `FunctionInput` `api.proto:2163`: `args`(1) oneof, `data_format`(10).
- `FunctionPutInputsRequest` `api.proto:2246`: `function_id`(1),
  `function_call_id`(3), `inputs`(4).
- `FunctionPutInputsResponse(Item)` `api.proto:2252/2256`: `inputs`/`idx`.
- `FunctionGetOutputsRequest` `api.proto:2094`: `function_call_id`(1),
  `max_values`(2), `timeout`(3), `last_entry_id`(6), `clear_on_success`(7),
  `start_idx`(10), `end_idx`(11) ← per-index get.
- `FunctionGetOutputsResponse` `api.proto:2107`: `outputs`(4), `last_entry_id`(5),
  `num_unfinished_inputs`(6).
- `FunctionGetOutputsItem` `api.proto:2082`: `result`(1), `idx`(2) ← reorder key,
  `data_format`(5).
- `FunctionCallType` `api.proto:172`: `UNARY=1` (spawn/single), `MAP=2` (map).
- `FunctionCallInvocationType` `api.proto:164`: `ASYNC=3` (spawn), `SYNC=4`
  (remote/map collect).
- `DataFormat` `api.proto:110`: `CBOR=4`.

---

## 4. Frozen-invariant compliance

- Runner protocol / HandlerFn / typed!/Registry / run-vs-deploy boundary: untouched
  — spawn/map/get hit the SAME wrapper `function_id` with the SAME
  `(entrypoint, input_json)` args + empty kwargs (`app.rs:243-253`).
- `retry_transient` on ALL new RPCs via `retry_unary` (every FunctionMap /
  FunctionPutInputs / FunctionGetOutputs above).
- Ephemeral-run vs persistent-deploy: spawn/map run on the EPHEMERAL app
  (`app.rs:171`), never `app_publish` — no lingering deploy.
- `.remote()`/deploy/call logic unchanged: `poll_outputs` is REFACTORED to
  `poll_outputs_indexed(.., index=None, ..)` preserving today's single-input
  behavior; no edits to `remote_invoke`'s call shape, `ensure_function`,
  `parse_envelope`, or `FunctionConfig`/decorator.
- Decorator gpu/timeout respected: spawn/map reuse the SAME cfg-resolution +
  `get_or_try_init` + `effective_timeout` deadline block as `remote_invoke`
  (`app.rs:213-242`).

---

## 5. Live test plan (CPU `add`, ephemeral, small N)

Add a live test next to the existing remote tests (behind `#[ignore]` + the live
feature, with retry-on-flake), using the CPU `add` entrypoint (fast build),
ephemeral run app, small N.

1. **spawn → get**
   - `let app = App::connect("modal-rust-spawn-live").await?;`
   - `let fc = app.function("add").spawn(AddInput{a:40,b:2}).await?;`
   - `let out: AddOutput = fc.get(None).await?;`
   - assert `out.sum == 42`.
   - Drives to a terminal result: spawn returns a call id immediately; `get` polls
     `FunctionGetOutputs` for that call (the first call pays the cold in-body
     `cargo build`, covered by the timeout+120 deadline).

2. **map (fan-out, ordered), N=3..5**
   - `let inputs = vec![AddInput{a:1,b:1}, AddInput{a:2,b:2}, AddInput{a:3,b:3},
     AddInput{a:10,b:5}];`
   - `let outs: Vec<AddOutput> = app.function("add").map(inputs).await?;`
   - assert `outs.iter().map(|o| o.sum).collect::<Vec<_>>() == vec![2,4,6,15]`
     (proves INPUT ORDER, not completion order — the idx reassembly).

Modal flakiness ⇒ RETRY the live attempt. Keep N small (3-5) and CPU-only so the
build is fast and cheap; ephemeral app is GC'd on disconnect (no lingering
deploy). Offline gates (`fmt`/`clippy -D warnings`/`build`/`test`) stay green
because the new live test is `#[ignore]`d and the only non-live change to the
proven path is the `poll_outputs` → `poll_outputs_indexed(index=None)` refactor.

---

## 6. Implementation order (smallest correct steps)

1. SDK: refactor `poll_outputs` → `poll_outputs_indexed(fcid, index: Option<i32>,
   deadline)`; `invoke_raw_with_deadline` calls it with `None`. Gates green
   (no behavior change).
2. SDK: add `get_by_call_raw`/`get_by_call_cbor` (calls
   `poll_outputs_indexed(.., Some(index), ..)`). Unit-testable shape; gates green.
3. SDK: add `spawn_raw`/`spawn_cbor` (FunctionMap UNARY+ASYNC + fix-#3, return
   call id). Gates green.
4. SDK: add `map_cbor` (FunctionMap MAP, N PutInputs with idx, collect+reorder).
   Gates green.
5. Facade: `App::remote_spawn`/`remote_get`/`remote_map` (factor the
   cfg/function_id/deadline head out of `remote_invoke`). Gates green.
6. Facade: implement `Function::spawn`/`Function::map` + `FunctionCall<'a>` +
   `FunctionCall::get`. Gates green.
7. Live test (spawn→get; map ordered N=4), `#[ignore]` + live feature; drive to
   `2/4/6/15` + spawn→get `42`.
