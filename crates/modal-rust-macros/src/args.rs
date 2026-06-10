//! The SHARED `#[function]` / `#[endpoint]` decorator grammar: [`HandlerKind`],
//! [`FunctionArgs`], and the one parser ([`parse_function_args`]) both attributes
//! go through. Split out of `lib.rs` mechanically (M1).

use syn::{LitBool, LitInt, LitStr};

use crate::specs::{
    parse_cpu_to_milli, parse_image_to_spec, parse_retries_to_spec, parse_schedule_to_spec,
    parse_str_list, parse_str_map,
};

/// Which user-facing attribute is expanding through the SHARED `#[function]`
/// parse+emit path: `#[function]` (the plain handler) or `#[endpoint]` (the same
/// handler + the web-endpoint marker). ONE parser/emitter serves both (web-endpoints
/// spec §5 — no forked grammar); the kind only gates the endpoint-only keys
/// (`method`/`requires_proxy_auth`) and the attribute name in diagnostics.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HandlerKind {
    Function,
    Endpoint,
}

impl HandlerKind {
    /// The attribute name as the user spells it, for diagnostics.
    pub(crate) fn display(self) -> &'static str {
        match self {
            HandlerKind::Function => "#[modal_rust::function]",
            HandlerKind::Endpoint => "#[modal_rust::endpoint]",
        }
    }
}

/// The HTTP verbs `#[endpoint(method = ..)]` accepts (uppercase, validated at
/// expansion time so a typo is a compile error, never a live-deploy surprise).
pub(crate) const ENDPOINT_METHODS: &[&str] = &["GET", "POST", "PUT", "DELETE", "PATCH"];

/// The expected `#[endpoint]` syntax, embedded in the missing/invalid-`method`
/// compile errors so the fix is copy-pasteable from the diagnostic.
pub(crate) const ENDPOINT_EXPECTED_SYNTAX: &str = "#[modal_rust::endpoint(method = \"GET\"|\"POST\"|\"PUT\"|\"DELETE\"|\"PATCH\", <any #[function] config>)]";

/// Every argument the SHARED `#[function]`/`#[endpoint]` decorator grammar parses, in
/// ONE record so the parse ([`parse_function_args`]) and emit ([`build_registration`])
/// paths have a single seam. The `webhook_*` fields are the `#[endpoint]`-only extras
/// (always `None`/`false` for a plain `#[function]` ⇒ inert, byte-identical wire).
#[derive(Default)]
pub(crate) struct FunctionArgs {
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
    pub(crate) secrets: Vec<String>,
    pub(crate) required_keys: Vec<String>,
    pub(crate) env: Vec<(String, String)>,
    pub(crate) volumes: Vec<(String, String)>,
    pub(crate) image: Option<String>,
    /// `method = "POST"` — the validated HTTP verb (`#[endpoint]`-only; REQUIRED there).
    pub(crate) webhook_method: Option<LitStr>,
    /// `requires_proxy_auth = true` — Modal proxy-auth opt-in (`#[endpoint]`-only;
    /// default `false` = PUBLIC, matching Modal).
    pub(crate) webhook_requires_proxy_auth: bool,
}

