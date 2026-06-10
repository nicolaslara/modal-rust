# Design: `modal-rust bind <app-name>` — typed Rust calls into EXTERNAL Python Modal apps

Status: design study (2026-06-10). No code changed; this doc is the proposal.
Audience: architecture workpad. Companion to the facts already proven in
`crates/modal-rust-sdk/src/ops/invoke.rs` and the p10 codegen-removal history
(`workpads/shim-backend/p10-spec.md`, `workpads/shim-backend/knowledge.md`).

---

## 0. Problem and verified ground truth

modal-rust users need to call Python Modal functions that were deployed
**independently by the Python SDK** — modal-rust never saw their source, and the
source may be unavailable. The call should keep the Python signature (names,
kwargs, defaults) and be typed on the Rust side.

Facts verified in this repo (file:line):

| Fact | Where |
| --- | --- |
| The wire payload is a CBOR `(args, kwargs)` 2-tuple, `DATA_FORMAT_CBOR`; `invoke_cbor_with_deadline(function_id, args, kwargs, deadline)` already takes a generic `K: Serialize` kwargs | `crates/modal-rust-sdk/src/ops/invoke.rs:168-186` (encode at 180-181), codec = `serde_cbor` (`ops/../codec.rs:17`, `Cargo.toml:27`) |
| Every current caller sends **empty kwargs** (`HashMap<String, ()>`) | `crates/modal-rust/src/app.rs:380`, `crates/modal-rust/src/deploy.rs:461-466` |
| A serde struct serialized through `serde_cbor` becomes a CBOR map with string keys → a Python `**kwargs` dict | codec behavior; spike S1 confirms end-to-end |
| `function_from_name(app_name, function_name, env)` resolves a deployed function via `FunctionGet` — but **discards** `FunctionGetResponse.handle_metadata` (which carries the schema) | `crates/modal-rust-sdk/src/ops/function.rs:964-987`; proto `FunctionGetResponse` at `proto/api.proto:2122-2126` |
| `FunctionHandleMetadata.function_schema = 45` — the schema rides on every handle-metadata response | `proto/api.proto:2153` |
| Whole-app enumeration exists: `AppGetByDeploymentName` (name+env → app_id, `proto/api.proto:435-441`), `AppGetLayout` (app_id → `AppLayout{objects, function_ids, class_ids}`, `proto/api.proto:442-448`, `509-513`), `AppGetObjects` (`proto/api.proto:477-484`). `AppLayout.objects[]` are `Object`s whose `function_handle_metadata` (`proto/api.proto:2719-2728`) includes `function_schema`. **None of these is wrapped by the Rust SDK yet** (grep: no `app_get_layout`/`AppGetByDeploymentName` in `crates/modal-rust-sdk/src/`) |
| Schema fidelity is COARSE: `FunctionSchema{schema_type, arguments: ClassParameterSpec[], return_type: GenericPayloadType}` (`proto/api.proto:2302-2310`); `ClassParameterSpec{name, type, has_default, default_oneof, full_type}` (`proto/api.proto:930-943`); `ParameterType = {unspecified, string, int, pickle(unused), bytes, UNKNOWN, list, dict, none, bool}` (`proto/api.proto:216-226`). **No float type.** Comment at `proto/api.proto:933-934`: "Default *values* are only registered for class parameters" — for plain functions we know a param HAS a default, not what it is. Schema may be missing entirely (`proto/api.proto:1792`: "may be missing: client doesn't block deployment if it fails to get it") |
| Output decode hard-fails on non-CBOR | `crates/modal-rust-sdk/src/ops/invoke.rs:66-71` |
| Class methods need `FunctionInput.method_name`; our invoke path always sends `None` | `invoke.rs:215`, `proto/api.proto:2137-2161` (`is_method`, `use_function_id`, `method_handle_metadata`) |

The central design constraint: **the schema is real but coarse**, so the design
must (a) be useful with no overrides at all, (b) make overriding a too-coarse
type trivial and durable across regeneration.

---

## 1. Recommended lifecycle end-to-end

The sqlx-offline pattern, transplanted: a CLI command snapshots the live
contract into a **checked-in data file**; a proc-macro generates the typed
calling surface **from that file at compile time** (hermetic — no network in the
build); a `--check` command keeps CI honest about drift.

### 1.1 Commands

