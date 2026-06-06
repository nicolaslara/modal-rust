# P4 Build Spec — the decorator IS the config

`#[modal_rust::function(gpu="T4", timeout=1800, cache=false)]` flows, at runtime, into the
`FunctionCreate` request (`Resources.gpu_config`, `timeout_secs`) when the facade creates the
function. No static pre-parse, no CLI flag — the Rust inventory registry is the source of truth.

This is the merged, build-ready spec. It supersedes the two design notes. Where they disagreed,
this resolves to **what Modal Python actually does** (`parse_gpu_config`) and to the **actual
current code shape** (verified file:line below).

---

## 0. Ground truth (verified, file:line)

- **Proto** (`crates/modal-rust-sdk/proto/api.proto`):
  - `Resources` = msg @2987: `memory_mb=2`, `milli_cpu=3`, **`GPUConfig gpu_config=4`**, …
  - `GPUConfig` @2327: `GPUType type=1 [deprecated]`, `uint32 count=2`, **`string gpu_type=4`**.
  - `Function.resources` = field 9 (always-set today — fix #1, function.rs:205); `Function.timeout_secs`
    = field 21 (function.rs:206, already wired via `spec.timeout_secs`).
  - `FunctionData.ranked_functions` (field 18) — GPU-LIST stretch, OUT OF SCOPE.
- **Generated Rust binding** (`target/.../out/modal.client.rs:3394`): prost camelCases the message →
  `pub struct GpuConfig { r#type: i32, count: u32, gpu_type: String }`, `derive(Default, Clone, PartialEq)`.
- **In-crate proto path is `crate::proto::api`** — function.rs:18-22 imports `crate::proto::api::{Resources, …}`.
  (The crate ALSO re-exports `pub use proto::modal;` at lib.rs:56 as the public path, but inside the SDK use
  `crate::proto::api`.) **Resolution of the note's `proto::modal::client::GpuConfig`: use `crate::proto::api::GpuConfig`.**
- **Python `parse_gpu_config`** (`references/.../_utils/function_utils.py:628-642`):
  `None → GPUConfig()` (empty); else split on FIRST `:` for count (default `1`; non-int → `InvalidError`);
  `gpu_type = value.upper()`; build `GPUConfig(gpu_type=<upper>, count=<n>)`. Deprecated `type` left at 0.
  **No `-MEM` special-casing** — `"A100-80GB"` → `gpu_type="A100-80GB"` verbatim (uppercased), count 1.
- **`Function.resources` stays always-`Some`** — only the NESTED `Resources.gpu_config` (field 4) is
  conditionally populated. CPU path bytes stay identical (`gpu_config` unset).

---

## 1. Macro — optional `gpu` / `timeout` / `cache` args
File: `crates/modal-rust-macros/src/lib.rs`

### 1.1 Imports (line 43)
`use syn::{parse_macro_input, ItemFn, LitStr};` → add `LitInt, LitBool`:
```rust
use syn::{parse_macro_input, ItemFn, LitBool, LitInt, LitStr};
```

### 1.2 Parsing — extend the attr-parse block (lines 62-74)
Keep `syn::meta::parser`. Add three optional keys. All optional; unknown keys still hard-error.
```rust
let mut explicit_name: Option<LitStr> = None;
let mut gpu: Option<LitStr> = None;        // gpu = "T4"
let mut timeout_secs: Option<u64> = None;  // timeout = 1800   (LitInt -> u64, narrow at emit)
let mut cache: Option<bool> = None;        // cache = false
if !attr.is_empty() {
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("name") {
            explicit_name = Some(meta.value()?.parse()?);
            Ok(())
        } else if meta.path.is_ident("gpu") {
            gpu = Some(meta.value()?.parse()?);                 // LitStr
            Ok(())
        } else if meta.path.is_ident("timeout") {
            let lit: LitInt = meta.value()?.parse()?;           // integer seconds
            timeout_secs = Some(lit.base10_parse()?);           // bad int -> compile_error!
            Ok(())
        } else if meta.path.is_ident("cache") {
            let lit: LitBool = meta.value()?.parse()?;          // true / false
            cache = Some(lit.value);
            Ok(())
        } else {
            Err(meta.error("unsupported `#[modal_rust::function]` argument; recognized: \
                `name = \"...\"`, `gpu = \"...\"`, `timeout = <int secs>`, `cache = <bool>`"))
        }
    });
    parse_macro_input!(attr with parser);
}
```
`LitInt` + `base10_parse` gives a clean `compile_error!` for non-integers/overflow, matching the
existing diagnostic style. The async/arg-count guards (lines 75-118) are UNCHANGED.

### 1.3 Emit — replace the `expanded` block (lines 119-133)
**Adopted: single emit arm, config always present, defaults to all-None.** The `gpu` literal is a
`&'static str` (so the `static` `inventory::submit!` initializer is `const`-valid, matching
`name: &'static str`). `timeout` is narrowed `u64 → u32` here.
```rust
let gpu_tok = match &gpu {
    Some(s) => quote! { ::core::option::Option::Some(#s) },        // &'static str literal
    None => quote! { ::core::option::Option::None },
};
let timeout_tok = match timeout_secs {
    Some(n) => { let n = n as u32; quote! { ::core::option::Option::Some(#n) } }
    None => quote! { ::core::option::Option::None },
};
let cache_tok = match cache {
    Some(b) => quote! { ::core::option::Option::Some(#b) },
    None => quote! { ::core::option::Option::None },
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
            },
        }
    }
};
expanded.into()
```

