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
use syn::punctuated::Punctuated;
use syn::{parse_macro_input, ItemFn, LitBool, LitInt, LitStr, Token};

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
    // The decorator config flows into the registration as a `FunctionConfig`. The
    // `gpu` literal is a `&'static str` (so the `static` `inventory::submit!`
    // initializer stays `const`-valid, matching `name: &'static str`); `timeout` is
    // narrowed `u64 -> u32` here. The bare form sets all three to `None` =>
    // `FunctionConfig::default()`, which the runner ignores (so behavior is
    // byte-identical; only the facade reads `config`).
    let gpu_tok = match &gpu {
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

    let expanded = quote! {
        #func

        ::inventory::submit! {
            ::modal_rust_runtime::Registration {
                name: #entry_name,
                handler: ::modal_rust_runtime::typed!(#fn_ident),
                config: ::modal_rust_runtime::FunctionConfig {
                    gpu: #gpu_tok,
                    timeout_secs: #timeout_tok,
                    cache: #cache_tok,
                    secrets: #secrets_tok,
                    volumes: #volumes_tok,
                },
            }
        }
    };

    expanded.into()
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
