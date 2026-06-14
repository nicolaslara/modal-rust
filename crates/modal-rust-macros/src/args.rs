//! The SHARED decorator grammar for `#[function]` / `#[endpoint]` / `#[cls]` +
//! `#[method]`: [`HandlerKind`] (the allow-set), [`DecoratorConfig`] (the ONE field
//! record), [`parse_decorator_config`] (the ONE parser), and
//! [`function_config_tokens`] (the ONE `FunctionConfig` emitter). Originally split
//! out of `lib.rs` mechanically (M1); the `#[cls]` copy of the grammar/emitter was
//! folded in here (M2) so a knob is parsed, validated, and emitted exactly once —
//! per-attribute drift (e.g. webhook fields threaded in one emitter but hardcoded
//! in the other) is structurally impossible.

use syn::{LitBool, LitInt, LitStr};

use crate::specs::{
    parse_cpu_to_milli, parse_image_to_spec, parse_retries_to_spec, parse_schedule_to_spec,
    parse_str_list, parse_str_map,
};

/// Which user-facing attribute is expanding through the SHARED parse+emit path:
/// `#[function]` (the plain handler), `#[endpoint]` (the same handler + the
/// web-endpoint marker), or `#[cls]`/`#[method]` (the stateful-class config). ONE
/// parser/emitter serves all of them (web-endpoints spec §5 — no forked grammar);
/// the kind only gates the per-attribute keys (`name`, the endpoint-only
/// `method`/`requires_proxy_auth`, the cls-only `enable_memory_snapshot`) and the
/// attribute name in diagnostics.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HandlerKind {
    Function,
    Endpoint,
    WebServer,
    Cls,
}

impl HandlerKind {
    /// The attribute name as the user spells it, for diagnostics.
    pub(crate) fn display(self) -> &'static str {
        match self {
            HandlerKind::Function => "#[modal_rust::function]",
            HandlerKind::Endpoint => "#[modal_rust::endpoint]",
            HandlerKind::WebServer => "#[modal_rust::web_server]",
            HandlerKind::Cls => "#[modal_rust::cls]",
        }
    }
}

/// The HTTP verbs `#[endpoint(method = ..)]` accepts (uppercase, validated at
/// expansion time so a typo is a compile error, never a live-deploy surprise).
pub(crate) const ENDPOINT_METHODS: &[&str] = &["GET", "POST", "PUT", "DELETE", "PATCH"];

/// The expected `#[endpoint]` syntax, embedded in the missing/invalid-`method`
/// compile errors so the fix is copy-pasteable from the diagnostic.
pub(crate) const ENDPOINT_EXPECTED_SYNTAX: &str = "#[modal_rust::endpoint(method = \"GET\"|\"POST\"|\"PUT\"|\"DELETE\"|\"PATCH\", <any #[function] config>)]";

/// The expected `#[web_server]` syntax, embedded in the missing-`port` compile error
/// so the fix is copy-pasteable from the diagnostic.
pub(crate) const WEB_SERVER_EXPECTED_SYNTAX: &str =
    "#[modal_rust::web_server(port = <u16>, startup_timeout = <secs, optional>, <any #[function] config>)]";

