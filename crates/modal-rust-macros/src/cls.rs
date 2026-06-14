//! `#[cls]` — load-once stateful classes (Cls v0, Shape A / Shape 1): the impl
//! walker/validator (the markers) and the emitter ([`emit_cls`]). Split out of
//! `lib.rs` mechanically (M1); the `#[proc_macro_attribute]` entrypoint stays in
//! `lib.rs` and delegates to [`expand_cls`].
//!
//! The decorator GRAMMAR does not live here: `#[cls(..)]`/`#[method(..)]` parse
//! through the SHARED [`parse_decorator_config`] (with [`HandlerKind::Cls`] as the
//! allow-set) and emit through the SHARED [`function_config_tokens`] (M2). The only
//! cls-specific config semantics is [`DecoratorConfig::merge_over`] below — the
//! class-default/method-override field-by-field merge.

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    parse_macro_input, FnArg, GenericArgument, ImplItem, ItemImpl, PathArguments, ReturnType, Type,
};

use crate::args::{function_config_tokens, parse_decorator_config, DecoratorConfig, HandlerKind};
use crate::emit::result_ok_type;
use crate::{facade_path, CLS_ENTRYPOINT_SEPARATOR};

impl DecoratorConfig {
    /// Merge a per-method override ON TOP of `self` (the class default), field by field:
    /// a `Some` method value wins, otherwise the class value is inherited. This is done
    /// at expansion time so the emitted `Registration` carries a fully-resolved config,
    /// byte-identical in shape to what `#[function]` emits (cls-design.md §4).
    ///
    /// `#[cls]`-only semantics, so it lives HERE, not in `args.rs` (M2: the shared
    /// grammar keeps no per-attribute behavior). The struct expression is TOTAL on
    /// purpose: adding a `DecoratorConfig` field is a compile error here until a merge
    /// rule is chosen. `explicit_name`/`webhook_*` are rejected by the parser for
    /// [`HandlerKind::Cls`], so they are always inert (`None`/`false`) on both sides;
    /// they still merge structurally rather than being silently dropped.
    pub(crate) fn merge_over(&self, over: &DecoratorConfig) -> DecoratorConfig {
        DecoratorConfig {
            explicit_name: over
                .explicit_name
                .clone()
                .or_else(|| self.explicit_name.clone()),
            gpu: over.gpu.clone().or_else(|| self.gpu.clone()),
            timeout_secs: over.timeout_secs.or(self.timeout_secs),
            cache: over.cache.or(self.cache),
            milli_cpu: over.milli_cpu.or(self.milli_cpu),
            memory_mb: over.memory_mb.or(self.memory_mb),
            retries: over.retries.or(self.retries),
            retries_spec: over
                .retries_spec
                .clone()
                .or_else(|| self.retries_spec.clone()),
            schedule: over.schedule.clone().or_else(|| self.schedule.clone()),
            min_containers: over.min_containers.or(self.min_containers),
            max_containers: over.max_containers.or(self.max_containers),
            buffer_containers: over.buffer_containers.or(self.buffer_containers),
            scaledown_window: over.scaledown_window.or(self.scaledown_window),
            secrets: over.secrets.clone().or_else(|| self.secrets.clone()),
            required_keys: over
                .required_keys
                .clone()
                .or_else(|| self.required_keys.clone()),
            env: over.env.clone().or_else(|| self.env.clone()),
            volumes: over.volumes.clone().or_else(|| self.volumes.clone()),
            image: over.image.clone().or_else(|| self.image.clone()),
            enable_memory_snapshot: over.enable_memory_snapshot.or(self.enable_memory_snapshot),
            webhook_method: over
                .webhook_method
                .clone()
                .or_else(|| self.webhook_method.clone()),
            webhook_requires_proxy_auth: over.webhook_requires_proxy_auth
                || self.webhook_requires_proxy_auth,
            // `web_server_*` are `#[web_server]`-only (the parser rejects them for
            // `#[cls]`/`#[method]`), so both sides are always inert `None`; they still
            // merge structurally rather than being silently dropped.
            web_server_port: over.web_server_port.or(self.web_server_port),
            web_server_startup_timeout: over
                .web_server_startup_timeout
                .or(self.web_server_startup_timeout),
        }
    }
}

