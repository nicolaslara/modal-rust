# Spec: make the EXAMPLES showcase macro ergonomics (behavior-preserving)

Status: build-ready design. No code written yet. Every change below is
*example-surface + registration-mechanism* only — it does NOT touch
`crates/modal-rust-runtime`, the SDK invoke logic, or `crates/modal-rust-macros`.
The runner CLI protocol, `typed!` behavior, `Registry::from_inventory` dispatch,
the 5 error kinds, the FILE-mode wire, and the decorator semantics
(gpu/timeout/cache/secrets/volumes) are all UNCHANGED.

## 0. Ground-truth facts established by reading the current sources

- **Template** (the converted form to mirror exactly):
  - `examples/add-macro/Cargo.toml:30` — single modal dep
    `modal_rust_facade = { path = "../../crates/modal-rust", package = "modal-rust" }`
    plus `serde`, `serde_json`, `anyhow`. NO `modal-rust-runtime`, NO `inventory`.
  - `examples/add-macro/src/lib.rs:22` — `extern crate modal_rust_facade as modal_rust;`
    so the attribute is spelled `#[modal_rust::function]`.
  - `examples/add-macro/src/bin/modal_runner.rs:13,18,25-27` — the inventory runner:
    ```rust
    use example_add_macro as _;                       // link the inventory submissions
    use modal_rust_facade::__private::runtime;         // frozen runner via the facade
    fn main() -> std::process::ExitCode {
        let (registry, configs) = runtime::from_inventory_with_configs();
        let code = runtime::run_cli_with_configs(registry, &configs);
        std::process::ExitCode::from(code as u8)
    }
    ```
    `from_inventory_with_configs` threads each decorator's `FunctionConfig` into the
    additive `--describe` manifest; the frozen `--entrypoint` dispatch ignores it.

- **Manual reference (DO NOT TOUCH):** `examples/add/src/lib.rs:152-160` keeps the
  hand-written `modal_registry()` + `typed!(add)`; `examples/add/Cargo.toml:17` keeps
  the direct `modal-rust-runtime` dep. The `macro_path_byte_identical_to_manual` proof
  lives in `examples/add-macro/src/lib.rs:288-302` and stays green.

- **Macro classifier (reference only):** `crates/modal-rust-macros/src/lib.rs:314-336,
  572-595` — Mode A (EXPLICIT, byte-identical to manual `typed!`) is triggered by a
  *single param whose type is a bare non-generic, non-scalar `Type::Path`*. Both
  `vector_add(input: VectorAddInput)` and `burn_add(input: BurnAddInput)` are Mode A,
  so adding `#[modal_rust::function(...)]` to them emits the UNCHANGED fn + `typed!(fn)`
  + an `inventory::submit!{ Registration { name, handler, config } }`. The body and the
  I/O types are byte-identical; only the registration mechanism + the recorded config
  change.

- **GPU string + entrypoint name come from the live tests (copy EXACTLY):**
  - cuda: `live_gpu.rs:83` → `#[modal_rust::function(gpu = "T4", name = "vector_add")]`,
    `PACKAGE = "example-cuda-vector-add"` (`live_gpu.rs:51`).
  - burn: `live_burn.rs:106` → `#[modal_rust::function(gpu = "T4", name = "burn_add")]`,
    `PACKAGE = "example-burn-add"` (`live_burn.rs:64`).

- **Duplicate-name panic (the load-bearing constraint for the live-test edit):**
  `from_inventory_with_configs` / `Registry::function`
  (`crates/modal-rust-runtime/src/lib.rs:379-384, 410-418`) PANIC if two inventory
  submissions share a name. So within ONE linked test binary there must be EXACTLY ONE
  `vector_add` / `burn_add` registration — the converted crate's real decorated fn OR
  the test's stub, never both.

- **`modal-rust` does NOT depend on the cuda/burn crates today** (verified: only
  `example-add` + `example-add-macro` are dev-deps in `crates/modal-rust/Cargo.toml`).
  The live tests' decorated STUBS are currently the ONLY inventory source of
  `vector_add`/`burn_add` in those test binaries.

- **`App::config_for` is `pub(crate)`** (`crates/modal-rust/src/app.rs:194`) — orchestrate
  CANNOT read a decorator config off an `App`. The offline gpu-config proof is therefore
  the converted crate's `modal_runner --describe`, not an orchestrate assertion.

