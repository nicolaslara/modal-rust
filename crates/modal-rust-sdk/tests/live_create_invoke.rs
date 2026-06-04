//! Live, best-effort FILE-mode create + invoke round-trip — the payoff proof.
//!
//! Drives the native ops layer end-to-end against REAL Modal to prove our
//! first-party client can do the full FILE-mode create+invoke with NO `modal`
//! CLI and NO per-project `.py` written to any user project (the wrapper module
//! is baked into the image via `dockerfile_commands`, in-memory only). This
//! mirrors `workpads/shim-backend/spike-main.rs.txt`, but every step is a method
//! on [`ModalClient`] from the `ops` surface.
//!
//! Gated behind BOTH the `live` cargo feature AND `#[ignore]` so the no-CUDA CI
//! box never runs it. Run locally with:
//!
//! ```text
//! cargo test -p modal-rust-sdk --features live --test live_create_invoke \
//!     -- --ignored --nocapture
//! ```
//!
//! The flow:
//!   app_create_ephemeral
//!     -> client_mount_id (hosted modal-client-mount; the Modal-native injection)
//!     -> image_get_or_create (bake a trivial Python echo wrapper into the image)
//!     -> function_precreate
//!     -> function_create (FILE mode, function_serialized = b"")
//!     -> app_publish (AppPublish ONLY — fix #2)
//!     -> function_from_name
//!     -> invoke_cbor((payload,), {}) -> decode the echo result.
//!
//! Modal flakiness ("socket connection closed unexpectedly", etc.) is transient
//! capacity, so each step retries with brief backoff before giving up.

#![cfg(feature = "live")]

use std::collections::HashMap;
use std::time::Duration;

use modal_rust_sdk::{Error, FunctionSpec, ImageSpec, ModalClient};

const FN_NAME: &str = "handler";
const MODULE_NAME: &str = "rust_sdk_live_wrapper";

/// Trivial undecorated FILE-mode wrapper, baked into the image in-memory (no .py
/// is ever written to a user project). Modal's FILE-mode container resolves it
/// via `importlib.import_module(MODULE_NAME)` + `getattr(module, FN_NAME)`.
const WRAPPER_SRC: &str = r#"def handler(payload):
    return {"echoed": payload, "ok": True, "source": "rust_sdk_live_wrapper.handler"}
"#;

/// Treat tonic transport errors and a known set of transient gRPC statuses (and
/// "socket connection closed" style build/transport blips) as retryable capacity
/// blips (per project verification rules — never "blocked on Modal").
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
        // Build/poll errors surface remote text; treat socket/transport blips as transient.
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
}

/// Retry an async expression a few times with linear backoff, bailing early on a
/// non-transient error. A macro (not a fn) so the awaited expression can freely
/// re-borrow `&mut client` each attempt without a closure lifetime escape.
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
#[ignore = "live Modal FILE-mode create+invoke; run with --features live -- --ignored"]
async fn file_mode_create_and_invoke_round_trip() {
    match round_trip().await {
        Ok(value) => {
            println!("LIVE OK: decoded invoke result = {value:?}");
            // The echo handler returns {"echoed": <payload>, "ok": true, "source": ...}.
            assert_eq!(
                value.get("ok"),
                Some(&serde_cbor::Value::Bool(true)),
                "echo handler should report ok=true; got {value:?}"
            );
            assert!(
                value.contains_key("echoed"),
                "echo handler should return an 'echoed' key; got {value:?}"
            );
        }
        Err(err) => {
            // Per project rules: a transient Modal blip after retries is NOT a
            // design block. Surface it loudly but do not pretend it is a bug.
            panic!("live FILE-mode create+invoke failed after retries: {err}");
        }
    }
}

