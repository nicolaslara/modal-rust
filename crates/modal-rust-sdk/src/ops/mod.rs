//! Typed control-plane operations: a native Rust port of the proven FILE-mode
//! recipe (the "shim-backend" spike), with the three modal-rs bug fixes baked in.
//!
//! Each operation family is its own submodule, all implemented as `impl
//! ModalClient` blocks so the public surface is method calls on a single client:
//!
//! - [`app`]    — `AppGetOrCreate` (preferred) / `AppCreate` (ephemeral) + `AppPublish` (fix #2).
//! - [`image`]  — `ImageGetOrCreate` (`from_registry` + wrapper bake) + `ImageJoinStreaming` poll.
//! - [`mount`]  — resolve the hosted client mount (`MountGetOrCreate`, GLOBAL) → `mount_id`.
//! - [`function`] — `FunctionPrecreate` + `FunctionCreate` (FILE mode, fix #1) + `FunctionGet`.
//! - [`invoke`] — `FunctionMap` → `FunctionPutInputs` fallback (fix #3) → poll `FunctionGetOutputs`.
//!
//! ## The three fixes (why modal-rs failed; see `workpads/shim-backend/spike-notes.md`)
//!
//! 1. `FunctionCreate` sends EXACTLY ONE of `function` / `function_data` (XOR) and
//!    ALWAYS sets `resources` ([`function`]).
//! 2. Deploy uses `AppPublish` ONLY — never the server-broken `AppSetObjects` ([`app`]).
//! 3. Invoke falls back to `FunctionPutInputs` when `FunctionMap` does not enqueue
//!    the input, then polls `FunctionGetOutputs` ([`invoke`]).

pub mod app;
pub mod blob;
pub mod function;
pub mod image;
pub mod invoke;
pub mod local_dir;
pub mod mount;

use crate::proto::api::generic_result::GenericStatus;
use crate::proto::api::GenericResult;

/// Modal client version we identify as. Keyed into the hosted client-mount name
/// (`modal-client-mount-{version}`) and sent as `x-modal-client-version`; kept in
/// sync with [`crate::auth::CLIENT_VERSION`].
pub const CLIENT_VERSION: &str = crate::auth::CLIENT_VERSION;

/// Default base image used by [`image`]'s `from_registry` recipe. FILE mode carries
/// no pickled bytecode, so the exact Python version is irrelevant to correctness;
/// a small, fast-pulling stock tag is preferred.
pub const DEFAULT_BASE_IMAGE: &str = "python:3.12-slim";

/// Classify a [`GenericResult`] as a terminal success / failure / still-building.
///
/// A `None` result, or a result whose status is `UNSPECIFIED`, means the remote
/// operation has not finished yet (keep polling). Any other status is terminal.
pub(crate) fn result_status(result: Option<&GenericResult>) -> ResultState {
    match result {
        None => ResultState::Pending,
        Some(r) => match GenericStatus::try_from(r.status).unwrap_or(GenericStatus::Unspecified) {
            GenericStatus::Unspecified => ResultState::Pending,
            GenericStatus::Success => ResultState::Success,
            other => ResultState::Failure(other),
        },
    }
}

/// Terminal state of a polled remote [`GenericResult`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResultState {
    /// Not finished yet — keep polling.
    Pending,
    /// Terminal success.
    Success,
    /// Terminal failure with the specific status.
    Failure(GenericStatus),
}

/// Render a terminal [`GenericResult`] failure into a human-readable reason that
/// surfaces the remote `exception` / `traceback` (spec §5.3 / §5.8).
pub(crate) fn describe_failure(
    context: &str,
    status: GenericStatus,
    result: &GenericResult,
) -> String {
    let mut msg = format!("{context} failed with {}", status.as_str_name());
    if !result.exception.is_empty() {
        msg.push_str(": ");
        msg.push_str(&result.exception);
    }
    if !result.traceback.is_empty() {
        msg.push('\n');
        msg.push_str(&result.traceback);
    }
    msg
}