/// One method collected from the `#[cls]` impl block: its `#[method(..)]` override
/// config, the user's method `fn`, and its non-receiver params.
pub(crate) struct ClsMethod {
    /// The method ident (`embed`); the entrypoint name is `"<Class>.<embed>"`.
    pub(crate) ident: syn::Ident,
    /// Effective per-method config (class default merged with the method override).
    pub(crate) config: DecoratorConfig,
    /// `(ident, type)` for each non-receiver param, in declaration order.
    pub(crate) params: Vec<(syn::Ident, Type)>,
    /// The method's return type (`-> anyhow::Result<Vec<f32>>`), copied verbatim for
    /// the shim so the `typed!` error specialization still selects the right path.
    pub(crate) output: ReturnType,
}

/// The `#[cls]` expansion body (the `#[proc_macro_attribute]` entrypoint in
/// `lib.rs` delegates here — proc-macro crates may only export from the root; M1).
pub(crate) fn expand_cls(attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut item_impl = parse_macro_input!(item as ItemImpl);

    // Parse the CLASS-level decorator config (the default inherited by each method)
    // through the SHARED grammar; `HandlerKind::Cls` is the allow-set (rejects `name`
    // and the endpoint-only keys, accepts `enable_memory_snapshot`).
    let class_config = match parse_decorator_config(attr.into(), HandlerKind::Cls) {
        Ok(c) => c,
        Err(e) => return e.to_compile_error().into(),
    };

    // The self type must be a bare path so we can name the class ident (`Embedder`).
    let class_ident = match cls_self_ident(&item_impl) {
        Ok(id) => id,
        Err(e) => return e.to_compile_error().into(),
    };

    // A trait impl (`impl Trait for T`) has no place for the class-config + handle; the
    // markers belong on the inherent impl.
    if item_impl.trait_.is_some() {
        return syn::Error::new_spanned(
            &item_impl,
            "#[cls] applies to an INHERENT impl block (`impl Embedder { .. }`), not a \
             trait impl",
        )
        .to_compile_error()
        .into();
    }
    if !item_impl.generics.params.is_empty() || item_impl.generics.where_clause.is_some() {
        return syn::Error::new_spanned(
            &item_impl.generics,
            "#[cls] does not support generic/lifetime params or where-clauses on the \
             impl block in v0",
        )
        .to_compile_error()
        .into();
    }

    let facade = facade_path();

    // Walk the impl items: find exactly one #[enter], collect #[method]s, reject #[exit]
    // (a v0 non-goal), and STRIP the consumed markers from the methods we keep so the
    // user's own `impl` stays directly callable.
    let mut enter_ident: Option<syn::Ident> = None;
    // Whether #[enter] returns a `Result<Self, _>` (fallible) vs a bare `Self`.
    let mut enter_fallible = false;
    let mut methods: Vec<ClsMethod> = Vec::new();
    let mut errors: Vec<proc_macro2::TokenStream> = Vec::new();

    for impl_item in item_impl.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };
        let marker = match take_cls_marker(method) {
            Ok(m) => m,
            Err(e) => {
                errors.push(e.to_compile_error());
                continue;
            }
        };
        match marker {
            Some(ClsMarker::Exit) => {
                errors.push(
                    syn::Error::new_spanned(
                        &method.sig.ident,
                        "#[exit] is a Cls v0 NON-GOAL: deterministic teardown does NOT \
                         run (warm containers are GC'd). Remove it; per-call cleanup \
                         belongs inside the method. The marker is reserved for a future \
                         release (cls-design.md §9.6 / cls-devx-design.md §6).",
                    )
                    .to_compile_error(),
                );
            }
            Some(ClsMarker::Enter) => {
                if enter_ident.is_some() {
                    errors.push(
                        syn::Error::new_spanned(
                            &method.sig.ident,
                            "#[cls] allows exactly ONE #[enter] method",
                        )
                        .to_compile_error(),
                    );
                } else {
                    match validate_enter_sig(method, &class_ident) {
                        Ok(fallible) => {
                            enter_ident = Some(method.sig.ident.clone());
                            enter_fallible = fallible;
                        }
                        Err(e) => errors.push(e.to_compile_error()),
                    }
                }
            }
            Some(ClsMarker::Method(over_tokens)) => {
                match build_cls_method(method, &class_config, over_tokens) {
                    Ok(m) => methods.push(m),
                    Err(e) => errors.push(e.to_compile_error()),
                }
            }
            None => {} // a plain helper method (no marker): left untouched, not registered
        }
    }

    if enter_ident.is_none() {
        errors.push(
            syn::Error::new_spanned(
                &item_impl.self_ty,
                "#[cls] requires exactly ONE #[enter] method returning `Self` / \
                 `anyhow::Result<Self>` (it builds the load-once singleton)",
            )
            .to_compile_error(),
        );
    }
    if methods.is_empty() && errors.is_empty() {
        errors.push(
            syn::Error::new_spanned(
                &item_impl.self_ty,
                "#[cls] requires at least one #[method] (an `&self` fn returning \
                 `Result<T>`)",
            )
            .to_compile_error(),
        );
    }

    if !errors.is_empty() {
        // Keep the (marker-stripped) impl so the rest of the crate still type-checks,
        // and surface every diagnostic at once.
        return quote! {
            #item_impl
            #( #errors )*
        }
        .into();
    }

    let enter_ident = enter_ident.expect("checked above");
    let generated = emit_cls(
        &class_ident,
        &enter_ident,
        enter_fallible,
        &methods,
        &facade,
    );

    quote! {
        #item_impl
        #generated
    }
    .into()
}

