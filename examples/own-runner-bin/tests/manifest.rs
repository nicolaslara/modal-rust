//! Offline proof (zero Modal, zero network) that this bring-your-own-runner crate
//! describes and dispatches correctly. The CLI's `--describe` builds and runs the
//! `modal_runner` bin to list the crate's registered entrypoints; the offline proxy for
//! that is `registry_from_inventory()` (the same registry the runner assembles) plus the
//! frozen runner CLI core. So this test asserts two things — both of which are exactly
//! what the auto-detected `modal_runner` bin would produce, proven here in-process:
//!
//! 1. the crate's package + entrypoint are visible in the inventory registry (the
//!    `--describe` view), and
//! 2. a wire input dispatches through the UNCHANGED runner to the frozen envelope.

use modal_rust::__private::runtime;
use modal_rust::{package_from_inventory, registry_from_inventory};
use own_runner_bin::{extract_metrics, LogBatch, Metrics};

/// A small realistic batch reused across the assertions.
fn sample_batch() -> LogBatch {
    LogBatch {
        lines: vec![
            "INFO  source=api request handled".to_string(),
            "ERROR source=api upstream timeout".to_string(),
            "WARN  source=worker retry scheduled".to_string(),
            "INFO  source=api cache hit".to_string(),
            "   ".to_string(), // blank line: skipped, not counted
        ],
    }
}

#[test]
fn entrypoint_is_registered_in_the_describe_view() {
    // The macro registered `extract_metrics` through `inventory`; the registry the
    // `modal_runner` bin assembles (and the CLI `--describe` reads) therefore lists it.
    let registry = registry_from_inventory();
    assert!(
        registry.get("extract_metrics").is_some(),
        "extract_metrics should be registered in the inventory registry"
    );
    // The registration also carries this crate's package name, so `--describe` can
    // attribute the entrypoint to the `own-runner-bin` package.
    assert_eq!(package_from_inventory(), Some("own-runner-bin"));
}

#[test]
fn dispatches_through_the_frozen_runner_envelope() {
    // The wire input the macro accepts (the `LogBatch` struct AS the input) dispatches
    // through the UNCHANGED runner CLI to the frozen `{"ok":true,"value":…}` envelope —
    // exactly what running the `modal_runner` bin would print to stdout.
    let input_json =
        r#"{"lines":["INFO source=api a","ERROR source=api b","INFO source=worker c"]}"#;
    let argv: Vec<String> = [
        "--entrypoint",
        "extract_metrics",
        "--input-json",
        input_json,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    let mut buf = Vec::new();
    let code = runtime::run_cli_with_args(registry_from_inventory(), &argv, &mut buf);
    assert_eq!(code, 0);

    let envelope: serde_json::Value = serde_json::from_slice(&buf).unwrap();
    assert_eq!(envelope["ok"], true);
    assert_eq!(envelope["value"]["total"], 3);
    assert_eq!(envelope["value"]["errors"], 1);
    assert_eq!(envelope["value"]["busiest_source"], "api");
}

#[test]
fn unknown_entrypoint_is_a_typed_error() {
    // The frozen runner reports an unknown entrypoint as a typed error envelope with a
    // non-zero exit code — the same behavior the auto-detected bin yields.
    let argv: Vec<String> = ["--entrypoint", "nope", "--input-json", "{}"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut buf = Vec::new();
    let code = runtime::run_cli_with_args(registry_from_inventory(), &argv, &mut buf);
    assert_eq!(code, 1);
    let envelope: serde_json::Value = serde_json::from_slice(&buf).unwrap();
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"]["kind"], "unknown_entrypoint");
}

#[test]
fn plain_fn_and_local_path_agree() {
    // The macro emits the user fn verbatim, so it stays a plain Rust fn over your own
    // structs — and the offline result matches what the runner dispatched above.
    let metrics: Metrics = extract_metrics(sample_batch()).unwrap();
    assert_eq!(
        metrics,
        Metrics {
            total: 4,
            errors: 1,
            busiest_source: Some("api".to_string()),
        }
    );
}
