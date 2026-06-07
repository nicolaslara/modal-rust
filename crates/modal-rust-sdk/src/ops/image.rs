//! Image operations: `ImageGetOrCreate` (a `from_registry` base plus the FILE-mode
//! wrapper module made importable) with an `ImageJoinStreaming` build poll → `image_id`.
//!
//! The image is a single registry layer: `FROM <base>` followed by `RUN` commands
//! that base64-decode the wrapper Python source to an importable path
//! (`/root/<module>.py`; `/root` is on `sys.path` in Modal containers). The
//! Modal-native way to make the `modal` **source** importable is the **client
//! mount** ([`crate::ops::mount`]) attached via `Function.mount_ids`.
//!
//! ## How Python + the modal client are provisioned (PRIMARY: `add_python`)
//!
//! We replicate the official Python client. Two pieces land independently:
//!
//! 1. **The Python interpreter** comes from the HOSTED python-build-standalone mount
//!    ([`ModalClient::python_standalone_mount_id`]), resolved by NAME exactly like
//!    the client mount — NO apt, NO build step. It is attached as the image build
//!    CONTEXT (`Image.context_mount_id`); the rendered Dockerfile then emits the
//!    client-blessed `_registry_setup_commands` add_python branch
//!    (_image.py:2041-2059): `COPY /python/. /usr/local`, an auto
//!    `RUN ln -s /usr/local/bin/python3 /usr/local/bin/python` for series < 3.13
//!    (the client-equivalent of `python-is-python3`, but a symlink against the
//!    standalone install — NOT an apt package), and `ENV TERMINFO_DIRS=…`. A
//!    standalone interpreter is relocatable and is NOT PEP-668 externally-managed,
//!    so `--break-system-packages` is moot.
//! 2. **The modal client source** rides the separate client mount (mounted at
//!    `/pkg`), attached via `Function.mount_ids` ([`crate::ops::mount`]).
//! 3. **The modal client's third-party dep closure** (`typing_extensions`,
//!    `grpclib`, `protobuf`, `aiohttp`, `cbor2`, `rich`, …) is injected by the worker
//!    AT CONTAINER START on the modern image builder (> "2024.10"), requested via
//!    `Function.mount_client_dependencies = true`
//!    ([`crate::ops::function::FunctionSpec`], proto field 82). Real Modal images on
//!    the current builder therefore do NOT `pip install modal`; the worker mounts
//!    both the source (client mount) and the deps (server-side). See
//!    `_image.py:2061-2074` ("past 2024.10, client dependencies are mounted at
//!    runtime") and `_functions.py:936-939`.
//!
//! Net: a `from_registry(<base>).with_add_python("3.12")` image has NO apt layer and
//! NO pip layer — just `COPY`/`ln`/`ENV` + the wrapper bake — so the build is a short
//! `ImageJoinStreaming` stream with far fewer transport resets.
//!
//! ## Documented fallback: apt + `pip install modal`
//!
//! The legacy provisioning ([`ImageSpec::with_apt`] + [`pip install
//! modal`](ImageSpec::with_pip_install_modal)) is retained ONLY as a documented
//! fallback for a base that already carries the deps, or an environment where
//! runtime dep-mounting is unavailable. It is selected ONLY when `add_python` is
//! unset. On a bare apt-provisioned Debian Python, `pip install` requires
//! `--break-system-packages` (PEP-668 externally-managed) and the entrypoint's bare
//! `python` requires the `python-is-python3` apt package — the three hacks
//! `add_python` dissolves.

use std::time::Duration;

use base64::Engine;

