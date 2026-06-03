//! modal-rust runtime: the frozen runner protocol seam.
//!
//! This crate is recompiled on **every** dev run and baked into **every** deploy
//! image, so it keeps a minimal dependency surface (only `serde`, `serde_json`,
//! `anyhow`, and a hand-rolled arg parser) — no Modal / network / Python deps
//! (boundaries.md §1).
//!
//! It provides:
//! - [`HandlerFn`] — a bare `fn` pointer (static dispatch — no `Box<dyn>`, no
//!   vtable).
//! - [`RunnerError`] — the five-kind error taxonomy serialized into the frozen
//!   failure envelope.
//! - the [`codec`] module — a JSON [`codec::Codec`] over bytes.
//! - the [`typed!`] macro — builds a monomorphized wrapper `fn` and yields its
//!   pointer.
//! - [`Registry`] — `BTreeMap<&'static str, HandlerFn>` with duplicate-name
//!   rejection.
//! - [`run_cli`] — parses the three runner flags, runs exactly one handler under
//!   `catch_unwind`, and prints exactly one JSON envelope to stdout.

use std::backtrace::Backtrace;
use std::collections::BTreeMap;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Mutex;

use serde::Serialize;

/// A handler reduced to a bare `fn` pointer: bytes in, bytes out (boundaries.md
/// §3). No `dyn`, no `Box`, no vtable — every [`Registry`] entry is a
/// monomorphized free function reached through one cheap indirect jump after the
/// name lookup.
pub type HandlerFn = fn(&[u8]) -> Result<Vec<u8>, RunnerError>;

/// The frozen failure taxonomy (boundaries.md §2). The runner models failure as a
/// Rust enum that **wraps** the user's error rather than stringifying it early, so
/// the monomorphized [`typed!`] wrapper can preserve structure.
#[derive(Debug)]
pub enum RunnerError {
    /// Input was not valid JSON, or valid JSON that failed to deserialize into the
    /// handler's `In`.
    Decode(String),
    /// `--entrypoint` named a handler not present in the [`Registry`].
    UnknownEntrypoint(String),
    /// The handler returned `Err(_)`. `message` is the `Display` / anyhow chain;
    /// `details` is the serialized user error when its type is `Serialize`, else
    /// `null`.
    Function {
        /// `Display` / anyhow-chain rendering of the user's error.
        message: String,
        /// The structurally serialized user error when `E: Serialize`, else `None`.
        details: Option<serde_json::Value>,
    },
    /// The handler's `Out` failed to serialize (e.g. non-string map keys, NaN).
    /// Must **not** be reported as [`RunnerError::Panic`].
    Encode(String),
    /// The handler unwound. `message` + `backtrace` are captured via a panic hook
    /// plus `catch_unwind`.
    Panic {
        /// The panic payload message.
        message: String,
        /// A captured `std::backtrace::Backtrace` rendering.
        backtrace: String,
    },
}

