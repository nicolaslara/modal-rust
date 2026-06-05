//! Additive proc-macro sugar for modal-rust (ergonomics E1).
//!
//! [`macro@function`] is an attribute macro that, applied to a handler like
//! `pub fn add(input: AddInput) -> anyhow::Result<AddOutput>`, expands to:
//!
//! 1. the **unchanged** original function, and
//! 2. an `inventory::submit!` registration whose handler is the SAME
//!    monomorphized `modal_rust_runtime::typed!(add)` wrapper `fn` pointer the
//!    manual `Registry::new().function("add", typed!(add))` builder produces.
//!
//! `Registry::from_inventory()` then collects every submission into the SAME
//! `BTreeMap<&'static str, HandlerFn>` as the manual path (boundaries.md §3). The
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
//! `#[modal_rust::function(gpu = "T4", timeout = 1800, cache = false)]` records a
//! [`modal_rust_runtime::FunctionConfig`] alongside the registration. This is
//! METADATA ONLY — the runner ignores it; only the control-plane facade reads it
//! when creating the Modal function (`Resources.gpu_config`, `timeout_secs`). All
//! keys are optional; the bare `#[modal_rust::function]` and `name = "..."` forms
//! record `FunctionConfig::default()` (all `None`, empty secrets/volumes), so the
//! runtime-observable behavior is byte-identical to before this addition.
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
//! ### Mode B emitted-path requirements (downstream `Cargo.toml`)
//!
//! Mode B emits two more absolute paths beyond the runtime/`inventory` deps every
//! macro-using crate already carries (see the crate-level dep caveat above):
//! - `::serde::{Serialize, Deserialize}` for the generated `Input` derives — every
//!   macro-using crate already has `serde` with `derive`, so this is no new dep.
//! - `::modal_rust_facade::{App, TypedCall}` for the typed `app.<fn>(..)` methods.
//!   Because a macro-using crate often aliases the MACRO crate as `modal_rust`
//!   (`extern crate modal_rust_macros as modal_rust;` so `#[modal_rust::function]`
//!   is spellable), the macro must NOT emit `::modal_rust` for the FACADE — it would
//!   resolve to the shadowing alias. Instead it emits `::modal_rust_facade`, which
//!   the downstream crate guarantees with a RENAMED dependency:
//!   `modal_rust_facade = { path = "...", package = "modal-rust" }`. (A crate that
//!   does NOT alias the macro can drop the rename and spell the attribute as
//!   `#[modal_rust_macros::function]`; then `::modal_rust` is unshadowed — but the
//!   canonical `examples/add-macro` keeps the alias + adds the rename.)
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
use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::{
    parse_macro_input, FnArg, GenericArgument, ItemFn, LitBool, LitInt, LitStr, PatType,
    PathArguments, ReturnType, Token, Type,
};

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
    // so the emitted `FunctionConfig` is `default()` (all `None`) — runtime-
    // observable behavior stays byte-identical (the runner ignores `config`).
    let mut explicit_name: Option<LitStr> = None;
    let mut gpu: Option<LitStr> = None; // gpu = "T4"
    let mut timeout_secs: Option<u64> = None; // timeout = 1800   (LitInt -> u64, narrow at emit)
    let mut cache: Option<bool> = None; // cache = false
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
                     `cache = <bool>`, `secrets = [\"name\", ..]`, \
                     `volumes = [\"/mount=name\", ..]`",
                ))
            }
        });
        parse_macro_input!(attr with parser);
    }

    let fn_ident = func.sig.ident.clone();
    let entry_name = explicit_name
        .map(|s| s.value())
        .unwrap_or_else(|| fn_ident.to_string());

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
        // registration. No generated module/shim/typed methods.
        return emit_registration(
            &func,
            &entry_name,
            quote! { ::modal_rust_runtime::typed!(#fn_ident) },
            &gpu,
            timeout_secs,
            cache,
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
            ) -> ::modal_rust_facade::TypedCall<
                '__modal_rust_a,
                self::#fn_ident::Input,
                self::#fn_ident::Output,
            >;
        }

        impl #trait_ident for ::modal_rust_facade::App {
            fn #fn_ident<'__modal_rust_a>(
                &'__modal_rust_a self,
                #( #field_idents : #field_types ),*
            ) -> ::modal_rust_facade::TypedCall<
                '__modal_rust_a,
                self::#fn_ident::Input,
                self::#fn_ident::Output,
            > {
                ::modal_rust_facade::TypedCall::new(
                    self,
                    #entry_name,
                    self::#fn_ident::Input { #( #field_idents ),* },
                )
            }
        }
    };

    let registration = build_registration(
        &entry_name,
        quote! { ::modal_rust_runtime::typed!(#shim_ident) },
        &gpu,
        timeout_secs,
        cache,
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

/// Mode-A emission helper: keep the original fn verbatim and submit a
/// `Registration` whose handler is `#handler_expr` (here `typed!(#fn_ident)`), with
/// the decorator config — byte-identical to the pre-auto-I/O path.
#[allow(clippy::too_many_arguments)]
fn emit_registration(
    func: &ItemFn,
    entry_name: &str,
    handler_expr: proc_macro2::TokenStream,
    gpu: &Option<LitStr>,
    timeout_secs: Option<u64>,
    cache: Option<bool>,
    secrets: &[String],
    volumes: &[(String, String)],
) -> TokenStream {
    let registration = build_registration(
        entry_name,
        handler_expr,
        gpu,
        timeout_secs,
        cache,
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
/// `inventory::submit!` places this in a link section that
/// `Registry::from_inventory()` collects at runner startup. The `typed!` macro
/// expands to a block that defines a local `fn` and coerces it to a `HandlerFn`
/// pointer — a const-evaluable expression valid in the `static` initializer
/// `inventory::submit!` generates.
///
/// The decorator config flows into the registration as a `FunctionConfig`. The
/// `gpu` literal is a `&'static str` (so the `static` `inventory::submit!`
/// initializer stays `const`-valid, matching `name: &'static str`); `timeout` is
/// narrowed `u64 -> u32` here. The bare form sets all three to `None` =>
/// `FunctionConfig::default()`, which the runner ignores (so behavior is
/// byte-identical; only the facade reads `config`).
fn build_registration(
    entry_name: &str,
    handler_expr: proc_macro2::TokenStream,
    gpu: &Option<LitStr>,
    timeout_secs: Option<u64>,
    cache: Option<bool>,
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
        ::inventory::submit! {
            ::modal_rust_runtime::Registration {
                name: #entry_name,
                handler: #handler_expr,
                config: ::modal_rust_runtime::FunctionConfig {
                    gpu: #gpu_tok,
                    timeout_secs: #timeout_tok,
                    cache: #cache_tok,
                    secrets: #secrets_tok,
                    volumes: #volumes_tok,
                },
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
    fn pascal_case_handles_underscores() {
        assert_eq!(to_pascal_case("add"), "Add");
        assert_eq!(to_pascal_case("add_plain"), "AddPlain");
        assert_eq!(to_pascal_case("add_gpu"), "AddGpu");
        assert_eq!(to_pascal_case("a_b_c"), "ABC");
        assert_eq!(to_pascal_case("already"), "Already");
    }
}
