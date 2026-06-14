//! Typed, SDK-owned planning projections for the offline dry-run / dump.
//!
//! The facade's network-free dump ([`modal_rust::dump`]) needs to project exactly
//! what the wire WOULD carry for the two load-bearing requests — the image's
//! `dockerfile_commands` and the FILE-mode `FunctionCreate` fields — WITHOUT
//! re-implementing the request shapes (which would let the dump drift from the live
//! path). Previously the SDK re-exported the raw `build_*_request` functions, which
//! return RAW proto types, so the facade reached across the crate boundary into
//! `modal.client` proto. That leaked proto into the SDK's public API.
//!
//! This module closes that leak: [`plan_image_request`] / [`plan_function_request`]
//! call the SAME internal `build_*_request` builders the live ops call (so there is
//! still ONE seam and ZERO drift), but PROJECT the result into SDK-OWNED plain
//! structs ([`PlannedImage`] / [`PlannedFunction`]). No proto type crosses the
//! crate boundary. The projected fields are exactly the values the corresponding
//! proto message carries on the wire.

use crate::ops::function::{build_function_create_request, FunctionSpec};
use crate::ops::image::{build_image_get_or_create_request, ImageSpec};

/// SDK-owned projection of an `ImageGetOrCreate` request, carrying just the fields
/// the dump renders. Built from [`plan_image_request`]; no proto leaks out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedImage {
    /// The rendered `dockerfile_commands` (the exact list the wire carries) — the
    /// SAME `Vec<String>` `ImageSpec::to_proto` produces.
    pub dockerfile_commands: Vec<String>,
}

/// SDK-owned projection of a FILE-mode `FunctionCreate` request, carrying just the
/// fields the dump renders/asserts. Built from [`plan_function_request`]; no proto
/// leaks out.
///
/// `PartialEq` only (not `Eq`): the retry backoff coefficient is an `f32`, which is not
/// `Eq` (NaN). Equality is still meaningful for the dump's assertions.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedFunction {
    /// The importable wrapper module name (`Function.module_name`).
    pub module_name: String,
    /// The Modal app-namespace object TAG (`Function.function_name` — the entrypoint
    /// name once decoupled from the in-container callable).
    pub function_name: String,
    /// Number of attached mount ids (`Function.mount_ids.len()`).
    pub mount_ids_count: usize,
    /// The GPU type, if any (`Function.resources.gpu_config.gpu_type`); `None` = CPU.
    pub gpu: Option<String>,
    /// Requested CPU in milli-cores (`Function.resources.milli_cpu`); `0` = server
    /// default.
    pub milli_cpu: u32,
    /// Requested memory in MiB (`Function.resources.memory_mb`); `0` = server default.
    pub memory_mb: u32,
    /// The function timeout in seconds (`Function.timeout_secs`).
    pub timeout_secs: u32,
    /// Volume mounts as `(mount_path, volume_id)` pairs (`Function.volume_mounts`).
    pub volume_mounts: Vec<(String, String)>,
    /// Number of attached secret ids (`Function.secret_ids.len()`).
    pub secret_ids_count: usize,
    /// The automatic retry COUNT, if a retry policy is set
    /// (`Function.retry_policy.retries`); `None` = no policy.
    pub retries: Option<u32>,
    /// The retry policy's backoff coefficient, if a policy is set
    /// (`Function.retry_policy.backoff_coefficient`); `None` = no policy. `1.0` for the
    /// bare-int fixed-interval form; a custom value for the `Retries(..)` struct form.
    pub retry_backoff_coefficient: Option<f32>,
    /// The retry policy's initial delay in ms, if a policy is set
    /// (`Function.retry_policy.initial_delay_ms`); `None` = no policy.
    pub retry_initial_delay_ms: Option<u32>,
    /// The retry policy's max delay in ms, if a policy is set
    /// (`Function.retry_policy.max_delay_ms`); `None` = no policy.
    pub retry_max_delay_ms: Option<u32>,
    /// A human-readable summary of the run schedule, if set (`Function.schedule`);
    /// `None` = no schedule. `"cron(<expr> @ <tz>)"` for a [`Cron`], or
    /// `"period(<components>)"` for a [`Period`]. Mirrors what the wire carries
    /// without leaking the proto oneof across the crate boundary.
    pub schedule: Option<String>,
    /// Autoscaler floor — `Function.autoscaler_settings.min_containers`; `None` = unset
    /// (scale to zero).
    pub min_containers: Option<u32>,
    /// Autoscaler ceiling — `Function.autoscaler_settings.max_containers`; `None` =
    /// unset.
    pub max_containers: Option<u32>,
    /// Warm buffer — `Function.autoscaler_settings.buffer_containers`; `None` = unset.
    pub buffer_containers: Option<u32>,
    /// Idle-before-scaledown seconds — `Function.autoscaler_settings.scaledown_window`;
    /// `None` = unset.
    pub scaledown_window: Option<u32>,
    /// Memory-snapshot opt-in (`Function.checkpointing_enabled`, field 41; field 40
    /// `is_checkpointing_function` always mirrors it).
    pub checkpointing_enabled: bool,
    /// Web-endpoint HTTP method (`Function.webhook_config.method`); `None` = not a
    /// web endpoint.
    pub webhook_method: Option<String>,
    /// Web-endpoint proxy-auth requirement
    /// (`Function.webhook_config.requires_proxy_auth`); `false` when not a web
    /// endpoint (and public-by-default when one).
    pub webhook_requires_proxy_auth: bool,
    /// Advertised input data formats (`Function.supported_input_formats`) as enum
    /// names (`"PICKLE"`, `"CBOR"`, `"ASGI"`, …) — surfaces the non-obvious ASGI swap
    /// a web endpoint performs, exactly the wire effect a dry-run exists to reveal.
    pub supported_input_formats: Vec<String>,
    /// Advertised output data formats (`Function.supported_output_formats`).
    pub supported_output_formats: Vec<String>,
    /// The FILE-mode XOR invariant: `FunctionCreateRequest.function_data` is unset.
    pub function_data_is_none: bool,
}