/// Every argument the SHARED decorator grammar parses, in ONE record so the parse
/// ([`parse_decorator_config`]) and emit ([`function_config_tokens`]) paths have a
/// single seam for every attribute.
///
/// The list fields are `Option<Vec<..>>` because `#[cls]` must distinguish UNSET
/// (inherit the class value in [`merge_over`](DecoratorConfig::merge_over)) from an
/// explicit empty override; for `#[function]`/`#[endpoint]` both emit the same `&[]`.
/// The `webhook_*` fields are the `#[endpoint]`-only extras (always `None`/`false`
/// elsewhere ⇒ inert, byte-identical wire); `enable_memory_snapshot` is the
/// `#[cls]`-only opt-in (always `None` ⇒ the inert `false` elsewhere).
#[derive(Default, Clone)]
pub(crate) struct DecoratorConfig {
    pub(crate) explicit_name: Option<LitStr>,
    pub(crate) gpu: Option<LitStr>,
    pub(crate) timeout_secs: Option<u64>,
    pub(crate) cache: Option<bool>,
    pub(crate) milli_cpu: Option<u32>,
    pub(crate) memory_mb: Option<u32>,
    pub(crate) retries: Option<u32>,
    pub(crate) retries_spec: Option<String>,
    pub(crate) schedule: Option<String>,
    pub(crate) min_containers: Option<u32>,
    pub(crate) max_containers: Option<u32>,
    pub(crate) buffer_containers: Option<u32>,
    pub(crate) scaledown_window: Option<u32>,
    // `None` = unset (inherit on `#[cls]`). `Some(vec)` = explicitly set (override,
    // even if empty).
    pub(crate) secrets: Option<Vec<String>>,
    pub(crate) required_keys: Option<Vec<String>>,
    pub(crate) env: Option<Vec<(String, String)>>,
    pub(crate) volumes: Option<Vec<(String, String)>>,
    // `None` = unset (inherit). `Some(spec)` = an explicit `image = Image(..)` override.
    pub(crate) image: Option<String>,
    // `None` = unset (inherit / the inert `false`). `Some(bool)` = an explicit
    // `enable_memory_snapshot = ..` opt-in (memory snapshot is `#[cls]`-only in v0;
    // it captures the `#[enter]` load).
    pub(crate) enable_memory_snapshot: Option<bool>,
    /// `method = "POST"` — the validated HTTP verb (`#[endpoint]`-only; REQUIRED there).
    pub(crate) webhook_method: Option<LitStr>,
    /// `requires_proxy_auth = true` — Modal proxy-auth opt-in (`#[endpoint]`-only;
    /// default `false` = PUBLIC, matching Modal).
    pub(crate) webhook_requires_proxy_auth: bool,
    /// `port = 3000` — the TCP port a `#[web_server]` handler binds (`#[web_server]`-only;
    /// REQUIRED there). `None` everywhere else ⇒ inert, byte-identical wire.
    pub(crate) web_server_port: Option<u16>,
    /// `startup_timeout = 30` — optional seconds Modal waits for the `#[web_server]`
    /// port to come up (`#[web_server]`-only). `None` ⇒ Modal default.
    pub(crate) web_server_startup_timeout: Option<u32>,
}

