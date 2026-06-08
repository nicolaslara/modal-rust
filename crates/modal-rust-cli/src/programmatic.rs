//! The DEFAULT (programmatic) `run`/`deploy`/`call` path (P9 §C).
//!
//! Instead of rendering a Python shim and shelling out to the official `modal` CLI,
//! the CLI drives the proven SDK/facade orchestration directly:
//!
//! 1. `cargo build -p <package> --bin modal_runner` (DEBUG; cwd = workspace root) —
//!    the SAME `-p <pkg>` the shims used. DEBUG (not `--release`) so it REUSES the
//!    user's normal `cargo build`/`cargo check` debug artifacts instead of paying a
//!    cold release compile in a throwaway target. This LOCAL build is for the manifest
//!    ONLY; the REMOTE build still happens per the frozen build boundary (in-body for
//!    `run`, at image-build for `deploy`, both `--release`). The CLI does NOT upload
//!    this local binary.
//! 2. Run `<target>/debug/modal_runner --describe`, parse the `modal-rust/describe@1`
//!    manifest (entrypoints + per-entrypoint `FunctionOptions`).
//!
//! A MANIFEST CACHE (`describe_cache`) keyed on the closure source + `Cargo.lock` short-
//! circuits steps 1+2 entirely on a hit (0s): the manifest is the only thing the CLI
//! needs from the local build, so a cached copy is a complete substitute.
//! 3. Drive the facade `App`: `run` = ephemeral app (`remote_envelope`), `deploy` =
//!    persistent (`deploy_with`), `call` = `from_name` + invoke (`call_envelope`).
//!
//! It emits NO generated `.py` and spawns NO `modal` subprocess. The only
//! subprocesses are `cargo` (build) and the user's `modal_runner` (`--describe`).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use modal_rust::{App, DeployConfig, FunctionOptions, RemoteConfig};

use crate::describe_cache;
use crate::workspace;

/// The `--describe` manifest schema this CLI understands. The CLI warns-and-proceeds
/// on an unknown MINOR; HARD-errors on an unknown MAJOR (P9 §A.3 / §C.3).
const DESCRIBE_SCHEMA_FAMILY: &str = "modal-rust/describe@";
const DESCRIBE_SCHEMA_MAJOR: u32 = 1;

/// One entrypoint in the parsed manifest.
#[derive(Debug, Clone, Deserialize)]
struct ManifestEntry {
    name: String,
    config: FunctionOptions,
}

/// The parsed `--describe` manifest.
#[derive(Debug, Deserialize)]
struct Manifest {
    schema: String,
    entrypoints: Vec<ManifestEntry>,
}

impl Manifest {
    /// Look up an entrypoint by name, returning a clear error (listing the known
    /// names) on a miss — parity with `run_cli`'s unknown-entrypoint diagnostic.
    fn entry(&self, name: &str) -> Result<&ManifestEntry> {
        self.entrypoints
            .iter()
            .find(|e| e.name == name)
            .with_context(|| {
                let known: Vec<String> = self
                    .entrypoints
                    .iter()
                    .map(|e| format!("{:?}", e.name))
                    .collect();
                format!(
                    "unknown entrypoint {name:?}; known entrypoints: [{}]",
                    known.join(", ")
                )
            })
    }
}

/// Build the manifest configs (`name -> FunctionOptions`) the facade `App` carries.
fn manifest_configs(manifest: &Manifest) -> Vec<(String, FunctionOptions)> {
    manifest
        .entrypoints
        .iter()
        .map(|e| (e.name.clone(), e.config.clone()))
        .collect()
}

/// Validate the manifest `schema` tag: warn-and-proceed on an unknown minor;
/// HARD-error on an unknown major (P9 §A.3 / §C.3).
fn check_schema(schema: &str) -> Result<()> {
    let version = schema
        .strip_prefix(DESCRIBE_SCHEMA_FAMILY)
        .with_context(|| format!("unrecognized --describe schema {schema:?}"))?;
    // Versions are bare major ("1") for describe@1; tolerate "major.minor".
    let major: u32 = version
        .split('.')
        .next()
        .unwrap_or("")
        .parse()
        .with_context(|| format!("unparseable --describe schema version in {schema:?}"))?;
    if major != DESCRIBE_SCHEMA_MAJOR {
        bail!(
            "incompatible --describe schema major {major} (this modal-rust expects \
             {DESCRIBE_SCHEMA_FAMILY}{DESCRIBE_SCHEMA_MAJOR}); rebuild your crate against \
             a matching modal-rust-runtime"
        );
    }
    Ok(())
}