impl RunnerError {
    /// The frozen `kind` discriminant string for the failure envelope.
    pub fn kind(&self) -> &'static str {
        match self {
            RunnerError::Decode(_) => "decode_error",
            RunnerError::UnknownEntrypoint(_) => "unknown_entrypoint",
            RunnerError::Function { .. } => "function_error",
            RunnerError::Encode(_) => "encode_error",
            RunnerError::Panic { .. } => "panic",
        }
    }

    /// The human-readable `message` field for the failure envelope.
    pub fn message(&self) -> &str {
        match self {
            RunnerError::Decode(m) => m,
            RunnerError::UnknownEntrypoint(m) => m,
            RunnerError::Function { message, .. } => message,
            RunnerError::Encode(m) => m,
            RunnerError::Panic { message, .. } => message,
        }
    }

    /// The optional structural `details` field (the wrapped user error).
    pub fn details(&self) -> Option<&serde_json::Value> {
        match self {
            RunnerError::Function { details, .. } => details.as_ref(),
            _ => None,
        }
    }

    /// The `backtrace` field; non-empty only for the [`RunnerError::Panic`] kind.
    pub fn backtrace(&self) -> &str {
        match self {
            RunnerError::Panic { backtrace, .. } => backtrace,
            _ => "",
        }
    }

    /// Build a [`RunnerError::Function`] from a user error, preserving structure
    /// when the error type is [`Serialize`].
    ///
    /// This is the entry point the [`typed!`] macro uses for `Serialize` errors.
    /// `details = serde_json::to_value(&e).ok()` (boundaries.md §2). For anyhow
    /// (and other opaque) errors, callers use [`RunnerError::function_opaque`]
    /// which sets `details = None`.
    pub fn function<E>(e: E) -> Self
    where
        E: std::fmt::Display + Serialize,
    {
        let message = e.to_string();
        let details = serde_json::to_value(&e).ok();
        RunnerError::Function { message, details }
    }

    /// Build a [`RunnerError::Function`] from an opaque (non-`Serialize`) user
    /// error such as `anyhow::Error`. `details` is `None`; `message` is the
    /// `Display` rendering (the full anyhow chain for `anyhow::Error`).
    pub fn function_opaque<E>(e: E) -> Self
    where
        E: std::fmt::Display,
    {
        RunnerError::Function {
            message: e.to_string(),
            details: None,
        }
    }

    /// Render this error into the frozen failure envelope value
    /// `{"ok":false,"error":{"kind":..,"message":..,"details":<json|null>,"backtrace":..}}`.
    pub fn to_envelope(&self) -> serde_json::Value {
        serde_json::json!({
            "ok": false,
            "error": {
                "kind": self.kind(),
                "message": self.message(),
                "details": self.details().cloned().unwrap_or(serde_json::Value::Null),
                "backtrace": self.backtrace(),
            }
        })
    }
}

impl std::fmt::Display for RunnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind(), self.message())
    }
}

impl std::error::Error for RunnerError {}

/// JSON wire-format codec over bytes (boundaries.md §3, codec-neutral seam). A
/// future `--input-format cbor` would add a sibling impl here without touching
/// [`HandlerFn`] or [`Registry`].
pub mod codec {
    use super::RunnerError;
    use serde::de::DeserializeOwned;
    use serde::Serialize;

    /// Decode input bytes into a handler's `In`. A failure (invalid JSON or
    /// wrong-shape JSON) maps to [`RunnerError::Decode`].
    pub fn decode<T: DeserializeOwned>(input: &[u8]) -> Result<T, RunnerError> {
        serde_json::from_slice(input).map_err(|e| RunnerError::Decode(e.to_string()))
    }

    /// Encode a handler's `Out` into output bytes. A failure (e.g. non-string map
    /// keys, NaN) maps to [`RunnerError::Encode`] — never `panic`.
    pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, RunnerError> {
        serde_json::to_vec(value).map_err(|e| RunnerError::Encode(e.to_string()))
    }
}

/// `typed!(f)` generates a **monomorphized wrapper `fn`** for the handler `f` and
/// yields its [`HandlerFn`] pointer (boundaries.md §3). decode/call/encode are all
/// inlined/monomorphized for `f`'s concrete `In`/`Out`/`Err`.
///
/// The user error is wrapped onto [`RunnerError::Function`]: `details` is the
/// serialized error when the error type is `Serialize`, else `null`. The selection
/// is resolved at compile time by autoref specialization (see
/// [`__macro_support`]), so `anyhow::Result` handlers land on `details = null`
/// while a `Serialize` error populates `details`.
#[macro_export]
macro_rules! typed {
    ($f:path) => {{
        fn __wrap(
            input: &[u8],
        ) -> ::core::result::Result<::std::vec::Vec<u8>, $crate::RunnerError> {
            let arg = $crate::codec::decode(input)?;
            match $f(arg) {
                ::core::result::Result::Ok(out) => $crate::codec::encode(&out),
                ::core::result::Result::Err(e) => {
                    // Inherent-priority specialization (boundaries.md §2): the
                    // inherent `Wrap<E>::to_runner_error` (requires `E: Serialize`,
                    // populates `details`) wins over the `FunctionErrorOpaque` trait
                    // method (`details = null`, covers `anyhow::Error`) at this
                    // monomorphic call site whenever its bound is satisfied; otherwise
                    // resolution falls through to the trait method. The trait must be
                    // in scope for the fallback; the inherent method needs no import.
                    // When the handler's error is `Serialize`, the inherent method is
                    // chosen and this import is (expectedly) unused.
                    #[allow(unused_imports)]
                    use $crate::__macro_support::FunctionErrorOpaque as _;
                    ::core::result::Result::Err($crate::__macro_support::Wrap(e).to_runner_error())
                }
            }
        }
        __wrap as $crate::HandlerFn
    }};
}

