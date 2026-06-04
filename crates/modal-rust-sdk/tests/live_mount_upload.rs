//! Live, best-effort local-directory UPLOAD round-trip — proves `mount_local_dir`
//! walks a real directory, hashes + uploads its files, and finalizes an ephemeral
//! Modal mount with a non-empty `mount_id`.
//!
//! Gated behind BOTH the `live` cargo feature AND `#[ignore]` so the no-CUDA CI
//! box never runs it. Run locally with:
//!
//! ```text
//! cargo test -p modal-rust-sdk --features live --test live_mount_upload \
//!     -- --ignored --nocapture
//! ```
//!
//! Modal flakiness ("socket connection closed unexpectedly", etc.) is transient
//! capacity, so each step retries with brief backoff before giving up (per the
//! project verification rules — never "blocked on Modal").

#![cfg(feature = "live")]

use std::time::Duration;

use modal_rust_sdk::{Error, ModalClient};

/// Treat tonic transport errors and known transient gRPC statuses (and
/// socket/transport blips surfaced as build errors) as retryable capacity blips.
fn is_transient(err: &Error) -> bool {
    match err {
        Error::Transport(_) => true,
        Error::Status(status) => {
            use tonic::Code::*;
            if matches!(
                status.code(),
                Unavailable | DeadlineExceeded | Aborted | ResourceExhausted | Unknown | Internal
            ) {
                return true;
            }
            transient_msg(status.message())
        }
        Error::Build(msg) => transient_msg(msg),
        _ => false,
    }
}

fn transient_msg(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("socket connection closed")
        || m.contains("connection reset")
        || m.contains("transport")
        || m.contains("unavailable")
        || m.contains("timed out")
        || m.contains("timeout")
        || m.contains("blob upload")
}

macro_rules! retry {
    ($label:expr, $attempts:expr, $op:expr) => {{
        let label: &str = $label;
        let attempts: u32 = $attempts;
        let mut last: Option<Error> = None;
        let mut out = None;
        for attempt in 1..=attempts {
            match $op.await {
                Ok(v) => {
                    out = Some(Ok(v));
                    break;
                }
                Err(err) => {
                    eprintln!("[{label}] attempt {attempt}/{attempts} failed: {err}");
                    let transient = is_transient(&err);
                    last = Some(err);
                    if !transient || attempt == attempts {
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(2 * attempt as u64)).await;
                }
            }
        }
        out.unwrap_or_else(|| Err(last.expect("an error was recorded")))
    }};
}

#[tokio::test]
#[ignore = "live Modal local-dir upload; run with --features live -- --ignored"]
async fn mount_local_dir_round_trip() {
    match round_trip().await {
        Ok(mount_id) => {
            println!("LIVE OK: mount_id = {mount_id}");
            assert!(!mount_id.is_empty(), "expected a non-empty mount_id");
        }
        Err(err) => {
            panic!("live mount_local_dir failed after retries: {err}");
        }
    }
}

async fn round_trip() -> Result<String, Error> {
    // Build a tiny temp dir with 2 files (+ an ignored target/ tree) so the test
    // is fast and self-contained — no dependence on repo layout.
    let dir = make_temp_source_tree();
    eprintln!("uploading dir: {}", dir.path().display());

    let mut client = retry!("connect", 5, ModalClient::connect())?;
    eprintln!("MILESTONE auth ok (connect + ClientHello)");

    // The temp tree's target/ and *.rlib are pruned by the built-in defaults
    // (DEFAULT_IGNORE_PATTERNS) resolved inside mount_local_dir; the tree has no
    // .gitignore/.modalignore, so the defaults are the only active layer.
    let mount_id = retry!(
        "mount_local_dir",
        4,
        client.mount_local_dir(
            dir.path(),
            "/src",
            modal_rust_sdk::DEFAULT_MODALIGNORE_NAME,
            None
        )
    )?;
    eprintln!("MILESTONE mount_id = {mount_id}");
    Ok(mount_id)
}

/// Minimal temp source tree: Cargo.toml + src/lib.rs, plus target/junk and a
/// foo.rlib that the ignore patterns must drop. Removed on drop.
fn make_temp_source_tree() -> TempTree {
    let base = std::env::temp_dir().join(format!(
        "modal_rust_live_upload_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("src")).unwrap();
    std::fs::create_dir_all(base.join("target/release")).unwrap();
    std::fs::write(
        base.join("Cargo.toml"),
        b"[package]\nname = \"upload-probe\"\nversion = \"0.0.0\"\n",
    )
    .unwrap();
    std::fs::write(base.join("src/lib.rs"), b"pub fn answer() -> i32 { 42 }\n").unwrap();
    std::fs::write(base.join("target/release/junk"), b"ignored").unwrap();
    std::fs::write(base.join("foo.rlib"), b"ignored-rlib").unwrap();
    TempTree { path: base }
}

struct TempTree {
    path: std::path::PathBuf,
}
impl TempTree {
    fn path(&self) -> &std::path::Path {
        &self.path
    }
}
impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