/// Build the user crate's `modal_runner` and read its `--describe` manifest (P9 §C.3).
///
/// Returns the parsed [`Manifest`] plus the resolved `(workspace_root, package)` the
/// caller threads into `RemoteConfig`/`DeployConfig`. The returned `workspace_root` is
/// ALWAYS the real on-disk root (the upload root run/deploy will scope) — never the
/// temp shadow, which is local-build-only and removed before return.
///
/// Auto-detect (inject-bin, design B):
/// - If the target crate ALREADY ships a `modal_runner` bin → build it DEBUG at the
///   real workspace root and run `<target>/debug/modal_runner --describe` (today's
///   path, backward-compatible, byte-identical manifest — only the profile changed).
/// - Otherwise GENERATE: materialize a temp SHADOW copy of the crate's cargo
///   dependency closure with the generated `src/bin/modal_runner.rs` injected, build
///   `-p <pkg> --bin modal_runner` (DEBUG) THERE (cwd = shadow root) but with
///   `CARGO_TARGET_DIR` pointed at the USER's shared target so the ~190 dep crates are
///   CACHE HITS (only the copied lib + the tiny generated bin recompile, ~0.5s), then
///   run the shared target's `debug/modal_runner --describe`. The user's on-disk `src/`
///   is never touched; the shadow resolves `modal-rust` identically to the real upload.
///
/// A debug profile (not `--release`) is correct because `--describe` only reads the
/// inventory registry + per-entrypoint `FunctionOptions` and serializes JSON — the
/// manifest is profile-independent. Debug lets the local build reuse the user's warm
/// debug artifacts; the REMOTE runner is built remotely (`--release`), so the local
/// binary is throwaway either way.
/// The clear, actionable error for a crate that `modal-rust run`/`deploy` CANNOT build a
/// runner for: it is neither generatable (no `#[modal_rust::function]`/inventory) NOR
/// ships a `modal_runner` bin. `bins` is THIS crate's real bin target(s) (from cargo
/// metadata) — never an unrelated sibling's — so the "run it via its own bin" hint names
/// the user's actual binary (e.g. `add-runner`), unlike cargo's confusing default help.
fn unrunnable_crate_error(package: &str, bins: &[String]) -> String {
    // Name the crate's own bin in the manual-registry hint when it ships exactly one;
    // otherwise keep a generic `<bin>` placeholder (zero bins, or ambiguous with several).
    let own_bin = match bins {
        [only] => only.clone(),
        _ => "<bin>".to_string(),
    };
    let ships = if bins.is_empty() {
        "ships no bin targets".to_string()
    } else {
        format!("ships bin target(s): {}", bins.join(", "))
    };
    format!(
        "cannot run crate '{package}': it exposes no #[modal_rust::function] entrypoints \
         (so the CLI cannot generate a runner) and ships no `modal_runner` bin ({ships}). \
         If this is a manual-registry crate, run it via its own bin (e.g. \
         `cargo run -p {package} --bin {own_bin} -- --entrypoint <fn> --input-json <json>`). \
         To use `modal-rust run`, add a #[modal_rust::function] (see examples/quickstart) \
         or ship a `modal_runner` bin."
    )
}

/// The clear error for a GENERATABLE crate whose `--describe` yielded ZERO entrypoints:
/// it has a `modal-rust` dep (so the runner built) but defines no `#[modal_rust::function]`
/// fns, so there is nothing to run or deploy.
fn no_entrypoints_error(package: &str) -> String {
    format!(
        "no #[modal_rust::function] entrypoints found in crate '{package}': the runner built \
         but reported zero entrypoints. Add a #[modal_rust::function] fn (see \
         examples/quickstart) so `modal-rust run`/`deploy` has something to invoke."
    )
}

/// The outcome of [`build_and_describe`]: the parsed manifest, the real workspace
/// root, the resolved package, and the path to the LOCAL `modal_runner` binary the
/// describe build produced — `None` only when describe was served from the manifest
/// CACHE and the binary is not on disk (so `--check-input` validation is skipped, not
/// forced to rebuild). The runner path is REUSED for `--check-input` local input
/// validation before any remote call.
struct DescribeOutcome {
    manifest: Manifest,
    root: std::path::PathBuf,
    package: String,
    runner_bin: Option<std::path::PathBuf>,
}

