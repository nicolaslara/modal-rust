//! Offline DRY-RUN / DUMP — the additive, network-free P8 dump tool.
//!
//! [`App::dry_run`] and [`App::dump_deploy_manifest`] assemble the FULL set of
//! control-plane requests a RUN (resp. DEPLOY) WOULD send and return them as
//! structured [`Manifest`] data plus a readable text render — with **ZERO** network.
//!
//! ## Built ON the same pure builders (no drift)
//!
//! The dump does NOT re-implement request shapes. It mirrors the ORDERING of
//! [`crate::remote::ensure_function`] / [`crate::deploy::deploy_function`] and, for
//! the load-bearing requests (the image layers + the `FunctionCreate`), feeds the
//! IDENTICAL [`modal_rust_sdk::FunctionSpec`] / [`modal_rust_sdk::ImageSpec`] the live
//! path builds into the SAME `modal_rust_sdk::planning::build_*_request` functions the
//! live ops call. So the projected manifest reflects exactly what the wire would
//! carry. Canned ids (`mo-1`, `im-1`, …) are threaded the way the mock backend
//! assigns them, so the cross-check test against the mock's recorded-request ORDER
//! holds (see `tests/mock_remote.rs`).
//!
//! This is purely ADDITIVE: it does NOT change `remote`/`deploy`/`call` semantics or
//! signatures. The live path is untouched.

use modal_rust_sdk::planning::{build_function_create_request, build_image_get_or_create_request};
use modal_rust_sdk::{FunctionSpec, ImageSpec};

use crate::deploy::{
    DeployConfig, DEPLOY_SRC, DEPLOY_WRAPPER_CALLABLE, DEPLOY_WRAPPER_MODULE, DEPLOY_WRAPPER_SRC,
};
use crate::remote::{
    run_wrapper_src, RemoteConfig, CACHE_MOUNT, CACHE_VOLUME_NAME, PYTHON_SERIES, WRAPPER_CALLABLE,
    WRAPPER_MODULE,
};
use crate::{Error, Result};

/// Which path a [`Manifest`] was assembled for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// The ephemeral RUN path ([`App::dry_run`]).
    Run,
    /// The persistent DEPLOY path ([`App::dump_deploy_manifest`]).
    Deploy,
}

/// The role a mount plays in the manifest (so the render names it, not a raw id).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountRole {
    /// The hosted modal-client mount (makes `modal` importable in-container).
    Client,
    /// The uploaded source mount: a runtime mount at `/src` (RUN) or the image build
    /// CONTEXT (DEPLOY).
    Source,
    /// The hosted python-build-standalone mount (`add_python`).
    PythonStandalone,
}

/// One planned outbound request, typed just enough to assert on + render.
#[derive(Debug, Clone, PartialEq)]
pub enum PlannedRequest {
    /// `AppCreate` (ephemeral RUN app).
    AppCreate {
        /// The app description (== the app name).
        description: String,
    },
    /// `AppGetOrCreate` (persistent DEPLOY app).
    AppGetOrCreate {
        /// The persistent app name re-deploys REPLACE under.
        app_name: String,
    },
    /// `VolumeGetOrCreate` (the cargo cache or a user volume).
    VolumeGetOrCreate {
        /// The volume deployment name.
        name: String,
        /// `true` ⇒ V2 (the cargo cache); `false` ⇒ V1 (a user volume).
        v2: bool,
    },
    /// `SecretGetOrCreate` (a named user secret resolved by from_name).
    SecretGetOrCreate {
        /// The secret deployment name.
        name: String,
    },
    /// `MountGetOrCreate` (one of the three mount roles).
    MountGetOrCreate {
        /// Which mount this is.
        role: MountRole,
    },
    /// `ImageGetOrCreate` (one image layer; layer 0 is the base/run image).
    ImageGetOrCreate {
        /// The rendered `dockerfile_commands` of this layer's image (the same list
        /// the wire carries) — so a test can assert e.g. the `cargo build` RUN.
        dockerfile_commands: Vec<String>,
        /// Layer ordinal (RUN: only layer 0; DEPLOY: 0 = base, 1 = top).
        layer: u8,
    },
    /// `FunctionPrecreate`.
    FunctionPrecreate {
        /// The function name (the wrapper callable, `"handler"`).
        function_name: String,
    },
    /// `FunctionCreate` (FILE mode) — the load-bearing projection.
    FunctionCreate {
        /// The wrapper module name (run vs deploy wrapper).
        module: String,
        /// The wrapper callable.
        function: String,
        /// Number of attached mount ids (RUN: 2 = client+source; DEPLOY: 1 = client).
        mount_ids_count: usize,
        /// The GPU type, if any (`None` = CPU).
        gpu: Option<String>,
        /// The function timeout (seconds).
        timeout_secs: u32,
        /// Volume mounts as `(mount_path, volume_id)` pairs.
        volume_mounts: Vec<(String, String)>,
        /// Number of attached secret ids.
        secret_count: usize,
        /// The FILE-mode XOR invariant: `function_data` is unset.
        function_data_is_none: bool,
    },
    /// `AppPublish` (ephemeral on RUN, deployed on DEPLOY).
    AppPublish {
        /// `"ephemeral"` or `"deployed"`.
        app_state: &'static str,
    },
}

