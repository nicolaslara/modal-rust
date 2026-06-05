//! SDK-ops level test: round-trip a REAL `modal_rust_sdk::ModalClient` against the
//! in-process `modal-rust-testkit` mock, OFFLINE (loopback only, no Modal/Python).
//!
//! This is the narrowest, most direct proof the mock works: it needs NO facade and
//! NO injection seam (the SDK is pointed at the mock purely via `from_config`).
//! It records the request and asserts the canned response round-trips — exactly the
//! shape the design spike proved.

use modal_rust_sdk::{FunctionSpec, ModalClient};
use modal_rust_testkit::prelude::*;

/// One `map_cbor` input as the SDK expects it: `(args, kwargs)` where
/// `args = (entrypoint, input_json)` and `kwargs` is the empty map. Aliased to keep
/// the `map` test's annotation readable (clippy `type_complexity`).
type MapInput<'a> = ((&'a str, String), std::collections::HashMap<String, ()>);

/// `function_create` round-trips: the SDK sends a `FunctionCreate`, the mock records
/// it and returns the canned `fu-1`, and the recorded request carries the spec's
/// fields (image id, module/function names, mount ids).
#[tokio::test]
async fn function_create_round_trips_and_is_recorded() {
    let mock = MockModal::start().await.expect("mock up");

    // A REAL SDK client dialed at the mock — zero transport change (plain http://).
    let mut client = ModalClient::from_config(mock.modal_config())
        .await
        .expect("connect");

    // Build a FILE-mode function spec and create it.
    let spec = FunctionSpec::new("modal_rust_run_wrapper", "handler", "im-1")
        .with_mount_ids(vec!["mo-1".to_string(), "mo-2".to_string()])
        .with_timeout_secs(1800)
        .with_gpu(Some("T4"))
        .expect("valid gpu");
    let created = client
        .function_create("ap-1", "fu-pre-1", &spec)
        .await
        .expect("function_create");

    // The canned deterministic response round-tripped.
    assert_eq!(created.function_id, "fu-1");
    assert_eq!(created.definition_id, "de-1");

    // The request was RECORDED, typed and queryable.
    assert_eq!(mock.took::<FunctionCreateRequest>(), 1);
    let fc = mock
        .last::<FunctionCreateRequest>()
        .expect("FunctionCreate recorded");
    let function = fc.function.expect("FILE-mode sets `function`");
    assert_eq!(function.module_name, "modal_rust_run_wrapper");
    assert_eq!(function.function_name, "handler");
    assert_eq!(function.image_id, "im-1");
    assert_eq!(function.mount_ids, vec!["mo-1", "mo-2"]);
    assert_eq!(function.timeout_secs, 1800);
    // GPU projection: a non-None gpu populates resources.gpu_config.
    let gpu = function
        .resources
        .and_then(|r| r.gpu_config)
        .expect("gpu_config set for T4");
    assert_eq!(gpu.gpu_type, "T4");
}

/// The CBOR invoke round-trip — the hard path `.remote()` drives. The mock returns
/// the runner ENVELOPE STRING (CBOR-encoded) which the SDK decodes back as
/// `R = String`; a per-test canned value flows through `function_get_outputs`.
#[tokio::test]
async fn invoke_cbor_round_trips_canned_result() {
    let mock = MockModal::builder()
        .function_result_value(serde_json::json!({ "sum": 42 }))
        .start()
        .await
        .expect("mock up");
    let mut client = ModalClient::from_config(mock.modal_config())
        .await
        .expect("connect");

    // The EXACT call `.remote()` makes: invoke_cbor with R=String (the envelope).
    let empty_kwargs: std::collections::HashMap<String, ()> = std::collections::HashMap::new();
    let envelope: String = client
        .invoke_cbor(
            "fu-1",
            &("add", r#"{"a":40,"b":2}"#.to_string()),
            &empty_kwargs,
        )
        .await
        .expect("invoke");

    // The mock wrapped the canned value as a success envelope; the SDK decoded it.
    let v: serde_json::Value = serde_json::from_str(&envelope).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["value"]["sum"], 42);

    // The invoke RPCs were recorded.
    assert_eq!(mock.took::<FunctionMapRequest>(), 1);
    assert!(mock.took::<FunctionGetOutputsRequest>() >= 1);
    let map = mock.last::<FunctionMapRequest>().unwrap();
    assert_eq!(map.function_id, "fu-1");
}