/// Parse a `#[function(..)]` / `#[endpoint(..)]` / `#[cls(..)]` / `#[method(..)]`
/// decorator argument list — the ONE shared grammar (web-endpoints spec §5). All
/// arguments are optional for a `#[function]`; an `#[endpoint]` additionally accepts
/// `requires_proxy_auth = <bool>` and REQUIRES `method = "GET"|"POST"|"PUT"|"DELETE"|
/// "PATCH"`; a `#[cls]`/`#[method]` additionally accepts `enable_memory_snapshot =
/// <bool>` and REJECTS `name =` (the entrypoint is derived as `"<Class>.<method>"`).
/// The bare form sets nothing, so the emitted facade `FunctionConfig` is `default()`
/// (all `None`) — runtime-observable behavior stays byte-identical.
pub(crate) fn parse_decorator_config(
    tokens: proc_macro2::TokenStream,
    kind: HandlerKind,
) -> syn::Result<DecoratorConfig> {
    let mut cfg = DecoratorConfig::default();
    if !tokens.is_empty() {
        let parser = syn::meta::parser(|meta| {
            if meta.path.is_ident("name") {
                if kind == HandlerKind::Cls {
                    return Err(meta.error(
                        "`name` is not valid on `#[cls]`/`#[method]`: the entrypoint name \
                         is derived as \"<Class>.<method>\". Rename the method instead.",
                    ));
                }
                cfg.explicit_name = Some(meta.value()?.parse()?);
                Ok(())
            } else if meta.path.is_ident("gpu") {
                cfg.gpu = Some(meta.value()?.parse()?); // LitStr
                Ok(())
            } else if meta.path.is_ident("timeout") {
                let lit: LitInt = meta.value()?.parse()?; // integer seconds
                cfg.timeout_secs = Some(lit.base10_parse()?); // bad int -> compile_error!
                Ok(())
            } else if meta.path.is_ident("cache") {
                let lit: LitBool = meta.value()?.parse()?; // true / false
                cfg.cache = Some(lit.value);
                Ok(())
            } else if meta.path.is_ident("cpu") {
                // cpu = <cores> — CPU CORES as a float (e.g. `2.0`) or an int (e.g.
                // `2`). Mirrors Modal's `cpu` kwarg: milli_cpu = int(1000 * cpu)
                // (truncation). Resolved to milli-cores HERE so `FunctionConfig`
                // carries a plain `Option<u32>` const-valid in the `static`
                // `inventory::submit!` initializer (like `timeout`).
                cfg.milli_cpu = Some(parse_cpu_to_milli(meta.value()?)?);
                Ok(())
            } else if meta.path.is_ident("memory") {
                // memory = <MiB> — requested memory in MEBIBYTES (an int), mirroring
                // Modal's `memory` kwarg (memory_mb = memory). Narrowed to u32 at emit.
                let lit: LitInt = meta.value()?.parse()?;
                cfg.memory_mb = Some(lit.base10_parse()?); // bad int -> compile_error!
                Ok(())
            } else if meta.path.is_ident("retries") {
                // retries = <count>  OR  retries = Retries(max_retries = N, ..)
                //
                // Two forms (peek the value): a bare INT literal keeps the current
                // fixed-interval shortcut (`retries: Some(u32)`, mirroring Modal's
                // bare-int `retries`); a `Retries(..)` CALL is the STRUCT form (custom
                // backoff/delays), canonicalized to a const SPEC string the facade hands
                // to the SDK's `parse_retries_spec` (same trick as `schedule`, keeping it
                // const-valid in the `static`). The two are mutually exclusive.
                let value = meta.value()?;
                if value.peek(LitInt) {
                    let lit: LitInt = value.parse()?;
                    cfg.retries = Some(lit.base10_parse()?); // bad int -> compile_error!
                } else {
                    cfg.retries_spec = Some(parse_retries_to_spec(value)?);
                }
                Ok(())
            } else if meta.path.is_ident("schedule") {
                // schedule = Cron("expr"[, "tz"])  OR  Period(days = 1, hours = 4, ..)
                // A run cadence for a DEPLOYED function (Modal `Cron`/`Period`,
                // schedule.py). Parsed into a const SPEC string the facade hands to the
                // SDK's `parse_schedule`, so `FunctionConfig.schedule` stays an
                // `Option<&'static str>` const-valid in the `inventory::submit!`
                // initializer (exactly like `gpu`).
                cfg.schedule = Some(parse_schedule_to_spec(meta.value()?)?);
                Ok(())
            } else if meta.path.is_ident("min_containers") {
                // min_containers = <N> — autoscaler floor (minimum warm containers,
                // mirroring Modal's `min_containers`, pka `keep_warm`). A plain
                // `Option<u32>` const-valid in the `static` initializer (like timeout).
                let lit: LitInt = meta.value()?.parse()?;
                cfg.min_containers = Some(lit.base10_parse()?); // bad int -> compile_error!
                Ok(())
            } else if meta.path.is_ident("max_containers") {
                // max_containers = <N> — autoscaler ceiling (max concurrent containers,
                // mirroring Modal's `max_containers`, pka `concurrency_limit`).
                let lit: LitInt = meta.value()?.parse()?;
                cfg.max_containers = Some(lit.base10_parse()?); // bad int -> compile_error!
                Ok(())
            } else if meta.path.is_ident("buffer_containers") {
                // buffer_containers = <N> — extra warm containers kept beyond demand,
                // mirroring Modal's `buffer_containers`.
                let lit: LitInt = meta.value()?.parse()?;
                cfg.buffer_containers = Some(lit.base10_parse()?); // bad int -> compile_error!
                Ok(())
            } else if meta.path.is_ident("scaledown_window") {
                // scaledown_window = <secs> — idle seconds before scaledown, mirroring
                // Modal's `scaledown_window` (pka `container_idle_timeout`).
                let lit: LitInt = meta.value()?.parse()?;
                cfg.scaledown_window = Some(lit.base10_parse()?); // bad int -> compile_error!
                Ok(())
            } else if meta.path.is_ident("secrets") {
                // secrets = ["my-secret", "other"] — a bracketed list of string
                // literals. Each is a Modal secret deployment-name the facade
                // resolves to a secret_id.
                let list = cfg.secrets.get_or_insert_with(Vec::new);
                for s in parse_str_list(meta.value()?)? {
                    list.push(s.value());
                }
                Ok(())
            } else if meta.path.is_ident("required_keys") {
                // required_keys = ["API_KEY", "DB_URL"] — a bracketed list of string
                // literals the facade asserts exist on the named `secrets = [..]` (one
                // flat list applied to all named secrets in v0). Mirrors Modal's
                // `Secret.from_name(.., required_keys=[..])`.
                let list = cfg.required_keys.get_or_insert_with(Vec::new);
                for s in parse_str_list(meta.value()?)? {
                    list.push(s.value());
                }
                Ok(())
            } else if meta.path.is_ident("env") {
                // env = {"API_TOKEN" = "dev", "REGION" = "us"} — an INLINE secret as a
                // brace-delimited map of `LitStr = LitStr` pairs, mirroring Modal's
                // `app.function(env={..})` → `Secret.from_dict(env)`. The facade derives
                // a deterministic per-entrypoint secret deployment name and resolves it
                // via `secret_from_dict` (CREATE_IF_MISSING), pushing the id into the
                // SAME secret_ids list named secrets use (so `env` + `secrets` compose).
                let list = cfg.env.get_or_insert_with(Vec::new);
                for (k, v) in parse_str_map(meta.value()?)? {
                    list.push((k.value(), v.value()));
                }
                Ok(())
            } else if meta.path.is_ident("volumes") {
                // volumes = ["/data=my-vol", ..] — a bracketed list of "MOUNT=NAME"
                // string literals. Split on the FIRST '=' into (mount_path, name).
                // Map syntax is hard to parse in attribute position, so the simplest
                // parseable form is a string list. Validated ONCE here for every
                // attribute (the validation used to be copy-pasted into `#[cls]`).
                let list = cfg.volumes.get_or_insert_with(Vec::new);
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
                    list.push((mount.to_string(), name.to_string()));
                }
                Ok(())
            } else if meta.path.is_ident("image") {
                // image = Image(base = "...", install_rust = <bool>, apt = ["..."],
                // pip = ["..."], run = ["..."]) — a PER-FUNCTION image declaration,
                // call-shaped like `Retries(..)`/`Cron(..)`. Lets a function declare its
                // OWN base image + the existing apt/pip/run `ImageStep` vocabulary
                // (PARITY.md §4 image=Partial), so e.g. a GPU function declares its CUDA
                // base in the decorator instead of `MODAL_RUST_BASE_IMAGE`. Canonicalized
                // to a const SPEC string the facade parses (`remote::parse_image_spec`),
                // so `FunctionConfig.image` stays an `Option<&'static str>` const-valid in
                // the `static` `inventory::submit!` initializer (like `schedule`/`gpu`).
                cfg.image = Some(parse_image_to_spec(meta.value()?)?);
                Ok(())
            } else if meta.path.is_ident("method") {
                // method = "GET"|"POST"|"PUT"|"DELETE"|"PATCH" — the REQUIRED HTTP verb
                // of a `#[modal_rust::endpoint]` (explicit, no silent default). Validated
                // HERE so a typo is a compile error with the expected syntax, never a
                // live-deploy surprise. On a plain `#[function]` (or a `#[cls]`/`#[method]`)
                // it is rejected with a pointer at `#[endpoint]` (pointed diagnostic, like
                // `enable_memory_snapshot` below).
                if kind != HandlerKind::Endpoint {
                    return Err(meta.error(
                        "`method` is `#[endpoint]`-only: it sets the HTTP verb of a web \
                         endpoint. To expose this handler over HTTP, decorate it \
                         `#[modal_rust::endpoint(method = \"POST\", ..)]` instead.",
                    ));
                }
                let lit: LitStr = meta.value()?.parse()?;
                let value = lit.value();
                if !ENDPOINT_METHODS.contains(&value.as_str()) {
                    return Err(syn::Error::new_spanned(
                        &lit,
                        format!(
                            "invalid endpoint method {value:?}; expected one of \
                             \"GET\", \"POST\", \"PUT\", \"DELETE\", \"PATCH\" \
                             (uppercase): {ENDPOINT_EXPECTED_SYNTAX}"
                        ),
                    ));
                }
                cfg.webhook_method = Some(lit);
                Ok(())
            } else if meta.path.is_ident("requires_proxy_auth") {
                // requires_proxy_auth = <bool> — Modal proxy-auth opt-in for an
                // `#[endpoint]` (the `Modal-Key`/`Modal-Secret` header pair). Default
                // unset ⇒ `false` = PUBLIC (matches Modal). Everything else rejects it
                // with a pointer at `#[endpoint]`.
                if kind != HandlerKind::Endpoint {
                    return Err(meta.error(
                        "`requires_proxy_auth` is `#[endpoint]`-only: it gates the web \
                         endpoint behind Modal proxy-auth. Use \
                         `#[modal_rust::endpoint(method = \"..\", requires_proxy_auth = \
                         true)]` instead.",
                    ));
                }
                let lit: LitBool = meta.value()?.parse()?;
                cfg.webhook_requires_proxy_auth = lit.value;
                Ok(())
            } else if meta.path.is_ident("port") {
                // port = <u16> — the REQUIRED TCP port a `#[web_server]` handler binds
                // (explicit, no silent default). `#[web_server]`-only: on any other
                // attribute, reject with a pointer at `#[web_server]` (pointed
                // diagnostic, like `method`/`enable_memory_snapshot`).
                if kind != HandlerKind::WebServer {
                    return Err(meta.error(
                        "`port` is `#[web_server]`-only: it sets the TCP port a long-running \
                         HTTP server binds. To launch a web server, decorate this handler \
                         `#[modal_rust::web_server(port = 3000)]` instead.",
                    ));
                }
                let lit: LitInt = meta.value()?.parse()?;
                cfg.web_server_port = Some(lit.base10_parse()?); // bad/overflow u16 -> compile_error!
                Ok(())
            } else if meta.path.is_ident("startup_timeout") {
                // startup_timeout = <secs> — OPTIONAL seconds Modal waits for the
                // `#[web_server]` port to come up. `#[web_server]`-only.
                if kind != HandlerKind::WebServer {
                    return Err(meta.error(
                        "`startup_timeout` is `#[web_server]`-only: it sets how long Modal \
                         waits for the server port. Use \
                         `#[modal_rust::web_server(port = .., startup_timeout = 30)]` instead.",
                    ));
                }
                let lit: LitInt = meta.value()?.parse()?;
                cfg.web_server_startup_timeout = Some(lit.base10_parse()?); // bad int -> compile_error!
                Ok(())
            } else if meta.path.is_ident("enable_memory_snapshot") {
                // enable_memory_snapshot = <bool> — a bare bool opting a `#[cls]` into
                // Modal memory snapshot (same shape/precedent as `cache`). When true, the
                // class's `#[enter]` load is captured into the deploy-time snapshot via
                // the serve loop's `prime` frame (deploy-only effect). Default unset ⇒
                // inert. `#[cls]`-only in v0: a free `#[function]` has no `#[enter]` to
                // capture, so reject with a pointed diagnostic rather than the generic
                // "unsupported argument".
                if kind != HandlerKind::Cls {
                    return Err(meta.error(
                        "memory snapshot is `#[cls]`-only in v0; it captures the `#[enter]` \
                         load. Move this handler into a `#[cls]` and set \
                         `#[cls(enable_memory_snapshot = true)]`.",
                    ));
                }
                let lit: LitBool = meta.value()?.parse()?;
                cfg.enable_memory_snapshot = Some(lit.value);
                Ok(())
            } else {
                Err(meta.error(unsupported_argument_message(kind)))
            }
        });
        syn::parse::Parser::parse2(parser, tokens)?;
    }

    // An endpoint's HTTP verb is REQUIRED — no silent default. (A plain `#[function]`
    // never sets it; the `method =` branch above is endpoint-gated.)
    if kind == HandlerKind::Endpoint && cfg.webhook_method.is_none() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            format!(
                "{} requires `method = ...` (the HTTP verb; no silent default): \
                 {ENDPOINT_EXPECTED_SYNTAX}",
                kind.display(),
            ),
        ));
    }

    // A web server's bound PORT is REQUIRED — no silent default. (Only the `port =`
    // branch above, web-server-gated, ever sets it.)
    if kind == HandlerKind::WebServer && cfg.web_server_port.is_none() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            format!(
                "{} requires `port = ...` (the TCP port the server binds; no silent \
                 default): {WEB_SERVER_EXPECTED_SYNTAX}",
                kind.display(),
            ),
        ));
    }

    Ok(cfg)
}

