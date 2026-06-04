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
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Once;

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
    /// Per-function deploy/run config sourced from the decorator
    /// (`#[modal_rust::function(gpu=…, timeout=…, cache=…)]`). METADATA ONLY — the
    /// runner ignores it (see [`FunctionConfig`]). Default = all `None`.
    pub config: FunctionConfig,
}

/// Per-function deploy/run CONFIG sourced from
/// `#[modal_rust::function(gpu=…, timeout=…, cache=…)]`.
///
/// METADATA ONLY. The runner IGNORES every field — `run_cli`/`run_handler`/dispatch
/// and [`Registry::from_inventory`] never read it. Only the control-plane facade
/// (`modal-rust`) reads it when CREATING the Modal function (`Resources.gpu_config`,
/// `timeout_secs`). The bare `#[modal_rust::function]` yields
/// `FunctionConfig::default()` (all `None` => server/facade defaults), so adding
/// this field changes nothing about how functions RUN (boundaries.md anticipated
/// this additive extension).
///
/// `gpu` is `Option<&'static str>` (not `String`): `inventory::submit!` builds a
/// `static` initializer, so only `const`-constructible values are allowed; a string
/// literal is `&'static str` (matches [`Registration::name`]). The same constraint
/// is why `secrets`/`volumes` are `&'static` slices (not `Vec`): a `Vec` is not
/// `const`-constructible, but a slice literal `&["a", "b"]` is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FunctionConfig {
    /// GPU spec string, Modal-format (`"T4"`, `"A100"`, `"A100-80GB"`, `"H100:4"`).
    /// `None` => CPU.
    pub gpu: Option<&'static str>,
    /// Function timeout (seconds). `None` => facade default.
    pub timeout_secs: Option<u32>,
    /// Cache hint. `None` => default. Reserved/inert for P4 (no proto target yet).
    pub cache: Option<bool>,
    /// Named Modal secrets to attach (`#[function(secrets = ["a", "b"])]`). The
    /// facade resolves each name to a `secret_id` and attaches it to
    /// `FunctionCreate.secret_ids`; the secret's key/values are injected as ENV VARS
    /// in the container. EMPTY (the bare-decorator default) => no secrets => wire-
    /// identical to before. METADATA ONLY — the runner ignores it.
    pub secrets: &'static [&'static str],
    /// User-volume mounts to attach (`#[function(volumes = ["/data=my-vol"])]`),
    /// parsed by the macro into `(mount_path, name)` pairs. The facade resolves each
    /// `name` via `volume_get_or_create` and attaches a `FunctionVolumeMount` at
    /// `mount_path` — a SEPARATE mount from the P6 cargo cache (`/cache`). EMPTY =>
    /// no user volumes => wire-identical to before. METADATA ONLY.
    pub volumes: &'static [(&'static str, &'static str)],
}

