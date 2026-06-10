//! Function authoring: `FunctionPrecreate` + `FunctionCreate` (FILE mode) +
//! `FunctionGet` (`from_name`).
//!
//! ## Fix #1 — `FunctionCreate` sends EXACTLY ONE of `function` / `function_data`
//!
//! modal-rs sent BOTH `function` and `function_data` → server "Internal error".
//! We use the single-`Function` path: set `function`, set `existing_function_id`
//! to the precreate id, and leave `function_data` UNSET. We also ALWAYS set
//! `resources` (omitting it contributed to the same server error).
//!
//! The precreate id is what makes an empty `function_serialized` legal in FILE
//! mode: it sets `allow_sparse_base = true` server-side, bypassing the
//! empty-serialized guard, so the function is identified purely by
//! `module_name` + `function_name`.
//!
//! Mechanically split (M1): [`spec`] holds the public config structs + builders,
//! [`parse`] the const spec-string parsers, [`rpc`] the request builders +
//! response projection + `ModalClient` RPCs. ALL public paths are preserved via
//! the re-exports below; the tests stay here, importing from the new paths.

mod parse;
mod rpc;
mod spec;

pub(crate) use rpc::build_function_create_request;
pub use rpc::CreatedFunction;
pub use spec::{
    FunctionAutoscaler, FunctionResources, FunctionSpec, FunctionVolumeMount, WebhookSpec,
};

#[cfg(test)]
mod tests {
    use super::parse::*;
    use super::rpc::*;
    use super::*;
    use crate::proto::api::function::{DefinitionType, FunctionType};
    use crate::proto::api::schedule::ScheduleOneof;
    use crate::proto::api::{DataFormat, FunctionCreateResponse, WebhookType};

    #[test]
    fn spec_defaults_and_builders() {
        let spec = FunctionSpec::new("spike_wrapper", "handler", "im-123")
            .with_mount_id("mo-client")
            .with_timeout_secs(120);
        assert_eq!(spec.module_name, "spike_wrapper");
        assert_eq!(spec.function_name, "handler");
        assert_eq!(spec.image_id, "im-123");
        assert_eq!(spec.mount_ids, vec!["mo-client".to_string()]);
        assert_eq!(spec.timeout_secs, 120);
        // add_python images rely on worker-injected client deps by default.
        assert!(spec.mount_client_dependencies);
        // No volume by default (wire-identical to pre-P6).
        assert!(spec.volume_mounts.is_empty());
        // No secrets by default (wire-identical to before).
        assert!(spec.secret_ids.is_empty());
    }

    #[test]
    fn volume_mounts_default_empty() {
        let spec = FunctionSpec::new("m", "handler", "im-1");
        assert!(
            spec.volume_mounts.is_empty(),
            "volume_mounts must default empty (wire-identical to pre-P6)"
        );
    }

    #[test]
    fn secret_ids_default_empty() {
        let spec = FunctionSpec::new("m", "handler", "im-1");
        assert!(
            spec.secret_ids.is_empty(),
            "secret_ids must default empty (wire-identical to before)"
        );
    }

    #[test]
    fn retry_policy_defaults_none() {
        let spec = FunctionSpec::new("m", "handler", "im-1");
        assert!(
            spec.retry_policy.is_none(),
            "retry_policy must default None (wire-identical to before retries)"
        );
    }

    #[test]
    fn with_retries_builds_modal_fixed_interval_policy() {
        // `with_retries(Some(N))` mirrors Modal's `_parse_retries(int)`:
        // fixed-interval, 1s initial delay, 60s max delay, N retries.
        let spec = FunctionSpec::new("m", "handler", "im-1").with_retries(Some(3));
        let policy = spec.retry_policy.expect("retries set ⇒ policy present");
        assert_eq!(policy.retries, 3);
        assert_eq!(policy.backoff_coefficient, 1.0, "fixed-interval backoff");
        assert_eq!(policy.initial_delay_ms, 1000, "1s initial delay");
        assert_eq!(policy.max_delay_ms, 60_000, "60s max delay");

        // `None` leaves the field unset — byte-identical to before.
        let bare = FunctionSpec::new("m", "handler", "im-1").with_retries(None);
        assert!(bare.retry_policy.is_none());

        // `retries = 0` is a valid (zero-retry) explicit policy, distinct from unset.
        let zero = FunctionSpec::new("m", "handler", "im-1").with_retries(Some(0));
        assert_eq!(zero.retry_policy.expect("policy present").retries, 0);
    }

    #[test]
    fn parse_retries_spec_custom_backoff_and_delays() {
        // The STRUCT form: all four FunctionRetryPolicy fields ride through. seconds were
        // converted to ms by the macro, so the spec carries integer ms delays.
        let p = parse_retries_spec("retries:max=5,backoff=2.0,initial_ms=500,max_ms=30000")
            .expect("valid retries spec");
        assert_eq!(p.retries, 5);
        assert_eq!(p.backoff_coefficient, 2.0);
        assert_eq!(p.initial_delay_ms, 500);
        assert_eq!(p.max_delay_ms, 30_000);
    }

