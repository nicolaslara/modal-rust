//! The macro-path twin of `examples/add` (ergonomics E1).
//!
//! This crate proves the macro-compatibility invariant (boundaries.md §3): the
//! `#[modal_rust::function]` attribute is **pure additive sugar** that compiles
//! down to the SAME monomorphized `typed!` wrapper `fn` pointer and the SAME
//! `Registry` / `HandlerFn` shape as the manual `examples/add`. There is no
//! `modal_registry()` builder here — the runner binary calls
//! `Registry::from_inventory()`, which collects the macro's `inventory::submit!`
//! registration into the identical `BTreeMap<&'static str, HandlerFn>`.
//!
//! Driven by the **unchanged** `run_cli`, the macro-registered `add` produces
//! byte-identical output to the manual path:
//! `modal_runner --entrypoint add --input-json '{"a":40,"b":2}'`
//! prints exactly `{"ok":true,"value":{"sum":42}}` and exits 0.

// Alias the proc-macro crate so the attribute is spelled `#[modal_rust::function]`
// exactly as boundaries.md §3 / the ergonomics tasks specify. The macro's
// generated code references the runtime and `inventory` by their real crate names
// (`::modal_rust_runtime`, `::inventory`), independent of this alias.
extern crate modal_rust_macros as modal_rust;

use serde::{Deserialize, Serialize};

/// The single named-JSON-object input for `add` (boundaries.md §3: never a
/// positional array). Mirrors `examples/add::AddInput`.
#[derive(Debug, Deserialize)]
pub struct AddInput {
    /// First addend.
    pub a: i64,
    /// Second addend.
    pub b: i64,
}

/// The output of `add`. Mirrors `examples/add::AddOutput`.
#[derive(Debug, Serialize)]
pub struct AddOutput {
    /// `a + b`.
    pub sum: i64,
}

/// Add two integers — the macro-registered entrypoint.
///
/// `#[modal_rust::function]` expands to this unchanged fn PLUS an
/// `inventory::submit!` of a `Registration { name: "add", handler:
/// modal_rust_runtime::typed!(add) }`. The name defaults to the fn name (`add`);
/// `#[modal_rust::function(name = "...")]` would override it. The handler is the
/// SAME monomorphized `typed!` wrapper the manual `examples/add` registers by
/// hand, so the runner protocol and envelope are identical.
#[modal_rust::function]
pub fn add(input: AddInput) -> anyhow::Result<AddOutput> {
    Ok(AddOutput {
        sum: input.a + input.b,
    })
}

/// The macro-path twin WITH PER-FUNCTION CONFIG (P4): the
/// `#[modal_rust::function(gpu=…, timeout=…, cache=…)]` decorator records a
/// [`modal_rust_runtime::FunctionConfig`] alongside the registration. This is
/// METADATA ONLY — the emitted handler and the runner dispatch are byte-identical
/// to the bare path; only the facade reads the config when CREATING the Modal
/// function. The compute is the same `a + b`, proving the config is additive sugar.
#[modal_rust::function(gpu = "T4", timeout = 1800, cache = false)]
pub fn add_gpu(input: AddInput) -> anyhow::Result<AddOutput> {
    Ok(AddOutput {
        sum: input.a + input.b,
    })
}

/// The macro-path twin WITH USER-FACING SECRETS + VOLUMES: the
/// `#[modal_rust::function(secrets = [..], volumes = [..])]` decorator records the
/// named secrets + the `(mount_path, name)` volume pairs onto the
/// [`modal_rust_runtime::FunctionConfig`]. METADATA ONLY — the facade resolves the
/// secrets to ids (injected as ENV VARS) and the volumes to mounts; the emitted
/// handler + runner dispatch stay byte-identical to the bare path. The user volume
/// is a SEPARATE mount from the P6 cargo cache (`/cache`), so both coexist.
#[modal_rust::function(secrets = ["my-secret"], volumes = ["/data=my-vol"])]
pub fn add_extras(input: AddInput) -> anyhow::Result<AddOutput> {
    Ok(AddOutput {
        sum: input.a + input.b,
    })
}

/// Input for [`secret_vol_probe`] — the REAL live secrets+volumes proof body.
#[derive(Debug, Serialize, Deserialize)]
pub struct ProbeInput {
    /// The env-var name to read (the attached secret's key). The live test passes
    /// `MODAL_RUST_TEST_SECRET`; reading it via `std::env` proves Modal injected the
    /// secret's key/values as container ENV VARS.
    pub secret_key: String,
    /// Absolute path of the marker file on the mounted user volume (e.g.
    /// `/data/marker`). Distinct from the cargo-cache mount (`/cache`).
    pub marker_path: String,
    /// When `Some`, WRITE this value to `marker_path` (first call). When `None`, only
    /// READ it back (second call) — proving the volume persisted across calls.
    pub write_value: Option<String>,
}

