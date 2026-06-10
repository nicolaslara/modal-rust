//! The RENDER side: [`ImageSpec`] (the declarative recipe + `with_*` builders)
//! and its pure Dockerfile/proto rendering (`dockerfile_commands` / `to_proto` /
//! the bake + add_python helpers). No I/O. Split out of `image.rs` mechanically
//! (M1); all public paths re-exported from the parent module.

use base64::Engine;

use crate::ops::DEFAULT_BASE_IMAGE;
use crate::proto::api::{BaseImage, Image, ImageContextFile};

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
    pub(super) fn dockerfile_commands(&self) -> Vec<String> {
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
    pub(super) fn resolved_context_mount_id(&self) -> Option<String> {
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
    pub(super) fn to_proto(&self) -> Image {
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
pub(super) fn python_series_lt_13(series: &str) -> bool {
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
pub(super) fn bake_command(module_name: &str, source: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(source.as_bytes());
    format!(
        "RUN python3 -c \"import base64,pathlib; \
         pathlib.Path('/root/{module_name}.py').write_bytes(base64.b64decode('{b64}'))\""
    )
}