/// The unrecognized-argument diagnostic: names the attribute that is actually
/// expanding and lists ONLY the keys its allow-set accepts.
fn unsupported_argument_message(kind: HandlerKind) -> String {
    if kind == HandlerKind::Cls {
        return "unsupported `#[cls]`/`#[method]` argument; recognized: `gpu`, \
                `timeout`, `cache`, `cpu`, `memory`, `retries`, `schedule`, \
                `min_containers`, `max_containers`, `buffer_containers`, \
                `scaledown_window`, `secrets`, `required_keys`, `env`, `volumes`, \
                `image`, `enable_memory_snapshot`"
            .to_string();
    }
    // An `#[endpoint]` / `#[web_server]` additionally lists its extra keys.
    let endpoint_extras = match kind {
        HandlerKind::Endpoint => {
            "`method = \"GET\"|\"POST\"|\"PUT\"|\"DELETE\"|\"PATCH\"` \
             (REQUIRED), `requires_proxy_auth = <bool>`, "
        }
        HandlerKind::WebServer => "`port = <u16>` (REQUIRED), `startup_timeout = <secs>`, ",
        _ => "",
    };
    format!(
        "unsupported `{}` argument; recognized: {}\
         `name = \"...\"`, `gpu = \"...\"`, `timeout = <int secs>`, \
         `cache = <bool>`, `cpu = <cores>`, `memory = <MiB>`, \
         `retries = <count>` or `retries = Retries(max_retries = N, ..)`, \
         `schedule = Cron(\"..\")/Period(..)`, \
         `min_containers = <N>`, `max_containers = <N>`, \
         `buffer_containers = <N>`, `scaledown_window = <secs>`, \
         `secrets = [\"name\", ..]`, `required_keys = [\"KEY\", ..]`, \
         `env = {{\"K\" = \"V\", ..}}`, `volumes = [\"/mount=name\", ..]`, \
         `image = Image(base = \"..\", apt = [..], pip = [..], run = [..])`",
        kind.display(),
        endpoint_extras,
    )
}

