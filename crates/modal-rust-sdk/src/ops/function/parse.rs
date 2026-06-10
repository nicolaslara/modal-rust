//! Spec-string parsers for the const, `&'static str` decorator forms the macros
//! emit (`retries:..` / GPU `"TYPE[:count]"` / `cron:..`|`period:..`) — pure
//! string -> proto, no I/O. Split out of `function.rs` mechanically (M1).

use crate::error::{Error, Result};
use crate::proto::api::schedule::{Cron, Period, ScheduleOneof};
use crate::proto::api::{FunctionRetryPolicy, GpuConfig, Schedule};

/// Default backoff coefficient for the bare integer `retries = N` form, mirroring
/// Modal's `_parse_retries(int)` -> `Retries(max_retries=N, backoff_coefficient=1.0,
/// initial_delay=1.0)` (`retries.py`, `_utils/function_utils.py:_parse_retries`).
/// `1.0` = fixed-interval backoff.
pub(super) const RETRY_DEFAULT_BACKOFF_COEFFICIENT: f32 = 1.0;
/// Default initial delay (ms) before the first retry for the bare `retries = N` form
/// (`initial_delay=1.0` second).
pub(super) const RETRY_DEFAULT_INITIAL_DELAY_MS: u32 = 1000;
/// Default max delay (ms) between retries — Modal's `Retries` default `max_delay=60.0`
/// seconds (`retries.py`).
pub(super) const RETRY_DEFAULT_MAX_DELAY_MS: u32 = 60_000;

/// Parse a modal-rust retry SPEC string (the `Retries(..)` STRUCT form) into a
/// [`FunctionRetryPolicy`], mirroring Modal's `Retries(max_retries, backoff_coefficient,
/// initial_delay, max_delay)` (`retries.py`). The spec is the canonical, const-string
/// form the `#[function(retries = Retries(..))]` macro emits (a `&'static str` is
/// const-valid in the `inventory::submit!` static initializer, exactly like `gpu` /
/// `schedule`).
///
/// Format: `"retries:max=<N>[,backoff=<f>][,initial_ms=<u32>][,max_ms=<u32>]"`. The
/// `max=` component (the retry COUNT) is REQUIRED; the rest default to Modal's
/// `Retries` defaults (`backoff_coefficient=1.0`, `initial_delay=1s ⇒ 1000ms`,
/// `max_delay=60s ⇒ 60000ms`). The macro converts seconds→ms at parse time so the
/// spec carries integer millisecond delays. A malformed spec maps to [`Error::invalid`]
/// (mirroring Python's `InvalidError`).
pub(super) fn parse_retries_spec(spec: &str) -> Result<FunctionRetryPolicy> {
    let rest = spec.strip_prefix("retries:").ok_or_else(|| {
        Error::invalid(format!(
            "Invalid retries spec {spec:?}: expected a \"retries:..\" prefix"
        ))
    })?;
    let mut retries: Option<u32> = None;
    let mut backoff_coefficient = RETRY_DEFAULT_BACKOFF_COEFFICIENT;
    let mut initial_delay_ms = RETRY_DEFAULT_INITIAL_DELAY_MS;
    let mut max_delay_ms = RETRY_DEFAULT_MAX_DELAY_MS;
    for part in rest.split(',').filter(|p| !p.is_empty()) {
        let (key, value) = part.split_once('=').ok_or_else(|| {
            Error::invalid(format!(
                "Invalid retries component {part:?} in spec {spec:?}: expected key=value"
            ))
        })?;
        let parse_u32 = |v: &str| -> Result<u32> {
            v.trim()
                .parse()
                .map_err(|_| Error::invalid(format!("Invalid integer {v:?} for retries {key:?}")))
        };
        match key.trim() {
            "max" => retries = Some(parse_u32(value)?),
            "backoff" => {
                backoff_coefficient = value.trim().parse().map_err(|_| {
                    Error::invalid(format!("Invalid float {value:?} for retries \"backoff\""))
                })?
            }
            "initial_ms" => initial_delay_ms = parse_u32(value)?,
            "max_ms" => max_delay_ms = parse_u32(value)?,
            other => {
                return Err(Error::invalid(format!(
                    "Unknown retries component {other:?} in spec {spec:?}"
                )))
            }
        }
    }
    let retries = retries.ok_or_else(|| {
        Error::invalid(format!(
            "Invalid retries spec {spec:?}: missing required \"max\" (the retry count)"
        ))
    })?;
    Ok(FunctionRetryPolicy {
        backoff_coefficient,
        initial_delay_ms,
        max_delay_ms,
        retries,
    })
}