/// The three inert inner markers `#[cls]` consumes.
pub(crate) enum ClsMarker {
    Enter,
    /// The `#[method(<override>)]` tokens (empty for a bare `#[method]`).
    Method(proc_macro2::TokenStream),
    Exit,
}

/// Find and REMOVE a `#[enter]` / `#[method(..)]` / `#[exit]` marker attribute from a
/// method, returning which one (if any). The marker is consumed so it does not survive
/// onto the user's own `impl` (it is not a real attribute). At most one marker per
/// method is allowed.
pub(crate) fn take_cls_marker(method: &mut syn::ImplItemFn) -> syn::Result<Option<ClsMarker>> {
    let mut found: Option<ClsMarker> = None;
    let mut kept = Vec::with_capacity(method.attrs.len());
    for attr in method.attrs.drain(..) {
        // `#[endpoint]` is free-fn-only in v0 (web-endpoints spec §5): a stateful
        // `#[cls]`+web method is an explicit follow-up, NOT silently ignored. The cls
        // expansion sees the method attrs before rustc would resolve them, so reject
        // HERE with a pointed diagnostic. Match the LAST path segment so the
        // facade-qualified `#[modal_rust::endpoint]` spelling is caught too.
        if attr
            .path()
            .segments
            .last()
            .is_some_and(|s| s.ident == "endpoint")
        {
            return Err(syn::Error::new_spanned(
                &attr,
                "#[endpoint] is free-fn-only in v0: it cannot be applied to a #[cls] \
                 method (stateful web endpoints are a follow-up). Move the handler to \
                 a free fn decorated `#[modal_rust::endpoint(method = \"...\")]` — it \
                 is still a normal function and may call into the class.",
            ));
        }
        let ident = attr.path().get_ident().map(|i| i.to_string());
        let marker = match ident.as_deref() {
            Some("enter") => Some(ClsMarker::Enter),
            Some("exit") => Some(ClsMarker::Exit),
            Some("method") => {
                // `#[method]` (no args) or `#[method(gpu = "..")]`.
                let tokens = match &attr.meta {
                    syn::Meta::Path(_) => proc_macro2::TokenStream::new(),
                    syn::Meta::List(list) => list.tokens.clone(),
                    syn::Meta::NameValue(_) => {
                        return Err(syn::Error::new_spanned(
                            &attr,
                            "#[method] takes a parenthesized config list, e.g. \
                             #[method(gpu = \"T4\")], not `#[method = ..]`",
                        ));
                    }
                };
                Some(ClsMarker::Method(tokens))
            }
            _ => None,
        };
        match marker {
            Some(m) => {
                if found.is_some() {
                    return Err(syn::Error::new_spanned(
                        &attr,
                        "a #[cls] method may carry at most ONE of #[enter]/#[method]/#[exit]",
                    ));
                }
                found = Some(m);
            }
            None => kept.push(attr),
        }
    }
    method.attrs = kept;
    Ok(found)
}

