# Architecture Boundaries — modal-rust

The curated, **frozen contract** every later phase builds against. Distilled from
the locked decisions in [`research-synthesis.md`](./research-synthesis.md) §2
(which carries the long-form rationale, the adversarial-review reconciliation, and
the verified facts). When this file and the synthesis agree, this file is the
quick reference; when in doubt, the synthesis §2 is authoritative.

This file is **drafted from the synthesis at the research stage** and is
**ratified section-by-section during the architecture phase** (tasks A0–A8). A
few decisions are **spike-contingent** and tagged `[spike: Rx]` — they are locked
on paper but the architecture gate (A8) confirms them against the empirical
research spike named.

## Design stances (bind everything)

1. **Direct-execution-first; Sandbox is a documented fallback** *(hypothesis to
   validate, not a permanent ban).* Try the core path on normal `@app.function`
   FIRST — runtime compile in a Function body is the central hypothesis to prove
   (M4). If direct Function execution proves infeasible for a step (e.g. build
   constraints), **iterate to a Modal Sandbox** for that step and record the
   decision. Sandboxes are a fallback that is explicitly on the table, not the
   default and not out of scope.
2. **The build boundary is the product** *(the hard, non-negotiable invariant).*
   `run` builds Rust at function-execution time; `deploy` builds at image-build
   time and the deployed runtime executes only a prebuilt binary and **never**
   invokes `cargo`. (Whether the build runs in a Function body or a Sandbox, this
   run-vs-deploy split holds.)

3. **Prefer static dispatch.** Favor compile-time polymorphism — `enum`
   (closed-world), generics (`T: Trait`) / `impl Trait`, marker/type-state, `cfg`
   features — over `dyn Trait`; reach for `dyn` only for genuinely open/unbounded
   sets. The handler registry is the one open set and uses `fn` pointers, not
   `Box<dyn>` (§3).

---

## 1. Workspace & crate layout (synthesis §2.1)

Single virtual cargo workspace at the repo root:

```text
modal-rust/
  Cargo.toml                       # [workspace]
  crates/
    modal-rust-runtime/            # Registry, typed!(), Codec, HandlerFn, run_cli, runner protocol
    modal-rust-cli/                # the `modal-rust` binary; generates+drives shims; clap lives here
    modal-rust-client/             # talk to Modal (shared protocol types; modal-rs only for `call`)
    modal-rust-macros/             # empty placeholder for v2 #[modal_rust::function]
  examples/
    add/                           # src/lib.rs (add + modal_registry) + src/bin/modal_runner.rs
```

**Acyclic dependency edges (no cycle):**

```text
macros  -> runtime
client  -> runtime            (shared protocol types only)
cli     -> client + runtime
<per-user runner binary> -> runtime + the user's own lib crate
```

- `modal-rust-runtime` has **zero Modal / network / Python deps** — only `serde`,
  `serde_json`, `anyhow`, and a tiny hand-rolled arg parser. Rationale: it is
  recompiled on **every** dev run and baked into **every** deploy image; keep the
  recompiled/baked artifact minimal to blunt cold-start and cascading-rebuild cost.
- **`clap` is CLI-only** (resolves the review conflict): the runner uses a
  hand-rolled 3-flag parser; `clap` lives only in `modal-rust-cli`.
- **Edition 2021** for published crates (2024 only in user/example crates).
- **The user does not own `main()`.** The CLI owns a ~15-line
  `src/bin/modal_runner.rs` template whose fixed body is
  `modal_rust_runtime::run_cli(user_crate::modal_registry())`. It is committed for
  `examples/add` and generated under `.modal-rust/` for arbitrary user crates. The
  user authors only `lib.rs` and `pub fn modal_registry() -> Registry`.

---

## 2. Runner CLI protocol — the frozen seam (synthesis §2.2)

> Do not break. Any change must be **additive-only** and reviewed against the
> manual-registry runner per `WORKING.md`.

- Binary `modal_runner`. Invocation:

  ```text
  modal_runner --entrypoint <name> ( --input-json <json> | --input-file <path> | --input-stdin )
  ```

  `--input-file`/`--input-stdin` exist so the shim can write large inputs to
  `/tmp` and avoid argv-length limits and the ~100 MB gRPC payload ceiling.