/// Macro-internal support used by [`typed!`] to choose, at compile time, between
/// the `Serialize` path (populates `details`) and the opaque path (`details =
/// null`). Not part of the stable API.
#[doc(hidden)]
pub mod __macro_support {
    use super::RunnerError;
    use serde::Serialize;

    /// Newtype the user error so an **inherent** constrained method (the `Serialize`
    /// path) and a **trait** blanket method (the opaque fallback) can share the name
    /// `to_runner_error`. At a monomorphic call site, an inherent method whose
    /// bounds are satisfied wins over the trait method; when the bounds are not
    /// satisfied (e.g. `anyhow::Error`, which is not `Serialize`), resolution falls
    /// through to the trait method. This is the stable autoref/inherent-priority
    /// specialization the `typed!` macro relies on.
    pub struct Wrap<E>(pub E);

    /// More-specific path: an **inherent** method available only when
    /// `E: Display + Serialize`. Populates `details` structurally (boundaries.md §2).
    impl<E: std::fmt::Display + Serialize> Wrap<E> {
        pub fn to_runner_error(&self) -> RunnerError {
            RunnerError::function(&self.0)
        }
    }

    /// Fallback path: a trait blanket method requiring only `Display`. Selected when
    /// the inherent `Serialize` method does not apply (e.g. `anyhow::Error`),
    /// yielding `details = null`.
    pub trait FunctionErrorOpaque {
        fn to_runner_error(&self) -> RunnerError;
    }

    impl<E: std::fmt::Display> FunctionErrorOpaque for Wrap<E> {
        fn to_runner_error(&self) -> RunnerError {
            RunnerError::function_opaque(&self.0)
        }
    }
}

/// A distributed registration entry collected by [`inventory`] (boundaries.md §3,
/// ergonomics E1). The `#[modal_rust::function]` proc-macro submits one of these
/// per annotated handler — `name` defaults to the fn name (overridable with
/// `#[modal_rust::function(name = "...")]`), and `handler` is the SAME
/// monomorphized `typed!` wrapper `fn` pointer the manual builder uses.
///
/// [`Registry::from_inventory`] collects every submission into the SAME
/// `BTreeMap<&'static str, HandlerFn>` the manual
/// `Registry::new().function(name, typed!(f))` builder produces, so both
/// registration paths converge on one dispatch path (boundaries.md §3). There is
/// no `dyn`, no `Box`, no vtable: `handler` is a bare `fn` pointer.
pub struct Registration {
    /// The entrypoint name (registry key).
    pub name: &'static str,
    /// The monomorphized [`typed!`] wrapper `fn` pointer.
    pub handler: HandlerFn,
}

inventory::collect!(Registration);

/// The handler registry: `BTreeMap<&'static str, HandlerFn>` (boundaries.md §3).
/// Static-str keys, fn-pointer values — no allocation, no `dyn`. Built with
/// [`Registry::new`] + [`Registry::function`]; **duplicate names are rejected**
/// (no silent last-write-wins).
#[derive(Default)]
pub struct Registry {
    handlers: BTreeMap<&'static str, HandlerFn>,
}