```text
modal-rust bind <app-name>
    [--env <environment>]          # default: profile default ("main")
    [--function <name>]...         # bind a subset; default = all functions in the app
    [--out <path>]                 # default: modal-bind/<app-name>.json
    [--check]                      # NO write: re-fetch, compare, exit 1 on drift (CI)
    [--emit-rs <path>]             # DEBUG escape: dump the .rs the macro would generate
```

`bind` (in `modal-rust-cli`, which already enables the `client` feature —
stance 4 keeps tonic out of the user's authoring build):

1. `AppGetByDeploymentName{name, environment_name}` → `app_id`
   (`proto/api.proto:428-441`). Empty `app_id` ⇒ actionable error (§7).
2. `AppGetLayout{app_id}` → `AppLayout.objects[]`; keep `Object`s with
   `function_handle_metadata`; skip methods/placeholders (`is_method`,
   `use_function_id` — recorded but not bound in v0). Each carries
   `function_name` + `function_schema` (spike S5 confirms population; fallback:
   one `FunctionGet` per tag, whose `handle_metadata` also carries the schema,
   `proto/api.proto:2122-2126`).
3. Normalize each `FunctionSchema` into the snapshot model (§2), compute a
   canonical per-function digest + a file digest.
4. Write `modal-bind/<app>.json`; print a binding report: every function, every
   param, and a **loud list of UNKNOWN-typed params with a copy-pasteable
   override stanza** (§3).

Enumeration decision: **whole-app by default** (one RPC pair gets every schema;
the snapshot is per-app, mirroring how the user thinks about "that deployed
service"), with `--function` to subset. Per-function-only binding was rejected
as the default because it multiplies snapshot files and check commands without
saving anything — `AppGetLayout` is a single cheap read.

### 1.2 Artifact — `modal-bind/text-summarizer.json` (checked in)

```json
{
  "bind_version": 1,
  "app": "text-summarizer",
  "environment": "main",
  "fetched_at": "2026-06-10T17:02:11Z",
  "digest": "sha256:9c1f…",
  "functions": {
    "summarize": {
      "digest": "sha256:41ab…",
      "arguments": [
        { "name": "text",        "type": { "base": "string" },                          "has_default": false },
        { "name": "max_words",   "type": { "base": "int" },                             "has_default": true  },
        { "name": "language",    "type": { "base": "string" },                          "has_default": true  },
        { "name": "temperature", "type": { "base": "unknown" },                         "has_default": true  }
      ],
      "return_type": { "base": "dict" }
    },
    "score_batch": {
      "digest": "sha256:77e0…",
      "arguments": [
        { "name": "items",     "type": { "base": "list", "sub": [{ "base": "string" }] }, "has_default": false },
        { "name": "model",     "type": { "base": "string" },                              "has_default": false },
        { "name": "top_k",     "type": { "base": "int" },                                 "has_default": true  },
        { "name": "normalize", "type": { "base": "bool" },                                "has_default": true  },
        { "name": "weights",   "type": { "base": "dict" },                                "has_default": true  },
        { "name": "tags",      "type": { "base": "list", "sub": [{ "base": "string" }] }, "has_default": true  },
        { "name": "seed",      "type": { "base": "int" },                                 "has_default": true  }
      ],
      "return_type": { "base": "list", "sub": [{ "base": "unknown" }] }
    }
  }
}
```

Digests are over a canonical (sorted-key, whitespace-free) encoding of each
function entry — the comparison unit for `--check` and the runtime drift check.
`fetched_at` is excluded from the digest so re-binding an unchanged app is a
no-op diff.

### 1.3 Compile — `include_bind!` in the user's crate

```rust
// src/lib.rs (user crate; default-light build — no tonic)
modal_rust::include_bind!("modal-bind/text-summarizer.json", {
    // overrides — user-owned source, survives every re-bind (§3):
    fn summarize(temperature: f64) -> Summary;       // UNKNOWN -> f64; dict return -> typed struct
    fn score_batch() -> Vec<ScoreRow>;               // list<unknown> return -> typed rows
});
```

The macro (in `modal-rust-macros`) reads the JSON relative to
`CARGO_MANIFEST_DIR`, applies the overrides, and expands to plain Rust. It also
emits `const _: &str = include_str!("…/modal-bind/text-summarizer.json");` so
cargo rebuilds when the snapshot changes (the same trick sqlx uses for `.sqlx/`
freshness). No network, no build.rs, no OUT_DIR.

Generated shape (the user-visible surface; abridged but realistic):

```rust
pub mod text_summarizer {
    /// Handle to the DEPLOYED app `text-summarizer` (env `main` by default).
    pub struct TextSummarizer { inner: ::modal_rust::bind::BoundApp }

    impl TextSummarizer {
        /// Connect using the snapshot's app name + environment.
        pub async fn connect() -> ::modal_rust::Result<Self> { /* delegates to facade */ }
        /// Same schema, different deployment (staging/prod app names — §6).
        pub async fn connect_to(app_name: &str, env: Option<&str>) -> ::modal_rust::Result<Self> { … }

        /// Python: `def summarize(text: str, max_words: int = 200,
        ///                        language: str = "en", temperature=0.3) -> dict`
        /// Required params are plain fn args; optional ones ride in the opts struct.
        pub async fn summarize(
            &self,
            text: impl Into<String>,
            opts: SummarizeOpts,
        ) -> ::modal_rust::Result<Summary> {
            // kwargs = the struct serialized as a CBOR map; args = EMPTY sequence.
            // None fields are OMITTED (skip_serializing_if) so Python defaults apply.
            self.inner.call_kwargs("summarize", &SummarizeKwargs {
                text: text.into(),
                max_words: opts.max_words,
                language: opts.language,
                temperature: opts.temperature,
            }).await
        }

        #[allow(clippy::too_many_arguments)]   // precedent: macros/src/lib.rs:1639
        pub async fn score_batch(
            &self,
            items: Vec<String>,
            model: impl Into<String>,
            opts: ScoreBatchOpts,
        ) -> ::modal_rust::Result<Vec<ScoreRow>> { … }
    }

    /// Optional (has_default) kwargs for `summarize`. ALWAYS derives Default.
    #[derive(Debug, Clone, Default)]
    pub struct SummarizeOpts {
        pub max_words: Option<i64>,
        pub language: Option<String>,
        pub temperature: Option<f64>,          // overridden UNKNOWN
    }

    #[derive(Debug, Clone, Default)]
    pub struct ScoreBatchOpts {
        pub top_k: Option<i64>,
        pub normalize: Option<bool>,
        pub weights: Option<::std::collections::BTreeMap<String, ::serde_json::Value>>,
        pub tags: Option<Vec<String>>,
        pub seed: Option<i64>,
    }

    // (private) the exact wire kwargs map — field names ARE the Python parameter names
    #[derive(serde::Serialize)]
    struct SummarizeKwargs {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")] max_words: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")] language: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")] temperature: Option<f64>,
    }
}
```

Call sites (no chained builders — plain async fns + a `Default`-able opts
struct, matching the user's stated preference and the existing
`TypedCall`-terminal style):

```rust
let app = text_summarizer::TextSummarizer::connect().await?;

