# P9 Build Spec — CLI drives the programmatic SDK (no codegen, no `modal` CLI)

**Goal.** `modal-rust run|deploy|call <entrypoint>` produces the SAME result as today
(same `{"sum":42}`, same one-JSON-envelope, same five error kinds) but emits NO generated
`.py` and spawns NO `modal` subprocess. The default path drives the proven SDK/facade
orchestration via a new `modal_runner --describe` manifest. The legacy Python-shim +
`modal`-CLI shell-out is retained verbatim behind `--use-shim` (P10 deletes it).

**Two additive seams, zero changes to frozen behavior:**
1. `modal_runner --describe` — a NEW first-token subcommand in the runtime that emits the
   registry entrypoints + each `FunctionConfig` as JSON. The frozen `--entrypoint`/`--input-*`
   protocol, the one-envelope output, and the five error kinds are byte-identical when
   `--describe` is absent.
2. A headless `App::from_manifest` in the facade — config but NO handlers — so the CLI
   reuses the EXISTING `remote_invoke` / `deploy_with` / `call` orchestration without
   linking the user crate.

---

## A. Enabler: `modal_runner --describe` (runtime, additive)

### A.1 The config-availability gap and its resolution

`run_cli(registry)` (`crates/modal-rust-runtime/src/lib.rs:573`) takes only a `Registry`,
which is `BTreeMap<&'static str, HandlerFn>` (lib.rs:324–326) — names + handlers, NO
`FunctionConfig`. The macro runner uses `Registry::from_inventory()` (add-macro
`modal_runner.rs:16`), which also drops configs. The config data exists in
`from_inventory_with_configs()` (lib.rs:394, returns `(Registry, Vec<(&'static str,
FunctionConfig)>)`) but is not threaded into the runner.

**Resolution — additive config-carrying entry pair; `run_cli`/`run_cli_with_args` stay frozen:**

```rust
// crates/modal-rust-runtime/src/lib.rs — ADDITIVE. Frozen dispatch unchanged.

/// `run_cli` + a per-entrypoint config map for `--describe`. The configs are used
/// ONLY by `--describe`; the frozen `--entrypoint` dispatch ignores them.
pub fn run_cli_with_configs(
    registry: Registry,
    configs: &[(&'static str, FunctionConfig)],
) -> i32 {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    run_cli_with_args_and_configs(registry, configs, &argv, &mut std::io::stdout())
}

pub fn run_cli_with_args_and_configs<W: std::io::Write>(
    registry: Registry,
    configs: &[(&'static str, FunctionConfig)],
    argv: &[String],
    out: &mut W,
) -> i32 {
    if argv.first().map(String::as_str) == Some("--describe") {
        return emit_describe(&registry, configs, out); // exit 0 on success
    }
    run_cli_with_args(registry, argv, out) // FROZEN path, byte-identical
}
```

`run_cli`/`run_cli_with_args` keep their EXACT signatures and are reimplemented as zero-config
wrappers (`run_cli` ⇒ `run_cli_with_configs(registry, &[])`; `run_cli_with_args` body
unchanged, or `run_cli_with_args_and_configs(reg, &[], argv, out)` with the `--describe`
branch only firing on first-token match). With empty configs, `--describe` emits names +
default config — correct for the manual-registry path. The existing runtime tests
(lib.rs:732–855) keep calling `run_cli_with_args` and stay green; add ONE new test asserting
`--describe` returns 0 and emits the manifest (proving the entrypoint path is untouched).

> **Why this and not a `--describe` branch inside the single-arg `run_cli`:** the manual
> `Registry` carries no config, so a single-arg branch could only ever emit default config
> for the macro path too, silently dropping real gpu/timeout. The config-carrying pair makes
> the macro path emit REAL config while keeping `run_cli` frozen — the smallest change that
> is also correct. (One note proposed the single-arg branch as "cleanest minimal"; it is
> rejected because it cannot surface decorator config, which P9 must plumb.)

### A.2 Emission rules

- Triggered ONLY when the FIRST argv token is `--describe` (no other flags). It can never
  collide with the frozen `--entrypoint <name> --input-*` shape.