    #[test]
    fn parse_retries_spec_defaults_optional_components() {
        // Only `max` (the count) is required; the rest fall back to Modal's Retries
        // defaults (backoff 1.0, 1s initial, 60s max).
        let p = parse_retries_spec("retries:max=3").expect("valid retries spec");
        assert_eq!(p.retries, 3);
        assert_eq!(p.backoff_coefficient, RETRY_DEFAULT_BACKOFF_COEFFICIENT);
        assert_eq!(p.initial_delay_ms, RETRY_DEFAULT_INITIAL_DELAY_MS);
        assert_eq!(p.max_delay_ms, RETRY_DEFAULT_MAX_DELAY_MS);
    }

    #[test]
    fn parse_retries_spec_rejects_malformed() {
        // Missing the "retries:" tag.
        assert!(parse_retries_spec("max=5").is_err());
        // Missing the required `max` count.
        assert!(parse_retries_spec("retries:backoff=2.0").is_err());
        // Unknown component.
        assert!(parse_retries_spec("retries:max=5,jitter=0.1").is_err());
        // Non-integer count.
        assert!(parse_retries_spec("retries:max=lots").is_err());
        // Non-float backoff.
        assert!(parse_retries_spec("retries:max=5,backoff=fast").is_err());
    }

    #[test]
    fn with_retry_policy_sets_and_clears() {
        // `Some(spec)` parses the struct form into the proto policy; `None` leaves it
        // unset (byte-identical to before retries).
        let spec = FunctionSpec::new("m", "handler", "im-1")
            .with_retry_policy(Some(
                "retries:max=5,backoff=2.0,initial_ms=500,max_ms=30000",
            ))
            .expect("valid retries spec");
        let policy = spec.retry_policy.expect("struct retries ⇒ policy present");
        assert_eq!(policy.retries, 5);
        assert_eq!(policy.backoff_coefficient, 2.0);
        assert_eq!(policy.initial_delay_ms, 500);
        assert_eq!(policy.max_delay_ms, 30_000);

        // `None` leaves the field unset.
        let bare = FunctionSpec::new("m", "handler", "im-1")
            .with_retry_policy(None)
            .expect("none is valid");
        assert!(bare.retry_policy.is_none());

        // A malformed spec surfaces as an error (mirrors `with_schedule`).
        assert!(FunctionSpec::new("m", "handler", "im-1")
            .with_retry_policy(Some("nonsense"))
            .is_err());
    }

    #[test]
    fn schedule_defaults_none() {
        let spec = FunctionSpec::new("m", "handler", "im-1");
        assert!(
            spec.schedule.is_none(),
            "schedule must default None (wire-identical to before schedule)"
        );
    }

    #[test]
    fn parse_schedule_cron_with_and_without_timezone() {
        // `cron:<timezone>:<cron_string>` — the timezone is parsed first; the cron
        // string is the colon-free remainder verbatim.
        let utc = parse_schedule("cron:UTC:5 4 * * *").expect("valid cron");
        match utc.schedule_oneof.expect("oneof") {
            ScheduleOneof::Cron(c) => {
                assert_eq!(c.cron_string, "5 4 * * *");
                assert_eq!(c.timezone, "UTC");
            }
            other => panic!("expected Cron, got {other:?}"),
        }
        // A non-UTC IANA timezone (contains a `/`, never a `:`) round-trips.
        let ny = parse_schedule("cron:America/New_York:0 6 * * *").expect("valid cron");
        match ny.schedule_oneof.expect("oneof") {
            ScheduleOneof::Cron(c) => {
                assert_eq!(c.cron_string, "0 6 * * *");
                assert_eq!(c.timezone, "America/New_York");
            }
            other => panic!("expected Cron, got {other:?}"),
        }
    }

    #[test]
    fn parse_schedule_period_components() {
        // Only the components present are set; the rest default to 0. `seconds` is float.
        let p = parse_schedule("period:hours=4,minutes=30,seconds=1.5").expect("valid period");
        match p.schedule_oneof.expect("oneof") {
            ScheduleOneof::Period(p) => {
                assert_eq!(p.hours, 4);
                assert_eq!(p.minutes, 30);
                assert_eq!(p.seconds, 1.5);
                // Unset components stay 0 (byte-identical to a Modal Period default).
                assert_eq!(p.years, 0);
                assert_eq!(p.months, 0);
                assert_eq!(p.weeks, 0);
                assert_eq!(p.days, 0);
            }
            other => panic!("expected Period, got {other:?}"),
        }
    }

    #[test]
    fn parse_schedule_rejects_malformed() {
        // No tag prefix.
        assert!(parse_schedule("4 * * * *").is_err());
        // Cron missing the cron string after the timezone.
        assert!(parse_schedule("cron:UTC").is_err());
        // Period with an unknown component.
        assert!(parse_schedule("period:fortnights=2").is_err());
        // Period with a non-integer day count.
        assert!(parse_schedule("period:days=many").is_err());
    }

