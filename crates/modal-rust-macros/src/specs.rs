//! The decorator-value canonicalizers: `cpu`/list/map literal parsers and the
//! `retries:..` / `img:..` / `cron:..`|`period:..` SPEC-string emitters (the
//! const `&'static str` forms `inventory::submit!` can carry). Split out of
//! `lib.rs` mechanically (M1).

use syn::punctuated::Punctuated;
use syn::{Expr, ExprLit, Lit, LitStr, Token};

/// Parse a `cpu = <cores>` value into milli-cores, mirroring Modal's
/// `milli_cpu = int(1000 * cpu)` (truncation toward zero). Accepts a FLOAT literal
/// (`2.0`, `0.5`) or an INT literal (`2` ⇒ `2.0` cores). Resolving HERE keeps
/// [`FunctionConfig::milli_cpu`] a plain const `Option<u32>` for the `static`
/// `inventory::submit!` initializer. A negative value is rejected (cores cannot be
/// negative).
pub(crate) fn parse_cpu_to_milli(input: syn::parse::ParseStream) -> syn::Result<u32> {
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
pub(crate) fn parse_str_list(input: syn::parse::ParseStream) -> syn::Result<Vec<LitStr>> {
    let content;
    syn::bracketed!(content in input);
    let items: Punctuated<LitStr, Token![,]> = Punctuated::parse_terminated(&content)?;
    Ok(items.into_iter().collect())
}

/// Parse a brace-delimited map of `LitStr = LitStr` pairs from a `meta.value()` parse
/// stream: `{"K" = "V", "K2" = "V2"}`. Used by `env = {..}` (the inline secret). Map
/// syntax IS parseable in the meta parser as a braced group of comma-separated
/// `name = value` string-literal assignments. Returns the `(key, value)` [`LitStr`]
/// pairs (keeping spans for diagnostics). An empty map `{}` is allowed (yields no
/// pairs). A duplicate key is rejected (it would silently clobber an env var).
pub(crate) fn parse_str_map(input: syn::parse::ParseStream) -> syn::Result<Vec<(LitStr, LitStr)>> {
    let content;
    syn::braced!(content in input);
    let mut pairs: Vec<(LitStr, LitStr)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    while !content.is_empty() {
        let key: LitStr = content.parse()?;
        content.parse::<Token![=]>()?;
        let value: LitStr = content.parse()?;
        if !seen.insert(key.value()) {
            return Err(syn::Error::new_spanned(
                &key,
                format!("duplicate `env` key {:?}", key.value()),
            ));
        }
        pairs.push((key, value));
        // Allow a trailing comma; stop at the end of the braced group otherwise.
        if content.is_empty() {
            break;
        }
        content.parse::<Token![,]>()?;
    }
    Ok(pairs)
}

/// Parse a `retries = Retries(..)` STRUCT-form value into a canonical SPEC string the
/// SDK's `parse_retries_spec` understands. Call-shaped, exactly like `Cron(..)` /
/// `Period(..)`, mirroring Modal's `Retries(max_retries, backoff_coefficient,
/// initial_delay, max_delay)` (`retries.py`):
///
/// - `max_retries` (REQUIRED, int) → `max=<N>` (the retry count).
/// - `backoff_coefficient` (optional, float; default `1.0`) → `backoff=<f>`.
/// - `initial_delay` (optional, SECONDS, int or float; default `1.0`) → `initial_ms=<ms>`.
/// - `max_delay` (optional, SECONDS, int or float; default `60.0`) → `max_ms=<ms>`.
///
/// Seconds are converted to integer milliseconds HERE (Modal stores
/// `initial_delay_ms`/`max_delay_ms`), so the spec stays a flat `&'static str`. Only
/// the components present are emitted (the SDK fills the rest with Modal's defaults).
/// A malformed form becomes a `compile_error!` so the user learns at compile time.
pub(crate) fn parse_retries_to_spec(input: syn::parse::ParseStream) -> syn::Result<String> {
    let call: syn::ExprCall = input.parse().map_err(|_| {
        syn::Error::new(
            input.span(),
            "`retries` must be a bare integer (`retries = 5`) or the struct form \
             `retries = Retries(max_retries = N[, backoff_coefficient = f] \
             [, initial_delay = s][, max_delay = s])`",
        )
    })?;
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
                "`retries` struct form must call `Retries(..)`",
            ))
        }
    };
    if kind != "Retries" {
        return Err(syn::Error::new_spanned(
            &call.func,
            format!("unknown retries kind {kind:?}; expected the `Retries(..)` struct form"),
        ));
    }
    let mut max_retries: Option<String> = None;
    let mut parts: Vec<String> = Vec::new();
    for arg in &call.args {
        let Expr::Assign(assign) = arg else {
            return Err(syn::Error::new_spanned(
                arg,
                "Retries components must be `name = value`, e.g. `max_retries = 5`",
            ));
        };
        let Expr::Path(name_path) = assign.left.as_ref() else {
            return Err(syn::Error::new_spanned(
                &assign.left,
                "Retries component name must be a bare identifier (e.g. `max_retries`)",
            ));
        };
        let name = name_path
            .path
            .get_ident()
            .map(|i| i.to_string())
            .ok_or_else(|| {
                syn::Error::new_spanned(
                    &assign.left,
                    "Retries component name must be an identifier",
                )
            })?;
        match name.as_str() {
            "max_retries" => max_retries = Some(format!("max={}", expect_u32_lit(&assign.right)?)),
            "backoff_coefficient" => {
                parts.push(format!("backoff={}", expect_f64_lit(&assign.right)?))
            }
            "initial_delay" => parts.push(format!("initial_ms={}", secs_lit_to_ms(&assign.right)?)),
            "max_delay" => parts.push(format!("max_ms={}", secs_lit_to_ms(&assign.right)?)),
            other => {
                return Err(syn::Error::new_spanned(
                    &assign.left,
                    format!(
                        "unknown Retries component {other:?}; expected one of \
                         `max_retries`, `backoff_coefficient`, `initial_delay`, `max_delay`"
                    ),
                ))
            }
        }
    }
    let max_retries = max_retries.ok_or_else(|| {
        syn::Error::new_spanned(
            &call,
            "Retries requires `max_retries = N` (the retry count), e.g. \
             `Retries(max_retries = 5)`",
        )
    })?;
    // `max` first, then the optional components in the order written.
    let mut all = vec![max_retries];
    all.extend(parts);
    Ok(format!("retries:{}", all.join(",")))
}