fn build_and_describe(project: &Path) -> Result<DescribeOutcome> {
    let root = workspace::workspace_root(project)?;
    // `-p <pkg>` disambiguates the shared `modal_runner` bin across workspace members
    // (boundaries.md §8) — the SAME package run/deploy build.
    let package = workspace::package_name(project)?;

    // Auto-detect: does the target ship its own `modal_runner` bin (or is it not
    // generatable)? `resolve_runner_target` reads the SAME `cargo metadata` +
    // target manifest the upload path uses, so the decision cannot drift.
    let target = modal_rust::resolve_runner_target(&root, &package);

    // Short-circuit BEFORE the doomed `cargo build -p <pkg> --bin modal_runner`: if the
    // crate is NEITHER generatable (no `#[modal_rust::function]`/inventory for the CLI to
    // synthesize a runner) NOR ships its OWN `modal_runner` bin, that build can only fail
    // with cargo's confusing "no bin target named `modal_runner`" (whose "available bin
    // in <pkg>" help points at an UNRELATED sibling crate). Emit a clear, actionable
    // error naming THIS crate's real bin(s) instead. Skipped when `target` is `None`
    // (metadata unavailable) so behavior is unchanged on the metadata-fallback path.
    if let Some(t) = &target {
        if !t.is_runnable() {
            bail!("{}", unrunnable_crate_error(&package, &t.bin_targets));
        }
    }

    let generate = target.as_ref().map(|t| t.is_generatable()).unwrap_or(false);

    // The user's REAL target dir (honoring a custom `CARGO_TARGET_DIR`, else
    // `<root>/target`). This is where BOTH the own-bin build (via cwd=root) and the
    // shadow build (via the env we set below) deposit artifacts, and where the manifest
    // cache lives — so describe artifacts reuse the user's warm deps and travel with the
    // gitignored `target/`.
    let shared_target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));

    // MANIFEST CACHE consult: a hit returns the parsed manifest with NO build + NO exec
    // (0s). The key cannot be computed (→ `None`) when metadata is unavailable, in which
    // case we fall through to building, exactly the prior behavior. The stored bytes are
    // re-validated through the SAME parse + schema check the live path uses, so a corrupt
    // entry degrades to a rebuild, never a bad manifest.
    let cache_key = describe_cache::key(&root, &package, generate);
    if let Some(key) = &cache_key {
        if let Some(bytes) = describe_cache::load(&shared_target, key) {
            if let Ok(manifest) = parse_and_check(&bytes) {
                // A cached EMPTY manifest gets the SAME clear "no entrypoints" error as a
                // fresh build, rather than skipping the check on a cache hit.
                if manifest.entrypoints.is_empty() {
                    bail!("{}", no_entrypoints_error(&package));
                }
                eprintln!("modal-rust: describe cache hit ({package}); skipping build");
                // The cache skips the build, so the runner binary MAY be absent (e.g.
                // after `cargo clean`). Surface it for `--check-input` only when it is
                // actually on disk; otherwise validation degrades to the remote decode
                // check (no forced rebuild — keeps the cache-hit happy path fast).
                let runner_bin = shared_target.join("debug").join("modal_runner");
                let runner_bin = runner_bin.is_file().then_some(runner_bin);
                return Ok(DescribeOutcome {
                    manifest,
                    root,
                    package,
                    runner_bin,
                });
            }
        }
    }

    // Pick the BUILD root: a temp shadow when generating, else the real root. Held in
    // an Option so the shadow temp dir is cleaned up on EVERY exit path.
    let shadow = if generate {
        let t = target.as_ref().expect("generate => target resolved");
        let dest = shadow_dir_for(&package);
        // Best-effort clean of a stale shadow from a previous run, then materialize.
        let _ = std::fs::remove_dir_all(&dest);
        modal_rust::materialize_shadow(&root, t, &dest).with_context(|| {
            format!(
                "failed to materialize shadow build tree at {}",
                dest.display()
            )
        })?;
        Some(ShadowDir(dest))
    } else {
        None
    };
    let build_root: &Path = shadow.as_ref().map(|s| s.0.as_path()).unwrap_or(&root);

    // 1. LOCAL build (manifest-only; NOT uploaded). cwd = the build root, inheriting
    //    stderr so the compile log streams. `Command::new("cargo")` — NOT `modal`.
    //    DEBUG (no `--release`) to reuse the user's warm artifacts. For the SHADOW build,
    //    point `CARGO_TARGET_DIR` at the user's shared target so the dep crates resolve
    //    against the user's warm fingerprints (cache hits). The own-bin build already
    //    deposits into the user's target via cwd=root (inheriting their CARGO_TARGET_DIR).
    if generate {
        eprintln!("modal-rust: generating {package} modal_runner (shadow build) for --describe …");
    } else {
        eprintln!("modal-rust: building {package} modal_runner (cargo) for --describe …");
    }
    let mut cmd = Command::new("cargo");
    cmd.args(["build", "-p", &package, "--bin", "modal_runner"])
        .current_dir(build_root);
    if generate {
        cmd.env("CARGO_TARGET_DIR", &shared_target);
    }
    let status = cmd
        .status()
        .context("failed to spawn `cargo` (is it on $PATH? run `modal-rust doctor --rust`)")?;
    if !status.success() {
        bail!(
            "cargo build of `{package}` modal_runner failed (exit {})",
            status.code().unwrap_or(-1)
        );
    }

    // 2. Run `modal_runner --describe`, capture stdout, parse the manifest. The shadow
    //    build deposits into the SHARED target (via the env above); the own-bin build
    //    deposits into the build root's own target (= the shared target, since cwd=root).
    let target_dir = if generate {
        shared_target.clone()
    } else {
        build_root.join("target")
    };
    let runner_bin = target_dir.join("debug").join("modal_runner");
    let out = Command::new(&runner_bin)
        .arg("--describe")
        .output()
        .with_context(|| format!("failed to run {} --describe", runner_bin.display()))?;
    if !out.status.success() {
        bail!(
            "{} --describe exited {} (stderr: {})",
            runner_bin.display(),
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let manifest = parse_and_check(&out.stdout)?;

    // A generatable crate that --describes to ZERO entrypoints has a `modal-rust` dep but
    // no `#[modal_rust::function]` fns: there is nothing to run/deploy. Surface this as a
    // clear "no entrypoints" message rather than letting the later `manifest.entry(...)`
    // emit a bare "unknown entrypoint; known: []".
    if manifest.entrypoints.is_empty() {
        bail!("{}", no_entrypoints_error(&package));
    }

    // Store the verbatim manifest bytes for the next invocation (best-effort; a read-only
    // target or an uncomputable key silently skips the write — never an error).
    if let Some(key) = &cache_key {
        describe_cache::store(&shared_target, key, &out.stdout);
    }

    // The shadow (if any) is dropped here, removing the temp tree. The returned
    // `root` is the REAL workspace root (the upload root), not the shadow.
    drop(shadow);
    Ok(DescribeOutcome {
        manifest,
        root,
        package,
        // Freshly built this invocation → present on disk; reused for `--check-input`.
        runner_bin: Some(runner_bin),
    })
}

/// Validate `input_json` LOCALLY against `entrypoint`'s declared input shape by running
/// the already-built `runner_bin` in DECODE-ONLY `--check-input` mode — NO Modal call,
/// NO handler body. Returns `Err` (fail fast) when the input cannot be decoded, with a
/// clear message naming the entrypoint + the runner's decode diagnostic; returns `Ok`
/// when the input decodes, the entrypoint has no checker (handler-only registration),
/// or the check could not run (so the happy path is never blocked by a tooling hiccup).
///
/// The runner emits ONE JSON line: `{"ok":true}` (decodes) / `{"ok":true,"checked":
/// false}` (no local checker) / the frozen failure envelope (`decode_error` /
/// `unknown_entrypoint`). We key off the exit code + the envelope's `error.kind`:
/// `decode_error` is a real input-shape mismatch (fail fast); `unknown_entrypoint`
/// means the on-disk runner does NOT register this entrypoint, which — since the
/// caller already confirmed it exists in the held describe manifest — can only mean the
/// runner binary at the shared `<target>/debug/modal_runner` path is a DIFFERENT
/// generatable crate's last build (all generated runners collide on that one path). In
/// that case the local check is not authoritative for THIS crate, so we degrade to the
/// remote decode check rather than false-rejecting a valid run.
fn validate_input_locally(runner_bin: &Path, entrypoint: &str, input_json: &str) -> Result<()> {
    let out = match Command::new(runner_bin)
        .args(["--check-input", "--entrypoint", entrypoint, "--input-json"])
        .arg(input_json)
        .output()
    {
        Ok(o) => o,
        // Could not spawn the checker (binary vanished, exec error): do NOT block the
        // run — degrade to the remote decode check, exactly the prior behavior.
        Err(_) => return Ok(()),
    };

    // Exit 0 ⇒ input decoded (or no local checker) ⇒ proceed.
    if out.status.success() {
        return Ok(());
    }

    // A non-zero exit is a local check failure. Parse the runner's one-line envelope to
    // read its `error.kind` + `error.message`.
    let stdout = String::from_utf8_lossy(&out.stdout);
    let envelope = stdout
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<serde_json::Value>(line).ok());
    let kind = envelope
        .as_ref()
        .and_then(|v| v.get("error"))
        .and_then(|e| e.get("kind"))
        .and_then(|k| k.as_str());

    // `unknown_entrypoint` here is NOT a user error: the manifest (consulted before this
    // call) DOES contain `entrypoint`, so an on-disk runner that does not know it is a
    // stale/wrong binary at the shared `modal_runner` path (the cache-hit collision
    // across generatable crates). Degrade to the remote decode check — do not block.
    if kind == Some("unknown_entrypoint") {
        return Ok(());
    }

    let detail = envelope
        .as_ref()
        .and_then(|v| v.get("error"))
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| stdout.trim().to_string());

    bail!(
        "input does not match entrypoint '{entrypoint}': {detail}. \
         The input did not decode locally, so the call was NOT sent to Modal. \
         Pass a matching --input '{{...}}' (see the example's README, e.g. \
         examples/autoscaling/README.md)."
    );
}

