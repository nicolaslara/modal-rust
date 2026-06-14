//! Additive proc-macro sugar for modal-rust (ergonomics E1).
//!
//! [`macro@function`] is an attribute macro that, applied to a handler like
//! `pub fn add(input: AddInput) -> anyhow::Result<AddOutput>`, expands to:
//!
//! 1. the **unchanged** original function, and
//! 2. one facade-owned `inventory::submit!` registration whose handler is the
//!    SAME monomorphized `modal_rust_runtime::typed!(add)` wrapper `fn` pointer
//!    the manual `Registry::new().function("add", typed!(add))` builder produces,
//!    paired atomically with the control-plane metadata.
//!
//! `modal_rust::registry_from_inventory()` then collects every submission into the
//! SAME `BTreeMap<&'static str, HandlerFn>` as the manual path (boundaries.md §3). The
//! macro is **purely additive**: it changes neither the runner CLI protocol, the
//! five-kind `RunnerError` envelope, nor the `HandlerFn` / `Registry` / `typed!`
//! shapes. The manual builder path stays fully intact and both converge on one
//! dispatch path: `name -> typed! wrapper (fn pointer) -> JSON bytes in -> JSON
//! bytes out`.
//!
//! ## Name
//!
//! The entrypoint name defaults to the function's own name; override it with
//! `#[modal_rust::function(name = "...")]`.
//!
//! ## Optional per-function config
//!
//! `#[modal_rust::function(gpu = "T4", timeout = 1800, cache = false, cpu = 2.0,
//! memory = 4096, retries = 3, schedule = Cron("0 9 * * 1"))]` records a facade-owned
//! [`modal_rust::FunctionConfig`] in the same inventory record as the handler. This is
//! control-plane metadata only: the facade reads it when creating the Modal function
//! (`Resources.gpu_config`, `timeout_secs`, `Resources.milli_cpu`,
//! `Resources.memory_mb`, `retry_policy`, `schedule`), while runtime dispatch sees only
//! `name` + `HandlerFn`. `cpu` is CPU CORES (a float, e.g. `2.0`; resolved to
//! `milli_cpu = int(1000 * cpu)`, mirroring Modal); `memory` is MEBIBYTES (an int);
//! `retries` is the automatic retry COUNT (an int; mirrors Modal's bare-int `retries`
//! kwarg → a fixed-interval retry policy); `schedule` is a run cadence for a DEPLOYED
//! function — `Cron("expr"[, "tz"])` or `Period(days = 1, hours = 4, ..)`, mirroring
//! Modal's `Cron`/`Period`. `min_containers`/`max_containers`/`buffer_containers` (ints)
//! and `scaledown_window` (idle seconds, int) are AUTOSCALER controls mirroring Modal's
//! `min_containers`/`max_containers`/`buffer_containers`/`scaledown_window` kwargs: they
//! ride into `FunctionCreate.autoscaler_settings` (warm-capacity floor/ceiling/buffer +
//! scale-to-zero window).
//!
//! ## User-facing secrets + volumes
//!
//! `#[modal_rust::function(secrets = ["my-secret", "other"])]` attaches named Modal
//! secrets: the facade resolves each name to a `secret_id`, attaches it to
//! `FunctionCreate.secret_ids`, and the secret's key/values are injected as ENV
//! VARS in the container (readable via `std::env`).
//!
//! `#[modal_rust::function(volumes = ["/data=my-vol", "/models=weights"])]` attaches
//! user Modal volumes: each `"MOUNT=NAME"` string is parsed into a `(mount_path,
//! name)` pair; the facade resolves `name` via `volume_get_or_create` and mounts it
//! at `mount_path` — a SEPARATE mount from the P6 cargo cache (`/cache`), so both
//! coexist. The string-list `"MOUNT=NAME"` form is used because map syntax is hard
//! to parse in attribute position. Both lists default EMPTY.
//!
//! ## async
//!
//! `async fn` handlers are detected and rejected with a clear `compile_error!`:
//! the reserved `typed_async!` shape (boundaries.md §3) is **not yet implemented**
//! in `modal-rust-runtime`, so emitting it would not compile. The sync path is
//! unaffected. When `typed_async!` lands, this arm switches from a diagnostic to
//! emitting `typed_async!(..)` with the same `HandlerFn` shape.
//!
//! ## Two signature styles (auto-I/O ergonomics)
//!
//! The frozen wire argument is ONE named JSON object (boundaries.md §3). The macro
//! supports the user writing EITHER style of handler signature:
//!
//! - **Mode A — EXPLICIT (byte-identical to before):** a single param whose type is
//!   a bare user struct path — `fn add(input: AddInput) -> Result<AddOutput>`. The
//!   user's struct IS the wire input. Emission is unchanged: the original fn plus an
//!   `inventory::submit!` of `typed!(add)`.
//! - **Mode B — PLAIN signature (auto-generated I/O):** anything else — multiple
//!   params, a single primitive/standalone param, or a no-arg fn:
//!   `fn add(a: i64, b: i64) -> anyhow::Result<i64>`. The macro GENERATES a named
//!   input type from the params (`pub mod add { pub struct Input { a, b }; pub type
//!   Output = i64; }`, both `Serialize + Deserialize`), a private SPREAD shim
//!   `fn(add::Input) -> _ { add(in.a, in.b) }` registered via the UNCHANGED
//!   `typed!(shim)`, and a typed positional `App` extension method
//!   `app.add(2, 3).local()/.remote()/.spawn()/.map()` (an `AddCall` trait
//!   implemented for the facade `App`). The wire input is the generated `Input`
//!   (still a named JSON object `{"a":2,"b":3}`); the wire output is the return
//!   type's inner `Ok` (`{"value":5}`). NOTHING about the runner protocol /
//!   `HandlerFn` / `typed!` changes — only the REGISTERED fn is the generated shim.
//!
//! The classifier is purely syntactic (a proc-macro cannot resolve types): a single
//! param is Mode A iff its type is a bare `Type::Path` (no generics) whose last
//! segment is NOT a primitive scalar (`i64`, `String`, …). See the inline classifier
//! for the exact rule.
//!
//! ### Single-dep path routing (downstream `Cargo.toml`)
//!
//! Every runtime / `inventory` path the macro emits is routed THROUGH the facade so a
//! macro-using crate needs ONLY `modal-rust` (plus `serde`/`anyhow` for the handler
//! types). The macro resolves the facade's import name with `proc-macro-crate` at
//! expansion time and emits:
//! - `#facade::{Registration, FunctionConfig}` — the facade-owned atomic
//!   discovery record and its control-plane config.
//! - `#facade::__private::runtime::typed!` — the frozen runner wrapper macro.
//! - `#facade::__private::inventory::submit!` — `inventory`, re-exported under
//!   `__private::inventory`.
//! - `#facade::{App, TypedCall}` for the Mode B typed `app.<fn>(..)` methods.
//! - `::serde::{Serialize, Deserialize}` for the generated `Input` derives — `serde`
//!   routes itself, and every macro-using crate already has `serde` with `derive`, so
//!   this is no new dep.
//!
//! `#facade` is whatever extern name the user crate gives the `modal-rust` package:
//! the default `modal_rust`, the in-workspace `crate` (`FoundCrate::Itself`), OR a
//! rename. The canonical `examples/add-macro` keeps `extern crate modal_rust_macros as
//! modal_rust;` (so `#[modal_rust::function]` is spellable) and renames the facade
//! `modal_rust_facade = { package = "modal-rust" }` to dodge that shadow;
//! `proc-macro-crate` returns `modal_rust_facade`, so EVERY routed path resolves and
//! the crate carries no direct `modal-rust-runtime` / `inventory` dep.
//!
//! ### Bringing the typed methods into scope
//!
//! The `app.<fn>(..)` methods live on a per-fn extension trait (`<Pascal>Call`, e.g.
//! `AddCall`) implemented for the facade `App` (one trait per fn keeps coherence
//! trivial). The trait must be in scope at the call site; the ergonomic one-import
//! path is a glob over the user crate:
//!
//! ```ignore
//! use my_crate::*;             // brings every `<Pascal>Call` into scope (one use)
//! // or, per-fn:
//! use my_crate::AddCall;
//! app.add(2, 3).remote().await?;
//! ```
//!
//! Mechanically split (M1): the `#[proc_macro_*]` entrypoints MUST live in this
//! root (proc-macro crates may only export from `lib.rs`), so they stay here as
//! thin delegates into [`args`] (the shared decorator grammar), [`emit`] (the
//! `#[function]`/`#[endpoint]` expansion + `Registration` emission), [`cls`] (the
//! stateful-class walker/validator/emitter), and [`specs`] (the decorator-value
//! canonicalizers). The tests stay here, importing from the new paths.