/// The assembled control-plane manifest a run/deploy WOULD send, with NO network.
///
/// Returned by [`App::dry_run`] / [`App::dump_deploy_manifest`]. Inspect
/// [`requests`](Manifest::requests) (in send order) or [`render`](Manifest::render)
/// for a readable text form.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// RUN vs DEPLOY.
    pub mode: RunMode,
    /// The app name (the ephemeral RUN name or the persistent DEPLOY name).
    pub app_name: String,
    /// The planned requests, in send order.
    pub requests: Vec<PlannedRequest>,
}

impl Manifest {
    /// A readable, deterministic text render — one line per planned request, in send
    /// order. The image line lists its `dockerfile_commands` so the build boundary
    /// (e.g. the DEPLOY top layer's `cargo build`) is visible at a glance.
    pub fn render(&self) -> String {
        let mode = match self.mode {
            RunMode::Run => "RUN",
            RunMode::Deploy => "DEPLOY",
        };
        let mut out = format!("{mode} manifest for app {:?}\n", self.app_name);
        for (i, req) in self.requests.iter().enumerate() {
            let n = i + 1;
            let line = match req {
                PlannedRequest::AppCreate { description } => {
                    format!("AppCreate              description={description:?} (ephemeral)")
                }
                PlannedRequest::AppGetOrCreate { app_name } => {
                    format!("AppGetOrCreate         app_name={app_name:?} (persistent)")
                }
                PlannedRequest::VolumeGetOrCreate { name, v2 } => {
                    format!("VolumeGetOrCreate      name={name:?} v2={v2}")
                }
                PlannedRequest::SecretGetOrCreate { name } => {
                    format!("SecretGetOrCreate      name={name:?}")
                }
                PlannedRequest::MountGetOrCreate { role } => {
                    format!("MountGetOrCreate       role={role:?}")
                }
                PlannedRequest::ImageGetOrCreate {
                    dockerfile_commands,
                    layer,
                } => {
                    // Abbreviate the long base64-baked wrapper RUN so the render stays
                    // readable (the FULL command list lives on `dockerfile_commands`).
                    let cmds: Vec<String> = dockerfile_commands
                        .iter()
                        .map(|c| {
                            if c.contains("b64decode(") {
                                "RUN <baked wrapper>".to_string()
                            } else {
                                c.clone()
                            }
                        })
                        .collect();
                    format!(
                        "ImageGetOrCreate       layer={layer}  [{}]",
                        cmds.join("; ")
                    )
                }
                PlannedRequest::FunctionPrecreate { function_name } => {
                    format!("FunctionPrecreate      function={function_name:?}")
                }
                PlannedRequest::FunctionCreate {
                    module,
                    function,
                    mount_ids_count,
                    gpu,
                    timeout_secs,
                    volume_mounts,
                    secret_count,
                    function_data_is_none,
                } => format!(
                    "FunctionCreate         module={module:?} function={function:?} \
                     mount_ids={mount_ids_count} gpu={gpu:?} timeout={timeout_secs}s \
                     volumes={volume_mounts:?} secrets={secret_count} \
                     function_data_is_none={function_data_is_none}"
                ),
                PlannedRequest::AppPublish { app_state } => {
                    format!("AppPublish             state={app_state}")
                }
            };
            out.push_str(&format!("  {n}. {line}\n"));
        }
        out
    }
}

/// A tiny "planning sink" that hands out the SAME canned ids the mock backend
/// assigns, so the threaded ids the live path would carry are reproduced offline.
/// Mount/volume ids increment (`mo-1`, `mo-2`, …); the rest are fixed (`ap-1`,
/// `im-1`, `fu-pre-1`, …) — mirroring `crates/modal-rust-testkit/src/servicer.rs`.
struct PlanningSink {
    counter: u64,
    requests: Vec<PlannedRequest>,
}

impl PlanningSink {
    fn new() -> Self {
        PlanningSink {
            counter: 0,
            requests: Vec::new(),
        }
    }

    fn next_id(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }

    fn push(&mut self, req: PlannedRequest) {
        self.requests.push(req);
    }

    /// Record a `MountGetOrCreate` (role) and return the canned `mo-{n}` id the live
    /// path would thread onward.
    fn mount(&mut self, role: MountRole) -> String {
        self.push(PlannedRequest::MountGetOrCreate { role });
        format!("mo-{}", self.next_id())
    }