### 1.4 Backward-compat resolution (the two notes' open question)
The hard freeze is on **runner behavior**: dispatch, `HandlerFn`, `typed!`, `Registry::from_inventory`
DISPATCH, the CLI protocol, the five `RunnerError` kinds. The macro's *emitted tokens* are an
implementation detail, NOT part of the frozen runner protocol. The bare `#[modal_rust::function]` and
`name = "..."` forms set none of gpu/timeout/cache → `config == FunctionConfig::default()` (all `None`)
→ the runner ignores it (§2.3) → **runtime-observable behavior is byte-identical** (same `name`, same
`handler` fn pointer, same `{sum:42}`).

The single-arm emit above adds a `config:` line to the bare-form token output. That is acceptable per
the freeze (tokens are not the frozen surface). **If the implementer insists on literally byte-identical
bare-form tokens,** gate it: when `gpu.is_none() && timeout_secs.is_none() && cache.is_none()`, emit
today's exact two-field literal (`name` + `handler`, no `config:`) — which requires `Registration` to
remain constructible by a two-field literal, i.e. `config` cannot be a plain required field. Since the
adopted §2 makes `config` a plain field, the byte-identical fallback would need a trailing
`..::core::default::Default::default()` token. **Recommend the single-arm emit** (simplest, correct);
the gate is a fallback only.

---

## 2. Additive `FunctionConfig` on `Registration` (runner FROZEN)
File: `crates/modal-rust-runtime/src/lib.rs`

### 2.1 New type (add near `Registration`, ~line 262)
```rust
/// Per-function deploy/run CONFIG sourced from `#[modal_rust::function(gpu=…, timeout=…, cache=…)]`.
///
/// METADATA ONLY. The runner IGNORES every field — `run_cli`/`run_handler`/dispatch never read it.
/// Only the control-plane facade (`modal-rust`) reads it when CREATING the Modal function
/// (`Resources.gpu_config`, `timeout_secs`). The bare `#[modal_rust::function]` yields
/// `FunctionConfig::default()` (all `None` => server/facade defaults), so adding this field
/// changes nothing about how functions RUN (boundaries.md anticipated this additive extension).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FunctionConfig {
    /// GPU spec string, Modal-format (`"T4"`, `"A100"`, `"A100-80GB"`, `"H100:4"`). `None` => CPU.
    pub gpu: Option<&'static str>,
    /// Function timeout (seconds). `None` => facade default.
    pub timeout_secs: Option<u32>,
    /// Cache hint. `None` => default. Reserved/inert for P4 (no proto target — §5.4).
    pub cache: Option<bool>,
}
```
`gpu: Option<&'static str>` (not `String`): `inventory::submit!` builds a `static` initializer; only
`const`-constructible values are allowed; a string literal is `&'static str` (matches `name`).