use crate::client::ModalClient;
use crate::error::{Error, Result};
use crate::ops::{describe_failure, result_status, ResultState, DEFAULT_BASE_IMAGE};
use crate::proto::api::{
    BaseImage, Image, ImageContextFile, ImageGetOrCreateRequest, ImageJoinStreamingRequest,
};

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
    /// User image-builder steps (PARITY.md §3: `pip_install` / `apt_install` /
    /// `run_commands`), already rendered to Dockerfile `RUN` lines, IN THE ORDER the
    /// user chained them. Emitted AFTER the python/rust provisioning (so `pip`/`apt`
    /// have a Python/toolchain on PATH) and BEFORE the wrapper bakes (so any system
    /// lib a Rust binary dynamically links is present when the runner runs — and on
    /// the DEPLOY base layer, present when `cargo build` runs at image-build time).
    /// Default empty ⇒ NO rendered-command drift on the default path. See
    /// [`ImageSpec::with_pip_install`], [`ImageSpec::with_apt_install`],
    /// [`ImageSpec::with_run_commands`].
    pub builder_steps: Vec<String>,
    /// Extra raw `RUN`/`ENV`/… Dockerfile commands appended after the bakes.
    pub extra_commands: Vec<String>,
    /// Off by default: append `RUN python3 -m pip install --no-cache-dir modal` to
    /// provision the modal client's pip dependency closure into the image. REQUIRED for a
    /// bare registry base (the client mount supplies only the modal *source*, not
    /// its deps — see the module docs); unnecessary for a base that already carries
    /// those deps. The mounted source at `/pkg` still wins on `PYTHONPATH`.
    pub pip_install_modal: bool,
    /// Image build CONTEXT mount id (a `mount_id` from
    /// [`ModalClient::mount_local_dir`]). When set, emitted as
    /// `Image.context_mount_id` (proto field 15); a `COPY` step (added via
    /// [`ImageSpec::with_command`]) then brings the context into an image LAYER at
    /// build time so `cargo build` can compile the source AT image-build time.
    /// `None` for the RUN path (default); DEPLOY-only.
    pub context_mount_id: Option<String>,
    /// Inline small context files → `Image.context_files` (proto field 7). Unused
    /// by the Rust deploy recipe (the source rides the context mount, not inline
    /// files); kept for proto parity. Default empty.
    pub context_files: Vec<(String, Vec<u8>)>,
    /// PRIMARY Python provisioning: the python-build-standalone series to add (e.g.
    /// `"3.12"`). When `Some`, the rendered Dockerfile emits the client's add_python
    /// branch (`COPY /python/. /usr/local`, the `ln -s python3 python` for series <
    /// 3.13, and the `TERMINFO_DIRS` ENV) and SUPPRESSES the apt/pip fallback lines.
    /// See [`ImageSpec::with_add_python`] and the module docs. `None` ⇒ the apt+pip
    /// fallback render branch.
    pub add_python: Option<String>,
    /// The HOSTED python-build-standalone mount id (from
    /// [`ModalClient::python_standalone_mount_id`]) that supplies `/python` to the
    /// `COPY /python/. /usr/local` add_python step. Emitted as
    /// `Image.context_mount_id` (proto field 15) when `context_mount_id` is not
    /// otherwise occupied by a source mount — see [`ImageSpec::with_add_python`] and
    /// the layered DEPLOY path ([`ImageSpec::with_base_image`]).
    pub python_standalone_mount_id: Option<String>,
    /// Base image id for a LAYERED build (`Image.base_images`, proto field 5). When
    /// `Some`, the image is layer N on top of a previously-built layer: the rendered
    /// Dockerfile starts with `FROM base` (NOT `FROM <registry tag>`) and the proto
    /// carries `base_images = [BaseImage { docker_tag: "base", image_id }]`. Used by
    /// the DEPLOY two-layer image so the source mount (this layer's
    /// `context_mount_id`) and the standalone mount (layer 1's `context_mount_id`)
    /// each get their own build context. See [`ImageSpec::with_base_image`].
    pub base_image_id: Option<String>,
    /// Off by default: install the Rust toolchain (rustup) + the CUDA build/run env
    /// into the image. REQUIRED when the base image carries NO Rust (e.g. a
    /// `nvidia/cuda:<ver>-devel` Tier-1 base; boundaries.md §9), so the in-body /
    /// image-build-time `cargo build` has a toolchain on PATH. Ports the PROVEN
    /// `gpu_app.py` recipe (M13): an apt prereq + rustup single-RUN, then a baked
    /// `PATH` (with `/root/.cargo/bin` + `/usr/local/cuda/bin`) and
    /// `CUDA_PATH=/usr/local/cuda` (load-bearing — tells CubeCL where the runtime
    /// NVRTC include path `$CUDA_PATH/include` is). Rendered AFTER the add_python
    /// if/else and BEFORE the wrapper bakes, so it composes with `add_python`
    /// (python/modal) WITHOUT touching the mutually-exclusive add_python/apt branches.
    /// See [`ImageSpec::with_rust_toolchain`]. Default `false` ⇒ NO rendered-command
    /// drift on the default `rust:1-slim` + add_python path.
    pub install_rust: bool,
}

