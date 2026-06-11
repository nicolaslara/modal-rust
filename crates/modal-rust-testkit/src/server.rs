//! [`MockModal`] — the live mock handle.
//!
//! Owns the running tonic server task + the shared [`RequestLog`]. Built by
//! [`MockModal::builder`] / [`MockModal::start`]; exposes the mock's loopback
//! [`url`](MockModal::url), a ready-made [`modal_config`](MockModal::modal_config)
//! for the SDK, and the typed request-query surface
//! ([`last`](MockModal::last) / [`requests`](MockModal::requests) /
//! [`took`](MockModal::took)). Tears down (aborts the task) on `Drop`.

use std::net::SocketAddr;

use crate::builder::MockModalBuilder;
use crate::proto::api::modal_client_server::ModalClientServer;
use crate::record::{FromRecorded, RecordedRequest, RequestLog};
use crate::responder::Responses;
use crate::servicer::MockServicer;

/// A live, in-process Modal gRPC mock on a loopback port.
///
/// Cheap to obtain ([`MockModal::start`] / [`MockModal::builder`]); the handle owns
/// the server task and aborts it on `Drop`, so a per-test (or per-table-case) mock
/// tears down cleanly with no shared global state.
pub struct MockModal {
    addr: SocketAddr,
    log: RequestLog,
    task: tokio::task::JoinHandle<()>,
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

    /// Bind a loopback port, spawn the server task with the configured responses,
    /// and return the handle. Used by [`MockModalBuilder::start`].
    pub(crate) async fn start_with_responses(responses: Responses) -> std::io::Result<MockModal> {
        let log = RequestLog::default();
        let servicer = MockServicer::new(log.clone(), responses);

        // Kernel-assigned loopback port; capture the concrete addr for `url()`.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let incoming =
            tonic::transport::server::TcpIncoming::from(listener).with_nodelay(Some(true));

        let task = tokio::spawn(async move {
            // The server runs until the task is aborted on `Drop`. A bind/serve error
            // would surface to a test as a failed dial, so it is intentionally
            // swallowed here (the handle is already returned).
            let _ = tonic::transport::Server::builder()
                .add_service(ModalClientServer::new(servicer))
                .serve_with_incoming(incoming)
                .await;
        });

        Ok(MockModal { addr, log, task })
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

impl Drop for MockModal {
    fn drop(&mut self) {
        self.task.abort();
    }
}