/// Parse `--describe` stdout into a [`Manifest`] and validate its schema. The SAME
/// validation runs on both the live build's stdout AND the cached bytes, so a corrupt or
/// schema-incompatible cache file degrades to `Err` (→ the caller rebuilds), never a bad
/// manifest.
fn parse_and_check(bytes: &[u8]) -> Result<Manifest> {
    let manifest: Manifest = serde_json::from_slice(bytes)
        .context("failed to parse `modal_runner --describe` manifest JSON")?;
    check_schema(&manifest.schema)?;
    Ok(manifest)
}

/// A temp shadow build dir removed on drop (so `build_and_describe` cleans up on every
/// exit path, including the `?`/`bail!` error paths).
struct ShadowDir(std::path::PathBuf);

impl Drop for ShadowDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A unique, gitignored temp dir for the `--describe` shadow build (PID + monotonic
/// counter so concurrent `modal-rust describe` invocations never collide).
fn shadow_dir_for(package: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "modal-rust-shadow-{package}-{}-{n}",
        std::process::id()
    ))
}

/// Print the runner's one-line JSON envelope VERBATIM to stdout and mirror its `ok`
/// field into the process exit code (0 success / 1 failure) — the SAME contract the
/// shim path produced (P9 §G.1).
fn print_envelope_and_exit_code(envelope: &str) -> i32 {
    println!("{envelope}");
    match serde_json::from_str::<serde_json::Value>(envelope) {
        Ok(v) if v.get("ok") == Some(&serde_json::Value::Bool(true)) => 0,
        _ => 1,
    }
}

