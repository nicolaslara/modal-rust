//! SDK-ops level test: round-trip a REAL `modal_rust_sdk::ModalClient` against the
//! in-process `modal-rust-testkit` mock, OFFLINE (loopback only, no Modal/Python).
//!
//! This is the narrowest, most direct proof the mock works: it needs NO facade and
//! NO injection seam (the SDK is pointed at the mock purely via `from_config`).
//! It records the request and asserts the canned response round-trips — exactly the
//! shape the design spike proved.

use modal_rust_sdk::{FunctionSpec, ModalClient};
use modal_rust_testkit::prelude::*;

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