impl ImageSpec {
    /// A registry-based image: `from_registry(base)`.
    pub fn from_registry(base_image: impl Into<String>) -> Self {
        Self {
            base_image: base_image.into(),
            pre_bake_commands: Vec::new(),
            wrapper_modules: Vec::new(),
            builder_steps: Vec::new(),
            extra_commands: Vec::new(),
            pip_install_modal: false,
            context_mount_id: None,
            context_files: Vec::new(),
            add_python: None,
            python_standalone_mount_id: None,
            base_image_id: None,
            install_rust: false,
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

    /// Image-builder step (PARITY.md §3, Modal `_image.py:992` `pip_install`): install
    /// arbitrary Python packages. Renders the canonical single `RUN` line:
    ///
    /// ```text
    /// RUN python3 -m pip install --no-cache-dir <pkgs>
    /// ```
    ///
    /// Appended to [`builder_steps`] in chain order (so it composes with
    /// [`with_apt_install`](ImageSpec::with_apt_install) /
    /// [`with_run_commands`](ImageSpec::with_run_commands), preserving the user's
    /// ordering). No-op for an empty slice. `python3 -m pip` is the universal launcher
    /// (works on the standalone interpreter `add_python` provisions and on stock
    /// `python:` bases); `--no-cache-dir` keeps the throwaway build image lean.
    ///
    /// [`builder_steps`]: ImageSpec::builder_steps
    pub fn with_pip_install(mut self, packages: &[&str]) -> Self {
        if !packages.is_empty() {
            let pkgs = packages.join(" ");
            self.builder_steps
                .push(format!("RUN python3 -m pip install --no-cache-dir {pkgs}"));
        }
        self
    }

    /// Image-builder step (PARITY.md §3, Modal `_image.py:2508` `apt_install`): install
    /// arbitrary system packages. Renders the proven single `RUN` form (update +
    /// install + clean) so quoting is correct and no apt cache is left in the layer:
    ///
    /// ```text
    /// RUN apt-get update && apt-get install -y --no-install-recommends <pkgs> && rm -rf /var/lib/apt/lists/*
    /// ```
    ///
    /// Appended to [`builder_steps`] in chain order. No-op for an empty slice. Unlike
    /// [`with_apt`](ImageSpec::with_apt) (which targets `pre_bake_commands`, the
    /// runtime the BAKE step itself needs), this is a general chainable image step that
    /// composes with `pip_install` / `run_commands` in user order.
    ///
    /// [`builder_steps`]: ImageSpec::builder_steps
    pub fn with_apt_install(mut self, packages: &[&str]) -> Self {
        if !packages.is_empty() {
            let pkgs = packages.join(" ");
            self.builder_steps.push(format!(
                "RUN apt-get update && apt-get install -y --no-install-recommends {pkgs} \
                 && rm -rf /var/lib/apt/lists/*"
            ));
        }
        self
    }

    /// Image-builder step (PARITY.md §3, Modal `_image.py:1893` `run_commands`): run
    /// arbitrary shell commands at image-build time. Each command renders as one
    /// `RUN <cmd>` line, appended to [`builder_steps`] in chain order. No-op for an
    /// empty slice. The commands are emitted VERBATIM (the user owns the shell), so
    /// they compose with `pip_install` / `apt_install` in the order chained.
    ///
    /// [`builder_steps`]: ImageSpec::builder_steps
    pub fn with_run_commands(mut self, commands: &[&str]) -> Self {
        for cmd in commands {
            self.builder_steps.push(format!("RUN {cmd}"));
        }
        self
    }

    /// Set the image build-context mount (a `mount_id` from
    /// [`ModalClient::mount_local_dir`]). Emitted as `Image.context_mount_id`
    /// (proto field 15). The caller adds the matching `COPY` step (use
    /// [`ImageSpec::with_command`], e.g. `"COPY . /"`) plus the build `RUN`s; those
    /// ride `extra_commands` and render LAST, after the context is available.
    /// DEPLOY-only (the RUN path leaves this `None`).
    pub fn with_context_mount(mut self, mount_id: impl Into<String>) -> Self {
        self.context_mount_id = Some(mount_id.into());
        self
    }

    /// PRIMARY Python provisioning: add the python-build-standalone `series` (e.g.
    /// `"3.12"`) the way the official client does. Renders the add_python branch
    /// (`COPY /python/. /usr/local` + `ln -s` for series < 3.13 + `TERMINFO_DIRS`)
    /// and suppresses the apt/pip fallback. Pair with
    /// [`ImageSpec::with_python_standalone_mount_id`] (supplies `/python` as the
    /// build context) and, on the function, `mount_client_dependencies = true` (the
    /// worker injects the client's dep closure at container start). See the module
    /// docs.
    pub fn with_add_python(mut self, series: impl Into<String>) -> Self {
        self.add_python = Some(series.into());
        self
    }

    /// Set the HOSTED python-build-standalone mount id (from
    /// [`ModalClient::python_standalone_mount_id`]) that supplies `/python` to the
    /// add_python `COPY`. For the RUN path this becomes the image's
    /// `context_mount_id`; for the layered DEPLOY path the source owns this layer's
    /// `context_mount_id`, so the standalone mount belongs on the BASE layer (also
    /// set there via this method). See [`ImageSpec::with_base_image`].
    pub fn with_python_standalone_mount_id(mut self, mount_id: impl Into<String>) -> Self {
        self.python_standalone_mount_id = Some(mount_id.into());
        self
    }

    /// Make this spec a LAYER on top of a previously-built image (`base_image_id`).
    /// The rendered Dockerfile starts with `FROM base` instead of `FROM <tag>`, and
    /// the proto carries `base_images = [BaseImage { docker_tag: "base", image_id }]`
    /// (proto field 5). Used by the DEPLOY two-layer image so the source mount and
    /// the python-standalone mount each occupy their own layer's `context_mount_id`.
    pub fn with_base_image(mut self, base_image_id: impl Into<String>) -> Self {
        self.base_image_id = Some(base_image_id.into());
        self
    }

    /// Install the Rust toolchain (rustup) + bake the CUDA build/run env into the
    /// image. Use when the base image carries NO Rust — e.g. a
    /// `nvidia/cuda:<ver>-devel` Tier-1 base (boundaries.md §9) — so the
    /// `cargo build` (in-body for the RUN path, at image-build time for the DEPLOY
    /// base layer) finds `cargo` on PATH. Ports the PROVEN `gpu_app.py` recipe (M13):
    /// an apt prereq + rustup single-RUN, then a baked `PATH`
    /// (`/root/.cargo/bin:/usr/local/cuda/bin:…`) and `CUDA_PATH=/usr/local/cuda`
    /// (so CubeCL resolves the runtime NVRTC include path `$CUDA_PATH/include`).
    /// Renders AFTER the add_python if/else and BEFORE the wrapper bakes, so it
    /// composes with `add_python` (the project's PRIMARY python provisioning) without
    /// touching the mutually-exclusive add_python/apt branches. The default base never
    /// sets this, so the default path stays byte-identical. See [`install_rust`].
    ///
    /// [`install_rust`]: ImageSpec::install_rust
    pub fn with_rust_toolchain(mut self) -> Self {
        self.install_rust = true;
        self
    }

    /// Render the full `dockerfile_commands` list.
    ///
    /// The opening line is `FROM base` for a LAYERED build ([`base_image_id`] set,
    /// referencing the prior layer via `base_images[0].docker_tag = "base"`,
    /// mirroring the client's `base_images={"base": self}` + `"FROM base"` pattern,
    /// _image.py:725-727) or `FROM <base_image>` for a registry base.
    ///
    /// PRIMARY ([`add_python`] set): emit the client's add_python branch
    /// (`COPY /python/. /usr/local`, the `ln -s python3 python` for series < 3.13,
    /// `ENV TERMINFO_DIRS=…`; _image.py:2041-2059) and SUPPRESS the apt/pip fallback.
    /// FALLBACK ([`add_python`] unset): the apt pre-bake commands then the optional
    /// pip line (order is load-bearing — both provision the runtime the bake invokes).
    /// Then the wrapper bakes, then the extra commands.
    ///
    /// [`base_image_id`]: ImageSpec::base_image_id
    /// [`add_python`]: ImageSpec::add_python
    fn dockerfile_commands(&self) -> Vec<String> {
        let from_line = if self.base_image_id.is_some() {
            // Layered build: reference the prior layer by the conventional tag.
            "FROM base".to_string()
        } else {
            format!("FROM {}", self.base_image)
        };
        let mut cmds = vec![from_line];

        if let Some(series) = &self.add_python {
            // Replicate `_registry_setup_commands` add_python branch
            // (_image.py:2041-2059): COPY the standalone tree onto PATH, symlink
            // `python` for series < 3.13 (the client-equivalent of python-is-python3
            // — a symlink, not an apt package), then the TERMINFO ENV. The standalone
            // mount supplies `/python` via this layer's `context_mount_id`.
            cmds.push("COPY /python/. /usr/local".to_string());
            if python_series_lt_13(series) {
                cmds.push("RUN ln -s /usr/local/bin/python3 /usr/local/bin/python".to_string());
            }
            cmds.push(
                "ENV TERMINFO_DIRS=/etc/terminfo:/lib/terminfo:/usr/share/terminfo:/usr/lib/terminfo"
                    .to_string(),
            );
            // No apt, no pip: the client's dep closure is injected at container start
            // via Function.mount_client_dependencies (see the module docs).
        } else {
            // FALLBACK provisioning: apt pre-bake commands, then the optional pip line.
            cmds.extend(self.pre_bake_commands.iter().cloned());
            if self.pip_install_modal {
                // `python3 -m pip` is universal: it works on a slim apt-provisioned
                // python (which may expose no bare `pip` shim) AND on stock `python:`
                // bases. Replaces the bare `pip install` form.
                //
                // `--break-system-packages` is required on modern Debian bases
                // (bookworm/trixie, e.g. `rust:1-slim` → Python 3.13) whose apt
                // python is PEP-668 externally-managed: without it, `pip install`
                // aborts with `error: externally-managed-environment` and the image
                // build fails (live-verified 2026-06-04). The flag is a benign no-op
                // on stock `python:` bases that are not externally-managed, so it is
                // safe to always emit. We install into the system site-packages
                // deliberately: this is a throwaway build image, and the mounted
                // client `/pkg` still wins on `PYTHONPATH`, so the mounted modal
                // source stays authoritative.
                cmds.push(
                    "RUN python3 -m pip install --no-cache-dir --break-system-packages modal"
                        .to_string(),
                );
            }
        }

        if self.install_rust {
            // PROVEN `gpu_app.py` (M13) recipe, rendered AFTER the add_python if/else
            // (python/modal) and BEFORE the wrapper bakes. The CUDA base needs BOTH
            // add_python AND apt+rustup, but the add_python and apt branches above are
            // mutually exclusive (apt's `pre_bake_commands` are suppressed once
            // `add_python` is set), so the rustup steps live in this dedicated block
            // instead of `with_apt`. rustup does not need python, so the order is fine.
            //
            // Single combined RUN (minimal layers): apt prereqs for rustup, then the
            // exact rustup one-liner from `gpu_app.py`.
            cmds.push(
                "RUN apt-get update && apt-get install -y --no-install-recommends \
                 curl ca-certificates build-essential pkg-config \
                 && rm -rf /var/lib/apt/lists/* \
                 && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
                 | sh -s -- -y --default-toolchain stable --profile minimal"
                    .to_string(),
            );
            // Bake PATH (cargo + the CUDA bin dir) and CUDA_PATH as image ENV so they
            // hold at BOTH image-build time (the DEPLOY top layer's `cargo build`) AND
            // runtime (the RUN-path in-body build + CubeCL's NVRTC include resolution).
            // LD_LIBRARY_PATH is intentionally NOT set: the `-devel` image's
            // /etc/ld.so.conf.d already puts /usr/local/cuda/lib64 on the loader path,
            // and M13 passed without it.
            cmds.push(
                "ENV PATH=/root/.cargo/bin:/usr/local/cuda/bin:/usr/local/sbin:\
                 /usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
                    .to_string(),
            );
            cmds.push("ENV CUDA_PATH=/usr/local/cuda".to_string());
        }

        // User image-builder steps (pip_install / apt_install / run_commands), in the
        // order chained. Rendered AFTER the python/rust provisioning (so pip/apt find a
        // Python/toolchain on PATH) and BEFORE the wrapper bakes — so a system lib a
        // Rust binary dynamically links is present at runtime (RUN path) and at
        // image-build time (DEPLOY base layer feeds the top layer's `cargo build`).
        // Empty by default ⇒ byte-identical default path.
        cmds.extend(self.builder_steps.iter().cloned());

        for (module_name, source) in &self.wrapper_modules {
            cmds.push(bake_command(module_name, source));
        }
        cmds.extend(self.extra_commands.iter().cloned());
        cmds
    }

    /// The build-context mount id this image's `Image.context_mount_id` (proto field
    /// 15) should carry. An explicit source context mount ([`context_mount_id`],
    /// the DEPLOY top layer's `COPY . /` source) takes precedence; otherwise, when
    /// `add_python` is set on a NON-layered image (the RUN path / the DEPLOY base
    /// layer), the python-standalone mount supplies `/python`.
    ///
    /// [`context_mount_id`]: ImageSpec::context_mount_id
    fn resolved_context_mount_id(&self) -> Option<String> {
        if let Some(id) = &self.context_mount_id {
            return Some(id.clone());
        }
        if self.add_python.is_some() {
            return self.python_standalone_mount_id.clone();
        }
        None
    }

    /// The `Image` proto message for this spec.
    ///
    /// `base_images` (field 5) is populated ONLY for a layered build
    /// ([`ImageSpec::with_base_image`]); a registry base leaves it empty and renders
    /// `FROM <tag>` instead. `context_mount_id` (field 15) carries this layer's build
    /// context — the source mount (DEPLOY top layer / RUN fallback's source), or the
    /// python-standalone mount when `add_python` is set with no source context (RUN
    /// path / DEPLOY base layer). `context_files` (field 7) carries any inline files;
    /// it defaults empty so RUN images stay byte-identical when unused.
    fn to_proto(&self) -> Image {
        let base_images = match &self.base_image_id {
            Some(id) => vec![BaseImage {
                docker_tag: "base".to_string(),
                image_id: id.clone(),
            }],
            None => Vec::new(),
        };
        Image {
            base_images,
            dockerfile_commands: self.dockerfile_commands(),
            context_mount_id: self.resolved_context_mount_id().unwrap_or_default(),
            context_files: self
                .context_files
                .iter()
                .map(|(filename, data)| ImageContextFile {
                    filename: filename.clone(),
                    data: data.clone(),
                })
                .collect(),
            ..Default::default()
        }
    }
}

/// True when a python-standalone `series` (e.g. `"3.12"`, `"3.14t"`) has minor < 13.
/// Mirrors the client's check (_image.py:2052-2059): the `python` binary symlink is
/// only needed for older standalone dists. A malformed series defaults to `false`
/// (no symlink) — the supported-series guard lives in
/// [`crate::ops::mount::python_standalone_mount_name`].
fn python_series_lt_13(series: &str) -> bool {
    series
        .split('.')
        .nth(1)
        .map(|minor| minor.trim_end_matches('t'))
        .and_then(|minor| minor.parse::<u32>().ok())
        .map(|minor| minor < 13)
        .unwrap_or(false)
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
    fn add_python_renders_client_branch_and_no_hacks() {
        // PRIMARY path: rust base + add_python("3.12"). Emits the client's
        // add_python branch (COPY/ln/ENV) and NONE of the three hacks.
        let cmds = ImageSpec::from_registry("rust:1-slim")
            .with_add_python("3.12")
            .with_python_standalone_mount_id("mo-py-standalone")
            .with_wrapper_module(
                "modal_rust_run_wrapper",
                "def handler(e, i):\n    return i\n",
            )
            .with_command("ENTRYPOINT []")
            .dockerfile_commands();

        assert_eq!(cmds[0], "FROM rust:1-slim");
        let copy = cmds
            .iter()
            .position(|c| c == "COPY /python/. /usr/local")
            .expect("add_python COPY present");
        let ln = cmds
            .iter()
            .position(|c| c == "RUN ln -s /usr/local/bin/python3 /usr/local/bin/python")
            .expect("ln -s present for series < 3.13");
        let env = cmds
            .iter()
            .position(|c| c.starts_with("ENV TERMINFO_DIRS="))
            .expect("TERMINFO_DIRS ENV present");
        // Order matches the client's insert(1, ln): COPY, ln, ENV.
        assert!(copy < ln && ln < env, "COPY < ln -s < ENV");

        // The three hacks are GONE from the default add_python path.
        assert!(!cmds.iter().any(|c| c.contains("apt-get")), "no apt layer");
        assert!(
            !cmds.iter().any(|c| c.contains("pip install")),
            "no pip layer"
        );
        assert!(
            !cmds.iter().any(|c| c.contains("python-is-python3")),
            "no python-is-python3 apt package"
        );
        assert!(
            !cmds.iter().any(|c| c.contains("--break-system-packages")),
            "no --break-system-packages"
        );
    }

    #[test]
    fn add_python_3_13_omits_python_symlink() {
        // Standalone series >= 3.13 ship the `python` binary already; no symlink.
        let cmds = ImageSpec::from_registry("rust:1-slim")
            .with_add_python("3.13")
            .with_python_standalone_mount_id("mo-py")
            .dockerfile_commands();
        assert!(cmds.iter().any(|c| c == "COPY /python/. /usr/local"));
        assert!(
            !cmds.iter().any(|c| c.contains("ln -s")),
            "no symlink for 3.13"
        );
    }

    #[test]
    fn add_python_sets_standalone_mount_as_context_when_no_source() {
        // RUN path / DEPLOY base layer: the standalone mount becomes the build
        // context (proto field 15) so `COPY /python/. /usr/local` has a source.
        let img = ImageSpec::from_registry("rust:1-slim")
            .with_add_python("3.12")
            .with_python_standalone_mount_id("mo-py-standalone")
            .to_proto();
        assert_eq!(img.context_mount_id, "mo-py-standalone");
        assert!(
            img.base_images.is_empty(),
            "registry base has no base_images"
        );
    }

    #[test]
    fn explicit_source_context_wins_over_standalone_mount() {
        // DEPLOY top layer: the source mount owns context_mount_id; the standalone
        // mount belongs on the base layer instead.
        let img = ImageSpec::from_registry("rust:1-slim")
            .with_add_python("3.12")
            .with_python_standalone_mount_id("mo-py-standalone")
            .with_context_mount("mo-deploy-src")
            .to_proto();
        assert_eq!(img.context_mount_id, "mo-deploy-src");
    }

    #[test]
    fn base_image_renders_from_base_and_populates_base_images() {
        // Layered build: FROM base + base_images[0] = {docker_tag:"base", image_id}.
        let spec = ImageSpec::from_registry("rust:1-slim")
            .with_base_image("im-layer1")
            .with_context_mount("mo-deploy-src")
            .with_wrapper_module("m", "x = 1\n");
        let cmds = spec.dockerfile_commands();
        assert_eq!(
            cmds[0], "FROM base",
            "layered build references the prior layer"
        );

        let img = spec.to_proto();
        assert_eq!(img.base_images.len(), 1);
        assert_eq!(img.base_images[0].docker_tag, "base");
        assert_eq!(img.base_images[0].image_id, "im-layer1");
        assert_eq!(img.context_mount_id, "mo-deploy-src");
    }

    #[test]
    fn rust_toolchain_renders_after_add_python_and_before_bakes() {
        // The CUDA Tier-1 recipe: a `nvidia/cuda:<ver>-devel` base + add_python
        // (python/modal) + with_rust_toolchain (apt+rustup + CUDA env), then the
        // wrapper bake. The rustup RUN + the PATH/CUDA_PATH ENVs render AFTER the
        // add_python COPY and BEFORE the wrapper bake.
        let cmds = ImageSpec::from_registry("nvidia/cuda:12.6.3-devel-ubuntu22.04")
            .with_add_python("3.12")
            .with_python_standalone_mount_id("mo-py-standalone")
            .with_rust_toolchain()
            .with_wrapper_module(
                "modal_rust_run_wrapper",
                "def handler(e, i):\n    return i\n",
            )
            .with_command("ENTRYPOINT []")
            .dockerfile_commands();

        assert_eq!(cmds[0], "FROM nvidia/cuda:12.6.3-devel-ubuntu22.04");

        // (a) the apt + rustup combined RUN is present.
        let rustup = cmds
            .iter()
            .position(|c| c.contains("sh.rustup.rs") && c.contains("apt-get install"))
            .expect("apt+rustup RUN present");
        // The apt prereqs the rustup install needs are all named.
        let rustup_line = &cmds[rustup];
        for pkg in ["curl", "ca-certificates", "build-essential", "pkg-config"] {
            assert!(rustup_line.contains(pkg), "rustup apt prereq {pkg} present");
        }
        assert!(
            rustup_line.contains("--default-toolchain stable --profile minimal"),
            "rustup uses stable + minimal profile (gpu_app.py)"
        );

        // (b) `/root/.cargo/bin` is on the baked PATH.
        let path_env = cmds
            .iter()
            .position(|c| c.starts_with("ENV PATH=") && c.contains("/root/.cargo/bin"))
            .expect("PATH ENV with /root/.cargo/bin present");
        assert!(
            cmds[path_env].contains("/usr/local/cuda/bin"),
            "CUDA bin dir also on PATH"
        );

        // (c) CUDA_PATH=/usr/local/cuda ENV is present (CubeCL NVRTC include path).
        let cuda_path = cmds
            .iter()
            .position(|c| c == "ENV CUDA_PATH=/usr/local/cuda")
            .expect("CUDA_PATH ENV present");

        // Ordering: add_python COPY < rustup RUN < PATH ENV < CUDA_PATH ENV < bake.
        let copy = cmds
            .iter()
            .position(|c| c == "COPY /python/. /usr/local")
            .expect("add_python COPY present");
        let bake = cmds
            .iter()
            .position(|c| c.contains("/root/modal_rust_run_wrapper.py"))
            .expect("wrapper bake present");
        assert!(copy < rustup, "add_python COPY precedes rustup");
        assert!(rustup < path_env, "rustup precedes the PATH ENV");
        assert!(path_env < cuda_path, "PATH ENV precedes CUDA_PATH ENV");
        assert!(cuda_path < bake, "CUDA env precedes the wrapper bake");
    }

    #[test]
    fn default_path_renders_none_of_the_rust_toolchain_steps() {
        // (d) WITHOUT with_rust_toolchain the default rust:1-slim + add_python path is
        // byte-identical to before this addition: NO rustup, NO /root/.cargo/bin PATH,
        // NO CUDA_PATH.
        let cmds = ImageSpec::from_registry("rust:1-slim")
            .with_add_python("3.12")
            .with_python_standalone_mount_id("mo-py-standalone")
            .with_wrapper_module(
                "modal_rust_run_wrapper",
                "def handler(e, i):\n    return i\n",
            )
            .with_command("ENV RUST_BACKTRACE=1")
            .with_command("ENTRYPOINT []")
            .dockerfile_commands();

        assert!(
            !cmds.iter().any(|c| c.contains("sh.rustup.rs")),
            "no rustup install on the default path"
        );
        assert!(
            !cmds.iter().any(|c| c.contains("/root/.cargo/bin")),
            "no /root/.cargo/bin PATH on the default path"
        );
        assert!(
            !cmds.iter().any(|c| c.contains("CUDA_PATH")),
            "no CUDA_PATH on the default path"
        );
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
    fn builder_steps_render_after_provisioning_in_chain_order_before_bake() {
        // PARITY.md §3: pip_install / apt_install / run_commands as general chainable
        // image-builder steps. They render AFTER the add_python provisioning and BEFORE
        // the wrapper bake, preserving the user's chain order across the three kinds.
        let cmds = ImageSpec::from_registry("rust:1-slim")
            .with_add_python("3.12")
            .with_python_standalone_mount_id("mo-py-standalone")
            .with_apt_install(&["libpng-dev", "libjpeg-dev"])
            .with_pip_install(&["numpy", "pillow"])
            .with_run_commands(&["echo built > /opt/marker"])
            .with_wrapper_module(
                "modal_rust_run_wrapper",
                "def handler(e, i):\n    return i\n",
            )
            .with_command("ENTRYPOINT []")
            .dockerfile_commands();

        // The exact rendered lines.
        let apt = "RUN apt-get update && apt-get install -y --no-install-recommends \
                   libpng-dev libjpeg-dev && rm -rf /var/lib/apt/lists/*";
        let pip = "RUN python3 -m pip install --no-cache-dir numpy pillow";
        let run = "RUN echo built > /opt/marker";

        let pos = |needle: &str| {
            cmds.iter()
                .position(|c| c == needle)
                .unwrap_or_else(|| panic!("missing dockerfile command {needle:?} in {cmds:?}"))
        };
        let copy = cmds
            .iter()
            .position(|c| c == "COPY /python/. /usr/local")
            .expect("add_python COPY present");
        let apt_i = pos(apt);
        let pip_i = pos(pip);
        let run_i = pos(run);
        let bake = cmds
            .iter()
            .position(|c| c.contains("/root/modal_rust_run_wrapper.py"))
            .expect("wrapper bake present");

        // Chain order is preserved: apt < pip < run_commands.
        assert!(
            apt_i < pip_i,
            "apt_install precedes pip_install (chain order)"
        );
        assert!(
            pip_i < run_i,
            "pip_install precedes run_commands (chain order)"
        );
        // Provisioning precedes the steps; the steps precede the wrapper bake.
        assert!(
            copy < apt_i,
            "add_python provisioning precedes the builder steps"
        );
        assert!(run_i < bake, "the builder steps precede the wrapper bake");
    }

    #[test]
    fn empty_builder_steps_render_byte_identical_default_path() {
        // No image-builder steps chained ⇒ NO extra RUN lines: the default add_python
        // path is byte-identical to before this addition (purely additive feature).
        let with = ImageSpec::from_registry("rust:1-slim")
            .with_add_python("3.12")
            .with_python_standalone_mount_id("mo-py")
            .with_apt_install(&[]) // empty ⇒ no-op
            .with_pip_install(&[]) // empty ⇒ no-op
            .with_run_commands(&[]) // empty ⇒ no-op
            .with_wrapper_module("m", "x = 1\n")
            .dockerfile_commands();
        let without = ImageSpec::from_registry("rust:1-slim")
            .with_add_python("3.12")
            .with_python_standalone_mount_id("mo-py")
            .with_wrapper_module("m", "x = 1\n")
            .dockerfile_commands();
        assert_eq!(with, without, "empty steps render no commands");
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
    fn context_mount_emits_in_proto_only_when_set() {
        // RUN-shape spec: no context mount → empty proto string (proto default),
        // empty context_files → empty repeated (RUN images stay byte-identical).
        let run = ImageSpec::from_registry("rust:1-slim")
            .with_wrapper_module("m", "x = 1\n")
            .to_proto();
        assert_eq!(run.context_mount_id, "");
        assert!(run.context_files.is_empty());

        // DEPLOY-shape spec: context mount id surfaces on proto field 15.
        let deploy = ImageSpec::from_registry("rust:1-slim")
            .with_context_mount("mo-deploy-src")
            .to_proto();
        assert_eq!(deploy.context_mount_id, "mo-deploy-src");
    }

    #[test]
    fn deploy_dockerfile_orders_copy_then_cargo_build_then_bake_after_apt_pip() {
        // The deploy recipe: apt + pip provision python BEFORE the COPY/cargo/cp
        // RUN steps (which ride extra_commands and render LAST), so the context is
        // available and the toolchain exists when cargo compiles AT build time.
        let cmds = ImageSpec::from_registry("rust:1-slim")
            .with_apt(&["python3", "python3-pip", "python-is-python3"])
            .with_pip_install_modal()
            .with_wrapper_module(
                "modal_rust_deploy_wrapper",
                "def handler(e, i):\n    return i\n",
            )
            .with_context_mount("mo-deploy-src")
            .with_command("COPY . /")
            .with_command(
                "RUN cd /app/src && cargo build --release -p example-add --bin modal_runner",
            )
            .with_command(
                "RUN cp /app/src/target/release/modal_runner /app/modal_runner \
                 && chmod +x /app/modal_runner",
            )
            .with_command("ENTRYPOINT []")
            .dockerfile_commands();

        let pos = |needle: &str| {
            cmds.iter()
                .position(|c| c.contains(needle))
                .unwrap_or_else(|| panic!("missing dockerfile command containing {needle:?}"))
        };
        let apt = pos("apt-get update");
        let pip = pos("python3 -m pip install");
        let copy = pos("COPY . /");
        let cargo = pos("cargo build --release -p example-add --bin modal_runner");
        let cp_bake = pos("cp /app/src/target/release/modal_runner /app/modal_runner");

        // apt < pip < COPY < cargo build < cp-bake (load-bearing order).
        assert!(apt < pip, "apt must precede pip");
        assert!(pip < copy, "pip must precede COPY");
        assert!(copy < cargo, "COPY must precede the cargo build");
        assert!(cargo < cp_bake, "cargo build must precede the cp/bake");
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

    #[test]
    fn build_image_get_or_create_request_carries_wrapper_image_and_version() {
        // The wrapper: image sub-message present, app_id + builder_version carried.
        // Pairs with the existing to_proto / dockerfile_commands sub-message tests.
        let spec = ImageSpec::from_registry("rust:1-slim")
            .with_add_python("3.12")
            .with_python_standalone_mount_id("mo-py")
            .with_wrapper_module(
                "modal_rust_run_wrapper",
                "def handler(e, i):\n    return i\n",
            );
        let req = build_image_get_or_create_request(&spec, "ap-1", "2025.06".to_string());
        assert_eq!(req.app_id, "ap-1");
        assert_eq!(req.builder_version, "2025.06");
        let image = req.image.expect("image sub-message present");
        // The first dockerfile command is the FROM line (proof the spec rode in).
        assert_eq!(image.dockerfile_commands[0], "FROM rust:1-slim");
        // The add_python COPY is present; no cargo build (RUN builds in-body).
        assert!(image
            .dockerfile_commands
            .iter()
            .any(|c| c == "COPY /python/. /usr/local"));
        assert!(
            !image
                .dockerfile_commands
                .iter()
                .any(|c| c.contains("cargo build")),
            "RUN image builds in-body, not at image-build time"
        );
    }
}