### 2.2 Extend `Registration` (lines 267-272) ADDITIVELY
```rust
pub struct Registration {
    /// The entrypoint name (registry key).
    pub name: &'static str,
    /// The monomorphized [`typed!`] wrapper `fn` pointer.
    pub handler: HandlerFn,
    /// Per-function config (METADATA; the runner ignores it). Default = all `None`.
    pub config: FunctionConfig,
}
```
`inventory::collect!(Registration)` (line 274) UNCHANGED.

### 2.3 PROOF the runner is byte-identical
`Registry::from_inventory` (lines 296-313) reads ONLY `registration.name` + `registration.handler`;
it NEVER touches `registration.config`. Therefore UNCHANGED, byte-identical:
- `Registry` stays `BTreeMap<&'static str, HandlerFn>` (no shape change).
- `Registry::function` insertion + duplicate-name panic.
- `HandlerFn`, `typed!`, `run_cli`/`run_cli_with_args`/`run_handler`/`parse_args`/`emit`.
- The five `RunnerError` kinds + the one-line envelope.
- Dispatch `name -> HandlerFn -> bytes in -> bytes out`.
The extra `config` field is dropped on the floor during `from_inventory` collection. The
`#[cfg(test)]` runner tests (lib.rs ~595-789) use the manual `Registry::new().function(...)` builder
(no `Registration`), so they need NO change.

### 2.4 New paired collector (so the facade never imports `inventory`)
Add to runtime (returns the SAME registry as `from_inventory()` PLUS per-name configs):
```rust
/// Like [`Registry::from_inventory`] but ALSO returns the per-name [`FunctionConfig`]
/// captured from the SAME inventory pass. The registry is byte-identical to
/// `from_inventory()` (same insertion, same duplicate-name panic); the facade reads
/// the configs to set `Resources.gpu_config`/`timeout_secs` when CREATING the function.
pub fn from_inventory_with_configs() -> (Registry, Vec<(&'static str, FunctionConfig)>) {
    let mut registry = Registry::new();
    let mut configs = Vec::new();
    for r in inventory::iter::<Registration> {
        registry = registry.function(r.name, r.handler); // SAME insertion + dup-name panic
        configs.push((r.name, r.config.clone()));
    }
    (registry, configs)
}
```
Place it as a free fn or a `Registry::` assoc fn (the facade calls it qualified). `Registry::from_inventory()`
stays AS-IS (frozen) for any caller that wants only the registry.

### 2.5 Re-export from the facade
File: `crates/modal-rust/src/lib.rs:54` — add `FunctionConfig` (and the collector if it is a free fn):
```rust
pub use modal_rust_runtime::{FunctionConfig, HandlerFn, Registration, Registry, RunnerError};
```
The macro references the type by full path `::modal_rust_runtime::FunctionConfig`, so the runtime
crate only needs it `pub`; the facade re-export is for `app.rs` to name the type ergonomically.

---

## 3. Facade — capture per-name config + read it
File: `crates/modal-rust/src/app.rs`