/// DEFAULT `run`: build + describe, then drive an EPHEMERAL app via the facade's
/// `App::connect_from_manifest` + `remote_envelope` (mirrors `.remote()`). NO
/// generated `.py`, NO `modal` subprocess (P9 §C.4).
pub async fn cmd_run_programmatic(
    entrypoint: &str,
    project: &Path,
    input_json: String,
    timeout: Option<u64>,
) -> Result<i32> {
    let DescribeOutcome {
        manifest,
        root,
        package,
        runner_bin,
    } = build_and_describe(project)?;
    let _ = manifest.entry(entrypoint)?;

    // LOCAL input validation (fail fast, NO Modal call): decode `--input` against the
    // entrypoint's input shape via the already-built runner's `--check-input` mode. A
    // bad-shape input (e.g. `run embed` with no `--input`) errors HERE with a clear
    // expected-shape message instead of building+running on Modal only to fail with a
    // remote `decode_error`. Skipped (degrades to the remote check) only when the local
    // runner binary is unavailable (a cache hit after `cargo clean`).
    if let Some(runner_bin) = &runner_bin {
        validate_input_locally(runner_bin, entrypoint, &input_json)?;
    }

    if let Some(t) = timeout {
        eprintln!(
            "modal-rust: note: --timeout {t}s is informational; the entrypoint's decorator \
             timeout (or the run-path default) applies."
        );
    }

    // Build the EXPLICIT RemoteConfig from the real workspace root + package + the
    // resolved per-entrypoint config (the SAME fields `App::remote_invoke` applies).
    let run_config = RemoteConfig {
        local_root: root,
        package,
        ..RemoteConfig::default()
    };

    let configs = manifest_configs(&manifest);
    let app = App::connect_from_manifest("modal-rust-cli-run", configs, run_config)
        .await
        .context("failed to connect to Modal for the run path")?;
    let envelope = app
        .remote_envelope(entrypoint, input_json)
        .await
        .context("remote run failed")?;
    Ok(print_envelope_and_exit_code(&envelope))
}