/// The self-type ident of the impl block (`Embedder`), or a clear error if the self
/// type is not a bare path.
pub(crate) fn cls_self_ident(item_impl: &ItemImpl) -> syn::Result<syn::Ident> {
    match item_impl.self_ty.as_ref() {
        Type::Path(tp) if tp.qself.is_none() => tp
            .path
            .segments
            .last()
            .map(|s| s.ident.clone())
            .ok_or_else(|| {
                syn::Error::new_spanned(
                    &item_impl.self_ty,
                    "#[cls] self type must be a named struct",
                )
            }),
        other => Err(syn::Error::new_spanned(
            other,
            "#[cls] applies to `impl <StructName> { .. }` where the self type is a \
             named struct",
        )),
    }
}

/// Validate the `#[enter]` signature: no `&self`/`&mut self` receiver, no params, no
/// generics, and a return of `Self` / `Result<Self, _>` (we accept both the fallible
/// `-> anyhow::Result<Self>` and the infallible `-> Self`). Returns `true` iff the
/// return is fallible (a `Result<Self, _>`), so the accessor body matches the form.
pub(crate) fn validate_enter_sig(
    method: &syn::ImplItemFn,
    class_ident: &syn::Ident,
) -> syn::Result<bool> {
    if let Some(FnArg::Receiver(recv)) = method.sig.inputs.first() {
        return Err(syn::Error::new_spanned(
            recv,
            "#[enter] must be an associated fn with NO receiver: `fn load() -> \
             anyhow::Result<Self>` (it CONSTRUCTS the value moved into the singleton)",
        ));
    }
    if !method.sig.inputs.is_empty() {
        return Err(syn::Error::new_spanned(
            &method.sig.inputs,
            "#[enter] takes NO parameters in v0 (class params are deferred to Shape B). \
             Read per-deployment config from std::env via #[cls(secrets = [..])].",
        ));
    }
    if !method.sig.generics.params.is_empty() || method.sig.generics.where_clause.is_some() {
        return Err(syn::Error::new_spanned(
            &method.sig.generics,
            "#[enter] cannot be generic",
        ));
    }
    // Return must be `Self` / the class name, or `Result<Self, _>` (syntactic check).
    match classify_enter_return(&method.sig.output, class_ident) {
        Some(fallible) => Ok(fallible),
        None => Err(syn::Error::new_spanned(
            &method.sig.output,
            "#[enter] must return `Self` / `anyhow::Result<Self>` (the constructed, \
             loaded value the macro moves into the load-once singleton)",
        )),
    }
}

/// Classify an `#[enter]` return: `Some(false)` = bare `Self`/`<Class>` (infallible),
/// `Some(true)` = `Result<Self, _>`/`Result<<Class>, _>` (fallible), `None` = not a
/// valid enter return. A proc-macro cannot resolve types, so this accepts the common
/// spellings syntactically.
pub(crate) fn classify_enter_return(output: &ReturnType, class_ident: &syn::Ident) -> Option<bool> {
    let ReturnType::Type(_, ty) = output else {
        return None; // `-> ()` is never a valid enter
    };
    let Type::Path(tp) = ty.as_ref() else {
        return None;
    };
    let last = tp.path.segments.last()?;
    // Bare `Self` / `Embedder` -> infallible.
    if matches!(last.arguments, PathArguments::None) {
        if last.ident == "Self" || &last.ident == class_ident {
            return Some(false);
        }
        return None;
    }
    // `Result<Self, _>` / `anyhow::Result<Embedder>` -> fallible, if the Ok type is Self.
    if last.ident == "Result" {
        if let PathArguments::AngleBracketed(args) = &last.arguments {
            if let Some(GenericArgument::Type(Type::Path(itp))) = args.args.first() {
                if let Some(iseg) = itp.path.segments.last() {
                    if matches!(iseg.arguments, PathArguments::None)
                        && (iseg.ident == "Self" || &iseg.ident == class_ident)
                    {
                        return Some(true);
                    }
                }
            }
        }
    }
    None
}

