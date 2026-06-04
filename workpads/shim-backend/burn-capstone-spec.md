# BURN/GPU Capstone — Build-Ready Spec

Single build-ready spec for the BURN/GPU capstone: a configurable CUDA-devel base
image with a rust-install step (Tier-1 image, boundaries.md §9), plus the burn-add
macro/gpu conversion and the facade deploy+call / `.remote()` invocation that proves
a real Burn/CubeCL tensor op on a CUDA GPU on Modal.

---

## 0. CUDA-version reconciliation (load-bearing — RESOLVED)

**Decision: `nvidia/cuda:12.6.3-devel-ubuntu22.04`** (devel, not runtime).

Ground truth in the current tree:

- `examples/burn-add/Cargo.toml` pin set (M13): `burn 0.21` + `burn-cuda 0.21` →
  `cubecl 0.10 (cuda)` / `cubecl-cuda 0.10` → `cudarc ^0.19`
  (`fallback-dynamic-loading`, `fallback-latest`, `cuda-version-from-build-system`).
  `Cargo.lock` resolves exactly: `burn 0.21.0`, `burn-cuda 0.21.0`, `cubecl 0.10.0`,
  `cubecl-cuda 0.10.0`, `cubecl-runtime 0.10.0`, `cudarc 0.19.7`.
- There is **no "CUDA-13-only symbols" note** in the current `Cargo.toml`. The
  brief's contingency about CUDA-13 symbols does **not** apply to this pin set.
- `examples/burn-add/src/lib.rs` self-check message names
  `nvidia/cuda:<12.x|13.x>-runtime-<os>` — 12.x is explicitly acceptable.
- `workpads/gpu-compute/knowledge.md` records M13 **PASSED on
  `nvidia/cuda:12.6.3-devel-ubuntu22.04`** with these exact pins, verified
  element-wise on a T4 (`valid:true`; samples 128→384, 255→765).
- `workpads/gpu-compute/gpu_app.py` (`CUDA_DEVEL_TAG`) used `12.6.3-devel-ubuntu22.04`.

Why **devel** (not runtime), why **12.6.3**:

- `cudarc 0.19.7` uses **dynamic-loading**: it links with NO CUDA at build time (so
  the crate compiles on the CPU-only Mac host) and `dlopen`s `libnvrtc`/`libcudart`
  at runtime. The build-time toolkit version is therefore NOT the link gate — there
  are no "undefined reference to cuda13-only symbols" at link time.
- **devel is required and empirically proven** (`knowledge.md`): CubeCL JIT-compiles
  CUDA-C kernels via NVRTC at runtime; the generated source `#include
  <cuda_runtime.h>`, and `cubecl-cuda` passes `--include-path=$CUDA_PATH/include` to
  NVRTC. The CUDA **headers** must be on disk. `-runtime-` ships
  `libcudart`+`libnvrtc` but NOT the headers → fails at first kernel launch with
  `cannot open source file "cuda_runtime.h"`. `-devel-` ships headers at
  `/usr/local/cuda/include`.
- CUDA container major 12.x ≤ host Driver API (observed 13.0 on the T4 host; the
  driver is forward-compatible), so the container is runtime-compatible with T4.

**Escalation (only if a future `cubecl-cuda`/`cudarc` bump introduces a CUDA-13-only
symbol):** change `CUDA_DEVEL_TAG` / `base_image` from `12.6.3-devel-ubuntu22.04` to a
`13.x-devel-ubuntu22.04` tag and re-deploy. That is a one-line config change, not a
design change. T4 supports CUDA 12/13 runtime; only the build-time toolkit matters.

---

## 1. The CUDA-devel + rust-install image recipe (port of `gpu_app.py`)

Base: `nvidia/cuda:12.6.3-devel-ubuntu22.04`.

Provisioning, in render order:

1. **Python** via `add_python="3.12"` (the project's PRIMARY path —
   python-build-standalone mount, same as the proven recipe's `add_python="3.12"`).
   Emits `COPY /python/. /usr/local` + `ln -s …/python3 …/python` (series < 3.13) +
   `ENV TERMINFO_DIRS=…`. The CUDA base has no Python; this supplies it so `python3`
   exists before the wrapper bake (`python3 -c`).
2. **apt prereqs for rustup**: `curl ca-certificates build-essential pkg-config`
   (matches `gpu_app.py`).
3. **rustup install** (exact command from `gpu_app.py`):
   `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
   --default-toolchain stable --profile minimal`.
4. **PATH + CUDA env** (baked as image ENV so they hold at build AND runtime):
   - `ENV CUDA_PATH=/usr/local/cuda` (load-bearing: tells CubeCL where the NVRTC
     include path `$CUDA_PATH/include` is).
   - `ENV PATH=/root/.cargo/bin:/usr/local/cuda/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin`
   - `ENV RUST_BACKTRACE=1`
   - `LD_LIBRARY_PATH` is NOT set explicitly: the `-devel` image's
     `/etc/ld.so.conf.d` already puts `/usr/local/cuda/lib64` on the loader path, and
     M13 passed without it (the dlopen self-check also tries versioned sonames
     `libnvrtc.so.12` / `libcudart.so.12`).
5. **wrapper bake** (existing `with_wrapper_module`; runs `python3 -c …` — python
   present from step 1).
6. **`ENTRYPOINT []`** to neutralize the CUDA base ENTRYPOINT (`gpu_app.py`).

**Critical ordering constraint (load-bearing):** in
`ImageSpec::dockerfile_commands`, the `add_python` branch and the
`pre_bake_commands` (from `with_apt`) branch are **mutually exclusive (if/else)**.
The CUDA image needs `add_python` (python/modal) AND apt+rustup, so `with_apt` is
**unusable** here (its `pre_bake_commands` are suppressed when `add_python` is set).
The apt+rustup+ENV steps must render in a dedicated `if self.install_rust { … }`
block placed AFTER the add_python if/else and BEFORE the wrapper bakes — they compose
with `add_python` without touching the mutually-exclusive branches. rustup does not
need python, so the order is fine.

**DEPLOY ordering:** for the deploy path the `cargo build` rides `extra_commands` /
the top layer; the rust-install must be emitted before it. Putting the rust-install on
the **base layer** (which owns `add_python`) guarantees the toolchain + CUDA headers
are present before the top layer's `cargo build` runs.

---

## 2. Additive SDK/facade extension

The default `rust:1-slim` + add_python path stays **byte-identical** (`install_rust`
defaults `false` → no rendered-command drift). Three additive pieces.

### 2a. `ImageSpec` — new field + builder + render block

File: `crates/modal-rust-sdk/src/ops/image.rs`.

- Add `pub install_rust: bool` (default `false`), initialized `false` in
  `from_registry`.
- Add builder `pub fn with_rust_toolchain(mut self) -> Self { self.install_rust =
  true; self }`.
- In `dockerfile_commands()`, when `install_rust` is true, emit — AFTER the
  add_python if/else and BEFORE the `for wrapper_modules` loop:
  - `RUN apt-get update && apt-get install -y --no-install-recommends curl
    ca-certificates build-essential pkg-config && rm -rf /var/lib/apt/lists/* && curl
    --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    --default-toolchain stable --profile minimal` (single combined RUN, minimal
    layers).
  - `ENV PATH=/root/.cargo/bin:/usr/local/cuda/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin`
  - `ENV CUDA_PATH=/usr/local/cuda`
- **Tests** (mirror existing image tests): (a) the apt+rustup RUN is present, (b)
  `/root/.cargo/bin` is on PATH, (c) `CUDA_PATH=/usr/local/cuda` ENV present, (d) the
  default (no `with_rust_toolchain`) renders NONE of these (byte-identical to today).

Rationale for a flag (not tag auto-detection): detecting "base lacks rust" from a tag
string is brittle; an explicit `install_rust` knob set by the caller when it selects a
non-rust base is the additive, testable choice. The default rust base never sets it.

### 2b. `RemoteConfig` / `DeployConfig` — surface the knob

`base_image` is ALREADY configurable on both (`RemoteConfig.base_image`,
`DeployConfig.base_image`) — no change there. Add one field to each:

- `RemoteConfig.install_rust: bool` (default `false`).
  File: `crates/modal-rust/src/remote.rs`.
  Add `MODAL_RUST_BASE_IMAGE` and `MODAL_RUST_INSTALL_RUST` env overrides in
  `RemoteConfig::default()` (parity with the existing `MODAL_RUST_SOURCE_DIR` /
  `MODAL_RUST_PACKAGE`) so a CLI/env-driven run path can point at the CUDA base
  without code.
- `DeployConfig.install_rust: bool` (default `false`; copy across in `for_app` like
  `base_image`). File: `crates/modal-rust/src/deploy.rs`. `for_app` reuses
  `RemoteConfig::default()`, so it inherits the env defaults automatically.

### 2c. Wire into image assembly

- **RUN path** (`remote::ensure_function`, `remote.rs`): when `config.install_rust`,
  add `.with_rust_toolchain()` to the `ImageSpec` chain. The in-body `cargo build`
  just needs `/root/.cargo/bin` on PATH (the baked ENV) — satisfied.
- **DEPLOY path** (`deploy.rs`, two-layer): add `.with_rust_toolchain()` to the
  **base layer** (`deploy_base_layer_spec`, which already owns `add_python`), so rust
  + CUDA env are baked into layer 1. The top layer (`FROM base`) then COPYs source and
  runs the existing `cargo build --release -p <package> --bin modal_runner` with the
  toolchain inherited. Thread `install_rust` into `deploy_base_layer_spec`'s signature.

### 2d. Why base_image stays a config field, not a decorator key

`FunctionConfig` is FROZEN to `gpu` / `timeout_secs` / `cache` — it does NOT carry
`base_image`. The image base is a build/deploy concern, not a per-function runtime
concern, so it correctly lives on `RemoteConfig` / `DeployConfig` (the build knobs),
set by the caller/env — mirroring how `base_image` already lives there. This keeps the
macro byte-identical (FROZEN) and the offline gates green.

---

## 3. burn-add through the facade

### 3a. The lib stays manual `modal_registry()` — do NOT add the macro to the lib

`examples/burn-add/src/lib.rs` registers `burn_add` via the manual
`Registry::new().function("burn_add", typed!(burn_add))`; the bin
(`src/bin/modal_runner.rs`) calls `run_cli(modal_registry())`. **Keep this
unchanged.** This is the *uploaded crate's runner*, which must register the REAL
`burn_add` (heavy CUDA body) — exactly like `cuda-vector-add` keeps its manual
`modal_registry()`. Adding `#[modal_rust::function]` to the lib would pull
`inventory`+macro deps into the CUDA-only crate for no benefit; the lib never drives
the facade.

### 3b. The decorator lives in the LIVE TEST binary (the facade driver)

This mirrors the **actual** P4 GPU proof: the decorator config rides in the *driver
binary's inventory* (`crates/modal-rust/tests/live_gpu.rs` decorated a stub
`vector_add`), while the *uploaded crate's runner* registers the real function via its
own `modal_registry()`. The CLI/CI box cannot link the CUDA crate, so the facade reads
the decorated gpu config from the binary that calls `App::from_inventory()` /
`App::connect`. Put a **decorated stub** in the new live test binary:

```rust
#[modal_rust::function(gpu = "T4", name = "burn_add")]
fn burn_add(_input: BurnAddIn) -> Result<BurnAddOut, String> {
    Err("local stub: burn_add runs on Modal (T4), not in-process".to_string())
}
```

This records `FunctionConfig { gpu: Some("T4"), .. }` under the entrypoint name
`burn_add` into the test binary's inventory; it is NEVER executed remotely (the
uploaded `example-burn-add` runner runs the real kernel). `name = "burn_add"` MUST
match the entrypoint the uploaded runner registers. `String` is `Display +
Serialize`, satisfying `typed!` without anyhow (same trick as `live_gpu.rs`).

### 3c. I/O types (mirror `example_burn_add::{BurnAddInput,BurnAddOutput}`)

```rust
#[derive(Debug, Serialize, Deserialize)] struct BurnAddIn { n: usize }
#[derive(Debug, Serialize, Deserialize)] struct BurnAddOut {
    valid: bool, n: usize, backend: String,
    libnvrtc: String, libcudart: String,
    samples: Vec<(usize, f32, f32)>,
}
```

Both derive `Serialize + Deserialize` (Serialize for `.remote()` input + the `typed!`
stub; Deserialize to decode the real output).

### 3d. PRIMARY proof — DEPLOY + CALL (build-once; the heavy-build strategy)