/// DEFAULT `deploy`: build + describe, then drive a PERSISTENT deploy via the
/// facade's `App::deploy_with` (mirrors `App::deploy`). The decorated gpu/timeout is
/// resolved INSIDE `deploy_with` from the manifest configs (P9 §C.4).
pub async fn cmd_deploy_programmatic(
    entrypoint: &str,
    project: &Path,
    app_name: &str,
) -> Result<i32> {
    let DescribeOutcome {
        manifest,
        root,
        package,
        runner_bin: _,
    } = build_and_describe(project)?;
    // Deploy publishes every manifest entrypoint as its own Modal function over one
    // shared image. The selected `entrypoint` is still not the only deployed
    // function, but validate it exists so a typo fails fast — parity with run.
    let _ = manifest.entry(entrypoint)?;
    eprintln!(
        "modal-rust: note: entrypoint {entrypoint:?} is bound at call time, not deploy time."
    );

    let deploy_config = DeployConfig {
        local_root: root,
        package,
        ..DeployConfig::for_app(app_name)
    };

    let configs = manifest_configs(&manifest);
    // `connect_from_manifest`'s RemoteConfig is unused by the deploy path (deploy
    // reads DeployConfig), so the default is fine here.
    let app = App::connect_from_manifest(app_name, configs, RemoteConfig::default())
        .await
        .context("failed to connect to Modal for the deploy path")?;
    let deployed = app
        .deploy_with(deploy_config)
        .await
        .context("deploy failed")?;
    eprintln!(
        "modal-rust: deployed app {:?} (function_id={}, image_id={}, url={:?})",
        deployed.name, deployed.function_id, deployed.image_id, deployed.url
    );
    println!("deployed: {}", deployed.name);
    Ok(0)
}

