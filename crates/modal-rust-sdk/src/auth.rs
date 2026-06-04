//! gRPC auth interceptor attaching the Modal `x-modal-*` metadata headers.
//!
//! Modal authenticates by request metadata, not a login RPC: every unary/stream
//! call carries the token id/secret plus client-identity headers. We send the
//! full Python header set (spec §4.3) so the control plane treats us as a
//! first-class Python-equivalent client — `x-modal-client-type = "1"`
//! (`CLIENT_TYPE_CLIENT`, api.proto:84) is the header that matters most.

use std::time::{SystemTime, UNIX_EPOCH};

use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::Interceptor;
use tonic::{Request, Status};

use crate::error::{Error, Result};

/// Our SDK client version (bumped from modal-rs's "1.3.1"; matches the verified
/// Modal 1.3.2 facts). Sent as `x-modal-client-version` and used to key the
/// hosted client mount (`modal-client-mount-{version}`) in later phases.
pub const CLIENT_VERSION: &str = "1.3.2";

/// `ClientType::CLIENT_TYPE_CLIENT` (api.proto:84) as its enum-int string. We
/// replicate the Python FILE-mode path, not the libmodal JS/Go identities.
pub const CLIENT_TYPE_CLIENT: &str = "1";

/// Authentication interceptor that injects the required Modal metadata headers
/// into every gRPC request. Cheap to clone (all values are pre-parsed).
#[derive(Clone)]
pub struct AuthInterceptor {
    token_id: MetadataValue<Ascii>,
    token_secret: MetadataValue<Ascii>,
    client_version: MetadataValue<Ascii>,
    client_type: MetadataValue<Ascii>,
    platform: MetadataValue<Ascii>,
}

impl AuthInterceptor {
    /// Build the interceptor from validated credentials. Constant headers
    /// (version/type) are infallible; the platform string degrades to
    /// `"unknown"` if it ever fails to parse as an ASCII header.
    pub fn new(token_id: &str, token_secret: &str) -> Result<Self> {
        Ok(Self {
            token_id: token_id.parse().map_err(Error::invalid_metadata)?,
            token_secret: token_secret.parse().map_err(Error::invalid_metadata)?,
            client_version: CLIENT_VERSION.parse().map_err(Error::invalid_metadata)?,
            client_type: CLIENT_TYPE_CLIENT
                .parse()
                .map_err(Error::invalid_metadata)?,
            platform: platform_string()
                .parse()
                .unwrap_or_else(|_| ascii_literal("unknown")),
        })
    }
}

impl Interceptor for AuthInterceptor {
    fn call(&mut self, mut req: Request<()>) -> std::result::Result<Request<()>, Status> {
        let md = req.metadata_mut();
        md.insert("x-modal-token-id", self.token_id.clone());
        md.insert("x-modal-token-secret", self.token_secret.clone());
        md.insert("x-modal-client-type", self.client_type.clone());
        md.insert("x-modal-client-version", self.client_version.clone());
        md.insert("x-modal-platform", self.platform.clone());
        // Per-call timestamp (grpc_utils.py:364); best-effort — never fails the call.
        if let Ok(ts) = format_unix_secs().parse::<MetadataValue<Ascii>>() {
            md.insert("x-modal-timestamp", ts);
        }
        Ok(req)
    }
}

/// `"{system}-{release}-{machine}"`, percent-escaped to stay header-safe. This
/// is a diagnostic header (harmless if approximate).
fn platform_string() -> String {
    let system = std::env::consts::OS; // "macos", "linux", …
    let machine = std::env::consts::ARCH; // "x86_64", "aarch64", …
    let raw = format!("{system}-{machine}");
    percent_escape_ascii(&raw)
}

/// Minimal percent-escape keeping only header-safe ASCII; anything else becomes
/// `%XX`. Avoids pulling a URL-encoding crate for a diagnostic header.
fn percent_escape_ascii(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        let safe = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
        if safe {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Current Unix time in fractional seconds (matches the Python client format).
fn format_unix_secs() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    format!("{secs}")
}

/// Parse an ASCII literal known to be valid at compile time.
fn ascii_literal(s: &str) -> MetadataValue<Ascii> {
    s.parse().expect("static ascii header literal")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_rejects_non_ascii_token() {
        // A header value containing a control char / non-visible byte is invalid.
        assert!(AuthInterceptor::new("bad\nvalue", "secret").is_err());
    }

    #[test]
    fn new_accepts_typical_tokens() {
        let interceptor = AuthInterceptor::new("ak-abc123", "as-def456");
        assert!(interceptor.is_ok());
    }

    #[test]
    fn interceptor_sets_all_headers() {
        let mut interceptor = AuthInterceptor::new("ak-abc123", "as-def456").unwrap();
        let req = interceptor.call(Request::new(())).unwrap();
        let md = req.metadata();
        assert_eq!(md.get("x-modal-token-id").unwrap(), "ak-abc123");
        assert_eq!(md.get("x-modal-token-secret").unwrap(), "as-def456");
        assert_eq!(md.get("x-modal-client-type").unwrap(), CLIENT_TYPE_CLIENT);
        assert_eq!(md.get("x-modal-client-version").unwrap(), CLIENT_VERSION);
        assert!(md.get("x-modal-platform").is_some());
        assert!(md.get("x-modal-timestamp").is_some());
    }

    #[test]
    fn platform_string_is_header_safe() {
        let p = platform_string();
        assert!(!p.is_empty());
        // No spaces / control chars survive escaping.
        assert!(p.bytes().all(|b| b.is_ascii_graphic()));
        let _: MetadataValue<Ascii> = p.parse().expect("platform header parses");
    }

    #[test]
    fn percent_escape_replaces_unsafe_bytes() {
        assert_eq!(percent_escape_ascii("a b/c"), "a%20b%2Fc");
        assert_eq!(percent_escape_ascii("linux-x86_64"), "linux-x86_64");
    }
}