/// Build the `#facade::FunctionConfig { .. }` token stream for a parsed (and, for
/// `#[cls]`, merge-resolved) [`DecoratorConfig`] — the ONE emitter every attribute
/// goes through, so a knob threaded here rides EVERY decorator's registration (M2).
///
/// The decorator config flows into the facade registration as a `FunctionConfig`,
/// const-valid in the `static` `inventory::submit!` initializer throughout:
/// `gpu`/`webhook_method` stay `&'static str` literals (like `name`);
/// `retries_spec`/`schedule`/`image` are canonicalized SPEC strings;
/// `timeout` is narrowed `u64 -> u32` here; the autoscaler knobs are plain
/// `Option<u32>`. Unset list fields emit `&[]` and an unset
/// `enable_memory_snapshot` emits the inert `false` — both byte-identical to the
/// bare default. The bare form emits `FunctionConfig::default()` (all `None`),
/// which runtime dispatch ignores (so behavior is byte-identical; only the facade
/// reads `config` for control-plane work).
pub(crate) fn function_config_tokens(
    cfg: &DecoratorConfig,
    facade: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let opt_u32 = |v: Option<u32>| match v {
        Some(n) => quote::quote! { ::core::option::Option::Some(#n) },
        None => quote::quote! { ::core::option::Option::None },
    };
    // `&'static str` literal fields (LitStr keeps the user's span/escaping).
    let opt_litstr = |v: &Option<LitStr>| match v {
        Some(s) => quote::quote! { ::core::option::Option::Some(#s) },
        None => quote::quote! { ::core::option::Option::None },
    };
    // The canonicalized SPEC strings (`retries_spec`/`schedule`/`image`) emit as
    // plain string literals, exactly like `gpu`.
    let opt_str = |v: &Option<String>| match v {
        Some(s) => quote::quote! { ::core::option::Option::Some(#s) },
        None => quote::quote! { ::core::option::Option::None },
    };
    // The `&'static` slice fields: unset (`None`) and explicitly-empty both emit
    // `&[]`, byte-identical to the bare default.
    let str_list = |v: &Option<Vec<String>>| {
        let items = v.as_deref().unwrap_or_default().iter();
        quote::quote! { &[ #( #items ),* ] }
    };
    let pair_list = |v: &Option<Vec<(String, String)>>| {
        let items = v
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|(a, b)| quote::quote! { (#a, #b) });
        quote::quote! { &[ #( #items ),* ] }
    };

    let gpu_tok = opt_litstr(&cfg.gpu);
    let timeout_tok = match cfg.timeout_secs {
        Some(n) => {
            let n = n as u32;
            quote::quote! { ::core::option::Option::Some(#n) }
        }
        None => quote::quote! { ::core::option::Option::None },
    };
    let cache_tok = match cfg.cache {
        Some(b) => quote::quote! { ::core::option::Option::Some(#b) },
        None => quote::quote! { ::core::option::Option::None },
    };
    let milli_cpu_tok = opt_u32(cfg.milli_cpu);
    let memory_mb_tok = opt_u32(cfg.memory_mb);
    let retries_tok = opt_u32(cfg.retries);
    let retries_spec_tok = opt_str(&cfg.retries_spec);
    let schedule_tok = opt_str(&cfg.schedule);
    let min_containers_tok = opt_u32(cfg.min_containers);
    let max_containers_tok = opt_u32(cfg.max_containers);
    let buffer_containers_tok = opt_u32(cfg.buffer_containers);
    let scaledown_window_tok = opt_u32(cfg.scaledown_window);
    let secrets_tok = str_list(&cfg.secrets);
    let required_keys_tok = str_list(&cfg.required_keys);
    let env_tok = pair_list(&cfg.env);
    let volumes_tok = pair_list(&cfg.volumes);
    let image_tok = opt_str(&cfg.image);
    // Unset ⇒ `false` (the inert default, byte-identical wire — `#[function]`/
    // `#[endpoint]` can never set it, the parser rejects the key there). A plain
    // `bool` literal, const-valid in the `static` initializer; `true` rides into
    // the deploy-time `FunctionCreate`.
    let snapshot_on = cfg.enable_memory_snapshot.unwrap_or(false);
    // Web-endpoint marker: only `#[endpoint]` can set these (the parser gates the
    // keys), so everything else keeps the inert `None`/`false` ⇒ byte-identical wire.
    let webhook_method_tok = opt_litstr(&cfg.webhook_method);
    let webhook_requires_proxy_auth = cfg.webhook_requires_proxy_auth;
    // Web-server marker: only `#[web_server]` can set these (the parser gates the keys),
    // so everything else keeps the inert `None` ⇒ byte-identical wire.
    let web_server_port_tok = match cfg.web_server_port {
        Some(p) => quote::quote! { ::core::option::Option::Some(#p) },
        None => quote::quote! { ::core::option::Option::None },
    };
    let web_server_startup_timeout_tok = opt_u32(cfg.web_server_startup_timeout);

    quote::quote! {
        #facade::FunctionConfig {
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
            enable_memory_snapshot: #snapshot_on,
            webhook_method: #webhook_method_tok,
            webhook_requires_proxy_auth: #webhook_requires_proxy_auth,
            web_server_port: #web_server_port_tok,
            web_server_startup_timeout: #web_server_startup_timeout_tok,
        }
    }
}
