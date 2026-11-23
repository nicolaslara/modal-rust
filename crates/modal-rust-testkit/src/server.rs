//! [`MockModal`] — the live mock handle.
//!
//! Owns the running tonic server thread + the shared [`RequestLog`]. Built by
//! [`MockModal::builder`] / [`MockModal::start`]; exposes the mock's loopback
//! [`url`](MockModal::url), a ready-made [`modal_config`](MockModal::modal_config)
//! for the SDK, and the typed request-query surface
//! ([`last`](MockModal::last) / [`requests`](MockModal::requests) /
//! [`took`](MockModal::took)). Tears down (stops the server thread) on `Drop`.

use std::net::SocketAddr;

use crate::builder::MockModalBuilder;
use crate::proto::api::modal_client_server::ModalClientServer;
use crate::record::{FromRecorded, RecordedRequest, RequestLog};
use crate::responder::Responses;
use crate::servicer::MockServicer;

/// Stack size of the dedicated server thread.
///
/// The generated `ModalClientServer::call` dispatch keeps one stack slot per RPC arm
/// in unoptimized builds (about 1 MiB at 286 RPCs). Serving on the caller's runtime
/// would put that frame on the test thread, whose default stack is 2 MiB, so the
/// server gets its own thread with room for the dispatch to grow with the proto.
const SERVER_STACK_BYTES: usize = 16 * 1024 * 1024;

/// A live, in-process Modal gRPC mock on a loopback port.
///
/// Cheap to obtain ([`MockModal::start`] / [`MockModal::builder`]); the handle owns
/// the server thread and stops it on `Drop`, so a per-test (or per-table-case) mock
/// tears down cleanly with no shared global state.
pub struct MockModal {
    addr: SocketAddr,
    log: RequestLog,
    /// Dropping the sender ends the server thread's serve loop.
    _shutdown: tokio::sync::oneshot::Sender<()>,
}

impl MockModal {
    /// Start a ZERO-CONFIG happy-path mock (the table-test / quick-test common case).
    /// Equivalent to `MockModal::builder().start()`.
    pub async fn start() -> std::io::Result<MockModal> {
        Self::builder().start().await
    }

    /// Begin configuring a mock: canned function result + per-RPC override closures.
    /// See [`MockModalBuilder`].
    pub fn builder() -> MockModalBuilder {
        MockModalBuilder::default()
    }

    /// Bind a loopback port, spawn the server thread with the configured responses,
    /// and return the handle. Used by [`MockModalBuilder::start`].
    pub(crate) async fn start_with_responses(responses: Responses) -> std::io::Result<MockModal> {
        let log = RequestLog::default();
        let servicer = MockServicer::new(log.clone(), responses);

        // Kernel-assigned loopback port; capture the concrete addr for `url()`. The
        // socket is bound here so the address is known (and dialable) on return.
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;

        let (shutdown, stopped) = tokio::sync::oneshot::channel::<()>();
        std::thread::Builder::new()
            .name("mock-modal-server".into())
            .stack_size(SERVER_STACK_BYTES)
            .spawn(move || {
                // A runtime/serve error would surface to a test as a failed dial, so
                // it is intentionally swallowed here (the handle is already returned).
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                runtime.block_on(async move {
                    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                        return;
                    };
                    let incoming = tonic::transport::server::TcpIncoming::from(listener)
                        .with_nodelay(Some(true));
                    let serve = tonic::transport::Server::builder()
                        .add_service(ModalClientServer::new(servicer))
                        .serve_with_incoming(incoming);
                    // The server runs until the handle is dropped. Dropping the serve
                    // future closes every connection at once; a graceful shutdown would
                    // wait on connections the test still holds open.
                    tokio::select! {
                        _ = serve => {}
                        _ = stopped => {}
                    }
                });
            })?;

        Ok(MockModal {
            addr,
            log,
            _shutdown: shutdown,
        })
    }

    /// The mock's base URL, e.g. `http://127.0.0.1:54321`. Feed this to the SDK /
    /// facade — the SDK channel dials plain `http://` with no transport change.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The bound loopback socket address.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// A ready-to-use [`modal_rust_sdk::ModalConfig`] pointed at this mock, with
    /// DUMMY credentials (no real Modal). Pass to
    /// [`modal_rust_sdk::ModalClient::from_config`] for an SDK-level test.
    pub fn modal_config(&self) -> modal_rust_sdk::ModalConfig {
        modal_rust_sdk::ModalConfig {
            profile: "mock".into(),
            server_url: self.url(),
            token_id: "ak-mock".into(),
            token_secret: "as-mock".into(),
            environment: Some("main".into()),
            image_builder_version: None,
            is_container: false,
        }
    }

    /// Every recorded request of type `T`, in arrival order — e.g.
    /// `mock.requests::<FunctionCreateRequest>()`.
    pub fn requests<T: FromRecorded + Clone>(&self) -> Vec<T> {
        self.log.requests::<T>()
    }

    /// The LAST recorded request of type `T` (`None` if absent) — e.g.
    /// `mock.last::<FunctionCreateRequest>()`.
    pub fn last<T: FromRecorded + Clone>(&self) -> Option<T> {
        self.log.last::<T>()
    }

    /// Count of recorded requests of type `T` (Python's
    /// `len(ctx.get_requests("X"))`) — e.g. `mock.took::<FunctionMapRequest>()`.
    pub fn took<T: FromRecorded + Clone>(&self) -> usize {
        self.log.took::<T>()
    }

    /// ALL recorded requests in arrival order (the untyped enum), for assertions
    /// that span RPC types or count totals.
    pub fn all_requests(&self) -> Vec<RecordedRequest> {
        self.log.all()
    }

    /// Total number of recorded requests across all RPCs.
    pub fn request_count(&self) -> usize {
        self.log.len()
    }

    /// A cheap clone of the shared request log, for code that wants to hold the log
    /// independently of the handle.
    pub fn log(&self) -> RequestLog {
        self.log.clone()
    }
}
