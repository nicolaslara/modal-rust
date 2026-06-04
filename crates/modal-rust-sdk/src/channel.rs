//! TLS gRPC channel construction for the Modal control plane.
//!
//! Isolated from `client.rs` (modal-rs bundled them) to keep TLS/endpoint
//! hardening in one place. The endpoint tuning mirrors the Python client
//! (`grpc_utils.py:196-214`): keepalives plus large HTTP/2 windows so big image
//! / CBOR payloads and long-poll streams (image build, function outputs) flow
//! without flow-control stalls.

use std::time::Duration;

use tonic::transport::{Channel, ClientTlsConfig};

use crate::error::{Error, Result};

/// Build and connect a TLS (or plain-HTTP for local dev) channel to the given
/// Modal server URL. TLS (native OS root store) is applied only for `https://`
/// URLs so a local `http://` dev server still works.
pub async fn build_channel(server_url: &str) -> Result<Channel> {
    let url = normalize_server_url(server_url);

    let mut endpoint = Channel::from_shared(url.clone())
        .map_err(|e| Error::invalid(format!("invalid Modal server url '{url}': {e}")))?
        .tcp_keepalive(Some(Duration::from_secs(30)))
        .http2_keep_alive_interval(Duration::from_secs(30))
        .keep_alive_timeout(Duration::from_secs(20))
        .initial_stream_window_size(Some(64 * 1024 * 1024)) // 64 MiB
        .initial_connection_window_size(Some(64 * 1024 * 1024));

    if url.starts_with("https://") {
        endpoint = endpoint.tls_config(ClientTlsConfig::new().with_native_roots())?;
    }

    endpoint.connect().await.map_err(Error::from)
}

/// Prepend `https://` when the URL carries no explicit scheme.
pub fn normalize_server_url(url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else {
        format!("https://{url}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_adds_https_scheme() {
        assert_eq!(
            normalize_server_url("api.modal.com"),
            "https://api.modal.com"
        );
    }

    #[test]
    fn normalize_preserves_existing_scheme() {
        assert_eq!(
            normalize_server_url("https://api.modal.com"),
            "https://api.modal.com"
        );
        assert_eq!(
            normalize_server_url("http://localhost:8080"),
            "http://localhost:8080"
        );
    }
}