### 3.1 New field on `App` (struct lines 20-26)
```rust
pub struct App {
    registry: Registry,
    /// Per-entrypoint config from `#[modal_rust::function(...)]`. EMPTY for the manual
    /// `App::new(registry)` / `connect_with_registry` path (no decorator => facade defaults).
    configs: std::collections::BTreeMap<String, modal_rust_runtime::FunctionConfig>,
    remote: Option<RemoteHandle>,
}
```
Key by `String` (lookups use the runtime `&str` name).

### 3.2 Constructors (all four construction sites get `configs`)
- `App::new(registry)` (lines 56-61): add `configs: std::collections::BTreeMap::new()` (EMPTY → §4 falls
  back to defaults; manual-builder behavior preserved).
- `App::from_inventory()` (lines 64-66): STOP delegating to `App::new(Registry::from_inventory())`. Build
  both from one pass:
  ```rust
  pub fn from_inventory() -> Self {
      let (registry, configs) = modal_rust_runtime::from_inventory_with_configs();
      App {
          registry,
          configs: configs.into_iter().map(|(n, c)| (n.to_string(), c)).collect(),
          remote: None,
      }
  }
  ```
- `App::connect(name)` (lines 74-76): currently `connect_with_registry(name, Registry::from_inventory())`.
  Change so the decorator config survives into the remote path:
  ```rust
  pub async fn connect(name: &str) -> Result<Self> {
      let (registry, configs) = modal_rust_runtime::from_inventory_with_configs();
      let configs = configs.into_iter().map(|(n, c)| (n.to_string(), c)).collect();
      App::connect_inner(name, registry, configs).await
  }
  ```
- `App::connect_with_registry(name, registry)` (lines 81-101): manual path → EMPTY configs:
  ```rust
  pub async fn connect_with_registry(name: &str, registry: Registry) -> Result<Self> {
      App::connect_inner(name, registry, std::collections::BTreeMap::new()).await
  }
  ```
  Extract the existing body (client connect, `app_create_ephemeral`, build `RemoteHandle`) into a private
  `connect_inner(name, registry, configs)` that stores `configs` on the returned `App`. The `RemoteHandle`
  construction is unchanged except the outer `App { registry, configs, remote: Some(...) }`.

### 3.3 Lookup helper on `App`
```rust
pub(crate) fn config_for(&self, name: &str) -> modal_rust_runtime::FunctionConfig {
    self.configs.get(name).cloned().unwrap_or_default()
}
```
Returns `FunctionConfig::default()` (all `None`) for the manual path and unknown names → defaults apply.

### 3.4 Thread the per-entrypoint config into ensure/deploy
> Superseded by the 2026-06-06 per-entrypoint Modal-function fix. The old
> "single wrapper function serves every entrypoint" plan was the source of the
> first-call config clobbering bug. Current code keeps one shared in-container
> callable (`handler(entrypoint, input_json)`) but creates one Modal function object
> tag per entrypoint. The memo key is entrypoint + effective gpu/timeout/cache/
> secrets/volumes.

- **RUN** — `remote_invoke` resolves the invoked entrypoint's config, folds it into
  a per-call clone of `RemoteConfig`, and calls `ensure_function(entrypoint, ...)`.
  `RemoteHandle.function_ids` is a `BTreeMap<RunFunctionKey, OnceCell<_>>`, so
  each entrypoint+config is single-flighted without binding the whole app to the
  first entrypoint's config.
- **DEPLOY** — `App::deploy_with` builds a per-entrypoint deploy plan and
  `deploy::deploy_function` publishes one Modal function per entrypoint over one
  shared deploy image. Each function carries its own gpu/timeout/secrets/volumes;
  `call(app, entrypoint)` resolves that entrypoint's object tag.

### 3.5 Manual `App::new` path preserved
EMPTY `configs` → `config_for` returns default → §4 uses `FunctionResources::default()` +
`REMOTE_TIMEOUT_SECS`/300 unchanged. Existing manual-builder tests + `App::new(example_add::modal_registry())`
are behavior-preserved.

---

## 4. Facade write — apply gpu + timeout to the `FunctionSpec`

### 4.1 `RemoteConfig` (remote.rs:148-171 struct; **hand-written** `impl Default` at 173-185)
`RemoteConfig` is NOT `#[derive(Default)]` — add the two fields to BOTH the struct AND the `impl Default`:
```rust
// struct (append):
/// GPU spec for this run's entrypoint (from the decorator `FunctionConfig`). `None` = CPU.
/// Set by `App::remote_invoke` from `config_for(entrypoint)` before `ensure_function`.
pub gpu: Option<String>,
/// Per-entrypoint timeout override (decorator `FunctionConfig.timeout_secs`). When `Some`,
/// REPLACES the path default (`REMOTE_TIMEOUT_SECS`).
pub timeout_override_secs: Option<u32>,
```
```rust
// impl Default (append): gpu: None, timeout_override_secs: None,
```