    /// Record a `VolumeGetOrCreate` and return the canned `vo-{n}` id.
    fn volume(&mut self, name: &str, v2: bool) -> String {
        self.push(PlannedRequest::VolumeGetOrCreate {
            name: name.to_string(),
            v2,
        });
        format!("vo-{}", self.next_id())
    }

    /// Record a `SecretGetOrCreate` and return the canned `sc-1` id.
    fn secret(&mut self, name: &str) -> String {
        self.push(PlannedRequest::SecretGetOrCreate {
            name: name.to_string(),
        });
        "sc-1".to_string()
    }

    /// Record an `ImageGetOrCreate` (built via the SAME SDK builder the live path
    /// calls) and return the canned `im-{n}` id.
    fn image(&mut self, spec: &ImageSpec, layer: u8) -> String {
        // Built ON the pure builder, so the rendered dockerfile_commands are exactly
        // what the wire would carry (no drift). app_id/builder_version are immaterial
        // to the projection but supplied for byte-fidelity.
        let req = build_image_get_or_create_request(spec, "ap-1", "2025.06".to_string());
        let dockerfile_commands = req.image.map(|i| i.dockerfile_commands).unwrap_or_default();
        self.push(PlannedRequest::ImageGetOrCreate {
            dockerfile_commands,
            layer,
        });
        format!("im-{}", self.next_id())
    }
}

/// Project a `FunctionSpec` (the SAME spec the live path builds) through the SDK's
/// `build_function_create_request` and record the `FunctionCreate` projection. The
/// projected fields come straight off the built request, so the dump can never drift
/// from the wire.
fn record_function_create(sink: &mut PlanningSink, spec: &FunctionSpec, precreate_id: &str) {
    let req = build_function_create_request("ap-1", precreate_id, spec);
    let function = req.function.expect("FILE-mode sets `function`");
    let gpu = function
        .resources
        .as_ref()
        .and_then(|r| r.gpu_config.as_ref())
        .map(|g| g.gpu_type.clone());
    let volume_mounts = function
        .volume_mounts
        .iter()
        .map(|m| (m.mount_path.clone(), m.volume_id.clone()))
        .collect();
    sink.push(PlannedRequest::FunctionCreate {
        module: function.module_name.clone(),
        function: function.function_name.clone(),
        mount_ids_count: function.mount_ids.len(),
        gpu,
        timeout_secs: function.timeout_secs,
        volume_mounts,
        secret_count: function.secret_ids.len(),
        function_data_is_none: req.function_data.is_none(),
    });
}

