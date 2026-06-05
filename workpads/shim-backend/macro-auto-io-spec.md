# SPEC: `#[modal_rust::function]` plain-signature ergonomics (auto I/O + typed `app.<fn>(..)`)

Build-ready, merged spec. Lets the user write `#[function] fn add(a: i64, b: i64) -> anyhow::Result<i64> { Ok(a + b) }`
and call `app.add(2, 3).remote().await? == 5`, never naming a type — while the fully-explicit single-struct form
(`fn add(input: AddInput) -> Result<AddOutput>`, `examples/add` + `examples/add-macro`) stays **byte-identical**.

Anchors (absolute):
- macro: `/Users/nicolas/devel/modal-rust/crates/modal-rust-macros/src/lib.rs`
- runtime (FROZEN): `/Users/nicolas/devel/modal-rust/crates/modal-rust-runtime/src/lib.rs`
  (`typed!`:188-215, `codec`:158-177, envelope splice:687-704, `Registration`:267)
- facade: `/Users/nicolas/devel/modal-rust/crates/modal-rust/src/{function.rs,app.rs,lib.rs}`
  (`Function::{local,remote,spawn,map}`:42/72/95/122, `FunctionCall::get`:192, `App::function`:483, re-exports:56-79)
- examples: `/Users/nicolas/devel/modal-rust/examples/{add,add-macro}/src/lib.rs`
- frozen contract: `/Users/nicolas/devel/modal-rust/workpads/architecture/boundaries.md` §2/§3

## 0. The FROZEN surface (audited — zero changes)

- `crates/modal-rust-runtime/src/lib.rs`: **no edits.** `typed!` (`:188-215`) takes ONE `$f:path`, does
  `let arg = codec::decode(input)?; match $f(arg) { Ok=>encode, Err=>autoref-specialized RunnerError }`. It calls `$f`
  with exactly ONE decoded arg. `HandlerFn = fn(&[u8])->Result<Vec<u8>,RunnerError>`, `Registration{name,handler,config}`
  (`:267`), `FunctionConfig`, `Registry::from_inventory`, `run_cli`, the five `RunnerError` kinds, and the success
  envelope `serde_json::json!({"ok":true,"value":value})` (`:699`) are untouched.
- **Wire (FROZEN, both modes):** INPUT is ONE named JSON object (never a positional array, boundaries.md §3); OUTPUT is
  `{"ok":true,"value":<encode(return)>}`. For `-> anyhow::Result<i64>` the `value` is a bare `42` — valid, `value` is any
  `Serialize` (`codec::encode`:174). Five error kinds preserved verbatim.
- `App::function(name) -> Function` (string-keyed, `app.rs:483`) and `Function::{local,remote,spawn,map}` stay. The new
  typed `app.add(..)` is **pure sugar** that builds the generated input and calls this exact path. `retry_transient`,
  build/deploy boundary, image/upload, secrets/volumes, cache, `FunctionConfig` (gpu/timeout/cache/secrets/volumes on the
  decorator), `config_for` keyed on the entrypoint name — all unchanged and still apply (they live below `Function`).
- Bare `#[modal_rust::function]` and every existing decorator arg (`name/gpu/timeout/cache/secrets/volumes`,
  `lib.rs:96-160`) keep working; signature style is orthogonal to them. The `async fn` reject (`lib.rs:170-181`) stays.

## 1. Signature-style classifier (replaces the `arg_count != 1` gate at `macros/src/lib.rs:187-202`)

Today the macro hard-rejects `func.sig.inputs.len() != 1` (`lib.rs:188-202`). Replace that single gate with a classifier
that returns one of two modes. Reject any `Receiver` (`self`) up front (free `fn` only). Let
`params: Vec<&PatType>` = the typed args.

**Mode A — EXPLICIT (current behavior, NO generation, byte-identical):** selected iff ALL of:
- `params.len() == 1`, AND
- the sole param's type is `syn::Type::Path` with `PathArguments::None` (no generic args) AND its last path-segment
  ident is **not** in the scalar denylist below.

