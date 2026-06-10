//! The `#[function]` / `#[endpoint]` expansion + registration emission: the Mode
//! A/B classifier, the generated I/O module + spread shim + typed `App` trait, and
//! the `inventory::submit!` `Registration` builder shared with `#[cls]`. Split out
//! of `lib.rs` mechanically (M1).

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    parse_macro_input, FnArg, GenericArgument, ItemFn, PatType, PathArguments, ReturnType, Type,
};

use crate::args::{parse_function_args, FunctionArgs, HandlerKind};
use crate::facade_path;

/// The ONE shared expansion path behind `#[function]` and `#[endpoint]`
/// (web-endpoints spec §5): parse the shared decorator grammar, classify Mode A/B,
/// and emit the SAME original fn + `Registration` + typed surface. The kind only
/// changes the endpoint-only keys (`method`/`requires_proxy_auth`, threaded into the
/// emitted `FunctionConfig`) and the attribute name in diagnostics.
pub(crate) fn expand_handler(
    attr: TokenStream,
    item: TokenStream,
    kind: HandlerKind,
) -> TokenStream {
    let func = parse_macro_input!(item as ItemFn);

    // Parse the decorator arguments through the SHARED grammar. On a parse error,
    // keep the original fn (so the rest of the user's crate still type-checks) and
    // surface the diagnostic.
    let args = match parse_function_args(attr.into(), kind) {
        Ok(args) => args,
        Err(e) => {
            let err = e.to_compile_error();
            return quote! {
                #func
                #err
            }
            .into();
        }
    };

    let fn_ident = func.sig.ident.clone();
    let entry_name = args
        .explicit_name
        .as_ref()
        .map(|s| s.value())
        .unwrap_or_else(|| fn_ident.to_string());

    // Resolve the facade crate name ONCE; every emitted runtime/`inventory`/facade
    // path is routed through it so a macro-using crate needs only `modal-rust`.
    let facade = facade_path();

    // async fn -> reserved `typed_async!` shape (boundaries.md §3) is not yet
    // implemented in the runtime. Reject clearly; keep the original fn so the rest
    // of the user's crate still type-checks, and do NOT touch the sync path.
    if let Some(async_token) = func.sig.asyncness {
        let msg = format!(
            "{} does not yet support `async fn`: the reserved `typed_async!` shape \
             (boundaries.md §3) is not yet implemented in modal-rust-runtime. Use a \
             synchronous handler (it may `block_on` internally) for now.",
            kind.display(),
        );
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
            format!(
                "{} applies to free functions only; a `self` receiver cannot be a \
                 runner entrypoint",
                kind.display(),
            ),
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
            quote! { #facade::__private::runtime::typed_check!(#fn_ident) },
            &facade,
            &args,
        );
    }

    // Mode B: generate the named input type, the spread shim, the typed App methods,
    // and register the SHIM. First, validate every param is a plain owned
    // `ident: Type` (no `self`, already excluded above) and the handler carries no
    // generics/lifetimes/where-clause (the generated Input/shim can't be
    // monomorphized generically).
    if let Some(err) = mode_b_signature_error(&func, &params, kind) {
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
        quote! { #facade::__private::runtime::typed_check!(#shim_ident) },
        &facade,
        &args,
    );

    quote! {
        #func
        #generated
        #registration
    }
    .into()
}

/// Mode-A emission helper: keep the original fn verbatim and submit one facade
/// `Registration` whose handler is `#handler_expr` (here `typed!(#fn_ident)`),
/// with the decorator config in the same record.
pub(crate) fn emit_registration(
    func: &ItemFn,
    entry_name: &str,
    handler_expr: proc_macro2::TokenStream,
    check_expr: proc_macro2::TokenStream,
    facade: &proc_macro2::TokenStream,
    args: &FunctionArgs,
) -> TokenStream {
    let registration = build_registration(entry_name, handler_expr, check_expr, facade, args);
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
pub(crate) fn build_registration(
    entry_name: &str,
    handler_expr: proc_macro2::TokenStream,
    check_expr: proc_macro2::TokenStream,
    facade: &proc_macro2::TokenStream,
    args: &FunctionArgs,
) -> proc_macro2::TokenStream {
    let gpu_tok = match &args.gpu {
        Some(s) => quote! { ::core::option::Option::Some(#s) }, // &'static str literal
        None => quote! { ::core::option::Option::None },
    };
    let timeout_tok = match args.timeout_secs {
        Some(n) => {
            let n = n as u32;
            quote! { ::core::option::Option::Some(#n) }
        }
        None => quote! { ::core::option::Option::None },
    };
    let cache_tok = match args.cache {
        Some(b) => quote! { ::core::option::Option::Some(#b) },
        None => quote! { ::core::option::Option::None },
    };
    // `cpu`/`memory` are resolved to wire units (milli-cores / MiB) at parse time, so
    // each is a plain `Option<u32>` const-valid in the `static` initializer (exactly
    // like `timeout`). `None` emits `None` => byte-identical to a bare decorator.
    let milli_cpu_tok = match args.milli_cpu {
        Some(n) => quote! { ::core::option::Option::Some(#n) },
        None => quote! { ::core::option::Option::None },
    };
    let memory_mb_tok = match args.memory_mb {
        Some(n) => quote! { ::core::option::Option::Some(#n) },
        None => quote! { ::core::option::Option::None },
    };
    // `retries` is a plain `Option<u32>` const-valid in the `static` initializer
    // (exactly like `timeout`). `None` emits `None` => byte-identical to a bare
    // decorator (no retry policy on the wire).
    let retries_tok = match args.retries {
        Some(n) => quote! { ::core::option::Option::Some(#n) },
        None => quote! { ::core::option::Option::None },
    };
    // `retries_spec` is the canonicalized `Retries(..)` STRUCT form as a `&'static str`
    // SPEC (the facade hands it to the SDK's `parse_retries_spec`), const-valid in the
    // `static` initializer exactly like `gpu`/`schedule`. `None` emits `None` =>
    // byte-identical to a bare decorator (the int form / no retries stays unchanged).
    let retries_spec_tok = match &args.retries_spec {
        Some(s) => quote! { ::core::option::Option::Some(#s) },
        None => quote! { ::core::option::Option::None },
    };
    // Each autoscaler knob is a plain `Option<u32>` const-valid in the `static`
    // initializer (exactly like `timeout`/`retries`). `None` emits `None` =>
    // byte-identical to a bare decorator (no autoscaler_settings on the wire).
    let opt_u32_tok = |v: Option<u32>| match v {
        Some(n) => quote! { ::core::option::Option::Some(#n) },
        None => quote! { ::core::option::Option::None },
    };
    let min_containers_tok = opt_u32_tok(args.min_containers);
    let max_containers_tok = opt_u32_tok(args.max_containers);
    let buffer_containers_tok = opt_u32_tok(args.buffer_containers);
    let scaledown_window_tok = opt_u32_tok(args.scaledown_window);
    // `schedule` is canonicalized to a `&'static str` SPEC string (the facade hands it
    // to the SDK's `parse_schedule`), so it stays const-valid in the `static`
    // initializer exactly like `gpu`. `None` emits `None` => byte-identical to a bare
    // decorator (no schedule on the wire).
    let schedule_tok = match &args.schedule {
        Some(s) => quote! { ::core::option::Option::Some(#s) },
        None => quote! { ::core::option::Option::None },
    };
    // `secrets`/`volumes` are `&'static` slices on `FunctionConfig` (const-valid in
    // the `static` `inventory::submit!` initializer, exactly like `gpu`/`name`). An
    // empty list emits `&[]`, byte-identical to the bare default.
    let secrets_tok = {
        let items = args.secrets.iter();
        quote! { &[ #( #items ),* ] }
    };
    // `required_keys` (asserted on the named secrets) + `env` (inline-secret key/values)
    // are `&'static` slices on `FunctionConfig`, const-valid in the `static` initializer
    // exactly like `secrets`/`volumes`. Empty lists emit `&[]`, byte-identical to the
    // bare default.
    let required_keys_tok = {
        let items = args.required_keys.iter();
        quote! { &[ #( #items ),* ] }
    };
    let env_tok = {
        let items = args.env.iter().map(|(k, v)| quote! { (#k, #v) });
        quote! { &[ #( #items ),* ] }
    };
    let volumes_tok = {
        let items = args
            .volumes
            .iter()
            .map(|(mount, name)| quote! { (#mount, #name) });
        quote! { &[ #( #items ),* ] }
    };
    // `image` is the canonicalized per-function `Image(..)` SPEC string as a
    // `&'static str` (the facade parses it via `remote::parse_image_spec`), const-valid
    // in the `static` initializer exactly like `schedule`/`gpu`. `None` emits `None` =>
    // byte-identical to a bare decorator (the env-only base image stays in effect).
    let image_tok = match &args.image {
        Some(s) => quote! { ::core::option::Option::Some(#s) },
        None => quote! { ::core::option::Option::None },
    };
    // Web-endpoint marker: `#[endpoint]` threads the VALIDATED `method` (+ the
    // proxy-auth opt-in); a plain `#[function]` keeps the inert `None`/`false`
    // defaults ⇒ byte-identical wire. A `&'static str` literal / plain `bool`,
    // const-valid in the `static` initializer exactly like `gpu`/
    // `enable_memory_snapshot`.
    let webhook_method_tok = match &args.webhook_method {
        Some(s) => quote! { ::core::option::Option::Some(#s) },
        None => quote! { ::core::option::Option::None },
    };
    let webhook_requires_proxy_auth = args.webhook_requires_proxy_auth;

    quote! {
        #facade::__private::inventory::submit! {
            #facade::Registration {
                name: #entry_name,
                handler: #handler_expr,
                // The DECODE-ONLY `typed_check!` companion: powers the runner's
                // `--check-input` LOCAL validation so `modal-rust run` fails fast on a
                // bad-shape `--input` before any Modal call. Same `In` type as the
                // handler decodes; const-valid in the `static` initializer (a `fn`
                // pointer coercion, exactly like `handler`).
                check: ::core::option::Option::Some(#check_expr),
                // `#[function]` is never snapshot-enabled (memory snapshot is
                // `#[cls]`-only in v0), so the prime hook is always `None` here ⇒ inert.
                snapshot_prime: ::core::option::Option::None,
                config: #facade::FunctionConfig {
                    gpu: #gpu_tok,
                    timeout_secs: #timeout_tok,
                    cache: #cache_tok,
                    milli_cpu: #milli_cpu_tok,
                    memory_mb: #memory_mb_tok,
                    retries: #retries_tok,
                    retries_spec: #retries_spec_tok,
                    schedule: #schedule_tok,
                    min_containers: #min_containers_tok,
                    max_containers: #max_containers_tok,
                    buffer_containers: #buffer_containers_tok,
                    scaledown_window: #scaledown_window_tok,
                    secrets: #secrets_tok,
                    required_keys: #required_keys_tok,
                    env: #env_tok,
                    volumes: #volumes_tok,
                    image: #image_tok,
                    // Memory snapshot is `#[cls]`-only in v0 (the shared parser rejects
                    // the arg here), so the inert `false` keeps the wire byte-identical.
                    enable_memory_snapshot: false,
                    // Web-endpoint config: `#[endpoint]` threads its validated
                    // `method`/`requires_proxy_auth`; `#[function]` keeps the inert
                    // `None`/`false` ⇒ byte-identical wire. Const-valid in the `static`
                    // initializer.
                    webhook_method: #webhook_method_tok,
                    webhook_requires_proxy_auth: #webhook_requires_proxy_auth,
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
pub(crate) const SCALAR_DENYLIST: &[&str] = &[
    "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32",
    "f64", "bool", "char", "str", "String",
];

/// Classify a SINGLE param's type for Mode A vs Mode B (spec §1). Returns `true` iff
/// the type is a bare `Type::Path` with NO generic arguments whose last path segment
/// ident is NOT in [`SCALAR_DENYLIST`] (i.e. a user struct used as-is — Mode A).
/// Anything else (`&T`, `(A, B)`, `[T; N]`, a generic path like `Vec<u8>`, or a
/// denylisted scalar) is Mode B.
pub(crate) fn is_mode_a_param_type(ty: &Type) -> bool {
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
pub(crate) fn mode_b_signature_error(
    func: &ItemFn,
    params: &[&PatType],
    kind: HandlerKind,
) -> Option<proc_macro2::TokenStream> {
    // No generics / lifetimes / where-clauses on the handler: the generated Input /
    // shim cannot be monomorphized generically.
    if !func.sig.generics.params.is_empty() || func.sig.generics.where_clause.is_some() {
        return Some(
            syn::Error::new_spanned(
                &func.sig.generics,
                format!(
                    "plain {} handlers cannot be generic (no type/lifetime params or \
                     where-clauses): the generated input type cannot be monomorphized. \
                     Use concrete owned param types.",
                    kind.display(),
                ),
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
                    format!(
                        "plain {} params must be owned; use String / Vec<u8> instead \
                         of a borrowed `&str` / `&[u8]`",
                        kind.display(),
                    ),
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
pub(crate) fn result_ok_type(output: &ReturnType) -> proc_macro2::TokenStream {
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
pub(crate) fn to_pascal_case(snake: &str) -> String {
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

/// The `modal_runner!` expansion body (the `#[proc_macro]` entrypoint in `lib.rs`
/// delegates here — proc-macro crates may only export from the root; M1).
pub(crate) fn expand_modal_runner(input: TokenStream) -> TokenStream {
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