New file `crates/modal-rust/tests/live_burn.rs`, gated `#![cfg(feature = "live")]` +
`#[ignore]` (mirror `live_gpu.rs` / `live_deploy.rs`). The `live` feature already
exists; `modal-rust-macros` + `inventory` are already dev-deps; macro re-exported as
`modal_rust::function`. No new deps, no new feature.

```rust
const DEPLOY_APP: &str = "modal-rust-burn-deploy";   // STABLE name; re-deploy replaces in place
const PACKAGE:    &str = "example-burn-add";
const CUDA_BASE:  &str = "nvidia/cuda:12.6.3-devel-ubuntu22.04";

let app = App::connect(CONNECT_APP).await?;          // captures the decorated burn_add gpu="T4"
let mut cfg = DeployConfig::for_app(DEPLOY_APP);
cfg.package      = PACKAGE.to_string();              // -p example-burn-add
cfg.base_image   = CUDA_BASE.to_string();            // the CUDA-devel base
cfg.install_rust = true;                             // rustup + CUDA env on layer 1
let _deployed = app.deploy_with(cfg).await?;         // cargo build at IMAGE-BUILD time (layer 2)
let out: BurnAddOut = app.call(DEPLOY_APP, "burn_add", BurnAddIn { n: 256 }).await?;
```

`deploy_with` resolves the decorated entrypoint's gpu so `gpu="T4"` rides into
`Resources.gpu_config` (gpu is on the deployed function, not the build). The CUDA base
+ `install_rust` ride the `DeployConfig`, independent of the decorator. The `cargo
build --release -p example-burn-add --bin modal_runner` command is already emitted by
the deploy top layer and picks up `cfg.package`.

Build once at image-build time; Modal caches the image by hash, so subsequent calls
are fast. Use the STABLE `modal-rust-burn-deploy` name (re-deploy replaces in place)
and a cheap T4.

### 3e. SECONDARY proof — `.remote()` (run path, in-body build) — only if time allows

The brief prefers deploy. If included, mirror `live_gpu.rs`'s `.remote()` flow but set
env: `MODAL_RUST_PACKAGE=example-burn-add`,
`MODAL_RUST_BASE_IMAGE=nvidia/cuda:12.6.3-devel-ubuntu22.04`,
`MODAL_RUST_INSTALL_RUST=1` (since `connect()` uses `RemoteConfig::default()`, which
2b makes env-aware). The decorated stub's `gpu="T4"` + timeout already ride via
`remote_invoke`. The run path rebuilds in-body each cold container, so it is slow —
deploy is primary.

### 3f. Success criteria / what the proof looks like (n=256)

- `valid == true` (GPU result matches CPU reference `c[i] = i + 2i = 3i`).
- `backend.contains("burn-cuda")` (e.g. `"burn-cuda (CubeCL CUDA / cudarc)"`) — proof
  it ran on the CUDA backend, not CPU.
- `libnvrtc` / `libcudart` non-empty resolved sonames (e.g. `libnvrtc.so` /
  `libcudart.so` or versioned `.12`) — the **Tier-1 proof** that NVRTC + cudart were
  on the loader path.
- `samples` e.g. `[(0, 0.0, 0.0), (128, 384.0, 384.0), (255, 765.0, 765.0)]`.

Assert `out.valid`, `out.backend.contains("burn-cuda")`, `!out.libnvrtc.is_empty()`.
Wrap the call in a 4-attempt transient-retry loop (mirror `live_gpu.rs` /
`live_deploy.rs`) delegating `is_transient` to `sdk_err.is_transient()`. Modal
flakiness => RETRY; drive to a terminal result; be patient (CUDA+burn build takes many
minutes); clean up ephemeral apps.

---

## 4. Upload scoping — confirmed correct as-is (no change)

The cargo-scoped upload (`workspace_closure(local_root, "example-burn-add")` +
`WorkspaceClosureSpec`) uploads the dependency closure of `example-burn-add`: the
crate dir + its workspace-member path deps (`modal-rust-runtime`) + the workspace
`Cargo.toml`/`Cargo.lock`. burn-add's heavy deps (burn/burn-cuda/cubecl-cuda/cudarc)
are **registry** crates, not workspace members, so they are NOT uploaded — cargo
fetches + builds them on Modal. `toml_edit` rewrites the uploaded workspace
`members`/`default-members` to the closure subset so the scoped upload is a
self-consistent workspace. `cfg.package = "example-burn-add"` is the only knob.
Confirmed build command: `cargo build --release -p example-burn-add --bin
modal_runner` (deploy top layer; run wrapper), matching the proven `gpu_app.py`.