impl Registry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Registry {
            handlers: BTreeMap::new(),
        }
    }

    /// Assemble a [`Registry`] from every [`Registration`] submitted via
    /// [`inventory`] by the `#[modal_rust::function]` macro (boundaries.md §3,
    /// ergonomics E1).
    ///
    /// This converges on the SAME dispatch path as the manual builder: each
    /// collected entry is inserted through the same insertion logic as
    /// [`Registry::function`], so the resulting `BTreeMap<&'static str, HandlerFn>`
    /// is shape-identical to one built by hand.
    ///
    /// # Panics
    /// Panics (the SAME hard startup error as [`Registry::function`]) if two
    /// submissions share a name — duplicate names are rejected, never silently
    /// last-write-wins. This matters more in the macro/inventory world, where a
    /// duplicate `#[modal_rust::function(name = "x")]` is easy to write.
    pub fn from_inventory() -> Self {
        let mut registry = Registry::new();
        for registration in inventory::iter::<Registration> {
            registry = registry.function(registration.name, registration.handler);
        }
        registry
    }

    /// Register a handler under `name`.
    ///
    /// # Panics
    /// Panics (a hard startup error) if `name` is already registered — duplicate
    /// names are rejected, never silently last-write-wins (boundaries.md §3).
    pub fn function(mut self, name: &'static str, handler: HandlerFn) -> Self {
        if self.handlers.insert(name, handler).is_some() {
            panic!("duplicate entrypoint registered: {name:?}");
        }
        self
    }

    /// Look up a handler by name.
    pub fn get(&self, name: &str) -> Option<HandlerFn> {
        self.handlers.get(name).copied()
    }

    /// The registered entrypoint names, sorted (for diagnostics).
    pub fn names(&self) -> impl Iterator<Item = &&'static str> {
        self.handlers.keys()
    }
}

/// Captured panic info populated by the installed panic hook (boundaries.md §2).
///
/// v0 uses a process-global slot and the process exits after one call, so this is
/// correct for v0; a future concurrent host must revisit per-call routing.
static PANIC_SLOT: Mutex<Option<(String, String)>> = Mutex::new(None);

/// Run a single handler under `catch_unwind`, converting any of the five failure
/// modes into a [`RunnerError`]. A panic is captured (message + backtrace) via a
/// panic hook installed **only** around the call and surfaced as
/// [`RunnerError::Panic`] (boundaries.md §2).
///
/// The hook is scoped: the previous hook is saved and restored afterward, so the
/// runner never leaves a global hook installed that would swallow unrelated panics
/// (e.g. a test harness's assertion output). v0 uses a process-global slot and the
/// process exits after one call, so this is correct for v0.
fn run_handler(handler: HandlerFn, input: &[u8]) -> Result<Vec<u8>, RunnerError> {
    if let Ok(mut slot) = PANIC_SLOT.lock() {
        *slot = None;
    }
    let previous = panic::take_hook();
    panic::set_hook(Box::new(|info| {
        let message = info.to_string();
        let backtrace = Backtrace::force_capture().to_string();
        if let Ok(mut slot) = PANIC_SLOT.lock() {
            *slot = Some((message, backtrace));
        }
    }));
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| handler(input)));
    panic::set_hook(previous);
    match outcome {
        Ok(result) => result,
        Err(_) => {
            let (message, backtrace) = PANIC_SLOT
                .lock()
                .ok()
                .and_then(|mut s| s.take())
                .unwrap_or_else(|| ("panic (no message captured)".to_string(), String::new()));
            Err(RunnerError::Panic { message, backtrace })
        }
    }
}

/// The parsed runner invocation (boundaries.md §2):
/// `--entrypoint <name> ( --input-json <json> | --input-file <path> | --input-stdin )`.
struct Args {
    entrypoint: String,
    input: InputSource,
}

enum InputSource {
    Json(String),
    File(String),
    Stdin,
}