// 1 required + 3 optional — defaults flow through:
let s: Summary = app.summarize("long text…", Default::default()).await?;

// override two kwargs, keep the rest at the Python defaults:
let s = app.summarize("long text…", SummarizeOpts {
    max_words: Some(50),
    temperature: Some(0.0),
    ..Default::default()
}).await?;

// many-kwargs case:
let rows = app.score_batch(items, "small-v2", ScoreBatchOpts {
    top_k: Some(10),
    normalize: Some(true),
    ..Default::default()
}).await?;
```

UNKNOWN **without** an override is still usable — it maps to
`serde_json::Value` (already a default-light facade dep,
`crates/modal-rust/Cargo.toml:54`), e.g. `temperature: Some(json!(0.3))`. The
bind report nags you toward the override; nothing blocks.

### 1.4 Runtime call path (facade + SDK)

New facade module `modal_rust::bind` (stance 4: **real under `client`, erroring
stub otherwise** — the macro never emits `cfg`, exactly like the existing
typed-method delegation, AGENTS.md stances):

- `BoundApp::connect(snapshot_meta) -> Result<BoundApp>` — builds a
  `ModalClient` from `~/.modal.toml`/env (existing client config path).
- `BoundApp::call_kwargs<K: Serialize, R: DeserializeOwned>(name, &kwargs)`:
  1. lazy `function_get_full(app, name, env)` → `(function_id, FunctionSchema)`
     (new SDK op — `function_from_name` extended to return the
     `handle_metadata` it currently drops, `ops/function.rs:964`); cached per
     function for the handle's lifetime.
  2. **drift check**: canonical-digest the live schema, compare to the
     snapshot digest baked into the generated code; mismatch ⇒ `tracing::warn!`
     by default, hard error under `BindMode::Strict` /
     `MODAL_RUST_BIND_STRICT=1` (§5).
  3. `invoke_cbor_with_deadline(function_id, &EMPTY_ARGS, &kwargs, deadline)` —
     **all parameters travel as kwargs** (keeps Python names; positional order
     becomes irrelevant). `EMPTY_ARGS` is an empty *sequence* (`[(); 0]` →
     CBOR `[]`), NOT `()` (serde_cbor encodes `()` as null and Python
     `handler(*None)` would explode) — spike S4 pins this.
  4. decode CBOR output into `R` (`invoke.rs:65-73`); non-CBOR output gets the
     actionable mapping in §7.

---

## 2. Artifact-format decision

| | (a) generated `.rs` checked in | **(b) JSON snapshot + `include_bind!` (CHOSEN)** | (c) build.rs → OUT_DIR |
| --- | --- | --- | --- |
| Hermeticity | hermetic (file is source) | **hermetic** — macro reads a local file; network only in `bind` | hermetic only if build.rs reads a checked-in file (then it's (b) with worse ergonomics); fetch-in-build is disqualified outright |
| Reviewability | poor — large generated-Rust diffs; reviewers approve noise | **good — the diff IS the contract change** ("param `top_k` gained", "type int→string"), small and semantic | none — artifact invisible in review |
| Regeneration UX | re-run + pray nobody hand-edited; merge conflicts in generated code | **re-run `bind`; overrides live in user source so regen never clobbers them** | automatic but opaque |
| Drift between snapshot and edits | the defining failure: users WILL edit the generated `.rs` (fix a type, add a doc) and the next `bind` destroys it — this is precisely the p10 hand-edit hazard | impossible by construction: the data file is never hand-edited (digest-guarded), the overrides are ordinary user Rust | n/a |
| IDE / rust-analyzer | best (plain source) | **good — RA expands proc-macros; same experience as today's `#[function]`/`#[cls]` codegen** | weakest (OUT_DIR `include!` is RA-fragile) |
| Type override | edit the file (and lose it on regen) or never regen | **first-class: override block in the macro invocation** (§3) | awkward (build.rs config) |
| Repo precedent | the thing p10 deleted (generated source as product surface) | **the repo's accepted codegen channel: proc-macro over data** | the repo has no build.rs codegen anywhere |