/// Build a [`ClsMethod`] from a `#[method(..)]`-marked fn: validate `&self`-only +
/// `Result<T>` return + no generics, strip the receiver, classify params, and merge
/// the method override on top of the class config.
pub(crate) fn build_cls_method(
    method: &syn::ImplItemFn,
    class_config: &DecoratorConfig,
    over_tokens: proc_macro2::TokenStream,
) -> syn::Result<ClsMethod> {
    let ident = method.sig.ident.clone();

    if let Some(async_token) = method.sig.asyncness {
        return Err(syn::Error::new_spanned(
            async_token,
            "#[method] does not support `async fn` in v0 (the runtime `typed_async!` \
             shape is not yet implemented). Use a synchronous method.",
        ));
    }
    if !method.sig.generics.params.is_empty() || method.sig.generics.where_clause.is_some() {
        return Err(syn::Error::new_spanned(
            &method.sig.generics,
            "#[method] cannot be generic: the generated input type cannot be \
             monomorphized. Use concrete owned param types.",
        ));
    }

    // Receiver MUST be `&self` (not `self` / `&mut self`): the singleton hands out a
    // shared `&'static` borrow (cls-devx-design.md §6.2). Mutable per-container state =
    // interior mutability inside the struct.
    match method.sig.inputs.first() {
        Some(FnArg::Receiver(recv)) => {
            if recv.reference.is_none() || recv.mutability.is_some() {
                return Err(syn::Error::new_spanned(
                    recv,
                    "#[method] must take `&self` in v0 (not `self` / `&mut self`): the \
                     load-once singleton is shared. Use interior mutability (a \
                     Mutex/RwLock field) for mutable per-container state.",
                ));
            }
        }
        _ => {
            return Err(syn::Error::new_spanned(
                &method.sig,
                "#[method] must take `&self` (it reads the loaded singleton state)",
            ));
        }
    }

    // The non-receiver params become the generated `Input` fields. Each must be a plain
    // owned `ident: Type` (same bar as `#[function]` Mode B), validated here.
    let mut params: Vec<(syn::Ident, Type)> = Vec::new();
    for arg in method.sig.inputs.iter().skip(1) {
        let FnArg::Typed(pt) = arg else {
            continue; // a second receiver is impossible after the first
        };
        let pat_ident = match pt.pat.as_ref() {
            syn::Pat::Ident(pi) if pi.subpat.is_none() => pi.ident.clone(),
            _ => {
                return Err(syn::Error::new_spanned(
                    pt,
                    "name each #[method] parameter so it can become an input field \
                     (a plain `ident: Type`, no destructuring)",
                ));
            }
        };
        if matches!(pt.ty.as_ref(), Type::Reference(_)) {
            return Err(syn::Error::new_spanned(
                pt,
                "#[method] params must be owned; use String / Vec<u8> instead of a \
                 borrowed `&str` / `&[u8]`",
            ));
        }
        params.push((pat_ident, (*pt.ty).clone()));
    }

    let method_over = parse_decorator_config(over_tokens, HandlerKind::Cls)?;
    let config = class_config.merge_over(&method_over);

    Ok(ClsMethod {
        ident,
        config,
        params,
        output: method.sig.output.clone(),
    })
}