/// Project the `ImageGetOrCreate` request the live image op WOULD send for `spec`
/// (under `app_id` + `builder_version`) into an SDK-owned [`PlannedImage`]. Built ON
/// the SAME [`build_image_get_or_create_request`] the live path calls, so the
/// projected `dockerfile_commands` are exactly what the wire would carry.
pub fn plan_image_request(spec: &ImageSpec, app_id: &str, builder_version: &str) -> PlannedImage {
    let req = build_image_get_or_create_request(spec, app_id, builder_version.to_string());
    PlannedImage {
        dockerfile_commands: req.image.map(|i| i.dockerfile_commands).unwrap_or_default(),
    }
}

/// Project the FILE-mode `FunctionCreate` request the live function op WOULD send for
/// `spec` (under `app_id` + `precreate_id`) into an SDK-owned [`PlannedFunction`].
/// Built ON the SAME [`build_function_create_request`] the live path calls, so the
/// projected fields are exactly what the wire would carry (including the
/// object-tag/implementation-name decoupling and the `function_data` XOR).
pub fn plan_function_request(
    app_id: &str,
    precreate_id: &str,
    spec: &FunctionSpec,
) -> PlannedFunction {
    let req = build_function_create_request(app_id, precreate_id, spec);
    let function_data_is_none = req.function_data.is_none();
    let function = req.function.expect("FILE-mode sets `function`");
    let gpu = function
        .resources
        .as_ref()
        .and_then(|r| r.gpu_config.as_ref())
        .map(|g| g.gpu_type.clone());
    let milli_cpu = function
        .resources
        .as_ref()
        .map(|r| r.milli_cpu)
        .unwrap_or(0);
    let memory_mb = function
        .resources
        .as_ref()
        .map(|r| r.memory_mb)
        .unwrap_or(0);
    let volume_mounts = function
        .volume_mounts
        .iter()
        .map(|m| (m.mount_path.clone(), m.volume_id.clone()))
        .collect();
    let schedule = function
        .schedule
        .as_ref()
        .and_then(|s| s.schedule_oneof.as_ref())
        .map(render_schedule);
    // Project the modern autoscaler knobs (the legacy mirror fields carry the same
    // values, so the settings is the single source of truth for the dump).
    let autoscaler = function.autoscaler_settings.as_ref();
    let min_containers = autoscaler.and_then(|a| a.min_containers);
    let max_containers = autoscaler.and_then(|a| a.max_containers);
    let buffer_containers = autoscaler.and_then(|a| a.buffer_containers);
    let scaledown_window = autoscaler.and_then(|a| a.scaledown_window);
    PlannedFunction {
        module_name: function.module_name.clone(),
        function_name: function.function_name.clone(),
        mount_ids_count: function.mount_ids.len(),
        gpu,
        milli_cpu,
        memory_mb,
        timeout_secs: function.timeout_secs,
        volume_mounts,
        secret_ids_count: function.secret_ids.len(),
        retries: function.retry_policy.map(|p| p.retries),
        retry_backoff_coefficient: function.retry_policy.map(|p| p.backoff_coefficient),
        retry_initial_delay_ms: function.retry_policy.map(|p| p.initial_delay_ms),
        retry_max_delay_ms: function.retry_policy.map(|p| p.max_delay_ms),
        schedule,
        min_containers,
        max_containers,
        buffer_containers,
        scaledown_window,
        checkpointing_enabled: function.checkpointing_enabled,
        webhook_method: function.webhook_config.as_ref().map(|w| w.method.clone()),
        webhook_requires_proxy_auth: function
            .webhook_config
            .as_ref()
            .map(|w| w.requires_proxy_auth)
            .unwrap_or(false),
        supported_input_formats: function
            .supported_input_formats
            .iter()
            .map(|f| data_format_name(*f))
            .collect(),
        supported_output_formats: function
            .supported_output_formats
            .iter()
            .map(|f| data_format_name(*f))
            .collect(),
        function_data_is_none,
    }
}