impl crate::App {
    /// Render the RUN manifest for `entrypoint` WITHOUT any network (the additive P8
    /// dump). Mirrors [`crate::remote::ensure_function`]'s ordering and feeds the
    /// SAME pure builders, so the returned [`Manifest`] is exactly what `.remote()`
    /// WOULD send (cargo cache volume, secrets, user volumes, client+source+python
    /// mounts, image, precreate, FILE-mode `FunctionCreate`, ephemeral `AppPublish`).
    ///
    /// Sync + offline: it never connects and never sends. It resolves the decorator
    /// config via [`config_for`](crate::App::config_for) exactly as `.remote()` does,
    /// so the dumped gpu/timeout/secrets/volumes match the wire. Additive — does NOT
    /// change [`remote`](crate::Function::remote).
    ///
    /// `config` is the base [`RemoteConfig`] (e.g. [`RemoteConfig::default`]); the
    /// per-entrypoint decorator gpu/timeout/cache/secrets/volumes are folded in the
    /// SAME way `resolve_function` does.
    pub fn dry_run(&self, entrypoint: &str, config: &RemoteConfig) -> Result<Manifest> {
        // Fold the decorator config exactly as `App::resolve_function` does, so the
        // dumped manifest matches what `.remote()` would send for this entrypoint.
        let dcfg = self.config_for(entrypoint);
        let cfg = {
            let mut c = config.clone();
            c.gpu = dcfg.gpu.map(|s| s.to_string());
            c.timeout_override_secs = dcfg.timeout_secs;
            c.cache = dcfg.cache.unwrap_or(c.cache);
            c.secrets = dcfg.secrets.iter().map(|s| s.to_string()).collect();
            c.volumes = dcfg
                .volumes
                .iter()
                .map(|(m, n)| (m.to_string(), n.to_string()))
                .collect();
            c
        };
        // The dump uses the connected ephemeral app's name if present, else falls
        // back to the config package (a bare, unconnected App has no app name).
        let app_name = self.dump_app_name(&cfg.package);

        let mut sink = PlanningSink::new();

        // 0. AppCreate (the ephemeral RUN app). The live ephemeral app is created at
        //    connect time (`App::connect`), but the dump renders the full set the RUN
        //    flow implies, so it leads with the ephemeral AppCreate.
        sink.push(PlannedRequest::AppCreate {
            description: app_name.clone(),
        });

        // 1. Cargo-cache volume (P6), only when caching is on (V2 + create).
        let cache_vol_id = if cfg.cache {
            Some(sink.volume(CACHE_VOLUME_NAME, true))
        } else {
            None
        };

        // 1b. User secrets (from_name lookup), in order.
        let secret_ids: Vec<String> = cfg.secrets.iter().map(|n| sink.secret(n)).collect();

        // 1c. User volumes (V1, create), in order. A `/cache` collision is rejected by
        //     the live path; the dump surfaces the same error.
        let mut user_volume_mounts: Vec<(String, String)> = Vec::with_capacity(cfg.volumes.len());
        for (mount_path, name) in &cfg.volumes {
            if cfg.cache && mount_path == CACHE_MOUNT {
                return Err(Error::config(format!(
                    "user volume mount path {CACHE_MOUNT:?} collides with the cargo-cache \
                     volume; choose a different mount path (or disable the cache)"
                )));
            }
            let vid = sink.volume(name, false);
            user_volume_mounts.push((vid, mount_path.clone()));
        }

        // 2. Client mount.
        let client_mount_id = sink.mount(MountRole::Client);
        // 3. Source mount (uploaded crate, mounted at /src).
        let source_mount_id = sink.mount(MountRole::Source);
        // 3b. Python-standalone mount.
        let py_mount_id = sink.mount(MountRole::PythonStandalone);

        // 4. Run image (layer 0): rust base + add_python + the baked wrapper. Built
        //    the SAME way `ensure_function` builds it.
        let mut spec = ImageSpec::from_registry(cfg.base_image.clone())
            .with_add_python(PYTHON_SERIES)
            .with_python_standalone_mount_id(py_mount_id);
        if cfg.install_rust {
            spec = spec.with_rust_toolchain();
        }
        let spec = spec
            .with_wrapper_module(WRAPPER_MODULE, run_wrapper_src(&cfg.package, cfg.cache))
            .with_command("ENV RUST_BACKTRACE=1")
            .with_command("ENTRYPOINT []");
        let image_id = sink.image(&spec, 0);

        // 5. Precreate under the PER-ENTRYPOINT object tag (the sanitized entrypoint),
        //    exactly as `ensure_function` does — so the dump matches the wire.
        let object_tag = crate::remote::sanitize_object_tag(entrypoint);
        sink.push(PlannedRequest::FunctionPrecreate {
            function_name: object_tag.clone(),
        });
        let precreate_id = "fu-pre-1";

        // 6. FunctionCreate (FILE mode) — build the SAME FunctionSpec the live path
        //    builds (object tag = entrypoint; in-container callable = WRAPPER_CALLABLE),
        //    then project it through the SDK builder.
        let timeout = cfg.timeout_override_secs.unwrap_or(cfg.timeout_secs);
        let mut fn_spec = FunctionSpec::new(WRAPPER_MODULE, WRAPPER_CALLABLE, &image_id)
            .with_app_function_name(&object_tag)
            .with_mount_ids(vec![client_mount_id, source_mount_id])
            .with_mount_client_dependencies(true)
            .with_timeout_secs(timeout)
            .with_gpu(cfg.gpu.clone())?;
        if let Some(vid) = cache_vol_id {
            fn_spec = fn_spec.with_volume_mount(vid, CACHE_MOUNT);
        }
        for (vid, mount_path) in user_volume_mounts {
            fn_spec = fn_spec.with_volume_mount(vid, mount_path);
        }
        if !secret_ids.is_empty() {
            fn_spec = fn_spec.with_secret_ids(secret_ids);
        }
        record_function_create(&mut sink, &fn_spec, precreate_id);

        // 7. AppPublish (EPHEMERAL on the RUN path).
        sink.push(PlannedRequest::AppPublish {
            app_state: "ephemeral",
        });

        Ok(Manifest {
            mode: RunMode::Run,
            app_name,
            requests: sink.requests,
        })
    }