    #[test]
    fn with_schedule_sets_and_clears() {
        // `Some(spec)` parses into the proto schedule; `None` leaves it unset
        // (byte-identical to before schedule).
        let spec = FunctionSpec::new("m", "handler", "im-1")
            .with_schedule(Some("cron:UTC:0 9 * * 1"))
            .expect("valid schedule");
        assert!(spec.schedule.is_some());

        let bare = FunctionSpec::new("m", "handler", "im-1")
            .with_schedule(None)
            .expect("none is valid");
        assert!(bare.schedule.is_none());

        // A malformed spec surfaces as an error (mirrors `with_gpu`).
        assert!(FunctionSpec::new("m", "handler", "im-1")
            .with_schedule(Some("nonsense"))
            .is_err());
    }

    #[test]
    fn autoscaler_defaults_empty() {
        let spec = FunctionSpec::new("m", "handler", "im-1");
        assert!(
            spec.autoscaler.is_empty(),
            "autoscaler must default empty (wire-identical to before autoscaling)"
        );
    }

    #[test]
    fn with_autoscaler_sets_settings_and_legacy_mirror_fields() {
        // All four knobs ride into `autoscaler_settings` AND the deprecated mirror
        // fields Modal still populates (`_functions.py:1019-1022`).
        let spec = FunctionSpec::new("m", "handler", "im-1")
            .with_autoscaler(FunctionAutoscaler {
                min_containers: Some(1),
                max_containers: Some(5),
                buffer_containers: Some(2),
                scaledown_window: Some(120),
            })
            .expect("valid autoscaler");
        let req = build_function_create_request("ap-1", "fu-pre-1", &spec);
        let function = req.function.expect("function set");

        let settings = function
            .autoscaler_settings
            .expect("autoscaler_settings set when a knob is configured");
        assert_eq!(settings.min_containers, Some(1));
        assert_eq!(settings.max_containers, Some(5));
        assert_eq!(settings.buffer_containers, Some(2));
        assert_eq!(settings.scaledown_window, Some(120));

        // Legacy mirror fields carry the same values (Modal sets both).
        assert_eq!(function.warm_pool_size, 1, "min -> warm_pool_size");
        assert_eq!(function.concurrency_limit, 5, "max -> concurrency_limit");
        assert_eq!(
            function.experimental_buffer_containers, 2,
            "buffer -> _experimental_buffer_containers"
        );
        assert_eq!(
            function.task_idle_timeout_secs, 120,
            "scaledown_window -> task_idle_timeout_secs"
        );
    }

    #[test]
    fn with_autoscaler_partial_leaves_unset_knobs_none() {
        // Only `min_containers` set: the modern settings carries Some(2) for min and
        // None for the rest; the unset legacy mirrors stay 0.
        let spec = FunctionSpec::new("m", "handler", "im-1")
            .with_autoscaler(FunctionAutoscaler {
                min_containers: Some(2),
                ..Default::default()
            })
            .expect("valid autoscaler");
        let function = build_function_create_request("ap-1", "fu-pre-1", &spec)
            .function
            .expect("function set");
        let settings = function.autoscaler_settings.expect("settings set");
        assert_eq!(settings.min_containers, Some(2));
        assert_eq!(settings.max_containers, None);
        assert_eq!(settings.buffer_containers, None);
        assert_eq!(settings.scaledown_window, None);
        assert_eq!(function.warm_pool_size, 2);
        assert_eq!(function.concurrency_limit, 0, "unset max => legacy 0");
        assert_eq!(
            function.task_idle_timeout_secs, 0,
            "unset window => legacy 0"
        );
    }

    #[test]
    fn empty_autoscaler_is_wire_identical() {
        // A default (all-None) autoscaler emits NOTHING: no autoscaler_settings, every
        // legacy mirror at 0 — byte-identical to before the feature.
        let spec = FunctionSpec::new("m", "handler", "im-1");
        let function = build_function_create_request("ap-1", "fu-pre-1", &spec)
            .function
            .expect("function set");
        assert!(
            function.autoscaler_settings.is_none(),
            "empty autoscaler => autoscaler_settings unset (wire-identical)"
        );
        assert_eq!(function.warm_pool_size, 0);
        assert_eq!(function.concurrency_limit, 0);
        assert_eq!(function.experimental_buffer_containers, 0);
        assert_eq!(function.task_idle_timeout_secs, 0);
    }

    #[test]
    fn with_autoscaler_rejects_invalid_bounds() {
        // max < min is rejected up front (mirrors Modal InvalidError).
        assert!(FunctionSpec::new("m", "handler", "im-1")
            .with_autoscaler(FunctionAutoscaler {
                min_containers: Some(5),
                max_containers: Some(2),
                ..Default::default()
            })
            .is_err());
        // scaledown_window == 0 is rejected (Modal requires > 0).
        assert!(FunctionSpec::new("m", "handler", "im-1")
            .with_autoscaler(FunctionAutoscaler {
                scaledown_window: Some(0),
                ..Default::default()
            })
            .is_err());
        // min == max is allowed (a fixed pool).
        assert!(FunctionSpec::new("m", "handler", "im-1")
            .with_autoscaler(FunctionAutoscaler {
                min_containers: Some(3),
                max_containers: Some(3),
                ..Default::default()
            })
            .is_ok());
    }