/// Row 7a — image build (get-or-create, inline SUCCESS): the SDK issues ONE
/// `ImageGetOrCreate`, the mock returns `im-{n}` with an inline `SUCCESS` result, so
/// the SDK short-circuits and never opens `ImageJoinStreaming`. Assert the recorded
/// image's `dockerfile_commands` carry the add_python COPY and NO cargo build (the
/// RUN spec builds in-body).
#[tokio::test]
async fn image_get_or_create_inline_success_records_layers() {
    use modal_rust_sdk::ImageSpec;

    let mock = MockModal::start().await.expect("mock up");
    let mut client = ModalClient::from_config(mock.modal_config())
        .await
        .expect("connect");

    let spec = ImageSpec::from_registry("rust:1-slim")
        .with_add_python("3.12")
        .with_python_standalone_mount_id("mo-py")
        .with_wrapper_module(
            "modal_rust_run_wrapper",
            "def handler(e, i):\n    return i\n",
        )
        .with_command("ENTRYPOINT []");
    let image_id = client
        .image_get_or_create("ap-1", &spec)
        .await
        .expect("image_get_or_create");

    assert_eq!(image_id, "im-1", "first image gets the canned id");
    assert_eq!(mock.took::<ImageGetOrCreateRequest>(), 1);
    // Inline success: the streaming RPC was never opened (no hang).
    let img = mock
        .last::<ImageGetOrCreateRequest>()
        .and_then(|r| r.image)
        .expect("image recorded");
    assert!(img
        .dockerfile_commands
        .iter()
        .any(|c| c == "COPY /python/. /usr/local"));
    assert!(
        !img.dockerfile_commands
            .iter()
            .any(|c| c.contains("cargo build")),
        "RUN spec builds in-body, not at image-build time"
    );
}

/// Row 7b — image build via the streaming poll: force the get-or-create result to
/// PENDING so the SDK opens `ImageJoinStreaming`, which the mock serves a terminal
/// SUCCESS. Proves the one server-streaming RPC completes (no hang).
#[tokio::test]
async fn image_get_or_create_pending_drains_streaming_to_success() {
    use modal_rust_sdk::ImageSpec;
    use modal_rust_testkit::prelude::ImageGetOrCreateResponse;

    let mock = MockModal::builder()
        // No inline result ⇒ the SDK treats it as PENDING and opens the stream.
        .on_image_get_or_create(|_req| {
            Ok(ImageGetOrCreateResponse {
                image_id: "im-9".to_string(),
                result: None,
                ..Default::default()
            })
        })
        .start()
        .await
        .expect("mock up");
    let mut client = ModalClient::from_config(mock.modal_config())
        .await
        .expect("connect");

    let spec = ImageSpec::from_registry("rust:1-slim")
        .with_add_python("3.12")
        .with_python_standalone_mount_id("mo-py")
        .with_wrapper_module(
            "modal_rust_run_wrapper",
            "def handler(e, i):\n    return i\n",
        );
    let image_id = client
        .image_get_or_create("ap-1", &spec)
        .await
        .expect("image build completes via the streaming poll");
    assert_eq!(image_id, "im-9");
    assert_eq!(mock.took::<ImageGetOrCreateRequest>(), 1);
}