- Emits EXACTLY ONE JSON object to **stdout**, exit `0`. All diagnostics to stderr (mirrors
  the one-envelope discipline).
- Iterates `registry.names()` (sorted `BTreeMap` order, lib.rs:376) for the authoritative
  entrypoint set; for each name, looks up its `FunctionConfig` in the `configs` slice,
  falling back to `FunctionConfig::default()` (all-`None`) when absent.

### A.3 `--describe` JSON schema (frozen contract for the CLI consumer)

```json
{
  "schema": "modal-rust/describe@1",
  "entrypoints": [
    { "name": "add",        "config": { "gpu": null, "timeout_secs": null, "cache": null } },
    { "name": "vector_add", "config": { "gpu": "T4", "timeout_secs": 1800, "cache": false } }
  ]
}
```

- `schema`: version tag. The CLI warns-and-proceeds on an unknown minor; HARD-errors on an
  unknown major (forward-compat; matches boundaries.md §2 reserved `meta`/`version`).
- `entrypoints`: array, sorted by `name` (BTreeMap order — deterministic).
- `config` mirrors `FunctionConfig` (lib.rs:292–301) EXACTLY: `gpu: string|null`
  (`Option<&'static str>` ⇒ string or null), `timeout_secs: u32|null`, `cache: bool|null`.
- Implemented with a private `#[derive(Serialize)]` view in the runtime (`serde` is already a
  runtime dep): `DescribeManifest { schema, entrypoints: Vec<DescribeEntry { name, config }> }`.
  No new dependency.

### A.4 Runner-bin update (CLI-owned ~15-line template; FROZEN protocol untouched)

- **Macro path** (`examples/add-macro/src/bin/modal_runner.rs`): switch to the config-carrying
  entry so real decorator config flows into `--describe`:
  ```rust
  let (registry, configs) = modal_rust_runtime::from_inventory_with_configs();
  let code = modal_rust_runtime::run_cli_with_configs(registry, &configs);
  ```
  `from_inventory_with_configs` returns exactly the `&[(&'static str, FunctionConfig)]` the
  entry wants — zero conversion.
- **Manual path** (`examples/add`, `cuda-vector-add`, `burn-add`): may stay on
  `run_cli(modal_registry())` — empty configs ⇒ default config in `--describe`, which is
  correct (manual registry has no decorator). Optionally switch to
  `run_cli_with_configs(reg, &[])` for uniformity; behavior is identical. `examples/add` is
  the CLI test target; either form yields `add` with null config, exercising the plumbing.

This edits only the CLI-owned template body; it does NOT touch the FROZEN `--entrypoint`
protocol, `HandlerFn`, `typed!`, or dispatch.

---

## B. Facade: headless `App::from_manifest` (reuse shape (a))

### B.1 Chosen shape and why

**Shape (a): a headless `App`** — empty `Registry`, populated `configs` map — drives the entire
run/deploy/call triad by REUSING the existing `App` methods, with no new orchestration code.

Verified against the code: NONE of the remote ops dereference a `HandlerFn`.
- `App::remote_invoke` (app.rs:152) reads only `config_for(entrypoint)` + `ensure_function`.
- `Function::remote` (function.rs:72) only serializes input + calls `remote_invoke` + `parse_envelope`.
- `App::deploy_with` (app.rs:234) reads only `deploy_target_config()` + `deploy_function`.
- `App::call` (app.rs:267) resolves `from_name` + invokes + `parse_envelope`.
Only `.local()` (function.rs:42) needs a handler. So a headless `App` is sufficient for
`.remote()`/`deploy`/`call`.

> **Rejected: shape (b)** (a new `cli_backend` module duplicating `connect → op → parse`). It
> would re-expose `ensure_function`/`deploy_function`/`call_function`/`parse_envelope` (all
> `pub(crate)`) through a new module and re-implement the lifecycle the `App` methods already
> own. Shape (a) reuses those methods directly, so the only facade additions are constructors
> + two thin string-envelope accessors — a strictly smaller and more honest change.

### B.2 The critical wiring fix (config must be explicit, not `default()`)