/// Parse a `#[function(..)]` / `#[endpoint(..)]` decorator argument list — the ONE
/// shared grammar (web-endpoints spec §5). All arguments are optional for a
/// `#[function]`; an `#[endpoint]` additionally accepts `requires_proxy_auth = <bool>`
/// and REQUIRES `method = "GET"|"POST"|"PUT"|"DELETE"|"PATCH"`. The bare
/// `#[modal_rust::function]` (and `name = "..."`) set none of gpu/timeout/cache, so
/// the emitted facade `FunctionConfig` is `default()` (all `None`) —
/// runtime-observable behavior stays byte-identical.
pub(crate) fn parse_function_args(
    tokens: proc_macro2::TokenStream,
    kind: HandlerKind,
) -> syn::Result<FunctionArgs> {
    let mut explicit_name: Option<LitStr> = None;
    let mut gpu: Option<LitStr> = None; // gpu = "T4"
    let mut timeout_secs: Option<u64> = None; // timeout = 1800   (LitInt -> u64, narrow at emit)
    let mut cache: Option<bool> = None; // cache = false
    let mut milli_cpu: Option<u32> = None; // cpu = 2.0 (cores) -> milli_cpu = 2000
    let mut memory_mb: Option<u32> = None; // memory = 4096 (MiB)
    let mut retries: Option<u32> = None; // retries = 3 (retry count)
    let mut retries_spec: Option<String> = None; // retries = Retries(..) -> spec string
    let mut schedule: Option<String> = None; // schedule = Cron("..") / Period(..) -> spec string
    let mut min_containers: Option<u32> = None; // min_containers = 1 (autoscaler floor)
    let mut max_containers: Option<u32> = None; // max_containers = 5 (autoscaler ceiling)
    let mut buffer_containers: Option<u32> = None; // buffer_containers = 2 (warm buffer)
    let mut scaledown_window: Option<u32> = None; // scaledown_window = 120 (idle secs)
    let mut secrets: Vec<String> = Vec::new(); // secrets = ["a", "b"]
    let mut required_keys: Vec<String> = Vec::new(); // required_keys = ["API_KEY", ..]
    let mut env: Vec<(String, String)> = Vec::new(); // env = {"K" = "V", ..} -> inline secret
    let mut volumes: Vec<(String, String)> = Vec::new(); // volumes = ["/data=vol"] -> (mount, name)
    let mut image: Option<String> = None; // image = Image(base=.., apt=[..], ..) -> spec string
    let mut webhook_method: Option<LitStr> = None; // method = "POST" (endpoint-only; REQUIRED there)
    let mut webhook_requires_proxy_auth: Option<bool> = None; // requires_proxy_auth = true (endpoint-only)
    if !tokens.is_empty() {
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
                    retries = Some(lit.base10_parse()?); // bad int -> compile_error!
                } else {
                    retries_spec = Some(parse_retries_to_spec(value)?);
                }
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
            } else if meta.path.is_ident("min_containers") {
                // min_containers = <N> — autoscaler floor (minimum warm containers,
                // mirroring Modal's `min_containers`, pka `keep_warm`). A plain
                // `Option<u32>` const-valid in the `static` initializer (like timeout).
                let lit: LitInt = meta.value()?.parse()?;
                min_containers = Some(lit.base10_parse()?); // bad int -> compile_error!
                Ok(())
            } else if meta.path.is_ident("max_containers") {
                // max_containers = <N> — autoscaler ceiling (max concurrent containers,
                // mirroring Modal's `max_containers`, pka `concurrency_limit`).
                let lit: LitInt = meta.value()?.parse()?;
                max_containers = Some(lit.base10_parse()?); // bad int -> compile_error!
                Ok(())
            } else if meta.path.is_ident("buffer_containers") {
                // buffer_containers = <N> — extra warm containers kept beyond demand,
                // mirroring Modal's `buffer_containers`.
                let lit: LitInt = meta.value()?.parse()?;
                buffer_containers = Some(lit.base10_parse()?); // bad int -> compile_error!
                Ok(())
            } else if meta.path.is_ident("scaledown_window") {
                // scaledown_window = <secs> — idle seconds before scaledown, mirroring
                // Modal's `scaledown_window` (pka `container_idle_timeout`).
                let lit: LitInt = meta.value()?.parse()?;
                scaledown_window = Some(lit.base10_parse()?); // bad int -> compile_error!
                Ok(())
            } else if meta.path.is_ident("secrets") {
                // secrets = ["my-secret", "other"] — a bracketed list of string
                // literals. Each is a Modal secret deployment-name the facade
                // resolves to a secret_id.
                for s in parse_str_list(meta.value()?)? {
                    secrets.push(s.value());
                }
                Ok(())
            } else if meta.path.is_ident("required_keys") {
                // required_keys = ["API_KEY", "DB_URL"] — a bracketed list of string
                // literals the facade asserts exist on the named `secrets = [..]` (one
                // flat list applied to all named secrets in v0). Mirrors Modal's
                // `Secret.from_name(.., required_keys=[..])`.
                for s in parse_str_list(meta.value()?)? {
                    required_keys.push(s.value());
                }
                Ok(())
            } else if meta.path.is_ident("env") {
                // env = {"API_TOKEN" = "dev", "REGION" = "us"} — an INLINE secret as a
                // brace-delimited map of `LitStr = LitStr` pairs, mirroring Modal's
                // `app.function(env={..})` → `Secret.from_dict(env)`. The facade derives
                // a deterministic per-entrypoint secret deployment name and resolves it
                // via `secret_from_dict` (CREATE_IF_MISSING), pushing the id into the
                // SAME secret_ids list named secrets use (so `env` + `secrets` compose).
                for (k, v) in parse_str_map(meta.value()?)? {
                    env.push((k.value(), v.value()));
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
                image = Some(parse_image_to_spec(meta.value()?)?);
                Ok(())
            } else if meta.path.is_ident("method") {
                // method = "GET"|"POST"|"PUT"|"DELETE"|"PATCH" — the REQUIRED HTTP verb
                // of a `#[modal_rust::endpoint]` (explicit, no silent default). Validated
                // HERE so a typo is a compile error with the expected syntax, never a
                // live-deploy surprise. On a plain `#[function]` it is rejected with a
                // pointer at `#[endpoint]` (pointed diagnostic, like
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
                webhook_method = Some(lit);
                Ok(())
            } else if meta.path.is_ident("requires_proxy_auth") {
                // requires_proxy_auth = <bool> — Modal proxy-auth opt-in for an
                // `#[endpoint]` (the `Modal-Key`/`Modal-Secret` header pair). Default
                // unset ⇒ `false` = PUBLIC (matches Modal). `#[function]` rejects it
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
                webhook_requires_proxy_auth = Some(lit.value);
                Ok(())
            } else if meta.path.is_ident("enable_memory_snapshot") {
                // Memory snapshot is `#[cls]`-only in v0: it captures the `#[enter]`
                // load into the deploy-time snapshot, and a free `#[function]` has no
                // `#[enter]` to capture. Reject with a pointed diagnostic rather than the
                // generic "unsupported argument" so the user knows to use `#[cls]`.
                Err(meta.error(
                    "memory snapshot is `#[cls]`-only in v0; it captures the `#[enter]` \
                     load. Move this handler into a `#[cls]` and set \
                     `#[cls(enable_memory_snapshot = true)]`.",
                ))
            } else {
                // The recognized list names the attribute that is actually expanding,
                // and an `#[endpoint]` additionally lists its two extra keys.
                let endpoint_extras = match kind {
                    HandlerKind::Endpoint => {
                        "`method = \"GET\"|\"POST\"|\"PUT\"|\"DELETE\"|\"PATCH\"` \
                         (REQUIRED), `requires_proxy_auth = <bool>`, "
                    }
                    HandlerKind::Function => "",
                };
                Err(meta.error(format!(
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
                )))
            }
        });
        syn::parse::Parser::parse2(parser, tokens)?;
    }

    // An endpoint's HTTP verb is REQUIRED — no silent default. (A plain `#[function]`
    // never sets it; the `method =` branch above is endpoint-gated.)
    if kind == HandlerKind::Endpoint && webhook_method.is_none() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            format!(
                "{} requires `method = ...` (the HTTP verb; no silent default): \
                 {ENDPOINT_EXPECTED_SYNTAX}",
                kind.display(),
            ),
        ));
    }

    Ok(FunctionArgs {
        explicit_name,
        gpu,
        timeout_secs,
        cache,
        milli_cpu,
        memory_mb,
        retries,
        retries_spec,
        schedule,
        min_containers,
        max_containers,
        buffer_containers,
        scaledown_window,
        secrets,
        required_keys,
        env,
        volumes,
        image,
        webhook_method,
        webhook_requires_proxy_auth: webhook_requires_proxy_auth.unwrap_or(false),
    })
}