- **burn-add BUILDS WITHOUT CUDA on this host:** `cargo check -p example-burn-add`
  finished `dev` in ~16s (cudarc dynamic-loading + CubeCL link with no CUDA toolkit).
  So burn-add can be `cargo build -p example-burn-add` here; the check-only fallback is
  not needed (kept documented in §7 as the contingency).

---

## 1. burn-add + cuda-vector-add → self-describing decorator form

### 1a. cuda-vector-add

**`examples/cuda-vector-add/src/lib.rs`**

1. Add the facade alias at the top of the file (mirrors add-macro/src/lib.rs:22), and
   DROP the `modal_rust_runtime` import line (`:18`):
   - Remove: `use modal_rust_runtime::{typed, Registry};`
   - Add, before the `use serde::...` line:
     ```rust
     // Alias the facade crate so the attribute is spelled `#[modal_rust::function]`;
     // the macro routes every runtime/inventory path through `modal_rust::__private::…`,
     // so this crate's only modal dep is the `modal-rust` facade.
     extern crate modal_rust_facade as modal_rust;
     ```
2. Decorate the REAL fn (`vector_add`, currently `:136`) — copy the gpu + name from
   `live_gpu.rs:83`:
   ```rust
   #[modal_rust::function(gpu = "T4", name = "vector_add")]
   pub fn vector_add(input: VectorAddInput) -> anyhow::Result<VectorAddOutput> {
   ```
   The body, `VectorAddInput`, `VectorAddOutput`, the PTX const, the self-check, and the
   helper fns are UNCHANGED. (Mode A: single bare user-struct param → byte-identical
   `typed!(vector_add)` + an `inventory::submit!` carrying `FunctionConfig{ gpu:
   Some("T4"), .. }`.)
3. DELETE `modal_registry()` (`:243-247`) — see §3 (single registration mechanism =
   inventory).
4. Update the in-crate test `registry_has_vector_add` (`:284-289`) — see §3.

**`examples/cuda-vector-add/src/bin/modal_runner.rs`** — replace the whole body with the
add-macro runner (mirror `examples/add-macro/src/bin/modal_runner.rs` EXACTLY, just the
crate name differs):
```rust
//! The runner binary for the M12 cuda-vector-add example (boundaries.md §1.4).
//! The user does NOT own `main()`. The registry is assembled from the macro's
//! `inventory` submissions via `from_inventory_with_configs()` (decorator gpu rides
//! into `--describe`); both converge on the UNCHANGED `run_cli`.

use example_cuda_vector_add as _;            // link the inventory submission
use modal_rust_facade::__private::runtime;

fn main() -> std::process::ExitCode {
    let (registry, configs) = runtime::from_inventory_with_configs();
    let code = runtime::run_cli_with_configs(registry, &configs);
    std::process::ExitCode::from(code as u8)
}
```

### 1b. burn-add

Identical shape. **`examples/burn-add/src/lib.rs`**:

1. Drop `use modal_rust_runtime::{typed, Registry};` (`:26`); add the
   `extern crate modal_rust_facade as modal_rust;` alias (next to the other `use`s).
   Keep `use burn_cuda::{Cuda, CudaDevice};` and `use serde::…` unchanged.
2. Decorate the REAL `burn_add` (currently `:116`) — copy from `live_burn.rs:106`:
   ```rust
   #[modal_rust::function(gpu = "T4", name = "burn_add")]
   pub fn burn_add(input: BurnAddInput) -> anyhow::Result<BurnAddOutput> {
   ```
   Body / `BurnAddInput` / `BurnAddOutput` / `tier1_self_check` UNCHANGED (Mode A).
3. DELETE `modal_registry()` (`:178-180`).
4. Update the in-crate test `registry_has_burn_add` (`:192-197`) — see §3.

**`examples/burn-add/src/bin/modal_runner.rs`** — replace body with the inventory runner
(mirror add-macro; crate is `example_burn_add`):
```rust
use example_burn_add as _;
use modal_rust_facade::__private::runtime;

fn main() -> std::process::ExitCode {
    let (registry, configs) = runtime::from_inventory_with_configs();
    let code = runtime::run_cli_with_configs(registry, &configs);
    std::process::ExitCode::from(code as u8)
}
```

> Note on the `as _` import: the lib `path = "src/lib.rs"` and the bin both live in the
> same package, but the lib is a SEPARATE crate (`name = "example_cuda_vector_add"` /
> `example_burn_add`), so the runner must `use <crate> as _;` to pull its
> `inventory::submit!` link-section into the binary — exactly as add-macro does
> (`examples/add-macro/src/bin/modal_runner.rs:13`).

---

## 2. Cargo.toml dep removals (single `modal-rust` facade dep each)

Both crates currently have (cuda `:16-21,29-38`, burn `:16-25,41-50`):
```toml
modal-rust-runtime = { path = "../../crates/modal-rust-runtime" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
# ... (cudarc / burn / burn-cuda / libloading kept verbatim)
```

**Edit (both):** replace the `modal-rust-runtime` line with the facade dep, mirroring
`examples/add-macro/Cargo.toml:30`:
```toml
# SINGLE modal dependency: the `modal-rust` facade (renamed `modal_rust_facade` via
# Cargo's `package = "..."`). The `#[modal_rust::function]` macro routes every emitted
# runtime/inventory path through this facade, so this crate needs NEITHER
# `modal-rust-runtime` NOR `inventory` as a direct dependency.
modal_rust_facade = { path = "../../crates/modal-rust", package = "modal-rust" }
```
KEEP unchanged in **both**: `serde`, `serde_json`, `anyhow`.
KEEP unchanged in **cuda**: the whole `[dependencies.cudarc]` block (`:29-38`).
KEEP unchanged in **burn**: `libloading = "0.8"` (`:25`), `[dependencies.burn]`
(`:41-44`), `[dependencies.burn-cuda]` (`:48-50`), and the pinned-version-set comment.

The `[package]`/`[lib]`/`[[bin]]` stanzas are unchanged. Optionally refresh the
`description` to mention the decorator (cosmetic; not required).

> Acyclicity check (verified): the cuda/burn crates depend ONLY on the facade `path`
> (no crate depends back on them), so `modal-rust` (path-dep on the facade) → cuda/burn
> (path-dep on the facade) is a tree, never a cycle. The facade itself does not depend
> on the example crates.

---

## 3. Registration mechanism decision — INVENTORY ONLY (no double-register)

**Decision: inventory, exactly like add-macro. DELETE `modal_registry()` from both
crates.** Rationale: the AGENTS note forbids double-registering; keeping a
hand-written `modal_registry()` AND the macro's `inventory::submit!` would register the
SAME name twice the moment any code built BOTH (e.g. a `modal_registry()` that calls
`.function("vector_add", typed!(vector_add))` while the decorator also submits
`vector_add` to inventory). Picking inventory matches the template and the runner above.

**In-crate test updates** (the only tests that referenced `modal_registry()`):

- **cuda** `examples/cuda-vector-add/src/lib.rs:284-289` `registry_has_vector_add` →
  rewrite to the inventory lookup (mirror add-macro's `from_inventory_registers_add`,
  `examples/add-macro/src/lib.rs:278-285`). The test module must import the facade
  runtime/types via `use modal_rust::Registry;` (the `extern crate … as modal_rust`
  alias makes `modal_rust::Registry` resolve through the facade re-export
  `crates/modal-rust/src/lib.rs:57`):
  ```rust
  #[test]
  fn registry_has_vector_add() {
      use modal_rust::Registry;
      let reg = Registry::from_inventory();
      assert!(reg.get("vector_add").is_some());
      assert!(reg.get("nope").is_none());
  }
  ```
  KEEP `ptx_is_embedded_and_names_the_kernel`, `sanitize_strips_comment_header_to_version`,
  `input_decodes_from_named_object` unchanged.

- **burn** `examples/burn-add/src/lib.rs:192-197` `registry_has_burn_add` → same rewrite:
  ```rust
  #[test]
  fn registry_has_burn_add() {
      use modal_rust::Registry;
      let reg = Registry::from_inventory();
      assert!(reg.get("burn_add").is_some());
      assert!(reg.get("nope").is_none());
  }
  ```
  KEEP `input_decodes_from_named_object`, `tier1_self_check_fails_loudly_on_cpu_host`.

> `Registry::from_inventory()` is the right call in the tests (not
> `from_inventory_with_configs`) — it is the public facade re-export and the lookup is
> handler-only, matching `examples/add-macro/src/lib.rs:226,282`. Adding a
> *config-asserting* test (e.g. `gpu == Some("T4")`) is OPTIONAL and would need
> `use modal_rust::{FunctionConfig, Registration}; use modal_rust::__private::inventory;`
> exactly like add-macro's `registration()` helper (`:183-191, 378-403`). Recommended
> but not required for the build gate (the `--describe` proof in §6 already shows the
> gpu). If added, it is the cleanest in-crate evidence the decorator rode through.

---

## 4. THE LIVE-TEST ADAPTATION (live_gpu.rs + live_burn.rs)

### The split, precisely

The OUTBOUND `FunctionCreate` (and the run-app create) is built from the TEST binary's
LOCAL `App::connect(...)` → `from_inventory_with_configs()` BEFORE the remote build, so
the `gpu="T4"` `FunctionConfig` for `vector_add`/`burn_add` MUST be present in the TEST
BINARY's linked inventory at create time. Today that is supplied by the decorated STUB
declared inside each test file (`live_gpu.rs:83-86`, `live_burn.rs:106-109`).

After the conversion the converted crate ALSO submits a `vector_add`/`burn_add`
registration with `gpu="T4"`. Because `from_inventory_with_configs` **panics on
duplicate names**, the test binary must contain EXACTLY ONE such registration.

### Decision: KEEP THE STUB, do NOT add the crate as a dep — minimal, zero-risk.

`modal-rust` currently does NOT dev-depend on `example-cuda-vector-add` /
`example-burn-add`, and the heavy burn crate is deliberately kept out of `modal-rust`'s
build graph (it would force CUDA-crate compiles into the facade's test build /
default-members reach). The stub already supplies the EXACT same outbound config
(`gpu="T4"`, `name="vector_add"`/`"burn_add"`). The real crate's decorator is only
*consumed remotely* (the uploaded crate's `modal_runner --describe` / its inventory is
what runs on Modal). So:

- **DO NOT change `crates/modal-rust/Cargo.toml`** (no new dev-dep on the cuda/burn
  crates). This AVOIDS the duplicate-name panic entirely (the converted crate's
  inventory submission never links into the modal-rust test binary).
- **KEEP both stubs verbatim** — `live_gpu.rs:83-86` and `live_burn.rs:106-109` stay.
  They remain the test binary's local source of the `gpu="T4"` config, so
  `App::connect(...)` → `app.function("vector_add").remote(...)` (`live_gpu.rs:146`) and
  `app.deploy_with(cfg)` / `app.call(...)` (`live_burn.rs:186,195`) produce the SAME
  outbound `Resources.gpu_config` as before. The server-side proof (decoded
  `gpu_name`/`backend`/`libnvrtc`) is UNTOUCHED.
- **Doc-comment refresh (non-load-bearing, recommended):** the stub doc-comments
  (`live_gpu.rs:78-82`, `live_burn.rs:100-105`) currently say the stub is the ONLY local
  source of config. After the conversion that is still TRUE *in this test binary* (the
  crate's decorator is not linked here), but add one clarifying clause so a future reader
  understands the redundancy is intentional and the panic is avoided by NOT depending on
  the crate. Suggested addition to each stub's doc block:
  > "The converted `example-cuda-vector-add` crate now carries its OWN
  > `#[modal_rust::function(gpu=\"T4\", name=\"vector_add\")]` decorator (consumed
  > remotely). This test binary deliberately does NOT depend on that crate — so its
  > inventory is not linked here and there is no duplicate-name panic; this local stub
  > remains the test binary's create-time source of the same `gpu=\"T4\"` config."

  This is a comment-only edit. The `#[modal_rust::function(...)]` line, the fn, and the
  rest of each test are UNCHANGED.

### Why NOT option (a) (depend on the crate, drop the stub)

Option (a) would require: add `example-cuda-vector-add` (and `example-burn-add`) as a
dev-dep of `modal-rust`, delete the stub, and rely on the linked crate's decorator. Two
problems make it worse, not cleaner:
1. **burn** would pull `cubecl-cuda`/`burn-cuda` into the `modal-rust` test build —
   burn-add is intentionally EXCLUDED from default-members for exactly this reason
   (`Cargo.toml` default-members comment). A dev-dep would drag that heavy/CUDA-adjacent
   tree into the core facade's `cargo test`. Unacceptable.
2. It removes the SELF-CONTAINED nature of each live test (today each test file fully
   declares what it creates). Keeping the stub preserves that and keeps the modal-rust
   test binary CUDA-crate-free.

So **(keep stub, no dep)** is the cleanest correct choice. Both tests still COMPILE under
`--features live` (the only deps they use — `modal_rust::{App, DeployConfig, Error}`,
`serde`, `tokio`, the re-exported `function` macro — are unchanged) and their live proof
is intact.

### Compile-gate note

`cargo build -p modal-rust --features live --tests` (offline) must still succeed. Nothing
in the §4 plan changes that — the stubs + imports are unchanged. The live `#[tokio::test]`
bodies stay `#[ignore]`; they compile offline and only run with
`--features live -- --ignored` against real Modal.

---

## 5. orchestrate — show the macro path too (keep explicit + zero-Modal default)

Goal: visibly exercise BOTH the manual `App::new(modal_registry())` path AND the
`App::from_inventory()` macro/inventory path, all in the zero-Modal `.local()` default.

**`examples/orchestrate/Cargo.toml`** — add the macro twin as a dep (it is in the
workspace; it depends only on the facade, so no cycle), alongside the existing
`example-add`:
```toml
# The MACRO twin — same `add`, authored with `#[modal_rust::function]` + auto-I/O
# `add_plain`. Importing it lets this driver show the inventory/typed-method ergonomics
# (`App::from_inventory()`, `app.add_plain(2,3).local()`) next to the manual path.
example-add-macro = { path = "../add-macro" }
```
KEEP `modal-rust`, `example-add`, `tokio` (`:16-22`) unchanged.

**`examples/orchestrate/src/main.rs`** — additive only. Keep section 1 (manual
`App::new(modal_registry())` → `app.function("add").local(...)`, `:39-46`) exactly as
the no-macro teaching path. Insert a new section 1b BEFORE the live block (`:48`), and
extend the doc header (`:1-24`) to mention it:

```rust
// ----- 1b. OFFLINE (MACRO PATH): App::from_inventory() + typed app.fn() -----------
//
// The SAME offline `.local()` dispatch, but the registry comes from the
// `#[modal_rust::function]` inventory instead of a hand-written builder, and the call
// is the typed positional method generated for the plain-signature `add_plain` — no
// input/output type is ever named.
use example_add_macro::AddPlainCall;          // the generated typed-method trait
let macro_app = App::from_inventory();

// Named-struct path through the macro registry (string-keyed), mirrors section 1:
let macro_out: example_add_macro::AddOutput =
    macro_app.function("add").local(example_add_macro::AddInput { a: 40, b: 2 })?;
println!("local (macro/inventory): add(40, 2) -> {{sum: {}}}", macro_out.sum);
assert_eq!(macro_out.sum, 42);

// Auto-I/O ergonomics: typed positional method, result decodes to the return type.
let plain_sum: i64 = macro_app.add_plain(2, 3).local()?;
println!("local (macro auto-I/O):  add_plain(2, 3) -> {}", plain_sum);
assert_eq!(plain_sum, 5, "the typed app.add_plain(2,3).local() path must compute 5");
```

Notes:
- `AddPlainCall` is the trait that carries the `app.add_plain(..)` method
  (`examples/add-macro/src/lib.rs:255` brings `crate::AddPlainCall` into scope inside the
  add-macro tests; here it is `example_add_macro::AddPlainCall`).
- Bring `App::from_inventory` in scope via the existing
  `use modal_rust::{App, DeployConfig};` (`:27`) — `from_inventory` is an `App` method,
  no extra import.
- Keep the existing imports `use example_add::{modal_registry, AddInput, AddOutput};`
  (`:26`); the macro types are spelled fully-qualified (`example_add_macro::...`) to
  avoid colliding with the `example_add` names already imported.
- The live `run_remote`/`run_deploy_and_call` block (`:52-104`) stays GATED on
  `RUN_REMOTE=1`, unchanged. The default run still prints
  `local: add(40, 2) -> {sum: 42}` plus the two new macro-path lines, all zero-Modal.
- Update the module doc header (`:1-24`) to add a bullet that the tour also shows the
  macro/inventory path (`App::from_inventory()` + `app.add_plain(2,3).local()`).
- OPTIONAL: add a `#[test]` mirroring `local_add_returns_42` (`:113-121`) that asserts
  `App::from_inventory()` + `app.add_plain(2,3).local()? == 5`, proving the macro path
  offline. Recommended (cheap, zero-Modal, guards the ergonomic surface).

> Cycle/build check: `example-add-macro` depends only on `modal-rust` (facade);
> `example-orchestrate` already depends on `modal-rust` + `example-add`. Adding
> `example-add-macro` keeps it a DAG. orchestrate is in default-members, so this is
> covered by the offline gate.

---

## 6. README Examples-section edits (`README.md`)

The table (`:436-442`) already labels add (manual) vs add-macro (macro). Make the
macro-vs-manual labelling explicit and show the ergonomic invocation; update the
cuda/burn rows to say they are now decorator-driven. Concrete edits:

1. **Table rows** (`:438-442`) — relabel:
   - `examples/add` → add the explicit tag: "**(manual / no-macro)** The walking
     skeleton: a hand-written `modal_registry()` with `typed!(add)` … This is the
     teaching reference for the no-macro path."
   - `examples/add-macro` → "**(macro)** The same `add` authored with
     `#[modal_rust::function]` — plus the auto-I/O plain-signature twin
     `add_plain(a, b)` callable as `app.add_plain(2, 3).local()/.remote()`, and the full
     decorator config (`gpu`/`timeout`/`cache`/`secrets`/`volumes`)."
   - `examples/orchestrate` → "A tour of the facade driving `add` via `.local()`,
     `.remote()`, and `deploy`+`call` — through BOTH the manual `App::new(modal_registry())`
     and the macro `App::from_inventory()` + typed `app.add_plain(2,3)` paths."
   - `examples/cuda-vector-add` → "**(macro)** A real GPU kernel — `cudarc` Driver API +
     precompiled PTX — authored with `#[modal_rust::function(gpu = \"T4\", name =
     \"vector_add\")]`; the decorator IS the config, run on a T4 via `.remote()`."
   - `examples/burn-add` → "**(macro)** A real ML workload — a Burn/CubeCL tensor op
     (NVRTC at runtime) authored with `#[modal_rust::function(gpu = \"T4\", name =
     \"burn_add\")]`, deployed and called on a T4."

2. **Add a short ergonomics snippet** under the table (new lines after `:442`), showing
   the macro surface concretely (keep it minimal, copy-faithful to add-macro):
   ````markdown
   The macro path is the ergonomic one — decorate a plain function and call it as a
   typed method, no input/output struct named:

   ```rust
   #[modal_rust::function]                       // auto-I/O from the plain signature
   pub fn add_plain(a: i64, b: i64) -> anyhow::Result<i64> { Ok(a + b) }

   #[modal_rust::function(gpu = "T4")]           // the decorator IS the config
   pub fn vector_add(input: VectorAddInput) -> anyhow::Result<VectorAddOutput> { /* … */ }

   // …then, against an inventory-built App:
   let app = modal_rust::App::from_inventory();
   let five: i64 = app.add_plain(2, 3).local()?;             // offline, zero Modal
   let out = app.add_plain(2, 3).remote().await?;            // on Modal
   ```
   ````

3. **"How to run" accuracy** (`:444-469`) — the orchestrate run block is still correct
   (the default `cargo run -p example-orchestrate` still prints `local: add(40, 2) ->
   {sum: 42}`; it now ALSO prints the two macro-path lines — optionally extend the
   sample `text` block at `:460-465` to include them, e.g.
   `local (macro/inventory): add(40, 2) -> {sum: 42}` and
   `local (macro auto-I/O):  add_plain(2, 3) -> 5`). Add one optional line showing the
   offline ergonomics proof for the converted GPU examples:
   ```bash
   # Offline proof that the decorator rides through inventory (no GPU, no Modal):
   cargo run -p example-cuda-vector-add --bin modal_runner -- --describe
   # -> {"schema":"modal-rust/describe@1","entrypoints":[{"name":"vector_add",
   #     "config":{"gpu":"T4","timeout_secs":null,"cache":null,"secrets":[],"volumes":[]}}]}
   ```
4. **DO NOT touch** the user's uncommitted `docs/testing-strategy.md`.

---

## 7. Verification plan + risks

**Offline HARD gate (default-members), all from repo root:**
```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo build
cargo test          # includes macro_path_byte_identical_to_manual (add-macro) + the
                    # updated registry_has_* in cuda; + orchestrate local tests
cargo build -p example-burn-add        # burn is out of default-members; builds here
cargo test -p example-burn-add         # runs the in-crate registry_has_burn_add etc.
```

**Ergonomics proof (REQUIRED, offline) for EACH converted example** — run the runner's
additive `--describe` and confirm the entrypoint reports `gpu: "T4"`:
```bash
cargo run -p example-cuda-vector-add --bin modal_runner -- --describe
# expect: entrypoints[0] = {"name":"vector_add","config":{"gpu":"T4", … }}

cargo run -p example-burn-add --bin modal_runner -- --describe
# expect: entrypoints[0] = {"name":"burn_add","config":{"gpu":"T4", … }}
```
This proves decorator-is-config rides through inventory (the runner reads the config via
`from_inventory_with_configs` → `run_cli_with_configs` → `emit_describe`,
`crates/modal-rust-runtime/src/lib.rs:410,603,762`). Paste the JSON as evidence.

**Live (best-effort, NOT required):** the gpu wire path is already live-proven by
`live_gpu`/`live_burn`, which are UNCHANGED in semantics. If Modal creds are present, a
cheap T4 ephemeral `live_gpu` run is a bonus (per the AFK note); NEVER run the heavy burn
build live; never block on Modal; never commit tokens.

**Frozen-invariant confirmations to report:**
- `examples/add/*` byte-identical (untouched).
- `macro_path_byte_identical_to_manual` (add-macro) passes.
- The 5 error kinds / runner protocol / `typed!` / `from_inventory` dispatch unchanged
  (no runtime/SDK/macro-crate edits).

**Risks / contingencies:**
- **burn-add without CUDA:** VERIFIED to `cargo check` AND it compiled the full burn tree
  in ~16s here, so `cargo build -p example-burn-add` should also succeed on this host. IF
  a future toolchain genuinely needs CUDA to BUILD (not expected — cudarc dynamic-loading
  + CubeCL link with no toolkit), fall back to `cargo check -p example-burn-add` and note
  precisely which step fails. Do NOT let burn-add block the default-members gate (it is
  excluded from default-members by design).
- **Duplicate-name panic** is AVOIDED by §4's "keep stub, no dep" decision — the converted
  crates' inventory submissions never link into the `modal-rust` test binary.
- **fmt:** the inserted runner bodies + orchestrate section must pass `cargo fmt --check`;
  use the add-macro runner formatting verbatim.
- **clippy `-D warnings`:** the `use <crate> as _;` import is the idiomatic "link for side
  effects" form (add-macro uses it, clippy-clean); the orchestrate `use
  example_add_macro::AddPlainCall;` is used (the method call), so no unused-import warning.

---

## 8. Change inventory (files touched)

| File | Change |
| --- | --- |
| `examples/cuda-vector-add/Cargo.toml` | swap `modal-rust-runtime` → `modal_rust_facade` (single facade dep) |
| `examples/cuda-vector-add/src/lib.rs` | `extern crate` alias; decorate `vector_add` with `(gpu="T4", name="vector_add")`; drop `modal_registry()` + the runtime `use`; rewrite `registry_has_vector_add` to inventory |
| `examples/cuda-vector-add/src/bin/modal_runner.rs` | replace body with the inventory runner |
| `examples/burn-add/Cargo.toml` | swap `modal-rust-runtime` → `modal_rust_facade` (keep burn/cudarc/libloading) |
| `examples/burn-add/src/lib.rs` | `extern crate` alias; decorate `burn_add` with `(gpu="T4", name="burn_add")`; drop `modal_registry()` + the runtime `use`; rewrite `registry_has_burn_add` to inventory |
| `examples/burn-add/src/bin/modal_runner.rs` | replace body with the inventory runner |
| `examples/orchestrate/Cargo.toml` | add `example-add-macro` path dep |
| `examples/orchestrate/src/main.rs` | add section 1b (`App::from_inventory()` + `app.add_plain(2,3).local()`); extend doc header; optional macro-path test |
| `crates/modal-rust/tests/live_gpu.rs` | comment-only doc refresh on the stub (semantics unchanged; stub KEPT) |
| `crates/modal-rust/tests/live_burn.rs` | comment-only doc refresh on the stub (semantics unchanged; stub KEPT) |
| `README.md` | Examples table relabel (macro vs manual) + ergonomics snippet + `--describe` proof line |
| `examples/add/*` | UNTOUCHED (frozen) |
| `crates/modal-rust/Cargo.toml` | UNTOUCHED (no new dev-dep — avoids the duplicate-name panic) |
| `crates/modal-rust-runtime`, SDK, `crates/modal-rust-macros` | UNTOUCHED (frozen) |
