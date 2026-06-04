//! The facade error type.
//!
//! [`Error`] wraps the frozen runtime's [`RunnerError`] taxonomy verbatim while
//! adding the facade-level failure modes the runtime does not model: an
//! unknown-entrypoint error (the runtime only builds that inside `run_cli`, never
//! from `Registry::get`), the two distinct serde JSON boundaries around
//! `.local()`, control-plane (`sdk`) failures, and the intentionally-stubbed
//! remote surface.

use crate::RunnerError; // re-exported from modal_rust_runtime

/// The facade's error type for App/Function operations.
#[derive(Debug)]
pub enum Error {
    /// Entrypoint name absent from the App's [`crate::Registry`]. Carries the
    /// requested name plus the known names (mirrors `run_cli`'s diagnostic). Built
    /// by the facade — `Registry::get` returning `None` yields no [`RunnerError`].
    UnknownEntrypoint {
        /// The entrypoint name that was requested but not found.
        name: String,
        /// The names that ARE registered, for a helpful diagnostic.
        known: Vec<String>,
    },
    /// In-process handler failed: the FROZEN five-kind taxonomy wrapped verbatim
    /// (handler decode of `In` / function body `Err` / encode of `Out` / panic /
    /// runtime unknown-entrypoint).
    Runner(RunnerError),
    /// Serializing the `.local()` input to JSON failed (BEFORE the handler ran).
    Encode(serde_json::Error),
    /// Deserializing the handler's JSON output into `Out` failed (AFTER it ran).
    Decode(serde_json::Error),
    /// A control-plane (remote) operation failed. Reserved for
    /// `.remote()`/`.spawn()`/`.map()` and `connect()`.
    Sdk(modal_rust_sdk::Error),
    /// A surface intentionally not wired this milestone
    /// (`.remote()`/`.spawn()`/`.map()`/`FunctionCall::get`): carries a message
    /// pointing to the next workflow.
    NotImplemented(String),
}

/// The facade `Result` alias.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Build the standard [`Error::NotImplemented`] for a remote surface, with a
    /// message pointing to the next milestone (SDK source-upload).
    pub(crate) fn not_implemented(surface: &str) -> Error {
        Error::NotImplemented(format!(
            "`{surface}` is not implemented yet: remote execution needs SDK \
             source-upload (MountPutFile/blob), which modal-rust-sdk does not \
             have yet. Tracked as the next workflow milestone. Use .local() for \
             in-process execution today."
        ))
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::UnknownEntrypoint { name, known } => {
                write!(
                    f,
                    "unknown entrypoint {name:?}; known entrypoints: [{}]",
                    known
                        .iter()
                        .map(|n| format!("{n:?}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            Error::Runner(e) => write!(f, "handler failed: {e}"),
            Error::Encode(e) => write!(f, "failed to encode .local() input to JSON: {e}"),
            Error::Decode(e) => write!(f, "failed to decode handler output from JSON: {e}"),
            Error::Sdk(e) => write!(f, "control-plane operation failed: {e}"),
            Error::NotImplemented(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Runner(e) => Some(e),
            Error::Encode(e) => Some(e),
            Error::Decode(e) => Some(e),
            Error::Sdk(e) => Some(e),
            Error::UnknownEntrypoint { .. } | Error::NotImplemented(_) => None,
        }
    }
}

impl From<RunnerError> for Error {
    fn from(e: RunnerError) -> Self {
        Error::Runner(e)
    }
}

impl From<modal_rust_sdk::Error> for Error {
    fn from(e: modal_rust_sdk::Error) -> Self {
        Error::Sdk(e)
    }
}

// NOTE: deliberately NO blanket `From<serde_json::Error>` — the same serde error
// type covers both the encode (input) and decode (output) boundaries, and they
// MUST map to distinct variants. Construct `Error::Encode` / `Error::Decode`
// explicitly at the two `.local()` call sites.