`App::connect_inner` (app.rs:111–136) currently hardcodes `config: RemoteConfig::default()`
(line 133), and `RemoteConfig::default()` discovers `local_root`/`package` from CWD/env
(remote.rs:183–184, 199–228). The CLI runs from an arbitrary CWD and knows the real
`workspace_root` + `package`, so the headless connect MUST accept an explicit `RemoteConfig`
rather than the default (do NOT mutate `MODAL_RUST_SOURCE_DIR`/`MODAL_RUST_PACKAGE` env — the
explicit constructor is cleaner and side-effect free).

### B.3 Facade additions (`crates/modal-rust/src/app.rs`; `pub` from lib.rs already covers `App`)

```rust
/// Build a HEADLESS App from a `--describe` manifest: per-entrypoint config but NO
/// handlers (empty Registry). `.local()` would fail (no handler), but
/// `.remote()`/`deploy`/`call` never need handlers. Used by the `modal-rust` CLI,
/// which cannot link the user crate.
pub fn from_manifest(
    configs: impl IntoIterator<Item = (String, FunctionConfig)>,
) -> Self {
    App { registry: Registry::new(), configs: configs.into_iter().collect(), remote: None }
}

/// As `connect`, but seeds an empty Registry + the manifest configs + an EXPLICIT
/// RemoteConfig (built from the CLI's workspace_root + package), instead of
/// connect_inner's hardcoded RemoteConfig::default().
pub async fn connect_from_manifest(
    name: &str,
    configs: impl IntoIterator<Item = (String, FunctionConfig)>,
    run_config: RemoteConfig,
) -> Result<Self> {
    let mut client = modal_rust_sdk::ModalClient::connect().await?;
    let app_id = client.app_create_ephemeral(name, None).await?; // EPHEMERAL run-app, GC'd
    Ok(App {
        registry: Registry::new(),
        configs: configs.into_iter().collect(),
        remote: Some(RemoteHandle {
            client: Mutex::new(client),
            app_id,
            app_name: name.to_string(),
            function_id: OnceCell::new(),
            config: run_config, // EXPLICIT, not default()
        }),
    })
}

/// Run one entrypoint and return the runner's one-line JSON envelope VERBATIM (the
/// CLI prints it). Thin generic-free wrapper over the existing pub(crate) remote_invoke.
pub async fn remote_envelope(&self, entrypoint: &str, input_json: String) -> Result<String> {
    self.remote_invoke(entrypoint, input_json).await
}

/// Call a DEPLOYED entrypoint and return the envelope VERBATIM (no build, no upload).
/// Reuses deploy::call_function exactly as App::call does.
pub async fn call_envelope(&self, app_name: &str, entrypoint: &str, input_json: String)
    -> Result<String>
{
    let handle = self.remote.as_ref().ok_or_else(Error::not_connected)?;
    let mut client = handle.client.lock().await;
    crate::deploy::call_function(&mut client, app_name, entrypoint, input_json).await
}
```

Refactor note: factor `connect_inner` + `connect_from_manifest` over a shared body that takes
the explicit `RemoteConfig` (the only delta is the empty registry + supplied configs +
explicit config). The string-envelope accessors exist because `Function::remote`/`App::call`
are generic over typed `In/Out`; the CLI is generic over entrypoints and needs the raw
envelope to print byte-for-byte and mirror `ok` → exit code. `ensure_function`,
`deploy_function`, `call_function`, `parse_envelope` STAY `pub(crate)`.

---

## C. CLI: programmatic default path (`crates/modal-rust-cli`)

### C.1 Dependencies (`crates/modal-rust-cli/Cargo.toml`)

```toml
modal-rust = { path = "../modal-rust" }                 # App::from_manifest, RemoteConfig, DeployConfig
tokio      = { version = "1", features = ["rt-multi-thread", "macros"] }
```
`clap`/`serde_json`/`anyhow` stay. The facade pulls the SDK transitively (no direct SDK dep).
The async facade ops run under a tokio runtime (`#[tokio::main]` or `Runtime::new().block_on`);
`main()` still returns the `i32` exit code as today.

### C.2 The `--use-shim` flag and dispatch