impl FunctionConfig {
    /// A `const` all-default config (the bare-decorator default): all `None`, empty
    /// `secrets`/`volumes`. Usable in a `static` `inventory::submit!` initializer,
    /// where the non-`const` `Default::default()` is not allowed. The bare
    /// `#[modal_rust::function]` macro emits the equivalent struct literal directly.
    pub const fn new() -> Self {
        FunctionConfig {
            gpu: None,
            timeout_secs: None,
            cache: None,
            secrets: &[],
            volumes: &[],
        }
    }
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

/// Like [`Registry::from_inventory`] but ALSO returns the per-name
/// [`FunctionConfig`] captured from the SAME inventory pass.
///
/// The returned [`Registry`] is byte-identical to one built by
/// [`Registry::from_inventory`] (same insertion order through
/// [`Registry::function`], same duplicate-name panic). The facade reads the
/// per-name configs to set `Resources.gpu_config` / `timeout_secs` when CREATING
/// the Modal function. The runner never calls this — it stays on
/// `Registry::from_inventory()` (frozen), which reads only `name` + `handler`.
///
/// # Panics
/// Panics (the SAME hard startup error as [`Registry::from_inventory`]) if two
/// submissions share a name.
pub fn from_inventory_with_configs() -> (Registry, Vec<(&'static str, FunctionConfig)>) {
    let mut registry = Registry::new();
    let mut configs = Vec::new();
    for registration in inventory::iter::<Registration> {
        // SAME insertion + duplicate-name panic as `Registry::from_inventory`.
        registry = registry.function(registration.name, registration.handler);
        configs.push((registration.name, registration.config.clone()));
    }
    (registry, configs)
}

thread_local! {
    /// Per-thread captured panic info `(message, backtrace)`, populated by the
    /// process-wide panic hook (boundaries.md §2). Using a `thread_local!` instead
    /// of a process-global slot means parallel panics on different threads never
    /// race — each panicking thread writes only its own slot, which its own
    /// `catch_unwind` then reads. This matters under the test harness, which runs
    /// tests (including other deliberate panics) concurrently. The §3 concurrency
    /// caveat about a process-global slot is thereby resolved for the
    /// process-exits-after-one-call v0 path and made safe under concurrency.
    static PANIC_SLOT: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
}

/// Ensures the panic hook is installed exactly once for the whole process
/// (boundaries.md §2). Installing via `Once` rather than swapping the hook per call
/// avoids the take/restore race that two concurrently panicking threads would hit.
static HOOK_INIT: Once = Once::new();

/// Install — exactly once — a process-wide panic hook that records the panicking
/// thread's `(message, backtrace)` into that thread's [`PANIC_SLOT`].
///
/// The hook always uses `Backtrace::force_capture()`, so the `panic` envelope's
/// `backtrace` is populated regardless of the `RUST_BACKTRACE` env var (no shim
/// env dependency). The hook only writes a thread-local; it neither prints nor
/// chains to a previous hook, so it stays quiet and never swallows or duplicates
/// unrelated panic output beyond suppressing the default stderr message.
fn install_panic_hook() {
    HOOK_INIT.call_once(|| {
        panic::set_hook(Box::new(|info| {
            let message = info.to_string();
            let backtrace = Backtrace::force_capture().to_string();
            // Guard against a poisoned/borrowed cell: if recording fails we still
            // unwind and `run_handler` falls back to a default message.
            let _ = PANIC_SLOT.try_with(|slot| {
                if let Ok(mut slot) = slot.try_borrow_mut() {
                    *slot = Some((message, backtrace));
                }
            });
        }));
    });
}

/// Run a single handler under `catch_unwind`, converting any of the five failure
/// modes into a [`RunnerError`]. A panic is captured (message + backtrace) via a
/// process-wide panic hook (installed once) and surfaced as [`RunnerError::Panic`]
/// (boundaries.md §2).
///
/// The captured panic info lives in a per-thread [`PANIC_SLOT`]: the hook writes the
/// panicking thread's info to its own slot, and this function — running on that same
/// thread after `catch_unwind` returns `Err` — reads it back. Because each thread
/// owns its slot, concurrent panics on other threads (e.g. parallel tests) cannot
/// clobber this call's capture.
fn run_handler(handler: HandlerFn, input: &[u8]) -> Result<Vec<u8>, RunnerError> {
    install_panic_hook();
    // Clear this thread's slot so a stale capture from a prior call can't leak in.
    PANIC_SLOT.with(|slot| {
        if let Ok(mut slot) = slot.try_borrow_mut() {
            *slot = None;
        }
    });
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| handler(input)));
    match outcome {
        Ok(result) => result,
        Err(_) => {
            let captured =
                PANIC_SLOT.with(|slot| slot.try_borrow_mut().ok().and_then(|mut s| s.take()));
            let (message, backtrace) = captured
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
    run_cli_with_configs(registry, &[])
}

/// [`run_cli`] plus a per-entrypoint config map for the additive `--describe`
/// subcommand. The configs are read ONLY by `--describe`; the FROZEN
/// `--entrypoint` dispatch ignores them entirely, so this is a strict superset of
/// [`run_cli`] (boundaries.md §2; P9 §A).
///
/// The macro runner bin uses this with [`from_inventory_with_configs`] so real
/// decorator gpu/timeout/cache flow into `--describe`. The manual-registry runner
/// can pass `&[]` (empty configs) — `--describe` then emits each entrypoint with
/// the default (all-`None`) config, which is correct (a manual registry carries no
/// decorator config).
pub fn run_cli_with_configs(registry: Registry, configs: &[(&'static str, FunctionConfig)]) -> i32 {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    run_cli_with_args_and_configs(registry, configs, &argv, &mut std::io::stdout())
}

/// The testable core of [`run_cli`]: takes explicit argv and an output sink so the
/// envelope can be captured in unit tests. Diagnostics still go to stderr.
///
/// FROZEN: this is the zero-config wrapper over
/// [`run_cli_with_args_and_configs`]. With empty configs the `--describe` branch
/// still fires (emitting default config per name), and the `--entrypoint` dispatch
/// is byte-identical to before P9.
pub fn run_cli_with_args<W: std::io::Write>(
    registry: Registry,
    argv: &[String],
    out: &mut W,
) -> i32 {
    run_cli_with_args_and_configs(registry, &[], argv, out)
}

/// The config-carrying core. ADDITIVE over [`run_cli_with_args`]: when the FIRST
/// argv token is `--describe`, emit the registry manifest (entrypoints + each
/// [`FunctionConfig`]) as ONE JSON object to `out` and exit `0`. Otherwise dispatch
/// EXACTLY as the frozen `--entrypoint` path (the configs are ignored).
///
/// `--describe` can never collide with the frozen `--entrypoint <name> --input-*`
/// shape (the first token differs), so the protocol/envelope/five-error-kinds are
/// byte-identical when `--describe` is absent (P9 §A.2).
pub fn run_cli_with_args_and_configs<W: std::io::Write>(
    registry: Registry,
    configs: &[(&'static str, FunctionConfig)],
    argv: &[String],
    out: &mut W,
) -> i32 {
    if argv.first().map(String::as_str) == Some("--describe") {
        return emit_describe(&registry, configs, out);
    }
    run_cli_dispatch(registry, argv, out)
}

/// The FROZEN `--entrypoint` dispatch body (formerly the entire `run_cli_with_args`
/// body). Byte-identical to pre-P9: parse the three runner flags, run exactly one
/// handler under `catch_unwind`, and print exactly one JSON envelope.
fn run_cli_dispatch<W: std::io::Write>(registry: Registry, argv: &[String], out: &mut W) -> i32 {
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

/// The `--describe` manifest schema version (P9 §A.3). `@1` is `describe@1`. The CLI
/// warns-and-proceeds on an unknown minor; hard-errors on an unknown major.
const DESCRIBE_SCHEMA: &str = "modal-rust/describe@1";

/// The `--describe` manifest: the schema tag + the registry entrypoints, each with
/// its [`FunctionConfig`] (P9 §A.3). A private `#[derive(Serialize)]` view — `serde`
/// is already a runtime dep, so no new dependency.
#[derive(Serialize)]
struct DescribeManifest<'a> {
    /// Version tag, e.g. `"modal-rust/describe@1"`.
    schema: &'a str,
    /// Entrypoints, sorted by name (BTreeMap order — deterministic).
    entrypoints: Vec<DescribeEntry<'a>>,
}

/// One entrypoint in the `--describe` manifest: its name + serialized config.
#[derive(Serialize)]
struct DescribeEntry<'a> {
    /// The entrypoint (registry key) name.
    name: &'a str,
    /// The per-entrypoint config, mirroring [`FunctionConfig`] EXACTLY.
    config: DescribeConfig,
}

/// The serialized view of [`FunctionConfig`] for the manifest (P9 §A.3): `gpu:
/// string|null`, `timeout_secs: u32|null`, `cache: bool|null`, plus the additive
/// `secrets: [string]` and `volumes: [[mount_path, name]]`. A dedicated view
/// (rather than `#[derive(Serialize)]` on `FunctionConfig`) keeps the `&'static`
/// lifetimes out of the public type and pins the wire shape here. The two new
/// fields are EMPTY for every pre-existing manifest, so the schema stays `@1`.
#[derive(Serialize)]
struct DescribeConfig {
    gpu: Option<&'static str>,
    timeout_secs: Option<u32>,
    cache: Option<bool>,
    secrets: &'static [&'static str],
    volumes: &'static [(&'static str, &'static str)],
}

impl From<&FunctionConfig> for DescribeConfig {
    fn from(c: &FunctionConfig) -> Self {
        DescribeConfig {
            gpu: c.gpu,
            timeout_secs: c.timeout_secs,
            cache: c.cache,
            secrets: c.secrets,
            volumes: c.volumes,
        }
    }
}

/// Emit the `--describe` manifest: iterate `registry.names()` (sorted BTreeMap
/// order, the authoritative entrypoint set) and, for each, look up its
/// [`FunctionConfig`] in `configs`, falling back to `FunctionConfig::default()`
/// (all-`None`) when absent. Writes EXACTLY ONE JSON object to `out` and returns
/// `0`; a serialize/write failure goes to stderr and returns `1` (P9 §A.2).
fn emit_describe<W: std::io::Write>(
    registry: &Registry,
    configs: &[(&'static str, FunctionConfig)],
    out: &mut W,
) -> i32 {
    let default = FunctionConfig::default();
    let entrypoints: Vec<DescribeEntry<'_>> = registry
        .names()
        .map(|&name| {
            let config = configs
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, c)| c)
                .unwrap_or(&default);
            DescribeEntry {
                name,
                config: DescribeConfig::from(config),
            }
        })
        .collect();
    let manifest = DescribeManifest {
        schema: DESCRIBE_SCHEMA,
        entrypoints,
    };
    match serde_json::to_string(&manifest) {
        Ok(s) => {
            if let Err(e) = writeln!(out, "{s}") {
                eprintln!("modal_runner: failed to write describe manifest: {e}");
                return 1;
            }
            0
        }
        Err(e) => {
            eprintln!("modal_runner: failed to serialize describe manifest: {e}");
            1
        }
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
        // No `RUST_BACKTRACE` mutation: the hook uses `Backtrace::force_capture()`,
        // which always captures, and the per-thread slot means this assertion is
        // robust even when other tests panic concurrently (boundaries.md §2/§3).
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
    fn describe_emits_manifest_with_configs() {
        // ADDITIVE: `--describe` as the FIRST token emits the manifest (entrypoints +
        // per-entrypoint config) and returns 0. The frozen `--entrypoint` dispatch is
        // untouched (the other tests above stay green).
        let configs: &[(&'static str, FunctionConfig)] = &[(
            "add",
            FunctionConfig {
                gpu: Some("T4"),
                timeout_secs: Some(1800),
                cache: Some(false),
                secrets: &["my-secret"],
                volumes: &[("/data", "my-vol")],
            },
        )];
        let argv = vec!["--describe".to_string()];
        let mut buf = Vec::new();
        let code = run_cli_with_args_and_configs(registry(), configs, &argv, &mut buf);
        assert_eq!(code, 0);
        let v: serde_json::Value = serde_json::from_slice(&buf).expect("one JSON manifest");
        assert_eq!(v["schema"], "modal-rust/describe@1");
        let eps = v["entrypoints"].as_array().expect("entrypoints array");
        // Sorted BTreeMap order: add, bad_encode, fail_anyhow, fail_structured, will_panic.
        let names: Vec<&str> = eps.iter().map(|e| e["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            vec![
                "add",
                "bad_encode",
                "fail_anyhow",
                "fail_structured",
                "will_panic"
            ]
        );
        // `add` carries the supplied decorator config.
        let add = &eps[0];
        assert_eq!(add["name"], "add");
        assert_eq!(add["config"]["gpu"], "T4");
        assert_eq!(add["config"]["timeout_secs"], 1800);
        assert_eq!(add["config"]["cache"], false);
        // Secrets + user volumes ride the manifest additively.
        assert_eq!(add["config"]["secrets"], serde_json::json!(["my-secret"]));
        assert_eq!(
            add["config"]["volumes"],
            serde_json::json!([["/data", "my-vol"]])
        );
        // An entrypoint absent from `configs` falls back to the all-null default.
        let bad = &eps[1];
        assert_eq!(bad["name"], "bad_encode");
        assert_eq!(bad["config"]["gpu"], serde_json::Value::Null);
        assert_eq!(bad["config"]["timeout_secs"], serde_json::Value::Null);
        assert_eq!(bad["config"]["cache"], serde_json::Value::Null);
        // The default config has EMPTY secrets/volumes (wire-identical to before).
        assert_eq!(bad["config"]["secrets"], serde_json::json!([]));
        assert_eq!(bad["config"]["volumes"], serde_json::json!([]));
    }

    #[test]
    fn function_config_default_has_empty_secrets_and_volumes() {
        // The bare-decorator default (and `FunctionConfig::new()` const ctor) carry
        // EMPTY secrets/volumes, so a function with no decorator extras is wire-
        // identical to before this addition.
        let d = FunctionConfig::default();
        assert!(d.secrets.is_empty());
        assert!(d.volumes.is_empty());
        let c = FunctionConfig::new();
        assert!(c.secrets.is_empty());
        assert!(c.volumes.is_empty());
        assert_eq!(d, c, "Default and new() agree");
    }

    #[test]
    fn describe_empty_configs_emits_default_config() {
        // The manual-registry path passes empty configs; every entrypoint then gets
        // the default (all-None) config in the manifest.
        let argv = vec!["--describe".to_string()];
        let mut buf = Vec::new();
        let code = run_cli_with_args_and_configs(registry(), &[], &argv, &mut buf);
        assert_eq!(code, 0);
        let v: serde_json::Value = serde_json::from_slice(&buf).expect("one JSON manifest");
        let add = &v["entrypoints"][0];
        assert_eq!(add["name"], "add");
        assert_eq!(add["config"]["gpu"], serde_json::Value::Null);
    }

    #[test]
    fn describe_does_not_affect_frozen_entrypoint_path() {
        // Sanity: a normal `--entrypoint` run through the config-carrying entry is
        // byte-identical to the frozen path (configs ignored).
        let argv: Vec<String> = ["--entrypoint", "add", "--input-json", r#"{"a":40,"b":2}"#]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut buf = Vec::new();
        let code = run_cli_with_args_and_configs(registry(), &[], &argv, &mut buf);
        let v: serde_json::Value = serde_json::from_slice(&buf).expect("one JSON envelope");
        assert_eq!(code, 0);
        assert_eq!(v, serde_json::json!({"ok": true, "value": {"sum": 42}}));
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
