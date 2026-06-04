//! Live, best-effort USER-FACING SECRETS + VOLUMES proof — the payoff for the
//! `#[modal_rust::function(secrets = [..], volumes = [..])]` decorator path.
//!
//! Drives the facade end-to-end against REAL Modal so the decorator-sourced secret
//! and user-volume config ride into `FunctionCreate`, and proves both work in the
//! container body. The flow:
//!
//! Step 1 — the test PROVISIONS its own resources programmatically (NO manual
//! setup): a Secret via `ModalClient::secret_from_dict({MODAL_RUST_TEST_SECRET:
//! "hello-secrets"})` and a user Volume via `ModalClient::volume_get_or_create`
//! (both idempotent / CREATE_IF_MISSING).
//!
//! Step 2 — a DECORATED STUB in this test binary,
//! `#[modal_rust::function(secrets=[SECRET_NAME], volumes=["/data=VOL_NAME"])]`,
//! records the `FunctionConfig` under the entrypoint name `secret_vol_probe`, so
//! `App::connect` (inventory) threads the secret_id + volume_mount into the outbound
//! `FunctionCreate` (the cache volume `/cache` coexists separately).
//!
//! Step 3 — the REAL body that runs remotely is `example_add_macro::secret_vol_probe`
//! (uploaded + built in the function body via `MODAL_RUST_PACKAGE`). It reads
//! `std::env::var("MODAL_RUST_TEST_SECRET")` (proving the secret's key/values were
//! injected as container ENV VARS), then WRITES a unique marker to `/data/marker` on
//! the mounted user volume on the first call; a SECOND call (a fresh ephemeral app
//! => a fresh container) READS it back, proving the volume is real persistent
//! storage committed across calls.
//!
//! The decoded `secret_value == "hello-secrets"` and `marker_read == <this run's
//! unique value>` ARE the server-side proof: a CPU container with no secret attached
//! could not return the value, and a fresh container with no persistent volume could
//! not read back the marker the first container wrote.
//!
//! Gated behind BOTH the `live` cargo feature AND `#[ignore]` so the no-CUDA CI box
//! never runs it. Run locally with:
//!
//! ```text
//! cargo test -p modal-rust --features live --test live_secrets_volumes \
//!     -- --ignored --nocapture
//! ```
//!
//! Uses a CHEAP CPU function and EPHEMERAL apps (the run path), so it leaves no
//! persistent deploy. The provisioned secret + volume are tiny and idempotent; the
//! marker value is unique per run so re-runs never collide. Modal flakiness
//! ("socket connection closed unexpectedly", build capacity blips) is transient —
//! retry, never block. The hard gates are the offline compiles.

#![cfg(feature = "live")]

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use modal_rust::sdk::ModalClient;
use modal_rust::{App, Error};
use serde::{Deserialize, Serialize};

/// FIRST ephemeral app: writes the marker (a fresh container builds + writes).
const APP_WRITE: &str = "modal-rust-live-secvol-write";
/// SECOND ephemeral app: a SEPARATE app => a fresh container, so warm-container
/// reuse can't mask the persistence proof; it READS the marker back.
const APP_READ: &str = "modal-rust-live-secvol-read";
/// The package whose runner registers the REAL `secret_vol_probe` body.
const PACKAGE: &str = "example-add-macro";

/// Deployment name of the programmatically-created Secret (matches the decorator
/// stub below). The injected env var key is [`SECRET_KEY`].
const SECRET_NAME: &str = "modal-rust-test-secret";
/// The secret's single key (injected as a container ENV VAR) — what the body reads.
const SECRET_KEY: &str = "MODAL_RUST_TEST_SECRET";
/// The secret's value — what the body must return (the secret-injection proof).
const SECRET_VALUE: &str = "hello-secrets";
/// Deployment name of the programmatically-created user Volume (matches the stub).
const VOL_NAME: &str = "modal-rust-test-vol";
/// The user-volume mount path in the container — DISTINCT from the cache `/cache`.
const MOUNT_PATH: &str = "/data";
/// The marker file on the user volume (the volume-persistence proof).
const MARKER_PATH: &str = "/data/marker";

