# modal-rust feasibility spike — checkpoint notes

GOAL: Prove Rust can PROGRAMMATICALLY create + invoke a Modal function via FILE mode
(DefinitionType=FILE, function_serialized=b"", module_name+function_name), NO `modal` CLI,
NO generated .py in a user project. Wrapper module baked into the image.

## Resume map (scratch crates)
- /tmp/modal-rust-spike  — THIS crate (FILE-mode attempt). Cargo.toml + spike_wrapper.py + src/main.rs
- /tmp/mrs-spike          — prior SERIALIZED (cloudpickle) attempt; working template for auth/app/image/precreate/create/deploy/from_name/remote. src/main.rs is a good reference.
- /tmp/mrs-probe          — tiny API probe crate
- /tmp/modal-rs-src/modal-rs-0.1.3 — extracted modal-rs source (READ to understand API)

## Key source findings (modal-rs 0.1.3)
- function_authoring.rs:759 `to_proto_function(allow_sparse_base)`: the empty-`function_serialized`
  guard (lines 762-780) is SKIPPED when allow_sparse_base=true.
- function_authoring.rs:975 `allow_sparse_base = self.existing_function_id.is_some()`.
  => Passing `.with_existing_function_id(precreate_id)` to FunctionCreateSpec BYPASSES the guard.
  => FILE mode w/ empty function_serialized works through the PUBLIC API. NO fork needed.
- `FunctionDefinitionType::File` exists and maps to api DefinitionType::File. `with_definition_type`.
- `Function::new` is pub(crate) — cannot build a Function from a raw id. Must deploy
  (apps().set_objects + apps().publish Deployed) then functions().from_name() to get a Function,
  then .remote(client, args). Same path the SERIALIZED mrs-spike used successfully.
- Image: use `run_commands([...])` to bake the wrapper via a heredoc `RUN` (avoids local COPY,
  which validate_dockerfile_commands rejects). /root is on sys.path in Modal containers.
- remote(): serializes (args,kwargs) as pickle/cbor, FunctionMap UNARY + FunctionFinishInputs +
  FunctionGetOutputs. Fully wired.

## Plan
AppCreate (ephemeral or get_or_create) -> Image::from_registry(python:3.x-slim) + run_commands
that write /root/spike_wrapper.py -> build (poll ImageJoinStreaming) -> image_id
-> FunctionPrecreate(handler) -> FunctionCreate(File, module=spike_wrapper, fn=handler,
   empty function_serialized, with_existing_function_id) -> set_objects + publish(Deployed)
-> from_name -> remote(({payload},)) -> expect {"echoed":..,"ok":true}.

## Milestone log
- 2026-06-04: Resumed. No prior SPIKE_NOTES existed. Inspected scratch + modal-rs source.
  Confirmed FILE-mode-with-empty-serialized is reachable via public API (existing_function_id
  bypass). Auth file ~/.modal.toml present. python3.14 local. Writing FILE-mode src/main.rs next.
- RUNTIME epoch=1780555449: auth ok (connect + builder_version read)
- RUNTIME epoch=1780555457: image_id obtained: im-NcAnN4su8JRcBj6l0ql1w1
- RUNTIME epoch=1780555458: precreate function_id: fu-K2xYPD3FFfa8VAtkr2i2tJ
- RUNTIME (run1 verdict): FunctionCreate FILE-mode FAILED with repeatable server-side
  "Internal error ... contact support@modal.com" (codes 4XCZQOO1/Q3IKSQ55/9QM9X1AI/1IYN5E4H/4TW2CHH5),
  5/5 attempts. NOT transient. Prior steps OK: auth, app ap-6TvhPbDWR2u1J6u9rDWKjF,
  image im-NcAnN4su8JRcBj6l0ql1w1 BUILT, precreate fu-K2xYPD3FFfa8VAtkr2i2tJ.
  builder_version reported = 2024.10 (old). Hypotheses to test:
   (a) FILE-mode FunctionCreate needs fields modal-rs omits (resources, pty_info, runtime, etc).
   (b) modal-rs sends BOTH function= and function_data= ; server may dislike for FILE+sparse.
   (c) builder version mismatch.
  Next: inspect what Python modal sends for a FILE-mode FunctionCreate; compare proto fields.