/// Extract a non-negative integer (`u32`) from a numeric-literal call argument, or a
/// clear `compile_error!`. Used for `max_retries`.
pub(crate) fn expect_u32_lit(expr: &Expr) -> syn::Result<u32> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Int(i), ..
        }) => i.base10_parse(),
        other => Err(syn::Error::new_spanned(
            other,
            "expected a non-negative integer literal",
        )),
    }
}

/// Extract an `f64` from a numeric-literal call argument (accepts a float OR an int),
/// rendered verbatim for the spec. Used for `backoff_coefficient`.
pub(crate) fn expect_f64_lit(expr: &Expr) -> syn::Result<f64> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Float(f), ..
        }) => f.base10_parse(),
        Expr::Lit(ExprLit {
            lit: Lit::Int(i), ..
        }) => Ok(i.base10_parse::<u64>()? as f64),
        other => Err(syn::Error::new_spanned(
            other,
            "expected a number (float or int)",
        )),
    }
}

/// Convert a delay given in SECONDS (a float or int literal) to integer MILLISECONDS,
/// mirroring Modal storing `initial_delay_ms`/`max_delay_ms`. Truncates toward zero
/// (like Modal's `int(1000 * secs)`). Rejects negative / non-finite values.
pub(crate) fn secs_lit_to_ms(expr: &Expr) -> syn::Result<u32> {
    let secs: f64 = match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Float(f), ..
        }) => f.base10_parse()?,
        Expr::Lit(ExprLit {
            lit: Lit::Int(i), ..
        }) => i.base10_parse::<u64>()? as f64,
        other => {
            return Err(syn::Error::new_spanned(
                other,
                "delay must be a number of SECONDS (float or int), e.g. `0.5` or `30`",
            ))
        }
    };
    if secs < 0.0 || !secs.is_finite() {
        return Err(syn::Error::new_spanned(
            expr,
            "delay (seconds) must be a finite, non-negative number",
        ));
    }
    Ok((secs * 1000.0) as u32)
}