/// Render a `DataFormat` enum value as its short name — keeps the proto enum from
/// leaking across the crate boundary.
fn data_format_name(value: i32) -> String {
    use crate::proto::api::DataFormat;
    match DataFormat::try_from(value) {
        Ok(DataFormat::Pickle) => "PICKLE".to_string(),
        Ok(DataFormat::Cbor) => "CBOR".to_string(),
        Ok(DataFormat::Asgi) => "ASGI".to_string(),
        Ok(DataFormat::GeneratorDone) => "GENERATOR_DONE".to_string(),
        Ok(other) => format!("{other:?}").to_uppercase(),
        Err(_) => format!("UNKNOWN({value})"),
    }
}

/// Render a `Schedule` oneof into a human-readable summary for the dump — keeps the
/// proto oneof from leaking across the crate boundary.
fn render_schedule(oneof: &crate::proto::api::schedule::ScheduleOneof) -> String {
    use crate::proto::api::schedule::ScheduleOneof;
    match oneof {
        ScheduleOneof::Cron(c) => format!("cron({} @ {})", c.cron_string, c.timezone),
        ScheduleOneof::Period(p) => {
            // List only the non-zero components, in the natural large→small order.
            let mut parts: Vec<String> = Vec::new();
            let mut push = |name: &str, v: i32| {
                if v != 0 {
                    parts.push(format!("{name}={v}"));
                }
            };
            push("years", p.years);
            push("months", p.months);
            push("weeks", p.weeks);
            push("days", p.days);
            push("hours", p.hours);
            push("minutes", p.minutes);
            if p.seconds != 0.0 {
                parts.push(format!("seconds={}", p.seconds));
            }
            format!("period({})", parts.join(","))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_image_projects_dockerfile_commands_from_the_builder() {
        let spec = ImageSpec::from_registry("rust:1-slim")
            .with_add_python("3.12")
            .with_python_standalone_mount_id("mo-py")
            .with_wrapper_module(
                "modal_rust_run_wrapper",
                "def handler(e, i):\n    return i\n",
            );
        let planned = plan_image_request(&spec, "ap-1", "2025.06");
        // Same first line + add_python COPY the raw proto would carry.
        assert_eq!(planned.dockerfile_commands[0], "FROM rust:1-slim");
        assert!(planned
            .dockerfile_commands
            .iter()
            .any(|c| c == "COPY /python/. /usr/local"));
    }

    #[test]
    fn plan_function_projects_file_mode_fields_and_xor() {
        let spec = FunctionSpec::new("modal_rust_run_wrapper", "handler", "im-1")
            .with_app_function_name("add")
            .with_mount_ids(vec!["mo-1".to_string(), "mo-2".to_string()])
            .with_timeout_secs(1800)
            .with_gpu(Some("T4"))
            .expect("valid gpu")
            .with_milli_cpu(Some(2000))
            .with_memory_mb(Some(4096))
            .with_retries(Some(3))
            .with_schedule(Some("cron:UTC:0 9 * * 1"))
            .expect("valid schedule")
            .with_autoscaler(crate::ops::function::FunctionAutoscaler {
                min_containers: Some(1),
                max_containers: Some(5),
                buffer_containers: Some(2),
                scaledown_window: Some(120),
            })
            .expect("valid autoscaler");
        let planned = plan_function_request("ap-1", "fu-pre-1", &spec);
        assert_eq!(planned.module_name, "modal_rust_run_wrapper");
        // Object tag = the entrypoint ("add"), decoupled from the "handler" callable.
        assert_eq!(planned.function_name, "add");
        assert_eq!(planned.mount_ids_count, 2);
        assert_eq!(planned.gpu.as_deref(), Some("T4"));
        assert_eq!(planned.milli_cpu, 2000);
        assert_eq!(planned.memory_mb, 4096);
        assert_eq!(planned.timeout_secs, 1800);
        assert_eq!(planned.secret_ids_count, 0);
        assert_eq!(planned.retries, Some(3));
        // Bare-int form ⇒ Modal's fixed-interval defaults.
        assert_eq!(planned.retry_backoff_coefficient, Some(1.0));
        assert_eq!(planned.retry_initial_delay_ms, Some(1000));
        assert_eq!(planned.retry_max_delay_ms, Some(60_000));
        assert_eq!(planned.schedule.as_deref(), Some("cron(0 9 * * 1 @ UTC)"));
        assert_eq!(planned.min_containers, Some(1));
        assert_eq!(planned.max_containers, Some(5));
        assert_eq!(planned.buffer_containers, Some(2));
        assert_eq!(planned.scaledown_window, Some(120));
        assert!(
            planned.function_data_is_none,
            "FILE-mode XOR: function_data is None"
        );
    }

    /// COMPLETENESS GUARD (architecture review 2026-06-10, in-flight #4): prove the
    /// dump's projection covers EVERY wire field the builder sets. Build a MAXIMAL
    /// spec, then clear (a) every field [`PlannedFunction`] projects and (b) every
    /// field the projection CONSCIOUSLY skips (each with its reason). If the cleared
    /// proto is not `Function::default()`, the builder started setting a wire field
    /// nobody projected or consciously skipped — the dump would silently
    /// under-report the wire. Fix by projecting it into [`PlannedFunction`] or
    /// adding it to the skip list below WITH a reason.
    #[test]
    fn plan_function_projection_is_complete_over_the_builder() {
        use crate::ops::function::{FunctionAutoscaler, FunctionVolumeMount, WebhookSpec};
        use crate::proto::api::Function;

        // MAXIMAL spec: every FunctionSpec knob set, so the builder emits every
        // wire field it knows how to set.
        let spec = FunctionSpec::new("modal_rust_deploy_wrapper", "handler", "im-1")
            .with_app_function_name("web_greet")
            .with_mount_ids(vec!["mo-1".to_string()])
            .with_timeout_secs(600)
            .with_gpu(Some("T4"))
            .expect("valid gpu")
            .with_milli_cpu(Some(1000))
            .with_memory_mb(Some(2048))
            .with_mount_client_dependencies(true)
            .with_volume_mounts(vec![FunctionVolumeMount::new("vo-1", "/data")])
            .with_secret_ids(vec!["st-1".to_string()])
            .with_retry_policy(Some("retries:max=2,backoff=2.0,initial_ms=100,max_ms=1000"))
            .expect("valid retries")
            .with_schedule(Some("cron:UTC:0 9 * * 1"))
            .expect("valid schedule")
            .with_autoscaler(FunctionAutoscaler {
                min_containers: Some(1),
                max_containers: Some(2),
                buffer_containers: Some(1),
                scaledown_window: Some(60),
            })
            .expect("valid autoscaler")
            .with_memory_snapshot(true)
            .with_webhook(Some(WebhookSpec {
                method: "POST".to_string(),
                requires_proxy_auth: true,
                ..Default::default()
            }))
            .expect("valid webhook");
        let req = build_function_create_request("ap-1", "fu-pre-1", &spec);
        let mut function = req.function.expect("FILE mode sets `function`");

        // (a) Every field the PlannedFunction projection covers:
        function.module_name = String::new();
        function.function_name = String::new();
        function.mount_ids.clear();
        function.resources = None; // projected as gpu / milli_cpu / memory_mb
        function.timeout_secs = 0;
        function.volume_mounts.clear();
        function.secret_ids.clear();
        function.retry_policy = None;
        function.schedule = None;
        function.autoscaler_settings = None;
        function.checkpointing_enabled = false;
        function.webhook_config = None; // projected as webhook_method + proxy_auth
        function.supported_input_formats.clear();
        function.supported_output_formats.clear();

        // (b) Fields the builder sets that the projection CONSCIOUSLY skips:
        // derived from the projected function_name decoupling, not independent config.
        function.implementation_name = String::new();
        // an input id threaded by the caller (the dump records the image separately).
        function.image_id = String::new();
        // constants pinned by build_function_create_request_file_mode_xor_and_wrapper.
        function.definition_type = 0;
        function.function_type = 0;
        // builder constant (modern image builder always mounts client deps).
        function.mount_client_dependencies = false;
        // legacy MIRRORS of the projected autoscaler knobs (same values by construction).
        function.warm_pool_size = 0;
        function.concurrency_limit = 0;
        function.experimental_buffer_containers = 0;
        function.task_idle_timeout_secs = 0;
        // mirror of the projected checkpointing_enabled (field 40 always equals 41).
        function.is_checkpointing_function = false;

        assert_eq!(
            function,
            Function::default(),
            "builder sets a wire field the dump projection does not cover; project it \
             into PlannedFunction or consciously skip it above with a reason"
        );
    }

    #[test]
    fn plan_function_projects_struct_form_retry_policy() {
        // The `Retries(..)` STRUCT form: custom backoff + delays ride into the projected
        // retry policy (the count, backoff, and both delays).
        let spec = FunctionSpec::new("modal_rust_run_wrapper", "handler", "im-1")
            .with_retry_policy(Some(
                "retries:max=5,backoff=2.0,initial_ms=500,max_ms=30000",
            ))
            .expect("valid retries spec");
        let planned = plan_function_request("ap-1", "fu-pre-1", &spec);
        assert_eq!(planned.retries, Some(5));
        assert_eq!(planned.retry_backoff_coefficient, Some(2.0));
        assert_eq!(planned.retry_initial_delay_ms, Some(500));
        assert_eq!(planned.retry_max_delay_ms, Some(30_000));
    }
}