    #[test]
    fn with_secret_ids_attaches_and_flows_to_proto() {
        // Builder appends; the resolved ids flow into Function.secret_ids (field 10).
        let spec = FunctionSpec::new("m", "handler", "im-1")
            .with_secret_id("se-1")
            .with_secret_id("se-2");
        assert_eq!(
            spec.secret_ids,
            vec!["se-1".to_string(), "se-2".to_string()]
        );
        // `with_secret_ids` replaces.
        let replaced = spec.with_secret_ids(vec!["se-3".to_string()]);
        assert_eq!(replaced.secret_ids, vec!["se-3".to_string()]);
    }

    #[test]
    fn user_volume_and_cache_volume_coexist() {
        // A user volume (e.g. /data) and the P6 cargo-cache volume (/cache) attach as
        // TWO DISTINCT mounts on the SAME function — they must coexist, not collide.
        let spec = FunctionSpec::new("m", "handler", "im-1")
            .with_volume_mount("vo-cache", "/cache") // P6 cargo cache
            .with_volume_mount("vo-data", "/data"); // user volume
        assert_eq!(spec.volume_mounts.len(), 2);
        let cache = spec.volume_mounts[0].to_proto();
        let data = spec.volume_mounts[1].to_proto();
        assert_eq!(cache.volume_id, "vo-cache");
        assert_eq!(cache.mount_path, "/cache");
        assert_eq!(data.volume_id, "vo-data");
        assert_eq!(data.mount_path, "/data");
        // Distinct mount paths => independent mounts.
        assert_ne!(cache.mount_path, data.mount_path);
    }

    #[test]
    fn with_volume_mount_appends_and_to_proto() {
        let spec = FunctionSpec::new("m", "handler", "im-1").with_volume_mount("vo-1", "/cache");
        assert_eq!(spec.volume_mounts.len(), 1);
        let m = spec.volume_mounts[0].to_proto();
        assert_eq!(m.volume_id, "vo-1");
        assert_eq!(m.mount_path, "/cache");
        // Cargo cache: writable + background commits, no sub_path.
        assert!(m.allow_background_commits, "bg-commits ON for cargo cache");
        assert!(!m.read_only, "cargo cache must be writable");
        assert!(m.sub_path.is_none(), "sub_path (field 5) unset");
    }

    #[test]
    fn mount_client_dependencies_defaults_true_and_is_overridable() {
        let spec = FunctionSpec::new("m", "handler", "im-1");
        assert!(spec.mount_client_dependencies);
        let off = spec.with_mount_client_dependencies(false);
        assert!(!off.mount_client_dependencies);
    }

    #[test]
    fn supported_formats_are_pickle_and_cbor() {
        assert_eq!(
            supported_formats(),
            vec![DataFormat::Pickle as i32, DataFormat::Cbor as i32]
        );
    }

    #[test]
    fn resources_default_is_zero() {
        let r = FunctionResources::default().to_proto();
        assert_eq!(r.memory_mb, 0);
        assert_eq!(r.milli_cpu, 0);
        // CPU-only default: gpu_config (proto field 4) stays UNSET — wire-identical
        // to before the GPU addition.
        assert!(
            r.gpu_config.is_none(),
            "CPU default must leave gpu_config unset"
        );
    }

