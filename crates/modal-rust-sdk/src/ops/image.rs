//! Image operations: `ImageGetOrCreate` (a `from_registry` base plus the FILE-mode
//! wrapper module made importable) with an `ImageJoinStreaming` build poll → `image_id`.
//!
//! The image is a single registry layer: `FROM <base>` followed by `RUN` commands
//! that base64-decode the wrapper Python source to an importable path
//! (`/root/<module>.py`; `/root` is on `sys.path` in Modal containers). The
//! Modal-native way to make the `modal` **source** importable is the **client
//! mount** ([`crate::ops::mount`]) attached via `Function.mount_ids`.
//!
//! ## Client mount supplies source; the base image must supply the deps
//!
//! LIVE FINDING (2026-06-04, this crate's first mount-only round-trip): the hosted
//! client mount carries only the `modal` + `synchronicity` *source* packages
//! (mounted at `/pkg`), NOT their third-party pip dependencies (`typing_extensions`,
//! `grpclib`, `protobuf`, `aiohttp`, `cbor2`, `rich`, `toml`, `watchfiles`, …).
//! Booting `python -m modal._container_entrypoint` on a bare `python:3-slim` base
//! therefore crash-loops with `ModuleNotFoundError: No module named
//! 'typing_extensions'` and the function produces no output. Real Modal users never
//! hit this because their images derive from a base that already carries the
//! client's dependency closure.
//!
//! So the base image must provide the client's pip dependencies. The robust,
//! version-correct way to materialize exactly that closure is [`pip install
//! modal`](ImageSpec::with_pip_install_modal) (pip resolves the deps for the mounted
//! client version; the mount's `/pkg` still wins on `PYTHONPATH`, so the mounted
//! source remains authoritative). This is no longer a "crude shortcut" — for a bare
//! registry base it is REQUIRED alongside the mount. It is still OFF by default
//! because a dependency-provisioned base (e.g. a Modal-style slim image) does not
//! need it.

use std::time::Duration;

use base64::Engine;

use crate::client::ModalClient;
use crate::error::{Error, Result};
use crate::ops::{describe_failure, result_status, ResultState, DEFAULT_BASE_IMAGE};
use crate::proto::api::{Image, ImageGetOrCreateRequest, ImageJoinStreamingRequest};

/// Per-stream timeout (seconds) for `ImageJoinStreaming` long-poll reconnects.
const JOIN_STREAM_TIMEOUT_SECS: f32 = 55.0;
/// Safety cap on total wall-clock time spent polling an image build.
const BUILD_DEADLINE: Duration = Duration::from_secs(600);

/// Declarative recipe for a registry-based FILE-mode image.
///
/// Produces a single-layer `Image`: `FROM <base_image>` then `RUN` commands that
/// bake each `(module_name, source)` wrapper into `/root/<module_name>.py`.
#[derive(Debug, Clone)]
pub struct ImageSpec {
    /// Base registry tag (default [`DEFAULT_BASE_IMAGE`]).
    pub base_image: String,
    /// Raw Dockerfile commands rendered BEFORE the wrapper bakes (and before the
    /// optional pip line). Used to provision a runtime that the bake step needs —
    /// e.g. `apt-get install python3` on a bare `rust:1-slim` base, whose
    /// base64-decode bake (`python3 -c ...`) requires python3 to already exist.
    /// See [`ImageSpec::with_apt`].
    pub pre_bake_commands: Vec<String>,
    /// Wrapper modules to bake: `(module_name, python_source)`. Each is written to
    /// `/root/<module_name>.py` (an importable path inside the container).
    pub wrapper_modules: Vec<(String, String)>,
    /// Extra raw `RUN`/`ENV`/… Dockerfile commands appended after the bakes.
    pub extra_commands: Vec<String>,
    /// Off by default: append `RUN python3 -m pip install --no-cache-dir modal` to
    /// provision the modal client's pip dependency closure into the image. REQUIRED for a
    /// bare registry base (the client mount supplies only the modal *source*, not
    /// its deps — see the module docs); unnecessary for a base that already carries
    /// those deps. The mounted source at `/pkg` still wins on `PYTHONPATH`.
    pub pip_install_modal: bool,
}

