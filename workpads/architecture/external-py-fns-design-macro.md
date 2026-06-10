# Calling External Python Modal Functions: Typed Macro Declaration WITHOUT Builder Chaining

**Status:** design study (2026-06-10). Read-only analysis; no code changed.
**Product question:** modal-rust users want to call Modal functions that were deployed
independently by the *Python* SDK — with kwargs, keeping the Python signature — and the
user explicitly wants a **plain typed call**, not a chained builder
(`embed("hi").model("small").remote().await?` — "B looks terrible, mostly because of
chaining").

---

## 0. Ground truth (verified in this repo, with file:line)

| Fact | Where |
| --- | --- |
| `invoke_cbor_with_deadline(function_id, args, kwargs, deadline)` already encodes a CBOR 2-tuple `(args, kwargs)` with `DATA_FORMAT_CBOR` and is **generic over `K: Serialize`** — any serde struct serializing to a map becomes the Python kwargs dict | `crates/modal-rust-sdk/src/ops/invoke.rs:168-186`, payload at `:180-181`, `DataFormat::Cbor` at `:213` |
| The facade today always sends **empty kwargs** (`HashMap<String, ()>`) and a wrapper-specific args tuple `(entrypoint, input_json)` | `crates/modal-rust/src/app.rs:380-389`, `crates/modal-rust/src/deploy.rs:449-471` (`call_function`) |
| `function_from_name(app_name, function_name, env)` resolves a deployed function via `FunctionGet`, but **drops `handle_metadata`** and returns only the `function_id` string | `crates/modal-rust-sdk/src/ops/function.rs:964-986`; `FunctionGetResponse` at `proto/api.proto:2122-2126` |
| `FunctionHandleMetadata` carries `function_schema` (field 45) **and** `supported_input_formats`/`supported_output_formats` (fields 50/51) — both available in the same `FunctionGet` round-trip we already make | `proto/api.proto:2137-2161` |
| `FunctionSchema = { schema_type, arguments: repeated ClassParameterSpec, return_type: GenericPayloadType }` | `proto/api.proto:2302-2310` |
| `ClassParameterSpec = { name, type, has_default, default_oneof, full_type }`; `ParameterType` has STRING/INT/BYTES/LIST/DICT/NONE/BOOL + **UNKNOWN for unannotated** and **no float type** | `proto/api.proto:930-943`, `:216-226` |
| Output decode hard-fails on non-CBOR (`expected CBOR output, got PICKLE`) | `invoke.rs:65-73` |
| Blob-sized results are **unimplemented** ("blob fetch is not yet implemented"); args are inline-only | `invoke.rs:286-294`, `:193` |
| Remote Python failures surface as `GenericResult` failures → `Error::build(describe_failure(..))` carrying `exception` + `traceback` — **not** our runner five-kind JSON envelope | `invoke.rs:298-303`, `crates/modal-rust-sdk/src/ops/mod.rs:76-101` |
| The existing `#[function]` Mode B generates `pub mod <fn> { Input, Output }` + a spread shim + a per-fn `<Pascal>Call` trait on `App` returning the chained `TypedCall` builder | `crates/modal-rust-macros/src/lib.rs:738-818`; `TypedCall` at `crates/modal-rust/src/function.rs:345-503` |
| Mode B's trait needs a glob import (`use my_crate::*`) to bring `app.add(..)` into scope — an acknowledged wart | `crates/modal-rust-macros/src/lib.rs:117-129` |
| Stance 3: prefer static dispatch. Stance 4: macro crate tonic-free, macro-generated code delegates to **always-present facade fns** (real under `client`, erroring stubs otherwise), macro never emits `cfg` | `AGENTS.md:96-113` |
| RUN-path function-id memoization precedent: `RemoteHandle.function_ids: Mutex<BTreeMap<Key, Arc<OnceCell<String>>>>` (single-flighted per key) | `crates/modal-rust/src/app.rs:82-98`, `:424-468` |
| Our own deploy wrapper is a **plain `@app.function` Python function** and answers our CBOR-in invoke with a CBOR string out, live-proven on the deploy/call path | `crates/modal-rust/src/deploy/wrapper.py:1-40`, `deploy.rs:449-471` |

**Spiked during this study (2026-06-10, rustc 1.96.0):** an attribute proc-macro **CAN
legalize a bodyless free `fn`** — `#[spikemacro::import] async fn embed(text: String,
#[kwarg] model: Option<String>) -> Vec<f32>;` at module scope (and inside a plain
`mod`) parses, the macro receives the tokens including the param attribute, replaces
the item, and the crate builds clean. Rust's "free function without a body" check is
AST-validation, which runs **after** macro expansion. This is the same mechanism
`#[cxx::bridge]`-style importers rely on; it unlocks the nicest declaration form.

---

## 1. API candidates

The running example: a Python app `text-embedder` (deployed by the Python SDK,
modal-rust knows nothing about it) with

```python
@app.function()
def embed(text: str, model: str = "small", normalize: bool = True, batch_size: int = 32) -> list[float]: ...

@app.function()
def train(dataset: str, epochs: int = 3, lr: float = 3e-4, warmup_steps: int = 0,
          seed: int = 42, wandb_project: str | None = None, resume_from: str | None = None) -> str: ...
```

### Candidate A — bare facade call, no macro (the foundation everything lowers to)

New always-present facade fns on `App` (real under `client`, stub otherwise — the
stance-4 pattern of `Function::remote`, `function.rs:73-95`):

```rust
#[derive(Default, serde::Serialize)]
struct EmbedKwargs {
    #[serde(skip_serializing_if = "Option::is_none")] model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] normalize: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")] batch_size: Option<u32>,
}

let v: Vec<f32> = app
    .py_remote("text-embedder", "embed", &("hi",), &EmbedKwargs { model: Some("small".into()), ..Default::default() })
    .await?;
```

* **Pros:** zero macro machinery; ships in a day on top of `invoke_cbor` (invoke.rs:168)
  + `function_from_name` (ops/function.rs:964); the user writes their own kwargs struct
  (full control). This is "design A" and must exist regardless — it is what the macro
  expands to.
* **Cons:** app/function names are stringly per call site; the user hand-maintains the
  kwargs struct and the output type; nothing checks the declared shape against the
  deployed `FunctionSchema`; turbofish or annotation needed for `R`.

### Candidate B — `#[modal_rust::import]` attribute on a bodyless fn (RECOMMENDED)

Declare the Python signature once, as a signature:

```rust
use modal_rust::import;

#[import(app = "text-embedder")]
async fn embed(text: String, model: Option<String>, normalize: Option<bool>, batch_size: Option<u32>) -> Vec<f32>;

#[import(app = "text-embedder", env = "main", timeout = 7200)]
async fn train(dataset: String, epochs: Option<u32>, lr: Option<f64>, warmup_steps: Option<u32>,
               seed: Option<u64>, wandb_project: Option<String>, resume_from: Option<String>) -> String;
```

Rule: **non-`Option` params are required → sent positionally; `Option<T>` params are
optional → folded into ONE generated `<Pascal>Opts` struct and sent as kwargs (skipped
when `None`, so Python defaults apply)**. The generated callable is a PLAIN free async
fn — required params positional, one trailing `impl Into<…Opts>`:

```rust
// zero optionals supplied — `()` converts into the default Opts:
let v = embed(&app, "hi".into(), ()).await?;

// one optional — struct-update is Rust's named arguments:
let v = embed(&app, "hi".into(), EmbedOpts { model: Some("small".into()), ..Default::default() }).await?;

// many kwargs — reads like a Python call with keywords:
let run_id = train(&app, "s3://corpus".into(), TrainOpts {
    epochs: Some(10),
    lr: Some(1e-4),
    wandb_project: Some("embed-v2".into()),
    ..Default::default()
}).await?;

// fire-and-forget / fan-out are SIBLING fns, not chain links:
let call = train_spawn(&app, "s3://corpus".into(), ()).await?;       // -> PyCall<String>
let id   = call.function_call_id();
let out  = call.get(None).await?;                                     // typed String
let vecs = embed_map(&app, texts.into_iter().map(|t| (t, EmbedOpts::default()))).await?;
```

* **Pros:** the declaration **is** the Python signature (one place, reviewable against
  the Python source); the call site has no chain, no builder, no trait import (free fns
  in the user's own module — strictly better scoping than Mode B's `<Pascal>Call` glob
  trait, macros lib.rs:117-129); optionals get true named-argument ergonomics with
  compile-time-required required params; `()` keeps the zero-kwargs case clean;
  rust-analyzer shows real fns with real types.
* **Cons:** bodyless free fns are unusual Rust (legal via post-expansion AST validation
  — spiked green on 1.96, but it's a parser-behavior dependence worth a CI canary); the
  generated signature differs from the declared one (macro inserts `&App` + folds
  options) — must be documented loudly and shown by rustdoc on the generated fn.

### Candidate C — `import_py!` function-like block (grouping variant of B)

```rust
modal_rust::import_py! {
    app "text-embedder", env "main";
    async fn embed(text: String, model: Option<String>, normalize: Option<bool>, batch_size: Option<u32>) -> Vec<f32>;
    async fn train(dataset: String, epochs: Option<u32>, /* … */) -> String;
}
```

Same generation and call sites as B; the app name is written once for N functions.

* **Pros:** no bodyless-fn novelty (a `macro_rules!`-style fn-like proc macro fully owns
  its grammar); natural place for shared app/env/timeout.
* **Cons:** invented grammar inside a bang-macro: worse rustfmt/IDE behavior, worse
  diagnostics spans, and it *looks* less like ordinary Rust than B does. B + repeating
  `app = "..."` (or B applied to fns inside a user `mod py { … }`) covers grouping well
  enough.

### Candidate D — args-struct-as-the-call ("record literal call")

```rust
let v = Embed { text: "hi".into(), model: Some("small".into()), ..Default::default() }
    .remote(&app).await?;
```

* **Pros:** one struct literal = named args for *everything*; single method, arguably
  not "chaining".
* **Cons (disqualifying):** required fields must be in the `Default` for `..Default::
  default()` to work, so **forgetting `text` compiles** and sends `""` — exactly the
  class of silent bug a typed surface exists to prevent. Splitting required out of the
  struct re-creates candidate B with extra steps. Also `.remote(&app)` is still a
  method hanging off a temporary, which is the aesthetic the user dislikes.

### Connection source — weighed honestly

| Option | Verdict |
| --- | --- |
| **Explicit `&App` first arg** (chosen) | Fits the whole codebase: `App` owns the client behind a `Mutex` (app.rs:68-98), the not-connected error, the env, and the testkit mock seam (`connect_at`, app.rs:291-315). Cost is literally one `&app` token per call. Multi-workspace/multi-env "just works". |
| Ambient global client (`modal_rust::init()` + `OnceLock`), Python-style `py::embed("hi", ()).await?` | Max ergonomics, and the user said they'd break conventions. But: it forks the connection story (everything else goes through `App`), poisons parallel tests that need per-test mock URLs (the exact reason `connect_at` exists, app.rs:285-291), makes env selection ambient, and saves only the `&app` token. **Rejected for v0; can be layered later** as `modal_rust::set_default_app(app)` + generated `embed_d(text, opts)` siblings IF demanded — nothing in candidate B's shape blocks it. |
| Per-module `py::init(&app)` once, then bare calls | Same global-state problems, scoped slightly better, still a second connection story. Rejected. |

Sync vs async: **async**, like every remote facade method; the declared fn must be
written `async fn` so the declaration is honest. Error type: `modal_rust::Result<T>`
(declared `-> T` is the `Ok` type), with remote Python failures surfacing as the
existing `Error::Sdk` ← `Error::build(describe_failure(..))` carrying the Python
`exception` + `traceback` (ops/mod.rs:76-101) — plus one new variant for schema
mismatch (below).

---

## 2. Recommendation

**Candidate B**, `#[modal_rust::import(app = "...", env = "...", timeout = N)]` on a
bodyless `async fn`, generating plain free async fns with `&App` first and a trailing
`impl Into<…Opts>` for the kwargs. Rationale:

1. **It is the no-chaining surface the user asked for**, and it beats the existing
   Mode B ergonomics on its own terms: no extension trait, no glob import, no builder.
2. **Struct-update on a generated `Opts` struct is the best Rust has for named/optional
   arguments** — call sites for 0 / 1 / many optionals are all readable (§1B), unlike
   `Option` positionals (`embed(&app, "hi", None, None, None)` is unreviewable: which
   `None` is which?).
3. **Declared-signature-as-source-of-truth enables first-call schema validation** for
   free: the macro consts out the declared param list; the facade compares it against
   the deployed `FunctionSchema` fetched in the same `FunctionGet` it already needs for
   the id (proto api.proto:2137-2161 — zero extra RPCs).
4. **It is stance-clean** (§4): the macro emits only calls to always-present facade
   fns; static dispatch throughout; the macro crate stays tonic-free.
5. Wire mapping "required → positional tuple, optional → kwargs map, `None` omitted"
   reproduces Python call semantics exactly: omitted kwargs fall through to Python
   defaults; positional-only required params (`def f(x, /)`) still work; the rare
   keyword-only required param is the one documented gap (per-param `#[kw]` marker is
   the reserved fix).

Candidate A (`App::py_remote` etc.) ships **as part of** this design — it is the facade
layer B expands into, and is independently useful for dynamic call sites.

---

## 3. Exact expansion sketch (recommended candidate)

Input:

```rust
#[modal_rust::import(app = "text-embedder", env = "main")]
async fn embed(text: String, model: Option<String>, normalize: Option<bool>, batch_size: Option<u32>) -> Vec<f32>;
```

Expansion (`#facade` resolved via the existing `facade_path()`, macros lib.rs:168-177;
visibility copied from the declaration):

```rust
/// Auto-generated optional-kwargs for `embed` (one field per `Option<T>` param,
/// declared order). `None` fields are OMITTED from the wire kwargs map, so the
/// Python default applies. Build with struct-update: `EmbedOpts { model: Some(..),
/// ..Default::default() }`, or pass `()` for all-defaults.
#[derive(Default, ::serde::Serialize)]
pub struct EmbedOpts {
    #[serde(skip_serializing_if = "Option::is_none")] pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub normalize: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")] pub batch_size: Option<u32>,
}
impl ::core::convert::From<()> for EmbedOpts {
    fn from(_: ()) -> Self { Self::default() }
}

/// Declared target + signature, const'd for memoized lookup + first-call schema
/// validation. `PyParam.ty` is the coarse `ParameterType` mapped from the Rust type
/// (String→STRING, i*/u*→INT, bool→BOOL, Vec<u8>→BYTES, Vec<_>→LIST, f32/f64 & other
/// →UNKNOWN — the proto has no float type, api.proto:216-226).
const __MODAL_RUST_PY_EMBED: #facade::PyTarget = #facade::PyTarget {
    app_name: "text-embedder",
    function_name: "embed",
    environment: Some("main"),
    timeout_secs: None,
    params: &[
        #facade::PyParam { name: "text",       required: true,  ty: #facade::PyParamType::Str },
        #facade::PyParam { name: "model",      required: false, ty: #facade::PyParamType::Str },
        #facade::PyParam { name: "normalize",  required: false, ty: #facade::PyParamType::Bool },
        #facade::PyParam { name: "batch_size", required: false, ty: #facade::PyParamType::Int },
    ],
};

/// Call the EXTERNAL Python Modal function `text-embedder/embed` (deployed by the
/// Python SDK). Generated by `#[modal_rust::import]`; requires a connected App.
pub async fn embed(
    app: &#facade::App,
    text: String,
    opts: impl ::core::convert::Into<EmbedOpts>,
) -> #facade::Result<Vec<f32>> {
    app.py_remote::<_, _, Vec<f32>>(&__MODAL_RUST_PY_EMBED, &(text,), &opts.into()).await
}

/// Fire-and-forget sibling: enqueue and return a typed handle immediately.
pub async fn embed_spawn(
    app: &#facade::App,
    text: String,
    opts: impl ::core::convert::Into<EmbedOpts>,
) -> #facade::Result<#facade::PyCall<'_, Vec<f32>>> {
    app.py_spawn::<_, _, Vec<f32>>(&__MODAL_RUST_PY_EMBED, &(text,), &opts.into()).await
}

