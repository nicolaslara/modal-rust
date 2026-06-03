# modal-rust — Research & Architecture Synthesis

Authoritative consolidation of the research digest, the architecture/design proposal,
and the three adversarial reviews (milestone sequencing, Modal semantics, idiomatic Rust
+ macro-compatibility). This document is the single source of truth for the architecture
gate.

- **Date:** 2026-06-03
- **Status:** locked decisions below incorporate every HIGH-severity `must_fix` from the
  reviews. Verdict: **plan-is-sound** for engineering to proceed, with the user-facing
  questions in Section 4 carrying recommended defaults (none are blockers).
- **Design stances (from `project.md` / `AGENTS.md`; see §0 Amendments):**
  1. **Direct-execution-first; Sandbox is a documented fallback** *(hypothesis to
     validate, not a permanent ban).* Try normal `@app.function` first; if runtime
     compile in a Function body is infeasible for a step, iterate to a Modal Sandbox
     and record it.
  2. **The build boundary is the product** *(the hard, non-negotiable invariant).*
     `run` = build at function-execution time; `deploy` = build at image-build time,
     deployed runtime executes only a prebuilt binary and never invokes `cargo`.
  3. **Prefer static dispatch** — favor `enum`/generics/`impl Trait` over
     `dyn Trait`; reach for `dyn` only for genuinely open sets (see §0.3, §2.3).

---

## 0. Design Amendments (2026-06-03, post-review user steer)

These three corrections supersede any conflicting text later in this document and
in §2 (Locked Decisions). Downstream workpad files and the workflow scripts are
aligned to them.

### 0.1 Sandbox is a fallback, not a ban (amends stance 1, §2.4, §3, §5)

"No Sandboxes" is **not** non-negotiable. The stance is **direct-execution-first**:
prove the core path on a normal `@app.function` (runtime compile in the Function
body, M4); **if that proves infeasible** for a step, **iterate to a Modal Sandbox**
for that step and record the decision. Sandboxes are an explicit, documented
fallback path — on the table, not out of scope. The **build boundary** (run =
build-at-exec, deploy = build-at-image-time, deployed runtime never runs cargo)
remains the hard invariant **regardless** of whether the build runs in a Function
body or a Sandbox. M4 acceptance gains a fallback branch: if the Function-body
build is infeasible, evaluate + record a Sandbox-based build rather than failing.

### 0.2 User errors are wrapped on the top-level enum (amends §2.2)

The runner models failure as a Rust `RunnerError` enum that **wraps** the user's
error instead of stringifying it early. The five kinds are unchanged; the failure
envelope gains an **optional `details`** field (additive, per the compat rule):
`{"ok":false,"error":{"kind":"function_error","message":"<Display/anyhow chain>","details":<serialized user error|null>,"backtrace":"..."}}`.
For `function_error`, the monomorphized wrapper preserves structure — `message`
from `Display`, and `details = serde_json::to_value(&e).ok()` **when the handler's
error type is `Serialize`** (else `null`). This surfaces rich, typed user errors
through the single top-level enum.

### 0.3 Prefer static dispatch (amends §2.3)

Favor compile-time polymorphism — `enum` (closed-world), generics (`T: Trait`) /
`impl Trait` (monomorphization), marker/type-state, `cfg` features — over
`dyn Trait`; reach for `dyn` only when the set of implementations is genuinely
open/unbounded. Concretely for v0: the handler **registry** is the one open set
(arbitrary user functions), so it erases each function to a monomorphized **`fn`
pointer** (`type HandlerFn = fn(&[u8]) -> Result<Vec<u8>, RunnerError>;`,
`Registry = BTreeMap<&'static str, HandlerFn>`, built via the `typed!(f)`
`macro_rules!`) — also static, no `Box<dyn>`/vtable. The future
`#[modal_rust::function]` proc-macro generates the same wrapper + an `inventory`
registration (or a static `match` table). `typed_async!` reserved with the same
shape. Boundaries.md §3 holds the canonical detail.

---

## 1. Verified Facts

Confidence is `high` (primary Modal doc or extracted crate source), `medium` (doc-by-example
or inference), or `low`. Point-in-time facts (driver versions, GPU catalog) WILL drift —
re-verify before pinning.

### 1.1 Images — local files, build steps, base images

| Claim | Source | Confidence |
| --- | --- | --- |
| `add_local_file(local, remote, *, copy=False)` and `add_local_dir(local, remote, *, copy=False, ignore=[])`; `copy` defaults to `False`. | modal.com/docs/reference/modal.Image | high |
| `copy=False`: files are added to the container **at startup**, NOT baked into an image layer; "you can't run additional build steps after". Enables fast redeploy. | modal.com/docs/guide/images | high |
| `copy=True`: files are copied into an **image layer at build time** (Docker `COPY`-like); required if later `run_commands` must see them; slows iteration. | modal.com/docs/reference/modal.Image | high |
| `run_commands(*commands, env=, secrets=, volumes=, gpu=, force_build=False)` runs shell at image-build time; each call is a separate Docker-RUN-like layer (no shell state persists between layers beyond the filesystem). Verified network access at build time (docs show `run_commands('git clone ...')`, `apt`). | modal.com/docs/reference/modal.Image; modal.com/docs/guide/custom-container | high |
| Images are cached per layer; breaking one layer cascades rebuilds of all later layers. Put frequently-changing layers last. | modal.com/docs/guide/images | high |
| `ignore=` is a predicate `Path->bool` OR a Sequence of dockerignore-syntax patterns (converted to a `FilePatternMatcher`). | modal.com/docs/reference/modal.Image | high |
| `from_registry(tag, secret=, *, setup_dockerfile_commands=[], force_build=False, add_python=None, ...)`. `add_python` installs a reproducible standalone Python (python-build-standalone) onto images lacking compatible python/pip. | modal.com/docs/reference/modal.Image; modal.com/docs/guide/existing-images | high |
| A non-Python base (ubuntu, nvidia/cuda, and by extension `rust:`) is usable via `from_registry(..., add_python=...)`. Documented examples use `add_python='3.11'`/`'3.12'`. | modal.com/docs/guide/existing-images | high |
| To run as a Function the image MUST have python+pip on `$PATH` (or `add_python`), be `linux/amd64`, and have a compatible ENTRYPOINT. | modal.com/docs/guide/existing-images | high |
| A custom base ENTRYPOINT must end by exec-ing its args (`exec "$@"`); else the Modal Python runtime never starts. `Image.entrypoint([...])` build step sets/overrides the ENTRYPOINT (use `entrypoint([])` to neutralize). | modal.com/docs/guide/existing-images; modal.com/docs/reference/modal.Image | high |
| Modal 1.0 deprecated `copy_local_*` in favor of `add_local_*`; new default is mount-at-runtime (`copy=False`). | modal.com/docs/guide/modal-1-0-migration | high |
| Exact `add_python` version set undocumented beyond 3.11/3.12 by example. | modal.com/docs/reference/changelog | medium |

### 1.2 Functions at runtime — can a normal `@app.function` compile Rust?