### 4.2 `ensure_function` — apply at the `fn_spec` build (remote.rs:321-324)
```rust
let timeout = config.timeout_override_secs.unwrap_or(config.timeout_secs);
let fn_spec = FunctionSpec::new(WRAPPER_MODULE, WRAPPER_CALLABLE, &image_id)
    .with_mount_ids(vec![client_mount_id, source_mount_id])
    .with_mount_client_dependencies(true)
    .with_timeout_secs(timeout)
    .with_gpu(config.gpu.clone())?;   // Result -> `?` (ensure_function returns Result<String>)
```
- `with_gpu(None)` is a no-op (CPU): gpu_config stays unset → CPU run path is byte-identical at the wire.
- Timeout: decorator OVERRIDES `REMOTE_TIMEOUT_SECS` literally (Python honors it literally too). **DOC:**
  RUN-path timeouts must budget for the cold in-body `cargo build`; a too-small decorator timeout can
  starve the first cold build. No floor is imposed (honor literally).

### 4.3 `DeployConfig` (deploy.rs:106-129 struct; built via `for_app` at 135-146; `Default` delegates 149-155)
Add the same two fields to the struct AND set them in `for_app` (`gpu: None, timeout_override_secs: None`).
`Default` delegates to `for_app`, so no extra edit there.

### 4.4 `deploy_function` — apply at the `fn_spec` build (deploy.rs:294-297)
```rust
let timeout = config.timeout_override_secs.unwrap_or(config.timeout_secs);
let fn_spec = FunctionSpec::new(DEPLOY_WRAPPER_MODULE, DEPLOY_WRAPPER_CALLABLE, &image_id)
    .with_mount_ids(vec![client_mount_id])
    .with_mount_client_dependencies(true)
    .with_timeout_secs(timeout)
    .with_gpu(config.gpu.clone())?;   // deploy_function returns Result<DeployedApp>
```
Deploy has no in-body build, so its timeout is purely the function's.

---

## 5. SDK — `Resources.gpu_config` mapping (mirror Python `parse_gpu_config`)
File: `crates/modal-rust-sdk/src/ops/function.rs`

### 5.1 Import (lines 19-22 `use crate::proto::api::{...}`)
Add `GpuConfig`:
```rust
use crate::proto::api::{
    DataFormat, Function, FunctionCreateRequest, FunctionGetRequest, FunctionPrecreateRequest,
    GpuConfig, Resources,
};
```
(`crate::proto::api` is the in-crate path; `proto::modal` is the public re-export. Use `proto::api` here.)

### 5.2 `parse_gpu_config` (free fn, near `FunctionResources`)
Mirror `function_utils.py:628` EXACTLY. Infallible except non-integer count (→ `Error::build`, mirroring
Python `InvalidError`). No `-MEM` split — the mem suffix rides inside `gpu_type` verbatim.
```rust
/// Parse a Modal GPU spec into a `GpuConfig`, mirroring `parse_gpu_config`
/// (modal `_utils/function_utils.py:628`). Format: `"TYPE"` or `"TYPE:count"`.
/// The MEM suffix (`"A100-80GB"`) is NOT split — it stays inside `gpu_type`
/// verbatim. `gpu_type` is uppercased; `count` defaults to 1; deprecated `type`
/// (field 1) stays 0 (Python never sets it).
fn parse_gpu_config(spec: &str) -> Result<GpuConfig> {
    let (type_part, count) = match spec.split_once(':') {
        Some((lhs, rhs)) => {
            let count: u32 = rhs.trim().parse().map_err(|_| {
                Error::build(format!("Invalid GPU count: {rhs}. Value must be an integer."))
            })?;
            (lhs, count)
        }
        None => (spec, 1),
    };
    Ok(GpuConfig {
        gpu_type: type_part.to_uppercase(),
        count,
        ..Default::default() // r#type (deprecated GPUType, field 1) stays 0
    })
}
```
`split_once(':')` = Python's `split(":", 1)`. `to_uppercase()` = `.upper()`.