async fn round_trip() -> Result<HashMap<String, serde_cbor::Value>, Error> {
    // Unique-ish ephemeral app name so reruns never collide.
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let app_name = format!("modal-rust-sdk-live-{suffix}");

    let mut client = retry!("connect", 5, ModalClient::connect())?;
    eprintln!("MILESTONE auth ok (connect + ClientHello)");

    // 1. Ephemeral app (discharged on disconnect).
    let app_id = retry!(
        "app_create",
        4,
        client.app_create_ephemeral(&app_name, None)
    )?;
    eprintln!("MILESTONE app_id = {app_id}");

    // 2. Resolve the hosted client mount — the Modal-native way to make `modal`
    //    importable in the FILE-mode container (no `pip install modal`).
    let client_mount_id = retry!("client_mount", 4, client.client_mount_id(None))?;
    eprintln!("MILESTONE client_mount_id = {client_mount_id}");

    // 3. Build the image: a registry base with the trivial wrapper baked in.
    //    The wrapper source lives only in memory + the image layer; nothing is
    //    written to any user project on disk.
    //
    //    The client MOUNT (step 2) supplies the `modal` *source*, but a bare
    //    `python:3-slim` base lacks the client's pip deps (typing_extensions,
    //    grpclib, protobuf, …) — without them the FILE-mode container crash-loops
    //    on `python -m modal._container_entrypoint` (verified live, 2026-06-04). So
    //    we provision the dependency closure with pip; the mount's /pkg still wins
    //    on PYTHONPATH so the mounted source stays authoritative.
    let image_spec = ImageSpec::default_base()
        .with_wrapper_module(MODULE_NAME, WRAPPER_SRC)
        .with_pip_install_modal();
    let image_id = retry!(
        "image_get_or_create",
        5,
        client.image_get_or_create(&app_id, &image_spec)
    )?;
    eprintln!("MILESTONE image_id = {image_id}");

    // 4. Precreate -> the function_id that legalizes empty function_serialized.
    let precreate_id = retry!(
        "function_precreate",
        5,
        client.function_precreate(&app_id, FN_NAME)
    )?;
    eprintln!("MILESTONE precreate function_id = {precreate_id}");

    // 5. FunctionCreate in FILE mode: module + function name, empty serialized,
    //    client mount attached, resources always set (the 3 fixes are baked in).
    let fn_spec = FunctionSpec::new(MODULE_NAME, FN_NAME, &image_id)
        .with_mount_id(&client_mount_id)
        .with_timeout_secs(300);
    let created = retry!(
        "function_create",
        5,
        client.function_create(&app_id, &precreate_id, &fn_spec)
    )?;
    eprintln!(
        "MILESTONE created function_id = {} (definition_id = {})",
        created.function_id, created.definition_id
    );
    for w in &created.warnings {
        eprintln!("create warning: {w}");
    }

    // 6. Publish via AppPublish ONLY (fix #2).
    let mut function_ids = HashMap::new();
    function_ids.insert(FN_NAME.to_string(), created.function_id.clone());
    let mut definition_ids = HashMap::new();
    if !created.definition_id.is_empty() {
        definition_ids.insert(created.function_id.clone(), created.definition_id.clone());
    }
    // EPHEMERAL publish (the app was created ephemeral, line ~150): publishing is
    // required to make the function invokable, but the ephemeral state keeps the
    // app throwaway so this live test leaves no lingering deploy.
    let published = retry!(
        "app_publish",
        5,
        client.app_publish_ephemeral(
            &app_id,
            &app_name,
            function_ids.clone(),
            definition_ids.clone(),
        )
    )?;
    eprintln!("MILESTONE published: {published:?}");

    // 7. Invoke via the FunctionCreate `function_id` DIRECTLY. The app was
    //    published EPHEMERAL (so it GCs on disconnect, leaving no lingering
    //    deploy), and `from_name`/`FunctionGet` only resolves DEPLOYED apps — an
    //    ephemeral app is not name-resolvable in the environment. So we invoke the
    //    created id directly, exactly like the facade RUN path and Modal Python's
    //    ephemeral `app.run()` (which invokes the loaded handle by `object_id`).
    let invoke_id = created.function_id.clone();
    eprintln!("MILESTONE invoke function_id = {invoke_id}");

    // 8. Invoke with CBOR (args, kwargs) = ((payload,), {}) and decode the echo.
    let mut payload = HashMap::new();
    payload.insert("hi".to_string(), 1_i64);
    payload.insert("n".to_string(), 42_i64);
    let args = (payload,); // positional tuple: handler(payload)
    let kwargs: HashMap<String, serde_cbor::Value> = HashMap::new();

    let result = retry!(
        "invoke",
        8,
        client.invoke_cbor::<_, _, HashMap<String, serde_cbor::Value>>(&invoke_id, &args, &kwargs)
    )?;
    eprintln!("MILESTONE invoke result obtained: {result:?}");
    Ok(result)
}