    /// Render the DEPLOY manifest WITHOUT any network (the additive P8 dump). Mirrors
    /// [`crate::deploy::deploy_function`]'s ordering and feeds the SAME pure builders,
    /// so the returned [`Manifest`] is exactly what `deploy` WOULD send: client +
    /// source(build context) + python mounts, the persistent `AppGetOrCreate`, TWO
    /// image layers (the top layer carries `cargo build --release`), precreate, the
    /// CLIENT-mount-only `FunctionCreate` (the deploy build boundary), and the
    /// persistent `AppPublish`.
    ///
    /// Sync + offline. Additive — does NOT change [`deploy`](crate::App::deploy).
    pub fn dump_deploy_manifest(&self, config: &DeployConfig) -> Result<Manifest> {
        let config = config.clone();
        // Per-entrypoint deploy plan, exactly as `App::deploy_with` builds it: one
        // function per entrypoint (object tag = entrypoint) with its OWN config; the
        // manual/no-decorator path falls back to a single default function.
        let plan = self.deploy_entrypoints_for_dump(&config);

        let mut sink = PlanningSink::new();

        // 1. Client mount.
        let client_mount_id = sink.mount(MountRole::Client);
        // 2. Source mount (the image BUILD CONTEXT — lands at /app/src).
        let source_mount_id = sink.mount(MountRole::Source);
        // 2b. Python-standalone mount.
        let py_mount_id = sink.mount(MountRole::PythonStandalone);

        // 3. Persistent named app.
        sink.push(PlannedRequest::AppGetOrCreate {
            app_name: config.app_name.clone(),
        });

        // 4. Two image layers — base (add_python) then top (source COPY + cargo
        //    build). Built the SAME way `deploy_function` builds them.
        let mut base_spec = ImageSpec::from_registry(config.base_image.clone())
            .with_add_python(PYTHON_SERIES)
            .with_python_standalone_mount_id(py_mount_id);
        if config.install_rust {
            base_spec = base_spec.with_rust_toolchain();
        }
        let base_image_id = sink.image(&base_spec, 0);

        let top_spec = ImageSpec::from_registry(String::new())
            .with_base_image(&base_image_id)
            .with_wrapper_module(DEPLOY_WRAPPER_MODULE, DEPLOY_WRAPPER_SRC)
            .with_context_mount(&source_mount_id)
            .with_command("COPY . /")
            .with_command(format!(
                "RUN cd {DEPLOY_SRC} && cargo build --release -p {} --bin modal_runner",
                config.package
            ))
            .with_command(format!(
                "RUN cp {DEPLOY_SRC}/target/release/modal_runner /app/modal_runner \
                 && chmod +x /app/modal_runner"
            ))
            .with_command("ENV RUST_BACKTRACE=1")
            .with_command("ENTRYPOINT []");
        let image_id = sink.image(&top_spec, 1);

        // 5/6. ONE precreate + FunctionCreate PER ENTRYPOINT over the shared image
        //      (object tag = the entrypoint, its OWN config). CLIENT mount ONLY (NO
        //      source mount: the binary is baked in the image layer) — the deploy
        //      invariant. Built the SAME way `deploy_function` builds each function.
        for ep in &plan {
            let object_tag = crate::remote::sanitize_object_tag(&ep.name);
            sink.push(PlannedRequest::FunctionPrecreate {
                function_name: object_tag.clone(),
            });
            let precreate_id = "fu-pre-1";
            let timeout = ep.timeout_secs.unwrap_or(config.timeout_secs);
            let mut fn_spec =
                FunctionSpec::new(DEPLOY_WRAPPER_MODULE, DEPLOY_WRAPPER_CALLABLE, &image_id)
                    .with_app_function_name(&object_tag)
                    .with_mount_ids(vec![client_mount_id.clone()])
                    .with_mount_client_dependencies(true)
                    .with_timeout_secs(timeout)
                    .with_gpu(ep.gpu.clone())?;
            let secret_ids: Vec<String> = ep.secrets.iter().map(|n| sink.secret(n)).collect();
            if !secret_ids.is_empty() {
                fn_spec = fn_spec.with_secret_ids(secret_ids);
            }
            for (mount_path, name) in &ep.volumes {
                let vid = sink.volume(name, false);
                fn_spec = fn_spec.with_volume_mount(vid, mount_path.clone());
            }
            record_function_create(&mut sink, &fn_spec, precreate_id);
        }

        // 7. Persistent AppPublish (the UNION of every per-entrypoint function).
        sink.push(PlannedRequest::AppPublish {
            app_state: "deployed",
        });

        Ok(Manifest {
            mode: RunMode::Deploy,
            app_name: config.app_name.clone(),
            requests: sink.requests,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::App;
    use crate::FunctionConfig;
    use std::collections::BTreeMap;

    /// A tiny base `RemoteConfig` for the dump (no network, no real workspace read):
    /// caching on so the cargo-cache volume rides, whole-dir upload (irrelevant to
    /// the dump — it never uploads).
    fn run_cfg() -> RemoteConfig {
        RemoteConfig {
            package: "app".to_string(),
            base_image: "rust:1-slim".to_string(),
            timeout_secs: 1800,
            use_cargo_scoping: false,
            cache: true,
            ..RemoteConfig::default()
        }
    }

    fn variants(reqs: &[PlannedRequest]) -> Vec<&'static str> {
        reqs.iter()
            .map(|r| match r {
                PlannedRequest::AppCreate { .. } => "AppCreate",
                PlannedRequest::AppGetOrCreate { .. } => "AppGetOrCreate",
                PlannedRequest::VolumeGetOrCreate { .. } => "VolumeGetOrCreate",
                PlannedRequest::SecretGetOrCreate { .. } => "SecretGetOrCreate",
                PlannedRequest::MountGetOrCreate { .. } => "MountGetOrCreate",
                PlannedRequest::ImageGetOrCreate { .. } => "ImageGetOrCreate",
                PlannedRequest::FunctionPrecreate { .. } => "FunctionPrecreate",
                PlannedRequest::FunctionCreate { .. } => "FunctionCreate",
                PlannedRequest::AppPublish { .. } => "AppPublish",
            })
            .collect()
    }

    #[test]
    fn dry_run_renders_the_full_run_sequence() {
        // A T4 + cache-on entrypoint (the decorator config the macro would set).
        let cfg = FunctionConfig {
            gpu: Some("T4"),
            timeout_secs: Some(1800),
            cache: Some(true),
            secrets: &[],
            volumes: &[],
        };
        let app = App::from_manifest([("add".to_string(), cfg)]);
        let manifest = app.dry_run("add", &run_cfg()).expect("dry_run");

        assert_eq!(manifest.mode, RunMode::Run);
        // The full ordered sequence (cache on, no secrets/user-volumes).
        assert_eq!(
            variants(&manifest.requests),
            vec![
                "AppCreate",
                "VolumeGetOrCreate", // cargo cache (V2)
                "MountGetOrCreate",  // client
                "MountGetOrCreate",  // source
                "MountGetOrCreate",  // python-standalone
                "ImageGetOrCreate",  // layer 0
                "FunctionPrecreate",
                "FunctionCreate",
                "AppPublish",
            ]
        );

        // The cargo cache volume is V2.
        let vol = manifest
            .requests
            .iter()
            .find_map(|r| match r {
                PlannedRequest::VolumeGetOrCreate { name, v2 } => Some((name.clone(), *v2)),
                _ => None,
            })
            .expect("cargo cache volume");
        assert_eq!(vol.0, "modal-rust-cargo-cache");
        assert!(vol.1, "cargo cache is V2");

        // FunctionCreate: 2 mounts (client+source), T4 gpu, timeout 1800, /cache
        // volume, no secrets, function_data unset (XOR), run wrapper module.
        let fc = manifest
            .requests
            .iter()
            .find_map(|r| match r {
                PlannedRequest::FunctionCreate {
                    module,
                    mount_ids_count,
                    gpu,
                    timeout_secs,
                    volume_mounts,
                    secret_count,
                    function_data_is_none,
                    ..
                } => Some((
                    module.clone(),
                    *mount_ids_count,
                    gpu.clone(),
                    *timeout_secs,
                    volume_mounts.clone(),
                    *secret_count,
                    *function_data_is_none,
                )),
                _ => None,
            })
            .expect("FunctionCreate");
        assert_eq!(fc.0, "modal_rust_run_wrapper");
        assert_eq!(fc.1, 2, "RUN attaches client + source mounts");
        assert_eq!(fc.2.as_deref(), Some("T4"));
        assert_eq!(fc.3, 1800);
        assert_eq!(fc.4, vec![("/cache".to_string(), "vo-1".to_string())]);
        assert_eq!(fc.5, 0, "no secrets");
        assert!(fc.6, "FILE-mode XOR: function_data is None");

        // The RUN image does NOT carry a cargo build (builds in-body).
        let img = manifest
            .requests
            .iter()
            .find_map(|r| match r {
                PlannedRequest::ImageGetOrCreate {
                    dockerfile_commands,
                    ..
                } => Some(dockerfile_commands.clone()),
                _ => None,
            })
            .expect("image");
        assert!(img.iter().any(|c| c == "FROM rust:1-slim"));
        assert!(img.iter().any(|c| c == "COPY /python/. /usr/local"));
        assert!(
            !img.iter().any(|c| c.contains("cargo build")),
            "RUN image builds in-body, not at image-build time"
        );

        // The render is non-empty and names the headline lines, with the long
        // base64-baked wrapper RUN abbreviated for readability.
        let render = manifest.render();
        assert!(render.contains("RUN manifest for app"));
        assert!(render.contains("VolumeGetOrCreate      name=\"modal-rust-cargo-cache\" v2=true"));
        assert!(
            render.contains("RUN <baked wrapper>"),
            "long bake RUN abbreviated"
        );
        assert!(
            !render.contains("b64decode("),
            "render hides the raw base64 blob"
        );
        // Object TAG = the entrypoint ("add"), so per-entrypoint configs never collide;
        // the in-container "handler" callable rides on `implementation_name` (not shown).
        assert!(render.contains(
            "FunctionCreate         module=\"modal_rust_run_wrapper\" function=\"add\" \
             mount_ids=2 gpu=Some(\"T4\") timeout=1800s"
        ));
        // Precreate is registered under the same per-entrypoint object tag.
        assert!(render.contains("FunctionPrecreate      function=\"add\""));
        assert!(render.contains("AppPublish             state=ephemeral"));
    }

    #[test]
    fn dry_run_cache_off_drops_the_volume() {
        // cache=false ⇒ no VolumeGetOrCreate, no /cache mount (byte-identical to pre-P6).
        let cfg = FunctionConfig {
            cache: Some(false),
            ..FunctionConfig::default()
        };
        let app = App::from_manifest([("add".to_string(), cfg)]);
        let manifest = app.dry_run("add", &run_cfg()).expect("dry_run");
        assert!(
            !variants(&manifest.requests).contains(&"VolumeGetOrCreate"),
            "cache off ⇒ no volume"
        );
        let fc_volumes = manifest
            .requests
            .iter()
            .find_map(|r| match r {
                PlannedRequest::FunctionCreate { volume_mounts, .. } => Some(volume_mounts.clone()),
                _ => None,
            })
            .expect("FunctionCreate");
        assert!(fc_volumes.is_empty(), "no /cache mount when cache off");
    }

    #[test]
    fn dry_run_secrets_and_user_volumes_ride_into_function_create() {
        // A decorated entrypoint with a secret + a user volume.
        let cfg = FunctionConfig {
            cache: Some(false), // keep the manifest minimal
            secrets: &["api-creds"],
            volumes: &[("/data", "my-vol")],
            ..FunctionConfig::default()
        };
        let app = App::from_manifest([("add".to_string(), cfg)]);
        let manifest = app.dry_run("add", &run_cfg()).expect("dry_run");

        // The secret + the user volume each fired before FunctionCreate.
        assert!(variants(&manifest.requests).contains(&"SecretGetOrCreate"));
        let user_vol = manifest.requests.iter().any(|r| {
            matches!(r, PlannedRequest::VolumeGetOrCreate { name, v2 } if name == "my-vol" && !*v2)
        });
        assert!(user_vol, "user volume V1 resolved");

        // The ids rode into FunctionCreate.
        let fc = manifest
            .requests
            .iter()
            .find_map(|r| match r {
                PlannedRequest::FunctionCreate {
                    volume_mounts,
                    secret_count,
                    ..
                } => Some((volume_mounts.clone(), *secret_count)),
                _ => None,
            })
            .expect("FunctionCreate");
        assert_eq!(fc.1, 1, "the secret id rode into FunctionCreate");
        assert!(
            fc.0.iter().any(|(path, _)| path == "/data"),
            "the user volume rode into FunctionCreate at /data"
        );
    }

    #[test]
    fn dump_deploy_manifest_is_client_mount_only_with_cargo_build() {
        // Deploy with a decorated entrypoint (gpu, to prove it flows through).
        let cfg = FunctionConfig {
            gpu: Some("A100"),
            timeout_secs: Some(900),
            ..FunctionConfig::default()
        };
        let app = App::from_manifest([("add".to_string(), cfg)]);
        let dcfg = DeployConfig {
            app_name: "my-deploy".to_string(),
            package: "app".to_string(),
            base_image: "rust:1-slim".to_string(),
            use_cargo_scoping: false,
            ..DeployConfig::for_app("my-deploy")
        };
        let manifest = app.dump_deploy_manifest(&dcfg).expect("dump_deploy");

        assert_eq!(manifest.mode, RunMode::Deploy);
        assert_eq!(manifest.app_name, "my-deploy");
        // The DEPLOY sequence: client+source+python mounts, persistent app, TWO image
        // layers, precreate, FunctionCreate, deployed publish.
        assert_eq!(
            variants(&manifest.requests),
            vec![
                "MountGetOrCreate", // client
                "MountGetOrCreate", // source (build context)
                "MountGetOrCreate", // python-standalone
                "AppGetOrCreate",   // persistent
                "ImageGetOrCreate", // base layer
                "ImageGetOrCreate", // top layer (cargo build)
                "FunctionPrecreate",
                "FunctionCreate",
                "AppPublish",
            ]
        );

        // The deploy build boundary: FunctionCreate attaches the CLIENT mount ONLY.
        let fc = manifest
            .requests
            .iter()
            .find_map(|r| match r {
                PlannedRequest::FunctionCreate {
                    module,
                    mount_ids_count,
                    gpu,
                    timeout_secs,
                    ..
                } => Some((module.clone(), *mount_ids_count, gpu.clone(), *timeout_secs)),
                _ => None,
            })
            .expect("FunctionCreate");
        assert_eq!(fc.0, "modal_rust_deploy_wrapper");
        assert_eq!(
            fc.1, 1,
            "DEPLOY attaches CLIENT mount ONLY (no source mount)"
        );
        assert_eq!(fc.2.as_deref(), Some("A100"));
        assert_eq!(fc.3, 900);

        // The TOP layer (layer 1) carries the cargo build --release RUN.
        let top = manifest
            .requests
            .iter()
            .find_map(|r| match r {
                PlannedRequest::ImageGetOrCreate {
                    dockerfile_commands,
                    layer,
                } if *layer == 1 => Some(dockerfile_commands.clone()),
                _ => None,
            })
            .expect("top layer image");
        assert!(
            top.iter().any(|c| c.contains("cargo build --release")),
            "the DEPLOY top layer builds at image-build time"
        );
        assert!(top.iter().any(|c| c == "COPY . /"));

        // The deployed publish.
        assert!(manifest.requests.iter().any(
            |r| matches!(r, PlannedRequest::AppPublish { app_state } if *app_state == "deployed")
        ));
        // Single entrypoint => the object tag is the entrypoint name.
        let names: Vec<String> = manifest
            .requests
            .iter()
            .filter_map(|r| match r {
                PlannedRequest::FunctionCreate { function, .. } => Some(function.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["add".to_string()], "object tag = entrypoint");
    }

    #[test]
    fn dump_deploy_manifest_renders_one_function_per_divergent_entrypoint() {
        // DEPLOY now publishes one function PER ENTRYPOINT: two divergent-config
        // entrypoints render TWO precreate+FunctionCreate pairs (distinct object tags,
        // each its own gpu/timeout) over ONE shared image — the rejection is gone.
        let app = App::from_manifest([
            ("cpu".to_string(), FunctionConfig::default()),
            (
                "gpu".to_string(),
                FunctionConfig {
                    gpu: Some("A100"),
                    timeout_secs: Some(900),
                    ..FunctionConfig::default()
                },
            ),
        ]);
        let dcfg = DeployConfig {
            app_name: "multi-deploy".to_string(),
            package: "app".to_string(),
            base_image: "rust:1-slim".to_string(),
            use_cargo_scoping: false,
            ..DeployConfig::for_app("multi-deploy")
        };
        let manifest = app.dump_deploy_manifest(&dcfg).expect("dump_deploy");

        // ONE base + ONE top image layer (shared), then a precreate+create per
        // entrypoint, then ONE deployed publish.
        assert_eq!(
            variants(&manifest.requests),
            vec![
                "MountGetOrCreate", // client
                "MountGetOrCreate", // source (build context)
                "MountGetOrCreate", // python-standalone
                "AppGetOrCreate",
                "ImageGetOrCreate",  // base layer (shared)
                "ImageGetOrCreate",  // top layer (shared, cargo build)
                "FunctionPrecreate", // cpu
                "FunctionCreate",    // cpu
                "FunctionPrecreate", // gpu
                "FunctionCreate",    // gpu
                "AppPublish",
            ]
        );

        // Two FunctionCreates, distinct object tags + their OWN configs (BTreeMap
        // orders "cpu" then "gpu").
        let creates: Vec<(String, Option<String>, u32)> = manifest
            .requests
            .iter()
            .filter_map(|r| match r {
                PlannedRequest::FunctionCreate {
                    function,
                    gpu,
                    timeout_secs,
                    ..
                } => Some((function.clone(), gpu.clone(), *timeout_secs)),
                _ => None,
            })
            .collect();
        assert_eq!(creates.len(), 2, "one FunctionCreate per entrypoint");
        assert_eq!(creates[0].0, "cpu");
        assert_eq!(creates[0].1, None, "cpu has no gpu");
        assert_eq!(creates[1].0, "gpu");
        assert_eq!(creates[1].1.as_deref(), Some("A100"));
        assert_eq!(creates[1].2, 900);
        assert_ne!(creates[0].0, creates[1].0, "distinct object tags");
    }

    #[test]
    fn dry_run_user_volume_at_cache_path_is_rejected() {
        // A user volume mounted at the reserved /cache path collides with the cargo
        // cache — the dump surfaces the SAME error the live path would.
        let cfg = FunctionConfig {
            cache: Some(true),
            volumes: &[("/cache", "rogue")],
            ..FunctionConfig::default()
        };
        let app = App::from_manifest([("add".to_string(), cfg)]);
        let err = app.dry_run("add", &run_cfg()).unwrap_err();
        assert!(format!("{err}").contains("/cache"), "collision is reported");
    }

    #[test]
    fn dry_run_works_without_a_connected_app() {
        // The dump never connects: a manual `App::from_manifest` (no remote handle)
        // still renders, using the package as the app name fallback.
        let app = App::from_manifest(BTreeMap::<String, FunctionConfig>::new());
        let manifest = app.dry_run("add", &run_cfg()).expect("offline dry_run");
        assert_eq!(manifest.app_name, "app", "falls back to the package name");
        assert!(!manifest.requests.is_empty());
    }
}