mod args;
mod cls;
mod emit;
mod specs;

use proc_macro::TokenStream;
use proc_macro_crate::{crate_name, FoundCrate};
use quote::{format_ident, quote};

use args::HandlerKind;

/// The Cargo package name of the facade crate the macro routes ALL paths through.
const FACADE_PACKAGE: &str = "modal-rust";

/// The separator joining a `Cls` class name and a method name into the entrypoint /
/// Modal object tag (`"Embedder" + SEP + "embed" = "Embedder.embed"`). A SINGLE named
/// constant so the live-spike fallback (if Modal's create RPC rejects a dotted object
/// tag) is exactly ONE edit here. The dot round-trips `sanitize_object_tag`
/// (allowlist `alnum | '_' | '-' | '.'`); the locked fallback is `"-"` (also in the
/// allowlist). See cls-design.md §3.4 / cls-devx-design.md §3.
const CLS_ENTRYPOINT_SEPARATOR: &str = ".";

/// Resolve the leading path to the `modal-rust` FACADE crate as the USER crate spells
/// it, so the macro can route every emitted path through the facade
/// (`#facade::__private::runtime::…`, `#facade::__private::inventory::…`,
/// `#facade::{App, TypedCall}`). This is the serde_derive / clap_derive single-dep
/// pattern: the user needs ONLY `modal-rust`, and `proc-macro-crate` finds whatever
/// extern name it carries.
///
/// - [`FoundCrate::Itself`] — the macro is expanding INSIDE the `modal-rust` crate
///   itself (e.g. a doctest in the facade): emit `crate`.
/// - [`FoundCrate::Name(name)`] — the facade is a dependency under `name` (the default
///   `modal_rust`, OR a rename such as the `modal_rust_facade` alias the canonical
///   `examples/add-macro` uses to dodge the `extern crate modal_rust_macros as
///   modal_rust` shadow): emit `::name`.
///
/// On a resolution failure (no `modal-rust` dep found) fall back to `::modal_rust` —
/// the unshadowed default name — which yields a clear "unresolved import" error
/// pointing the user at the missing dependency.
fn facade_path() -> proc_macro2::TokenStream {
    match crate_name(FACADE_PACKAGE) {
        Ok(FoundCrate::Itself) => quote!(crate),
        Ok(FoundCrate::Name(name)) => {
            let ident = format_ident!("{}", name);
            quote!(::#ident)
        }
        Err(_) => quote!(::modal_rust),
    }
}

/// Attribute macro that registers a handler with the modal-rust runner via
/// `inventory`, producing the SAME registry shape as the manual `typed!` path.
///
/// See the crate-level docs for the full contract. Usage:
///
/// ```ignore
/// // Mode A — EXPLICIT (single user-struct param; byte-identical to before):
/// #[modal_rust::function]                  // name defaults to "add"
/// pub fn add(input: AddInput) -> anyhow::Result<AddOutput> { /* ... */ }
///
/// // Mode B — PLAIN signature (auto-generated `add::Input`/`add::Output` + typed
/// // `app.add(2, 3).remote()`):
/// #[modal_rust::function]
/// pub fn add(a: i64, b: i64) -> anyhow::Result<i64> { Ok(a + b) }
///
/// #[modal_rust::function(name = "add")]    // explicit name override (either mode)
/// pub fn add(input: AddInput) -> anyhow::Result<AddOutput> { /* ... */ }
/// ```
#[proc_macro_attribute]
pub fn function(attr: TokenStream, item: TokenStream) -> TokenStream {
    emit::expand_handler(attr, item, HandlerKind::Function)
}

/// Attribute macro that registers a WEB-ENDPOINT handler: everything
/// [`macro@function`] does (the same auto-IO Mode A/B, the same `Registration` +
/// typed `app.<fn>(..)` surface, the same decorator vocabulary — ONE shared
/// parse+emit path), PLUS the web-endpoint marker
/// (`webhook_method`/`webhook_requires_proxy_auth`) on the emitted facade
/// `FunctionConfig`. On `modal-rust deploy` the function ALSO gets an HTTP URL
/// (Modal `WEBHOOK_TYPE_FUNCTION`); `modal-rust run` and the typed call path are
/// unchanged (the facade suppresses the webhook on the RUN boundary).
///
/// `method` is REQUIRED — one of `"GET" | "POST" | "PUT" | "DELETE" | "PATCH"`
/// (explicit, no silent default). `requires_proxy_auth = true` opts into Modal
/// proxy-auth (the `Modal-Key`/`Modal-Secret` header pair); the default is PUBLIC
/// (matches Modal). Every other argument is the shared `#[function]` vocabulary
/// (`gpu`/`timeout`/`secrets`/`volumes`/…).
///
/// ```ignore
/// #[modal_rust::endpoint(method = "POST", gpu = "T4", timeout = 600)]
/// fn add(a: i64, b: i64) -> anyhow::Result<i64> { Ok(a + b) }
/// // deploy ⇒ POST {"a":40,"b":2} -> 42 at the printed URL;
/// // app.add(40, 2).remote() still works (the dual surface).
/// ```
///
/// v0 limits: free fns only — `#[endpoint]` on a `#[cls]` method is a compile
/// error (stateful endpoints are a follow-up); the URL is DEPLOY-only.
#[proc_macro_attribute]
pub fn endpoint(attr: TokenStream, item: TokenStream) -> TokenStream {
    emit::expand_handler(attr, item, HandlerKind::Endpoint)
}

/// Attribute macro that registers a LONG-RUNNING WEB SERVER handler: a function that
/// LAUNCHES an HTTP server bound to `port` and BLOCKS, serving forever. Unlike
/// [`macro@endpoint`] (a request/response `fn(In) -> Out` mapped onto a Modal FUNCTION
/// webhook), `#[web_server]` is a RAW PORT PROXY (Modal `WEBHOOK_TYPE_WEB_SERVER`): on
/// `modal-rust deploy` Modal assigns a URL and forwards ALL traffic to the bound port.
///
/// The handler signature is `(port: u16) -> ()` / `-> anyhow::Result<()>` (sync or
/// `async`). It does NOT use the `fn(&[u8]) -> Vec<u8>` request/response shape — it owns
/// the socket and serves until the container stops.
///
/// `port` is REQUIRED (the TCP port the server binds; no silent default).
/// `startup_timeout = <secs>` is OPTIONAL (how long Modal waits for the port to come
/// up). Every other argument is the shared `#[function]` vocabulary
/// (`gpu`/`memory`/`timeout`/`image`/`secrets`/`volumes`/…).
///
/// ```ignore
/// #[modal_rust::web_server(port = 3000, gpu = "T4")]
/// async fn serve(port: u16) -> anyhow::Result<()> {
///     burn_lm_http::App::new(port).serve().await.map_err(|e| anyhow::anyhow!(e))
/// }
/// // deploy ⇒ Modal prints the assigned URL and proxies traffic to port 3000.
/// ```
///
/// v0 limits: DEPLOY-only (the URL is assigned by `modal-rust deploy`); free fns only.
#[proc_macro_attribute]
pub fn web_server(attr: TokenStream, item: TokenStream) -> TokenStream {
    emit::expand_handler(attr, item, HandlerKind::WebServer)
}

/// `#[modal_rust::cls(<class config>)]` — the load-once stateful-class attribute.
///
/// Applied to an `impl` block, it parses the inner `#[enter]` / `#[method]` / `#[exit]`
/// markers (inert; consumed by this macro) and, in ONE expansion, emits Shape A
/// (cls-design.md): each `#[method]` becomes its OWN entrypoint `"<Class>.<method>"` in
/// the frozen `Registry` (a `Registration` byte-identical to a free fn except the
/// dotted name + a singleton-dispatch shim); the entered struct is a process-lifetime
/// `OnceLock` singleton built by the `#[enter]` body and reused across calls via
/// `modal_runner --serve`; and a borrowing `<Class>Handle` + `<Class>Cls` extension
/// trait give the caller `app.<class>().<method>(..).local()/.remote()`.
///
/// ```ignore
/// use modal_rust::cls;
/// pub struct Embedder { model: Model }
/// #[cls(gpu = "T4", timeout = 600)]
/// impl Embedder {
///     #[enter]               fn load() -> anyhow::Result<Self> { Ok(Embedder { model: Model::load()? }) }
///     #[method(gpu = "A10G")] fn embed(&self, text: String) -> anyhow::Result<Vec<f32>> { Ok(self.model.encode(&text)) }
///     #[method]              fn dim(&self) -> anyhow::Result<usize> { Ok(self.model.dim()) }
/// }
/// ```
#[proc_macro_attribute]
pub fn cls(attr: TokenStream, item: TokenStream) -> TokenStream {
    cls::expand_cls(attr, item)
}

/// Generate the modal-rust runner `main()` — the whole `src/bin/modal_runner.rs`
/// body in ONE line, so the user never writes a runner `main()` and never names the
/// `#[doc(hidden)] __private` runtime re-exports (the serde_derive pattern: the
/// `__private` usage lives only in GENERATED code).
///
/// Usage — the user's `src/bin/modal_runner.rs` is exactly:
///
/// ```ignore
/// // Functions live in the package's LIBRARY crate (the usual layout): name it so
/// // its `inventory` submissions are linked into this runner binary.
/// modal_rust::modal_runner!(my_crate);
///
/// // Functions live in THIS file (a single-binary crate, no separate lib): no name
/// // needed — the submissions are already in this crate.
/// modal_rust::modal_runner!();
/// ```
///
/// This expands to the SAME runner the hand-written bin produced: it (optionally
/// links the lib crate then) assembles the registry + decorator configs from the
/// facade's atomic inventory records and runs the FROZEN runner CLI protocol,
/// mirroring `ok` in the process exit code.
///
/// ## Why the optional crate name
///
/// A `[[bin]]` target does NOT automatically link its package's library crate in
/// Cargo, so a runner in `src/bin/modal_runner.rs` would not see the lib's
/// `inventory::submit!` registrations unless it references the lib. Passing the lib
/// crate's name emits a `use <crate> as _;` link — the one fact the macro cannot
/// infer (it has no knowledge of the host package's lib name). This keeps the body a
/// single line while removing BOTH the `main()` boilerplate AND the `__private`
/// leak.
///
/// Every emitted path is routed through the resolved facade
/// (`#facade::__private::runtime::…`), so a crate that writes `modal_runner!()`
/// needs ONLY the `modal-rust` dependency. The host crate still needs the
/// `[[bin]] name = "modal_runner"` target (the RUN/DEPLOY wrapper builds
/// `--bin modal_runner`).
#[proc_macro]
pub fn modal_runner(input: TokenStream) -> TokenStream {
    emit::expand_modal_runner(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::*;
    use crate::cls::*;
    use crate::emit::*;
    use crate::specs::*;
    use syn::{LitStr, ReturnType, Type};

    fn ty(src: &str) -> Type {
        syn::parse_str(src).expect("valid type")
    }

    #[test]
    fn mode_a_selects_bare_user_struct_paths() {
        // A single bare non-scalar path is the EXPLICIT form (Mode A): used as-is,
        // byte-identical to before. Covers plain, qualified, and leading-`::` paths.
        assert!(is_mode_a_param_type(&ty("AddInput")));
        assert!(is_mode_a_param_type(&ty("crate::AddInput")));
        assert!(is_mode_a_param_type(&ty("mymod::Req")));
        assert!(is_mode_a_param_type(&ty("::foo::Bar")));
    }

    #[test]
    fn mode_b_selects_scalars_generics_refs_tuples_arrays() {
        // Denylisted scalars -> Mode B (generate), even as a single param.
        for s in SCALAR_DENYLIST {
            assert!(
                !is_mode_a_param_type(&ty(s)),
                "scalar {s} must force Mode B (generate)"
            );
        }
        // Generic paths, references, tuples, arrays -> Mode B.
        assert!(!is_mode_a_param_type(&ty("Vec<u8>")));
        assert!(!is_mode_a_param_type(&ty("Option<i64>")));
        assert!(!is_mode_a_param_type(&ty(
            "std::collections::HashMap<String, i64>"
        )));
        assert!(!is_mode_a_param_type(&ty("&str")));
        assert!(!is_mode_a_param_type(&ty("&[u8]")));
        assert!(!is_mode_a_param_type(&ty("(i64, i64)")));
        assert!(!is_mode_a_param_type(&ty("[u8; 4]")));
    }

    #[test]
    fn result_ok_type_extracts_inner_ok() {
        let parse_out = |src: &str| -> String {
            let sig: syn::Signature = syn::parse_str(&format!("fn f() {src}")).unwrap();
            result_ok_type(&sig.output).to_string()
        };
        assert_eq!(parse_out("-> anyhow::Result<i64>"), "i64");
        assert_eq!(parse_out("-> Result<i64, MyErr>"), "i64");
        assert_eq!(parse_out("-> Result<Vec<u8>, E>"), "Vec < u8 >");
        // No return -> unit fallback (a non-Result handler is a `typed!` compile
        // error anyway, so this fallback is never registered).
        assert_eq!(parse_out(""), "()");
    }

    #[test]
    fn cpu_cores_resolve_to_milli_cores_like_modal() {
        // `parse_cpu_to_milli` mirrors Modal's `milli_cpu = int(1000 * cpu)`. Drive it
        // through `syn::parse` so we exercise the real ParseStream path.
        let milli = |src: &str| -> u32 {
            syn::parse::Parser::parse_str(parse_cpu_to_milli, src).expect("valid cpu")
        };
        assert_eq!(milli("2.0"), 2000); // float cores
        assert_eq!(milli("0.5"), 500); // fractional core
        assert_eq!(milli("1"), 1000); // bare int cores
        assert_eq!(milli("0.25"), 250);
        // Truncation toward zero (Python `int()`), not rounding.
        assert_eq!(milli("1.9995"), 1999);
        // Negative cores are rejected.
        assert!(syn::parse::Parser::parse_str(parse_cpu_to_milli, "-1.0").is_err());
    }

    #[test]
    fn pascal_case_handles_underscores() {
        assert_eq!(to_pascal_case("add"), "Add");
        assert_eq!(to_pascal_case("add_plain"), "AddPlain");
        assert_eq!(to_pascal_case("add_gpu"), "AddGpu");
        assert_eq!(to_pascal_case("a_b_c"), "ABC");
        assert_eq!(to_pascal_case("already"), "Already");
    }

    #[test]
    fn snake_case_lowers_pascal_class_idents() {
        // The `app.<class>()` accessor name.
        assert_eq!(to_snake_case("Embedder"), "embedder");
        assert_eq!(to_snake_case("MyEmbedder"), "my_embedder");
        assert_eq!(to_snake_case("HTTPClient"), "h_t_t_p_client");
        assert_eq!(to_snake_case("A"), "a");
    }

    #[test]
    fn cls_entrypoint_separator_round_trips_object_tag() {
        // The dotted entrypoint name "<Class>.<method>" must survive the facade's
        // `sanitize_object_tag` allowlist unchanged (alnum | '_' | '-' | '.'), or the
        // live create RPC would get a corrupted tag. We reproduce that allowlist HERE
        // (the facade is a sibling crate the macro cannot depend on) and assert the
        // joined name is a fixed point.
        let join = |class: &str, method: &str| format!("{class}{CLS_ENTRYPOINT_SEPARATOR}{method}");
        let sanitize = |s: &str| -> String {
            s.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect()
        };
        for (class, method) in [
            ("Embedder", "embed"),
            ("Embedder", "dim"),
            ("My_Cls", "do_it"),
        ] {
            let name = join(class, method);
            assert_eq!(
                sanitize(&name),
                name,
                "dotted entrypoint {name:?} must round-trip sanitize_object_tag"
            );
        }
        // Guard the chosen default: the separator is the dot (one-edit fallback to "-").
        assert_eq!(CLS_ENTRYPOINT_SEPARATOR, ".");
    }

    fn enter_output(src: &str) -> ReturnType {
        let sig: syn::Signature = syn::parse_str(&format!("fn load() {src}")).unwrap();
        sig.output
    }

    #[test]
    fn classify_enter_return_distinguishes_fallible_and_infallible() {
        let cls: syn::Ident = syn::parse_str("Embedder").unwrap();
        // Infallible -> Some(false).
        assert_eq!(
            classify_enter_return(&enter_output("-> Self"), &cls),
            Some(false)
        );
        assert_eq!(
            classify_enter_return(&enter_output("-> Embedder"), &cls),
            Some(false)
        );
        // Fallible -> Some(true).
        assert_eq!(
            classify_enter_return(&enter_output("-> anyhow::Result<Self>"), &cls),
            Some(true)
        );
        assert_eq!(
            classify_enter_return(&enter_output("-> Result<Embedder, MyErr>"), &cls),
            Some(true)
        );
        // Not a valid enter return -> None.
        assert_eq!(classify_enter_return(&enter_output(""), &cls), None);
        assert_eq!(classify_enter_return(&enter_output("-> usize"), &cls), None);
        assert_eq!(
            classify_enter_return(&enter_output("-> Result<usize, E>"), &cls),
            None
        );
    }

    #[test]
    fn cls_config_merge_overrides_field_by_field() {
        let class = DecoratorConfig {
            gpu: Some(syn::parse_str::<LitStr>(r#""T4""#).unwrap()),
            timeout_secs: Some(600),
            ..Default::default()
        };
        let method = DecoratorConfig {
            gpu: Some(syn::parse_str::<LitStr>(r#""A10G""#).unwrap()),
            ..Default::default()
        };
        let merged = class.merge_over(&method);
        // Method gpu wins; class timeout is inherited.
        assert_eq!(merged.gpu.as_ref().unwrap().value(), "A10G");
        assert_eq!(merged.timeout_secs, Some(600));
        // A bare #[method] (empty override) inherits the whole class config.
        let inherited = class.merge_over(&DecoratorConfig::default());
        assert_eq!(inherited.gpu.as_ref().unwrap().value(), "T4");
        assert_eq!(inherited.timeout_secs, Some(600));
    }

    #[test]
    fn cls_kind_reads_the_decorator_vocabulary() {
        let tokens: proc_macro2::TokenStream =
            syn::parse_str(r#"gpu = "T4", timeout = 600, secrets = ["a", "b"]"#).unwrap();
        let cfg = parse_decorator_config(tokens, HandlerKind::Cls).expect("valid config");
        assert_eq!(cfg.gpu.as_ref().unwrap().value(), "T4");
        assert_eq!(cfg.timeout_secs, Some(600));
        assert_eq!(
            cfg.secrets.as_deref().unwrap(),
            ["a".to_string(), "b".to_string()]
        );
        // `name =` is rejected on a class/method.
        let bad: proc_macro2::TokenStream = syn::parse_str(r#"name = "x""#).unwrap();
        assert!(parse_decorator_config(bad, HandlerKind::Cls).is_err());
    }

    #[test]
    fn cls_kind_rejects_endpoint_only_keys_via_the_shared_allow_set() {
        // `#[cls]`/`#[method]` parse through the SHARED grammar (M2); the
        // `HandlerKind::Cls` allow-set rejects the endpoint-only keys with a pointed
        // diagnostic instead of silently accepting (and inert-emitting) them.
        let err = parse_err(r#"method = "POST""#, HandlerKind::Cls);
        assert!(
            err.to_string().contains("`method` is `#[endpoint]`-only"),
            "got: {err}"
        );
        let err = parse_err("requires_proxy_auth = true", HandlerKind::Cls);
        assert!(
            err.to_string()
                .contains("`requires_proxy_auth` is `#[endpoint]`-only"),
            "got: {err}"
        );
        // ...and the cls-only snapshot opt-in is rejected on a free `#[function]`.
        let err = parse_err("enable_memory_snapshot = true", HandlerKind::Function);
        assert!(err.to_string().contains("`#[cls]`-only"), "got: {err}");
    }

    #[test]
    fn schedule_cron_and_period_canonicalize_to_spec() {
        // Drive `parse_schedule_to_spec` through the real ParseStream path. The output
        // is the canonical SPEC string the SDK's `parse_schedule` consumes.
        let spec = |src: &str| -> String {
            syn::parse::Parser::parse_str(parse_schedule_to_spec, src).expect("valid schedule")
        };
        // Cron with the default UTC timezone.
        assert_eq!(spec(r#"Cron("0 9 * * 1")"#), "cron:UTC:0 9 * * 1");
        // Cron with an explicit IANA timezone.
        assert_eq!(
            spec(r#"Cron("0 6 * * *", "America/New_York")"#),
            "cron:America/New_York:0 6 * * *"
        );
        // A fully-qualified path still resolves by its last segment.
        assert_eq!(
            spec(r#"modal_rust::Cron("* * * * *")"#),
            "cron:UTC:* * * * *"
        );
        // Period — only the named components, in the order written; `seconds` is float.
        assert_eq!(spec("Period(days = 1)"), "period:days=1");
        assert_eq!(
            spec("Period(hours = 4, minutes = 30, seconds = 1.5)"),
            "period:hours=4,minutes=30,seconds=1.5"
        );
    }

    #[test]
    fn schedule_rejects_malformed() {
        let bad = |src: &str| syn::parse::Parser::parse_str(parse_schedule_to_spec, src).is_err();
        // Not a call expression.
        assert!(bad(r#""0 9 * * 1""#));
        // Unknown kind.
        assert!(bad(r#"Daily("0 9 * * 1")"#));
        // Cron with a non-string arg.
        assert!(bad("Cron(5)"));
        // Period with no components.
        assert!(bad("Period()"));
        // Period with an unknown component.
        assert!(bad("Period(fortnights = 2)"));
        // Period with a float for a non-`seconds` component.
        assert!(bad("Period(days = 1.5)"));
        // A literal colon in the cron string would corrupt the colon-delimited spec.
        assert!(bad(r#"Cron("0 9:30 * * 1")"#));
    }

    #[test]
    fn retries_struct_form_canonicalizes_to_spec() {
        // Drive `parse_retries_to_spec` through the real ParseStream path. The output is
        // the canonical SPEC string the SDK's `parse_retries_spec` consumes (seconds →
        // ms at parse time).
        let spec = |src: &str| -> String {
            syn::parse::Parser::parse_str(parse_retries_to_spec, src).expect("valid retries")
        };
        // Full struct form: max first, then the components in the order written.
        assert_eq!(
            spec("Retries(max_retries = 5, backoff_coefficient = 2.0, initial_delay = 0.5, max_delay = 30.0)"),
            "retries:max=5,backoff=2,initial_ms=500,max_ms=30000"
        );
        // Only the required `max_retries` — the SDK fills the rest with Modal defaults.
        assert_eq!(spec("Retries(max_retries = 3)"), "retries:max=3");
        // Integer delays (seconds) convert to ms too.
        assert_eq!(
            spec("Retries(max_retries = 2, initial_delay = 1, max_delay = 60)"),
            "retries:max=2,initial_ms=1000,max_ms=60000"
        );
        // A fully-qualified path still resolves by its last segment.
        assert_eq!(
            spec("modal_rust::Retries(max_retries = 4)"),
            "retries:max=4"
        );
    }

    #[test]
    fn retries_struct_form_rejects_malformed() {
        let bad = |src: &str| syn::parse::Parser::parse_str(parse_retries_to_spec, src).is_err();
        // Not a call expression.
        assert!(bad("5"));
        // Unknown kind.
        assert!(bad("Backoff(max_retries = 5)"));
        // Missing the required max_retries.
        assert!(bad("Retries(backoff_coefficient = 2.0)"));
        // Unknown component.
        assert!(bad("Retries(max_retries = 5, jitter = 0.1)"));
        // Positional (non `name = value`) component.
        assert!(bad("Retries(5)"));
        // Negative delay.
        assert!(bad("Retries(max_retries = 5, initial_delay = -1.0)"));
    }

    #[test]
    fn env_map_parses_pairs_and_rejects_dupes() {
        let pairs = |src: &str| -> Vec<(String, String)> {
            syn::parse::Parser::parse_str(parse_str_map, src)
                .expect("valid env map")
                .into_iter()
                .map(|(k, v)| (k.value(), v.value()))
                .collect()
        };
        assert_eq!(
            pairs(r#"{"API_TOKEN" = "dev", "REGION" = "us"}"#),
            vec![
                ("API_TOKEN".to_string(), "dev".to_string()),
                ("REGION".to_string(), "us".to_string()),
            ]
        );
        // Trailing comma allowed; empty map allowed.
        assert_eq!(
            pairs(r#"{"K" = "V",}"#),
            vec![("K".to_string(), "V".to_string())]
        );
        assert!(pairs("{}").is_empty());
        // A duplicate key is rejected (it would silently clobber an env var).
        assert!(syn::parse::Parser::parse_str(parse_str_map, r#"{"K" = "a", "K" = "b"}"#).is_err());
    }

    #[test]
    fn cls_kind_reads_enable_memory_snapshot_bool() {
        // The bare-bool opt-in parses on `#[cls]` (precedent: `cache`). Unset ⇒ `None`
        // (inherit / inert); `true`/`false` are recorded explicitly.
        let on: proc_macro2::TokenStream = syn::parse_str("enable_memory_snapshot = true").unwrap();
        assert_eq!(
            parse_decorator_config(on, HandlerKind::Cls)
                .unwrap()
                .enable_memory_snapshot,
            Some(true)
        );
        let off: proc_macro2::TokenStream =
            syn::parse_str("enable_memory_snapshot = false").unwrap();
        assert_eq!(
            parse_decorator_config(off, HandlerKind::Cls)
                .unwrap()
                .enable_memory_snapshot,
            Some(false)
        );
        // Unset on a bare `#[cls]` ⇒ `None` (inert default).
        assert_eq!(
            parse_decorator_config(proc_macro2::TokenStream::new(), HandlerKind::Cls)
                .unwrap()
                .enable_memory_snapshot,
            None
        );
        // A non-bool value is rejected (mirrors `cache`).
        let bad: proc_macro2::TokenStream = syn::parse_str("enable_memory_snapshot = 1").unwrap();
        assert!(parse_decorator_config(bad, HandlerKind::Cls).is_err());
    }

    #[test]
    fn cls_config_merge_threads_enable_memory_snapshot() {
        // A class-level opt-in is inherited by a bare `#[method]`; a method override
        // wins (field-by-field merge, same as the other config fields).
        let class = DecoratorConfig {
            enable_memory_snapshot: Some(true),
            ..Default::default()
        };
        assert_eq!(
            class
                .merge_over(&DecoratorConfig::default())
                .enable_memory_snapshot,
            Some(true)
        );
        let method_off = DecoratorConfig {
            enable_memory_snapshot: Some(false),
            ..Default::default()
        };
        assert_eq!(
            class.merge_over(&method_off).enable_memory_snapshot,
            Some(false)
        );
    }

    #[test]
    fn cls_config_registration_emits_resolved_snapshot_flag() {
        let facade = quote! { ::modal_rust };
        // Unset ⇒ inert `false` in the emitted `FunctionConfig`.
        let off = function_config_tokens(&DecoratorConfig::default(), &facade).to_string();
        assert!(
            off.contains("enable_memory_snapshot : false"),
            "unset opt-in must emit the inert `false`, got: {off}"
        );
        // Set ⇒ `true` rides into the emitted `FunctionConfig`.
        let cfg = DecoratorConfig {
            enable_memory_snapshot: Some(true),
            ..Default::default()
        };
        let on = function_config_tokens(&cfg, &facade).to_string();
        assert!(
            on.contains("enable_memory_snapshot : true"),
            "opt-in must emit `true`, got: {on}"
        );
    }

    /// Build a minimal `&self`-only `#[method]`-style [`ClsMethod`] for `emit_cls` tests.
    fn cls_method(name: &str, snapshot: Option<bool>) -> ClsMethod {
        let sig: syn::Signature =
            syn::parse_str(&format!("fn {name}(&self) -> anyhow::Result<usize>")).unwrap();
        ClsMethod {
            ident: syn::parse_str(name).unwrap(),
            config: DecoratorConfig {
                enable_memory_snapshot: snapshot,
                ..Default::default()
            },
            params: Vec::new(),
            output: sig.output,
        }
    }

    #[test]
    fn emit_cls_generates_snapshot_prime_when_enabled() {
        let class: syn::Ident = syn::parse_str("Embedder").unwrap();
        let enter: syn::Ident = syn::parse_str("load").unwrap();
        let facade = quote! { ::modal_rust };

        // Snapshot-enabled `#[cls]`: emits the `__modal_snapshot_prime_<Class>` free fn
        // (forcing the EXISTING accessor) and sets `snapshot_prime: Some(..)` on the
        // method registration.
        let on = emit_cls(
            &class,
            &enter,
            true,
            &[cls_method("dim", Some(true))],
            &facade,
        )
        .to_string();
        assert!(
            on.contains("fn __modal_snapshot_prime_Embedder"),
            "snapshot `#[cls]` must emit the prime free fn, got: {on}"
        );
        // The prime forces the EXISTING singleton accessor (no second singleton).
        assert!(
            on.contains("__modal_rust_cls_embedder () . map"),
            "the prime must force the existing accessor, got: {on}"
        );
        assert!(
            on.contains("snapshot_prime : :: core :: option :: Option :: Some (__modal_snapshot_prime_Embedder)"),
            "the method registration must wire `snapshot_prime: Some(prime)`, got: {on}"
        );
    }

    #[test]
    fn emit_cls_omits_snapshot_prime_when_disabled() {
        let class: syn::Ident = syn::parse_str("Embedder").unwrap();
        let enter: syn::Ident = syn::parse_str("load").unwrap();
        let facade = quote! { ::modal_rust };

        // Plain `#[cls]` (no opt-in): NO prime fn, every `snapshot_prime` is `None` ⇒
        // byte-identical to before.
        let off = emit_cls(&class, &enter, true, &[cls_method("dim", None)], &facade).to_string();
        assert!(
            !off.contains("__modal_snapshot_prime"),
            "non-snapshot `#[cls]` must NOT emit a prime fn, got: {off}"
        );
        assert!(
            off.contains("snapshot_prime : :: core :: option :: Option :: None"),
            "non-snapshot method must keep `snapshot_prime: None`, got: {off}"
        );
    }

    // =======================================================================
    // `#[endpoint]` — the SHARED `#[function]` grammar + the endpoint-only keys
    // (web-endpoints spec §5).
    // =======================================================================

    /// Parse a decorator argument list through the SHARED grammar as the given kind.
    fn parse_args(src: &str, kind: HandlerKind) -> syn::Result<DecoratorConfig> {
        let tokens: proc_macro2::TokenStream = syn::parse_str(src).expect("tokenizable args");
        parse_decorator_config(tokens, kind)
    }

    /// Unwrap the parse ERROR (syn types are not `Debug` without `extra-traits`, so
    /// `expect_err` does not apply; a plain match does).
    fn parse_err(src: &str, kind: HandlerKind) -> syn::Error {
        match parse_args(src, kind) {
            Err(e) => e,
            Ok(_) => panic!("args {src:?} must be rejected"),
        }
    }

    #[test]
    fn endpoint_parses_method_and_proxy_auth_with_shared_vocab() {
        // The ONE shared parser: an `#[endpoint]` accepts `method` (validated) +
        // `requires_proxy_auth` ALONGSIDE the unchanged `#[function]` vocabulary.
        let args = parse_args(
            r#"method = "POST", requires_proxy_auth = true, gpu = "T4", timeout = 600"#,
            HandlerKind::Endpoint,
        )
        .expect("valid endpoint args");
        assert_eq!(args.webhook_method.as_ref().unwrap().value(), "POST");
        assert!(args.webhook_requires_proxy_auth);
        // The shared `#[function]` vocab flows through the SAME parse (no fork).
        assert_eq!(args.gpu.as_ref().unwrap().value(), "T4");
        assert_eq!(args.timeout_secs, Some(600));

        // Every accepted verb parses; proxy-auth defaults to PUBLIC (false).
        for verb in ENDPOINT_METHODS {
            let args = parse_args(&format!(r#"method = "{verb}""#), HandlerKind::Endpoint)
                .unwrap_or_else(|e| panic!("verb {verb} must parse: {e}"));
            assert_eq!(args.webhook_method.as_ref().unwrap().value(), *verb);
            assert!(
                !args.webhook_requires_proxy_auth,
                "default exposure is PUBLIC (requires_proxy_auth = false), like Modal"
            );
        }

        // An explicit `requires_proxy_auth = false` is also accepted (a no-op).
        let public = parse_args(
            r#"method = "GET", requires_proxy_auth = false"#,
            HandlerKind::Endpoint,
        )
        .expect("explicit public endpoint");
        assert!(!public.webhook_requires_proxy_auth);
    }

    #[test]
    fn endpoint_missing_method_errors_with_expected_syntax() {
        // `method` is REQUIRED on an `#[endpoint]` (D3 — no silent default): a bare
        // attribute AND an attribute with only `#[function]` vocab both fail, and the
        // diagnostic carries the copy-pasteable expected syntax.
        for src in ["", r#"gpu = "T4""#, "requires_proxy_auth = true"] {
            let msg = parse_err(src, HandlerKind::Endpoint).to_string();
            assert!(
                msg.contains("requires `method ="),
                "missing-method diagnostic must say method is required, got: {msg}"
            );
            assert!(
                msg.contains(ENDPOINT_EXPECTED_SYNTAX),
                "missing-method diagnostic must carry the expected syntax, got: {msg}"
            );
        }
        // The same args are FINE on a plain `#[function]` (method is endpoint-only).
        assert!(parse_args(r#"gpu = "T4""#, HandlerKind::Function).is_ok());
    }

    #[test]
    fn endpoint_invalid_method_errors_with_expected_syntax() {
        // The verb is VALIDATED at expansion time (uppercase GET/POST/PUT/DELETE/PATCH
        // only) so a typo is a compile error, never a live-deploy surprise.
        for src in [
            r#"method = "post""#,    // lowercase
            r#"method = "HEAD""#,    // unsupported verb
            r#"method = "OPTIONS""#, // unsupported verb
            r#"method = """#,        // empty
        ] {
            let msg = parse_err(src, HandlerKind::Endpoint).to_string();
            assert!(
                msg.contains("invalid endpoint method"),
                "invalid-method diagnostic, got: {msg}"
            );
            assert!(
                msg.contains(ENDPOINT_EXPECTED_SYNTAX),
                "invalid-method diagnostic must carry the expected syntax, got: {msg}"
            );
        }
        // A non-bool proxy-auth value is rejected too.
        assert!(parse_args(
            r#"method = "GET", requires_proxy_auth = "yes""#,
            HandlerKind::Endpoint
        )
        .is_err());
    }

    #[test]
    fn function_rejects_endpoint_only_keys_with_pointer() {
        // The endpoint-only keys on a plain `#[function]` get a POINTED diagnostic
        // (use `#[endpoint]` instead), not the generic unsupported-argument error.
        let err = parse_err(r#"method = "POST""#, HandlerKind::Function);
        assert!(
            err.to_string().contains("`method` is `#[endpoint]`-only"),
            "got: {err}"
        );
        let err = parse_err("requires_proxy_auth = true", HandlerKind::Function);
        assert!(
            err.to_string()
                .contains("`requires_proxy_auth` is `#[endpoint]`-only"),
            "got: {err}"
        );
    }

    #[test]
    fn endpoint_config_threads_webhook_into_registration() {
        // The SHARED emitter threads the validated endpoint config onto the emitted
        // facade `FunctionConfig` (webhook_method = Some(verb), proxy-auth bool).
        let facade = quote! { ::modal_rust };
        let args = parse_args(
            r#"method = "POST", requires_proxy_auth = true"#,
            HandlerKind::Endpoint,
        )
        .expect("valid endpoint args");
        let tokens = build_registration(
            "add",
            quote! { handler_expr },
            quote! { check_expr },
            &facade,
            &args,
        )
        .to_string();
        assert!(
            tokens.contains(r#"webhook_method : :: core :: option :: Option :: Some ("POST")"#),
            "the validated method must ride the emitted FunctionConfig, got: {tokens}"
        );
        assert!(
            tokens.contains("webhook_requires_proxy_auth : true"),
            "the proxy-auth opt-in must ride the emitted FunctionConfig, got: {tokens}"
        );
    }

    #[test]
    fn function_registration_keeps_inert_webhook_defaults() {
        // A plain `#[function]` through the SAME emitter keeps the inert webhook
        // defaults (`None`/`false`) ⇒ byte-identical wire when no endpoint exists.
        let facade = quote! { ::modal_rust };
        let args = parse_args(r#"gpu = "T4""#, HandlerKind::Function).expect("valid fn args");
        let tokens = build_registration(
            "add",
            quote! { handler_expr },
            quote! { check_expr },
            &facade,
            &args,
        )
        .to_string();
        assert!(
            tokens.contains("webhook_method : :: core :: option :: Option :: None"),
            "a plain #[function] must emit webhook_method: None, got: {tokens}"
        );
        assert!(
            tokens.contains("webhook_requires_proxy_auth : false"),
            "a plain #[function] must emit webhook_requires_proxy_auth: false, got: {tokens}"
        );
    }

    #[test]
    fn cls_method_rejects_endpoint_attribute() {
        // `#[endpoint]` on a `#[cls]` method is a compile error in v0 (free fns only;
        // stateful web endpoints are a follow-up). Both the bare and the
        // facade-qualified spellings are caught.
        for attr in ["#[endpoint(method = \"POST\")]", "#[modal_rust::endpoint]"] {
            let mut method: syn::ImplItemFn = syn::parse_str(&format!(
                "{attr}\nfn serve(&self, q: String) -> anyhow::Result<String> {{ Ok(q) }}"
            ))
            .expect("parseable method");
            let err = match take_cls_marker(&mut method) {
                Err(e) => e,
                Ok(_) => panic!("{attr} on a #[cls] method must be rejected"),
            };
            assert!(
                err.to_string().contains("free-fn-only"),
                "diagnostic must say endpoint is free-fn-only, got: {err}"
            );
        }
        // A plain `#[method]` marker still parses fine (the rejection is targeted).
        let mut ok: syn::ImplItemFn =
            syn::parse_str("#[method]\nfn dim(&self) -> anyhow::Result<usize> { Ok(1) }")
                .expect("parseable method");
        assert!(matches!(
            take_cls_marker(&mut ok),
            Ok(Some(ClsMarker::Method(_)))
        ));
    }
}