---

## 5. FROZEN invariants — preserved

- Default `rust:1-slim` + add_python path unchanged (`install_rust` defaults `false`
  → no rendered-command drift on the CPU/default path).
- Runner protocol / `HandlerFn` / `typed!` / Registry dispatch untouched.
- Run-vs-deploy build boundary untouched: RUN = in-body `cargo build`; DEPLOY =
  `cargo build` at image-build time (top layer), runtime execs prebuilt
  `modal_runner`.
- `gpu=` verbatim passthrough (`parse_gpu_config` → `Resources.gpu_config`) reused.
- `retry_transient` on RPCs; ephemeral-run vs persistent-deploy; cargo-scoped upload;
  add_python path for the rust:slim default (the CUDA base is an ADDITIONAL
  configurable option, not a replacement).
- `example-burn-add` stays a member but **OUT of `default-members`** in root
  `Cargo.toml` (it cannot compile without CUDA). KEEP.
- `FunctionConfig` (decorator) stays FROZEN to gpu/timeout/cache; the macro is
  byte-identical.

---

## 6. Offline gates (default-members; burn-add NOT built)

- New SDK `install_rust` field + `with_rust_toolchain` builder + unit tests are pure
  string-rendering, no CUDA — compile on default-members.
- `crates/modal-rust/tests/live_burn.rs` is `#![cfg(feature = "live")]` + `#[ignore]`,
  so `cargo build/test/clippy` on default-members never compiles or runs it.
- Run before/after, all green on default-members (these do NOT build burn-add):
  `cargo fmt --check`; `cargo clippy --all-targets -- -D warnings`; `cargo build`;
  `cargo test`.

---

## 7. Files to touch (precise)

1. `crates/modal-rust-sdk/src/ops/image.rs` — add `install_rust: bool` field
   (default false in `from_registry`) + `with_rust_toolchain()` builder + render
   block (after add_python if/else, before wrapper bakes) + unit tests. (§2a)
2. `crates/modal-rust/src/remote.rs` — add `install_rust` to `RemoteConfig` +
   `MODAL_RUST_BASE_IMAGE` / `MODAL_RUST_INSTALL_RUST` env in `default()`; call
   `.with_rust_toolchain()` when set in `ensure_function`. (§2b, §2c)
3. `crates/modal-rust/src/deploy.rs` — add `install_rust` to `DeployConfig` (copy
   across in `for_app` like `base_image`); thread into `deploy_base_layer_spec`; call
   `.with_rust_toolchain()` on the base layer when set. (§2b, §2c)
4. `crates/modal-rust/tests/live_burn.rs` — NEW: decorated `burn_add` stub
   (gpu="T4", name="burn_add") + I/O types + deploy+call primary proof + optional
   `.remote()` secondary + 4-attempt transient retry. (§3)
5. `examples/burn-add/Cargo.toml` — (optional cosmetic) fix stale "CUDA-runtime
   image" wording → "CUDA-devel image (headers needed for NVRTC)".
6. `examples/burn-add/src/{lib.rs, bin/modal_runner.rs}` — **UNCHANGED** (manual
   `modal_registry()` is the uploaded runner; §3a).
7. Root `Cargo.toml` — **UNCHANGED** (burn-add stays member-but-excluded from
   default-members).

---

## 8. Live-iteration note (CUDA version)

Start at `nvidia/cuda:12.6.3-devel-ubuntu22.04` (proven in `gpu_app.py`; cudarc 0.19 /
cubecl-cuda 0.10 are CUDA-12-compatible via dynamic-loading). If a live build/runtime
fails on a missing CUDA-13-only symbol (`undefined reference to …` / NVRTC version
error) — which is NOT expected for this pin set — bump `CUDA_BASE` /
`cfg.base_image` to a `13.x-devel-ubuntu22.04` tag and re-deploy. T4 supports CUDA
12/13 runtime; only the build-time toolkit version matters. Use the STABLE
`modal-rust-burn-deploy` app name and a cheap T4.