- **stdout carries EXACTLY ONE JSON envelope and nothing else.** All
  cargo/rustc/user diagnostics go to **stderr**. This is load-bearing: it lets the
  Python shim parse the result robustly despite build noise.
- **Exit code mirrors `ok`:** `0` success, `1` failure. The error kind lives in
  the JSON, not the exit code.
- **Envelopes (verbatim):**
  - Success: `{"ok":true,"value":<json>}`
  - Failure: `{"ok":false,"error":{"kind":"<kind>","message":"...","details":<json|null>,"backtrace":"..."}}`
  - `details` is an **optional** field that carries the **wrapped user error**
    structurally (see "User-error wrapping" below). It is `null`/absent for
    framework kinds and for opaque user errors.

### Error-kind taxonomy — frozen at **FIVE** kinds

| kind | cause |
| --- | --- |
| `decode_error` | input not valid JSON, **or** valid JSON that failed to deserialize into the handler's `In` |
| `unknown_entrypoint` | `--entrypoint` name not in the registry |
| `function_error` | handler returned `Err(_)` — the **user error wrapped** on the top-level enum; `message` = Display/anyhow chain, `details` = the serialized user error when it is `Serialize` |
| `encode_error` | handler's `Out` failed to serialize (e.g. non-string map keys, NaN) — **must NOT be reported as `panic`** |
| `panic` | handler unwound; message + backtrace captured via a panic hook + `catch_unwind` |

- **Precedence (frozen):** top-level JSON parse → entrypoint lookup → decode `In`
  → call → encode `Out`. Malformed JSON + bad entrypoint → `decode_error`
  (documented + unit-tested in M0).
- **Panic capture:** panic hook records message + `std::backtrace::Backtrace` into
  a per-thread slot; each handler runs in `std::panic::catch_unwind`. Requires
  `panic = "unwind"` (see §6). *(Additive note — M0-R, 2026-06-03, not a protocol
  change: the implementation captures with `Backtrace::force_capture()` so the
  backtrace is populated regardless of `RUST_BACKTRACE` — the shim no longer needs
  to set it; and uses a `thread_local!` slot + a `std::sync::Once`-installed hook
  rather than a process-global `Mutex` slot, so parallel panics on different
  threads never race. The frozen envelope is unchanged.)*
- **User-error wrapping (top-level enum):** the runner models failure as a Rust
  enum that **wraps** the user's error rather than stringifying it early:

  ```rust
  pub enum RunnerError {
      Decode(String),
      UnknownEntrypoint(String),
      Function { message: String, details: Option<serde_json::Value> }, // user error wrapped
      Encode(String),
      Panic { message: String, backtrace: String },
  }
  ```

  The monomorphized `typed!` wrapper (§3) knows the handler's concrete error type
  `E`, so it preserves structure: `message` from `Display`/the anyhow chain, and
  `details = serde_json::to_value(&e).ok()` **when `E: Serialize`** (otherwise
  `details` is `null` and only `message` is set). This surfaces a rich, typed user
  error through the single top-level envelope instead of flattening it to a string.
- **Compatibility rule:** envelope additions must be **additive optional fields**
  (`details` follows this rule); an optional `meta`/`version` field is reserved.
  Consumers ignore unknown fields.

---

## 3. Registry / `typed!()` / `HandlerFn` — static dispatch, macro-compatible (synthesis §2.3)

**Prefer static dispatch — no trait objects.** Every entry is a **monomorphized
wrapper** reduced to a bare function pointer; there is **no `Box<dyn Handler>`** and
no vtable. `run_cli`/`Registry` never change shape regardless of how an entry was
registered. The manual path and the future `inventory` / `#[modal_rust::function]`
path converge on one dispatch path:

```text
name -> monomorphized typed! wrapper (fn pointer) -> bytes in -> bytes out   (JSON Codec in v0)
```

- **Handler is a bare `fn` pointer (static dispatch):**
  `type HandlerFn = fn(&[u8]) -> Result<Vec<u8>, RunnerError>;`. No `dyn`, no `Box`,
  no vtable — just a monomorphized free function per entry, called through one
  cheap indirect `fn`-pointer jump after the name lookup.