Scalar denylist (forces Mode B even as a single param — satisfies the maintainer's "single primitive/standalone param =>
generate"):
`i8 i16 i32 i64 i128 isize u8 u16 u32 u64 u128 usize f32 f64 bool char str String`.

A proc-macro cannot resolve types, so the rule is purely syntactic: a single param is Mode A iff its `Type::Path`
last-segment ident is a non-denylisted bare path (`AddInput`, `crate::AddInput`, `mymod::Req`). Anything else is Mode B:
non-`Type::Path` (`&T`, `(A,B)`, `[T;N]`), a path WITH generic args (`Vec<u8>`, `Option<i64>`, `HashMap<..>`), or a
denylisted scalar. Rationale: a single `AddInput` is the existing `examples/add`/`examples/add-macro` form and MUST stay
byte-identical (proven by `macro_path_byte_identical_to_manual`, `examples/add-macro/src/lib.rs:180-195`); a single
`n: i64` is "standalone" and must generate.

**Mode B — GENERATE (plain signature):** everything else — `params.len() >= 2`, OR one scalar/non-path/generic param,
OR `params.len() == 0` (no-arg fn → empty generated input deserializing from `{}`, mirroring the `{}`-shaped inputs the
runtime already accepts).

Mode-B param requirements (else clear `compile_error!`):
- Each param is a plain `ident: Type` (`Pat::Ident`, no `mut`, no destructuring, no attrs). A destructured pattern
  (`(a,b): (i64,i64)`) → `compile_error!("name each parameter so its name can become an input field")`.
- **Owned only.** A reference/borrowed param (`&str`, `&[u8]`, any `Type::Reference`) →
  `compile_error!("plain #[function] params must be owned; use String / Vec<u8>")`. v0 round-trips owned params only.
- **No generics/lifetimes/where-clauses** on the handler in Mode B (the `Input`/shim can't be monomorphized generically)
  → `compile_error!`. Mode A is unaffected (the existing path never supported them).

Unchanged in BOTH modes: the decorator-arg parser (`:96-160`), the `async` reject (`:170-181`), `FunctionConfig`
emission (`:217-261`), and `entry_name`/`name=` handling (`:162-165`).

## 2. Resolving the crate-name contradiction (LOAD-BEARING — both source notes glossed this)

`examples/add-macro/src/lib.rs:20` does `extern crate modal_rust_macros as modal_rust;`. So **inside a macro-using
crate, the path `::modal_rust` aliases the MACRO crate, NOT the facade.** A macro that emits `::modal_rust::App` /
`::modal_rust::TypedCall` would FAIL to resolve there. Therefore:

**Decision:** the macro emits facade references via the facade's REAL extern name `::modal_rust`... **no.** Because the
alias shadows it, the macro must NOT hard-code `::modal_rust`. Two clean options; this spec mandates (B):

- (A) Re-export the facade types the macro needs FROM the runtime crate (which the macro already references by its real
  name `::modal_rust_runtime`), then emit `::modal_rust_runtime::App` etc. Rejected: `App`/`TypedCall` live in the
  facade and depend on the SDK; pulling them into `modal-rust-runtime` inverts the crate layering (runtime is the
  no-SDK leaf).
- **(B) The macro emits the FACADE path through a macro-stable indirection the user crate controls.** The macro emits
  `$crate`-style absolute paths to the runtime (`::modal_rust_runtime`, `::inventory`) UNCHANGED, and for the facade it
  emits **`::modal_rust_facade::…`** — a name the user crate guarantees by adding
  `modal_rust_facade = { path = "...", package = "modal-rust" }` to its `Cargo.toml` (a renamed dependency; Cargo's
  `package = "..."` lets the same crate be referred to by an unshadowed extern name). The canonical `examples/add-macro`
  keeps `extern crate modal_rust_macros as modal_rust;` (so `#[modal_rust::function]` still spells the attribute) AND
  adds the `modal_rust_facade` rename — no collision.

This mirrors the runtime/inventory dependency contract already documented at `crates/modal-rust/src/lib.rs:37-48`
(macro-using crates already carry direct `modal-rust-runtime` + `inventory` deps because the macro emits absolute paths
to them). Mode B simply adds two more emitted-path requirements — `serde` (for the derives, §3) and `modal_rust_facade`
(for the typed methods, §5) — both documented in the macro crate docs and added to `examples/add-macro/Cargo.toml`.

> Implementation note: if a project does NOT want the `modal-rust` rename, it can instead drop the
> `extern crate ... as modal_rust;` alias and spell the attribute fully as `#[modal_rust_macros::function]`; then
> `::modal_rust` is unshadowed and the facade emission can target `::modal_rust` directly. The canonical example keeps the
> alias + adds the rename (option B) so `#[modal_rust::function]` stays spellable AND `app.add(..)` resolves.

## 3. Generated I/O types (Mode B only): a per-fn module `pub mod <fn> { Input, Output }`

**Convention (resolves the note-1/note-2 naming agreement and the maintainer's "pick the cleanest"):** a per-fn module
named after the fn, holding `Input` + `Output` — `add::Input`, `add::Output`. Chosen over `AddInput`/`AddOutput`
because: (a) collision-safe by construction (the fn name is already unique; a duplicate is already a duplicate-fn
error), no PascalCase-mangling ambiguity (`add_gpu` → `add_gpu::Input`, never `Add_gpuInput`); (b) it gives the
maintainer's exact requested explicit spelling `app.function("add").remote(add::Input { a: 2, b: 3 })` (constraint #3);
(c) reads like Modal Python's module-scoped function object.

Mode B emits, alongside the unchanged `#func`:

```rust
#[allow(non_snake_case)]
pub mod #fn_ident {
    use super::*;                                   // param types (i64, user types) resolve from the fn's own scope
    #[derive(::serde::Serialize, ::serde::Deserialize)]
    pub struct Input {
        pub a: i64,                                 // one pub field per param: field name = param ident, type = param type
        pub b: i64,
    }
    pub type Output = i64;                          // = the inner Ok type (see §4)
}
```

Notes:
- **`Serialize + Deserialize` both** (constraint #1): `Deserialize` is consumed on the wire by the wrapper
  (`codec::decode`, `runtime:168`); `Serialize` is consumed at the call site when `Function::remote/local` serialize the
  `In` (`function.rs:48,77`). Same symmetric-derive rationale already documented for hand-written `AddInput`
  (`examples/add/src/lib.rs:19-30`).
- Paths are absolute `::serde::…`, matching the macro's existing absolute-path style (`::modal_rust_runtime`,
  `::inventory`). Every macro-using crate already depends on `serde` with `derive` (`examples/add-macro/Cargo.toml:23`),
  so this adds no new ecosystem dep — only a documented emitted-path requirement (joins runtime/inventory at
  `crates/modal-rust/src/lib.rs:37-48`).
- `use super::*;` lets param types written in the fn's scope resolve inside the module.
- `pub mod` so `add::Input` is nameable from the user crate and from typed-method callers (constraint #3).
- Empty-params: `pub struct Input {}` — deserializes from `{}`.
- **Naming caveat (documented, non-blocking):** `fn add` → `pub mod add`; if the user also declares `mod add` they
  collide — an ordinary Rust name clash with a clear compiler error. `AddInput` carries the identical risk.

## 4. Return type → `Output` (Mode B)

From `func.sig.output`:
- `ReturnType::Type(_, ty)` with `ty` a `Result<T,E>` / `anyhow::Result<T>`: `Output = T` (the FIRST generic type arg of
  the last path segment whose ident is `Result`). `anyhow::Result<T>` → `T`; `Result<T,E>` → `T`. This `T` is what the
  success envelope carries as `value` (`{"ok":true,"value":<T as Serialize>}`, `runtime:699`).
- `ReturnType::Default` (`-> ()` implied) or any non-`Result`: not a valid handler — but the EXISTING path already
  requires a `Result` (the shim's body returns the fn's return type verbatim and `typed!` matches `Ok/Err`,
  `runtime:195-211`), so a non-`Result` return is already a compile error inside the wrapper. No new diagnostic needed.

`pub type Output = T;` (e.g. `add::Output = i64`). The user never names it — it is the `Out` the typed method binds (§5).

## 5. The generated `HandlerFn` wrapper (Mode B): a private SPREAD shim wrapped by the FROZEN `typed!`

`typed!` only calls `$f(arg)` with ONE decoded arg. For a spread call `f(input.a, input.b)` the macro emits a **private
shim** that takes the generated `Input` and spreads its fields, then registers the SHIM via the unchanged `typed!`. This
is exactly the "private shim that destructures and calls `f(a,b)`" reserved at the runtime docs (`runtime:56-63`) and
boundaries.md §3 — now implemented.

```rust
#[doc(hidden)]
fn #shim_ident(__in: #fn_ident::Input) #orig_output {     // #orig_output = func.sig.output copied VERBATIM
    #fn_ident( __in.a , __in.b )                           // SPREAD: one field per param, in declared order
}
```

Copy `func.sig.output` token-for-token as the shim's return type (do NOT reconstruct `Result`/`E`): this keeps `E`
(Serialize-or-not) intact so the autoref specialization in `typed!` (`runtime:204-209,236-253`) still selects the right
`details` path (`anyhow::Error` → `details=null`; `Serialize` error → populated). Field/spread order = param declaration
order — deterministic.

Register the SHIM (everything else byte-identical to today, `macros/src/lib.rs:246-262`):

```rust
::inventory::submit! {
    ::modal_rust_runtime::Registration {
        name: #entry_name,
        handler: ::modal_rust_runtime::typed!(#shim_ident),    // SHIM, not the user fn
        config: ::modal_rust_runtime::FunctionConfig { /* gpu/timeout/cache/secrets/volumes — UNCHANGED */ },
    }
}
```

What `typed!(#shim_ident)` expands to, monomorphized for `In = #fn_ident::Input`:
1. `codec::decode(input)?` decodes the named JSON object `{"a":2,"b":3}` into `Input` (Deserialize). Failure →
   `RunnerError::Decode`. ✓
2. `match #shim_ident(arg)` → shim spreads to `add(arg.a, arg.b)`. `Ok(out)` → `codec::encode(&out)` → `value`
   (`encode` error → `RunnerError::Encode`). `Err(e)` → autoref-specialized `RunnerError::Function`. ✓ all five kinds.

**Mode A is unchanged:** it still emits `typed!(#fn_ident)` directly (`macros/src/lib.rs:252`) — no shim, no module, no
typed-method module-types. Byte-identical to today.

### Wire confirmation (both modes)
- INPUT: one named JSON object. Mode A: the user `In`'s fields. Mode B: the generated `Input`'s fields (= param names).
  Both `{ "<param>": <val>, … }` — never a positional array. ✓
- OUTPUT: `{"ok":true,"value":<encode(return)>}`. For `-> anyhow::Result<i64>`, `value` is `42`. ✓
- `HandlerFn`/`Registration`/`typed!`/`Registry::from_inventory`/dispatch byte-identical in shape — only the REGISTERED
  fn is the generated shim instead of the user fn. ✓

## 6. Typed positional `App` methods — per-fn extension trait (orphan rule) + a facade `TypedCall` handle

`App` is foreign to the user crate, so methods are added via a **generated extension trait, one per function**, named
`<Pascal>Call`, implemented `for ::modal_rust_facade::App` (the unshadowed facade path from §2). Per-fn (not per-crate)
because each fn has a distinct positional arg list/arity; one trait per fn keeps each method cleanly monomorphic and
makes coherence trivial (each `impl … for App` implements a distinct LOCAL trait → never a conflicting-impl error even
with many `#[function]`s). The trait is `pub`; the orphan rule is satisfied because the trait is local to the user crate
(impl-ing a local trait for a foreign type is always legal).

The method is POSITIONAL sugar over the string-keyed path. For `#[function] fn add(a: i64, b: i64) -> anyhow::Result<i64>`
(entrypoint `"add"`):

```rust
pub trait AddCall {
    fn add(&self, a: i64, b: i64)
        -> ::modal_rust_facade::TypedCall<'_, #fn_ident::Input, #fn_ident::Output>;
}
impl AddCall for ::modal_rust_facade::App {
    fn add(&self, a: i64, b: i64)
        -> ::modal_rust_facade::TypedCall<'_, #fn_ident::Input, #fn_ident::Output> {
        ::modal_rust_facade::TypedCall::new(self, #entry_name, #fn_ident::Input { a, b })
    }
}
```

- `&self` matches `App::function(&self)` (`app.rs:483`); the handle borrows `&App` (`'_`), like `Function<'a>`.
- The method returns a BUILDER `TypedCall`, not a result, so the same `app.add(2,3)` chains into
  `.local()? / .remote().await? / .spawn().await? / .map(..).await?`.
- `In = add::Input` (serializes to the frozen `{"a":2,"b":3}`); `Out = add::Output` (= `i64`; decodes from
  `{"value":5}` → `5`). The user names NO type.
- `#entry_name` (NOT the fn ident) keys the registry, so `#[function(name="plus")] fn add(..)` still produces
  `app.add(..)`/`add::Input` but dispatches under `"plus"` (§9).

**The `use` the user needs (document in macro crate docs + the example):** the per-fn trait must be in scope. The macro
also emits, per crate, a `pub use` aggregation so one import suffices — each fn contributes `pub use self::<Pascal>Call;`
to a generated **`pub mod modal_prelude`** in the user crate, so:

```rust
use example_add_macro::modal_prelude::*;   // brings app.add(..), app.add_gpu(..), … into scope (ONE use)
// (or per-fn: `use example_add_macro::AddCall;`)
```

Because a proc-macro cannot append to a prior `mod`, `modal_prelude` is generated by emitting each trait into it via the
per-fn expansion writing `pub use crate::AddCall;` inside a freshly-declared `pub mod modal_prelude { … }` is NOT
possible (duplicate module). Concrete mechanism: each fn emits its trait at the fn's module level AND the macro relies on
the user doing a glob `use example_add_macro::*;` to pull every `<Pascal>Call` at once. **Mandated:** document BOTH the
single glob (`use example_add_macro::*;`) and the per-fn (`use example_add_macro::AddCall;`) — the glob is the one-`use`
ergonomic path; no generated `modal_prelude` module is required (drop it to avoid the accretion problem).

### `TypedCall<'a, In, Out>` — NEW facade handle (added ONCE to `crates/modal-rust/src/function.rs`)

Thin generic that owns the pre-built `In` and forwards to the existing generic `Function` methods, pinning the type
params so callers never name a type. Constructor takes `(&App, &'static str, In)` (note-2 shape — chosen over note-1's
`(Function, In)` because the macro knows `name` as a `&'static str` at compile time and `App::function` takes `&str`, so
no allocation and the handle stays construct-cheap):

```rust
pub struct TypedCall<'a, In, Out> {
    app: &'a crate::App,
    name: &'static str,                 // frozen entrypoint key (registry name)
    input: In,                          // generated named-input, already built from the positional args
    _out: core::marker::PhantomData<Out>,
}
impl<'a, In, Out> TypedCall<'a, In, Out>
where In: serde::Serialize, Out: serde::de::DeserializeOwned {
    pub fn new(app: &'a crate::App, name: &'static str, input: In) -> Self {
        TypedCall { app, name, input, _out: core::marker::PhantomData }
    }
    pub fn local(self) -> crate::Result<Out> {
        self.app.function(self.name).local::<In, Out>(self.input)                    // wraps function.rs:42
    }
    pub async fn remote(self) -> crate::Result<Out> {
        self.app.function(self.name).remote::<In, Out>(self.input).await             // wraps function.rs:72
    }
    pub async fn spawn(self) -> crate::Result<crate::FunctionCall<'a>> {
        self.app.function(self.name).spawn::<In>(self.input).await                   // wraps function.rs:95; then .get::<Out>(None)
    }
    pub async fn map<I>(self, inputs: I) -> crate::Result<Vec<Out>>
    where I: IntoIterator<Item = In> {
        self.app.function(self.name).map::<In, Out, I>(inputs).await                 // wraps function.rs:122; `self.input` discarded
    }
}
```

| chain | delegates to | returns |
| --- | --- | --- |
| `app.add(2,3).local()?` | `Function::local` (`function.rs:42`) | `Result<Out>` (here `i64`) |
| `app.add(2,3).remote().await?` | `Function::remote` (`function.rs:72`) | `Result<Out>` |
| `app.add(2,3).spawn().await?` | `Function::spawn` (`function.rs:95`) | `Result<FunctionCall<'a>>` → `.get::<Out>(None).await` (`function.rs:192`) |
| `app.add(0,0).map(iter)` | `Function::map` (`function.rs:122`) | `Result<Vec<Out>>` |

This is pure sugar: it builds `add::Input{a,b}` and calls the existing `Function` methods, which already do
serialize → frozen path → decode. `retry_transient`/build-boundary/config all live BELOW `Function`/`App::remote_invoke`
and are unchanged. **`.map` note:** the leading `app.add(a,b)` only fixes the entrypoint+types; its single input is
discarded — `map` overrides with the iterator of `add::Input`. (Optional later sugar — a zero-arg `add_calls()`
accessor returning a `TypedFn` so map reads `app.add_calls().map([...])` without a throwaway `add(0,0)` — is NOT required
for v0; positional `.map` on `TypedCall` is sufficient.)

**Explicit generated-input path still works (constraint #3), no extra code:** because `add::Input` derives serde and
`Function::remote<In,Out>` is generic over `In: Serialize`,
`app.function("add").remote(add::Input { a: 2, b: 3 }).await?` serializes to the SAME `{"a":2,"b":3}`, hits the SAME
wrapper/config path, decodes `{"value":5}` → `5`. `.local`/`.spawn`/`.map` likewise.

## 7. Mode A typed methods (optional, additive)

For the explicit single-struct arm the SAME trait/impl shape applies with the user struct as `In` and NO generated
module: `fn add(&self, input: AddInput) -> TypedCall<'_, AddInput, AddOutput>` /
`TypedCall::new(self, "add", input)`. This gives `examples/add-macro`'s explicit `add(AddInput{..})` a typed
`app.add(AddInput{..}).remote()` chain WITHOUT changing the registered handler (still `typed!(add)`). Emitting Mode-A
typed methods is OPTIONAL for v0 (the maintainer's hard requirement is only that the explicit FORM keeps compiling +
the string-keyed path works; both hold without it). If emitted, the `Out` is the explicit return's inner `Ok` type via
§4. `examples/add` (manual `modal_registry()`, no macro) is entirely untouched.

## 8. Files that change (net deltas)

1. **Facade** `crates/modal-rust/src/function.rs`: add `TypedCall<'a, In, Out>` + `new/local/remote/spawn/map`
   delegating verbatim to `App::function(name).{local,remote,spawn,map}`. No change to `Function`/`FunctionCall`.
2. **Facade** `crates/modal-rust/src/lib.rs:76`: `pub use function::{Function, FunctionCall, TypedCall};`.
3. **Macro** `crates/modal-rust-macros/src/lib.rs`: replace the `arg_count != 1` gate (`:188-202`) with the §1
   classifier. Mode A → unchanged emission (`typed!(#fn_ident)`). Mode B → additionally emit `pub mod #fn_ident {Input,
   Output}` (§3) + the private spread shim (§5) + register `typed!(#shim_ident)` + the `<Pascal>Call` trait & impl for
   `::modal_rust_facade::App` (§6). Update the macro crate docs (`:56-63`) to document: Mode B requires the user crate
   to also depend on `modal-rust` renamed as `modal_rust_facade` (§2) + already-present `serde` derive; document the
   `use <crate>::*;` (or per-fn `use <crate>::<Pascal>Call;`) for the typed methods.
4. **Example** `examples/add-macro/src/lib.rs`: add a plain-signature fn, e.g.
   `#[modal_rust::function] fn add_plain(a: i64, b: i64) -> anyhow::Result<i64> { Ok(a + b) }` (new name to avoid
   clashing with the existing explicit `add`), plus a unit test asserting `app.add_plain(2,3).local()? == 5`,
   `app.function("add_plain").remote(add_plain::Input{a:2,b:3})` typing, and that the bare explicit `add` stays
   byte-identical. Add `use example_add_macro::*;` where the typed method is called.
5. **Example** `examples/add-macro/Cargo.toml`: add `modal_rust_facade = { path = "../../crates/modal-rust", package = "modal-rust" }`
   (the renamed facade dep; §2). `modal-rust-runtime`/`modal-rust-macros`/`inventory`/`serde` already present.
6. **Runtime** `crates/modal-rust-runtime/src/lib.rs`: **ZERO changes.**

## 9. Edge cases / decisions locked

- **Field & spread order = param declaration order.** Deterministic.
- **`name = "..."` override:** registry key = `entry_name` (`macros:163-165`) as today; generated `mod`/trait/method
  names use the FN IDENT. So `#[function(name="plus")] fn add(..)` → `add::Input`, trait `AddCall`, method `app.add(..)`,
  registered under `"plus"`; `TypedCall::new(self, "plus", …)` dispatches to the right key.
- **Reference/borrowed/destructured/generic params in Mode B → clear `compile_error!`** (§1). Mode A unaffected.
- **Generated `mod #fn_ident` / `<Pascal>Call` collision** with a user item of the same name → ordinary Rust error,
  documented.
- **Crate-name shadowing** (`extern crate modal_rust_macros as modal_rust;`) handled by the `modal_rust_facade` rename
  (§2); the canonical example keeps the attribute spelling AND resolves the facade path.

## 10. Live proof (to drive — CPU, ephemeral run app, cheap)

`app.add_plain(2,3).remote().await? == 5` through the auto-generated `add_plain::Input`/return-type, AND
`app.function("add_plain").remote(add_plain::Input{a:2,b:3}).await? == 5`, AND the explicit `examples/add` form still
returns `AddOutput{sum:42}` byte-identically. Gates on default-members: `cargo fmt --check`,
`cargo clippy --all-targets -- -D warnings`, `cargo build`, `cargo test` — all green. Live tests behind
`#[ignore]` + the live feature; `retry_transient` on all RPCs; Modal flakiness => RETRY.