/// Parse an `image = Image(..)` value into a canonical SPEC string the facade's
/// `remote::parse_image_spec` understands. Call-shaped (like `Retries(..)`/`Cron(..)`),
/// mirroring a v0 slice of Modal's `Image` builder (PARITY.md §4 image=Partial). Lets a
/// function declare its OWN base image + the existing apt/pip/run `ImageStep` vocabulary:
///
/// - `base = "registry/tag"` (optional `LitStr`) → the base image tag (overrides the
///   env-only `MODAL_RUST_BASE_IMAGE`). Omit to keep the path default base.
/// - `install_rust = <bool>` (optional) → install the rustup toolchain + CUDA env into
///   the image (set for a non-Rust base, e.g. a `nvidia/cuda:*-devel` base).
/// - `apt = ["pkg", ..]` (optional) → an `ImageStep::Apt`.
/// - `pip = ["pkg", ..]` (optional) → an `ImageStep::Pip`.
/// - `run = ["cmd", ..]` (optional) → an `ImageStep::Run`.
///
/// Emitted as a compact JSON object (escaped here, so arbitrary `run` commands round-trip
/// safely) the facade deserializes once. A malformed form becomes a `compile_error!` so
/// the user learns at compile time, never on the wire. v0 scope: base + install_rust +
/// the existing apt/pip/run steps; a fully general `Image` builder type is a follow-up.
pub(crate) fn parse_image_to_spec(input: syn::parse::ParseStream) -> syn::Result<String> {
    let call: syn::ExprCall = input.parse().map_err(|_| {
        syn::Error::new(
            input.span(),
            "`image` must be the struct form `image = Image(base = \"..\"[, \
             install_rust = <bool>][, apt = [..]][, pip = [..]][, run = [..]])`",
        )
    })?;
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
                "`image` struct form must call `Image(..)`",
            ))
        }
    };
    if kind != "Image" {
        return Err(syn::Error::new_spanned(
            &call.func,
            format!("unknown image kind {kind:?}; expected the `Image(..)` struct form"),
        ));
    }
    let mut base: Option<String> = None;
    let mut install_rust: Option<bool> = None;
    let mut apt: Vec<String> = Vec::new();
    let mut pip: Vec<String> = Vec::new();
    let mut run: Vec<String> = Vec::new();
    for arg in &call.args {
        let Expr::Assign(assign) = arg else {
            return Err(syn::Error::new_spanned(
                arg,
                "Image components must be `name = value`, e.g. `base = \"rust:1-slim\"`",
            ));
        };
        let Expr::Path(name_path) = assign.left.as_ref() else {
            return Err(syn::Error::new_spanned(
                &assign.left,
                "Image component name must be a bare identifier (e.g. `base`, `apt`)",
            ));
        };
        let name = name_path
            .path
            .get_ident()
            .map(|i| i.to_string())
            .ok_or_else(|| {
                syn::Error::new_spanned(&assign.left, "Image component name must be an identifier")
            })?;
        match name.as_str() {
            "base" => {
                let Expr::Lit(ExprLit {
                    lit: Lit::Str(s), ..
                }) = assign.right.as_ref()
                else {
                    return Err(syn::Error::new_spanned(
                        &assign.right,
                        "`base` must be a string literal, e.g. `base = \"rust:1-slim\"`",
                    ));
                };
                base = Some(s.value());
            }
            "install_rust" => {
                let Expr::Lit(ExprLit {
                    lit: Lit::Bool(b), ..
                }) = assign.right.as_ref()
                else {
                    return Err(syn::Error::new_spanned(
                        &assign.right,
                        "`install_rust` must be a bool literal, e.g. `install_rust = true`",
                    ));
                };
                install_rust = Some(b.value);
            }
            "apt" => apt = image_str_list(&assign.right)?,
            "pip" => pip = image_str_list(&assign.right)?,
            "run" => run = image_str_list(&assign.right)?,
            other => {
                return Err(syn::Error::new_spanned(
                    &assign.left,
                    format!(
                        "unknown Image component {other:?}; expected one of \
                         `base`, `install_rust`, `apt`, `pip`, `run`"
                    ),
                ))
            }
        }
    }
    // Build a compact JSON object the facade deserializes. All fields optional; emit only
    // what was set (and never emit an empty step list) so the spec is minimal and a
    // bare `Image()` is the no-op default. JSON-escape every string so arbitrary `run`
    // commands round-trip without a delimiter collision.
    let mut parts: Vec<String> = Vec::new();
    if let Some(b) = base {
        parts.push(format!("\"base\":\"{}\"", json_escape(&b)));
    }
    if let Some(r) = install_rust {
        parts.push(format!("\"install_rust\":{r}"));
    }
    let json_arr = |items: &[String]| -> String {
        let inner = items
            .iter()
            .map(|s| format!("\"{}\"", json_escape(s)))
            .collect::<Vec<_>>()
            .join(",");
        format!("[{inner}]")
    };
    if !apt.is_empty() {
        parts.push(format!("\"apt\":{}", json_arr(&apt)));
    }
    if !pip.is_empty() {
        parts.push(format!("\"pip\":{}", json_arr(&pip)));
    }
    if !run.is_empty() {
        parts.push(format!("\"run\":{}", json_arr(&run)));
    }
    Ok(format!("{{{}}}", parts.join(",")))
}