/// Input for the remote `secret_vol_probe` (mirrors
/// `example_add_macro::ProbeInput`). Derives both serde traits so the `.remote()`
/// serialize path AND the decorated stub's `typed!` wrapper type-check.
#[derive(Debug, Serialize, Deserialize)]
struct ProbeInput {
    secret_key: String,
    marker_path: String,
    write_value: Option<String>,
}

/// Decoded output of the remote `secret_vol_probe` (mirrors
/// `example_add_macro::ProbeOutput`).
#[derive(Debug, Serialize, Deserialize)]
struct ProbeOutput {
    secret_value: Option<String>,
    marker_read: Option<String>,
    wrote: bool,
}

/// The DECORATED STUB: its only job is to record `FunctionConfig { secrets:
/// [SECRET_NAME], volumes: [("/data", VOL_NAME)] }` under the entrypoint name
/// `secret_vol_probe` into THIS test binary's inventory, so the facade attaches the
/// resolved secret_id + the user-volume mount on the outbound `FunctionCreate`. It
/// is never executed remotely (the uploaded `example-add-macro` runner runs the real
/// body), so its own body just errors. The literal names MUST match the constants
/// the test provisions above. The mount path `/data` is distinct from `/cache`, so
/// the user volume and the P6 cargo cache coexist.
#[modal_rust::function(
    secrets = ["modal-rust-test-secret"],
    volumes = ["/data=modal-rust-test-vol"],
    name = "secret_vol_probe"
)]
fn secret_vol_probe_stub(_input: ProbeInput) -> Result<ProbeOutput, String> {
    Err("local stub: secret_vol_probe runs on Modal, not in-process".to_string())
}

/// Treat transport blips and known transient gRPC messages as retryable. Delegates
/// to the SDK's own classifier so the test and the SDK agree on what is transient.
fn is_transient(err: &Error) -> bool {
    match err {
        Error::Sdk(sdk_err) => sdk_err.is_transient(),
        _ => false,
    }
}

/// Provision the Secret + Volume the decorator references — idempotent, no manual
/// setup. Retries transient transport errors. Logs the IDs (never the value).
async fn provision() -> Result<(), Error> {
    let attempts = 4u32;
    let mut last: Option<Error> = None;
    for attempt in 1..=attempts {
        let res = async {
            let mut client = ModalClient::connect().await?;
            let mut env = HashMap::new();
            env.insert(SECRET_KEY.to_string(), SECRET_VALUE.to_string());
            let secret_id = client.secret_from_dict(SECRET_NAME, &env, None).await?;
            // User volume: V1 (general-purpose persistent storage), create-if-missing
            // — exactly what the facade resolves for `#[function(volumes=..)]`.
            let volume_id = client
                .volume_get_or_create(VOL_NAME, false, true, None)
                .await?;
            println!(
                "[provision] secret '{SECRET_NAME}' -> {secret_id} (key {SECRET_KEY}, value redacted); \
                 volume '{VOL_NAME}' -> {volume_id} (mount {MOUNT_PATH})"
            );
            Ok::<(), Error>(())
        }
        .await;
        match res {
            Ok(()) => return Ok(()),
            Err(err) => {
                eprintln!("[provision] attempt {attempt}/{attempts} failed: {err}");
                let transient = is_transient(&err);
                last = Some(err);
                if !transient || attempt == attempts {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(3 * attempt as u64)).await;
            }
        }
    }
    Err(last.expect("an error was recorded"))
}