- RUNTIME epoch=1780555808: auth ok (connect + builder_version read)
- RUNTIME epoch=1780555808: image_id obtained: im-NcAnN4su8JRcBj6l0ql1w1
- RUNTIME epoch=1780555808: precreate function_id: fu-K9TlQPdNDbHgZAc15Us6Hu
- RUNTIME epoch=1780555808: FILE-mode function_id created: fu-K9TlQPdNDbHgZAc15Us6Hu
- RUNTIME (run2): PATCHED FORK at /tmp/modal-rs-fork (Cargo [patch.crates-io] modal-rs -> path).
  Patch = send only `function` (not function+function_data XOR) + set resources=Some(default).
  RESULT: FunctionCreate FILE-mode SUCCEEDED -> fu-K9TlQPdNDbHgZAc15Us6Hu (image im-NcAnN4su8JRcBj6l0ql1w1).
  So FILE-mode function CREATE is feasible from Rust. New failure is downstream at set_objects/publish:
  gRPC "Unknown error: module 'grpc' has no attribute 'experimental'" (server-side handler bug in
  AppSetObjects or AppPublish). Need to confirm which call and whether we can skip deploy and invoke
  the ephemeral function directly.
- RUNTIME epoch=1780555878: auth ok (connect + builder_version read)
- RUNTIME epoch=1780555879: image_id obtained: im-NcAnN4su8JRcBj6l0ql1w1
- RUNTIME epoch=1780555879: precreate function_id: fu-h6zN6aD1xznV0erxSw2oPx
- RUNTIME epoch=1780555879: FILE-mode function_id created: fu-h6zN6aD1xznV0erxSw2oPx
- RUNTIME epoch=1780555879: app published (Deployed) via AppPublish only
- RUNTIME epoch=1780555991: INVOKE FAILED: gRPC error: code: 'Some requested entity was not found', message: "Function call not found."
- RUNTIME (run3): Dropped legacy AppSetObjects; publish via AppPublish ONLY (function_ids +
  definition_ids), matching Python runner._publish_app. RESULT: PUBLISH SUCCEEDED. App deployed:
  https://modal.com/apps/nicolaslara/main/deployed/modal-rust-spike-file
  function fu-h6zN6aD1xznV0erxSw2oPx, definition de-Eq362dOntVYgDb05nWDaM0, image im-NcAnN4su8JRcBj6l0ql1w1.
  from_name resolved OK. So FILE-mode CREATE + DEPLOY + RESOLVE all feasible from Rust.
  New failure at INVOKE: FunctionMap returns a function_call_id but FunctionGetOutputs returns
  "Function call not found" (8/8). Likely FunctionFinishInputs / get-outputs sequencing or the
  unary call_id handling in modal-rs call.get. Investigating call.get path next.