Add a global `--use-shim: bool` (clap `#[arg(long)]`) to EACH of `Run`/`Deploy`/`Call`
(main.rs:60–94). `run()` (main.rs:139) branches per arm:
- `false` (DEFAULT) → `cmd_run_programmatic` / `cmd_deploy_programmatic` / `cmd_call_programmatic`.
- `true` → the EXISTING `cmd_run`/`cmd_deploy`/`cmd_call` bodies (main.rs:209–286), renamed
  `cmd_*_shim`, byte-for-byte UNCHANGED (they keep `write_shim`, `templates::*`, `run_modal`/
  `Command::new("modal")`).

### C.3 The build + describe helper (default path, replaces shim codegen)

A new helper used by `cmd_run_programmatic` / `cmd_deploy_programmatic`:

1. `cargo build --release -p <package> --bin modal_runner`, cwd = `workspace::workspace_root(project)`
   (workspace.rs:18), `<package>` = `workspace::package_name(project)` (workspace.rs:66) — the
   SAME `-p <pkg>` the shims used. `Command::new("cargo")` (NOT `modal`), inheriting stderr so
   the compile log streams. Binary lands at `<workspace_root>/target/release/modal_runner`.
   (This LOCAL build is for the manifest ONLY; the REMOTE build still happens in-body for `run`
   / at-image-build for `deploy` per the frozen build boundary — the CLI does NOT upload this
   local binary.)
2. Run `<workspace_root>/target/release/modal_runner --describe`, capture stdout, parse the
   §A.3 JSON via `serde_json` (already a dep) into `Manifest { schema, entrypoints: Vec<{ name:
   String, config: FunctionConfigView }> }`. HARD-error on `schema` major mismatch.
3. Look up the requested `entrypoint`; on miss, emit an unknown-entrypoint error listing the
   manifest's names (parity with `run_cli`'s diagnostic, lib.rs:612–620).

### C.4 Per-command flow

- **`run <e> --input <json>`** (EPHEMERAL app; mirrors `.remote()`):
  build + describe (C.3) → resolve entrypoint config → build `RemoteConfig`:
  `RemoteConfig { ..RemoteConfig::default(), local_root: workspace_root, package, gpu:
  config.gpu, timeout_override_secs: config.timeout_secs }` (the SAME fields
  `App::remote_invoke` sets at app.rs:166–177) → `App::connect_from_manifest(
  "modal-rust-cli-run", manifest_configs, remote_config)` →
  `app.remote_envelope(e, input_json)` → print the envelope to **stdout** VERBATIM, parse
  `{"ok":..}` and mirror it to exit 0/1.
- **`deploy <e> --app <name>`** (PERSISTENT; mirrors `App::deploy_with`):
  build + describe (C.3) → `DeployConfig { ..DeployConfig::for_app(app), local_root, package }`
  (deploy.rs) → `App::connect_from_manifest(...)` → `app.deploy_with(deploy_config)`. The
  CLI passes the full manifest configs; current `deploy_with` publishes one Modal function per
  entrypoint over a shared image, so each entrypoint's gpu/timeout/secrets/volumes ride on its
  own object tag. Print the `DeployedApp` summary to stderr (informational) and a success line
  to stdout. The CLI's selected entrypoint is validated for typo parity with `run`, but deploy
  publishes every described entrypoint.
- **`call <e> --app <name> --input <json>`** (`from_name` + invoke; NO build, NO upload):
  SKIP C.3 entirely (no per-call config; the deployed wrapper already carries its config) —
  matching the current shim `call` which passes empty package (main.rs:272). Build an empty
  headless app: `App::connect_from_manifest("modal-rust-cli-call", [], RemoteConfig::default())`
  → `app.call_envelope(app_name, e, input_json)` → print envelope VERBATIM, mirror exit code.
  This keeps `call` fast and build-free, preserving the deploy-call invariant.

`InputArg::resolve` (main.rs:107–125, default `{"a":40,"b":2}`, `@file` support) is reused
unchanged for both paths.

---

## D. `doctor` changes (`crates/modal-rust-cli/src/doctor.rs`)

The `modal` CLI is no longer a hard requirement on the default path; auth + the `--rust`
checks remain (the local `cargo build` for `--describe` makes cargo/rustc MORE load-bearing).