/// A flag-parse failure. The message goes to stderr; the process exits 1.
struct ArgError(String);

fn parse_args(argv: &[String]) -> Result<Args, ArgError> {
    let mut entrypoint: Option<String> = None;
    let mut input: Option<InputSource> = None;
    let mut i = 0;
    while i < argv.len() {
        let arg = &argv[i];
        match arg.as_str() {
            "--entrypoint" => {
                let v = argv
                    .get(i + 1)
                    .ok_or_else(|| ArgError("--entrypoint requires a value".to_string()))?;
                if entrypoint.is_some() {
                    return Err(ArgError("--entrypoint given more than once".to_string()));
                }
                entrypoint = Some(v.clone());
                i += 2;
            }
            "--input-json" => {
                let v = argv
                    .get(i + 1)
                    .ok_or_else(|| ArgError("--input-json requires a value".to_string()))?;
                if input.is_some() {
                    return Err(ArgError("multiple input sources given".to_string()));
                }
                input = Some(InputSource::Json(v.clone()));
                i += 2;
            }
            "--input-file" => {
                let v = argv
                    .get(i + 1)
                    .ok_or_else(|| ArgError("--input-file requires a value".to_string()))?;
                if input.is_some() {
                    return Err(ArgError("multiple input sources given".to_string()));
                }
                input = Some(InputSource::File(v.clone()));
                i += 2;
            }
            "--input-stdin" => {
                if input.is_some() {
                    return Err(ArgError("multiple input sources given".to_string()));
                }
                input = Some(InputSource::Stdin);
                i += 1;
            }
            other => {
                return Err(ArgError(format!("unrecognized argument: {other}")));
            }
        }
    }
    let entrypoint = entrypoint.ok_or_else(|| ArgError("--entrypoint is required".to_string()))?;
    let input = input.ok_or_else(|| {
        ArgError("one of --input-json, --input-file, or --input-stdin is required".to_string())
    })?;
    Ok(Args { entrypoint, input })
}

fn read_input(source: &InputSource) -> Result<Vec<u8>, ArgError> {
    match source {
        InputSource::Json(s) => Ok(s.clone().into_bytes()),
        InputSource::File(path) => std::fs::read(path)
            .map_err(|e| ArgError(format!("failed to read --input-file {path:?}: {e}"))),
        InputSource::Stdin => {
            use std::io::Read;
            let mut buf = Vec::new();
            std::io::stdin()
                .read_to_end(&mut buf)
                .map_err(|e| ArgError(format!("failed to read stdin: {e}")))?;
            Ok(buf)
        }
    }
}

/// Parse the runner flags, dispatch into `registry`, and print **exactly one** JSON
/// envelope to stdout (all diagnostics to stderr). Returns the process exit code:
/// `0` on success, `1` on any failure (boundaries.md §2). Installs a panic hook so
/// handler panics are captured as the `panic` error kind.
///
/// Precedence (frozen): top-level JSON parse → entrypoint lookup → decode `In` →
/// call → encode `Out`. A malformed-JSON input therefore yields `decode_error`
/// even when the entrypoint name is also unknown.
pub fn run_cli(registry: Registry) -> i32 {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    run_cli_with_args(registry, &argv, &mut std::io::stdout())
}