Decision: **(b)**. The snapshot is *data*, the macro is a *deterministic
projection* — the exact "config-as-data, not generated source" principle the
shim review locked in (`workpads/shim-backend/knowledge.md:262-266`,
"richer values are better represented as typed config data than as generated
Python syntax"). `--emit-rs` keeps a debug escape (precedent: the deleted
shim path's `--dump-shim` escape, and `crates/modal-rust/src/dump.rs`), and is
also the migration path for a user who truly wants to own the source: emit
once, delete the snapshot, maintain by hand.

---

## 3. Type mapping policy + override mechanism

Default mapping (`GenericPayloadType`, recursive via `sub_types`,
`proto/api.proto:2338-2341`):

| Schema type (`proto/api.proto:216-226`) | Rust (param) | Rust (return) | Notes |
| --- | --- | --- | --- |
| `string` | `String` (sig sugar: `impl Into<String>` for required) | `String` | |
| `int` | `i64` | `i64` | Python int is unbounded; >i64 fails encode loudly — documented |
| `bool` | `bool` | `bool` | |
| `bytes` | `serde_bytes::ByteBuf` | `serde_bytes::ByteBuf` | serde_cbor needs `serde_bytes` to emit a CBOR byte string (a bare `Vec<u8>` becomes an int array → Python `list[int]`, wrong). Adds one tiny light dep to the facade. Spike S3 |
| `none` | `()` | `()` | return-only in practice |
| `list` + typed sub | `Vec<T>` | `Vec<T>` | recursive |
| `list` untyped/unknown sub | `Vec<serde_json::Value>` | same | |
| `dict` | `BTreeMap<String, serde_json::Value>` | same | values are untyped in the schema — always coarse unless overridden |
| `UNKNOWN` / `unspecified` | `serde_json::Value` | `serde_json::Value` | the safe lingua franca; serializes through serde_cbor to any CBOR shape; usable immediately, override recommended |
| *(Python `float`)* | — | — | **no float ParameterType exists**; expectation: the Python SDK emits `PARAM_TYPE_UNKNOWN` for `float` annotations (unverifiable offline — spike S2). Practical answer: floats arrive as UNKNOWN and the override (`temperature: f64`) is the supported path; `serde_json::Value` numbers also round-trip without it |
| `has_default = true` | wrap in `Option<T>`, move to the `<Fn>Opts` struct, `#[serde(skip_serializing_if = "Option::is_none")]` | — | omitted kwarg ⇒ Python applies its own default; we never need the default *value* (good, because the proto only records values for class params, `proto/api.proto:933-934`) |
| `pickle` | rejected at bind time with a clear error | — | "currently unused" per proto:220; if ever seen, unsupported |

Override mechanism — **in the macro invocation, not in the JSON** (user-owned
source; regeneration can never clobber it; rustc type-checks it):

```rust
modal_rust::include_bind!("modal-bind/text-summarizer.json", {
    fn summarize(temperature: f64, weights: MyWeights) -> Summary;
    fn score_batch() -> Vec<ScoreRow>;
});
```

Rules:
- An override may name any subset of params and/or the return type; named
  params must exist in the snapshot (compile error with the available names
  otherwise — typo-proof).
- Any `Serialize`/`DeserializeOwned` user type is allowed; the macro only
  substitutes the type, the wire stays "CBOR value under that kwarg name".
- Overriding a *typed* param (e.g. `int` → `u32`) is allowed but emits a
  build-script-style note in `--emit-rs`/bind report; overriding `UNKNOWN` is
  the blessed case.
- A `bind.toml` sidecar was considered and rejected: a second config surface,
  not type-checked, and the macro block already IS Rust the user owns.

Schema-missing functions (`FUNCTION_SCHEMA_UNSPECIFIED` or absent — legal per
`proto/api.proto:1792`): the snapshot records `"schema": null`; the macro
generates **only** an untyped escape hatch
`call_raw(name, kwargs: &serde_json::Value) -> Result<serde_json::Value>`
unless the override block supplies the full signature
(`fn legacy_fn(a: String, b: Option<i64>) -> serde_json::Value;`) — which is
exactly the hand-declared-macro alternative, embedded as the degraded mode of
bind (§8).

---

## 4. Many-kwargs / signature-shape policy

- Required params (`has_default = false`, snapshot order) → positional fn
  arguments. Beyond clap-ish ergonomics this preserves "the Python signature
  reads the same in Rust".
- Optional params → one `<Fn>Opts` struct per function, all `Option<T>`,
  `#[derive(Default)]`, constructed with struct-update syntax
  (`..Default::default()`). No builders, no chaining — per the user's stance;
  matches the existing `#[cls]` handle-method shape
  (`crates/modal-rust-macros/src/lib.rs:1638-1650`).
- Many required params: keep positional + `#[allow(clippy::too_many_arguments)]`
  (existing precedent at `macros/src/lib.rs:1639`). If a function has >7
  required params the bind report suggests an all-struct override form
  (`fn f(#[args] FArgs) -> …`) — a v1.1 nicety, not v0.
- Also generated per function (parity with the existing surface): `spawn_`
  (`spawn_cbor`, `invoke.rs:322`) and `map_` (`map_cbor`, `invoke.rs:412`)
  variants are *deferred to v1.1* — v0 is `.remote()`-equivalent only, to keep
  the spike surface small.

---

## 5. Drift story (the sqlx `prepare --check` analogy)

Three layers, weakest-to-strongest:

1. **Compile-time freshness**: the `include_str!` anchor reties the build to the
   snapshot file; editing/regenerating it recompiles the generated module.
   Hand-editing the JSON breaks the file digest → the macro **fails the build**
   ("snapshot digest mismatch — regenerate with `modal-rust bind`, don't
   hand-edit"). This kills the (a)-style silent-drift failure class.
2. **CI**: `modal-rust bind text-summarizer --check` (no write) re-fetches the
   live app and diffs canonical digests per function; exit 1 with a humane diff
   (`summarize: param 'max_words' type int -> string`). Teams run it next to
   `cargo sqlx prepare --check`. `--check --app-override <staging-name>` lets
   one snapshot be validated against multiple deployments.
3. **Runtime**: `BoundApp` lazily resolves each function (`function_get_full`)
   and compares the live schema digest against the snapshot digest baked into
   the generated code. Default `BindMode::Warn` (`tracing::warn!` once per
   function); `Strict` (env `MODAL_RUST_BIND_STRICT=1` or
   `connect_with(BindMode::Strict)`) errors before sending bytes; `Off`
   available for hot paths. Resolution is one extra read RPC already on the
   call path (`from_name` happens anyway, `deploy.rs:459`), so the check is
   ~free.

Snapshot versioning: `bind_version` gates the macro's parser (unknown version ⇒
compile error "upgrade modal-rust"); `fetched_at` is informational and
digest-excluded.

---

## 6. Auth / environments / error UX

- `bind` runs in the CLI (client feature already on) and uses the same
  credential path as `run`/`deploy`/`call`; `modal-rust doctor` already checks
  credentials offline (`crates/modal-rust-cli/src/doctor.rs`). Missing/invalid
  `~/.modal.toml` ⇒ the existing actionable auth error, plus a bind-specific
  hint ("bind reads the DEPLOYED app's schema — it needs credentials for the
  workspace that owns '<app>'").
- Environment is a first-class bind input (`--env`), recorded in the snapshot,
  and overridable at runtime: `connect()` uses the snapshot's
  `app`/`environment`; `connect_to("text-summarizer-staging", Some("staging"))`
  re-targets **the same bound schema** at a different deployment — names are
  runtime data, the schema is compile-time data. Drift between the snapshot and
  the re-targeted deployment is caught by layer 3 (and by
  `--check --app-override` in CI).
- App not deployed: `AppGetByDeploymentName` returns empty `app_id`
  (`proto/api.proto:436` "Null when App with requested name is not deployed");
  error: `app 'x' is not deployed in environment 'main' (it may exist in
  another environment — try --env, or 'modal app list')`, mentioning
  `previous_app_id` ("was recently stopped") when populated.

---

## 7. Failure-mode table

| # | Failure | When | Behavior |
| --- | --- | --- | --- |
| F1 | No/invalid Modal credentials | bind / connect | existing client auth error + bind hint; `doctor` preflight |
| F2 | App name not deployed in env | bind / connect | actionable error (§6), distinguishes "recently stopped" |
| F3 | Function present at bind, deleted/renamed later | runtime `from_name` | map `FunctionGet` empty-id error (`ops/function.rs:982-985`) to "bound function 'summarize' no longer exists on 'text-summarizer' — re-run `modal-rust bind`" |
| F4 | Schema missing server-side | bind | snapshot `"schema": null`; only `call_raw` generated; report says "supply a signature override for a typed surface" |
| F5 | UNKNOWN param, no override | bind / compile | not an error — `serde_json::Value`; bind report prints the ready-to-paste override stanza |
| F6 | Hand-edited snapshot | compile | macro digest check fails the build with "regenerate, don't hand-edit" |
| F7 | Live schema drifted from snapshot | runtime | Warn (default) / Strict error / Off (§5); `bind --check` catches it in CI first |
| F8 | Python returns non-CBOR (pickle) output | runtime decode | `invoke.rs:66` error, re-worded: "the deployed app returned PICKLE — its `modal` version may predate CBOR output negotiation; see spike S1"; raw escape `call_raw_bytes` returns the `Invocation` untouched |
| F9 | Oversized args/result (blob path) | runtime | existing explicit inline-only errors (`invoke.rs:287-293`); pre-existing SDK limitation, not bind-specific |
| F10 | int overflow (Python int > i64) | encode/decode | loud serde error; documented in the mapping table |
| F11 | Class methods (`is_method`) | bind | v0: listed in the report as "not bindable yet (class method — needs `FunctionInput.method_name`)"; snapshot records them under `"methods"` for forward-compat |
| F12 | `bind_version` from a newer CLI | compile | macro errors "snapshot v2 needs a newer modal-rust" |

---

## 8. Why this is NOT the p10 mistake (the anti-codegen history, head-on)

What p10 actually deleted (`workpads/shim-backend/p10-spec.md`,
`knowledge.md:296-359`): **generated Python source** — per-project `.py` shims
rendered by hand-rolled string `.replace()` templates, shelling out to the
`modal` CLI. The recorded lessons, mapped onto bind:

| p10 lesson | The shim path | `bind` |
| --- | --- | --- |
| "Config is data, not generated source" (`knowledge.md:262-266`) | config baked into Python text | the artifact IS data (JSON); the only "source" is produced inside a proc-macro at compile time — the same channel as today's `#[function]`/`#[cls]` expansion, which the repo fully embraces |
| Generated source is checked by nothing | Python: no type-checker, quote-escaping hazards (cf. the wrapper.py `include_str!` refactor, commit c11f579) | generated Rust goes through rustc + clippy on every build; overrides are type-checked user source |
| Per-project generated files become an accidental product surface users hand-edit | `.modal-rust/generated/*.py` | the checked-in file is a digest-guarded snapshot users *cannot* usefully hand-edit (F6); the editable surface is ordinary Rust (the override block) |
| "Why must the CLI read config pre-build? isn't it dynamic?" — derive from the live system, don't pre-render it (`knowledge.md:336-345`) | static templates pretending to know the dynamic config | bind's input is **the deployed app itself over gRPC** — ground truth that does not live in any Rust source, so *something* must materialize it; the alternatives (hand-declared externs) don't remove the contract copy, they just make a human type it with no verification |
| The pivot's destination: drive Modal programmatically | shell-out to `modal` | bind is *more* programmatic surface: two new read RPCs on the same first-party client |

The honest residual risk: a point-in-time snapshot is still a copy, and copies
drift. That is inherent to "call something whose source you don't have" — the
design spends its complexity budget exactly there (three drift layers, §5)
instead of pretending the copy away.

**Coexistence with hand-declared macros**: the hand-declared alternative
(`extern_function!(app="x", fn summarize(text: String, …) -> Summary)`) is not a
competing feature — it is `include_bind!` with a null snapshot (F4's degraded
mode) or, inverted, the snapshot can *validate* hand declarations: a future
`include_bind!(…, declare)` mode type-checks the user's signatures against the
snapshot and errors on mismatch (compile-time `--check`). Both fall out of the
same macro + snapshot model; neither needs separate machinery. The centerpiece
remains bind-generates.

---

## 9. Implementation map

| Piece | Where | What |
| --- | --- | --- |
| `app_get_by_deployment_name` op | `crates/modal-rust-sdk/src/ops/app.rs` (new fn next to `app_get_or_create_id`, :97) | pure read, retried like the rest |
| `app_get_layout` op | same | returns `AppLayout`; helper to extract `(tag, FunctionHandleMetadata)` pairs |
| `function_get_full` | `crates/modal-rust-sdk/src/ops/function.rs:964` — extend (keep `function_from_name` as the thin wrapper) | return `(function_id, Option<FunctionSchema>)` instead of dropping `handle_metadata` |
| Snapshot model + canonical digest | new tiny crate `modal-rust-bind-schema` (serde + serde_json + sha2 only — **light**) | shared by the CLI (writes), the macro (reads), and the facade (runtime digest compare); avoids the macro depending on the tonic-heavy SDK |
| `bind` subcommand | `crates/modal-rust-cli/src/main.rs` (clap) + new `bind.rs` | fetch → normalize → report → write / `--check` diff / `--emit-rs` |
| `include_bind!` proc-macro | `crates/modal-rust-macros/src/lib.rs` (+ `serde_json` dep, currently absent — `macros/Cargo.toml` has only syn/quote/proc-macro2/proc-macro-crate) | parse JSON + override block; generate module; `include_str!` anchor; digest check |
| `modal_rust::bind` runtime | `crates/modal-rust/src/bind.rs` | `BoundApp`, `call_kwargs`, `BindMode`, drift warn/strict; **real under `client`, erroring stub at default features** (stance 4; same pattern as the existing remote surface) |
| Empty-args constant | `bind.rs` | `[(); 0]`-shaped empty CBOR sequence (spike S4) |
| Gates | per AGENTS.md verification | facade builds BOTH ways (light + `--features client`); README example only after an external-crate proof (per repo memory: prove from a /tmp crate) |

Build order: spikes (below) → SDK ops → bind-schema crate → CLI `bind` →
macro → facade runtime → `--check` → external-crate live proof.

---

## 10. Open spikes (cheap, ordered)

- **S1 — CBOR-in/CBOR-out against a vanilla Python app** (the gating spike).
  Deploy a 5-line Python modal app (`def echo_kwargs(**kw) -> dict`), invoke
  from Rust with `args=[]`, `kwargs={…}` covering str/int/bool/float/list/dict;
  assert the decoded result AND that the output `data_format` is CBOR. If
  output comes back PICKLE: check whether `FunctionInput`'s format negotiation
  (`supported_input_formats`/`supported_output_formats`,
  `proto/api.proto:2159-2160`) lets us request CBOR; otherwise F8 is the v0
  boundary and this design's runtime leg needs a JSON-over-web-endpoint
  fallback discussion.
- **S2 — what does the Python SDK emit for `float` annotations?** Expect
  `PARAM_TYPE_UNKNOWN` (no float ParameterType exists). Verify via
  `FunctionGet` on the S1 app with an annotated `x: float = 0.3` param.
- **S3 — bytes**: confirm `serde_bytes::ByteBuf` → CBOR byte string → Python
  `bytes` (and the reverse).
- **S4 — empty-args shape**: confirm Python's container runtime accepts
  `([], {…})` (and whether `((), {…})`/null differ). Trivially part of S1.
- **S5 — does `AppGetLayout` populate `function_schema`** inside each object's
  handle metadata, or only `FunctionGet` does? Decides whether enumeration is 2
  RPCs total or 1 + N.
- **S6 — `has_default` on plain functions**: confirm the schema sets it for
  defaulted params of ordinary `@app.function`s (the default-*value* fields are
  class-only, but the flag should be set; if not, everything degrades to
  required — ugly but visible in the bind report).
- **S7 — schema presence in the wild**: bind a real older-modal-version app to
  see how common `schema: null` (F4) is; informs how loud the degraded mode
  should be.

---

## 11. Ratified decisions & author workflow (2026-06-10 design review)

**Sequencing: bind ships AFTER `#[import]`** (external-py-fns-design-macro.md).
The shared substrate + hand-declared macro land first; bind layers on, starting
with `--check`-style *validation* of hand declarations in CI, and full generation
only if hand-typing signatures for large apps becomes a real complaint. Both
surfaces must stay call-site-identical: bind generates exactly the `#[import]`
shape (plain fns + `Opts` struct), so authors can mix freely — bind an app
wholesale, hand-declare one hot function precisely.

**Wire-shape correction (supersedes §1.4):** adopt the macro doc's ratified args
rule — required params positional in the CBOR args tuple, optionals as kwargs
with `None` omitted — NOT kwargs-only. This keeps bind-generated and
hand-declared calls byte-identical on the wire and preserves positional-only
Python params; keyword-only-required is the shared documented v0 gap.

**Ergonomics caveat:** the v0 call shape (`&app` first arg + `()`/`Opts` tail) is
accepted for v0 only — the user is explicitly not fully satisfied with it.
Revisit before v1; bind must regenerate cleanly into whatever v1 surface wins.

**Author workflow (the contract):**

```bash
$ modal-rust bind text-embedder        # gRPC against the DEPLOYED app; no Python source
  wrote modal-bind/text-embedder.json  (3 functions, schema digest a91f…)
$ git add modal-bind/
```

```rust
modal_rust::include_bind!("modal-bind/text-embedder.json", {
    fn summarize(temperature: f64) -> Summary;   // override: schema said UNKNOWN
});
// → module with embed / embed_spawn / EmbedOpts — call sites identical to #[import]
```

CI: `modal-rust bind --check` re-fetches and diffs digests so a Python-side
redeploy that changes a signature fails the build, not production. Un-overridden
`UNKNOWN` params surface as `serde_json::Value` — callable on day one, precision
added only where touched.

## 12. Pickle boundary (ratified)

Shared analysis lives in external-py-fns-design-macro.md §9 (three scenarios:
pickle-only inputs → preflight error from `supported_input_formats`; non-CBOR-able
*outputs* → v0 hard error with diagnostic, Python-side plain-data adapter as the
blessed pattern, opt-in plain-data serde-pickle decode at most; foreign pickle
blobs → opaque bytes, never decode or re-forward to Python consumers).

Bind-specific consequences:

- **Bind-time preflight:** the snapshot already carries each function's
  `supported_input_formats`/`supported_output_formats` — `bind` warns at snapshot
  time (not first call) when a function is pickle-only, naming the offending
  function and the modal-version remedy.
- **UNKNOWN return types:** bind marks functions whose schema `return_type` is
  UNKNOWN with a "may return non-CBOR-able data (Python objects don't cross;
  JSON-shaped data does)" warning in the generated docs/comments and the bind
  report.
- **Spike S1 fixture** must include a dataclass-returning function so the PICKLE
  output error path (F8) is exercised and worded before any user hits it.
