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
    /// A control-plane (remote) operation failed. Reserved for the RUN-path surface
    /// (`.remote()`/`.spawn()`/`.map()`/`FunctionCall::get`) and `connect()`.
    ///
    /// Gated on `client`: the SDK error type only exists when the gRPC client is
    /// compiled in.
    #[cfg(feature = "client")]
    Sdk(modal_rust_sdk::Error),
    /// A RUN-path call (`.remote()`/`.spawn()`/`.map()`) was made on an
    /// [`App`](crate::App) that was built offline (`App::local`/`App::local_with_registry`)
    /// and never [`connect`](crate::App::connect)ed. Remote execution needs the live
    /// control-plane handle.
    NotConnected(String),
    /// A surface intentionally not wired yet: carries a message pointing to the
    /// alternative. (No facade surface returns this currently; retained as a stable
    /// variant for future stubs.)
    NotImplemented(String),
    /// An invalid facade CONFIG was supplied (e.g. a user volume whose mount path
    /// collides with the reserved cargo-cache mount). Distinct from a control-plane
    /// failure: nothing was sent to Modal.
    Config(String),
}

/// The facade `Result` alias.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Build the standard [`Error::NotConnected`] for `.remote()` on an offline App.
    /// Used only by the client surface, so the LIGHT build allows it dead.
    #[cfg_attr(not(feature = "client"), allow(dead_code))]
    pub(crate) fn not_connected() -> Error {
        Error::NotConnected(
            "`.remote()` requires a connected App: call `App::connect(name).await` \
             (App::local / App::local_with_registry are offline-only, for `.local()`)."
                .to_string(),
        )
    }

    /// Build an [`Error::Config`] from a message (invalid facade config). Used only by
    /// the client surface (remote/deploy/dump), so the LIGHT build allows it dead.
    #[cfg_attr(not(feature = "client"), allow(dead_code))]
    pub(crate) fn config(msg: impl Into<String>) -> Error {
        Error::Config(msg.into())
    }

    /// The called surface needs the `client` feature, which is OFF in this build.
    /// Returned by the DEFAULT-build stubs of the talk-to-Modal surface
    /// (`App::connect*`, `.remote()/.spawn()/.map()`, `deploy`/`call`, …) so a
    /// function-only crate ALWAYS compiles light and gets a clear error if it calls
    /// the client path. Always present (the light build needs it).
    #[allow(dead_code)] // unused when `client` is on (the real bodies replace the stubs)
    pub(crate) fn client_feature(what: &str) -> Error {
        Error::NotImplemented(format!(
            "{what} requires the `client` feature on modal-rust: set \
             `modal-rust = {{ features = [\"client\"] }}` in your Cargo.toml \
             (the modal-rust CLI enables it for you). The default build is light \
             (no gRPC client) so authoring `#[function]`s + `.local()` stays fast."
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
            #[cfg(feature = "client")]
            Error::Sdk(e) => write!(f, "control-plane operation failed: {e}"),
            Error::NotConnected(msg) => write!(f, "{msg}"),
            Error::NotImplemented(msg) => write!(f, "{msg}"),
            Error::Config(msg) => write!(f, "invalid config: {msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Runner(e) => Some(e),
            Error::Encode(e) => Some(e),
            Error::Decode(e) => Some(e),
            #[cfg(feature = "client")]
            Error::Sdk(e) => Some(e),
            Error::UnknownEntrypoint { .. }
            | Error::NotConnected(_)
            | Error::NotImplemented(_)
            | Error::Config(_) => None,
        }
    }
}

impl From<RunnerError> for Error {
    fn from(e: RunnerError) -> Self {
        Error::Runner(e)
    }
}

#[cfg(feature = "client")]
impl From<modal_rust_sdk::Error> for Error {
    fn from(e: modal_rust_sdk::Error) -> Self {
        Error::Sdk(e)
    }
}

// NOTE: deliberately NO blanket `From<serde_json::Error>` — the same serde error
// type covers both the encode (input) and decode (output) boundaries, and they
// MUST map to distinct variants. Construct `Error::Encode` / `Error::Decode`
// explicitly at the two `.local()` call sites.