- **`typed!(f)` is a `macro_rules!` (not a `dyn`-returning fn):** it generates a
  monomorphized wrapper `fn` and yields its pointer, so decode/call/encode are all
  inlined/monomorphized for `f`'s concrete `In`/`Out`/`Err`:

  ```rust
  macro_rules! typed {
      ($f:path) => {{
          fn __wrap(input: &[u8]) -> ::core::result::Result<Vec<u8>, $crate::RunnerError> {
              let arg = $crate::codec::decode(input)?;            // In inferred from $f
              match $f(arg) {
                  Ok(out) => $crate::codec::encode(&out),         // Out inferred from $f
                  Err(e)  => Err($crate::RunnerError::function(e)),// wraps the user error (§2)
              }
          }
          __wrap as $crate::HandlerFn
      }};
  }
  ```

  (A non-macro generic `typed::<In, Out, F>()` is possible only via a ZST-`F`
  `unsafe` materialization; the macro avoids that and is the chosen v0 API. The
  architecture phase A2 finalizes the exact mechanism.)
- **Codec-neutral on bytes:** the wrapper decodes/encodes via a `Codec` (JSON in
  v0). A future `--input-format cbor` adds a `Codec` impl only — it never touches
  `HandlerFn` or `Registry`.
- **Async reserved now:** `HandlerFn` stays synchronous; a reserved `typed_async!`
  variant wraps an `async fn` by `block_on`-ing a runtime-owned Tokio executor
  inside the same `fn(&[u8]) -> Result<Vec<u8>, _>` shape. May be unimplemented in
  v0, but the shape is committed; the future macro detects `async fn` → `typed_async!`.
- **`Registry`** = `BTreeMap<&'static str, HandlerFn>` (static-str keys, fn-pointer
  values — no allocation, no `dyn`). Builder:
  `Registry::new().function("add", typed!(add))`. **Duplicate names are rejected
  with a hard error at runner startup** (no silent last-write-wins).
- **Argument shape frozen:** the runner input is always a **single named JSON
  object**. Single-arg handlers take `In` directly. A future multi-arg macro
  generates a private `#[derive(Deserialize)]` named-field args struct (field names
  = parameter names) + a shim that destructures and calls `f(a, b)`. **Never a
  positional array.**

```rust
// v0 manual registry (examples/add/src/lib.rs)
pub fn modal_registry() -> Registry {
    Registry::new().function("add", typed!(add))
}
// v2 #[modal_rust::function] generates the SAME __wrap fn and registers its
// pointer via inventory (or a generated static `match` table) — protocol unchanged.
```

> **Concurrency caveat (recorded):** v0 panic-capture uses a process-global slot
> and the process exits after one call → correct for v0. A future concurrent host
> (PyO3 Mode B) must revisit per-call panic routing and the panic-then-reuse
> hazard before enabling concurrency.
> *(Additive note — M0-R, 2026-06-03: capture is now a `thread_local!` slot + a
> `Once`-installed hook, so the process-global-slot race is already resolved for the
> process-exits-after-one-call v0 path and made safe under the parallel test
> harness. The PyO3-Mode-B panic-then-reuse review still stands.)*

---

## 4. Run-vs-deploy build boundary (synthesis §2.4–§2.5)