1. Add `--use-shim: bool` to the `Doctor` command (main.rs:52, alongside `rust`/`project`);
   thread into `doctor::run(with_rust, with_shim, project_dir)`.
2. Rebuild the check vector (doctor.rs:279):
   ```rust
   let mut checks = vec![check_modal_credentials()];      // auth ALWAYS (default path connects)
   if with_shim { checks.insert(0, check_modal_cli()); }   // modal CLI only under --use-shim
   if with_rust {
       checks.push(check_cargo());
       checks.push(check_rustc());
       checks.push(check_panic_profile(project_dir));      // panic=unwind still required
   }
   ```
   - KEEP `check_modal_credentials()` (doctor.rs:103) as a hard requirement — `App::connect_*`
     reads `~/.modal.toml` / `MODAL_TOKEN_*`.
   - KEEP `check_cargo`/`check_rustc`/`check_panic_profile` (doctor.rs:133/148/171) under `--rust`.
   - `check_modal_cli` (doctor.rs:76) is relevant ONLY under `--use-shim`.
3. Banner (doctor.rs:272–277): default header notes the modal CLI is not required
   (`(default path is programmatic; the modal CLI is required only with --use-shim)`); under
   `--use-shim`, note it is also checked. The `[ok]`/`[FAIL]` lines + the single-JSON-envelope
   failure shape (doctor.rs:33–43, 287–314) are UNCHANGED — only WHICH checks run differs.

**Behavioral delta:** on a machine with valid creds + cargo/rustc but NO `modal` CLI,
`modal-rust doctor` now PASSES (today it FAILs at `check_modal_cli`). `modal-rust doctor
--use-shim` on that machine still FAILs.

---

## E. `--use-shim` fallback (KEPT; P10 removes)

`templates.rs` + `src/templates/*.tmpl` (the `dev_app`/`deploy_app`/`call_app` renderers) and
`run_modal` + `Command::new("modal")` (main.rs:200–207) are KEPT intact, reachable ONLY from
the renamed `cmd_*_shim` functions under `--use-shim`. The byte-equivalence tests
(main.rs:328–411) stay green — they validate the path `--use-shim` still uses. Deleting them is
P10, explicitly NOT P9.

---

## F. Frozen-invariant compliance

- Runner `--entrypoint`/`--input-*` protocol, one-JSON-envelope output, five error kinds,
  `HandlerFn`, `typed!`, dispatch: **byte-identical**. `--describe` is a NEW first-token branch
  in a NEW config-carrying entry; `run_cli`/`run_cli_with_args` signatures + behavior preserved
  (reimplemented as `*_with_configs(reg, &[])`). Runtime tests (lib.rs:732–855) untouched.
- Run-vs-deploy build boundary, `retry_transient`, add_python image + cargo-scoped upload,
  ephemeral-run vs persistent-deploy lifecycle: **REUSED, not reimplemented** — the CLI drives
  `App::remote_envelope` (→ `ensure_function`, ephemeral publish, remote.rs:251),
  `App::deploy_with` (→ `deploy_function`, persistent publish, deploy.rs:234), and
  `App::call_envelope` (→ `call_function`, from_name, deploy.rs:349). The LOCAL `--describe`
  build is manifest-only and is NOT uploaded; the remote build path is unchanged.
- Container wrapper still execs `modal_runner --entrypoint <name> --input-file …` exactly as
  `.remote()`/deploy do (remote.rs WRAPPER_SRC, deploy.rs). No Python shim, no `modal`
  subprocess on the default path.

---

## G. Parity assertions / acceptance

### G.1 Same result, same envelope
`modal-rust run add --input '{"a":40,"b":2}'` (default) prints to stdout the SAME success
envelope the shim produced — `{"ok":true,"value":{"sum":42}}` — exit 0. Guaranteed: both paths
exec `modal_runner --entrypoint add --input-file …`, which emits the frozen envelope
(`run_cli_with_args`, lib.rs:637); the CLI prints runner stdout verbatim. Error parity (five
kinds) follows from the verbatim envelope (CLI mirrors `ok`; `App::*_envelope` returns the raw
string; typed callers would use `parse_envelope`, remote.rs:374).