/// Fan-out sibling: one item per input = (required params tuple…, Opts).
pub async fn embed_map<I>(app: &#facade::App, inputs: I) -> #facade::Result<Vec<Vec<f32>>>
where I: IntoIterator<Item = (String, EmbedOpts)> {
    let inputs: Vec<((String,), EmbedOpts)> =
        inputs.into_iter().map(|(text, o)| ((text,), o)).collect();
    app.py_map::<_, _, Vec<f32>>(&__MODAL_RUST_PY_EMBED, &inputs).await
}
```

Notes:
* `args` is the required-params tuple `&(text,)` — serde serializes a 1-tuple as a
  1-element CBOR array, matching the `(args, kwargs)` payload `invoke_cbor` already
  builds (invoke.rs:180-181). Zero required params ⇒ `&()` (empty array — verify in the
  CBOR spike that `()` serializes as `[]` not `null`; if not, emit `&[(); 0]` or a
  unit-struct workaround).
* The kwargs struct serializes as a CBOR map keyed by the Rust param idents — these
  **must equal the Python parameter names**; a per-param rename attr
  (`#[kwarg(name = "...")]`) is reserved follow-up.
* No optional params ⇒ no Opts struct, no trailing arg: `fn rerank(app, query, docs)`.
* Spawn/map as **sibling fns** keeps everything flat (the no-chaining promise);
  `PyCall<Out>` is the one small handle type (mirroring `TypedFunctionCall`,
  function.rs:514-540, but decoding the raw CBOR value instead of the runner JSON
  envelope).
* Declaring `async` is REQUIRED; a non-`Option` param after the first `Option` param is
  allowed (order only matters within the positional tuple). `#[kw]` on a required param
  (forces by-name send, for Python keyword-only params) is reserved v-next.

---

## 4. Repo stances: what bends, what breaks

| Stance / convention | Status | Justification |
| --- | --- | --- |
| Stance 4 — macro crate tonic-free; generated code delegates to always-present facade fns; macro never emits `cfg` (AGENTS.md:101-113) | **KEPT** | `#[import]` emits only `#facade::App::py_remote/py_spawn/py_map` calls + plain serde structs. Those facade fns get light-build erroring stubs exactly like `Function::remote` (function.rs:88-95). The wire bytes are feature-independent. |
| Stance 3 — static dispatch (AGENTS.md:96-100) | **KEPT** | Everything is monomorphized generics (`A/K/R: Serialize/DeserializeOwned`); no `dyn`, no registry. |
| Runner protocol / five-kind envelope (AGENTS.md:115-137) | **KEPT (by exclusion)** | External Python functions never touch the runner. Their failures are Modal `GenericResult` failures → `Error::Sdk` with `exception`+`traceback` (ops/mod.rs:76-101). Docs must say plainly: no five-kind taxonomy, no `details` field, for imported fns. |
| Established typed-call surface = chained `TypedCall` builder off `App` trait methods (function.rs:345-503, macros lib.rs:774-803) | **BENT** | This introduces a second, flat call style for imports. Justified: explicit user demand ("no chaining"), and imports lack `.local()` — the chain's main payoff (one handle, many verbs) doesn't apply to a function whose body lives in another repo. Long-term, the flat style could back-port to Mode B; out of scope here. |
| "Macro-generated typed methods hang off `App` as traits" | **BROKEN (improved)** | Free fns need no trait import / glob (the Mode B wart, macros lib.rs:117-129). Coherence is a non-issue for free fns. |
| Idiomatic-Rust conservatism: bodyless free fns | **NOVEL** | Legal (post-expansion AST validation; spiked green on rustc 1.96, incl. param attributes and inside `mod`). Risk: hypothetical future parser tightening. Mitigations: a trybuild canary test in CI; candidate C (`import_py!` block) is the drop-in fallback with identical generation. |
| Explicit-handle philosophy (no ambient client) | **KEPT** | Ambient/global client rejected for v0 (§1, connection table) — it forks the connection story and breaks per-test mock URLs (app.rs:285-291) to save one token. |

---

## 5. Implementation map

**SDK (`crates/modal-rust-sdk`)** — small, additive:
* `src/ops/function.rs`: add `function_from_name_with_metadata(app_name, function_name, env) -> Result<PyFunctionMeta>` next to `function_from_name` (:964, unchanged), where `PyFunctionMeta = { function_id, function_schema: Option<FunctionSchema>, supported_input_formats: Vec<DataFormat>, supported_output_formats: Vec<DataFormat> }` — all read from the existing `FunctionGetResponse.handle_metadata` (api.proto:2122-2161).
* `src/ops/invoke.rs`: **no changes** — `invoke_cbor_with_deadline` (:168), `spawn_cbor` (:322), `map_cbor` (:412), `get_by_call_cbor` (:376) are already generic over `(A, K)` and fit verbatim.

**Facade (`crates/modal-rust`)**:
* New `src/py.rs`: `PyTarget` (`&'static` fields, const-constructible), `PyParam`/`PyParamType`, `PyCall<'a, Out>` (wraps `function_call_id` + `&App`; `get()` → `App::py_get` → `get_by_call_cbor`, decoding the raw value — NOT `parse_envelope`), and `check_schema(declared: &[PyParam], deployed: &FunctionSchema) -> Result<()>` (pure fn, unit-testable offline).
* `src/app.rs`: `App::py_remote<A, K, R>(&self, target, args, kwargs)`, `py_spawn`, `py_map`, `py_get` — `#[cfg(feature = "client")]` real + `#[cfg(not(...))]` stubs, the existing pattern (function.rs:73-95). Resolution head mirrors `resolve_function` (app.rs:410-468): `RemoteHandle` gains `py_functions: Mutex<BTreeMap<PyKey, Arc<OnceCell<ResolvedPy>>>>` where `PyKey = (app_name, function_name, environment)` and `ResolvedPy = { function_id, /* schema check already performed */ }` — the memo lives in the App, single-flighted per target, because there is no user-held handle to hang it on. First resolution: `function_from_name_with_metadata` → (a) error clearly if `supported_input_formats` excludes CBOR, (b) run `check_schema` if a schema is present (absent schema ⇒ skip, log nothing — Python clients may omit it, api.proto:1792 comment), (c) memoize the id. `timeout_secs` from the target (default `DEFAULT_INVOKE_DEADLINE`-compatible 600s, invoke.rs:51) feeds the poll deadline.
* `src/error.rs`: add `Error::PySchema { target, problems }` (schema-mismatch with the declared-vs-deployed diff) — distinct from `Error::Sdk` so callers can catch drift specifically.
* `src/lib.rs`: export `py::{PyTarget, PyParam, PyParamType, PyCall}` and re-export the `import` macro.

**Macros (`crates/modal-rust-macros`)**:
* New `#[proc_macro_attribute] pub fn import` — parses `app = ".."` (required), `env = ".."`, `timeout = N`; parses the item as a bodyless fn (`syn` via `TraitItemFn`-style parse or a small custom parse — `ItemFn` requires a body, so parse the signature + `;` manually); classifies params `Option<T>` → kwarg / other → positional (syntactic, same spirit as the Mode A/B classifier, lib.rs:680); emits §3. Reuses `facade_path()` (:168) and `to_pascal_case`. Rejects: non-`async`, `self`, generics/lifetimes, patterns other than plain idents, zero-`Option` duplicate Opts, `-> ()`-less missing return type (require explicit `-> T`).
* Diagnostics: missing `app =` names the expected syntax; an `Option<Option<T>>` param is rejected (ambiguous None-vs-absent).

**Tests / examples**:
* Offline: expansion ui-tests; `check_schema` table tests; mock-backend `FunctionGet` returning handle metadata with/without schema and with pickle-only formats (reuse the existing mock harness used by `connect_at`, app.rs:291).
* Live spike crate `examples/py-import` (external-crate proof per repo memory): deploy a tiny Python app with `modal deploy` (kwargs + defaults), then call via the macro — proves CBOR-in/CBOR-out and defaults fall-through.

**Estimated size:** SDK ~80 lines; facade ~350; macro ~400; tests dominate.

---

## 6. Failure-mode table

| Failure | When it surfaces | Behavior / mitigation |
| --- | --- | --- |
| App not deployed / wrong app name | First call (memoized lookup) | `FunctionGet` NOT_FOUND → error naming `app/function@env` and suggesting `modal app list` / checking `env =`. |
| Env mismatch (deployed in `prod`, target says `main`) | First call | Same NOT_FOUND path; the error must print the environment it searched (today `function_from_name`'s error omits env — fix in the new metadata fn). |
| Function deployed pickle-only (Python SDK version that doesn't advertise/accept CBOR) | First call (format check) or output decode | Pre-check `supported_input_formats` from handle metadata → actionable error ("upgrade modal>=X / function does not accept CBOR") instead of a remote TypeError; output side already hard-fails with `expected CBOR output, got PICKLE` (invoke.rs:66-71). |
| Signature drift: kwarg renamed/removed in Python | First call if schema present (PySchema error with the diff); else at invoke | Without schema: Python raises `TypeError: unexpected keyword argument 'model'` → `Error::Sdk` with full traceback (ops/mod.rs:76-89) — survivable but noisier. |
| Required param added on the Python side (with no default) | First call (schema: `has_default=false` name not in declared) or remote TypeError | Schema check catches it before paying a container cold start. |
| Type drift (str→int) | Schema check (coarse) or remote behavior | Only coarse types exist; `UNKNOWN` (unannotated, api.proto:222) and floats (no PARAM_TYPE) are unverifiable → those comparisons are skip-not-fail. Type problems are WARN-level in the PySchema report; name problems are errors. |
| Positional-only Python params (`def f(x, /)`) | Never (by construction) | Required params are sent positionally — works. |
| Keyword-only required params (`def f(*, x)`) | Remote TypeError ("takes 0 positional arguments") | v0 gap; documented. Reserved `#[kw]` param marker forces by-name send. Schema note: proto doesn't expose positional/keyword-only kinds, so this cannot be pre-checked. |
| Rust field name ≠ Python kwarg name | Remote TypeError / silently-ignored? (No — Python raises) | `#[kwarg(name = "...")]` rename attr is the reserved fix; schema name-check catches it first when schema exists. |
| Large args (> inline limit) | Enqueue | `args_blob_id` upload unimplemented (invoke.rs:193 "inline args only") — gRPC error or server rejection; document the limit; blob upload is a pre-existing roadmap item. |
| Large results | Output poll | Hard error today: "blob result; blob fetch is not yet implemented" (invoke.rs:288-292). Same roadmap item; the error already names the cause. |
| Python function raises | Output poll | `GenericResult` FAILURE → `Error::Sdk` with `exception` + `traceback` — the правильный envelope for external fns; do NOT wrap in the runner taxonomy. |
| CBOR type fidelity (datetime, set, tuple-vs-list, bytes) | Decode either direction | Spike (§7); restrict v0 docs to JSON-ish types (str/int/float/bool/bytes/list/dict/None). |
| Schema absent (older Python client / collection failed) | First call | Skip validation silently (the proto explicitly allows missing schema, api.proto:1792); everything still works, drift errors degrade to remote TypeErrors. |

---

## 7. Open spikes (ordered)

1. **CBOR round-trip on a vanilla Python-SDK function** (the gating spike): deploy
   `def echo(a, b=2): return {"a": a, "b": b}` with current `modal` Python, invoke with
   `DATA_FORMAT_CBOR` `(args, kwargs)` from this SDK, confirm CBOR comes back and
   kwargs/defaults behave. Evidence it works: our deploy wrapper is itself a plain
   `@app.function` answering CBOR today (wrapper.py + deploy.rs:449-471). Also record:
   which `modal` Python versions advertise CBOR in `supported_input_formats`, and what
   the server does when the field is empty (older deployments).
2. **Schema availability:** does `FunctionGet.handle_metadata.function_schema` actually
   populate for ordinary Python deployments (and is `full_type` set vs legacy `type`)?
   Drives whether the schema check is on-by-default or best-effort.
3. **Empty-args encoding:** confirm serde→CBOR of `&()` for zero required params yields
   an empty array (Python expects `args` to be a sequence); pick the workaround if not.
4. **CBOR type fidelity matrix:** ciborium ↔ Python `cbor2` for f64, bytes, datetime
   (tag 0/1), large ints, None-in-Option round-trips.
5. **Bodyless-fn canary:** keep the /tmp spike as a trybuild test so a future rustc
   regression on attribute-legalized bodyless fns is caught in CI, with the
   `import_py!` block (candidate C) as the planned fallback surface.
6. **Keyword-only params + per-param rename:** design `#[kw]` / `#[kwarg(name = ..)]`
   once a real user hits them (cheap, additive).

---

## 8. Ratified decisions & author workflow (2026-06-10 design review)

**Status: accepted as the v0 surface — with an explicit ergonomics caveat.** The
user is not fully satisfied with the call-site shape (the `&app` first argument
plus the `()` / `Opts { .. }` trailing argument) and accepts it **for v0 only**.
Revisit before v1; candidates to explore then: an ambient/per-module client (drops
`&app`), a function-like surface, or named-args sugar if the language ever grows
it. Do not treat the v0 call shape as a frozen contract.

**Sequencing (vs the sibling bind doc):** the shared substrate
(`function_from_name_with_metadata`, facade `py.rs`, spike S1) plus `#[import]`
ship FIRST. `modal-rust bind` (external-py-fns-design-bind.md) layers on later as
a generator/validator of this exact surface — a hand-declared `#[import]` is the
null-snapshot mode of the same machinery.

**Args rule (ratified, supersedes the bind doc's kwargs-only shape):** required
(non-`Option`) params travel positionally in the CBOR args tuple; `Option<T>`
params travel as kwargs with `None` omitted. This mirrors how a Python caller
writes the call; positional-only params work; keyword-only-required is the
documented v0 gap (`#[kw]` reserved).

**Author workflow (the contract this design must preserve):**

```toml
modal-rust = { version = "...", features = ["client"] }   # + ~/.modal.toml
```

```rust
// Python side (deployed independently; source may be unavailable):
//   @app.function()
//   def embed(text: str, model: str = "small", normalize: bool = True,
//             batch_size: int = 32) -> list[float]: ...

#[modal_rust::import(app = "text-embedder")]
async fn embed(text: String, model: Option<String>, normalize: Option<bool>,
               batch_size: Option<u32>) -> Vec<f32>;

let app = App::connect("my-rust-app").await?;
let v  = embed(&app, "hello".into(), ()).await?;                          // all defaults
let v  = embed(&app, "hello".into(),
               EmbedOpts { model: Some("large".into()), ..Default::default() }).await?;
let c  = embed_spawn(&app, "hello".into(), ()).await?;                    // fire-and-forget
let v: Vec<f32> = c.get().await?;
```

First call resolves + memoizes the function ID and validates the declaration
against the deployed `FunctionSchema` (names hard, types best-effort), so drift
fails loudly at first call, not as a mid-call decode error.

## 9. Pickle boundary (ratified)

Modal's Python SDK serializes with cloudpickle by default; this design works by
sending `DATA_FORMAT_CBOR` and expecting CBOR back (decode hard-fails otherwise,
invoke.rs:66). Three distinct scenarios:

1. **Inputs — function only accepts pickle** (older `modal` client deployment).
   Clean preflight failure: `supported_input_formats` arrives in the same
   `handle_metadata` we already fetch — error reads "deployed with modal < X /
   does not advertise CBOR input; redeploy with a newer client". A plain-data
   pickle *encode* compat mode (serde-pickle serializes; no code in plain-data
   pickles) is possible but deferred until a real user hits this.
2. **Outputs — function returns non-CBOR-able data** (numpy array, dataclass,
   datetime, custom class → runtime pickles the result). No Rust-side fix exists
   for the hard cases: a cloudpickled object references Python class definitions
   that don't exist in Rust. Tiers: **v0 = hard error with a diagnostic message**
   ("result arrived as PICKLE; return JSON-able data or add a thin wrapper
   endpoint") — never a silent fallback; later, *opt-in* serde-pickle decode for
   plain-data pickles only; the durable answer is a five-line Python-side adapter
   `@app.function` returning plain data — document this as the blessed pattern,
   not a defeat.
3. **Pass-through blobs** (spawn handles consumed by Python, Dict/Queue values
   written by Python): treat foreign pickle as opaque bytes — never decode, never
   re-emit as validated. Security asymmetry: Rust *receiving* pickle carries no
   deserialization-RCE risk (we never execute it; serde-pickle interprets
   without executing), but a Rust service must not become a laundering hop that
   forwards untrusted pickle to Python consumers who will execute it.

**Design consequences:** (a) preflight reports input AND output format support at
first call; (b) spike S1's Python fixture must include one function returning a
dataclass so the error path is exercised and worded before any user hits it;
(c) the type-domain rule — "JSON-shaped data crosses; Python objects don't" —
goes in README-level docs as a one-liner.
