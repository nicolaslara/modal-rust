//! Offline proof (zero Modal, zero network) that the cron schedule on the decorator
//! rides into the planned `FunctionCreate` manifest.
//!
//! `#[modal_rust::function(schedule = Cron("0 9 * * 1"))]` records the canonical spec
//! `schedule = Some("cron:UTC:0 9 * * 1")` on the entrypoint's config. The facade's
//! public, network-free `App::dry_run` projects exactly the request sequence a deploy
//! WOULD send — so we assert that for the decorated `weekly_report` entrypoint the
//! planned `FunctionCreate` carries the cron schedule. No live Modal, no credentials.

use modal_rust::{App, PlannedRequest, RemoteConfig};

/// A deterministic config for the projection. No cargo scoping so the projection never
/// shells out to `cargo metadata`; cache off so the manifest stays minimal.
fn run_cfg() -> RemoteConfig {
    RemoteConfig {
        package: "example-scheduled-job".to_string(),
        use_cargo_scoping: false,
        cache: false,
        ..RemoteConfig::default()
    }
}

#[test]
fn schedule_rides_into_function_create() {
    // The example's OWN decorator submissions, collected from inventory — the SAME
    // (registry, configs) the runner assembles. `App::from_manifest` reads the
    // per-entrypoint config via the same `config_for` path a deploy uses.
    let (_registry, configs) = modal_rust::from_inventory_with_configs();
    let app = App::from_manifest(
        configs
            .into_iter()
            .map(|(name, options)| (name.to_string(), options)),
    );

    let manifest = app
        .dry_run("weekly_report", &run_cfg())
        .expect("dry_run projects the manifest");

    // The decorator's `schedule = Cron("0 9 * * 1")` rode into FunctionCreate as a
    // Cron schedule (rendered `cron(<expr> @ <tz>)` by the SDK projection).
    let schedule = manifest
        .requests
        .iter()
        .find_map(|r| match r {
            PlannedRequest::FunctionCreate { schedule, .. } => Some(schedule.clone()),
            _ => None,
        })
        .expect("the manifest plans a FunctionCreate");
    assert_eq!(
        schedule.as_deref(),
        Some("cron(0 9 * * 1 @ UTC)"),
        "the decorator's `schedule = Cron(\"0 9 * * 1\")` rode into FunctionCreate.schedule"
    );
}

#[test]
fn body_produces_a_report_for_the_dataset() {
    // The macro emits the user fn verbatim, so it stays a plain Rust fn. The cron
    // cadence is metadata, not behavior, so the body itself just does its work.
    let report = example_scheduled_job::weekly_report(example_scheduled_job::Tick {
        dataset: "events".to_string(),
    })
    .expect("the report runs");
    assert_eq!(report.dataset, "events");
    assert_eq!(report.rows, example_scheduled_job::ROWS_PER_RUN);
    assert!(report.note.contains("events"));
}