/// Parse a bracketed list of string literals from an `Image(..)` component value
/// expression (`apt = ["a", "b"]`). Shared by `apt`/`pip`/`run`. Rejects a non-list /
/// non-string-literal element with a clear `compile_error!`.
pub(crate) fn image_str_list(expr: &Expr) -> syn::Result<Vec<String>> {
    let Expr::Array(arr) = expr else {
        return Err(syn::Error::new_spanned(
            expr,
            "expected a bracketed list of string literals, e.g. `[\"libpng-dev\"]`",
        ));
    };
    let mut out = Vec::with_capacity(arr.elems.len());
    for elem in &arr.elems {
        let Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        }) = elem
        else {
            return Err(syn::Error::new_spanned(
                elem,
                "image step entries must be string literals",
            ));
        };
        out.push(s.value());
    }
    Ok(out)
}

/// Minimal JSON string escaper for the compile-time `image` spec (the macro crate has no
/// serde dep). Escapes the seven characters JSON requires; control chars below 0x20 go to
/// `\u00XX`. Sufficient for image tags + apt/pip/run command strings.
pub(crate) fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// The seven `Period(..)` component names, in Modal's large→small order
/// (`schedule.py:90`). `seconds` is the only float; the rest are integers.
pub(crate) const PERIOD_COMPONENTS: &[&str] = &[
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
pub(crate) fn parse_schedule_to_spec(input: syn::parse::ParseStream) -> syn::Result<String> {
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
pub(crate) fn expect_str_lit(
    arg: Option<&Expr>,
    call: &syn::ExprCall,
    msg: &str,
) -> syn::Result<String> {
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
pub(crate) fn parse_period_components(call: &syn::ExprCall) -> syn::Result<String> {
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
        let value = match assign.right.as_ref() {
            Expr::Lit(ExprLit {
                lit: Lit::Int(i), ..
            }) => i.base10_digits().to_string(),
            Expr::Lit(ExprLit {
                lit: Lit::Float(f), ..
            }) if name == "seconds" => f.base10_digits().to_string(),
            Expr::Lit(ExprLit {
                lit: Lit::Float(_), ..
            }) => {
                return Err(syn::Error::new_spanned(
                    &assign.right,
                    format!(
                        "Period component {name:?} must be an integer (only `seconds` is a float)"
                    ),
                ))
            }
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