impl ImageSpec {
    /// A registry-based image: `from_registry(base)`.
    pub fn from_registry(base_image: impl Into<String>) -> Self {
        Self {
            base_image: base_image.into(),
            pre_bake_commands: Vec::new(),
            wrapper_modules: Vec::new(),
            extra_commands: Vec::new(),
            pip_install_modal: false,
        }
    }

    /// A registry-based image on the default base tag.
    pub fn default_base() -> Self {
        Self::from_registry(DEFAULT_BASE_IMAGE)
    }

    /// Bake a wrapper module: writes `source` to `/root/<module_name>.py`.
    pub fn with_wrapper_module(
        mut self,
        module_name: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        self.wrapper_modules
            .push((module_name.into(), source.into()));
        self
    }

    /// Append a raw Dockerfile command (e.g. `"RUN apt-get update"`).
    pub fn with_command(mut self, command: impl Into<String>) -> Self {
        self.extra_commands.push(command.into());
        self
    }

    /// Append a canonical `apt-get install` line to [`pre_bake_commands`] (rendered
    /// BEFORE the wrapper bakes). Required on a bare base whose bake step itself
    /// runs `python3 -c ...`: the runtime must exist before the bake. Renders the
    /// proven single-RUN form (update + install + clean) so quoting is correct:
    ///
    /// ```text
    /// RUN apt-get update && apt-get install -y --no-install-recommends <pkgs> && rm -rf /var/lib/apt/lists/*
    /// ```
    ///
    /// [`pre_bake_commands`]: ImageSpec::pre_bake_commands
    pub fn with_apt(mut self, packages: &[&str]) -> Self {
        let pkgs = packages.join(" ");
        self.pre_bake_commands.push(format!(
            "RUN apt-get update && apt-get install -y --no-install-recommends {pkgs} \
             && rm -rf /var/lib/apt/lists/*"
        ));
        self
    }

    /// Provision the modal client's pip dependency closure via `pip install
    /// modal`. Required for a bare registry base; the client mount only supplies
    /// the modal *source* (see the module docs).
    pub fn with_pip_install_modal(mut self) -> Self {
        self.pip_install_modal = true;
        self
    }

    /// Render the full `dockerfile_commands` list: `FROM`, pre-bake commands (e.g.
    /// apt), optional pip line, the wrapper bakes, then extra commands.
    ///
    /// Order is load-bearing: pre-bake commands MUST precede both the pip line and
    /// the bakes, because on a bare base they provision the runtime (`python3`)
    /// those later steps invoke.
    fn dockerfile_commands(&self) -> Vec<String> {
        let mut cmds = vec![format!("FROM {}", self.base_image)];
        cmds.extend(self.pre_bake_commands.iter().cloned());
        if self.pip_install_modal {
            // `python3 -m pip` is universal: it works on a slim apt-provisioned
            // python (which may expose no bare `pip` shim) AND on stock `python:`
            // bases. Replaces the bare `pip install` form.
            //
            // `--break-system-packages` is required on modern Debian bases
            // (bookworm/trixie, e.g. `rust:1-slim` → Python 3.13) whose apt python
            // is PEP-668 externally-managed: without it, `pip install` aborts with
            // `error: externally-managed-environment` and the image build fails
            // (live-verified 2026-06-04). The flag is a benign no-op on stock
            // `python:` bases that are not externally-managed, so it is safe to
            // always emit. We install into the system site-packages deliberately:
            // this is a throwaway build image, and the mounted client `/pkg` still
            // wins on `PYTHONPATH`, so the mounted modal source stays authoritative.
            cmds.push(
                "RUN python3 -m pip install --no-cache-dir --break-system-packages modal"
                    .to_string(),
            );
        }
        for (module_name, source) in &self.wrapper_modules {
            cmds.push(bake_command(module_name, source));
        }
        cmds.extend(self.extra_commands.iter().cloned());
        cmds
    }