| Claim | Source | Confidence |
| --- | --- | --- |
| **Feasibility CONFIRMED in principle:** a normal `@app.function` body can run subprocesses (Modal's own examples `subprocess.run(['nvidia-smi'])`); Python requirement satisfiable on a Rust base via `add_python`; filesystem writable; long timeouts available. | modal.com/docs/guide/cuda; modal.com/docs/guide/existing-images | high |
| Container filesystem is writable; default per-container disk 512 GiB, up to 3 TiB via `ephemeral_disk`; `/tmp` guaranteed writable. | modal.com/docs/guide/resources; modal.com/docs/guide/dataset-ingestion | high |
| Function timeout defaults to **300 s**, settable 1 s – 24 h via `timeout=`; per-attempt; retries reset it. | modal.com/docs/guide/timeouts | high |
| Functions accept/return cloudpickle-serializable args; plain `str` in / `str` out works for `.remote()`. | modal.com/docs/guide/local-data | high |
| `.remote()` args/results have a **~100 MB gRPC payload limit** (413 on overflow); parametrized-function args limited to 16 KiB; web request bodies up to 4 GiB. | modal.com/docs/guide/troubleshooting | medium |
| **Open / unverified:** whether `add_local_dir(copy=False)` mounts are **read-only or writable** in place. (Reviews + digest call this the single biggest unverified assumption.) | digest open_question | n/a |
| **Open / unverified:** whether `add_python`'s standalone Python coexists cleanly on `$PATH` with a system `python3` already present in a Debian-based `rust:slim`. | digest open_question | n/a |

### 1.3 Volumes — Cargo cache across invocations (run-path only)

| Claim | Source | Confidence |
| --- | --- | --- |
| `modal.Volume.from_name(name, *, create_if_missing=False, version=None, ...)`; idiomatic `create_if_missing=True`. Mounted via `@app.function(volumes={"/path": vol})` on normal Functions. | modal.com/docs/reference/modal.Volume; modal.com/docs/guide/volumes | high |
| Writes are not durable until committed; Modal does **automatic background commits "every few seconds"** plus a final commit on clean shutdown. Explicit `vol.commit()` often unnecessary. | modal.com/docs/guide/volumes | high |
| `vol.reload()` fetches latest committed state but **fails with "volume busy" if files are open**; avoid on the hot build path (Cargo holds lock files). | modal.com/docs/reference/modal.Volume; modal.com/docs/guide/volumes | high |
| Concurrency: v1 = last-write-wins per file, avoid >~5 concurrent commits, no distributed file locking. v2 allows concurrent writes to **distinct** files from many containers; same-file still last-write-wins. | modal.com/docs/guide/volumes | high |
| Pointing a tool cache env var at a Volume path is the first-class Modal caching pattern (Modal's own `HF_HUB_CACHE`/`HF_HOME` examples). Directly analogous to `CARGO_HOME`/`CARGO_TARGET_DIR`. | modal.com/docs/examples/* | high |
| `CARGO_HOME` = registry index + crate downloads; `CARGO_TARGET_DIR` = compiled artifacts; both env-overridable. Cargo incremental reuse needs **stable mount path + stable toolchain + stable rustflags**, else full rebuild. Cargo assumes a **single writer** per target dir (local advisory locks). | doc.rust-lang.org/cargo/* | high (single-writer caveat: medium) |
| **Open / unverified on Modal:** real warm-rebuild speedup over a network FS (many small stat/read ops may erase it); atomicity of background commits at partial-file level. | digest open_question | n/a |

### 1.4 GPU + CUDA for Rust

| Claim | Source | Confidence |
| --- | --- | --- |
| `gpu=` takes a string with optional count suffix (`"H100:8"`) and a fallback list (`gpu=["H100","A100-40GB:2"]`). | modal.com/docs/guide/gpu | high |
| Families: T4, L4, A10, L40S, A100(40/80GB), H100, H200, B200, RTX-PRO-6000. Catalog + per-type max counts **drift** — pass strings through, surface Modal's error. | modal.com/docs/guide/gpu | high (catalog: medium) |
| GPU machines preinstall the NVIDIA driver and CUDA **Driver API** (`libcuda.so`) + `nvidia-smi` ("Tier 0"). Observed point-in-time: driver 580.95.05 / Driver API 13.0 — **will drift**. | modal.com/docs/guide/cuda | high (versions: point-in-time) |
| CUDA **Toolkit** (`libcudart`, `nvcc`, `libnvrtc`) is NOT preinstalled. Add via `nvidia/cuda:*-runtime-*`/`*-devel-*` images (with `add_python`) or pip `nvidia-cuda-runtime-cu12`/`nvidia-cuda-nvrtc-cu12`. Keep toolkit major ≤ host (12.x/13.x guaranteed compatible). | modal.com/docs/guide/cuda | high |
| `cudarc` (0.19.x) defaults to `dynamic-loading`: links with **no CUDA present at build time**; dlopens libs at runtime. Supports CUDA 11.4–13.0 via features or `cuda-version-from-build-system`. | github.com/coreylowman/cudarc; docs.rs/cudarc | high |
| cudarc driver-API path loading **precompiled PTX/cubin** needs only `libcuda` (Tier 0). Runtime NVRTC (`nvrtc::compile_ptx`) needs `libnvrtc.so` present (Tier 1). | docs.rs/cudarc | high |
| Burn (`burn-cuda`) → cubecl → cudarc; CubeCL JIT-compiles kernels via NVRTC at runtime → needs `libnvrtc`+`libcudart` (Tier 1). README: "Requires CUDA 12.x on PATH". Pre-1.0, frequent breaking releases. | lib.rs/crates/burn-cuda; github.com/tracel-ai/burn | high (Burn version churn: high) |
| Rust-CUDA / `rustc_codegen_nvvm` (Rust-authored kernels) is rebooted-but-experimental, pins exact nightlies + LLVM 7. **Out of scope for v0.** | rust-gpu.github.io | medium |

### 1.5 Deploy / invoke + modal-rs SDK surface

| Claim | Source | Confidence |
| --- | --- | --- |
| `modal run` = ephemeral one-off app; `modal deploy` = persistent app, version-incremented with zero-downtime rollout. Rollback is Team/Enterprise-only. | modal.com/docs/guide/managing-deployments | high |
| Deployed functions invoked from any Python process: `modal.Function.from_name(app, fn).remote(*args)` (sync), `.spawn()` (async handle), `.map()` (fan-out). Auth via `~/.modal.toml` or `MODAL_TOKEN_*`. | modal.com/docs/guide/trigger-deployed-functions | high |
| **`modal run app.py::fn --flag val` auto-binds CLI flags ONLY for `@app.local_entrypoint()`**, NOT for a bare `@app.function`. For functions, use a local entrypoint that parses flags and calls `.remote()`, or use argparse. | modal.com/docs/guide/apps | high |
| Web endpoints: `@modal.fastapi_endpoint()`, `@modal.asgi_app()`, `@modal.wsgi_app()`, `@modal.web_server(PORT)` (long-lived server; must bind `0.0.0.0`). URLs `https://<workspace>--<label>.modal.run`; public unless proxy-auth (`Modal-Key`/`Modal-Secret`). | modal.com/docs/guide/webhooks; webhook-urls | high |
| Autoscaling: `min_containers` (was keep_warm), `max_containers` (was concurrency_limit), `scaledown_window` (was container_idle_timeout), `buffer_containers`; scale-to-zero by default. Per-container input concurrency via `@modal.concurrent(max_inputs=, target_inputs=)`. | modal.com/docs/guide/scale; concurrent-inputs | high |
| `modal-rs` (crates.io 0.1.3, 2026-03-09) is **UNOFFICIAL**, single-maintainer (`thehumanworks`), pre-1.0, ~200 downloads. gRPC/tonic over TLS; vendors Modal's `api.proto`; exposes `inner_mut()` raw escape hatch. Reads `~/.modal.toml`/`MODAL_*` like official SDKs. | crates.io/api/v1/crates/modal-rs; extracted 0.1.3 source | high |
| modal-rs exposes app lifecycle, **FunctionCreate/FunctionGet**, image build, Mount/Volume/Secret, sandboxes (most complete). `.remote()/.spawn()/.map()` invocation present; webhook fns rejected from `.remote()`. | extracted 0.1.3 source | high |
| **Critical:** `FunctionCreate` requires `function_serialized` (a serialized **Python** callable) + `image_id`. There is no Rust-native "function body" concept — the deployed unit is still a Python-defined function. So a deployed Modal Function **always needs a Python (or Modal-runtime-compatible) entrypoint**; modal-rs does NOT remove the Python shim. | extracted 0.1.3 source: function_authoring.rs | high |
| Arg/result wire: CBOR (when the function's metadata advertises it) else Pickle. modal-rs's `serde-pickle` emits protocol **2/3**, but Modal Python uses cloudpickle protocol **4** — compat caveat for non-trivial types. | extracted 0.1.3 source: function.rs, pickle.rs | high |

---

## 2. Locked Architecture Decisions

Each decision notes what (if anything) changed versus the original proposal in response to a
review `must_fix`. Decisions marked **[CHANGED]** address a HIGH-severity finding.

### 2.1 Workspace & crate layout

- Single virtual cargo workspace at repo root:
  `crates/modal-rust-runtime`, `crates/modal-rust-cli`, `crates/modal-rust-client`,
  `crates/modal-rust-macros` (empty placeholder), plus `examples/add` (and later GPU examples).
- **Edition 2021** for published crates (2024 only allowed in user/example crates).
- Strictly acyclic deps: `macros -> runtime`; `client -> runtime` (shared protocol types only);
  `cli -> client + runtime`; the per-user **runner binary -> runtime + the user's own lib crate**.
- `modal-rust-runtime` has **zero Modal/network/Python deps** (only serde, serde_json, anyhow,
  and a tiny arg parser). Rationale: it is recompiled on every dev run and baked into the deploy
  image — keep the recompiled/baked artifact minimal to blunt cold-start and cascading-rebuild costs.
- **Where the runner lives:** the CLI **owns** a ~15-line `src/bin/modal_runner.rs` template and
  writes it into the user crate (committed for `examples/add`; generated under `.modal-rust/` for
  arbitrary user crates). The user authors only `lib.rs` and one `pub fn modal_registry() -> Registry`.
  The runner `main()` is fixed: `modal_rust_runtime::run_cli(user_crate::modal_registry())`.
  This realizes "the user does not own `main()`" and keeps the runner template identical between the
  manual-registry and future-macro worlds.

> **Review conflict note (clap):** the runtime crate must stay minimal, yet a sketch listed `clap`.
> **Resolution:** the runner uses a hand-rolled arg parser (3 flags) — `clap` is NOT a runtime
> dependency. `clap` lives only in `modal-rust-cli`.

### 2.2 Runner CLI protocol (the frozen seam — do not break)

- Binary name `modal_runner`. Invocation:
  `modal_runner --entrypoint <name> ( --input-json <json> | --input-file <path> | --input-stdin )`.
- **stdout carries EXACTLY ONE JSON envelope and nothing else.** All cargo/rustc/user diagnostics
  go to **stderr**. This is load-bearing: it lets the Python shim parse the result robustly despite
  build noise.
- Exit code mirrors `ok`: `0` on success, `1` on failure. The error kind lives in the JSON, not the
  exit code.
- Envelope schema (matches `project.md`/`AGENTS.md` verbatim):
  - Success: `{"ok":true,"value":<json>}`
  - Failure: `{"ok":false,"error":{"kind":"<kind>","message":"...","details":<json|null>,"backtrace":"..."}}`
    where `details` is the additive optional field carrying the **wrapped user error** for
    `function_error` (see §0.2): `message` = Display/anyhow chain, `details = serde_json::to_value(&e).ok()`
    when the handler's error type is `Serialize`, else `null`.
- **`--input-file`/`--input-stdin` exist specifically** so the shim can write large inputs to
  `/tmp` and avoid argv-length limits and the ~100 MB gRPC payload ceiling.

#### Error-kind taxonomy **[CHANGED]**

The closed enum is **frozen now** as **five** kinds (the original four plus `encode_error`):

| kind | cause |
| --- | --- |
| `decode_error` | input was not valid JSON **OR** valid JSON that failed to deserialize into the handler's `In` type |
| `unknown_entrypoint` | `--entrypoint` name not in the registry |
| `function_error` | handler returned `Err(_)` — the **user error wrapped** on the top-level `RunnerError` enum; `message` = Display/anyhow chain, `details` = serialized user error when `Serialize` (§0.2) |
| `encode_error` | the handler's `Out` value failed to serialize (e.g. non-string map keys, NaN) |
| `panic` | the handler unwound; message+backtrace captured via a panic hook + `catch_unwind` |

- **[CHANGED — Rust review HIGH #3]** Output serialization failure must NOT be reported as `panic`.
  The original sketch did `serde_json::to_value(out).expect(...)`, which would surface an encode
  failure as a user panic. We add `encode_error` so the four failure modes (bad input, missing
  entrypoint, logic error, encode error) are never conflated, plus `panic` for true unwinds.
- **Precedence (frozen):** top-level JSON parse → entrypoint lookup → decode `In` → call → encode `Out`.
  A request with malformed JSON yields `decode_error` before `unknown_entrypoint`. (Documented and
  unit-tested in M0.)
- **Panic capture:** the runner installs a panic hook recording message + `std::backtrace::Backtrace`
  into a Mutex slot, runs each handler in `std::panic::catch_unwind`, and the **shim sets
  `RUST_BACKTRACE=1`** so the field is populated. Requires `panic = "unwind"` (see 2.6).
- **Compatibility rule:** future additions to the envelope must be **additive optional fields**;
  an optional `meta`/`version` field is reserved. Unknown fields are ignored by consumers.

### 2.3 Registry / `typed!()` / `HandlerFn` API (static dispatch, macro-compatible)

> Amended by §0.3: **prefer static dispatch — no trait objects.** The text below reflects the
> bare-`fn`-pointer design (the original `Box<dyn Handler>` thesis is superseded).

The core thesis: every function is reduced to the **same** bare `fn` pointer at registration time
via a **monomorphized wrapper**; the wrapper owns all decode/encode; `run_cli` and `Registry` never
change shape regardless of how an entry was registered. The manual path and the future `inventory`
path converge on one dispatch code path — no `Box<dyn>`, no vtable.

Folded in NOW so later phases are additive rather than seam-breaking:

- **Static dispatch (no trait objects).** `type HandlerFn = fn(&[u8]) -> Result<Vec<u8>, RunnerError>;`
  — a bare monomorphized function pointer per entry, called through one cheap indirect jump after the
  name lookup.
- **Codec-neutral on bytes.** The monomorphized wrapper decodes/encodes via a `Codec` (JSON for v0);
  a future `--input-format cbor` adds a `Codec` impl only — it never touches `HandlerFn` or
  `Registry`. (Bytes also eliminates the per-call `Value` clone.)
- **`typed!(f)` is a `macro_rules!`** (not a `dyn`-returning fn): it generates the monomorphized
  wrapper `fn` and yields its pointer, inlining decode/call/encode for `f`'s concrete `In`/`Out`/`Err`
  and wrapping a user `Err` via `RunnerError::function(e)` (§0.2). A non-macro generic
  `typed::<In,Out,F>()` is possible only via an `unsafe` ZST-`F` materialization; the macro is the
  chosen v0 API.
- **[Rust review HIGH #4] Async path reserved now.** `HandlerFn` stays **synchronous**; a reserved
  `typed_async!` variant wraps an `async fn` by `block_on`-ing a runtime-owned Tokio executor inside
  the same `fn(&[u8]) -> Result<Vec<u8>, _>` shape. The future `#[modal_rust::function]` macro detects
  `async fn` and expands to `typed_async!` vs `typed!`. Additive; may be unimplemented in v0 but the
  shape is committed.
- **[Rust review MED] Duplicate-name policy.** `Registry::function()` and `Registry::from_inventory()`
  **reject duplicate names** with a hard error at runner startup (silent last-write-wins is a footgun
  the macro/inventory world makes easy).
- **[Rust review MED] Argument shape frozen.** The runner input is always a **single named JSON
  object**. Single-arg handlers take `In` directly. A future multi-arg macro generates a private
  `#[derive(Deserialize)]` named-field args struct (field names = parameter names) and a shim fn that
  destructures and calls `f(a, b)`, registered via `typed!(shim)`. **Arguments are a named object,
  never a positional array.**

`Registry` is a `BTreeMap<&'static str, HandlerFn>` (static-str keys, fn-pointer values — no
allocation, no `dyn`). Builder: `Registry::new().function("add", typed!(add))`. The future
`#[modal_rust::function]` generates the same wrapper `fn` and registers its pointer via `inventory`
(or a generated static `match` table) — protocol unchanged.

> **Concurrency caveat (recorded):** the v0 panic-capture uses a process-global slot and the process
> exits after one call, so it is correct for v0. A future concurrent host (PyO3 Mode B) must revisit
> per-call panic routing and the panic-then-reuse hazard before enabling concurrency.

### 2.4 `run` path shim (`dev_app.py`) — build at function-execution time

- Image: `Image.from_registry(f"rust:{RUST_VER}-slim", add_python="3.12").entrypoint([])` then
  `.env({"RUST_BACKTRACE":"1", ...})` and
  `.add_local_dir(LOCAL_SRC, "/src", copy=False, ignore=["target",".git",".modal-rust","**/*.rlib"])`.
- `.entrypoint([])` neutralizes any base-image ENTRYPOINT so the Modal Python runtime starts.
- `add_python="3.12"` is mandatory (a bare `rust:` image is an invalid Function image) and is the
  lowest-risk value (only 3.11/3.12 are documented-by-example).
- **[CHANGED — Modal review HIGH #1] CLI argument routing.** The shim defines a
  `@app.local_entrypoint()` that parses `--entrypoint`/`--input-json` and calls
  `run_entrypoint.remote(entrypoint, input_json)`. A bare `@app.function` does NOT auto-bind CLI
  flags. (Alternatively the CLI invokes via `Function.from_name(...).remote(...)`; either way, M1
  explicitly proves a CLI-passed value reaches the function body.)
- **[CHANGED — Modal review HIGH #2] Build location decoupled from the cache.** The function body
  builds into a **known-writable LOCAL path by default** (`CARGO_TARGET_DIR=/tmp/target`), NOT
  directly into a Volume. `CARGO_HOME` (read-mostly registry/download cache) MAY sit on the Volume
  earlier (lower risk). Only after M6 benchmarks the Volume as net-positive and lock-safe may
  `CARGO_TARGET_DIR` optionally point at the Volume. Building directly into a network-FS target dir
  is rejected as the default because of cargo's many small stat/lock ops and the "volume busy"/
  partial-commit hazards.
- **[CHANGED — Modal review MED] Read-only mount handling.** If the M2 write-probe shows `/src` is
  read-only (or for arbitrary crates whose build scripts write into the source tree), the canonical
  recipe is: **mount read-only → `cp -a /src /tmp/build` → `cargo build` with `CARGO_TARGET_DIR`
  on a known-writable path.** This sidesteps the unverified mount-writability assumption entirely.
- Function body: `cargo build --release --bin modal_runner` (logs → stderr), write input to
  `/tmp/in.json`, exec `…/release/modal_runner --entrypoint <name> --input-file /tmp/in.json`,
  capture stdout, return the envelope string. Body **never** mutates correctness based on cache state.
- `timeout=1800` (digest: 300 s default is too low for cold-start + first full compile + crate
  downloads). Configurable per invocation. Do NOT call `vol.reload()` mid-build.

### 2.5 `deploy` path shim (`deploy_app.py`) + `call` path (`call_app.py`)

- Deploy image built at IMAGE-BUILD time:
  `from_registry(f"rust:{RUST_VER}-slim", add_python="3.12").entrypoint([])`
  `.env({"RUST_BACKTRACE":"1"})`
  `.add_local_dir(MANIFEST_DIR, "/app", copy=True, ignore=["target",".git",".modal-rust"])`
  `.run_commands("cd /app && cargo build --release --bin modal_runner")`
  `.run_commands("cp /app/target/release/modal_runner /app/modal_runner && chmod +x /app/modal_runner")`.
- Deployed `@app.function` body writes input to `/tmp/in.json` and execs **only**
  `/app/modal_runner --entrypoint <name> --input-file /tmp/in.json`. It **never** invokes cargo and
  **no cargo-cache Volume is mounted**. Proof obligation (AGENTS.md): `cargo build` appears in
  deploy/build logs and is ABSENT from call logs.
- `add_python` is still required (the deployed Function image must host Modal's Python runtime even
  though the workload binary is native).
- Optional hardening (documented, not v0-default): a dependency-prebuild layer (copy
  `Cargo.toml`/`Cargo.lock` + stub, build deps, then copy real src) to blunt cascading rebuilds; and
  a `--vendor` flag (`cargo vendor` into the `copy=True` context) for hermetic builds if a target
  account restricts build-time egress.
- **[CHANGED — Modal review HIGH #6] `call_app.py` arg routing.** The call logic lives inside a
  `@app.local_entrypoint()` (so the auto-CLI binds flags), e.g.
  `def main(entrypoint: str, input_json: str): print(modal.Function.from_name(APP, "call_entrypoint").remote(entrypoint, input_json))`.
  The original module-scope `print(fn.remote(entrypoint, input_json))` would `NameError`. Equivalently
  the default `call` may be driven purely from the Rust client via `Function.from_name(...).remote()`.
- Invoked argument and return value are plain `str` (the JSON envelope text) — well under the
  ~100 MB limit and inside cloudpickle/CBOR-safe scalar territory. Large I/O must route via a Volume/
  object storage (out of scope for the `add` POC, documented as a boundary).
- Web endpoint is **opt-in, authenticated** only — never auto-exposed public on deploy.

### 2.6 Build-profile constraint **[CHANGED — Rust review MED]**

`panic = "unwind"` is required for `catch_unwind` to upgrade a panic into a structured envelope.
A user's `[profile.release] panic = "abort"` would silently degrade the `panic` kind into a raw
process abort. Mitigation: `modal-rust doctor --rust` detects `panic = "abort"` in the resolved
release profile and warns/fails; and/or the runner is built under a dedicated cargo profile (or a
`--config` override) that forces `panic = "unwind"` independent of the user's release profile. M0
acceptance asserts the build is NOT `panic = "abort"`.

### 2.7 CLI surface

- `modal-rust doctor [--rust]`: preflight `~/.modal.toml`/`MODAL_TOKEN_*`, `modal` CLI on `$PATH`,
  pinned rust/python/image-builder versions; `--rust` adds `cargo`/`rustc`/`target` + `panic=abort`
  checks. Missing prerequisites produce an actionable structured error (reuses the M0 error model).
- `modal-rust run <entrypoint> [--input <json|@file>] [--gpu] [--timeout]`: generate `dev_app.py`
  under gitignored `.modal-rust/generated/`, then `modal run`.
- `modal-rust deploy <entrypoint> [--gpu] [--app-name]`: generate `deploy_app.py`, then `modal deploy`.
- `modal-rust call <entrypoint> [--input <json|@file>] [--app-name]`: generate/locate `call_app.py`
  via `modal run` (default), or behind a validated `--use-modal-rs` flag, `Function::from_name().remote()`.
- All generated shims are disposable artifacts under gitignored `.modal-rust/generated/`. The CLP is a
  pure wrapper: generated shims must be byte-equivalent (modulo injected params) to the
  M1/M4/M7/M8 shims — M9 must not become a second, divergent control path.

> **Review conflict — modal-rs vs generated Python.** The digest flags modal-rs as a HARD-BLOCKER
> risk for **authoring** (unofficial, pre-1.0, `FunctionCreate` needs a Python `function_serialized`,
> pickle protocol mismatch). **Resolution:** v0 authoring/build uses **generated Python + the official
> `modal` CLI** (the known-good control path). modal-rs is confined to the `call` invocation behind a
> validated flag. If adopted deeper later, vendor the proto. This is consistent across all three
> reviews — no conflict remains.

### 2.8 GPU tiering & `gpu=` passthrough

- `--gpu <spec>` is passed through verbatim (incl. `"H100:8"` and fallback lists). modal-rust does
  NOT validate the (drifting) catalog; it surfaces Modal's error.
- Tier 0 (default `rust:slim`, driver-only): `nvidia-smi` from Rust; cudarc driver-API execution of
  **precompiled PTX**.
- Tier 1 (`+ pip nvidia-cuda-nvrtc-cu12` / `nvidia-cuda-runtime-cu12`, or `nvidia/cuda:*-runtime-*`):
  runtime NVRTC / Burn / cubecl.
- Tier 2 (`nvidia/cuda:*-devel-*` + `add_python`): only when `nvcc` is needed.
- cudarc pinned with default `dynamic-loading` (links with no CUDA at build time). Keep container
  toolkit major ≤ host (12.x/13.x). A startup self-check should dlopen the required libs and fail
  loudly (dynamic-loading hides missing libs until runtime).
- Rust-CUDA/`rustc_codegen_nvvm` is **out of scope for v0**.

---

## 3. Milestone Plan (M0–M13)

DAG (verified acyclic): `M0 → M1`; `M1 → {M2, M3}`; `{M0, M2, M3} → M4`; `M4 → {M5, M6, M7}`;
`M7 → M8`; `{M5, M8} → M9`; `M1 → M10` (parallel GPU fork); `{M9, M10} → M11 → M12 → M13`.
`M6` (cache) is best-effort and **not** a dependency of M7 or M9. Each milestone fails for exactly
one reason.

> Review fixes folded into milestones: M1 proves CLI arg routing reaches the body (Modal HIGH #1);
> M2 keeps the mount write-probe (the single biggest unverified assumption) and labels the upload
> timing as "early signal, not a gate"; M3 asserts `which -a python python3` and the resolved
> interpreter (Modal MED #5) plus the bare-image negative control; M4 derives `CARGO_TARGET_DIR`
> from the M2 probe (local-writable default, not Volume) and adds an explicit egress-confirmation
> evidence item (sequencing nice-to-have); M6 builds local-writable, cache optional, null-result
> escape hatch; M11 reuses the exact M4/M7 build recipe with `gpu=` as the only new variable.

### M0 — Local dispatcher + runner contract (no Modal). risk: low. depends_on: []
- **validates:** the full runner protocol locally — name → typed wrapper → bytes/JSON in → JSON out,
  with all five error kinds, exit codes, panic capture, and the frozen envelope — before any network.
- **acceptance:** workspace exists (`crates/modal-rust-runtime` + `examples/add`) with
  `Registry::new().function("add", typed!(add))`. `modal_runner --entrypoint add --input-json
  '{"a":40,"b":2}'` → `{"ok":true,"value":{"sum":42}}` on stdout, exit 0. All FIVE kinds exercised:
  `unknown_entrypoint`, `decode_error` (malformed JSON AND wrong-shape JSON), `function_error`,
  `encode_error`, `panic` — each with the exact schema and a non-zero exit. Precedence test:
  malformed JSON + bad entrypoint → `decode_error`. Build asserted NOT `panic = "abort"`.
  `cargo fmt --check`, `clippy -D warnings`, `cargo test --workspace` all pass.
- **evidence:** captured stdout + exit code for success and each of the five error kinds; precedence
  test; `cargo test` green; clippy/fmt clean.
- **spike_commands:**
  - `cargo build -p modal-rust-runtime --bin modal_runner`
  - `…/target/debug/modal_runner --entrypoint add --input-json '{"a":40,"b":2}'`
  - `…/modal_runner --entrypoint nope --input-json '{}'; echo "exit=$?"`
  - `…/modal_runner --entrypoint add --input-json 'not-json'; echo "exit=$?"`
  - `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --workspace`

### M1 — Generated Modal Function runs a shell command (control path, no Rust). risk: low. depends_on: [M0]
- **validates:** the Python-shim control plane end to end (Modal auth, app authoring, subprocess,
  result marshalling) AND that a CLI-passed argument reaches the function body via a
  `@app.local_entrypoint()`.
- **acceptance:** generated `dev_app.py` defines a normal `@app.function` (no web, no Sandbox) whose
  body runs `subprocess.run(['uname','-a'])` and returns stdout; a `@app.local_entrypoint()` parses
  `--cmd` and forwards it via `.remote()`. `modal run dev_app.py` prints the remote `uname -a`
  (Linux/x86_64). A CLI-passed value is echoed back, proving arg routing. A failing-command variant
  shows captured non-zero exit + stderr (not dropped).
- **evidence:** `modal run` console output (Linux … x86_64); echoed CLI arg; failing-variant output;
  the generated shim contents + exact CLI invocation.
- **spike_commands:**
  - `modal run /Users/nicolas/devel/modal-rust/workpads/prototype/dev_app.py --cmd 'uname -a'`
  - `modal run /Users/nicolas/devel/modal-rust/workpads/prototype/dev_app.py --cmd 'false'`

### M2 — Source mount via `add_local_dir(copy=False)`. risk: medium. depends_on: [M1]
- **validates:** local source mounts at startup (not a layer), is visible at the remote path, ignore
  patterns applied client-side, content byte-identical; AND resolves mount writability.
- **acceptance:** `…add_local_dir(local_src, '/workspace', copy=False, ignore=['target','.git'])`;
  body runs `find /workspace -maxdepth 2` and `sha256sum /workspace/Cargo.toml`. Remote `find` lists
  the tree with `target/`/`.git` ABSENT. Remote sha256 == local sha256. **Write-probe** in the same
  run (`touch /workspace/.write_probe`) records writable vs read-only (EROFS) — this gates M4's build
  location. Wall-clock recorded as an **early signal, not a gate**.
- **evidence:** remote `find` output; side-by-side sha256 (equal); write-probe result; recorded
  wall-clock + approx uploaded bytes.
- **spike_commands:**
  - `shasum -a 256 /Users/nicolas/devel/modal-rust/examples/add/Cargo.toml`
  - `modal run /Users/nicolas/devel/modal-rust/workpads/prototype/dev_app.py::mount_probe`

### M3 — Rust toolchain image with Modal's Python requirement satisfied. risk: medium. depends_on: [M1]
- **validates:** one image hosts both the Rust toolchain AND Modal's runtime; base ENTRYPOINT doesn't
  break the runtime; `add_python` and any system `python3` coexist cleanly.
- **acceptance:** `from_registry('rust:1.<pinned>-slim', add_python='3.12')`; a normal `@app.function`
  returns `cargo --version`, `rustc --version`, `python --version`, AND `which -a python python3` with
  the resolved interpreter path. All version strings non-empty; cargo + python both on `$PATH`. The
  Function STARTS cleanly (else record the `entrypoint([])` workaround). Negative control: bare
  `from_registry('rust:1.<ver>-slim')` WITHOUT `add_python` is shown to fail as a Function image
  (or, if it unexpectedly works, that is recorded). Pins recorded: rust tag, `add_python` value,
  `MODAL_IMAGE_BUILDER_VERSION`.
- **evidence:** remote stdout (cargo/rustc/python versions + `which -a`); clean start (or workaround);
  negative-control result; recorded pins.
- **spike_commands:**
  - `modal run /Users/nicolas/devel/modal-rust/workpads/prototype/dev_app.py::toolchain_probe`
  - `modal run /Users/nicolas/devel/modal-rust/workpads/prototype/dev_app_no_python.py::toolchain_probe`

### M4 — RUNTIME COMPILE without Sandbox (the key validation). risk: high. depends_on: [M0, M2, M3]
- **validates:** THE central claim — a normal `@app.function` can `cargo build` the mounted source in
  its body, exec the freshly built `modal_runner`, and `add(40,2)` → 42, end to end.
- **acceptance:** on the M3 image with the M2 mount: build location derived from the M2 write-probe
  (copy `/src` to `/tmp/build` and/or `CARGO_TARGET_DIR=/tmp/target` if read-only; in-place only if
  writable — **local-writable, not Volume**). The **first observable build step is a successful
  crates.io index/download** (explicit egress-confirmed evidence). `cargo build --bin modal_runner`,
  then exec via the M0 protocol. `modal run dev_app.py::run_add --input '{"a":40,"b":2}'` →
  `{"ok":true,"value":{"sum":42}}`. Build log lines appear in the SAME invocation (compile at exec
  time). A deliberate failing entrypoint propagates as a Modal failure / structured error (not silent
  success). `timeout=1800` recorded. Cold first-build wall-clock recorded (M6 baseline).
- **evidence:** single `modal run` log showing BOTH cargo output AND the 42 result; the
  build-output-location decision tied to the M2 probe; egress-confirmed download line; failure-
  propagation evidence; cold wall-clock + timeout used.
- **spike_commands:**
  - `modal run /Users/nicolas/devel/modal-rust/workpads/prototype/dev_app.py::run_add --input '{"a":40,"b":2}'`
  - `modal run /Users/nicolas/devel/modal-rust/workpads/prototype/dev_app.py::run_add --entrypoint will_panic --input '{}'`

### M5 — Source-edit reactivity. risk: low. depends_on: [M4]
- **validates:** the dev loop reflects local edits with no redeploy — `copy=False` re-uploads current
  source each run.
- **acceptance:** `add(40,2)` → 42; edit `add` locally (return `a+b+1`), re-run → 43; revert, re-run
  → 42. No `modal deploy`, no image rebuild, no manual cache busting between the three runs.
- **evidence:** three consecutive `modal run` outputs (42 / 43 / 42) with timestamps; `git diff`
  confirming only source bytes changed.
- **spike_commands:**
  - `modal run …/dev_app.py::run_add --input '{"a":40,"b":2}'`
  - `sed -i '' 's/a + b/a + b + 1/' …/examples/add/src/lib.rs && modal run …/dev_app.py::run_add --input '{"a":40,"b":2}'`
  - `git -C /Users/nicolas/devel/modal-rust checkout -- examples/add/src/lib.rs && modal run …/dev_app.py::run_add --input '{"a":40,"b":2}'`

### M6 — Cargo-cache Volume (best-effort dev-iteration speedup). risk: medium. depends_on: [M4]
- **validates:** a Volume holding `CARGO_HOME` (+ optionally `CARGO_TARGET_DIR`) persists across
  invocations so a warm rebuild is materially faster — and a cache miss only costs time, never a wrong
  result. **Not** a dependency of deploy.
- **acceptance:** `Volume.from_name('modal-rust-cargo-cache', create_if_missing=True)` at a STABLE
  path; `CARGO_HOME` on the Volume; `CARGO_TARGET_DIR` may be promoted to the Volume only if it
  benchmarks net-positive and lock-safe (default stays `/tmp/target`). Toolchain + mount path held
  constant. Cold (empty volume) vs second-run (no source change) wall-clocks recorded; warm is
  meaningfully faster OR the null result is recorded as the deliverable. Relies on automatic
  background/shutdown commits; **no `vol.reload()` on the hot path**. Correctness unchanged (42 both
  runs). Documented cache reset (`modal volume rm` / new name). Single-writer/low-concurrency
  documented; parallel shared-cache writes out of scope for v0 (last-write-wins noted).
- **evidence:** two wall-clocks (cold vs warm) + speedup/null; both returned 42; `modal volume list` +
  documented reset command.
- **spike_commands:**
  - `modal volume create modal-rust-cargo-cache`
  - `modal run …/dev_app.py::run_add_cached --input '{"a":40,"b":2}'` (×2)
  - `modal volume list`

### M7 — Deploy-time build (`copy=True` + `run_commands` cargo build, bake `/app/modal_runner`). risk: high. depends_on: [M4]
- **validates:** source copied into an image LAYER, `cargo build --release` at IMAGE-BUILD time via
  `run_commands` with crates.io egress, binary baked into the image.
- **acceptance:** deploy image = `…add_local_dir(src,'/app/src',copy=True).run_commands('cd /app/src &&
  cargo build --release --bin modal_runner','cp …/release/modal_runner /app/modal_runner')`;
  `add_python='3.12'` present. Image build SUCCEEDS, proving build-time egress (verified-by-docs;
  re-confirm on this account). If it fails, the `--vendor` (`cargo vendor`) fallback is applied and
  recorded. A throwaway run confirms `/app/modal_runner` exists, is executable, and returns 42 when
  exec'd directly. The dependency-prebuild caching trick is documented as the cascading-rebuild
  mitigation.
- **evidence:** image-build log showing `cargo build` compiling at BUILD time + the `cp`; a run
  showing `/app/modal_runner` → 42; crate downloads in the build log (egress) or the recorded
  vendoring workaround.
- **spike_commands:**
  - `modal deploy /Users/nicolas/devel/modal-rust/workpads/prototype/deploy_app.py`
  - `modal run /Users/nicolas/devel/modal-rust/workpads/prototype/deploy_app.py::probe_binary`

### M8 — Deployed runtime does NOT compile (the deploy invariant). risk: medium. depends_on: [M7]
- **validates:** the deployed body only EXECs `/app/modal_runner` and never invokes cargo —
  `cargo build` in deploy/build logs, ABSENT from call logs; result stable until explicit redeploy.
- **acceptance:** `modal deploy`; runtime body execs `/app/modal_runner --entrypoint add` (no cargo,
  no source mount, no cache Volume). A call (`Function.from_name(...).remote()` or `modal run`) → 42;
  CALL logs contain NO compilation/cargo lines. Stability: repeated calls → 42; editing local source
  does NOT change the deployed result. Redeploy reactivity: change to `a+b+1`, `modal deploy`, calls →
  43, with the new cargo build only in the NEW deploy's build logs. Negative check: `which cargo` in
  the runtime body fails, or (if the toolchain image still carries cargo) the body provably never
  calls it.
- **evidence:** deploy/build log with cargo vs CALL log without (side by side); sequence
  42 → (edit) 42 → (redeploy) 43 with timestamps; runtime-body cargo-not-invoked check.
- **spike_commands:**
  - `modal deploy …/deploy_app.py`
  - `modal run …/call_app.py::call_deployed --input '{"a":40,"b":2}'`
  - `sed -i '' 's/a + b/a + b + 1/' …/examples/add/src/lib.rs && modal run …/call_app.py::call_deployed --input '{"a":40,"b":2}'`
  - `modal deploy …/deploy_app.py && modal run …/call_app.py::call_deployed --input '{"a":40,"b":2}'`

### M9 — modal-rust CLI wraps the shims (run/deploy/call/doctor). risk: medium. depends_on: [M5, M8]
- **validates:** the public UX — one `modal-rust` binary generates shims and orchestrates the build
  stage; the user never touches Modal Python. Introduces NO new Modal capability (pure wrapper).
- **acceptance:** `modal-rust run add --input '{"a":40,"b":2}'` reproduces M4/M5; `modal-rust deploy
  add` reproduces M7/M8; `modal-rust call add --input '{"a":40,"b":2}'` → 42; `modal-rust doctor`
  reports cargo, modal CLI/creds, pinned versions, and `panic=abort` detection, with actionable
  structured errors. Generated shims remain private (gitignored) and are byte-equivalent (modulo
  injected params) to the M1/M4/M7/M8 shims. `cargo fmt`/`clippy`/`test` clean for the CLI crate.
- **evidence:** transcripts of `run`/`deploy`/`call` returning expected results; `doctor` output here
  (+ a simulated-missing-prereq run); confirmation the generated shim matches the prior shims.
- **spike_commands:**
  - `cargo run -p modal-rust-cli -- doctor`
  - `cargo run -p modal-rust-cli -- run add --input '{"a":40,"b":2}'`
  - `cargo run -p modal-rust-cli -- deploy add`
  - `cargo run -p modal-rust-cli -- call add --input '{"a":40,"b":2}'`

### M10 — GPU `nvidia-smi` from the Python shim (Tier 0 sanity). risk: low. depends_on: [M1]
- **validates:** `gpu=` placement lands on a real NVIDIA GPU (driver + `nvidia-smi` preinstalled),
  proven from Python before any Rust/CUDA. Can run in PARALLEL with the prototype chain.
- **acceptance:** `@app.function(gpu='T4')` body runs `subprocess.run(['nvidia-smi'])` and returns it;
  output shows a GPU + driver + CUDA Driver API version. `gpu=` passthrough exercised (validate `'T4'`
  parses; a bad type surfaces Modal's error — catalog NOT re-implemented). No CUDA toolkit installed
  (Tier 0). GPU cost recorded.
- **evidence:** remote `nvidia-smi` output; confirmation no toolkit added; recorded GPU type + cost.
- **spike_commands:**
  - `modal run /Users/nicolas/devel/modal-rust/workpads/gpu-compute/gpu_app.py::smi_py`

### M11 — `nvidia-smi` from a Rust function (Tier 0, no CUDA crate). risk: low. depends_on: [M9, M10]
- **validates:** Rust can observe the GPU on Modal — `modal_runner` on a GPU-attached Function shells
  out to `nvidia-smi`. The ONLY new variable vs M4/M7 is `gpu=` placement; the build path is the exact
  M4/M7 recipe (CPU-proven), isolating one boundary.
- **acceptance:** a `gpu_info` entrypoint runs `std::process::Command::new("nvidia-smi")` and returns
  it via the M0 envelope. On a `gpu='T4'` Function (via the M9 CLI), the result matches M10 — now
  produced BY RUST. No CUDA crate in the tree (`cargo tree` shows no cudarc); image still Tier 0.
  Acceptance states explicitly: build path identical to M4/M7; sole new variable is `gpu=`.
- **evidence:** JSON-enveloped Rust result with `nvidia-smi` output; `cargo tree`/`Cargo.lock` showing
  no CUDA dep; build/run command + recorded GPU cost.
- **spike_commands:**
  - `cargo run -p modal-rust-cli -- run gpu_info --gpu T4 --input '{}'`

### M12 — Real Rust CUDA vector add (cudarc, precompiled PTX, driver-only). risk: high. depends_on: [M11]
- **validates:** genuine Rust GPU COMPUTE with the leanest stack — cudarc + `dynamic-loading` runs a
  **precompiled PTX** vector-add through the Driver API, needing only `libcuda` (Tier 0). No nvcc, no
  NVRTC at runtime.
- **acceptance:** cudarc added with `-F dynamic-loading` (links with no CUDA at build time); kernel
  shipped as PTX (checked-in or generated at deploy/image-build in a Tier 2 builder), NOT NVRTC at
  runtime. `c[i]=a[i]+b[i]` on `gpu='T4'` verified element-wise vs a CPU reference. PTX (driver-JIT,
  forward-compatible) not a fixed-arch cubin. Startup self-check loads `libcuda` and fails loudly on
  misconfig. cuda-feature pin (or `fallback-latest`) vs Modal's host driver recorded (do NOT hardcode
  the point-in-time driver version).
- **evidence:** verified vector-add (computed vs CPU-reference, equal); confirmation runtime image is
  Tier 0 and the kernel was precompiled PTX; `cargo tree` showing cudarc dynamic-loading; recorded
  cuda feature pin + GPU cost; where the PTX was generated.
- **spike_commands:**
  - `cargo run -p modal-rust-cli -- run cuda_vector_add --gpu T4 --input '{"n":1024}'`

### M13 — Burn tensor smoke (Tier 1, CUDA-runtime image). risk: high. depends_on: [M12]
- **validates:** the downstream consumer — a Burn (burn-cuda/cubecl) tensor op runs on a Modal GPU
  Function, which REQUIRES a Tier 1 image (CubeCL JIT-compiles via NVRTC at runtime).
- **acceptance:** Tier 1 image: `nvidia/cuda:<12.x|13.x>-runtime-<os>` + `add_python='3.12'`, OR
  Tier 0 + pip `nvidia-cuda-nvrtc-cu12` + `nvidia-cuda-runtime-cu12` so `libnvrtc.so`/`libcudart.so`
  are on the loader path. A minimal Burn CUDA-backend tensor add on `gpu='T4'` verified correct.
  `burn`, `burn-cuda`, `cubecl`, `cudarc` versions pinned TOGETHER and recorded. Container CUDA major
  ≤ host (12.x/13.x). A startup self-check dlopens `libnvrtc` + `libcudart` as a HARD gate (fail loudly
  if accidentally Tier 0).
- **evidence:** verified Burn tensor-add result; the Tier 1 recipe (runtime tag or exact pip wheels) +
  confirmation `libnvrtc`/`libcudart` present; pinned versions + CUDA major; recorded GPU cost.
- **spike_commands:**
  - `cargo run -p modal-rust-cli -- run burn_add --gpu T4 --input '{"n":256}'`

---

## 4. Open Questions For The User (each has a recommended default)

None of these block engineering from starting M0–M3; they shape later milestones.

1. **GPU / cost confirmation.** GPU milestones (M10–M13) and even cold full cargo builds cost real
   money on Modal. **Recommended default:** require an explicit `--yes` confirmation flag for
   `modal-rust run --gpu` and for `modal-rust deploy` (persistent app), with a per-run cost note.
   *Option to override:* set a budget ceiling / disable confirmation.
2. **Public deploys / auth.** Deployed web endpoints are public-by-default unless proxy-auth is on.
   **Recommended default (a):** NO web endpoint in v0 — callable only via `Function.from_name().remote()`.
   *Options:* (b) opt-in authenticated endpoint; (c) public endpoint (not recommended).
3. **Default `call` invoke mode.** **Recommended default:** generated `call_app.py` via `modal run`
   (known-good) for v0; wire modal-rs `Function::from_name().remote()` behind an explicit
   `--use-modal-rs` flag, promoted to default only after a smoke test of a non-scalar round-trip.
4. **Wire format.** **Recommended default:** JSON for v0 (text envelope on stdout; the `Handler`
   trait is already codec-neutral on `&[u8]`, so a CBOR/msgpack `Codec` + `--input-format` is additive
   later for large/binary payloads). *Option:* plan a CBOR path now if early use cases pass binary data.
5. **Rust toolchain pin.** **Recommended default:** `from_registry('rust:1.83-slim', add_python='3.12')`
   as the single image backing both run and deploy (3.12 is the only doc-by-example value, hence lowest
   risk). *Option:* specify a required MSRV / different pin.
6. **Cache sharing / concurrency.** **Recommended default:** a single shared `modal-rust-cargo-cache`
   (fine for one developer; matches "avoid >5 concurrent commits"). *Option:* per-user/per-project
   sharded cache names and/or Volume v2 if multiple developers or parallel runs are expected.

---

## 5. Residual Risks

1. **Runtime-compile + build-time egress (HARD, validated in M4/M7).** Both the run-path body building
   in a normal Function and the deploy-path `run_commands` reaching crates.io are central. Build-time
   egress is verified-by-docs (Modal `run_commands` does `git clone`/`apt`); re-confirm on the target
   account. Fallback: implemented `--vendor` (`cargo vendor`) hermetic build.
2. **Mount writability (M2 probe gates M4).** `add_local_dir(copy=False)` mount permissions are
   undocumented. The design sidesteps it (build into a local-writable path; copy `/src` to scratch if
   read-only), so the risk is contained to a one-line empirical probe in M2.
3. **Cascading rebuild on deploy.** Any change in the `copy=True` source layer busts that and all later
   layers → full cargo build per deploy (no incremental, no cache Volume on deploy). Mitigation
   (dependency-prebuild layer) is documented; large workspaces could also hit undocumented image-build
   time limits.
4. **Cold-start build latency on the run path.** No warm cache → cold start + crate download + full
   compile may approach the 1800 s timeout for large dep graphs. The Volume cache mitigates warm runs,
   but network-FS latency on Cargo's many small stat/reads may erase the speedup — M6 must benchmark;
   a null result is acceptable and must NOT block deploy.
5. **modal-rs adoption.** Unofficial, single-maintainer, pre-1.0, vendored proto that can drift;
   `FunctionCreate` needs a Python `function_serialized` (does NOT remove the Python shim); serde-pickle
   protocol 2/3 vs cloudpickle 4. Mitigation: v0 uses generated Python + the official `modal` CLI for
   authoring/build; modal-rs confined to `call` behind a flag; vendor the proto if adopted deeper.
6. **GPU footgun + drift.** Burn/cubecl on a driver-only image fails at RUNTIME (`libnvrtc` missing)
   even though cudarc compiles fine (dynamic-loading hides the gap). The tier plan + startup self-check
   address this. Host driver/CUDA versions drift and cap the max toolkit major — re-verify, never
   hardcode.
7. **Protocol-freeze pressure.** The five error kinds + stdout-only-envelope are the cross-version seam.
   The codec-neutral `&[u8]` trait, reserved `typed_async`, frozen named-object argument shape, and the
   reserved optional `meta`/`version` field make GPU/PyO3/macro phases **additive**. Any change must be
   additive-only and reviewed against the manual-registry runner per WORKING.md.
8. **Payload ceiling.** `.remote()` str args hit the ~100 MB gRPC limit; `--input-file` covers the
   runner side but the shim↔client hop is still bounded. Large I/O must route via a Volume/object
   storage (out of scope for the `add` POC; documented as a boundary).
9. **`add_python` coexistence (M3).** If a system `python3` in `rust:slim` shadows the standalone
   `add_python` build, the runtime could pick the wrong interpreter. M3 asserts `which -a` + the
   resolved path to catch this.

---

## 6. Review Reconciliation Summary

- **All three reviews returned `sound-with-changes`.** No review returned a blocking verdict.
- **HIGH-severity `must_fix` items folded in (all addressed):**
  - Modal #1 — `modal run` flag binding requires `@app.local_entrypoint()` → §2.4, M1.
  - Modal #2 — do not build into a Volume target dir by default → §2.4, M4, M6.
  - Modal #6 — `call_app.py` module-scope `NameError` → use `local_entrypoint` → §2.5.
  - Rust #4 — reserve the async path (`typed_async`) now → §2.3.
  - Rust #5 — codec-neutral, static-dispatch `HandlerFn = fn(&[u8]) -> Result<Vec<u8>, RunnerError>` (no `Box<dyn>`) → §2.3, amended by §0.3.
  - Rust #3 — output encode failure must not be `panic`; add `encode_error` → §2.2.
- **MEDIUM items also folded:** read-only-mount copy-to-scratch recipe (§2.4); `panic=abort` doctor
  check + dedicated unwind profile (§2.6); duplicate-name rejection and frozen named-object argument
  shape (§2.3); M3 `which -a` interpreter assertion; M7 egress re-ranked with implemented `--vendor`
  fallback.
- **Conflicts resolved explicitly:** (a) clap is CLI-only, runner uses a hand-rolled parser (§2.1);
  (b) modal-rs vs generated Python — generated Python for authoring, modal-rs only for `call` behind a
  flag (§2.7) — consistent across all reviews.
- **Verdict:** with every HIGH-severity `must_fix` addressed in the locked decisions, the plan is
  sound to proceed. The Section 4 questions carry recommended defaults and are not blockers.
