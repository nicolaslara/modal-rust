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
//! ## async
//!
//! `async fn` handlers are detected and rejected with a clear `compile_error!`:
//! the reserved `typed_async!` shape (boundaries.md §3) is **not yet implemented**
//! in `modal-rust-runtime`, so emitting it would not compile. The sync path is
//! unaffected. When `typed_async!` lands, this arm switches from a diagnostic to
//! emitting `typed_async!(..)` with the same `HandlerFn` shape.
//!
//! ## Multi-arg (reserved, boundaries.md §3)
//!
//! The frozen argument shape is a single named JSON object. A single-argument
//! handler takes its `In` directly (the common case, used by `examples/add`). A
//! future multi-arg expansion will synthesize a private `#[derive(Deserialize)]`
//! named-field args struct + a shim registered via `typed!(shim)` — never a
//! positional array. Multi-arg is rejected today with a clear `compile_error!`
//! rather than silently mis-registering.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemFn, LitStr};

/// Attribute macro that registers a handler with the modal-rust runner via
/// `inventory`, producing the SAME registry shape as the manual `typed!` path.
///
/// See the crate-level docs for the full contract. Usage:
///
/// ```ignore
/// #[modal_rust::function]                  // name defaults to "add"
/// pub fn add(input: AddInput) -> anyhow::Result<AddOutput> { /* ... */ }
///
/// #[modal_rust::function(name = "add")]    // explicit name override
/// pub fn add(input: AddInput) -> anyhow::Result<AddOutput> { /* ... */ }
/// ```
#[proc_macro_attribute]
pub fn function(attr: TokenStream, item: TokenStream) -> TokenStream {
    let func = parse_macro_input!(item as ItemFn);

    // Parse the optional `name = "..."` argument. Default to the fn name.
    let mut explicit_name: Option<LitStr> = None;
    if !attr.is_empty() {
        let parser = syn::meta::parser(|meta| {
            if meta.path.is_ident("name") {
                explicit_name = Some(meta.value()?.parse()?);
                Ok(())
            } else {
                Err(meta.error("unsupported `#[modal_rust::function]` argument; only `name = \"...\"` is recognized"))
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

    // Frozen argument shape: a single named JSON object. v0 supports exactly one
    // argument (the handler's `In`), exactly like the manual `typed!(add)` path.
    // Multi-arg expansion (private args struct + shim) is reserved (boundaries.md
    // §3) but not implemented here; reject clearly rather than mis-register.
    let arg_count = func.sig.inputs.len();
    if arg_count != 1 {
        let msg = format!(
            "#[modal_rust::function] currently supports exactly one argument (the \
             handler's `In`), but `{fn_ident}` has {arg_count}. Multi-argument \
             expansion (a private named-field args struct + shim, boundaries.md \
             §3) is reserved but not yet implemented; wrap the parameters in a \
             single `#[derive(Deserialize)]` input struct for now."
        );
        let err = syn::Error::new_spanned(&func.sig.inputs, msg).to_compile_error();
        return quote! {
            #func
            #err
        }
        .into();
    }

    // Additive expansion: keep the original fn verbatim, then submit a
    // `Registration` whose handler is the SAME monomorphized `typed!` wrapper the
    // manual builder uses. `inventory::submit!` places this in a link section that
    // `Registry::from_inventory()` collects at runner startup. The `typed!` macro
    // expands to a block that defines a local `fn` and coerces it to a
    // `HandlerFn` pointer — a const-evaluable expression valid in the `static`
    // initializer `inventory::submit!` generates.
    let expanded = quote! {
        #func

        ::inventory::submit! {
            ::modal_rust_runtime::Registration {
                name: #entry_name,
                handler: ::modal_rust_runtime::typed!(#fn_ident),
            }
        }
    };

    expanded.into()
}