### 5.3 `FunctionResources.gpu` (struct lines 28-33) + `to_proto` (lines 36-42)
Additive field, default `None` (backward-compatible; `resources_default_is_zero` still passes):
```rust
#[derive(Debug, Clone, Default)]
pub struct FunctionResources {
    pub memory_mb: u32,
    pub milli_cpu: u32,
    /// Optional GPU spec ("T4", "A100", "A100-80GB", "H100:4"). `None` = CPU-only
    /// (empty `gpu_config`, mirroring `parse_gpu_config(None)`).
    pub gpu: Option<String>,
}
```
`to_proto` stays infallible (`-> Resources`), so `function_create` (line 191) is UNTOUCHED. The GPU string
is VALIDATED at set time (§5.4 `with_gpu`), so `to_proto` re-parses with `unwrap_or_default`:
```rust
fn to_proto(&self) -> Resources {
    let gpu_config = self.gpu.as_deref().map(|s| {
        // Validated at set time (FunctionSpec::with_gpu). Re-parse is infallible here.
        parse_gpu_config(s).unwrap_or_default()
    });
    Resources {
        memory_mb: self.memory_mb,
        milli_cpu: self.milli_cpu,
        gpu_config, // None when CPU-only -> field 4 unset, same as today
        ..Default::default()
    }
}
```
**Always-set invariant preserved:** `Function.resources` (field 9) stays `Some(spec.resources.to_proto())`
(function.rs:205, UNCHANGED). Only the nested `gpu_config` (field 4) is conditionally populated. CPU →
`gpu_config: None` → wire-equivalent to an empty `GPUConfig` (server reads `gpu_type == ""` either way).

### 5.4 `FunctionSpec::with_gpu` (fallible builder, near `with_resources` line 114)
The validating entry the facade calls (validates UP FRONT so `to_proto` stays infallible):
```rust
/// Set the GPU spec on the function's resources (validated now). `None` = CPU-only.
/// Mirrors `parse_gpu_config`: "TYPE", "TYPE:count", "TYPE-MEM" (mem rides in
/// gpu_type), uppercased, count default 1. Bad count -> `Error::build`.
pub fn with_gpu(mut self, gpu: Option<impl Into<String>>) -> Result<Self> {
    let gpu = gpu.map(Into::into);
    if let Some(spec) = gpu.as_deref() {
        parse_gpu_config(spec)?; // validate up front
    }
    self.resources.gpu = gpu;
    Ok(self)
}
```
Keep `with_resources` (line 114) AS-IS for the manual path. `function_create` signature UNCHANGED — once
`FunctionResources.gpu` is populated, `gpu_config` rides into `FunctionCreate` automatically.

### 5.5 `cache` is inert in P4
`FunctionConfig.cache` has NO `Function`/`Resources` proto target in this triad (closest is image-layer
build caching, out of scope). **Store it in `FunctionConfig`/macro for forward-compat, but wire it to NO
proto field.** Do NOT invent one. Document as accepted-but-inert.

### 5.6 GPU LIST → `ranked_functions` (OUT OF SCOPE)
Single-GPU is the required path: `FunctionConfig.gpu` is `Option<&'static str>` (one spec), not a list. A
GPU LIST would route through `FunctionData.ranked_functions` (field 18) and conflicts with the FROZEN
single-`Function` + `existing_function_id` precreate trick (function.rs:184-225) — it needs its own create
variant. DEFERRED; design-only.

---

## 6. CLI — drop the legacy `--gpu` flag
Files: `crates/modal-rust-cli/src/main.rs` + `templates.rs` (config is now dynamic from the decorator).
- `main.rs`: drop the `gpu: Option<String>` clap fields, the destructures, and the `gpu` params on
  `shim_params` / `cmd_run` / `cmd_deploy`.
- `templates.rs`: drop `ShimParams.gpu`, `gpu_kwarg()`, and the `{{GPU_KWARG}}` substitutions; remove
  `{{GPU_KWARG}}` from the template strings. The no-gpu prototype test already expects no `gpu=`, so it
  passes once the field is gone.
- Secondary/cheap; CLI is the legacy path. Keep CLI tests green.

---

## 7. FROZEN invariants honored
- Runner CLI protocol, `HandlerFn`, `typed!`, `Registry::from_inventory` DISPATCH: UNTOUCHED — config is
  read ONLY by the facade (a NEW reader); `from_inventory` still reads only `name`+`handler`.