**Offline HARD-gate tests:**
- `App::from_manifest([("add", FunctionConfig::default())])` builds an App whose
  `config_for("add")` is default and whose `known_names()` is empty (headless) — assert both.
- Round-trip a sample §A.3 manifest JSON line into `(name, FunctionConfig)` and assert
  gpu/timeout/cache.
- Runtime: `run_cli_with_args_and_configs(reg, &[("add", cfg)], &["--describe".into()], &mut buf)`
  returns 0 and emits a manifest containing `"add"` (additive; the five frozen tests stay green).

### G.2 NO generated `.py` (default path)
The programmatic functions never call `write_shim`/`generated_dir`/`templates::*`.
- Live: before a default `run`, `rm -rf <workspace_root>/.modal-rust/generated`; after, assert
  it is absent/empty (`test ! -e .modal-rust/generated || test -z "$(ls -A .modal-rust/generated)"`).
- Static: the `cmd_*_programmatic` functions contain no `templates::`/`write_shim` reference.

### G.3 NO `modal` subprocess (default path)
The only `Command::new("modal")` is `run_modal` (main.rs:202), reachable only from
`cmd_*_shim` (i.e. only under `--use-shim`). The programmatic path spawns `Command::new("cargo")`
(build) and `Command::new(<runner_bin>)` (`--describe`), never `modal`.
- Live: run with a PATH lacking `modal` (or a recording shim) — `modal-rust run add --input
  '{"a":40,"b":2}'` → `{sum:42}`, proving no `modal` dependency.
- Static: default-path code has no `run_modal` call.

### G.4 Live drive (best-effort; CPU only; retry on Modal flakiness)
- `run` → ephemeral app (no lingering deploy) → `{sum:42}`.
- `deploy` → STABLE name (`DeployConfig` default `modal-rust-add-deploy`, deploy.rs:46/160) so
  re-deploys REPLACE; `cargo build` appears in deploy/build logs.
- `call <stable-name> add --input '{"a":40,"b":2}'` → `{sum:42}`, no build, no upload.

---

## H. Files changed (implementation map)

| File | Change |
|---|---|
| `crates/modal-rust-runtime/src/lib.rs` | ADD `run_cli_with_configs` + `run_cli_with_args_and_configs` + private `emit_describe` + describe view structs; reimplement `run_cli`/`run_cli_with_args` as zero-config wrappers. Frozen entrypoint path + tests unchanged. |
| `examples/add-macro/src/bin/modal_runner.rs` | Switch to `from_inventory_with_configs()` + `run_cli_with_configs`. (Other runner bins optional, identical behavior.) |
| `crates/modal-rust/src/app.rs` | ADD `from_manifest`, `connect_from_manifest(name, configs, RemoteConfig)`, `remote_envelope`, `call_envelope`; factor a shared connect body taking an explicit `RemoteConfig`. Reuse `remote_invoke`/`deploy_with`/`deploy::call_function`. |
| `crates/modal-rust-cli/Cargo.toml` | ADD `modal-rust` (path) + `tokio` deps. |
| `crates/modal-rust-cli/src/main.rs` | ADD `--use-shim` to Run/Deploy/Call; rename existing bodies to `cmd_*_shim`; ADD `cmd_*_programmatic` + the build/`--describe`/manifest helper; dispatch branch in `run()`; tokio runtime in `main()`. |
| `crates/modal-rust-cli/src/doctor.rs` | ADD `--use-shim` to Doctor + `run()` signature; drop `check_modal_cli` from the default vector (behind `--use-shim`); keep auth + `--rust`; banner note. |
| KEPT (P10) | `crates/modal-rust-cli/src/templates.rs`, `src/templates/*.tmpl`, `run_modal` + `Command::new("modal")`, byte-equivalence tests — behind `--use-shim`. |

**Verification gate (offline HARD):** `cargo fmt --check` · `cargo clippy --all-targets -- -D
warnings` · `cargo build` · `cargo test` on default-members (no-CUDA CI green). Live CLI
run/deploy/call is best-effort (retry on Modal blips; ephemeral run apps; stable deploy name;
CPU only).
