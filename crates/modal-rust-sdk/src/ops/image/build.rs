//! The BUILD side: the pure `ImageGetOrCreate` request builder and the
//! `ModalClient` build/long-poll RPCs (`ImageJoinStreaming` until a terminal
//! `GenericResult`). Split out of `image.rs` mechanically (M1).

use std::time::Duration;

use super::render::ImageSpec;
use crate::client::ModalClient;
use crate::error::{Error, Result};
use crate::ops::{describe_failure, result_status, ResultState};
use crate::proto::api::{ImageGetOrCreateRequest, ImageJoinStreamingRequest};

/// Per-stream timeout (seconds) for `ImageJoinStreaming` long-poll reconnects.
const JOIN_STREAM_TIMEOUT_SECS: f32 = 55.0;
/// Safety cap on total wall-clock time spent polling an image build.
const BUILD_DEADLINE: Duration = Duration::from_secs(600);

/// Build the `ImageGetOrCreate` request (api.proto:4260) — pure, no I/O.
///
/// Extracted from [`ModalClient::image_get_or_create`]; the method resolves the
/// `builder_version` (config override > environment setting) and passes it in. The
/// image sub-message comes from [`ImageSpec::to_proto`] (already covered by the
/// `dockerfile_commands` / `to_proto` sub-message tests); this wraps it with
/// `app_id` + `builder_version`.
pub(crate) fn build_image_get_or_create_request(
    spec: &ImageSpec,
    app_id: &str,
    builder_version: String,
) -> ImageGetOrCreateRequest {
    ImageGetOrCreateRequest {
        image: Some(spec.to_proto()),
        app_id: app_id.to_string(),
        builder_version,
        ..Default::default()
    }
}

impl ModalClient {
    /// Build (or fetch the cached) image for `spec` under `app_id` and return its
    /// `image_id`, blocking until the build finishes.
    ///
    /// Issues `ImageGetOrCreate` (api.proto:4260). The response's `image_id` is set
    /// regardless of build state; if the build has not finished
    /// ([`ResultState::Pending`]) we long-poll `ImageJoinStreaming`
    /// (api.proto:4261), advancing `last_entry_id` across reconnects, until a
    /// terminal `GenericResult` arrives. A terminal failure surfaces as
    /// [`Error::Build`] carrying the remote `exception`/`traceback`.
    ///
    /// `environment` is unused by `ImageGetOrCreate` directly (the image is scoped
    /// to `app_id`); the parameter is accepted for call-site symmetry.
    pub async fn image_get_or_create(&mut self, app_id: &str, spec: &ImageSpec) -> Result<String> {
        // Resolve the builder version (config override > environment setting). The
        // worker only mounts the client dep closure at container start for a builder
        // > "2024.10", so the add_python image needs a modern version pinned here — an
        // empty version TERMINATES the container at boot (no modal deps). See
        // [`ModalClient::resolved_image_builder_version`].
        let builder_version = self.resolved_image_builder_version().await;

        // Modal dedups images by content hash: re-issuing the initial
        // get-or-create returns the same image_id/build, so it is safe to retry on
        // a transient reset. (The build POLL has its own reconnect loop below.)
        let req = build_image_get_or_create_request(spec, app_id, builder_version);
        let resp = self
            .retry_rpc("image_get_or_create", req, |mut stub, req| async move {
                stub.image_get_or_create(req).await
            })
            .await?;

        let image_id = resp.image_id;
        if image_id.is_empty() {
            return Err(Error::build(
                "ImageGetOrCreate returned an empty image_id".to_string(),
            ));
        }

        // Already finished? Branch on the inline result.
        match result_status(resp.result.as_ref()) {
            ResultState::Success => return Ok(image_id),
            ResultState::Failure(status) => {
                let result = resp.result.expect("failure implies a result");
                return Err(Error::build(describe_failure(
                    "image build",
                    status,
                    &result,
                )));
            }
            ResultState::Pending => {}
        }

        self.poll_image_build(&image_id).await?;
        Ok(image_id)
    }

    /// Long-poll `ImageJoinStreaming` until the build reaches a terminal result.
    ///
    /// Image builds routinely outlast a single gRPC stream window (a heavy
    /// `pip install` / `apt-get` step can take minutes), so the long-poll connection
    /// is reconnected — resuming from `last_entry_id` — on BOTH a clean window end
    /// AND a transient transport reset (`h2 protocol error` / connection reset),
    /// bounded by [`BUILD_DEADLINE`]. A *real* build failure is never a transport
    /// error: it arrives in-band as a terminal [`ResultState::Failure`], which we
    /// always surface immediately. So this reconnect-on-transient logic cannot mask
    /// a genuine build failure — it only rides out the network blips Modal warns
    /// long polls will see.
    async fn poll_image_build(&mut self, image_id: &str) -> Result<()> {
        let started = std::time::Instant::now();
        let mut last_entry_id = String::new();

        loop {
            if started.elapsed() > BUILD_DEADLINE {
                return Err(Error::build(format!(
                    "image build {image_id} did not finish within {}s",
                    BUILD_DEADLINE.as_secs()
                )));
            }

            match self.drain_build_window(image_id, &mut last_entry_id).await {
                Ok(Some(())) => return Ok(()),
                // Clean window end with no terminal result — reconnect and resume.
                Ok(None) => {}
                // Transient transport reset mid-poll — reconnect and resume from the
                // last entry. Re-surface any non-transient error (e.g. auth).
                Err(err) if err.is_transient() => {
                    eprintln!("[image-build] stream reconnect after transient error: {err}");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// Open one `ImageJoinStreaming` window and drain it. Returns `Ok(Some(()))` on
    /// terminal success, `Ok(None)` when the window ends without a terminal result
    /// (caller should reconnect), and `Err` for a terminal build failure or a
    /// transport error (the caller decides whether the latter is retryable).
    async fn drain_build_window(
        &mut self,
        image_id: &str,
        last_entry_id: &mut String,
    ) -> Result<Option<()>> {
        let mut stream = self
            .inner_mut()
            .image_join_streaming(ImageJoinStreamingRequest {
                image_id: image_id.to_string(),
                timeout: JOIN_STREAM_TIMEOUT_SECS,
                last_entry_id: last_entry_id.clone(),
                include_logs_for_finished: true,
            })
            .await?
            .into_inner();

        while let Some(item) = stream.message().await? {
            for log in &item.task_logs {
                if !log.data.is_empty() {
                    eprint!("[image-build] {}", log.data);
                }
            }
            if !item.entry_id.is_empty() {
                *last_entry_id = item.entry_id;
            }
            match result_status(item.result.as_ref()) {
                ResultState::Success => return Ok(Some(())),
                ResultState::Failure(status) => {
                    let result = item.result.expect("failure implies a result");
                    return Err(Error::build(describe_failure(
                        "image build",
                        status,
                        &result,
                    )));
                }
                ResultState::Pending => {}
            }
        }
        Ok(None)
    }
}