- Bare `#[modal_rust::function]` → `FunctionConfig::default()` (all `None`) → runner ignores → `{sum:42}`
  unchanged. CPU `.remote()` wire bytes identical (gpu_config unset).
- `function_create` signature + single-`Function`/`existing_function_id`/empty-`function_serialized`/
  `function_data:None` shape: UNTOUCHED. `Function.resources` (field 9) stays always-`Some`; only nested
  `gpu_config` (field 4) conditionally set.
- run-vs-deploy build boundary; `retry_transient`/`retry_unary` on all RPCs; add_python image +
  cargo-scoped upload: UNTOUCHED. Do NOT touch README.md or examples/orchestrate.

---

## 8. Files changed (summary)
- `crates/modal-rust-macros/src/lib.rs` — imports (L43); attr-parse block (L62-74); emit (L119-133).
- `crates/modal-rust-runtime/src/lib.rs` — add `FunctionConfig` (~L262); add `config` to `Registration`
  (L267-272); add `from_inventory_with_configs()`. Runner code + tests UNCHANGED.
- `crates/modal-rust/src/lib.rs` — re-export `FunctionConfig` (L54).
- `crates/modal-rust/src/app.rs` — `App.configs` field (L20-26); `new`/`from_inventory`/`connect`/
  `connect_with_registry` (+ `connect_inner`) carry configs (L56-101); `config_for`; set
  `RemoteConfig.gpu/timeout_override_secs` in `remote_invoke` (L108-152) and `DeployConfig` in
  `deploy_with`.
- `crates/modal-rust/src/remote.rs` — `RemoteConfig` struct + hand-written `impl Default` (L148-185) gain
  `gpu`/`timeout_override_secs`; apply at `fn_spec` (L321-324).
- `crates/modal-rust/src/deploy.rs` — `DeployConfig` struct (L106-129) + `for_app` (L135-146) gain the two
  fields; apply at `fn_spec` (L294-297).
- `crates/modal-rust-sdk/src/ops/function.rs` — import `GpuConfig` (L19-22); `parse_gpu_config`; `gpu` on
  `FunctionResources` (L28-33); populate `gpu_config` in `to_proto` (L36-42); `FunctionSpec::with_gpu`
  (~L114); unit tests.
- `crates/modal-rust-cli/src/main.rs` + `templates.rs` — drop `--gpu` flag/passthrough/`{{GPU_KWARG}}`.

---

## 9. Verification (default-members)
`cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test`.
- Runtime runner tests (lib.rs ~595-789): NO change (manual-builder path).
- Add tests:
  - **Macro**: bare `#[modal_rust::function]` yields `FunctionConfig::default()`;
    `(gpu="T4", timeout=1800, cache=false)` yields the populated config (expansion/inventory assertion).
  - **SDK `parse_gpu_config`**: `"T4"`→`{gpu_type:"T4",count:1}`; `"t4"`→`"T4"`; `"H100:4"`→`{count:4}`;
    `"A100-80GB"`→`{gpu_type:"A100-80GB",count:1}`; `"A100-80GB:2"`→`{count:2}`; lowercase→uppercased;
    `"T4:x"`→`Err`. Plus `to_proto` with/without gpu; `FunctionSpec::new(...).with_gpu(Some("T4"))?` populates
    field 4; keep `resources_default_is_zero` (gpu `None` → gpu_config `None`).
  - **Facade**: `App::from_inventory().config_for("name")` populated; `App::new(registry).config_for("x")`
    is `default()`.
- cuda-vector-add stays cudarc dynamic-loading (builds WITHOUT a CUDA toolkit) — keep no-CUDA CI green.
- **Live GPU proof**: behind `#[ignore]` + the `live` feature, cheap **T4**, EPHEMERAL app (run path),
  `retry_transient`; DRIVE to a terminal result. PROOF `gpu_config` was set = decode the outbound
  `FunctionCreateRequest.function.resources.gpu_config` (gpu_type="T4", count=1) and/or server-side function
  inspection, plus the decoded GPU result. Modal flakiness => RETRY.
