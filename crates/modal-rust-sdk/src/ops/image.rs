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
//!
//! Mechanically split (M1): [`render`] holds [`ImageSpec`] + the pure Dockerfile/
//! proto rendering, [`build`] the request builder + the build/long-poll RPCs. ALL
//! public paths are preserved via the re-exports below; the tests stay here,
//! importing from the new paths.

mod build;
mod render;

pub(crate) use build::build_image_get_or_create_request;
pub use render::ImageSpec;

#[cfg(test)]
mod tests {
    use super::build::*;
    use super::render::*;
    use crate::ops::DEFAULT_BASE_IMAGE;
    use base64::Engine;

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