/// Output of [`secret_vol_probe`].
#[derive(Debug, Serialize, Deserialize)]
pub struct ProbeOutput {
    /// The value read from the secret env var (`std::env::var(secret_key)`), or `None`
    /// if it was unset — the secret-injection proof.
    pub secret_value: Option<String>,
    /// The contents read back from `marker_path` after the (optional) write, or `None`
    /// if the file does not exist — the volume-persistence proof.
    pub marker_read: Option<String>,
    /// `true` iff this call wrote the marker file.
    pub wrote: bool,
}

/// The REAL user-facing secrets+volumes proof body — runs REMOTELY on Modal.
///
/// This is the entrypoint the live test uploads + builds in the function body. It:
///   1. reads `std::env::var(secret_key)` — proves the attached Modal secret's
///      key/values were injected as container ENV VARS (readable by the user fn);
///   2. optionally WRITES `write_value` to `marker_path` on the mounted user volume,
///      then READS it back — proving the volume is real persistent storage,
///      committed across calls (a second call in a fresh container reads it back).
///
/// The mount path (`/data`) is DISTINCT from the P6 cargo-cache mount (`/cache`), so
/// the user volume and the cache volume coexist on the same function. NO decorator
/// here: the live test binary carries the decorated stub that attaches the (uniquely
/// named, programmatically created) secret + volume. The body never logs the value.
#[modal_rust::function]
pub fn secret_vol_probe(input: ProbeInput) -> anyhow::Result<ProbeOutput> {
    use std::path::Path;

    // (a) Read the secret env var Modal injected from the attached Secret.
    let secret_value = std::env::var(&input.secret_key).ok();

    // (b) Volume IO at the mounted user-volume path (distinct from /cache).
    let marker_path = Path::new(&input.marker_path);
    let mut wrote = false;
    if let Some(value) = &input.write_value {
        if let Some(parent) = marker_path.parent() {
            // The volume mount root already exists; this is a no-op if so.
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(marker_path, value)?;
        wrote = true;
    }
    // Always try to read it back (proves persistence on the second, write-less call).
    let marker_read = std::fs::read_to_string(marker_path).ok();

    Ok(ProbeOutput {
        secret_value,
        marker_read,
        wrote,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use modal_rust_runtime::{FunctionConfig, Registration, Registry};

    /// Look up a `Registration` by entrypoint name from the inventory pass.
    fn registration(name: &str) -> Option<&'static Registration> {
        inventory::iter::<Registration>
            .into_iter()
            .find(|r| r.name == name)
    }

    #[test]
    fn add_works() {
        let out = add(AddInput { a: 40, b: 2 }).unwrap();
        assert_eq!(out.sum, 42);
    }

    #[test]
    fn from_inventory_registers_add() {
        // The macro's `inventory::submit!` must surface `add` through
        // `Registry::from_inventory()` — the same lookup the manual builder gives.
        let reg = Registry::from_inventory();
        assert!(reg.get("add").is_some(), "macro did not register `add`");
        assert!(reg.get("nope").is_none());
    }

    #[test]
    fn macro_path_byte_identical_to_manual() {
        // Drive the UNCHANGED run_cli with the macro-built registry and assert the
        // success envelope is byte-for-byte the manual-path output.
        let argv: Vec<String> = ["--entrypoint", "add", "--input-json", r#"{"a":40,"b":2}"#]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut buf = Vec::new();
        let code =
            modal_rust_runtime::run_cli_with_args(Registry::from_inventory(), &argv, &mut buf);
        assert_eq!(code, 0);
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "{\"ok\":true,\"value\":{\"sum\":42}}\n"
        );
    }

    #[test]
    fn unknown_entrypoint_still_works() {
        // An error kind on the macro-built runner behaves identically to manual.
        let argv: Vec<String> = ["--entrypoint", "nope", "--input-json", "{}"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut buf = Vec::new();
        let code =
            modal_rust_runtime::run_cli_with_args(Registry::from_inventory(), &argv, &mut buf);
        assert_eq!(code, 1);
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["kind"], "unknown_entrypoint");
        assert_eq!(v["error"]["details"], serde_json::Value::Null);
    }

    #[test]
    fn bare_macro_config_is_default() {
        // P4 backward-compat proof: the BARE `#[modal_rust::function]` records
        // `FunctionConfig::default()` (all `None`) — runtime-observable behavior is
        // byte-identical (same name, same handler, same `{sum:42}`; runner ignores
        // config). The `macro_path_byte_identical_to_manual` test above proves the
        // envelope is unchanged; this asserts the recorded config is the default.
        let reg = registration("add").expect("macro must register `add`");
        assert_eq!(reg.config, FunctionConfig::default());
        assert_eq!(reg.config.gpu, None);
        assert_eq!(reg.config.timeout_secs, None);
        assert_eq!(reg.config.cache, None);
        // The bare macro also records EMPTY secrets/volumes (byte-identical config).
        assert!(reg.config.secrets.is_empty());
        assert!(reg.config.volumes.is_empty());
    }

    #[test]
    fn secrets_and_volumes_decorator_parses_into_config() {
        // `#[modal_rust::function(secrets = ["my-secret"], volumes = ["/data=my-vol"])]`
        // records the named secret + the (mount_path, name) volume pair.
        let reg = registration("add_extras").expect("macro must register `add_extras`");
        assert_eq!(reg.config.secrets, &["my-secret"]);
        assert_eq!(reg.config.volumes, &[("/data", "my-vol")]);
        // gpu/timeout/cache stay unset (only secrets/volumes were given).
        assert_eq!(reg.config.gpu, None);
        assert_eq!(reg.config.timeout_secs, None);
        assert_eq!(reg.config.cache, None);
        // The handler still dispatches the same compute through the unchanged runner.
        let argv: Vec<String> = [
            "--entrypoint",
            "add_extras",
            "--input-json",
            r#"{"a":40,"b":2}"#,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut buf = Vec::new();
        let code =
            modal_rust_runtime::run_cli_with_args(Registry::from_inventory(), &argv, &mut buf);
        assert_eq!(code, 0);
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "{\"ok\":true,\"value\":{\"sum\":42}}\n"
        );
    }

    #[test]
    fn user_volume_and_cache_mount_paths_are_distinct() {
        // The user volume mounts at `/data`; the P6 cargo cache mounts at `/cache`.
        // They are DISTINCT mount paths, so both coexist on the same function — the
        // decorator records ONLY the user volume (the cache volume is facade-internal).
        let reg = registration("add_extras").expect("macro must register `add_extras`");
        for (mount, _name) in reg.config.volumes {
            assert_ne!(*mount, "/cache", "user volume must not collide with cache");
        }
    }

    #[test]
    fn configured_macro_populates_function_config() {
        // P4: `#[modal_rust::function(gpu="T4", timeout=1800, cache=false)]` records
        // the parsed config into the inventory registration.
        let reg = registration("add_gpu").expect("macro must register `add_gpu`");
        assert_eq!(reg.config.gpu, Some("T4"));
        assert_eq!(reg.config.timeout_secs, Some(1800));
        assert_eq!(reg.config.cache, Some(false));
        // The handler still dispatches the same compute through the unchanged runner.
        let argv: Vec<String> = [
            "--entrypoint",
            "add_gpu",
            "--input-json",
            r#"{"a":40,"b":2}"#,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut buf = Vec::new();
        let code =
            modal_rust_runtime::run_cli_with_args(Registry::from_inventory(), &argv, &mut buf);
        assert_eq!(code, 0);
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "{\"ok\":true,\"value\":{\"sum\":42}}\n"
        );
    }

    #[test]
    fn secret_vol_probe_reads_env_and_does_volume_io() {
        // The REAL live-proof body exercised offline: it reads an env var (the
        // "secret") and writes+reads a marker file (the "volume"). Uses a unique
        // env key + a temp path so it never collides with the real container env.
        let key = "MODAL_RUST_PROBE_UNITTEST_SECRET";
        std::env::set_var(key, "hello-unit");
        let dir = std::env::temp_dir().join("modal_rust_probe_unittest");
        let _ = std::fs::remove_dir_all(&dir);
        let marker = dir.join("marker");
        let marker_path = marker.to_string_lossy().to_string();

        // First call: writes the marker, returns the secret value + the read-back.
        let first = secret_vol_probe(ProbeInput {
            secret_key: key.to_string(),
            marker_path: marker_path.clone(),
            write_value: Some("persisted-value".to_string()),
        })
        .unwrap();
        assert_eq!(first.secret_value.as_deref(), Some("hello-unit"));
        assert!(first.wrote);
        assert_eq!(first.marker_read.as_deref(), Some("persisted-value"));

        // Second call: write_value=None => read-only; the marker persists on disk.
        let second = secret_vol_probe(ProbeInput {
            secret_key: key.to_string(),
            marker_path: marker_path.clone(),
            write_value: None,
        })
        .unwrap();
        assert!(!second.wrote);
        assert_eq!(second.marker_read.as_deref(), Some("persisted-value"));

        std::env::remove_var(key);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