    #[test]
    fn parse_gpu_config_mirrors_python() {
        // "TYPE" -> gpu_type uppercased, count 1, deprecated type field 0.
        let g = parse_gpu_config("T4").unwrap();
        assert_eq!(g.gpu_type, "T4");
        assert_eq!(g.count, 1);
        assert_eq!(g.r#type, 0);

        // Lowercase is uppercased (`.upper()`).
        assert_eq!(parse_gpu_config("t4").unwrap().gpu_type, "T4");

        // "TYPE:count" -> count parsed; default split on FIRST ':'.
        let h = parse_gpu_config("H100:4").unwrap();
        assert_eq!(h.gpu_type, "H100");
        assert_eq!(h.count, 4);

        // MEM suffix is NOT split — rides inside gpu_type verbatim (uppercased).
        let a = parse_gpu_config("A100-80GB").unwrap();
        assert_eq!(a.gpu_type, "A100-80GB");
        assert_eq!(a.count, 1);

        // MEM suffix + count.
        let a2 = parse_gpu_config("A100-80GB:2").unwrap();
        assert_eq!(a2.gpu_type, "A100-80GB");
        assert_eq!(a2.count, 2);

        // Non-integer count -> Err (mirrors Python InvalidError).
        assert!(parse_gpu_config("T4:x").is_err());
    }

    #[test]
    fn to_proto_populates_gpu_config_when_set() {
        let r = FunctionResources {
            gpu: Some("T4".to_string()),
            ..Default::default()
        }
        .to_proto();
        let g = r
            .gpu_config
            .expect("gpu_config must be set when gpu is Some");
        assert_eq!(g.gpu_type, "T4");
        assert_eq!(g.count, 1);
    }

    #[test]
    fn with_gpu_populates_field_4_and_validates() {
        // `with_gpu(Some("T4"))` populates the nested GPUConfig (proto field 4).
        let spec = FunctionSpec::new("m", "handler", "im-1")
            .with_gpu(Some("T4"))
            .unwrap();
        let g = spec
            .resources
            .to_proto()
            .gpu_config
            .expect("gpu_config must be set");
        assert_eq!(g.gpu_type, "T4");
        assert_eq!(g.count, 1);

        // `with_gpu(None)` is CPU (no gpu_config).
        let cpu = FunctionSpec::new("m", "handler", "im-1")
            .with_gpu(None::<String>)
            .unwrap();
        assert!(cpu.resources.to_proto().gpu_config.is_none());

        // A bad count is rejected UP FRONT at set time.
        assert!(FunctionSpec::new("m", "handler", "im-1")
            .with_gpu(Some("T4:nope"))
            .is_err());
    }

    #[test]
    fn with_cpu_and_memory_populate_resources_and_default_is_zero() {
        // `with_milli_cpu(Some)` / `with_memory_mb(Some)` ride into Resources.
        let spec = FunctionSpec::new("m", "handler", "im-1")
            .with_milli_cpu(Some(2000))
            .with_memory_mb(Some(4096));
        let r = spec.resources.to_proto();
        assert_eq!(r.milli_cpu, 2000);
        assert_eq!(r.memory_mb, 4096);

        // `None` clears to 0 (server default) — wire-identical to unset.
        let bare = FunctionSpec::new("m", "handler", "im-1")
            .with_milli_cpu(None)
            .with_memory_mb(None);
        let rb = bare.resources.to_proto();
        assert_eq!(rb.milli_cpu, 0);
        assert_eq!(rb.memory_mb, 0);
    }

    // M8: every Option-taking setter clears on None (never keep-previous).
    // Rule: None == "server default / unset on the wire"; 0 is the numeric sentinel.
    #[test]
    fn all_option_setters_clear_on_none() {
        // Start with a maximally-populated spec, then call every Option setter with None.
        // All should produce the same result as a fresh spec that never called them.

        let populated = FunctionSpec::new("m", "handler", "im-1")
            .with_milli_cpu(Some(2000))
            .with_memory_mb(Some(4096))
            .with_gpu(Some("T4"))
            .expect("valid gpu")
            .with_retries(Some(3))
            .with_retry_policy(Some("retries:max=5"))
            .expect("valid policy")
            .with_schedule(Some("cron:UTC:0 9 * * 1"))
            .expect("valid schedule")
            .with_webhook(Some(WebhookSpec {
                method: "GET".to_string(),
                requires_proxy_auth: false,
            }))
            .expect("valid webhook");

        // Now clear every optional setter.
        let cleared = populated
            .with_milli_cpu(None)
            .with_memory_mb(None)
            .with_gpu(None::<String>)
            .expect("None gpu always ok")
            .with_retries(None)
            .with_retry_policy(None)
            .expect("None policy always ok")
            .with_schedule(None)
            .expect("None schedule always ok")
            .with_webhook(None)
            .expect("None webhook always ok");

        // Numeric fields clear to 0 (server-default sentinel).
        assert_eq!(cleared.resources.milli_cpu, 0, "milli_cpu cleared");
        assert_eq!(cleared.resources.memory_mb, 0, "memory_mb cleared");
        assert!(cleared.resources.gpu.is_none(), "gpu cleared");

        // Option fields clear to None (proto fields unset on the wire).
        assert!(cleared.retry_policy.is_none(), "retry_policy cleared");
        assert!(cleared.schedule.is_none(), "schedule cleared");
        assert!(cleared.webhook.is_none(), "webhook cleared");
    }

    #[test]
    fn build_function_precreate_request_advertises_formats() {
        let req = build_function_precreate_request("ap-1", "handler");
        assert_eq!(req.app_id, "ap-1");
        assert_eq!(req.function_name, "handler");
        assert_eq!(req.function_type, FunctionType::Function as i32);
        // [PICKLE, CBOR] for both directions.
        assert_eq!(req.supported_input_formats, supported_formats());
        assert_eq!(req.supported_output_formats, supported_formats());
    }

    #[test]
    fn build_function_create_request_file_mode_xor_and_wrapper() {
        // The headline: a FILE-mode spec with two mount ids + a T4 gpu + a cache
        // volume + secrets projects the full wrapper invariant offline.
        let spec = FunctionSpec::new("modal_rust_run_wrapper", "handler", "im-1")
            .with_mount_ids(vec!["mo-client".to_string(), "mo-source".to_string()])
            .with_timeout_secs(1800)
            .with_gpu(Some("T4"))
            .expect("valid gpu")
            .with_volume_mount("vo-cache", "/cache")
            .with_secret_id("sc-1")
            .with_retries(Some(3))
            .with_schedule(Some("cron:UTC:0 9 * * 1"))
            .expect("valid schedule");
        let req = build_function_create_request("ap-1", "fu-pre-1", &spec);

        // XOR: function is set, function_data is NOT (fix #1).
        let function = req.function.expect("FILE-mode sets `function`");
        assert!(req.function_data.is_none(), "XOR: function_data unset");
        // Wrapper invariant: app_id + existing_function_id == precreate id.
        assert_eq!(req.app_id, "ap-1");
        assert_eq!(req.existing_function_id, "fu-pre-1");
        // FILE mode: empty serialized, FILE definition, FUNCTION type.
        assert!(function.function_serialized.is_empty());
        assert_eq!(function.definition_type, DefinitionType::File as i32);
        assert_eq!(function.function_type, FunctionType::Function as i32);
        assert_eq!(function.module_name, "modal_rust_run_wrapper");
        assert_eq!(function.function_name, "handler");
        assert_eq!(function.image_id, "im-1");
        assert_eq!(function.timeout_secs, 1800);
        // Mount ids ride through in order (client, source).
        assert_eq!(function.mount_ids, vec!["mo-client", "mo-source"]);
        // GPU projects onto resources.gpu_config.
        let gpu = function
            .resources
            .as_ref()
            .and_then(|r| r.gpu_config.as_ref())
            .expect("gpu_config set for T4");
        assert_eq!(gpu.gpu_type, "T4");
        // The cargo-cache volume mount rode in.
        assert_eq!(function.volume_mounts.len(), 1);
        assert_eq!(function.volume_mounts[0].mount_path, "/cache");
        // Secrets round-trip.
        assert_eq!(function.secret_ids, vec!["sc-1"]);
        // The retry policy rode into Function.retry_policy (field 18).
        let policy = function
            .retry_policy
            .expect("retry_policy set for retries=3");
        assert_eq!(policy.retries, 3);
        assert_eq!(policy.backoff_coefficient, 1.0);
        assert_eq!(policy.initial_delay_ms, 1000);
        assert_eq!(policy.max_delay_ms, 60_000);
        // The schedule rode into Function.schedule (field 72) as a Cron.
        match function
            .schedule
            .as_ref()
            .and_then(|s| s.schedule_oneof.as_ref())
            .expect("schedule set")
        {
            ScheduleOneof::Cron(c) => {
                assert_eq!(c.cron_string, "0 9 * * 1");
                assert_eq!(c.timezone, "UTC");
            }
            other => panic!("expected Cron, got {other:?}"),
        }
    }

    #[test]
    fn build_function_create_request_bare_cpu_is_byte_identical_to_pre_p6() {
        // A bare CPU spec leaves gpu_config / volume_mounts / secret_ids unset — the
        // byte-identical-to-pre-P6 path.
        let spec = FunctionSpec::new("modal_rust_run_wrapper", "handler", "im-1")
            .with_mount_ids(vec!["mo-client".to_string(), "mo-source".to_string()]);
        let req = build_function_create_request("ap-1", "fu-pre-1", &spec);
        let function = req.function.expect("function set");
        // CPU: resources set (fix #1) but gpu_config unset.
        assert!(
            function
                .resources
                .as_ref()
                .and_then(|r| r.gpu_config.as_ref())
                .is_none(),
            "bare CPU leaves gpu_config unset"
        );
        assert!(function.volume_mounts.is_empty(), "no volume mounts");
        assert!(function.secret_ids.is_empty(), "no secrets");
        assert!(
            function.retry_policy.is_none(),
            "no retries ⇒ retry_policy unset (wire-identical)"
        );
        assert!(
            function.schedule.is_none(),
            "no schedule ⇒ Function.schedule unset (wire-identical)"
        );
        assert!(
            function.autoscaler_settings.is_none(),
            "no autoscaling ⇒ autoscaler_settings unset (wire-identical)"
        );
        assert_eq!(function.warm_pool_size, 0, "no autoscaling ⇒ legacy min 0");
        assert_eq!(
            function.concurrency_limit, 0,
            "no autoscaling ⇒ legacy max 0"
        );
        assert!(req.function_data.is_none(), "XOR holds for CPU too");
    }

    #[test]
    fn object_tag_defaults_to_function_name() {
        // Not decoupled: the object tag IS the in-container callable (single-callable
        // shape) — keeps single-function apps wire-identical.
        let spec = FunctionSpec::new("m", "handler", "im-1");
        assert_eq!(spec.object_tag(), "handler");
        assert!(spec.app_function_name.is_none());
    }

    #[test]
    fn with_app_function_name_decouples_tag_from_callable() {
        // Decoupled: object tag = entrypoint name, in-container callable stays "handler".
        let spec = FunctionSpec::new("m", "handler", "im-1").with_app_function_name("add_gpu");
        assert_eq!(spec.object_tag(), "add_gpu");
        assert_eq!(spec.function_name, "handler");
    }

    #[test]
    fn build_function_create_decoupled_tag_sets_implementation_name() {
        // Per-entrypoint object tag: `Function.function_name` becomes the entrypoint
        // (the unique app tag) and the in-container callable moves to
        // `implementation_name` (Modal's `name=` mechanism). Two entrypoints sharing one
        // "handler" callable thus become DISTINCT Modal functions, not a clobber.
        let spec = FunctionSpec::new("modal_rust_run_wrapper", "handler", "im-1")
            .with_app_function_name("add_gpu");
        let req = build_function_create_request("ap-1", "fu-pre-1", &spec);
        let function = req.function.expect("function set");
        // Object tag = entrypoint; implementation = the shared dispatch callable.
        assert_eq!(function.function_name, "add_gpu");
        assert_eq!(function.implementation_name, "handler");
        // The importlib module is unchanged (the wrapper still resolves there).
        assert_eq!(function.module_name, "modal_rust_run_wrapper");
    }

    #[test]
    fn build_function_create_non_decoupled_leaves_implementation_empty() {
        // Single-callable shape (no app_function_name): tag == callable and
        // `implementation_name` stays EMPTY — byte-identical to before this fix.
        let spec = FunctionSpec::new("modal_rust_run_wrapper", "handler", "im-1");
        let req = build_function_create_request("ap-1", "fu-pre-1", &spec);
        let function = req.function.expect("function set");
        assert_eq!(function.function_name, "handler");
        assert!(
            function.implementation_name.is_empty(),
            "non-decoupled keeps implementation_name unset (wire-identical)"
        );
    }

    #[test]
    fn build_function_get_request_is_pure_read() {
        let req = build_function_get_request("my-app", "handler", "main".to_string());
        assert_eq!(req.app_name, "my-app");
        assert_eq!(req.object_tag, "handler");
        assert_eq!(req.environment_name, "main");
        // Latest version.
        assert_eq!(req.app_version, 0);
    }

    #[test]
    fn checkpointing_defaults_false_and_is_wire_identical() {
        // DEFAULT false: a bare spec leaves BOTH checkpoint bools unset on the wire
        // (prost omits fields 40 + 41) — byte-identical to before memory snapshot.
        let spec = FunctionSpec::new("m", "handler", "im-1");
        assert!(
            !spec.checkpointing_enabled,
            "checkpointing must default false"
        );
        let function = build_function_create_request("ap-1", "fu-pre-1", &spec)
            .function
            .expect("function set");
        assert!(
            !function.checkpointing_enabled,
            "no memory snapshot ⇒ checkpointing_enabled unset (wire-identical)"
        );
        assert!(
            !function.is_checkpointing_function,
            "no memory snapshot ⇒ is_checkpointing_function unset (wire-identical)"
        );
    }

    #[test]
    fn with_memory_snapshot_sets_both_proto_fields() {
        // `with_memory_snapshot(true)` flips BOTH `checkpointing_enabled` (field 41) and
        // `is_checkpointing_function` (field 40) on the built Function.
        let spec = FunctionSpec::new("m", "handler", "im-1").with_memory_snapshot(true);
        assert!(spec.checkpointing_enabled, "setter flips the spec flag");
        let function = build_function_create_request("ap-1", "fu-pre-1", &spec)
            .function
            .expect("function set");
        assert!(
            function.checkpointing_enabled,
            "with_memory_snapshot(true) ⇒ checkpointing_enabled (field 41)"
        );
        assert!(
            function.is_checkpointing_function,
            "with_memory_snapshot(true) ⇒ is_checkpointing_function (field 40)"
        );

        // `with_memory_snapshot(false)` leaves both unset (back to wire-identical).
        let off = FunctionSpec::new("m", "handler", "im-1").with_memory_snapshot(false);
        let off_fn = build_function_create_request("ap-1", "fu-pre-1", &off)
            .function
            .expect("function set");
        assert!(!off_fn.checkpointing_enabled);
        assert!(!off_fn.is_checkpointing_function);
    }

    #[test]
    fn webhook_defaults_none_and_is_wire_identical() {
        // DEFAULT None: a bare spec leaves `webhook_config` (field 15) unset AND the
        // advertised formats at `[PICKLE, CBOR]` — byte-identical to before web
        // endpoints for every non-endpoint function.
        let spec = FunctionSpec::new("m", "handler", "im-1");
        assert!(spec.webhook.is_none(), "webhook must default None");
        let function = build_function_create_request("ap-1", "fu-pre-1", &spec)
            .function
            .expect("function set");
        assert!(
            function.webhook_config.is_none(),
            "no webhook ⇒ webhook_config unset (wire-identical)"
        );
        assert_eq!(
            function.supported_input_formats,
            supported_formats(),
            "no webhook ⇒ input formats stay [PICKLE, CBOR] (wire-identical)"
        );
        assert_eq!(
            function.supported_output_formats,
            supported_formats(),
            "no webhook ⇒ output formats stay [PICKLE, CBOR] (wire-identical)"
        );

        // `with_webhook(None)` is the same wire-identical path (the facade's RUN leg).
        let run = FunctionSpec::new("m", "handler", "im-1")
            .with_webhook(None)
            .expect("None is always a valid webhook");
        let run_fn = build_function_create_request("ap-1", "fu-pre-1", &run)
            .function
            .expect("function set");
        assert!(run_fn.webhook_config.is_none());
        assert_eq!(run_fn.supported_input_formats, supported_formats());
        assert_eq!(run_fn.supported_output_formats, supported_formats());
    }

    #[test]
    fn with_webhook_rides_config_and_swaps_formats_to_asgi() {
        // `Some(WebhookSpec)` ⇒ a FUNCTION-type webhook_config rides field 15 AND the
        // advertised formats swap to the ASGI pair (spike finding 3): input [ASGI],
        // output [ASGI, GENERATOR_DONE]. `function_type` stays FUNCTION.
        let spec = FunctionSpec::new("m", "handler", "im-1")
            .with_webhook(Some(WebhookSpec {
                method: "POST".to_string(),
                requires_proxy_auth: false,
            }))
            .expect("POST is a valid method");
        let function = build_function_create_request("ap-1", "fu-pre-1", &spec)
            .function
            .expect("function set");
        let webhook = function
            .webhook_config
            .expect("webhook set ⇒ webhook_config present");
        assert_eq!(webhook.r#type, WebhookType::Function as i32);
        assert_eq!(webhook.method, "POST");
        assert!(!webhook.requires_proxy_auth, "public by default (D4)");
        // The rest of WebhookConfig stays at its zero default (FUNCTION shape).
        assert!(webhook.requested_suffix.is_empty());
        assert_eq!(webhook.web_server_port, 0);
        assert!(webhook.custom_domains.is_empty());
        // Formats swap to ASGI (advertising PICKLE breaks modal-http on webhooks).
        assert_eq!(
            function.supported_input_formats,
            vec![DataFormat::Asgi as i32],
            "webhook ⇒ input formats [ASGI]"
        );
        assert_eq!(
            function.supported_output_formats,
            vec![DataFormat::Asgi as i32, DataFormat::GeneratorDone as i32],
            "webhook ⇒ output formats [ASGI, GENERATOR_DONE]"
        );
        // The function stays a normal FUNCTION (webhooks are not generators).
        assert_eq!(function.function_type, FunctionType::Function as i32);
    }

    #[test]
    fn with_webhook_requires_proxy_auth_rides_through() {
        let spec = FunctionSpec::new("m", "handler", "im-1")
            .with_webhook(Some(WebhookSpec {
                method: "GET".to_string(),
                requires_proxy_auth: true,
            }))
            .expect("GET is a valid method");
        // And the set-time allowlist REJECTS a malformed method (in-flight fix #5).
        assert!(FunctionSpec::new("m", "handler", "im-1")
            .with_webhook(Some(WebhookSpec {
                method: "BREW".to_string(),
                requires_proxy_auth: false,
            }))
            .is_err());
        let webhook = build_function_create_request("ap-1", "fu-pre-1", &spec)
            .function
            .expect("function set")
            .webhook_config
            .expect("webhook_config present");
        assert_eq!(webhook.method, "GET");
        assert!(
            webhook.requires_proxy_auth,
            "proxy-auth opt-in rides WebhookConfig.requires_proxy_auth (field 10)"
        );
    }

    #[test]
    fn created_function_surfaces_web_url_from_handle_metadata() {
        use crate::proto::api::FunctionHandleMetadata;

        // web_url plumbed from the create response's handle_metadata.
        let resp = FunctionCreateResponse {
            function_id: "fu-1".to_string(),
            handle_metadata: Some(FunctionHandleMetadata {
                definition_id: "de-1".to_string(),
                web_url: "https://ws--app-add.modal.run".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let created = created_function_from_response(resp).expect("valid response");
        assert_eq!(created.function_id, "fu-1");
        assert_eq!(created.definition_id, "de-1");
        assert_eq!(created.web_url, "https://ws--app-add.modal.run");

        // Non-webhook create: Modal leaves web_url empty — surfaced as empty.
        let plain = FunctionCreateResponse {
            function_id: "fu-2".to_string(),
            handle_metadata: Some(FunctionHandleMetadata {
                definition_id: "de-2".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let created = created_function_from_response(plain).expect("valid response");
        assert_eq!(created.web_url, "");

        // No handle_metadata at all: both ids default empty.
        let bare = FunctionCreateResponse {
            function_id: "fu-3".to_string(),
            ..Default::default()
        };
        let created = created_function_from_response(bare).expect("valid response");
        assert_eq!(created.definition_id, "");
        assert_eq!(created.web_url, "");

        // An empty function_id is still rejected (the pre-existing guard).
        assert!(created_function_from_response(FunctionCreateResponse::default()).is_err());
    }
}