/// One `.remote("secret_vol_probe", input)` round-trip on `app_name`, retrying only
/// transient errors. `App::connect` reads THIS binary's inventory, so the decorated
/// stub's secrets/volumes config rides into the create.
async fn probe(label: &str, app_name: &str, input: ProbeInput) -> ProbeOutput {
    let attempts = 4u32;
    let mut last: Option<Error> = None;
    for attempt in 1..=attempts {
        let res = async {
            let app = App::connect(app_name).await?;
            app.function("secret_vol_probe")
                .remote(ProbeInput {
                    secret_key: input.secret_key.clone(),
                    marker_path: input.marker_path.clone(),
                    write_value: input.write_value.clone(),
                })
                .await
        }
        .await;
        match res {
            Ok(out) => {
                println!("[{label}] secret_vol_probe -> {out:?}");
                return out;
            }
            Err(err) => {
                eprintln!("[{label}] attempt {attempt}/{attempts} failed: {err}");
                let transient = is_transient(&err);
                last = Some(err);
                if !transient || attempt == attempts {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(3 * attempt as u64)).await;
            }
        }
    }
    panic!(
        "[{label}] live secret_vol_probe failed after {attempts} attempts: {}",
        last.expect("an error was recorded")
    );
}

#[tokio::test]
#[ignore = "live Modal secrets+volumes round-trip; run with --features live -- --ignored"]
async fn secret_injected_and_user_volume_persists() {
    // The facade uploads/builds the package named by MODAL_RUST_PACKAGE; point it at
    // example-add-macro so the remote runner registers the REAL `secret_vol_probe`.
    std::env::set_var("MODAL_RUST_PACKAGE", PACKAGE);

    // 0. Provision the secret + volume programmatically (idempotent; no manual setup).
    provision()
        .await
        .expect("provision secret + volume (transient errors retried)");

    // A unique marker value per run so a leftover marker from a prior run cannot
    // produce a false positive — the read-back must equal THIS run's value.
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let marker_value = format!("persisted-{nonce}");

    // 1. WRITE call (first container): reads the secret env var AND writes the marker
    //    to the mounted user volume. Proves (a) secret injection + the write half of
    //    (b) volume persistence.
    let write_out = probe(
        "WRITE",
        APP_WRITE,
        ProbeInput {
            secret_key: SECRET_KEY.to_string(),
            marker_path: MARKER_PATH.to_string(),
            write_value: Some(marker_value.clone()),
        },
    )
    .await;
    assert_eq!(
        write_out.secret_value.as_deref(),
        Some(SECRET_VALUE),
        "the attached secret '{SECRET_NAME}' must be injected as env var {SECRET_KEY} \
         and read by the fn (got {:?})",
        write_out.secret_value
    );
    assert!(
        write_out.wrote,
        "the write call must have written the marker"
    );
    assert_eq!(
        write_out.marker_read.as_deref(),
        Some(marker_value.as_str()),
        "the write call must read back exactly what it wrote to {MARKER_PATH}"
    );

    // 2. READ call (a SEPARATE ephemeral app => a fresh container): write_value=None
    //    so it only READS the marker back. If the value persisted, the user volume is
    //    real persistent storage committed across calls (the persistence half of (b)).
    let read_out = probe(
        "READ",
        APP_READ,
        ProbeInput {
            secret_key: SECRET_KEY.to_string(),
            marker_path: MARKER_PATH.to_string(),
            write_value: None,
        },
    )
    .await;
    assert!(
        !read_out.wrote,
        "the read call must NOT write (write_value=None)"
    );
    assert_eq!(
        read_out.marker_read.as_deref(),
        Some(marker_value.as_str()),
        "the user volume must PERSIST the marker across calls (fresh container) — \
         read back {:?}, expected {marker_value:?}",
        read_out.marker_read
    );
    // The secret is also injected into the second (fresh) container — confirms the
    // attach is per-function, not a one-off of the first container.
    assert_eq!(
        read_out.secret_value.as_deref(),
        Some(SECRET_VALUE),
        "the secret must be injected into the fresh container too"
    );

    println!(
        "LIVE OK: secret env injected (read {SECRET_KEY}={:?} in-fn) + user volume \
         persisted across calls (wrote then re-read {marker_value:?} from {MARKER_PATH}, \
         a SEPARATE container) — cargo cache (/cache) + user volume ({MOUNT_PATH}) coexist",
        write_out.secret_value
    );
}