- 2026-06-04 (analysis run3->run4): Root-caused INVOKE failure. modal-rs-0.1.3 invoke_unary sends
  the input only as FunctionMap.pipelined_inputs and assumes acceptance, then calls
  FunctionFinishInputs. Modal's Python client (_functions.py _call_function_nowait) instead: if
  FunctionMap response.pipelined_inputs is EMPTY, it falls back to FunctionPutInputs to actually
  enqueue the input. Without that fallback the input is never queued => FunctionGetOutputs returns
  "Function call not found". PATCH applied to fork: add FunctionPutInputs fallback when
  response.pipelined_inputs empty; drop the FunctionFinishInputs call (Python doesn't use it here).
- RUNTIME epoch=1780556088: auth ok (connect + builder_version read)
- RUNTIME epoch=1780556089: image_id obtained: im-NcAnN4su8JRcBj6l0ql1w1
- RUNTIME epoch=1780556089: precreate function_id: fu-F7C5DGfSv6bTQPqZlCJXcp
- RUNTIME epoch=1780556089: FILE-mode function_id created: fu-F7C5DGfSv6bTQPqZlCJXcp
- RUNTIME epoch=1780556089: app published (Deployed) via AppPublish only
- RUNTIME epoch=1780556657: auth ok (connect + builder_version read)
- RUNTIME epoch=1780556658: image_id obtained: im-NcAnN4su8JRcBj6l0ql1w1
- RUNTIME epoch=1780556658: precreate function_id: fu-Jq6sJfY5X7oGOgRFZBZ8ED
- RUNTIME epoch=1780556658: FILE-mode function_id created: fu-Jq6sJfY5X7oGOgRFZBZ8ED
- RUNTIME epoch=1780556658: app published (Deployed) via AppPublish only
- 2026-06-04 (run4 runtime diag via `modal app logs`): INVOKE reached the container but it
  crash-loops: "python -m modal._container_entrypoint ... ModuleNotFoundError: No module named 'modal'".
  ROOT CAUSE (design requirement, not infra): the FILE-mode container entrypoint is
  `python -m modal._container_entrypoint`, so the IMAGE MUST contain the `modal` client package.
  Bare python:3.12-slim lacks it. FIX: bake `pip install modal` into the image alongside the
  wrapper. Re-run.
- RUNTIME epoch=1780557201: auth ok (connect + builder_version read)
- RUNTIME epoch=1780557218: image_id obtained: im-8lwwjAfMF9jp4dTcnlZ88R
- RUNTIME epoch=1780557218: precreate function_id: fu-gTBseISOKcTtSCbrY9kaHD
- RUNTIME epoch=1780557218: FILE-mode function_id created: fu-gTBseISOKcTtSCbrY9kaHD
- RUNTIME epoch=1780557218: app published (Deployed) via AppPublish only
- RUNTIME epoch=1780557221: INVOKE OK result: {"echoed":{"hi":1,"n":42},"ok":true,"source":"spike_wrapper.handler"}

## VERDICT: FEASIBLE
- 2026-06-04 (run5): FULL FILE-MODE ROUND TRIP SUCCEEDED.
  App modal-rust-spike-file2 (ap-UsPgG1uNL2WU59MOEAdzp1), image im-8lwwjAfMF9jp4dTcnlZ88R
  (python:3.12-slim + `pip install modal` + baked /root/spike_wrapper.py), FILE-mode function
  fu-gTBseISOKcTtSCbrY9kaHD (module_name=spike_wrapper, function_name=handler,
  definition_type=FILE, function_serialized=b""), definition de-XBWyn3bHb7Mhvfaf95dVxP.
  Invoked from Rust with payload {"hi":1,"n":42}:
    RESULT VALUE = {"echoed":{"hi":1,"n":42},"ok":true,"source":"spike_wrapper.handler"}
  NO modal CLI for create/invoke. NO generated .py in a user project (wrapper baked from a Rust
  string literal into the image). cloudpickle NOT used.

## What it took (4 fixes layered on modal-rs 0.1.3, all in fork /tmp/modal-rs-fork)
1. FILE mode w/ empty function_serialized: reachable via PUBLIC api by passing
   with_existing_function_id(precreate_id) (allow_sparse_base bypass). No fork needed for this.
2. FunctionCreate: send only `function` (NOT function+function_data XOR) + set resources=Some(default).
   (modal-rs sent both -> server "Internal error / contact support".) [FORK PATCH]
3. Deploy: skip legacy AppSetObjects (server handler throws grpc.experimental); use AppPublish
   ONLY with function_ids + definition_ids, matching Python runner._publish_app. [spike code]
4. Invoke: add FunctionPutInputs fallback when FunctionMap response.pipelined_inputs is empty;
   drop FunctionFinishInputs. (modal-rs lost the input -> "Function call not found".) [FORK PATCH]
5. Image MUST contain the `modal` pip package (container boots `python -m modal._container_entrypoint`).