/// Row 2 — map fan-out in INPUT ORDER. Drive `map_cbor` over 4 inputs; the mock's
/// `on_function_get_outputs` returns N items with per-`idx` computed outputs in a
/// SHUFFLED order, proving the SDK reorders by input ordinal. Assert the MAP opens
/// EMPTY and the 4 inputs each carry their `idx`.
#[tokio::test]
async fn map_cbor_returns_outputs_in_input_order() {
    use modal_rust_testkit::prelude::{
        generic_result, DataFormat, FunctionGetOutputsItem, FunctionGetOutputsResponse,
        GenericResult,
    };

    let mock = MockModal::builder()
        // Return all 4 outputs at once, in a SHUFFLED idx order, each output the
        // sum a+b of the corresponding input (idx i => the i-th input's sum).
        .on_function_get_outputs(|_req| {
            // Inputs are {a:1,b:1},{a:2,b:2},{a:3,b:3},{a:20,b:22} ⇒ sums 2,4,6,42.
            let sums = [2_i64, 4, 6, 42];
            // Emit in a deliberately shuffled completion order: 2, 0, 3, 1.
            let order = [2_usize, 0, 3, 1];
            let outputs = order
                .iter()
                .map(|&i| {
                    let envelope = serde_json::json!({ "ok": true, "value": sums[i] }).to_string();
                    let cbor = modal_rust_sdk::codec::encode(&envelope).unwrap();
                    FunctionGetOutputsItem {
                        idx: i as i32,
                        data_format: DataFormat::Cbor as i32,
                        result: Some(GenericResult {
                            status: generic_result::GenericStatus::Success as i32,
                            data_oneof: Some(generic_result::DataOneof::Data(cbor)),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }
                })
                .collect();
            Ok(FunctionGetOutputsResponse {
                outputs,
                last_entry_id: "1-0".to_string(),
                num_unfinished_inputs: 0,
                ..Default::default()
            })
        })
        .start()
        .await
        .expect("mock up");
    let mut client = ModalClient::from_config(mock.modal_config())
        .await
        .expect("connect");

    // Each input's (args, kwargs) is ((entrypoint, input_json), {}) — the SAME shape
    // the facade's map sends; R = String (the envelope), parsed below.
    let empty = std::collections::HashMap::<String, ()>::new();
    let inputs: Vec<MapInput<'_>> = [
        r#"{"a":1,"b":1}"#,
        r#"{"a":2,"b":2}"#,
        r#"{"a":3,"b":3}"#,
        r#"{"a":20,"b":22}"#,
    ]
    .iter()
    .map(|j| (("add", j.to_string()), empty.clone()))
    .collect();
    let envelopes: Vec<String> = client
        .map_cbor("fu-1", &inputs, std::time::Duration::from_secs(30))
        .await
        .expect("map");

    // The decoded outputs are in INPUT ORDER [2,4,6,42] despite shuffled completion.
    let sums: Vec<i64> = envelopes
        .iter()
        .map(|e| {
            serde_json::from_str::<serde_json::Value>(e).unwrap()["value"]
                .as_i64()
                .unwrap()
        })
        .collect();
    assert_eq!(
        sums,
        vec![2, 4, 6, 42],
        "outputs reassembled in input order"
    );

    // The MAP opened EMPTY; the 4 inputs each carried their idx 0..3.
    let map = mock.last::<FunctionMapRequest>().expect("FunctionMap");
    assert_eq!(map.function_call_type, FunctionCallType::Map as i32);
    assert!(map.pipelined_inputs.is_empty(), "MAP opens empty");
    let put = mock
        .last::<FunctionPutInputsRequest>()
        .expect("FunctionPutInputs");
    assert_eq!(put.inputs.len(), 4);
    let idxs: Vec<i32> = put.inputs.iter().map(|i| i.idx).collect();
    assert_eq!(idxs, vec![0, 1, 2, 3]);
}

/// Row 3 — spawn → get. `spawn_raw` sends a UNARY + ASYNC `FunctionMap` (the spawn
/// invariant vs remote's SYNC) and returns the `function_call_id` WITHOUT polling.
/// `get_by_call_raw` then polls with `start_idx == end_idx == index`.
#[tokio::test]
async fn spawn_async_then_get_by_index() {
    let mock = MockModal::builder()
        .function_result_value(serde_json::json!({ "sum": 42 }))
        .start()
        .await
        .expect("mock up");
    let mut client = ModalClient::from_config(mock.modal_config())
        .await
        .expect("connect");

    let empty = std::collections::HashMap::<String, ()>::new();
    let call_id = client
        .spawn_cbor("fu-1", &("add", r#"{"a":40,"b":2}"#.to_string()), &empty)
        .await
        .expect("spawn");
    assert_eq!(call_id, "fc-1", "spawn returns the canned function_call_id");

    // Spawn invariant: the FunctionMap was UNARY + ASYNC (vs remote's SYNC).
    let map = mock.last::<FunctionMapRequest>().expect("FunctionMap");
    assert_eq!(map.function_call_type, FunctionCallType::Unary as i32);
    assert_eq!(
        map.function_call_invocation_type,
        FunctionCallInvocationType::Async as i32
    );
    // No poll happened during spawn (fire-and-forget).
    assert_eq!(
        mock.took::<FunctionGetOutputsRequest>(),
        0,
        "spawn does not poll"
    );

    // Now get the output at index 0 — the per-index poll sets start_idx==end_idx==0.
    let envelope: String = client
        .get_by_call_cbor(&call_id, 0, std::time::Duration::from_secs(30))
        .await
        .expect("get");
    let v: serde_json::Value = serde_json::from_str(&envelope).unwrap();
    assert_eq!(v["value"]["sum"], 42);
    let poll = mock
        .last::<FunctionGetOutputsRequest>()
        .expect("polled after get");
    assert_eq!(poll.start_idx, Some(0));
    assert_eq!(poll.end_idx, Some(0));
}