/// DEFAULT `call`: `from_name` + invoke via the facade's `App::call_envelope`. NO
/// build, NO describe, NO upload — the deployed wrapper already carries its config
/// (P9 §C.4). Builds a headless app with empty configs + a default RemoteConfig.
pub async fn cmd_call_programmatic(
    entrypoint: &str,
    app_name: &str,
    input_json: String,
) -> Result<i32> {
    let app = App::connect_from_manifest(
        "modal-rust-cli-call",
        std::iter::empty::<(String, FunctionOptions)>(),
        RemoteConfig::default(),
    )
    .await
    .context("failed to connect to Modal for the call path")?;
    let envelope = app
        .call_envelope(app_name, entrypoint, input_json)
        .await
        .context("call failed")?;
    Ok(print_envelope_and_exit_code(&envelope))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_accepts_describe_v1() {
        assert!(check_schema("modal-rust/describe@1").is_ok());
    }

    #[test]
    fn schema_accepts_unknown_minor() {
        // Unknown minor warns-and-proceeds (still major 1).
        assert!(check_schema("modal-rust/describe@1.5").is_ok());
    }

    #[test]
    fn schema_rejects_unknown_major() {
        let err = check_schema("modal-rust/describe@2").unwrap_err();
        assert!(err.to_string().contains("incompatible"));
    }

    #[test]
    fn schema_rejects_foreign_family() {
        assert!(check_schema("other/thing@1").is_err());
    }

    #[test]
    fn manifest_parses_and_resolves_entry() {
        let json = r#"{
            "schema": "modal-rust/describe@1",
            "entrypoints": [
                {"name": "add", "config": {"gpu": null, "timeout_secs": null, "cache": null}},
                {"name": "vector_add", "config": {"gpu": "T4", "timeout_secs": 1800, "cache": false}}
            ]
        }"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        check_schema(&m.schema).unwrap();
        let add = m.entry("add").unwrap();
        assert_eq!(add.config.gpu, None);
        let va = m.entry("vector_add").unwrap();
        assert_eq!(va.config.gpu.as_deref(), Some("T4"));
        assert_eq!(va.config.timeout_secs, Some(1800));
        assert_eq!(va.config.cache, Some(false));
    }

    #[test]
    fn manifest_unknown_entrypoint_lists_known() {
        let json = r#"{"schema":"modal-rust/describe@1","entrypoints":[
            {"name":"add","config":{"gpu":null,"timeout_secs":null,"cache":null}}]}"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        let err = m.entry("nope").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown entrypoint \"nope\""));
        assert!(msg.contains("\"add\""));
    }

    #[test]
    fn manifest_configs_preserve_owned_options() {
        let json = r#"{"schema":"modal-rust/describe@1","entrypoints":[
            {"name":"add","config":{
                "gpu":"A100",
                "timeout_secs":900,
                "cache":true,
                "secrets":["my-secret"],
                "volumes":[["/data","my-vol"]]
            }}]}"#;
        let manifest: Manifest = serde_json::from_str(json).unwrap();
        let configs = manifest_configs(&manifest);
        let c = &configs[0].1;
        assert_eq!(configs[0].0, "add");
        assert_eq!(c.gpu.as_deref(), Some("A100"));
        assert_eq!(c.timeout_secs, Some(900));
        assert_eq!(c.cache, Some(true));
        assert_eq!(c.secrets, vec!["my-secret".to_string()]);
        assert_eq!(c.volumes, vec![("/data".to_string(), "my-vol".to_string())]);
    }

    #[test]
    fn unrunnable_crate_error_names_real_bin() {
        // The examples/add case: one real bin `add-runner`, no facade dep.
        let msg = unrunnable_crate_error("example-add", &["add-runner".to_string()]);
        // Names the crate and BOTH failure conditions.
        assert!(msg.contains("cannot run crate 'example-add'"), "{msg}");
        assert!(msg.contains("#[modal_rust::function]"), "{msg}");
        assert!(msg.contains("modal_runner"), "{msg}");
        // Lists the crate's REAL bin and uses it in the cargo-run hint…
        assert!(msg.contains("ships bin target(s): add-runner"), "{msg}");
        assert!(
            msg.contains("cargo run -p example-add --bin add-runner"),
            "{msg}"
        );
        // …and points at examples/quickstart for the macro path.
        assert!(msg.contains("examples/quickstart"), "{msg}");
        // It must NOT mention any unrelated sibling package (cargo's confusing default).
        assert!(!msg.contains("own-runner-bin"), "{msg}");
    }

    #[test]
    fn unrunnable_crate_error_handles_zero_and_multiple_bins() {
        // Zero bins → generic <bin> placeholder + "ships no bin targets".
        let none = unrunnable_crate_error("libonly", &[]);
        assert!(none.contains("ships no bin targets"), "{none}");
        assert!(none.contains("--bin <bin>"), "{none}");
        // Several bins → list them all; keep a generic placeholder (ambiguous which).
        let many =
            unrunnable_crate_error("multi", &["a-runner".to_string(), "b-runner".to_string()]);
        assert!(
            many.contains("ships bin target(s): a-runner, b-runner"),
            "{many}"
        );
        assert!(many.contains("--bin <bin>"), "{many}");
    }

    #[test]
    fn no_entrypoints_error_is_clear() {
        let msg = no_entrypoints_error("quickstart");
        assert!(
            msg.contains("no #[modal_rust::function] entrypoints found"),
            "{msg}"
        );
        assert!(msg.contains("'quickstart'"), "{msg}");
        assert!(msg.contains("examples/quickstart"), "{msg}");
    }

    #[test]
    fn print_envelope_exit_code_mirrors_ok() {
        assert_eq!(
            print_envelope_and_exit_code(r#"{"ok":true,"value":{"sum":42}}"#),
            0
        );
        assert_eq!(
            print_envelope_and_exit_code(r#"{"ok":false,"error":{"kind":"panic"}}"#),
            1
        );
        assert_eq!(print_envelope_and_exit_code("garbage"), 1);
    }

    // ---- local input validation (fix A): fail fast, NO Modal call ----

    /// Write a fake `modal_runner` that mimics the runner's `--check-input` contract:
    /// it exits 1 + prints the frozen `decode_error` envelope when the input is the
    /// "bad" sentinel, else exits 0 + prints `{"ok":true}`. Stands in for the real
    /// built runner so the test exercises `validate_input_locally` with NO Modal call,
    /// NO cargo build, and NO network — purely the local decode-only seam.
    #[cfg(unix)]
    fn write_fake_runner(behavior: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!(
            "fake-modal-runner-{}-{}",
            std::process::id(),
            // a per-call nonce so concurrent tests never share a path
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, behavior).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn validate_input_locally_rejects_bad_input_without_network() {
        // A runner that ALWAYS reports a decode failure (the `embed`-with-no-input case).
        let runner = write_fake_runner(
            "#!/bin/sh\n\
             echo '{\"ok\":false,\"error\":{\"kind\":\"decode_error\",\"message\":\"missing field `text`\",\"details\":null,\"backtrace\":\"\"}}'\n\
             exit 1\n",
        );
        let err = validate_input_locally(&runner, "embed", r#"{"a":40,"b":2}"#).unwrap_err();
        let msg = err.to_string();
        // Names the entrypoint, surfaces the runner's decode diagnostic, and states the
        // call was NOT sent to Modal (fail fast, no remote).
        assert!(msg.contains("entrypoint 'embed'"), "{msg}");
        assert!(msg.contains("missing field `text`"), "{msg}");
        assert!(msg.contains("NOT sent to Modal"), "{msg}");
        let _ = std::fs::remove_file(&runner);
    }

    #[cfg(unix)]
    #[test]
    fn validate_input_locally_accepts_good_input() {
        // A runner that reports the input decoded fine → validation passes (proceeds).
        let runner = write_fake_runner("#!/bin/sh\necho '{\"ok\":true}'\nexit 0\n");
        assert!(validate_input_locally(&runner, "add", r#"{"a":40,"b":2}"#).is_ok());
        let _ = std::fs::remove_file(&runner);
    }

    #[cfg(unix)]
    #[test]
    fn validate_input_locally_passes_when_no_local_checker() {
        // A handler-only entrypoint reports `checked:false` (exit 0) → do not block; the
        // remote decode check still applies, exactly the prior behavior.
        let runner =
            write_fake_runner("#!/bin/sh\necho '{\"ok\":true,\"checked\":false}'\nexit 0\n");
        assert!(validate_input_locally(&runner, "add", "{}").is_ok());
        let _ = std::fs::remove_file(&runner);
    }

    #[test]
    fn validate_input_locally_degrades_when_runner_missing() {
        // A non-existent runner binary must NOT block the run (degrade to remote check).
        let missing = std::path::Path::new("/no/such/modal_runner-xyz");
        assert!(validate_input_locally(missing, "add", "{}").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn validate_input_locally_degrades_on_unknown_entrypoint() {
        // The shared `modal_runner` path can hold a DIFFERENT generatable crate's last
        // build (all generated runners collide on `<target>/debug/modal_runner`). When the
        // describe cache skips the rebuild, the on-disk runner may not register THIS
        // crate's entrypoint and reports `unknown_entrypoint`. Since the caller already
        // confirmed the entrypoint exists in the held manifest, this is a stale/wrong
        // binary — NOT a user input error — so validation must degrade to the remote check
        // (return Ok), never false-reject the run.
        let runner = write_fake_runner(
            "#!/bin/sh\n\
             echo '{\"ok\":false,\"error\":{\"kind\":\"unknown_entrypoint\",\"message\":\"unknown entrypoint \\\"score\\\"; known entrypoints: [\\\"analyze\\\"]\",\"details\":null,\"backtrace\":\"\"}}'\n\
             exit 1\n",
        );
        assert!(
            validate_input_locally(&runner, "score", r#"{"name":"ada","hits":7,"shots":10}"#)
                .is_ok(),
            "unknown_entrypoint from a stale shared runner must degrade, not fail fast"
        );
        let _ = std::fs::remove_file(&runner);
    }
}
