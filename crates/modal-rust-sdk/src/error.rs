//! Error type and `Result` alias for the Modal SDK.
//!
//! The public error type is a flat enum (`Error`) covering the failure modes of
//! config resolution, channel/transport construction, gRPC status responses, and
//! the operation surface. Helper constructors keep call sites terse; `From` impls
//! make `?` ergonomic against the underlying tonic / toml / io errors.

use std::fmt;

/// The error type for `modal-rust-sdk` operations.
#[derive(Debug)]
pub enum Error {
    /// Credential / config-file resolution failure (missing tokens, unreadable
    /// `~/.modal.toml`, unknown `MODAL_PROFILE`, malformed TOML, …).
    Config(String),
    /// gRPC channel / TLS / connection establishment failure.
    Transport(tonic::transport::Error),
    /// A gRPC call returned a non-OK status (includes auth rejections).
    Status(tonic::Status),
    /// Invalid input or unsupported configuration prepared client-side
    /// (e.g. a server URL that cannot be parsed, an un-encodable header value).
    Invalid(String),
    /// CBOR (or other payload) encode/decode failure.
    Codec(String),
    /// A Modal build/operation terminated with a remote failure result
    /// (surfaces `GenericResult.exception` for image/function builds).
    Build(String),
}

impl Error {
    /// Construct a [`Error::Config`] from any displayable value.
    pub fn config(msg: impl fmt::Display) -> Self {
        Error::Config(msg.to_string())
    }

    /// Construct a [`Error::Invalid`] from any displayable value.
    pub fn invalid(msg: impl fmt::Display) -> Self {
        Error::Invalid(msg.to_string())
    }

    /// Construct a [`Error::Codec`] from any displayable value.
    pub fn codec(msg: impl fmt::Display) -> Self {
        Error::Codec(msg.to_string())
    }

    /// Construct a [`Error::Build`] from any displayable value.
    pub fn build(msg: impl fmt::Display) -> Self {
        Error::Build(msg.to_string())
    }

    /// Map a header-value parse failure (token/version/platform metadata that is
    /// not valid ASCII for a gRPC header) into a clean [`Error::Invalid`].
    pub fn invalid_metadata(err: impl fmt::Display) -> Self {
        Error::Invalid(format!("invalid gRPC metadata value: {err}"))
    }

    /// Whether this error is a transient transport blip that is safe to retry
    /// (connection reset, h2 protocol body read, broken pipe, `UNAVAILABLE`,
    /// deadline-exceeded). Long-poll streams (image builds) reconnect on these;
    /// callers may also retry whole operations. Terminal failures
    /// ([`Error::Build`], [`Error::Status`] with a definite error like
    /// `INVALID_ARGUMENT`/`UNAUTHENTICATED`) are NOT transient.
    pub fn is_transient(&self) -> bool {
        match self {
            // Channel/TLS establishment errors are virtually always transient.
            Error::Transport(_) => true,
            Error::Status(s) => {
                use tonic::Code;
                if matches!(
                    s.code(),
                    Code::Unavailable
                        | Code::DeadlineExceeded
                        | Code::ResourceExhausted
                        // h2/transport resets land on Unknown/Internal mid-stream;
                        // Python (grpc_utils.py) + modal-rs retry both. Adding the
                        // codes makes us robust to message wording the sniff misses.
                        | Code::Internal
                        | Code::Unknown
                ) {
                    return true;
                }
                // h2/transport resets often arrive as Unknown/Internal with a
                // recognizable message; sniff the text.
                let m = s.message().to_ascii_lowercase();
                m.contains("connection reset")
                    || m.contains("connectionreset")
                    || m.contains("error reading a body")
                    || m.contains("h2 protocol error")
                    || m.contains("broken pipe")
                    || m.contains("transport error")
                    || m.contains("socket connection closed")
                    || m.contains("goaway")
                    || m.contains("connection closed")
            }
            _ => false,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Config(msg) => write!(f, "configuration error: {msg}"),
            Error::Transport(e) => write!(f, "transport error: {e}"),
            Error::Status(s) => write!(f, "gRPC error: {s}"),
            Error::Invalid(msg) => write!(f, "invalid input: {msg}"),
            Error::Codec(msg) => write!(f, "codec error: {msg}"),
            Error::Build(msg) => write!(f, "build error: {msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Transport(e) => Some(e),
            Error::Status(e) => Some(e),
            Error::Config(_) | Error::Invalid(_) | Error::Codec(_) | Error::Build(_) => None,
        }
    }
}

impl From<tonic::transport::Error> for Error {
    fn from(e: tonic::transport::Error) -> Self {
        Error::Transport(e)
    }
}

impl From<tonic::Status> for Error {
    fn from(e: tonic::Status) -> Self {
        Error::Status(e)
    }
}

impl From<tonic::metadata::errors::InvalidMetadataValue> for Error {
    fn from(e: tonic::metadata::errors::InvalidMetadataValue) -> Self {
        Error::invalid_metadata(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Config(format!("IO error: {e}"))
    }
}

impl From<toml::de::Error> for Error {
    fn from(e: toml::de::Error) -> Self {
        Error::Config(format!("TOML parse error: {e}"))
    }
}

/// A specialized `Result` for `modal-rust-sdk` operations.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::{Code, Status};

    fn status(code: Code) -> Error {
        Error::Status(Status::new(code, "x"))
    }

    #[test]
    fn transient_codes_are_retryable() {
        for code in [
            Code::Unavailable,
            Code::DeadlineExceeded,
            Code::ResourceExhausted,
            Code::Internal,
            Code::Unknown,
        ] {
            assert!(
                status(code).is_transient(),
                "{code:?} must be classified transient"
            );
        }
    }

    #[test]
    fn terminal_codes_are_not_retryable() {
        for code in [
            Code::Unauthenticated,
            Code::PermissionDenied,
            Code::InvalidArgument,
            Code::NotFound,
            Code::AlreadyExists,
            Code::FailedPrecondition,
        ] {
            assert!(
                !status(code).is_transient(),
                "{code:?} must surface immediately (never retried)"
            );
        }
    }

    #[test]
    fn build_and_config_are_not_transient() {
        assert!(!Error::build("remote build failed").is_transient());
        assert!(!Error::config("missing token").is_transient());
        assert!(!Error::invalid("bad arg").is_transient());
        assert!(!Error::codec("bad cbor").is_transient());
    }

    #[test]
    fn reset_message_text_is_transient_even_on_ok_code() {
        // A reset reported as Code::Ok-with-text still trips the substring sniff.
        let e = Error::Status(Status::new(Code::Ok, "h2 protocol error: connection reset"));
        assert!(e.is_transient());
    }
}
