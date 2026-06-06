//! `examples/secrets` — attach a named Modal secret, read it as an env var.
//!
//! Teaching ONE concept: name a Modal secret on the decorator
//! (`#[modal_rust::function(secrets = ["my-api-key"])]`) and read its keys inside
//! the function with `std::env::var(...)`. The decorator IS the config — at run /
//! deploy time the facade resolves the named secret and rides its id into the
//! `FunctionCreate` manifest, so the secret's keys land as container env vars. The
//! function body never names Modal; it just reads `std::env::var`, exactly like any
//! Rust program reading its environment.
//!
//! A Modal secret named `my-api-key` is a deployed dict of `KEY=VALUE` pairs (e.g.
//! `modal secret create my-api-key MY_API_KEY=sk-...`); each key becomes an env var
//! in the container. Here we read `MY_API_KEY`.
//!
//! `src/bin/modal_runner.rs` is the one-line runner; `tests/manifest.rs` proves
//! OFFLINE (no live Modal) that the named secret rides into the planned
//! `FunctionCreate` manifest.

use modal_rust::function;
use serde::{Deserialize, Serialize};

/// The env-var key carried by the `my-api-key` secret (Modal injects each of the
/// secret's keys as a container env var).
const API_KEY_VAR: &str = "MY_API_KEY";

/// Input for [`check_secret`] — nothing the caller supplies; the secret arrives via
/// the environment, not the wire.
#[derive(Debug, Serialize, Deserialize)]
pub struct Request {}

/// Output for [`check_secret`].
#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    /// `true` iff the `MY_API_KEY` env var was present (the attached secret was
    /// injected). The KEY VALUE itself is never returned — only that it was found,
    /// plus its length, so the proof never leaks the secret.
    pub present: bool,
    /// The length of the secret value, so the envelope carries proof it was read
    /// without echoing the value.
    pub len: usize,
}

/// Read the attached secret from the environment. The decorator names the Modal
/// secret `my-api-key`; the facade resolves it and attaches it to the function, so
/// its keys (here `MY_API_KEY`) are present as env vars when this body runs. We read
/// it with plain `std::env::var` and report only its presence + length.
#[function(secrets = ["my-api-key"])]
pub fn check_secret(_req: Request) -> anyhow::Result<Report> {
    match std::env::var(API_KEY_VAR) {
        Ok(value) => Ok(Report {
            present: true,
            len: value.len(),
        }),
        Err(_) => Ok(Report {
            present: false,
            len: 0,
        }),
    }
}