/// Emit the per-class singleton + retry accessor, and per method the auto-IO module,
/// the spread shim, the `inventory::submit!` registration, and the handle methods +
/// extension trait.
pub(crate) fn emit_cls(
    class_ident: &syn::Ident,
    enter_ident: &syn::Ident,
    enter_fallible: bool,
    methods: &[ClsMethod],
    facade: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    // The process-global singleton + the get-or-try-init accessor. Caches SUCCESS only,
    // so an #[enter] `Err` is surfaced (as a function_error) and RETRIED on the next
    // call — a transient load failure never poisons the warm container
    // (cls-devx-design.md §5.2 / §6.3). Sequential serve (v0) makes the benign
    // double-build race a plain get-then-get_or_init allows impossible.
    let singleton_static = format_ident!(
        "__MODAL_RUST_CLS_{}",
        class_ident.to_string().to_uppercase()
    );
    let accessor = format_ident!(
        "__modal_rust_cls_{}",
        class_ident.to_string().to_lowercase()
    );

    // The #[enter] return is normalized to a `Result<Self, anyhow::Error>` token expr.
    // Fallible: `Embedder::load().map_err(anyhow::Error::from)`; infallible: wrap in
    // `Ok(..)`. Detected syntactically by the macro so ONE accessor body fits both.
    let enter_call = if enter_fallible {
        quote! {
            ::core::result::Result::map_err(
                #class_ident::#enter_ident(),
                ::core::convert::Into::<::anyhow::Error>::into,
            )
        }
    } else {
        quote! { ::core::result::Result::<#class_ident, ::anyhow::Error>::Ok(#class_ident::#enter_ident()) }
    };

    let singleton = quote! {
        #[doc(hidden)]
        static #singleton_static: ::std::sync::OnceLock<#class_ident> = ::std::sync::OnceLock::new();

        /// get-or-try-init: return the cached singleton, else run #[enter] ONCE; cache
        /// only on success so a transient failure retries on the next call.
        #[doc(hidden)]
        fn #accessor() -> ::core::result::Result<&'static #class_ident, #facade::RunnerError> {
            if let ::core::option::Option::Some(v) = #singleton_static.get() {
                return ::core::result::Result::Ok(v);
            }
            // `#[enter]` may return `Self` (infallible) or `anyhow::Result<Self>`; the
            // macro emits the matching normalization to `Result<Self, anyhow::Error>`
            // so this body is identical for both forms.
            match #enter_call {
                ::core::result::Result::Ok(v) => {
                    ::core::result::Result::Ok(#singleton_static.get_or_init(|| v))
                }
                ::core::result::Result::Err(e) => {
                    ::core::result::Result::Err(#facade::RunnerError::function_opaque(e))
                }
            }
        }
    };

    // The SNAPSHOT-PRIME hook. The class is snapshot-enabled iff ANY method resolved
    // `enable_memory_snapshot = true` (the class-level `#[cls(enable_memory_snapshot =
    // true)]` flag is inherited by every method; a per-method override can also set it).
    // When enabled, emit ONE free fn matching `fn() -> Result<(), RunnerError>` that
    // FORCES the EXISTING singleton accessor (running `#[enter]` once inside Modal's
    // snapshot freeze window) and set `snapshot_prime: Some(..)` on EACH method's
    // `Registration`. When disabled, no prime fn is emitted and every `snapshot_prime`
    // is `None` ⇒ inert, byte-identical to before.
    let snapshot_enabled = methods
        .iter()
        .any(|m| m.config.enable_memory_snapshot.unwrap_or(false));
    let prime_ident = format_ident!("__modal_snapshot_prime_{}", class_ident);
    let prime_fn = if snapshot_enabled {
        quote! {
            /// Snapshot-prime hook for this `#[cls]`: forces the EXISTING load-once
            /// singleton (running `#[enter]` once), discarding the borrow. Fired by the
            /// serve loop on a `prime` frame so `#[enter]` lands inside Modal's
            /// memory-snapshot freeze window. Reuses the SAME accessor as request
            /// dispatch (no second singleton).
            #[doc(hidden)]
            #[allow(non_snake_case)] // the ident embeds the PascalCase class name
            fn #prime_ident() -> ::core::result::Result<(), #facade::RunnerError> {
                #accessor().map(|_| ())
            }
        }
    } else {
        quote! {}
    };

    // Per-method codegen.
    let mut method_mods = Vec::new();
    let mut handle_methods = Vec::new();
    for m in methods {
        let method_ident = &m.ident;
        let entry_name = format!(
            "{}{}{}",
            class_ident, CLS_ENTRYPOINT_SEPARATOR, method_ident
        );
        // The generated mod ident uses `_` (an ident cannot contain `.`).
        let mod_ident = format_ident!("{}_{}", class_ident, method_ident);
        let shim_ident = format_ident!("__modal_rust_cls_shim_{}_{}", class_ident, method_ident);

        let field_idents: Vec<&syn::Ident> = m.params.iter().map(|(id, _)| id).collect();
        let field_types: Vec<&Type> = m.params.iter().map(|(_, ty)| ty).collect();
        let output_ty = result_ok_type(&m.output);
        let orig_output = &m.output;

        // The ONE shared `FunctionConfig` emitter (M2): the same tokens
        // `build_registration` emits for `#[function]`/`#[endpoint]`, so a knob
        // threaded there rides the cls registration too — per-attribute drift is
        // structurally impossible.
        let config = function_config_tokens(&m.config, facade);

        // This method's snapshot-prime: `Some(<class prime fn>)` iff this method opted
        // into memory snapshot (the prime fn is emitted once above when ANY method does),
        // else `None` ⇒ the serve loop's `prime` frame is a no-op for it.
        let snapshot_prime_tok = if m.config.enable_memory_snapshot.unwrap_or(false) {
            quote! { ::core::option::Option::Some(#prime_ident) }
        } else {
            quote! { ::core::option::Option::None }
        };

        method_mods.push(quote! {
            #[doc(hidden)]
            #[allow(non_snake_case)]
            pub mod #mod_ident {
                #[allow(unused_imports)]
                use super::*;

                /// Auto-generated named input for this #[method]: one `pub` field per
                /// non-receiver param. Serializes to the frozen named JSON object.
                #[derive(::serde::Serialize, ::serde::Deserialize)]
                pub struct Input {
                    #( pub #field_idents : #field_types ),*
                }

                /// Auto-generated output alias = the method's return `Ok` type.
                pub type Output = #output_ty;
            }

            /// Private SPREAD shim: resolves the load-once singleton (running #[enter]
            /// once), then calls the user method with the decoded input fields.
            /// Registered via the UNCHANGED `typed!`, so decode/call/encode + the five
            /// error kinds are byte-identical to a free fn (only the dispatch resolves
            /// a singleton first).
            #[doc(hidden)]
            #[allow(non_snake_case)] // the ident embeds the PascalCase class name
            fn #shim_ident(__modal_rust_in: self::#mod_ident::Input) #orig_output {
                let __modal_rust_self = #accessor()
                    .map_err(|e| ::anyhow::anyhow!(e.to_string()))?;
                __modal_rust_self.#method_ident( #( __modal_rust_in.#field_idents ),* )
            }

            #facade::__private::inventory::submit! {
                #facade::Registration {
                    name: #entry_name,
                    handler: #facade::__private::runtime::typed!(#shim_ident),
                    // DECODE-ONLY companion for `--check-input` local validation
                    // (fail fast before any Modal call); same `In` as the handler.
                    check: ::core::option::Option::Some(
                        #facade::__private::runtime::typed_check!(#shim_ident)
                    ),
                    // `Some(<class prime fn>)` for a snapshot-enabled `#[cls]` method,
                    // else `None` (byte-identical default ⇒ the serve loop's `prime`
                    // frame is a no-op). The prime forces the shared singleton, so a
                    // class with several methods is primed once even if called N times.
                    snapshot_prime: #snapshot_prime_tok,
                    config: #config,
                    package: ::core::env!("CARGO_PKG_NAME"),
                }
            }
        });

        handle_methods.push(quote! {
            #[allow(clippy::too_many_arguments)]
            pub fn #method_ident(
                &self,
                #( #field_idents : #field_types ),*
            ) -> #facade::TypedCall<'a, self::#mod_ident::Input, self::#mod_ident::Output> {
                #facade::TypedCall::new(
                    self.app,
                    #entry_name,
                    self::#mod_ident::Input { #( #field_idents ),* },
                )
            }
        });
    }

    // The handle + extension trait (the only NEW codegen vs `#[function]`).
    let handle_ident = format_ident!("{}Handle", class_ident);
    let trait_ident = format_ident!("{}Cls", class_ident);
    // The accessor method on App is the snake_case class name (`embedder`).
    let app_method = format_ident!("{}", to_snake_case(&class_ident.to_string()));

    let handle = quote! {
        /// A cheap borrowing handle to one #[cls] class on an `App` (mirrors Python's
        /// `Embedder()`): its methods return `TypedCall`, chaining into
        /// `.local()/.remote()/.spawn()/.map()`.
        pub struct #handle_ident<'a> {
            app: &'a #facade::App,
        }

        /// Brings `app.#app_method()` into scope. Implemented for the facade `App`; one
        /// trait per class keeps coherence trivial (mirrors the free-fn `<Pascal>Call`).
        pub trait #trait_ident {
            fn #app_method(&self) -> #handle_ident<'_>;
        }

        impl #trait_ident for #facade::App {
            fn #app_method(&self) -> #handle_ident<'_> {
                #handle_ident { app: self }
            }
        }

        impl<'a> #handle_ident<'a> {
            #( #handle_methods )*
        }
    };

    quote! {
        #singleton
        #prime_fn
        #( #method_mods )*
        #handle
    }
}

/// Convert a PascalCase class ident to snake_case for the `app.<class>()` accessor
/// (`Embedder` -> `embedder`, `MyEmbedder` -> `my_embedder`).
pub(crate) fn to_snake_case(pascal: &str) -> String {
    let mut out = String::with_capacity(pascal.len() + 4);
    for (i, ch) in pascal.chars().enumerate() {
        if ch.is_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}