| | `run` (dev) | `deploy` (prod) |
| --- | --- | --- |
| Source into container | `add_local_dir(LOCAL_SRC, "/src", copy=False)` — mounted at **startup** | `add_local_dir(MANIFEST_DIR, "/app", copy=True)` — copied into an **image layer** |
| Where `cargo build` runs | **In the function body**, at execution time | **At image-build time** via `run_commands(...)` |
| Cargo cache Volume | optional, run-path only (best-effort, §7) | **none** |
| Runtime executes | freshly built `modal_runner` | **only** the baked `/app/modal_runner` |
| `cargo` at call time | yes (that's the point) | **never** |

**Deployed-runtime invariant:** the deployed body execs only
`/app/modal_runner …`, mounts no source and no cache Volume, and **never** invokes
`cargo`. **Proof obligation (AGENTS.md):** `cargo build` appears in deploy/build
logs and is **ABSENT** from call logs (M8).

**Run-path build location `[spike: R-mount / M2]`:** build into a **known-writable
LOCAL path by default** (`CARGO_TARGET_DIR=/tmp/target`), **not** a Volume.
`CARGO_HOME` (read-mostly index/downloads) may sit on the Volume (lower risk).
Building directly into a network-FS target dir is rejected as the default (cargo's
many small stat/lock ops + the "volume busy"/partial-commit hazards). If the M2
write-probe shows `/src` is read-only (or a build script writes into the source
tree), the canonical recipe is: **mount read-only → `cp -a /src /tmp/build` →
`cargo build` with `CARGO_TARGET_DIR` on a known-writable path.** If a normal
Function body proves unable to build at all (a hard infeasibility, not merely a
read-only mount), the documented fallback is a Modal **Sandbox** build for the
`run` path (stance 1); the run-vs-deploy split is unchanged.

`timeout=1800` on the run path (Modal's 300 s default is too low for cold start +
first full compile + crate downloads). Never call `vol.reload()` mid-build.

---

## 5. Generated Python shims (synthesis §2.4–§2.5)

Private, disposable artifacts under gitignored `.modal-rust/generated/`. v0
authoring/build uses **generated Python + the official `modal` CLI** (the
known-good control path). The shims must stay byte-equivalent (modulo injected
params) across M1/M4/M7/M8 and M9 — M9 must not become a second control path.

Common image preconditions for the `rust:` base:

- `add_python="3.12"` is **mandatory** (a bare `rust:` image is an invalid
  Function image; 3.12 is the lowest-risk doc-by-example value).
- `.entrypoint([])` neutralizes any base ENTRYPOINT so Modal's Python runtime
  starts. `[spike: R-image / M3]` confirms the toolchain+python coexistence.

**`dev_app.py` (run):**

```python
image = (modal.Image.from_registry(f"rust:{RUST_VER}-slim", add_python="3.12")
         .entrypoint([]).env({"RUST_BACKTRACE": "1"})
         .add_local_dir(LOCAL_SRC, "/src", copy=False,
                        ignore=["target", ".git", ".modal-rust", "**/*.rlib"]))

@app.function(image=image, timeout=1800)            # + volumes={...} for the optional cache
def run_entrypoint(entrypoint: str, input_json: str) -> str:
    # build into a writable path; copy /src -> /tmp/build if mount is read-only
    # cargo build --release --bin modal_runner   (logs -> stderr)
    # write input -> /tmp/in.json; exec modal_runner --entrypoint .. --input-file /tmp/in.json
    # return the single stdout JSON envelope

@app.local_entrypoint()                              # REQUIRED: a bare @app.function does not bind `modal run` flags
def main(entrypoint: str, input_json: str = "{}"):
    print(run_entrypoint.remote(entrypoint, input_json))
```

**`deploy_app.py` (deploy):**

```python
image = (modal.Image.from_registry(f"rust:{RUST_VER}-slim", add_python="3.12")
         .entrypoint([]).env({"RUST_BACKTRACE": "1"})
         .add_local_dir(MANIFEST_DIR, "/app", copy=True,
                        ignore=["target", ".git", ".modal-rust"])
         .run_commands("cd /app && cargo build --release --bin modal_runner")
         .run_commands("cp /app/target/release/modal_runner /app/modal_runner && chmod +x /app/modal_runner"))

@app.function(image=image)                           # autoscaling knobs: min/max_containers, scaledown_window
def call_entrypoint(entrypoint: str, input_json: str) -> str:
    # write input -> /tmp/in.json; exec ONLY /app/modal_runner --entrypoint .. --input-file /tmp/in.json
    # NO cargo, NO source mount, NO cache Volume
```

Optional deploy hardening (documented, not v0-default): a dependency-prebuild
layer (copy `Cargo.toml`/`Cargo.lock` + stub, build deps, then copy real src) to
blunt cascading rebuilds; and a `--vendor` (`cargo vendor`) flag for hermetic
builds if a target account restricts build-time egress `[spike: R-egress / M7]`.

**`call_app.py` (call):**

```python
@app.local_entrypoint()                              # module-scope print(fn.remote(..)) would NameError
def main(entrypoint: str, input_json: str = "{}"):
    print(modal.Function.from_name(APP, "call_entrypoint").remote(entrypoint, input_json))
```

Invoked arg + return are plain `str` (the JSON envelope text) — well under the
~100 MB gRPC limit. Large I/O routes via a Volume/object storage (out of scope for
the `add` POC). Web endpoints are **opt-in, authenticated** only — never
auto-exposed public on deploy.

---

## 6. Build-profile constraint (synthesis §2.6)

`panic = "unwind"` is required for `catch_unwind` to upgrade a panic into a
structured envelope. A user's `[profile.release] panic = "abort"` would silently
degrade the `panic` kind into a raw process abort. **Mitigation:**
`modal-rust doctor --rust` detects `panic = "abort"` in the resolved release
profile and warns/fails; and/or the runner is built under a dedicated profile
(or `--config` override) that forces `panic = "unwind"`. M0 asserts the build is
NOT `panic = "abort"`.

---

## 7. Smart run cache (run-path only; synthesis §2.4 cache, §1.3)

Goal: `modal-rust run` should rebuild only when build inputs changed. The public
dev-loop target is:

- **Exact source-fingerprint hit:** compute a content fingerprint over the mounted
  source after the normal ignore rules (`target`, `.git`, `.modal-rust`, generated
  shims), plus the Rust image/toolchain, build profile, target triple, features,
  lockfile, and runner/generator version. If
  `/cache/run/<fingerprint>/modal_runner` exists, execute it directly and skip
  `cargo build`.
- **Fingerprint miss:** run `cargo build` in the Function body, but with warm
  cache state. `CARGO_HOME` lives on the Volume. The compiled-artifact strategy
  must be benchmarked before becoming default: either `CARGO_TARGET_DIR` directly
  on a Volume path, or restore a cached target snapshot into `/tmp/target`, build
  on local disk, then sync the target snapshot and freshly built `modal_runner`
  back to the Volume after success.
- **Correctness rule:** cache state is advisory. A hit is valid only when the
  fingerprint manifest matches the current inputs; a miss only costs time, never a
  wrong result.
- Modal Volume semantics: automatic background commits "every few seconds" + a
  final commit exist, but an explicit commit after a successful build/cache-sync is
  allowed once cargo has exited and file handles are closed. `vol.reload()` fails
  "volume busy" when files are open (cargo holds locks) — **never on the hot build
  path.**
- **Run-path only and NOT a dependency of deploy.** Reset via `modal volume rm` / a
  new name. Single-writer / low concurrency (v1 last-write-wins, avoid >~5
  concurrent commits); parallel shared-cache writes out of scope for v0 unless the
  implementation adds a proven cross-container lock or shard key.

---

## 8. CLI surface (synthesis §2.7)

- `modal-rust doctor [--rust]` — preflight `~/.modal.toml`/`MODAL_TOKEN_*`, `modal`
  CLI on `$PATH`, pinned rust/python/image-builder versions; `--rust` adds
  `cargo`/`rustc`/`target` + `panic = "abort"` detection. Missing prerequisites →
  an actionable **structured error** reusing the runner error model.
- `modal-rust run <entrypoint> [--input <json|@file>] [--gpu] [--timeout]` —
  generate `dev_app.py`, then `modal run`.
- `modal-rust deploy <entrypoint> [--gpu] [--app-name]` — generate `deploy_app.py`,
  then `modal deploy`.
- `modal-rust call <entrypoint> [--input <json|@file>] [--app-name] [--use-modal-rs]`
  — generate/locate `call_app.py` via `modal run` (default), or behind a validated
  `--use-modal-rs` flag use `Function::from_name().remote()`.
- The CLI is a **pure wrapper introducing no new Modal capability.** Generated
  shims stay private/gitignored and byte-equivalent (modulo params) to the
  prototype shims.

**Local Rust remote-call API (`modal-rust-client`):** separate from the CLI
`call` command, local user code needs typed calls analogous to Modal Python's
`.remote()`. The low-level transport can be explicit:

```rust
let add = modal_rust_client::RemoteFunction::<__AddInput, i32>::from_name(
    "modal-rust-add-poc",
    "add",
);
let sum = add.remote(__AddInput { a: 40, b: 2 }).await?;
assert_eq!(sum, 42);
```

But the ergonomic target hides both generated transport types:

```rust
#[modal_rust::function]
fn add(a: i32, b: i32) -> anyhow::Result<i32> { ... }

let app = modal_rust::app("modal-rust-add-poc");
let sum = app.add(40, 2).await?;
assert_eq!(sum, 42);
```

Macro-generated stubs synthesize the private named input struct (preserving the
single named-object wire shape) and return the handler's real output type
directly. The transport still serializes `In` with the same codec, invokes the
deployed `call_entrypoint` (`entrypoint`, `input_json`) through the validated
generated-Python path or a validated `modal-rs` backend, parses the runner
envelope, and returns typed `Out` or the frozen `RunnerError` shape. The macro
must compile to this same client API and must not change the runner protocol,
registry shape, or run-vs-deploy build boundary.

> **modal-rs vs generated Python (resolved):** `FunctionCreate` requires a
> serialized **Python** callable + `image_id`, so modal-rs does **not** remove the
> Python shim, and its `serde-pickle` (protocol 2/3) vs cloudpickle (protocol 4)
> is a compat risk. v0 authoring/build = generated Python + official `modal` CLI;
> modal-rs is confined to `call` behind `--use-modal-rs`. Vendor the proto if
> adopted deeper later.

---

## 9. GPU tiering & `gpu=` passthrough (synthesis §2.8)

- `--gpu <spec>` passed through **verbatim** (incl. `"H100:8"` and fallback lists).
  modal-rust does NOT validate the drifting catalog — it surfaces Modal's error.
- **Tier 0** (default `rust:slim`, driver-only — `libcuda` + `nvidia-smi`):
  `nvidia-smi` from Rust; cudarc driver-API execution of **precompiled PTX**.
- **Tier 1** (`+ pip nvidia-cuda-nvrtc-cu12` / `nvidia-cuda-runtime-cu12`, or
  `nvidia/cuda:*-runtime-*`): runtime NVRTC / Burn / cubecl.
- **Tier 2** (`nvidia/cuda:*-devel-*` + `add_python`): only when `nvcc` is needed.
- cudarc pinned with default `dynamic-loading` (links with no CUDA at build time);
  keep container toolkit major ≤ host (12.x/13.x). A **startup self-check** dlopens
  the required libs and fails loudly (dynamic-loading hides missing libs until
  runtime — the Burn-on-driver-only footgun).
- Rust-CUDA / `rustc_codegen_nvvm` (Rust-authored kernels) is **out of scope v0.**
- **Never hardcode** the point-in-time driver/CUDA version; re-verify per account.

---

## 10. Ignore rules (synthesis §2.4 ignore, §1.1)

- **Mount/copy ignore** (client-side; dockerignore-syntax patterns or a
  `Path->bool` predicate, converted to a `FilePatternMatcher`): `target`, `.git`,
  `.modal-rust`, and build artifacts (e.g. `**/*.rlib`). Applied to both
  `add_local_dir(copy=False)` (run) and `add_local_dir(copy=True)` (deploy) to keep
  the upload minimal and reactive. A future user-facing **`.modalrustignore`**
  mirrors `.dockerignore` for this set.
- **`.gitignore`:** `.modal-rust/` (generated shims + generated runner), `target/`,
  scratch `tmp/`. Generated shims and scratch are disposable, regenerable
  artifacts and must never be a committed source of truth.

---

## 11. Open questions (user-sensitive; recommended defaults — synthesis §4)

Recorded here so the architecture gate surfaces them; each has a safe default and
none block M0–M3.

1. **GPU/cost confirmation** → default: require `--yes` for `run --gpu` and
   `deploy`, with a per-run cost note.
2. **Public deploys/auth** → default: **no** web endpoint in v0 (callable only via
   `Function.from_name().remote()`).
3. **Default `call` mode** → default: generated `call_app.py` via `modal run`;
   modal-rs behind `--use-modal-rs`, promoted only after a non-scalar round-trip
   smoke.
4. **Wire format** → default: JSON for v0 (the codec-neutral `&[u8]` trait makes
   CBOR/msgpack additive later).
5. **Toolchain pin** → default: `rust:1.83-slim` + `add_python="3.12"` as the
   single image backing run and deploy.
6. **Cache sharing/concurrency** → default: one shared `modal-rust-cargo-cache`
   (fine for one developer); shard or use Volume v2 if many developers / parallel
   runs.

---

## 12. Residual risks (synthesis §5)

Runtime-compile + build-time egress (M4/M7, `--vendor` fallback); mount writability
(M2 probe gates M4); cascading rebuild on deploy (dependency-prebuild mitigation);
cold-start build latency vs the 1800 s timeout (M6 cache, null result allowed);
modal-rs immaturity (confined to `call`); GPU footgun + version drift (tiering +
startup self-check); protocol-freeze pressure (additive-only seam); payload ceiling
(`--input-file` + Volume routing for large I/O); `add_python` interpreter
coexistence (M3 `which -a`).