/// The testable core of [`run_cli`]: takes explicit argv and an output sink so the
/// envelope can be captured in unit tests. Diagnostics still go to stderr.
pub fn run_cli_with_args<W: std::io::Write>(
    registry: Registry,
    argv: &[String],
    out: &mut W,
) -> i32 {
    let args = match parse_args(argv) {
        Ok(a) => a,
        Err(ArgError(msg)) => {
            eprintln!("modal_runner: argument error: {msg}");
            return 1;
        }
    };

    let raw_input = match read_input(&args.input) {
        Ok(b) => b,
        Err(ArgError(msg)) => {
            eprintln!("modal_runner: {msg}");
            return 1;
        }
    };

    // Frozen precedence: top-level JSON parse precedes entrypoint lookup. A
    // malformed-JSON input is a `decode_error` even when the entrypoint is unknown.
    if let Err(e) = serde_json::from_slice::<serde_json::Value>(&raw_input) {
        let err = RunnerError::Decode(e.to_string());
        return emit(out, &err.to_envelope(), 1);
    }

    // Entrypoint lookup precedes decode of `In`.
    let handler = match registry.get(&args.entrypoint) {
        Some(h) => h,
        None => {
            let err = RunnerError::UnknownEntrypoint(format!(
                "unknown entrypoint {:?}; known entrypoints: [{}]",
                args.entrypoint,
                registry
                    .names()
                    .map(|n| format!("{n:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            return emit(out, &err.to_envelope(), 1);
        }
    };

    match run_handler(handler, &raw_input) {
        Ok(bytes) => {
            // The handler already encoded a valid JSON value via the codec; splice
            // it into the success envelope without a redundant string round-trip.
            let value: serde_json::Value = match serde_json::from_slice(&bytes) {
                Ok(v) => v,
                Err(e) => {
                    let err =
                        RunnerError::Encode(format!("handler output was not valid JSON: {e}"));
                    return emit(out, &err.to_envelope(), 1);
                }
            };
            let envelope = serde_json::json!({ "ok": true, "value": value });
            emit(out, &envelope, 0)
        }
        Err(err) => emit(out, &err.to_envelope(), 1),
    }
}

/// Write exactly one JSON envelope (one line) to `out` and return `code`. Any write
/// failure is reported to stderr and forces exit 1.
fn emit<W: std::io::Write>(out: &mut W, envelope: &serde_json::Value, code: i32) -> i32 {
    match serde_json::to_string(envelope) {
        Ok(s) => {
            if let Err(e) = writeln!(out, "{s}") {
                eprintln!("modal_runner: failed to write envelope: {e}");
                return 1;
            }
            code
        }
        Err(e) => {
            eprintln!("modal_runner: failed to serialize envelope: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Deserialize)]
    struct In {
        a: i64,
        b: i64,
    }

    #[derive(Serialize)]
    struct Out {
        sum: i64,
    }

    fn add(input: In) -> Result<Out, anyhow::Error> {
        Ok(Out {
            sum: input.a + input.b,
        })
    }

    #[derive(Debug, Serialize)]
    struct StructuredErr {
        code: u32,
        reason: String,
    }
    impl std::fmt::Display for StructuredErr {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "structured failure ({}): {}", self.code, self.reason)
        }
    }
    impl std::error::Error for StructuredErr {}

    fn fail_structured(_input: In) -> Result<Out, StructuredErr> {
        Err(StructuredErr {
            code: 7,
            reason: "boom".to_string(),
        })
    }

    fn fail_anyhow(_input: In) -> anyhow::Result<Out> {
        Err(anyhow::anyhow!("anyhow failure"))
    }

    #[derive(Serialize)]
    struct BadOut {
        // A tuple-keyed map has no JSON object representation -> encode_error
        // ("key must be a string"). int/bool keys would be coerced; NaN -> null.
        by_pair: std::collections::BTreeMap<(i32, i32), i32>,
    }
    fn bad_encode(_input: In) -> anyhow::Result<BadOut> {
        let mut by_pair = std::collections::BTreeMap::new();
        by_pair.insert((1, 2), 3);
        Ok(BadOut { by_pair })
    }

    fn will_panic(_input: In) -> anyhow::Result<Out> {
        panic!("deliberate test panic")
    }

    fn registry() -> Registry {
        Registry::new()
            .function("add", typed!(add))
            .function("fail_structured", typed!(fail_structured))
            .function("fail_anyhow", typed!(fail_anyhow))
            .function("bad_encode", typed!(bad_encode))
            .function("will_panic", typed!(will_panic))
    }

    fn run(args: &[&str]) -> (serde_json::Value, i32) {
        let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut buf = Vec::new();
        let code = run_cli_with_args(registry(), &argv, &mut buf);
        let v: serde_json::Value = serde_json::from_slice(&buf).expect("one JSON envelope");
        (v, code)
    }

    #[test]
    fn success_add() {
        let (v, code) = run(&["--entrypoint", "add", "--input-json", r#"{"a":40,"b":2}"#]);
        assert_eq!(code, 0);
        assert_eq!(v, serde_json::json!({"ok": true, "value": {"sum": 42}}));
    }

    #[test]
    fn unknown_entrypoint() {
        let (v, code) = run(&["--entrypoint", "nope", "--input-json", "{}"]);
        assert_eq!(code, 1);
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["kind"], "unknown_entrypoint");
        assert_eq!(v["error"]["details"], serde_json::Value::Null);
    }

    #[test]
    fn decode_error_malformed_json() {
        let (v, code) = run(&["--entrypoint", "add", "--input-json", "not-json"]);
        assert_eq!(code, 1);
        assert_eq!(v["error"]["kind"], "decode_error");
    }

    #[test]
    fn decode_error_wrong_shape() {
        let (v, code) = run(&["--entrypoint", "add", "--input-json", r#"{"a":1}"#]);
        assert_eq!(code, 1);
        assert_eq!(v["error"]["kind"], "decode_error");
    }

    #[test]
    fn function_error_anyhow_details_null() {
        let (v, code) = run(&[
            "--entrypoint",
            "fail_anyhow",
            "--input-json",
            r#"{"a":40,"b":2}"#,
        ]);
        assert_eq!(code, 1);
        assert_eq!(v["error"]["kind"], "function_error");
        assert_eq!(v["error"]["details"], serde_json::Value::Null);
        assert!(v["error"]["message"].as_str().unwrap().contains("anyhow"));
    }

    #[test]
    fn function_error_serialize_details_populated() {
        let (v, code) = run(&[
            "--entrypoint",
            "fail_structured",
            "--input-json",
            r#"{"a":40,"b":2}"#,
        ]);
        assert_eq!(code, 1);
        assert_eq!(v["error"]["kind"], "function_error");
        assert_eq!(
            v["error"]["details"],
            serde_json::json!({"code": 7, "reason": "boom"})
        );
    }

    #[test]
    fn encode_error_not_panic() {
        let (v, code) = run(&[
            "--entrypoint",
            "bad_encode",
            "--input-json",
            r#"{"a":40,"b":2}"#,
        ]);
        assert_eq!(code, 1);
        assert_eq!(v["error"]["kind"], "encode_error");
    }

    #[test]
    fn panic_captured_with_backtrace() {
        std::env::set_var("RUST_BACKTRACE", "1");
        let (v, code) = run(&[
            "--entrypoint",
            "will_panic",
            "--input-json",
            r#"{"a":0,"b":0}"#,
        ]);
        assert_eq!(code, 1);
        assert_eq!(v["error"]["kind"], "panic");
        assert!(v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("deliberate test panic"));
        assert!(!v["error"]["backtrace"].as_str().unwrap().is_empty());
    }

    #[test]
    fn precedence_malformed_json_beats_unknown_entrypoint() {
        let (v, code) = run(&["--entrypoint", "nope", "--input-json", "not-json"]);
        assert_eq!(code, 1);
        assert_eq!(v["error"]["kind"], "decode_error");
    }

    #[test]
    #[should_panic(expected = "duplicate entrypoint")]
    fn duplicate_names_rejected() {
        let _ = Registry::new()
            .function("add", typed!(add))
            .function("add", typed!(add));
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn not_panic_abort_profile() {
        // catch_unwind requires the unwind strategy; on `panic = "abort"` the
        // process would abort inside `will_panic` instead of producing the `panic`
        // envelope (boundaries.md §6). The success of `panic_captured_with_backtrace`
        // already proves unwind at test time; assert the cfg as an explicit signal.
        assert!(!cfg!(panic = "abort"), "build must not be panic = abort");
    }
}
