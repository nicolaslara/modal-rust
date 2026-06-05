//! Proof entrypoints kept out of the headline `lib.rs`.
//!
//! These exist so the macro's decorator-config (`gpu`/`timeout`/`cache`/`secrets`/
//! `volumes`) and the live secrets+volumes body keep compiling AND keep their
//! `inventory` registration (the `modal-rust` crate's `App` tests assert
//! `config_for("add_gpu")` / `config_for("add_extras")`, and the live
//! `secret_vol_probe` runner entrypoint is built from here). They are NOT part of
//! the ergonomic showcase — `add` in `lib.rs` is.

use modal_rust::function;
use serde::{Deserialize, Serialize};

/// A single-struct input, mirroring `examples/add::AddInput`. Used by the Mode-A
/// (explicit-struct) decorator-config entrypoints below.
#[derive(Debug, Deserialize)]
pub struct AddInput {
    /// First addend.
    pub a: i64,
    /// Second addend.
    pub b: i64,
}

/// The output of the struct-form entrypoints. Mirrors `examples/add::AddOutput`.
#[derive(Debug, Serialize)]
pub struct AddOutput {
    /// `a + b`.
    pub sum: i64,
}

/// Per-function CONFIG demo (P4): `#[modal_rust::function(gpu=…, timeout=…,
/// cache=…)]` records a `FunctionConfig` alongside the registration. METADATA
/// ONLY — the emitted handler and runner dispatch are byte-identical to the bare
/// path; the facade reads the config when creating the Modal function.
#[function(gpu = "T4", timeout = 1800, cache = false)]
pub fn add_gpu(input: AddInput) -> anyhow::Result<AddOutput> {
    Ok(AddOutput {
        sum: input.a + input.b,
    })
}

/// User-facing SECRETS + VOLUMES demo: the decorator records the named secrets +
/// the `(mount_path, name)` volume pairs onto the `FunctionConfig`. METADATA ONLY.
/// The user volume (`/data`) is a SEPARATE mount from the P6 cargo cache (`/cache`).
#[function(secrets = ["my-secret"], volumes = ["/data=my-vol"])]
pub fn add_extras(input: AddInput) -> anyhow::Result<AddOutput> {
    Ok(AddOutput {
        sum: input.a + input.b,
    })
}

/// Input for [`secret_vol_probe`] — the REAL live secrets+volumes proof body.
#[derive(Debug, Serialize, Deserialize)]
pub struct ProbeInput {
    /// The env-var name to read (the attached secret's key).
    pub secret_key: String,
    /// Absolute path of the marker file on the mounted user volume.
    pub marker_path: String,
    /// When `Some`, WRITE this value to `marker_path` (first call); when `None`,
    /// only READ it back (second call) — proving the volume persisted across calls.
    pub write_value: Option<String>,
}

/// Output of [`secret_vol_probe`].
#[derive(Debug, Serialize, Deserialize)]
pub struct ProbeOutput {
    /// The value read from the secret env var, or `None` if unset.
    pub secret_value: Option<String>,
    /// The contents read back from `marker_path`, or `None` if absent.
    pub marker_read: Option<String>,
    /// `true` iff this call wrote the marker file.
    pub wrote: bool,
}

/// The REAL user-facing secrets+volumes proof body — runs REMOTELY on Modal.
///
/// Reads `std::env::var(secret_key)` (proves the attached secret was injected as a
/// container ENV VAR), optionally writes `write_value` to `marker_path` on the
/// mounted user volume, then reads it back (proves the volume persists). The live
/// test (`crates/modal-rust/tests/live_secrets_volumes.rs`) uploads this package so
/// the remote runner registers this entrypoint; the decorated stub that attaches the
/// real secret + volume lives in that test binary.
#[function]
pub fn secret_vol_probe(input: ProbeInput) -> anyhow::Result<ProbeOutput> {
    use std::path::Path;

    let secret_value = std::env::var(&input.secret_key).ok();

    let marker_path = Path::new(&input.marker_path);
    let mut wrote = false;
    if let Some(value) = &input.write_value {
        if let Some(parent) = marker_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(marker_path, value)?;
        wrote = true;
    }
    let marker_read = std::fs::read_to_string(marker_path).ok();

    Ok(ProbeOutput {
        secret_value,
        marker_read,
        wrote,
    })
}
