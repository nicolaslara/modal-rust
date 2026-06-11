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

/// Build and connect a channel to the given Modal server URL.
///
/// Three URL shapes (mirroring the Python client's `create_channel`,
/// `grpc_utils.py:185-210`):
/// - `https://…` — TLS (native OS roots + bundled webpki roots);
/// - `http://…` — plaintext (local dev servers, the in-process mock);
/// - `unix:/path` (or `unix:///path`) — a unix-domain socket. This is what Modal's
///   worker injects as `MODAL_SERVER_URL` inside EVERY container
///   (`unix:/run/modal.sock`, verified live): a worker-local proxy socket that is
///   ITSELF the credential — which is why `CLIENT_TYPE_CONTAINER` sends no token
///   headers.
pub async fn build_channel(server_url: &str) -> Result<Channel> {
    if let Some(path) = unix_socket_path(server_url) {
        return build_unix_channel(path).await;
    }
    let url = normalize_server_url(server_url);

    let mut endpoint = Channel::from_shared(url.clone())
        .map_err(|e| Error::invalid(format!("invalid Modal server url '{url}': {e}")))?
        .tcp_keepalive(Some(Duration::from_secs(30)))
        .http2_keep_alive_interval(Duration::from_secs(30))
        .keep_alive_timeout(Duration::from_secs(20))
        .initial_stream_window_size(Some(64 * 1024 * 1024)) // 64 MiB
        .initial_connection_window_size(Some(64 * 1024 * 1024));

    if url.starts_with("https://") {
        // Native OS roots PLUS the bundled webpki (Mozilla) roots: slim container
        // images carry no /etc/ssl/certs, so native-only TLS fails as a bare
        // "transport error" the first time an in-container client (a Dict/Queue
        // handle inside a `#[function]` body) dials api.modal.com. Bundling the
        // webpki roots mirrors what certifi gives the Python client.
        endpoint = endpoint.tls_config(
            ClientTlsConfig::new()
                .with_native_roots()
                .with_webpki_roots(),
        )?;
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

/// Extract the socket path from a `unix:` URL, accepting both the bare
/// `unix:/run/modal.sock` form Modal's worker injects and the `unix:///path`
/// authority form. `None` for non-unix URLs.
fn unix_socket_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("unix:")?;
    Some(rest.strip_prefix("//").unwrap_or(rest).to_string())
}

/// Connect over a unix-domain socket. The endpoint URI is a dummy (hyper requires
/// one); the connector ignores it and dials `path`. No TLS and no TCP keepalive —
/// it is a local socket.
#[cfg(unix)]
async fn build_unix_channel(path: String) -> Result<Channel> {
    use hyper_util::rt::TokioIo;
    use tonic::transport::{Endpoint, Uri};
    use tower::service_fn;

    let endpoint = Endpoint::try_from("http://[::1]:443")
        .map_err(|e| Error::invalid(format!("internal: dummy unix endpoint uri: {e}")))?
        .initial_stream_window_size(Some(64 * 1024 * 1024))
        .initial_connection_window_size(Some(64 * 1024 * 1024));
    let display_path = path.clone();
    endpoint
        .connect_with_connector(service_fn(move |_: Uri| {
            let path = path.clone();
            async move {
                Ok::<_, std::io::Error>(TokioIo::new(tokio::net::UnixStream::connect(path).await?))
            }
        }))
        .await
        .map_err(|e| {
            Error::invalid(format!(
                "failed to connect unix socket '{display_path}': {e}"
            ))
        })
}

/// Unix-domain sockets do not exist on this platform; Modal containers are Linux,
/// so this arm only triggers if someone points `MODAL_SERVER_URL` at a `unix:` URL
/// on a non-unix dev machine.
#[cfg(not(unix))]
async fn build_unix_channel(path: String) -> Result<Channel> {
    Err(Error::invalid(format!(
        "unix socket server URL 'unix:{path}' is not supported on this platform"
    )))
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
    fn unix_socket_path_accepts_both_forms() {
        // The bare form is what Modal's worker actually injects (verified live:
        // MODAL_SERVER_URL=unix:/run/modal.sock).
        assert_eq!(
            unix_socket_path("unix:/run/modal.sock").as_deref(),
            Some("/run/modal.sock")
        );
        assert_eq!(
            unix_socket_path("unix:///run/modal.sock").as_deref(),
            Some("/run/modal.sock")
        );
        assert_eq!(unix_socket_path("https://api.modal.com"), None);
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
