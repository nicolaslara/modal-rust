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
//! path builds into the SAME `modal_rust_sdk::planning::plan_*_request` projections —
//! which call the live ops' internal `build_*_request` builders and return SDK-owned
//! typed structs (no raw proto leaks across the crate boundary). So the projected
//! manifest reflects exactly what the wire would carry. Canned ids (`mo-1`, `im-1`,
//! …) are threaded the way the mock backend assigns them, so the cross-check test
//! against the mock's recorded-request ORDER holds (see `tests/mock_remote.rs`).
//!
//! This is purely ADDITIVE: it does NOT change `remote`/`deploy`/`call` semantics or
//! signatures. The live path is untouched.

use modal_rust_sdk::planning::{plan_function_request, plan_image_request};
use modal_rust_sdk::{FunctionSpec, ImageSpec};

use crate::control_plane::{
    provision, AppState, Boundary, ControlPlane, Entrypoint, ProvisionInputs, Published,
    SourceInputs, DEPLOY_BOUNDARY, RUN_BOUNDARY,
};
use crate::deploy::{DeployConfig, DEPLOY_SRC};
use crate::remote::RemoteConfig;
use crate::Result;

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
        /// Requested CPU in milli-cores (`0` = server default).
        milli_cpu: u32,
        /// Requested memory in MiB (`0` = server default).
        memory_mb: u32,
        /// The function timeout (seconds).
        timeout_secs: u32,
        /// Volume mounts as `(mount_path, volume_id)` pairs.
        volume_mounts: Vec<(String, String)>,
        /// Number of attached secret ids.
        secret_count: usize,
        /// The automatic retry COUNT, if a retry policy is set; `None` = no policy.
        retries: Option<u32>,
        /// A human-readable summary of the run schedule, if set; `None` = no schedule.
        schedule: Option<String>,
        /// Autoscaler floor (`min_containers`); `None` = unset (scale to zero).
        min_containers: Option<u32>,
        /// Autoscaler ceiling (`max_containers`); `None` = unset.
        max_containers: Option<u32>,
        /// Warm buffer (`buffer_containers`); `None` = unset.
        buffer_containers: Option<u32>,
        /// Idle-before-scaledown seconds (`scaledown_window`); `None` = unset.
        scaledown_window: Option<u32>,
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
                    milli_cpu,
                    memory_mb,
                    timeout_secs,
                    volume_mounts,
                    secret_count,
                    retries,
                    schedule,
                    min_containers,
                    max_containers,
                    buffer_containers,
                    scaledown_window,
                    function_data_is_none,
                } => format!(
                    "FunctionCreate         module={module:?} function={function:?} \
                     mount_ids={mount_ids_count} gpu={gpu:?} cpu={milli_cpu}m \
                     memory={memory_mb}MiB timeout={timeout_secs}s \
                     volumes={volume_mounts:?} secrets={secret_count} \
                     retries={retries:?} schedule={schedule:?} \
                     min_containers={min_containers:?} max_containers={max_containers:?} \
                     buffer_containers={buffer_containers:?} \
                     scaledown_window={scaledown_window:?} \
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

/// The RECORDING control plane = the dry-run / dump. Each [`ControlPlane`] method
/// RECORDS the request it was handed as a [`PlannedRequest`] and returns a
/// DETERMINISTIC fake id (`ap-1`, `im-{n}`, `mo-{n}`, …) so [`provision`] keeps
/// threading. The recorded [`requests`](RecordingControlPlane::requests) ARE the
/// manifest — so `dump` is literally `provision(RecordingControlPlane, …)` and can
/// never drift from the live path.
///
/// Ids mirror `crates/modal-rust-testkit/src/servicer.rs`: mount/volume/image share
/// ONE incrementing counter (`mo-{n}` / `vo-{n}` / `im-{n}`); the rest are fixed
/// (`ap-1`, `sc-1`, `fu-pre-1`, `fu-1`). The `FunctionCreate` / `ImageGetOrCreate`
/// projections are built ON the SAME pure SDK `build_*_request` builders the live
/// path calls, so the projected fields are exactly what the wire would carry.
///
/// Dump fidelity boundary: this records the TOP-LEVEL RPCs (with fabricated ids), not
/// the live-only `ImageJoinStreaming` poll loop or the per-file mount PUT/probe
/// traffic — those are encapsulated inside `LiveControlPlane` and never reach here.
struct RecordingControlPlane {
    counter: u64,
    requests: Vec<PlannedRequest>,
}

impl RecordingControlPlane {
    fn new() -> Self {
        RecordingControlPlane {
            counter: 0,
            requests: Vec::new(),
        }
    }

    fn next_id(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }
}

impl ControlPlane for RecordingControlPlane {
    async fn ensure_app(
        &mut self,
        app_name: &str,
        _pre_resolved: Option<&str>,
        state: AppState,
    ) -> Result<String> {
        // The dump renders the FULL set the path implies, so the RUN ephemeral
        // AppCreate (issued at connect time on the live path) is recorded here too.
        match state {
            AppState::Ephemeral => self.requests.push(PlannedRequest::AppCreate {
                description: app_name.to_string(),
            }),
            AppState::Deployed => self.requests.push(PlannedRequest::AppGetOrCreate {
                app_name: app_name.to_string(),
            }),
        }
        Ok("ap-1".to_string())
    }

    async fn ensure_volume(&mut self, name: &str, v2: bool) -> Result<String> {
        self.requests.push(PlannedRequest::VolumeGetOrCreate {
            name: name.to_string(),
            v2,
        });
        Ok(format!("vo-{}", self.next_id()))
    }

    async fn ensure_secret(&mut self, name: &str) -> Result<String> {
        self.requests.push(PlannedRequest::SecretGetOrCreate {
            name: name.to_string(),
        });
        Ok("sc-1".to_string())
    }

    async fn ensure_client_mount(&mut self) -> Result<String> {
        self.requests.push(PlannedRequest::MountGetOrCreate {
            role: MountRole::Client,
        });
        Ok(format!("mo-{}", self.next_id()))
    }

    async fn ensure_source_mount(
        &mut self,
        _source: &SourceInputs<'_>,
        _remote_path: &str,
    ) -> Result<String> {
        self.requests.push(PlannedRequest::MountGetOrCreate {
            role: MountRole::Source,
        });
        Ok(format!("mo-{}", self.next_id()))
    }

    async fn ensure_python_mount(&mut self, _series: &str) -> Result<String> {
        self.requests.push(PlannedRequest::MountGetOrCreate {
            role: MountRole::PythonStandalone,
        });
        Ok(format!("mo-{}", self.next_id()))
    }

    async fn ensure_image(&mut self, app_id: &str, spec: &ImageSpec, layer: u8) -> Result<String> {
        // Project through the SAME pure SDK builder the live path calls (via the typed
        // planning API, so no raw proto crosses the boundary), so the rendered
        // dockerfile_commands are exactly what the wire would carry.
        let planned = plan_image_request(spec, app_id, "2025.06");
        self.requests.push(PlannedRequest::ImageGetOrCreate {
            dockerfile_commands: planned.dockerfile_commands,
            layer,
        });
        Ok(format!("im-{}", self.next_id()))
    }

    async fn precreate(&mut self, _app_id: &str, object_tag: &str) -> Result<String> {
        self.requests.push(PlannedRequest::FunctionPrecreate {
            function_name: object_tag.to_string(),
        });
        Ok("fu-pre-1".to_string())
    }

    async fn create(
        &mut self,
        app_id: &str,
        precreate_id: &str,
        spec: &FunctionSpec,
    ) -> Result<(String, String)> {
        // Project the SAME FunctionSpec the live path builds through the typed planning
        // API (which calls the SDK's internal builder, then returns an SDK-owned
        // struct — no raw proto crosses the boundary).
        let planned = plan_function_request(app_id, precreate_id, spec);
        self.requests.push(PlannedRequest::FunctionCreate {
            module: planned.module_name,
            function: planned.function_name,
            mount_ids_count: planned.mount_ids_count,
            gpu: planned.gpu,
            milli_cpu: planned.milli_cpu,
            memory_mb: planned.memory_mb,
            timeout_secs: planned.timeout_secs,
            volume_mounts: planned.volume_mounts,
            secret_count: planned.secret_ids_count,
            retries: planned.retries,
            schedule: planned.schedule,
            min_containers: planned.min_containers,
            max_containers: planned.max_containers,
            buffer_containers: planned.buffer_containers,
            scaledown_window: planned.scaledown_window,
            function_data_is_none: planned.function_data_is_none,
        });
        // A deterministic function id keeps the cumulative publish union non-empty;
        // it never appears in the recorded manifest (the publish carries no ids).
        Ok(("fu-1".to_string(), String::new()))
    }

    async fn publish(
        &mut self,
        _app_id: &str,
        _app_name: &str,
        _function_ids: std::collections::HashMap<String, String>,
        _definition_ids: std::collections::HashMap<String, String>,
        state: AppState,
    ) -> Result<String> {
        let app_state = match state {
            AppState::Ephemeral => "ephemeral",
            AppState::Deployed => "deployed",
        };
        self.requests.push(PlannedRequest::AppPublish { app_state });
        Ok(String::new())
    }
}

impl crate::App {
    /// Render the RUN manifest for `entrypoint` WITHOUT any network (the additive P8
    /// dump). It is literally `provision(RecordingControlPlane, …, RUN_BOUNDARY)`, so
    /// the returned [`Manifest`] is exactly what `.remote()` WOULD send (cargo cache
    /// volume, secrets, user volumes, client+source+python mounts, image, precreate,
    /// FILE-mode `FunctionCreate`, ephemeral `AppPublish`) — it cannot drift from the
    /// live path because it drives the SAME [`provision`] sequence.
    ///
    /// Sync + offline: it never connects and never sends. It resolves the decorator
    /// config via [`config_for`](crate::App::config_for) exactly as `.remote()` does,
    /// so the dumped gpu/timeout/secrets/volumes match the wire. Additive — does NOT
    /// change [`remote`](crate::Function::remote).
    pub fn dry_run(&self, entrypoint: &str, config: &RemoteConfig) -> Result<Manifest> {
        // Fold the decorator config exactly as `App::resolve_function` does, so the
        // dumped manifest matches what `.remote()` would send for this entrypoint.
        let mut options = self.config_for(entrypoint);
        let cfg = {
            let mut c = config.clone();
            let effective_cache = options.cache.unwrap_or(c.cache);
            options.cache = Some(effective_cache);
            c.options = options;
            c
        };
        // The dump uses the connected ephemeral app's name if present, else falls
        // back to the config package (a bare, unconnected App has no app name).
        let app_name = self.dump_app_name(&cfg.package);

        // ONE RUN entrypoint per provision call (the live path memoizes per entrypoint).
        let timeout = cfg.options.timeout_secs.unwrap_or(cfg.timeout_secs);
        let entrypoints = [Entrypoint {
            name: entrypoint.to_string(),
            options: cfg.options.clone(),
            timeout_secs: timeout,
        }];
        let inputs = ProvisionInputs {
            app_name: &app_name,
            app_id: None,
            source: SourceInputs {
                local_root: &cfg.local_root,
                package: &cfg.package,
                use_cargo_scoping: cfg.use_cargo_scoping,
                modalignore_name: &cfg.modalignore_name,
                remote_src: &cfg.remote_src,
            },
            base_image: &cfg.base_image,
            install_rust: cfg.install_rust,
            image_steps: &cfg.image_steps,
            cache: cfg.options.cache.unwrap_or(cfg.cache),
            entrypoints: &entrypoints,
        };

        let requests = record_provision(&inputs, &RUN_BOUNDARY)?;
        Ok(Manifest {
            mode: RunMode::Run,
            app_name,
            requests,
        })
    }

    /// Render the DEPLOY manifest WITHOUT any network (the additive P8 dump). It is
    /// literally `provision(RecordingControlPlane, …, DEPLOY_BOUNDARY)`, so the
    /// returned [`Manifest`] is exactly what `deploy` WOULD send: client +
    /// source(build context) + python mounts, the persistent `AppGetOrCreate`, TWO
    /// image layers (the top layer carries `cargo build --release`), per-entrypoint
    /// precreate + the CLIENT-mount-only `FunctionCreate` (the deploy build boundary),
    /// and the persistent `AppPublish` — driven by the SAME [`provision`] sequence as
    /// the live path, so it cannot drift.
    ///
    /// Sync + offline. Additive — does NOT change [`deploy`](crate::App::deploy).
    pub fn dump_deploy_manifest(&self, config: &DeployConfig) -> Result<Manifest> {
        let config = config.clone();
        // Per-entrypoint deploy plan, exactly as `App::deploy_with` builds it: one
        // function per entrypoint (object tag = entrypoint) with its OWN config; the
        // manual/no-decorator path falls back to a single default function.
        let plan = self.deploy_entrypoints_for_dump(&config);
        let entrypoints: Vec<Entrypoint> = plan
            .iter()
            .map(|ep| Entrypoint {
                name: ep.name.clone(),
                options: ep.options.clone(),
                timeout_secs: ep.options.timeout_secs.unwrap_or(config.timeout_secs),
            })
            .collect();

        let inputs = ProvisionInputs {
            app_name: &config.app_name,
            app_id: None,
            source: SourceInputs {
                local_root: &config.local_root,
                package: &config.package,
                use_cargo_scoping: config.use_cargo_scoping,
                modalignore_name: &config.modalignore_name,
                remote_src: DEPLOY_SRC,
            },
            base_image: &config.base_image,
            install_rust: config.install_rust,
            image_steps: &config.image_steps,
            cache: false,
            entrypoints: &entrypoints,
        };

        let requests = record_provision(&inputs, &DEPLOY_BOUNDARY)?;
        Ok(Manifest {
            mode: RunMode::Deploy,
            app_name: config.app_name.clone(),
            requests,
        })
    }
}

/// Drive the ONE [`provision`] sequence over a [`RecordingControlPlane`] and return
/// the recorded [`PlannedRequest`]s, in send order — the manifest body. Offline +
/// sync-from-the-caller's-view (the futures the recording impl returns are all
/// already-ready, so `block_on` a current-thread runtime never touches the network).
fn record_provision(
    inputs: &ProvisionInputs<'_>,
    boundary: &Boundary,
) -> Result<Vec<PlannedRequest>> {
    let mut cp = RecordingControlPlane::new();
    let mut published = Published::default();
    // The recording impl performs NO I/O — every method body returns a ready future —
    // so a minimal current-thread executor drives it to completion synchronously,
    // keeping `dry_run` / `dump_deploy_manifest` non-async (no network, no tokio rt).
    block_on_ready(provision(&mut cp, inputs, boundary, &mut published))?;
    Ok(cp.requests)
}

/// Drive an I/O-free future to completion on the current thread with a no-op waker.
/// The [`RecordingControlPlane`] never yields `Poll::Pending` (it does no real I/O),
/// so a single `poll` resolves it — letting the dump stay a synchronous, offline API.
fn block_on_ready<F: std::future::Future>(fut: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn noop_raw_waker() -> RawWaker {
        fn no_op(_: *const ()) {}
        fn clone(_: *const ()) -> RawWaker {
            noop_raw_waker()
        }
        let vtable = &RawWakerVTable::new(clone, no_op, no_op, no_op);
        RawWaker::new(std::ptr::null(), vtable)
    }
    let waker = unsafe { Waker::from_raw(noop_raw_waker()) };
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(out) => return out,
            // The recording control plane does no real I/O, so it never parks; a busy
            // re-poll is correct and terminates immediately in practice.
            Poll::Pending => continue,
        }
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
            milli_cpu: None,
            memory_mb: None,
            secrets: &[],
            volumes: &[],
            retries: None,
            schedule: None,
            ..FunctionConfig::default()
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
             mount_ids=2 gpu=Some(\"T4\") cpu=0m memory=0MiB timeout=1800s"
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
    fn dry_run_image_steps_ride_into_the_run_image_dockerfile() {
        // PARITY.md §3: apt_install / pip_install / run_commands ride the RUN image
        // dockerfile (layer 0), in chain order, AFTER provisioning and BEFORE the
        // (baked) wrapper. Build-path config on RemoteConfig, not the decorator.
        use crate::ImageStep;
        let app = App::from_manifest([("add".to_string(), FunctionConfig::default())]);
        let cfg = RemoteConfig {
            image_steps: vec![
                ImageStep::apt(["libpng-dev"]),
                ImageStep::pip(["numpy"]),
                ImageStep::run(["echo hi > /opt/marker"]),
            ],
            ..run_cfg()
        };
        let manifest = app.dry_run("add", &cfg).expect("dry_run");
        let img = manifest
            .requests
            .iter()
            .find_map(|r| match r {
                PlannedRequest::ImageGetOrCreate {
                    dockerfile_commands,
                    layer: 0,
                } => Some(dockerfile_commands.clone()),
                _ => None,
            })
            .expect("RUN image (layer 0)");

        let apt = img
            .iter()
            .position(|c| c.contains("apt-get install") && c.contains("libpng-dev"))
            .expect("apt_install rendered");
        let pip = img
            .iter()
            .position(|c| c == "RUN python3 -m pip install --no-cache-dir numpy")
            .expect("pip_install rendered");
        let run = img
            .iter()
            .position(|c| c == "RUN echo hi > /opt/marker")
            .expect("run_commands rendered");
        let copy = img
            .iter()
            .position(|c| c == "COPY /python/. /usr/local")
            .expect("add_python provisioning present");
        let bake = img
            .iter()
            .position(|c| c.contains("b64decode("))
            .expect("wrapper bake present");

        assert!(
            apt < pip && pip < run,
            "chain order preserved (apt<pip<run)"
        );
        assert!(copy < apt, "provisioning precedes the image steps");
        assert!(run < bake, "image steps precede the wrapper bake");
        // RUN image still has no cargo build (builds in-body).
        assert!(!img.iter().any(|c| c.contains("cargo build")));
    }

    #[test]
    fn dump_deploy_image_steps_ride_into_the_base_layer_not_the_top() {
        // The DEPLOY base layer (layer 0) carries the image steps so the TOP layer's
        // image-build-time cargo build inherits the deps; the top layer (layer 1) keeps
        // the COPY + cargo build, with NO image-step duplication.
        use crate::ImageStep;
        let app = App::from_manifest([("add".to_string(), FunctionConfig::default())]);
        let dcfg = DeployConfig {
            app_name: "dep".to_string(),
            package: "app".to_string(),
            base_image: "rust:1-slim".to_string(),
            use_cargo_scoping: false,
            image_steps: vec![ImageStep::apt(["libssl-dev"]), ImageStep::pip(["requests"])],
            ..DeployConfig::for_app("dep")
        };
        let manifest = app.dump_deploy_manifest(&dcfg).expect("dump_deploy");
        let layer = |n: u8| {
            manifest
                .requests
                .iter()
                .find_map(|r| match r {
                    PlannedRequest::ImageGetOrCreate {
                        dockerfile_commands,
                        layer,
                    } if *layer == n => Some(dockerfile_commands.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("layer {n} image present"))
        };
        let base = layer(0);
        let top = layer(1);

        // Base layer carries the steps + provisioning, NO cargo build.
        assert!(base
            .iter()
            .any(|c| c.contains("apt-get install") && c.contains("libssl-dev")));
        assert!(base
            .iter()
            .any(|c| c == "RUN python3 -m pip install --no-cache-dir requests"));
        assert!(!base.iter().any(|c| c.contains("cargo build")));

        // Top layer keeps COPY + cargo build and does NOT duplicate the image steps.
        assert!(top.iter().any(|c| c.contains("cargo build --release")));
        assert!(top.iter().any(|c| c == "COPY . /"));
        assert!(
            !top.iter()
                .any(|c| c.contains("libssl-dev") || c.contains("pip install")),
            "image steps live ONLY on the base layer"
        );
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
