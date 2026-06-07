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
//! Modal's `Cron`/`Period`.
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

use proc_macro::TokenStream;
use proc_macro_crate::{crate_name, FoundCrate};
use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::{
    parse_macro_input, Expr, ExprLit, FnArg, GenericArgument, ItemFn, Lit, LitBool, LitInt, LitStr,
    PatType, PathArguments, ReturnType, Token, Type,
};

/// The Cargo package name of the facade crate the macro routes ALL paths through.
const FACADE_PACKAGE: &str = "modal-rust";

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
    let func = parse_macro_input!(item as ItemFn);

    // Parse the optional arguments. All are optional; the bare
    // `#[modal_rust::function]` (and `name = "..."`) set none of gpu/timeout/cache,
    // so the emitted facade `FunctionConfig` is `default()` (all `None`) —
    // runtime-observable behavior stays byte-identical.
    let mut explicit_name: Option<LitStr> = None;
    let mut gpu: Option<LitStr> = None; // gpu = "T4"
    let mut timeout_secs: Option<u64> = None; // timeout = 1800   (LitInt -> u64, narrow at emit)
    let mut cache: Option<bool> = None; // cache = false
    let mut milli_cpu: Option<u32> = None; // cpu = 2.0 (cores) -> milli_cpu = 2000
    let mut memory_mb: Option<u32> = None; // memory = 4096 (MiB)
    let mut retries: Option<u32> = None; // retries = 3 (retry count)
    let mut schedule: Option<String> = None; // schedule = Cron("..") / Period(..) -> spec string
    let mut secrets: Vec<String> = Vec::new(); // secrets = ["a", "b"]
    let mut volumes: Vec<(String, String)> = Vec::new(); // volumes = ["/data=vol"] -> (mount, name)
    if !attr.is_empty() {
        let parser = syn::meta::parser(|meta| {
            if meta.path.is_ident("name") {
                explicit_name = Some(meta.value()?.parse()?);
                Ok(())
            } else if meta.path.is_ident("gpu") {
                gpu = Some(meta.value()?.parse()?); // LitStr
                Ok(())
            } else if meta.path.is_ident("timeout") {
                let lit: LitInt = meta.value()?.parse()?; // integer seconds
                timeout_secs = Some(lit.base10_parse()?); // bad int -> compile_error!
                Ok(())
            } else if meta.path.is_ident("cache") {
                let lit: LitBool = meta.value()?.parse()?; // true / false
                cache = Some(lit.value);
                Ok(())
            } else if meta.path.is_ident("cpu") {
                // cpu = <cores> — CPU CORES as a float (e.g. `2.0`) or an int (e.g.
                // `2`). Mirrors Modal's `cpu` kwarg: milli_cpu = int(1000 * cpu)
                // (truncation). Resolved to milli-cores HERE so `FunctionConfig`
                // carries a plain `Option<u32>` const-valid in the `static`
                // `inventory::submit!` initializer (like `timeout`).
                milli_cpu = Some(parse_cpu_to_milli(meta.value()?)?);
                Ok(())
            } else if meta.path.is_ident("memory") {
                // memory = <MiB> — requested memory in MEBIBYTES (an int), mirroring
                // Modal's `memory` kwarg (memory_mb = memory). Narrowed to u32 at emit.
                let lit: LitInt = meta.value()?.parse()?;
                memory_mb = Some(lit.base10_parse()?); // bad int -> compile_error!
                Ok(())
            } else if meta.path.is_ident("retries") {
                // retries = <count> — the number of automatic retries, mirroring
                // Modal's bare-int `retries` kwarg (a fixed-interval policy). The
                // facade builds the FunctionRetryPolicy from this count. A plain
                // `Option<u32>` const-valid in the `static` initializer (like timeout).
                let lit: LitInt = meta.value()?.parse()?;
                retries = Some(lit.base10_parse()?); // bad int -> compile_error!
                Ok(())
            } else if meta.path.is_ident("schedule") {
                // schedule = Cron("expr"[, "tz"])  OR  Period(days = 1, hours = 4, ..)
                // A run cadence for a DEPLOYED function (Modal `Cron`/`Period`,
                // schedule.py). Parsed into a const SPEC string the facade hands to the
                // SDK's `parse_schedule`, so `FunctionConfig.schedule` stays an
                // `Option<&'static str>` const-valid in the `inventory::submit!`
                // initializer (exactly like `gpu`).
                schedule = Some(parse_schedule_to_spec(meta.value()?)?);
                Ok(())
            } else if meta.path.is_ident("secrets") {
                // secrets = ["my-secret", "other"] — a bracketed list of string
                // literals. Each is a Modal secret deployment-name the facade
                // resolves to a secret_id.
                for s in parse_str_list(meta.value()?)? {
                    secrets.push(s.value());
                }
                Ok(())
            } else if meta.path.is_ident("volumes") {
                // volumes = ["/data=my-vol", ..] — a bracketed list of "MOUNT=NAME"
                // string literals. Split on the FIRST '=' into (mount_path, name).
                // Map syntax is hard to parse in attribute position, so the simplest
                // parseable form is a string list.
                for s in parse_str_list(meta.value()?)? {
                    let raw = s.value();
                    let (mount, name) = raw.split_once('=').ok_or_else(|| {
                        syn::Error::new_spanned(
                            &s,
                            format!(
                                "`volumes` entries must be \"MOUNT_PATH=VOLUME_NAME\" \
                                 (path=name pairs), got {raw:?}"
                            ),
                        )
                    })?;
                    let mount = mount.trim();
                    let name = name.trim();
                    if mount.is_empty() || name.is_empty() {
                        return Err(syn::Error::new_spanned(
                            &s,
                            format!(
                                "`volumes` entry {raw:?} must have a non-empty mount path \
                                 AND volume name (\"MOUNT_PATH=VOLUME_NAME\")"
                            ),
                        ));
                    }
                    volumes.push((mount.to_string(), name.to_string()));
                }
                Ok(())
            } else {
                Err(meta.error(
                    "unsupported `#[modal_rust::function]` argument; recognized: \
                     `name = \"...\"`, `gpu = \"...\"`, `timeout = <int secs>`, \
                     `cache = <bool>`, `cpu = <cores>`, `memory = <MiB>`, \
                     `retries = <count>`, `schedule = Cron(\"..\")/Period(..)`, \
                     `secrets = [\"name\", ..]`, `volumes = [\"/mount=name\", ..]`",
                ))
            }
        });
        parse_macro_input!(attr with parser);
    }

    let fn_ident = func.sig.ident.clone();
    let entry_name = explicit_name
        .map(|s| s.value())
        .unwrap_or_else(|| fn_ident.to_string());

    // Resolve the facade crate name ONCE; every emitted runtime/`inventory`/facade
    // path is routed through it so a macro-using crate needs only `modal-rust`.
    let facade = facade_path();

    // async fn -> reserved `typed_async!` shape (boundaries.md §3) is not yet
    // implemented in the runtime. Reject clearly; keep the original fn so the rest
    // of the user's crate still type-checks, and do NOT touch the sync path.
    if let Some(async_token) = func.sig.asyncness {
        let msg = "#[modal_rust::function] does not yet support `async fn`: the \
                   reserved `typed_async!` shape (boundaries.md §3) is not yet \
                   implemented in modal-rust-runtime. Use a synchronous handler \
                   (it may `block_on` internally) for now.";
        let err = syn::Error::new_spanned(async_token, msg).to_compile_error();
        return quote! {
            #func
            #err
        }
        .into();
    }

    // Reject any `self` receiver up front (free `fn` only) in BOTH modes: the
    // registered handler is a free function, and a method on a type cannot be a
    // `HandlerFn`.
    if let Some(FnArg::Receiver(recv)) = func.sig.inputs.first() {
        let err = syn::Error::new_spanned(
            recv,
            "#[modal_rust::function] applies to free functions only; a `self` \
             receiver cannot be a runner entrypoint",
        )
        .to_compile_error();
        return quote! {
            #func
            #err
        }
        .into();
    }

    // Collect the typed params (every non-receiver arg). The receiver is already
    // rejected above, so an `unwrap`-free filter suffices.
    let params: Vec<&PatType> = func
        .sig
        .inputs
        .iter()
        .filter_map(|a| match a {
            FnArg::Typed(pt) => Some(pt),
            FnArg::Receiver(_) => None,
        })
        .collect();

    // Classify the signature style (auto-I/O ergonomics; see the crate docs / spec
    // §1). Mode A (EXPLICIT, byte-identical to before): exactly one param whose type
    // is a bare non-scalar `Type::Path`. Mode B (GENERATE): everything else.
    let mode_a = params.len() == 1 && is_mode_a_param_type(params[0].ty.as_ref());

    if mode_a {
        // Mode A: byte-identical to before — emit the unchanged fn + `typed!(fn)`
        // registration. No generated module/shim/typed methods. The handler/registration
        // paths are routed through the facade (`#facade::__private::runtime::…`) so the
        // generated code is semantically identical; only the names it spells change.
        return emit_registration(
            &func,
            &entry_name,
            quote! { #facade::__private::runtime::typed!(#fn_ident) },
            &facade,
            &gpu,
            timeout_secs,
            cache,
            milli_cpu,
            memory_mb,
            retries,
            schedule.as_deref(),
            &secrets,
            &volumes,
        );
    }

    // Mode B: generate the named input type, the spread shim, the typed App methods,
    // and register the SHIM. First, validate every param is a plain owned
    // `ident: Type` (no `self`, already excluded above) and the handler carries no
    // generics/lifetimes/where-clause (the generated Input/shim can't be
    // monomorphized generically).
    if let Some(err) = mode_b_signature_error(&func, &params) {
        return quote! {
            #func
            #err
        }
        .into();
    }

    // The named-field list `(ident, type)` for the generated `Input` struct + spread,
    // in declaration order.
    let field_idents: Vec<&syn::Ident> = params
        .iter()
        .map(|pt| match pt.pat.as_ref() {
            syn::Pat::Ident(pi) => &pi.ident,
            // `mode_b_signature_error` already rejected non-ident patterns; this arm
            // is unreachable in practice.
            _ => unreachable!("non-ident param survived mode_b validation"),
        })
        .collect();
    let field_types: Vec<&Type> = params.iter().map(|pt| pt.ty.as_ref()).collect();

    // The return type's inner `Ok` type, used as `pub type Output = T;`. If the
    // return is not a recognizable `Result<T, ..>` we fall back to the whole return
    // type token; a non-`Result` handler is already a compile error inside `typed!`
    // (it matches `Ok/Err`), so no extra diagnostic is needed here.
    let output_ty = result_ok_type(&func.sig.output);

    // The shim copies the ORIGINAL return type token-for-token (keeps `E` intact so
    // the `typed!` autoref specialization still selects the right `details` path).
    let orig_output = &func.sig.output;
    let shim_ident = format_ident!("__modal_rust_shim_{}", fn_ident);

    // The per-fn extension trait name: `<Pascal>Call`.
    let trait_ident = format_ident!("{}Call", to_pascal_case(&fn_ident.to_string()));

    // The generated I/O module + spread shim + typed App extension trait.
    let generated = quote! {
        #[doc(hidden)]
        #[allow(non_snake_case)]
        pub mod #fn_ident {
            // Param types written in the fn's own scope (e.g. user structs) resolve
            // here via the parent glob.
            #[allow(unused_imports)]
            use super::*;

            /// Auto-generated named input for this `#[modal_rust::function]` handler:
            /// one `pub` field per parameter (field name = param ident, in declared
            /// order). Serializes to the frozen named JSON object the runner decodes;
            /// `Serialize` is consumed at the call site, `Deserialize` on the wire.
            #[derive(::serde::Serialize, ::serde::Deserialize)]
            pub struct Input {
                #( pub #field_idents : #field_types ),*
            }

            /// Auto-generated output alias = the handler's return `Ok` type (the
            /// value the success envelope carries).
            pub type Output = #output_ty;
        }

        /// Private SPREAD shim: decodes the generated `Input`, spreads its fields as
        /// positional args to the user fn, and returns the user fn's result verbatim.
        /// Registered via the UNCHANGED `typed!` so the frozen decode/call/encode +
        /// five-error taxonomy is byte-identical; only the registered fn differs.
        #[doc(hidden)]
        fn #shim_ident(__modal_rust_in: self::#fn_ident::Input) #orig_output {
            #fn_ident( #( __modal_rust_in.#field_idents ),* )
        }

        /// Auto-generated typed positional CALL trait for this handler, implemented
        /// for the facade `App`. Brings `app.#fn_ident(args)` into scope; chains into
        /// `.local()/.remote()/.spawn()/.map()`. Pure sugar over the string-keyed
        /// `App::function(name)` path.
        pub trait #trait_ident {
            /// Build a typed positional call to this entrypoint.
            #[allow(clippy::too_many_arguments)]
            fn #fn_ident<'__modal_rust_a>(
                &'__modal_rust_a self,
                #( #field_idents : #field_types ),*
            ) -> #facade::TypedCall<
                '__modal_rust_a,
                self::#fn_ident::Input,
                self::#fn_ident::Output,
            >;
        }

        impl #trait_ident for #facade::App {
            fn #fn_ident<'__modal_rust_a>(
                &'__modal_rust_a self,
                #( #field_idents : #field_types ),*
            ) -> #facade::TypedCall<
                '__modal_rust_a,
                self::#fn_ident::Input,
                self::#fn_ident::Output,
            > {
                #facade::TypedCall::new(
                    self,
                    #entry_name,
                    self::#fn_ident::Input { #( #field_idents ),* },
                )
            }
        }
    };

    let registration = build_registration(
        &entry_name,
        quote! { #facade::__private::runtime::typed!(#shim_ident) },
        &facade,
        &gpu,
        timeout_secs,
        cache,
        milli_cpu,
        memory_mb,
        retries,
        schedule.as_deref(),
        &secrets,
        &volumes,
    );

    quote! {
        #func
        #generated
        #registration
    }
    .into()
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
    // Optional single argument: the library crate ident whose `inventory`
    // submissions must be linked into this runner binary (`use <crate> as _;`).
    // Empty = the functions are in THIS crate (single-binary layout).
    let link_crate: Option<syn::Ident> = if input.is_empty() {
        None
    } else {
        match syn::parse::<syn::Ident>(input) {
            Ok(ident) => Some(ident),
            Err(_) => {
                let err = syn::Error::new(
                    proc_macro2::Span::call_site(),
                    "modal_rust::modal_runner! takes at most ONE argument: the library \
                     crate name to link (e.g. `modal_runner!(my_crate);`), or nothing \
                     for a single-binary crate (`modal_runner!();`)",
                )
                .to_compile_error();
                return err.into();
            }
        }
    };

    let facade = facade_path();
    // Link the lib crate's `inventory::submit!` link-section into this binary when a
    // crate name was given; a `use <crate> as _;` is the idiomatic side-effect link.
    let link = link_crate.map(|c| quote! { use #c as _; });
    quote! {
        #link
        fn main() -> ::std::process::ExitCode {
            // Assemble the registry + per-entrypoint decorator configs from the
            // facade-owned atomic `inventory` submissions, then run the FROZEN
            // runner CLI protocol. The configs ride into the additive `--describe`
            // manifest; the `--entrypoint` dispatch ignores them.
            let code = #facade::__private::run_cli_from_inventory();
            ::std::process::ExitCode::from(code as u8)
        }
    }
    .into()
}

/// Mode-A emission helper: keep the original fn verbatim and submit one facade
/// `Registration` whose handler is `#handler_expr` (here `typed!(#fn_ident)`),
/// with the decorator config in the same record.
#[allow(clippy::too_many_arguments)]
fn emit_registration(
    func: &ItemFn,
    entry_name: &str,
    handler_expr: proc_macro2::TokenStream,
    facade: &proc_macro2::TokenStream,
    gpu: &Option<LitStr>,
    timeout_secs: Option<u64>,
    cache: Option<bool>,
    milli_cpu: Option<u32>,
    memory_mb: Option<u32>,
    retries: Option<u32>,
    schedule: Option<&str>,
    secrets: &[String],
    volumes: &[(String, String)],
) -> TokenStream {
    let registration = build_registration(
        entry_name,
        handler_expr,
        facade,
        gpu,
        timeout_secs,
        cache,
        milli_cpu,
        memory_mb,
        retries,
        schedule,
        secrets,
        volumes,
    );
    quote! {
        #func
        #registration
    }
    .into()
}

/// Build the `inventory::submit! { Registration { .. } }` token stream registering
/// `#handler_expr` under `entry_name` with the decorator `FunctionConfig`.
///
/// Every path is routed through the resolved `#facade`
/// (`#facade::__private::inventory::submit!`, `#facade::{Registration,
/// FunctionConfig}`) so a macro-using crate needs ONLY `modal-rust`.
/// `inventory::submit!` — invoked here THROUGH the facade re-export — places this
/// in a link section that the facade collects at runner startup. The
/// `typed!` macro expands to a block that defines a local `fn` and coerces it to a
/// `HandlerFn` pointer — a const-evaluable expression valid in the `static`
/// initializer `inventory::submit!` generates.
///
/// The decorator config flows into the facade registration as a `FunctionConfig`. The
/// `gpu` literal is a `&'static str` (so the `static` `inventory::submit!`
/// initializer stays `const`-valid, matching `name: &'static str`); `timeout` is
/// narrowed `u64 -> u32` here. The bare form sets all three to `None` =>
/// `FunctionConfig::default()`, which runtime dispatch ignores (so behavior is
/// byte-identical; only the facade reads `config` for control-plane work).
#[allow(clippy::too_many_arguments)]
fn build_registration(
    entry_name: &str,
    handler_expr: proc_macro2::TokenStream,
    facade: &proc_macro2::TokenStream,
    gpu: &Option<LitStr>,
    timeout_secs: Option<u64>,
    cache: Option<bool>,
    milli_cpu: Option<u32>,
    memory_mb: Option<u32>,
    retries: Option<u32>,
    schedule: Option<&str>,
    secrets: &[String],
    volumes: &[(String, String)],
) -> proc_macro2::TokenStream {
    let gpu_tok = match gpu {
        Some(s) => quote! { ::core::option::Option::Some(#s) }, // &'static str literal
        None => quote! { ::core::option::Option::None },
    };
    let timeout_tok = match timeout_secs {
        Some(n) => {
            let n = n as u32;
            quote! { ::core::option::Option::Some(#n) }
        }
        None => quote! { ::core::option::Option::None },
    };
    let cache_tok = match cache {
        Some(b) => quote! { ::core::option::Option::Some(#b) },
        None => quote! { ::core::option::Option::None },
    };
    // `cpu`/`memory` are resolved to wire units (milli-cores / MiB) at parse time, so
    // each is a plain `Option<u32>` const-valid in the `static` initializer (exactly
    // like `timeout`). `None` emits `None` => byte-identical to a bare decorator.
    let milli_cpu_tok = match milli_cpu {
        Some(n) => quote! { ::core::option::Option::Some(#n) },
        None => quote! { ::core::option::Option::None },
    };
    let memory_mb_tok = match memory_mb {
        Some(n) => quote! { ::core::option::Option::Some(#n) },
        None => quote! { ::core::option::Option::None },
    };
    // `retries` is a plain `Option<u32>` const-valid in the `static` initializer
    // (exactly like `timeout`). `None` emits `None` => byte-identical to a bare
    // decorator (no retry policy on the wire).
    let retries_tok = match retries {
        Some(n) => quote! { ::core::option::Option::Some(#n) },
        None => quote! { ::core::option::Option::None },
    };
    // `schedule` is canonicalized to a `&'static str` SPEC string (the facade hands it
    // to the SDK's `parse_schedule`), so it stays const-valid in the `static`
    // initializer exactly like `gpu`. `None` emits `None` => byte-identical to a bare
    // decorator (no schedule on the wire).
    let schedule_tok = match schedule {
        Some(s) => quote! { ::core::option::Option::Some(#s) },
        None => quote! { ::core::option::Option::None },
    };
    // `secrets`/`volumes` are `&'static` slices on `FunctionConfig` (const-valid in
    // the `static` `inventory::submit!` initializer, exactly like `gpu`/`name`). An
    // empty list emits `&[]`, byte-identical to the bare default.
    let secrets_tok = {
        let items = secrets.iter();
        quote! { &[ #( #items ),* ] }
    };
    let volumes_tok = {
        let items = volumes
            .iter()
            .map(|(mount, name)| quote! { (#mount, #name) });
        quote! { &[ #( #items ),* ] }
    };

    quote! {
        #facade::__private::inventory::submit! {
            #facade::Registration {
                name: #entry_name,
                handler: #handler_expr,
                config: #facade::FunctionConfig {
                    gpu: #gpu_tok,
                    timeout_secs: #timeout_tok,
                    cache: #cache_tok,
                    milli_cpu: #milli_cpu_tok,
                    memory_mb: #memory_mb_tok,
                    retries: #retries_tok,
                    schedule: #schedule_tok,
                    secrets: #secrets_tok,
                    volumes: #volumes_tok,
                },
                // Capture the USER crate's cargo package name HERE — this macro
                // expands in the user's crate, so `env!("CARGO_PKG_NAME")` is the
                // user's package, not the facade's. The RUN/DEPLOY path threads it
                // into `RemoteConfig.package` so `cargo build -p <pkg>` targets the
                // right crate WITHOUT the user setting `MODAL_RUST_PACKAGE` (which
                // still overrides). METADATA ONLY — the runner ignores it.
                package: ::core::env!("CARGO_PKG_NAME"),
            }
        }
    }
}

/// The scalar denylist (spec §1): a single param of one of these primitive/standard
/// types forces Mode B (auto-I/O), even though it is a bare path. Anything not here
/// AND a bare non-generic `Type::Path` is treated as a user struct (Mode A).
const SCALAR_DENYLIST: &[&str] = &[
    "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32",
    "f64", "bool", "char", "str", "String",
];

/// Classify a SINGLE param's type for Mode A vs Mode B (spec §1). Returns `true` iff
/// the type is a bare `Type::Path` with NO generic arguments whose last path segment
/// ident is NOT in [`SCALAR_DENYLIST`] (i.e. a user struct used as-is — Mode A).
/// Anything else (`&T`, `(A, B)`, `[T; N]`, a generic path like `Vec<u8>`, or a
/// denylisted scalar) is Mode B.
fn is_mode_a_param_type(ty: &Type) -> bool {
    let Type::Path(tp) = ty else {
        return false;
    };
    // A leading `::` or a qualified self type is fine — only the last segment's
    // generics + ident matter for the syntactic rule.
    let Some(last) = tp.path.segments.last() else {
        return false;
    };
    if !matches!(last.arguments, PathArguments::None) {
        return false; // generic path (Vec<u8>, Option<i64>, …) -> Mode B
    }
    let ident = last.ident.to_string();
    !SCALAR_DENYLIST.contains(&ident.as_str())
}

/// Validate a Mode-B handler signature (spec §1). Returns `Some(compile_error)` on
/// the first violation, else `None`. Enforces: every param is a plain `ident: Type`
/// (no destructuring, no `mut`), owned (no `&T`/reference), and the handler carries
/// no generics/lifetimes/where-clause.
fn mode_b_signature_error(func: &ItemFn, params: &[&PatType]) -> Option<proc_macro2::TokenStream> {
    // No generics / lifetimes / where-clauses on the handler: the generated Input /
    // shim cannot be monomorphized generically.
    if !func.sig.generics.params.is_empty() || func.sig.generics.where_clause.is_some() {
        return Some(
            syn::Error::new_spanned(
                &func.sig.generics,
                "plain #[modal_rust::function] handlers cannot be generic (no type/\
                 lifetime params or where-clauses): the generated input type cannot \
                 be monomorphized. Use concrete owned param types.",
            )
            .to_compile_error(),
        );
    }

    for pt in params {
        // Each param must be a plain identifier pattern (no `(a, b)`, no `mut`).
        match pt.pat.as_ref() {
            syn::Pat::Ident(pi) if pi.subpat.is_none() => {}
            _ => {
                return Some(
                    syn::Error::new_spanned(
                        pt,
                        "name each parameter so its name can become an input field \
                         (a plain `ident: Type`, no destructuring)",
                    )
                    .to_compile_error(),
                );
            }
        }
        // Owned only: reject references / borrowed params.
        if matches!(pt.ty.as_ref(), Type::Reference(_)) {
            return Some(
                syn::Error::new_spanned(
                    pt,
                    "plain #[modal_rust::function] params must be owned; use String / \
                     Vec<u8> instead of a borrowed `&str` / `&[u8]`",
                )
                .to_compile_error(),
            );
        }
    }
    None
}

/// Extract the inner `Ok` type `T` from a handler return type `-> Result<T, E>` /
/// `-> anyhow::Result<T>` (spec §4). Returns the first generic TYPE argument of the
/// last path segment whose ident is `Result`. Falls back to the whole return type
/// token when the shape is unrecognized (a non-`Result` return is already a compile
/// error inside `typed!`, so no extra diagnostic is needed).
fn result_ok_type(output: &ReturnType) -> proc_macro2::TokenStream {
    let ReturnType::Type(_, ty) = output else {
        return quote! { () };
    };
    if let Type::Path(tp) = ty.as_ref() {
        if let Some(seg) = tp.path.segments.last() {
            if seg.ident == "Result" {
                if let PathArguments::AngleBracketed(args) = &seg.arguments {
                    for arg in &args.args {
                        if let GenericArgument::Type(inner) = arg {
                            return quote! { #inner };
                        }
                    }
                }
            }
        }
    }
    quote! { #ty }
}

/// Convert a snake_case fn ident to PascalCase for the `<Pascal>Call` trait name
/// (`add` -> `Add`, `add_gpu` -> `AddGpu`). Underscores are separators; each
/// following segment is capitalized.
fn to_pascal_case(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    let mut capitalize = true;
    for ch in snake.chars() {
        if ch == '_' {
            capitalize = true;
        } else if capitalize {
            out.extend(ch.to_uppercase());
            capitalize = false;
        } else {
            out.push(ch);
        }
    }
    out
}

/// Parse a `cpu = <cores>` value into milli-cores, mirroring Modal's
/// `milli_cpu = int(1000 * cpu)` (truncation toward zero). Accepts a FLOAT literal
/// (`2.0`, `0.5`) or an INT literal (`2` ⇒ `2.0` cores). Resolving HERE keeps
/// [`FunctionConfig::milli_cpu`] a plain const `Option<u32>` for the `static`
/// `inventory::submit!` initializer. A negative value is rejected (cores cannot be
/// negative).
fn parse_cpu_to_milli(input: syn::parse::ParseStream) -> syn::Result<u32> {
    let lit: Lit = input.parse()?;
    let cores: f64 = match &lit {
        Lit::Float(f) => f.base10_parse()?,
        Lit::Int(i) => i.base10_parse::<u64>()? as f64,
        other => {
            return Err(syn::Error::new_spanned(
                other,
                "`cpu` must be a number of cores, e.g. `cpu = 2.0` or `cpu = 1`",
            ))
        }
    };
    if cores < 0.0 || !cores.is_finite() {
        return Err(syn::Error::new_spanned(
            &lit,
            "`cpu` (cores) must be a finite, non-negative number",
        ));
    }
    // int(1000 * cpu): multiply then TRUNCATE toward zero, matching Modal's Python.
    Ok((cores * 1000.0) as u32)
}

/// Parse a bracketed list of string literals from a `meta.value()` parse stream:
/// `["a", "b", "c"]`. Used by both `secrets = [..]` and `volumes = [..]`. Returns
/// the [`LitStr`]s (so callers keep the spans for good diagnostics). An empty list
/// `[]` is allowed (yields no items).
fn parse_str_list(input: syn::parse::ParseStream) -> syn::Result<Vec<LitStr>> {
    let content;
    syn::bracketed!(content in input);
    let items: Punctuated<LitStr, Token![,]> = Punctuated::parse_terminated(&content)?;
    Ok(items.into_iter().collect())
}

/// The seven `Period(..)` component names, in Modal's large→small order
/// (`schedule.py:90`). `seconds` is the only float; the rest are integers.
const PERIOD_COMPONENTS: &[&str] = &[
    "years", "months", "weeks", "days", "hours", "minutes", "seconds",
];

/// Parse a `schedule = ..` value into a canonical SPEC string the SDK's
/// `parse_schedule` understands. Two call-shaped forms mirror Modal's `Cron`/`Period`
/// (`schedule.py`):
///
/// - `Cron("expr")` / `Cron("expr", "tz")` → `"cron:<tz>:<expr>"` (timezone defaults
///   to `UTC`, matching `Cron(cron_string, timezone="UTC")`).
/// - `Period(days = 1, hours = 4, seconds = 1.5)` → `"period:days=1,hours=4,seconds=1.5"`
///   (only the named components; omitted ones default to `0`).
///
/// A malformed form (`gpu`-style) becomes a `compile_error!` so the user learns at
/// compile time, never on the wire.
fn parse_schedule_to_spec(input: syn::parse::ParseStream) -> syn::Result<String> {
    let call: syn::ExprCall = input.parse().map_err(|_| {
        syn::Error::new(
            input.span(),
            "`schedule` must be `Cron(\"expr\"[, \"tz\"])` or `Period(days = 1, ..)`",
        )
    })?;
    // The callee is a path like `Cron` / `modal_rust::Cron` — take the last segment.
    let kind = match call.func.as_ref() {
        Expr::Path(p) => p
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default(),
        other => {
            return Err(syn::Error::new_spanned(
                other,
                "`schedule` must call `Cron(..)` or `Period(..)`",
            ))
        }
    };

    match kind.as_str() {
        "Cron" => {
            // Cron("expr") or Cron("expr", "tz") — string-literal args only.
            let mut args = call.args.iter();
            let cron_string = expect_str_lit(args.next(), &call, "Cron expects a cron string")?;
            let timezone = match args.next() {
                Some(a) => expect_str_lit(Some(a), &call, "Cron timezone must be a string")?,
                None => "UTC".to_string(), // mirrors Cron(.., timezone="UTC")
            };
            if args.next().is_some() {
                return Err(syn::Error::new_spanned(
                    &call,
                    "Cron takes at most two arguments: Cron(\"expr\"[, \"tz\"])",
                ));
            }
            if cron_string.contains(':') || timezone.contains(':') {
                // The spec is colon-delimited; a literal colon would corrupt it. Cron
                // expressions and IANA timezones never contain a colon, so reject early.
                return Err(syn::Error::new_spanned(
                    &call,
                    "Cron expression / timezone must not contain a ':'",
                ));
            }
            Ok(format!("cron:{timezone}:{cron_string}"))
        }
        // Period(days = 1, hours = 4, ..) — `name = value` named components only.
        "Period" => parse_period_components(&call),
        other => Err(syn::Error::new_spanned(
            &call.func,
            format!("unknown schedule kind {other:?}; expected `Cron` or `Period`"),
        )),
    }
}

/// Extract a string-literal value from a call argument, or a clear `compile_error!`.
fn expect_str_lit(arg: Option<&Expr>, call: &syn::ExprCall, msg: &str) -> syn::Result<String> {
    match arg {
        Some(Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        })) => Ok(s.value()),
        Some(other) => Err(syn::Error::new_spanned(other, msg)),
        None => Err(syn::Error::new_spanned(call, msg)),
    }
}

/// Parse `Period(days = 1, hours = 4, seconds = 1.5)` arguments into the canonical
/// `period:..` spec. Each argument is `name = value`; `name` must be a known component
/// and `value` an int (or a float for `seconds`).
fn parse_period_components(call: &syn::ExprCall) -> syn::Result<String> {
    if call.args.is_empty() {
        return Err(syn::Error::new_spanned(
            call,
            "Period needs at least one component, e.g. `Period(days = 1)`",
        ));
    }
    let mut parts: Vec<String> = Vec::new();
    for arg in &call.args {
        let Expr::Assign(assign) = arg else {
            return Err(syn::Error::new_spanned(
                arg,
                "Period components must be `name = value`, e.g. `hours = 4`",
            ));
        };
        let Expr::Path(name_path) = assign.left.as_ref() else {
            return Err(syn::Error::new_spanned(
                &assign.left,
                "Period component name must be a bare identifier (e.g. `days`)",
            ));
        };
        let name = name_path
            .path
            .get_ident()
            .map(|i| i.to_string())
            .ok_or_else(|| {
                syn::Error::new_spanned(&assign.left, "Period component name must be an identifier")
            })?;
        if !PERIOD_COMPONENTS.contains(&name.as_str()) {
            return Err(syn::Error::new_spanned(
                &assign.left,
                format!("unknown Period component {name:?}; expected one of {PERIOD_COMPONENTS:?}"),
            ));
        }
        // The value must be a numeric literal. `seconds` may be a float; all others
        // must be integers. We render the literal verbatim into the spec; the SDK
        // re-parses it.
        let value =
            match assign.right.as_ref() {
                Expr::Lit(ExprLit {
                    lit: Lit::Int(i), ..
                }) => i.base10_digits().to_string(),
                Expr::Lit(ExprLit {
                    lit: Lit::Float(f), ..
                }) if name == "seconds" => f.base10_digits().to_string(),
                Expr::Lit(ExprLit {
                    lit: Lit::Float(_), ..
                }) => return Err(syn::Error::new_spanned(
                    &assign.right,
                    format!(
                        "Period component {name:?} must be an integer (only `seconds` is a float)"
                    ),
                )),
                other => {
                    return Err(syn::Error::new_spanned(
                        other,
                        format!("Period component {name:?} must be a numeric literal"),
                    ))
                }
            };
        parts.push(format!("{name}={value}"));
    }
    Ok(format!("period:{}", parts.join(",")))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
