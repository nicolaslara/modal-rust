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
//! - [`volume`] — `VolumeGetOrCreate` (create-if-missing, V2) → `volume_id` (P6 cargo cache).
//! - [`secret`] — `SecretGetOrCreate` (from_name lookup / from_dict create) → `secret_id`.
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
pub mod planning;
pub mod secret;
mod transport;
pub mod volume;

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
    // A TERMINATED/TIMEOUT status with an EMPTY exception + traceback gives the user
    // nothing to act on — the container was killed before it could report. The common
    // cause on the RUN path is the remote process being OOM-killed during a heavy
    // in-body `cargo build` (a GPU/ML crate like `burn-add`/`cuda-vector-add`), which
    // loses the client-visible build output. Append an actionable hint pointing at
    // `deploy` (builds at image-build time with full resources) + a CUDA base image +
    // a higher `memory=`. See docs/local/burn-add-run-failure.md for the confirmed root
    // cause. Skipped when the remote DID report an exception/traceback (the real error
    // is already above).
    let no_remote_detail = result.exception.is_empty() && result.traceback.is_empty();
    if no_remote_detail && matches!(status, GenericStatus::Terminated | GenericStatus::Timeout) {
        msg.push_str(
            "\n\nhint: the remote container was killed with no error output — most often \
             an OUT-OF-MEMORY kill during a heavy in-body `cargo build` (e.g. a GPU/ML \
             crate). For such crates, prefer `modal-rust deploy` (builds at image-build \
             time with full resources, not in the function body) with a CUDA base image \
             and more memory:\n  \
             MODAL_RUST_BASE_IMAGE=nvidia/cuda:<tag>-devel MODAL_RUST_INSTALL_RUST=1 \
             modal-rust deploy <fn> --app <name>\n\
             then call it with `modal-rust call <fn> --app <name> --input '{...}'`. Also \
             raise `memory=` on the #[modal_rust::function]. The build log (lost \
             client-side when the container is killed) is in `modal app logs`. See \
             docs/local/burn-add-run-failure.md.",
        );
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(status: GenericStatus, exception: &str, traceback: &str) -> GenericResult {
        GenericResult {
            status: status as i32,
            exception: exception.to_string(),
            traceback: traceback.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn terminated_with_no_detail_appends_oom_deploy_hint() {
        let r = result(GenericStatus::Terminated, "", "");
        let msg = describe_failure("run", GenericStatus::Terminated, &r);
        assert!(
            msg.contains("GENERIC_STATUS_TERMINATED"),
            "names the status: {msg}"
        );
        // The actionable hint: OOM cause + deploy + base image + memory + logs pointer.
        assert!(msg.contains("OUT-OF-MEMORY"), "{msg}");
        assert!(msg.contains("modal-rust deploy"), "{msg}");
        assert!(msg.contains("MODAL_RUST_BASE_IMAGE"), "{msg}");
        assert!(msg.contains("memory="), "{msg}");
        assert!(msg.contains("modal app logs"), "{msg}");
        assert!(msg.contains("docs/local/burn-add-run-failure.md"), "{msg}");
    }

    #[test]
    fn timeout_with_no_detail_also_gets_the_hint() {
        let r = result(GenericStatus::Timeout, "", "");
        let msg = describe_failure("run", GenericStatus::Timeout, &r);
        assert!(
            msg.contains("hint:"),
            "TIMEOUT with no detail gets the hint: {msg}"
        );
    }

    #[test]
    fn terminated_with_remote_exception_does_not_append_hint() {
        // When the remote DID report an exception, the real error is shown and the
        // speculative OOM hint is suppressed (no double-talk).
        let r = result(GenericStatus::Terminated, "RuntimeError: boom", "");
        let msg = describe_failure("run", GenericStatus::Terminated, &r);
        assert!(msg.contains("RuntimeError: boom"), "{msg}");
        assert!(
            !msg.contains("hint:"),
            "no OOM hint when a real exception exists: {msg}"
        );
    }

    #[test]
    fn ordinary_failure_status_is_unchanged() {
        // A plain FAILURE with an exception is rendered exactly as before — no hint.
        let r = result(GenericStatus::Failure, "ValueError: x", "Traceback...");
        let msg = describe_failure("call", GenericStatus::Failure, &r);
        assert_eq!(
            msg,
            "call failed with GENERIC_STATUS_FAILURE: ValueError: x\nTraceback..."
        );
    }
}