    /// The `Image` proto message for this spec. `base_images` is empty because we
    /// base on a registry `FROM <tag>` line (only layered builds populate it).
    fn to_proto(&self) -> Image {
        Image {
            dockerfile_commands: self.dockerfile_commands(),
            ..Default::default()
        }
    }
}

/// A `RUN` command that base64-decodes `source` into `/root/<module_name>.py`.
///
/// Encoding the source avoids any heredoc/quoting escapes reaching the Dockerfile.
fn bake_command(module_name: &str, source: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(source.as_bytes());
    format!(
        "RUN python3 -c \"import base64,pathlib; \
         pathlib.Path('/root/{module_name}.py').write_bytes(base64.b64decode('{b64}'))\""
    )
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
        let builder_version = self.image_builder_version().unwrap_or_default().to_string();

        let resp = self
            .inner_mut()
            .image_get_or_create(ImageGetOrCreateRequest {
                image: Some(spec.to_proto()),
                app_id: app_id.to_string(),
                builder_version,
                ..Default::default()
            })
            .await?
            .into_inner();

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_registry_renders_from_line_first() {
        let spec = ImageSpec::from_registry("python:3.12-slim")
            .with_wrapper_module("spike_wrapper", "def handler(p):\n    return p\n");
        let cmds = spec.dockerfile_commands();
        assert_eq!(cmds[0], "FROM python:3.12-slim");
        assert!(cmds[1].contains("/root/spike_wrapper.py"));
        // No pip fallback by default (client mount is the native path).
        assert!(!cmds.iter().any(|c| c.contains("pip install")));
    }

    #[test]
    fn pip_fallback_is_opt_in_and_before_bakes() {
        let cmds = ImageSpec::default_base()
            .with_pip_install_modal()
            .with_wrapper_module("m", "x = 1\n")
            .dockerfile_commands();
        assert_eq!(cmds[0], format!("FROM {DEFAULT_BASE_IMAGE}"));
        assert!(cmds[1].contains("python3 -m pip install"));
        assert!(cmds[1].contains("--break-system-packages"));
        assert!(cmds[1].ends_with(" modal"));
    }

    #[test]
    fn apt_renders_before_pip_and_bake() {
        let cmds = ImageSpec::from_registry("rust:1-slim")
            .with_apt(&["python3", "python3-pip"])
            .with_pip_install_modal()
            .with_wrapper_module(
                "modal_rust_run_wrapper",
                "def handler(e, i):\n    return i\n",
            )
            .with_command("ENTRYPOINT []")
            .dockerfile_commands();
        assert_eq!(cmds[0], "FROM rust:1-slim");
        // apt line first (provisions python3 the bake/pip steps invoke).
        assert!(cmds[1].starts_with("RUN apt-get update"));
        assert!(cmds[1].contains("python3 python3-pip"));
        // pip uses `python3 -m pip` (universal launcher), AFTER apt.
        let pip_idx = cmds
            .iter()
            .position(|c| c.contains("python3 -m pip install"))
            .expect("pip line present");
        let bake_idx = cmds
            .iter()
            .position(|c| c.contains("/root/modal_rust_run_wrapper.py"))
            .expect("bake line present");
        assert!(pip_idx > 1, "apt must precede pip");
        assert!(bake_idx > pip_idx, "pip must precede the wrapper bake");
        // ENTRYPOINT [] is last (extra command).
        assert_eq!(cmds.last().unwrap(), "ENTRYPOINT []");
    }

    #[test]
    fn bake_command_round_trips_source_via_base64() {
        let src = "def handler(payload):\n    return payload\n";
        let cmd = bake_command("spike_wrapper", src);
        // Extract the base64 blob and confirm it decodes back to the source.
        let b64 = cmd
            .split("b64decode('")
            .nth(1)
            .and_then(|s| s.split('\'').next())
            .expect("embedded base64");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .expect("valid base64");
        assert_eq!(decoded, src.as_bytes());
    }
}