/// Parse a Modal GPU spec into a [`GpuConfig`], mirroring `parse_gpu_config`
/// (modal `_utils/function_utils.py:628`). Format: `"TYPE"` or `"TYPE:count"`.
///
/// The MEM suffix (`"A100-80GB"`) is NOT split — it stays inside `gpu_type`
/// verbatim. `gpu_type` is uppercased; `count` defaults to `1`; the deprecated
/// `type` field (proto field 1, `GPUType`) stays `0` (Python never sets it). A
/// non-integer count maps to [`Error::invalid`], mirroring Python's `InvalidError`.
pub(super) fn parse_gpu_config(spec: &str) -> Result<GpuConfig> {
    // `split_once(':')` = Python's `value.split(":", 1)`.
    let (type_part, count) = match spec.split_once(':') {
        Some((lhs, rhs)) => {
            let count: u32 = rhs.trim().parse().map_err(|_| {
                Error::invalid(format!(
                    "Invalid GPU count: {rhs}. Value must be an integer."
                ))
            })?;
            (lhs, count)
        }
        None => (spec, 1),
    };
    Ok(GpuConfig {
        gpu_type: type_part.to_uppercase(), // `.upper()`
        count,
        ..Default::default() // r#type (deprecated GPUType, field 1) stays 0
    })
}

/// Parse a modal-rust schedule SPEC string into a [`Schedule`] proto, mirroring
/// Modal's `Cron`/`Period` constructors (`schedule.py`). The spec is the canonical,
/// const-string form the `#[function(schedule = ...)]` macro emits (a `&'static str`
/// is const-valid in the `inventory::submit!` static initializer, exactly like `gpu`).
///
/// Two forms, discriminated by the leading tag:
/// - `"cron:<timezone>:<cron_string>"` → `Schedule.Cron { cron_string, timezone }`.
///   The timezone is first because a cron string contains spaces but never a colon
///   (`split_once(':')` twice is unambiguous). An IANA timezone (`UTC`,
///   `America/New_York`) likewise has no colon.
/// - `"period:years=Y,months=M,weeks=W,days=D,hours=H,minutes=Mi,seconds=S"` →
///   `Schedule.Period { .. }`. Components are comma-separated `key=value`; any subset
///   may appear and omitted components default to `0` (only `seconds` is a float).
///
/// A malformed spec maps to [`Error::invalid`], mirroring Python's `InvalidError`.
pub(super) fn parse_schedule(spec: &str) -> Result<Schedule> {
    let oneof = if let Some(rest) = spec.strip_prefix("cron:") {
        // `<timezone>:<cron_string>` — timezone first (colon-free), cron string is the
        // remainder verbatim (it contains spaces, never a colon).
        let (timezone, cron_string) = rest.split_once(':').ok_or_else(|| {
            Error::invalid(format!(
                "Invalid cron schedule spec {spec:?}: expected \"cron:<timezone>:<cron_string>\""
            ))
        })?;
        ScheduleOneof::Cron(Cron {
            cron_string: cron_string.to_string(),
            timezone: timezone.to_string(),
        })
    } else if let Some(rest) = spec.strip_prefix("period:") {
        let mut period = Period::default();
        // Empty component list (`"period:"`) is a zero period; otherwise parse each
        // `key=value`. Unknown keys / bad numbers map to `Error::invalid`.
        for part in rest.split(',').filter(|p| !p.is_empty()) {
            let (key, value) = part.split_once('=').ok_or_else(|| {
                Error::invalid(format!(
                    "Invalid period component {part:?} in schedule spec {spec:?}: expected key=value"
                ))
            })?;
            let parse_i32 = |v: &str| -> Result<i32> {
                v.trim().parse().map_err(|_| {
                    Error::invalid(format!("Invalid integer {v:?} for period {key:?}"))
                })
            };
            match key.trim() {
                "years" => period.years = parse_i32(value)?,
                "months" => period.months = parse_i32(value)?,
                "weeks" => period.weeks = parse_i32(value)?,
                "days" => period.days = parse_i32(value)?,
                "hours" => period.hours = parse_i32(value)?,
                "minutes" => period.minutes = parse_i32(value)?,
                "seconds" => {
                    period.seconds = value.trim().parse().map_err(|_| {
                        Error::invalid(format!("Invalid float {value:?} for period \"seconds\""))
                    })?
                }
                other => {
                    return Err(Error::invalid(format!(
                        "Unknown period component {other:?} in schedule spec {spec:?}"
                    )))
                }
            }
        }
        ScheduleOneof::Period(period)
    } else {
        return Err(Error::invalid(format!(
            "Invalid schedule spec {spec:?}: expected a \"cron:..\" or \"period:..\" prefix"
        )));
    };
    Ok(Schedule {
        schedule_oneof: Some(oneof),
    })
}
