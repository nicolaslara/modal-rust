//! Live, best-effort Modal auth round-trip.
//!
//! Gated behind BOTH the `live` cargo feature AND `#[ignore]` so the no-CUDA CI
//! box never runs it. Run locally with:
//!
//! ```text
//! cargo test -p modal-rust-sdk --features live -- --ignored --nocapture
//! ```
//!
//! It resolves credentials from the environment / `~/.modal.toml`, connects
//! (which performs the `ClientHello` handshake), and issues a cheap, safe
//! `AppGetOrCreate` for an ephemeral app — no cost, no GPU. Modal flakiness
//! ("socket connection closed unexpectedly", etc.) is transient capacity, so we
//! retry with brief backoff before giving up.

#![cfg(feature = "live")]

use std::time::Duration;

use modal_rust_sdk::{Error, ModalClient};

/// Treat tonic transport errors and a known set of transient gRPC statuses as
/// retryable capacity blips (per project verification rules).
fn is_transient(err: &Error) -> bool {
    match err {
        Error::Transport(_) => true,
        Error::Status(status) => {
            use tonic::Code::*;
            if matches!(
                status.code(),
                Unavailable | DeadlineExceeded | Aborted | ResourceExhausted
            ) {
                return true;
            }
            let msg = status.message().to_ascii_lowercase();
            msg.contains("socket connection closed")
                || msg.contains("connection reset")
                || msg.contains("transport")
        }
        _ => false,
    }
}

#[tokio::test]
#[ignore = "live Modal round-trip; run with --features live -- --ignored"]
async fn auth_round_trip_app_get_or_create() {
    let attempts = 4;
    let mut last_err: Option<Error> = None;

    for attempt in 1..=attempts {
        match round_trip().await {
            Ok(app_id) => {
                println!("LIVE OK: AppGetOrCreate returned app_id = {app_id}");
                assert!(!app_id.is_empty(), "app_id should be non-empty");
                return;
            }
            Err(err) => {
                eprintln!("attempt {attempt}/{attempts} failed: {err}");
                if !is_transient(&err) || attempt == attempts {
                    last_err = Some(err);
                    break;
                }
                last_err = Some(err);
                tokio::time::sleep(Duration::from_secs(2 * attempt as u64)).await;
            }
        }
    }

    panic!(
        "live auth round-trip failed after {attempts} attempts: {}",
        last_err.expect("an error was recorded")
    );
}

async fn round_trip() -> Result<String, Error> {
    let mut client = ModalClient::connect().await?;
    // Unique-ish ephemeral app name; AppGetOrCreate is idempotent + free.
    let name = format!(
        "modal-rust-sdk-auth-probe-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );
    client.app_get_or_create(&name, None).await
}
