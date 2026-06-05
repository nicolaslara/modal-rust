# Examples + README Ergonomics Spec (`add_plain` → `add`)

Build-ready, file-by-file edit list to promote the ergonomic plain-signature form
to be the headline `add` in `examples/add-macro`, slim the showcased `lib.rs` to
read like user code, relocate the non-headline bits so they keep compiling AND
keep their (load-bearing) inventory registration, and rewire every consumer +
README reference. The manual `examples/add` infra stays FROZEN.

Goal: the face of `examples/add-macro/src/lib.rs` reads like the Python
`@app.function() def add(a, b): return a + b`.

---

## 0. Coupling map — VERIFIED against the tree (do not re-derive)

Verified by reading the files + `grep` across `examples/ crates/ README.md`.

### `examples/add` (MANUAL, FROZEN) — leave 100% as-is
- Defines struct `add(AddInput) -> AddOutput` returning `{sum}` ({40,2} ⇒ {sum:42}),
  error-kind entrypoints `fail`/`fail_structured`/`bad_encode`/`will_panic`,
  `gpu_info`, and `modal_registry()` (registers all six via `typed!`).
- Consumers of this crate (the backbone): `crates/modal-rust/tests/local.rs`
  (imports `example_add::{modal_registry, AddInput, AddOutput}`; drives `add`→42,
  `fail`, decode-error, unknown-entrypoint, NotConnected),
  `examples/orchestrate/src/main.rs` (imports the same; `.function("add")`→{sum:42}),
  and `crates/modal-rust/tests/{live_deploy,live_cache,live_remote}.rs` (not read
  here, but per the coupling map they assert `.function("add")…{sum:42}` against
  `example_add`).
- **DECISION: leave `examples/add` entirely untouched (NO `error_kinds` module
  tidy).** `crates/modal-rust/tests/local.rs` imports `modal_registry` + drives
  `add`/`fail` by name, and `examples/add`'s OWN `#[cfg(test)]` asserts the registry
  has all six entrypoints. The error-kind FN BODIES feed `typed!` autoref
  specialization (`fail_structured` returns a `Serialize` error → `details`;
  `bad_encode` → `encode_error`); relocating them into a `pub mod` risks the
  registration/`details` coupling for zero showcase benefit (the manual example
  already reads acceptably as hand-written code). SAFE: do not touch it. This is a
  deliberate deviation from the task's "optional tidy" — flagged as the safe choice.

### `examples/add-macro` (MACRO) — the crate to clean
Externally-coupled symbols (what other crates import/link):
- `crates/modal-rust/tests/live_auto_io.rs` — `use example_add_macro::add_plain;`
  (the `add_plain::Input` module) + `use example_add_macro::AddPlainCall;` (the
  generated trait). Asserts `app.add_plain(2,3).remote().await? == 5` and
  `app.function("add_plain").remote(add_plain::Input{a:2,b:3}) == 5`. Sets
  `MODAL_RUST_PACKAGE=example-add-macro`, `#[ignore]`, `#[cfg(feature="live")]`.
- `examples/orchestrate/src/main.rs` — `use example_add_macro::AddPlainCall;`
  (twice: line 64 in `main`, line 165 in the unit test), `macro_app.add_plain(2,3).local()`,
  unit test `local_macro_add_plain_returns_5`. Also asserts the inventory registers
  `add` by name (`Registry::from_inventory().get("add")`).
- **`crates/modal-rust/src/app.rs` `#[cfg(test)] mod tests`** (lines 501–563) —
  `use example_add_macro as _;` links the LIB inventory, then asserts
  `App::from_inventory().config_for("add_gpu") == {gpu:T4, timeout:1800, cache:false}`,
  `config_for("add_extras") == {secrets:["my-secret"], volumes:[("/data","my-vol")]}`,
  `config_for("add") == default`, and `deploy_target_config()` is `Some` (picks the
  decorated target). **⚠ HARD CONSTRAINT (see §0.1).**
- `crates/modal-rust/Cargo.toml` (line 46) + `crates/modal-rust/src/app.rs` (line 504)
  + `examples/add-macro/src/bin/modal_runner.rs` (line 13) link the crate as `_`.
- `crates/modal-rust/tests/live_secrets_volumes.rs` — does **NOT** import
  `secret_vol_probe`/`ProbeInput`/`ProbeOutput` as symbols. It defines its OWN local
  `ProbeInput`/`ProbeOutput` structs + its OWN decorated `secret_vol_probe_stub`
  (entrypoint name `"secret_vol_probe"`), and uploads the package via
  `MODAL_RUST_PACKAGE=example-add-macro` so the REMOTE runner registers the real body.
  It references `example_add_macro::secret_vol_probe` only in a doc comment (line 19).
  ⇒ **No symbol import to rewire**, BUT the add-macro RUNNER must still register a
  `secret_vol_probe` entrypoint (its `inventory::submit!` must link into the lib).

Symbols used ONLY by add-macro's own tests (not by any external crate): the
struct-form `add` + `AddInput`/`AddOutput`. (`add_gpu`/`add_extras` ARE used
externally — by `app.rs` tests — so they are NOT "test-only"; see §0.1.)

### `tests/duplicate_rejected.rs` — leave untouched
Uses entrypoint name `"dup"`; submits `Registration` directly; references no renamed
symbol. No edit.

### Macro behavior (verified in `crates/modal-rust-macros/src/lib.rs`)
- Mode B (plain params, e.g. `fn add(a:i64,b:i64)`) generates `pub mod #fn_ident`
  (lowercase fn name) → `add::Input { a, b }` (derives `Serialize + Deserialize`) +
  `pub type add::Output = i64`, a private spread shim registered via `typed!`, and a
  `#{Pascal}Call` trait (`to_pascal_case("add") == "Add"` ⇒ `AddCall`) impl'd for
  `App`, so `app.add(2,3).local()/.remote()/.spawn()/.map()`.
- Mode A (single bare user-struct param, e.g. `fn add_gpu(input: AddInput)` /
  `fn secret_vol_probe(input: ProbeInput)`) emits the fn verbatim + `typed!(fn)`
  registration + config ONLY — **no generated module, no `<Pascal>Call` trait, no
  typed App method.** So the relocated Mode-A fns need no trait re-exports.
- Renaming `add_plain` → `add` after DELETING the struct-form `add` ⇒ no name
  collision: `add::Input`/`add::Output`/`AddCall` are fresh.

### 0.1 ⚠ HARD CONSTRAINT — `add_gpu`/`add_extras` must stay in the LIB inventory
`crates/modal-rust/src/app.rs` tests link the add-macro LIB via `use example_add_macro
as _;` and assert `config_for("add_gpu")` and `config_for("add_extras")`. Rust
integration-test binaries (`examples/add-macro/tests/*.rs`) are SEPARATE crates whose
`inventory::submit!` does NOT link into the `modal-rust` crate. Therefore moving
`add_gpu`/`add_extras` to `examples/add-macro/tests/config.rs` **WOULD BREAK
`cargo test -p modal-rust`** (`from_inventory_captures_decorator_config`,
`from_inventory_captures_secrets_and_volumes`, `decorator_cache_override_precedence`).

⇒ **`add_gpu` and `add_extras` move into `examples/add-macro/src/proof.rs`
(`pub mod proof;`, compiled INTO the lib), NOT into `tests/`.** This is a deliberate
deviation from the task's "tests/config.rs OR drop" suggestion — both of those break
`app.rs`. The macro crate's own unit tests (`crates/modal-rust-macros/src/lib.rs`
lines 705–770) cover only internal helpers (`to_pascal_case`, mode selection,
`result_ok_type`) — they do NOT cover decorator→Registration end-to-end, so the
coverage cannot be "already elsewhere; drop it." It must be preserved in the lib.

⇒ `examples/add-macro/tests/config.rs` is **NOT created.** (`secret_vol_probe` is
likewise Mode A and lives in the same `proof.rs` because the runner must register it.)

---

## 1. `examples/add-macro/src/lib.rs` — REPLACE ENTIRELY (the showcase)

Overwrite the whole 442-line file with the short, user-style version below
(`Write` the file). It contains ONLY: a 2–3 line module doc, the facade alias, the
`use`, the ~3-line `add`, and a one-line `pub mod proof;`. Verbatim:

```rust
//! `examples/add-macro` — the macro path, written the way a user would write it.
//!
//! `#[modal_rust::function]` turns a plain Rust function into a Modal function:
//! it generates the JSON input/output plumbing, registers the entrypoint through
//! `inventory` (no `modal_registry()` builder to maintain), and adds a typed
//! `app.add(2, 3)` method — so the call site never names an input/output type.
//! This is the Rust twin of Python's `@app.function()\ndef add(a, b): return a + b`.

// Alias the facade crate (`modal-rust`, renamed `modal_rust_facade` in Cargo.toml)
// so the attribute is spelled `#[modal_rust::function]`. The macro routes every
// emitted path through this facade, so this crate needs ONLY `modal-rust`.
extern crate modal_rust_facade as modal_rust;

use modal_rust::function;

/// Add two integers — the whole function.
///
/// The macro generates `add::Input { a, b }` / `add::Output` (= `i64`), registers
/// the entrypoint via `inventory`, and adds a typed `app.add(2, 3)` method that
/// chains into `.local()` / `.remote().await` / `.spawn()` / `.map(..)`.
#[function]
pub fn add(a: i64, b: i64) -> anyhow::Result<i64> {
    Ok(a + b)
}

/// Extra entrypoints that keep the decorator-config and live secrets/volumes
/// coverage compiling and registered, kept out of the headline so `add` above
/// reads clean. See `proof.rs`.
pub mod proof;
```

Notes:
- DELETED from this file: the struct-form `add` + `AddInput` + `AddOutput`; the old
  `add_plain`; `add_gpu`; `add_extras`; `secret_vol_probe` + `ProbeInput` +
  `ProbeOutput`; the entire ~260-line `#[cfg(test)] mod tests`; the `serde` import.
- `use modal_rust::function;` lets the attribute be spelled `#[function]` (matches
  the README tutorial). `#[modal_rust::function]` would also still work.
- `pub mod proof;` is the single line that keeps the relocated registrations linked
  into the lib (so `Registry::from_inventory()` and `App::from_inventory()` still see
  `add_gpu`/`add_extras`/`secret_vol_probe`).

---

## 2. NEW FILE `examples/add-macro/src/proof.rs` — relocated non-headline bits

`Write` this file. It carries `add_gpu`, `add_extras`, `secret_vol_probe`, and the
`Probe*`/`Add*` types they need, each still decorated/registered EXACTLY as today so
the inventory shape is byte-identical (only the source location changed). Verbatim:

```rust
//! Proof entrypoints kept out of the headline `lib.rs`.
//!
//! These exist so the macro's decorator-config (`gpu`/`timeout`/`cache`/`secrets`/
//! `volumes`) and the live secrets+volumes body keep compiling AND keep their
//! `inventory` registration (the `modal-rust` crate's `App` tests assert
//! `config_for("add_gpu")` / `config_for("add_extras")`, and the live
//! `secret_vol_probe` runner entrypoint is built from here). They are NOT part of
//! the ergonomic showcase — `add` in `lib.rs` is.

use modal_rust::function;
use serde::{Deserialize, Serialize};

/// A single-struct input, mirroring `examples/add::AddInput`. Used by the Mode-A
/// (explicit-struct) decorator-config entrypoints below.
#[derive(Debug, Deserialize)]
pub struct AddInput {
    /// First addend.
    pub a: i64,
    /// Second addend.
    pub b: i64,
}

/// The output of the struct-form entrypoints. Mirrors `examples/add::AddOutput`.
#[derive(Debug, Serialize)]
pub struct AddOutput {
    /// `a + b`.
    pub sum: i64,
}

/// Per-function CONFIG demo (P4): `#[modal_rust::function(gpu=…, timeout=…,
/// cache=…)]` records a `FunctionConfig` alongside the registration. METADATA
/// ONLY — the emitted handler and runner dispatch are byte-identical to the bare
/// path; the facade reads the config when creating the Modal function.
#[function(gpu = "T4", timeout = 1800, cache = false)]
pub fn add_gpu(input: AddInput) -> anyhow::Result<AddOutput> {
    Ok(AddOutput {
        sum: input.a + input.b,
    })
}

/// User-facing SECRETS + VOLUMES demo: the decorator records the named secrets +
/// the `(mount_path, name)` volume pairs onto the `FunctionConfig`. METADATA ONLY.
/// The user volume (`/data`) is a SEPARATE mount from the P6 cargo cache (`/cache`).
#[function(secrets = ["my-secret"], volumes = ["/data=my-vol"])]
pub fn add_extras(input: AddInput) -> anyhow::Result<AddOutput> {
    Ok(AddOutput {
        sum: input.a + input.b,
    })
}

/// Input for [`secret_vol_probe`] — the REAL live secrets+volumes proof body.
#[derive(Debug, Serialize, Deserialize)]
pub struct ProbeInput {
    /// The env-var name to read (the attached secret's key).
    pub secret_key: String,
    /// Absolute path of the marker file on the mounted user volume.
    pub marker_path: String,
    /// When `Some`, WRITE this value to `marker_path` (first call); when `None`,
    /// only READ it back (second call) — proving the volume persisted across calls.
    pub write_value: Option<String>,
}

/// Output of [`secret_vol_probe`].
#[derive(Debug, Serialize, Deserialize)]
pub struct ProbeOutput {
    /// The value read from the secret env var, or `None` if unset.
    pub secret_value: Option<String>,
    /// The contents read back from `marker_path`, or `None` if absent.
    pub marker_read: Option<String>,
    /// `true` iff this call wrote the marker file.
    pub wrote: bool,
}

/// The REAL user-facing secrets+volumes proof body — runs REMOTELY on Modal.
///
/// Reads `std::env::var(secret_key)` (proves the attached secret was injected as a
/// container ENV VAR), optionally writes `write_value` to `marker_path` on the
/// mounted user volume, then reads it back (proves the volume persists). The live
/// test (`crates/modal-rust/tests/live_secrets_volumes.rs`) uploads this package so
/// the remote runner registers this entrypoint; the decorated stub that attaches the
/// real secret + volume lives in that test binary.
#[function]
pub fn secret_vol_probe(input: ProbeInput) -> anyhow::Result<ProbeOutput> {
    use std::path::Path;

    let secret_value = std::env::var(&input.secret_key).ok();

    let marker_path = Path::new(&input.marker_path);
    let mut wrote = false;
    if let Some(value) = &input.write_value {
        if let Some(parent) = marker_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(marker_path, value)?;
        wrote = true;
    }
    let marker_read = std::fs::read_to_string(marker_path).ok();

    Ok(ProbeOutput {
        secret_value,
        marker_read,
        wrote,
    })
}
```

Notes / risk:
- `proof.rs` is a child module of the crate root, so `#[modal_rust::function]` /
  `use modal_rust::function;` resolve through the crate's `extern crate
  modal_rust_facade as modal_rust;` (declared in `lib.rs`, visible crate-wide).
  ✔ no extra `extern crate` needed in `proof.rs`.
- `serde` derives in `proof.rs` resolve via the crate's `serde` dep (already present).
- The macro emits `::serde::Serialize`/`::serde::Deserialize` (fully-qualified) for
  the generated `Input`, so the relocated `#[function]` fns do not depend on a local
  `serde` import for the GENERATED code; the hand-written `#[derive(Serialize,
  Deserialize)]` on `AddInput`/`ProbeInput`/etc. DO need the `use serde::…` shown.
- These are all Mode A (`add_gpu`/`add_extras`) or Mode A (`secret_vol_probe`, single
  `ProbeInput` param) ⇒ NO `<Pascal>Call` trait is generated, so nothing new is
  exported and `examples/orchestrate` / live tests that glob-import are unaffected.
- The registered NAMES are unchanged (`add_gpu`, `add_extras`, `secret_vol_probe`),
  so `App::from_inventory().config_for(..)` and the runner `--describe`/dispatch are
  byte-identical to today.

---

## 3. NEW FILE `examples/add-macro/tests/behavior.rs` — slimmed essentials for `add`

The old `#[cfg(test)] mod tests` in `lib.rs` is deleted; recreate ONLY the essential
behavior for the new `add` as an integration test (it can reach `__private` through
the facade, same as the old inline tests). `Write` this file:

```rust
//! Essential offline behavior for the new headline `add` (the macro auto-I/O form).

use example_add_macro::add;
use example_add_macro::AddCall;
use modal_rust_facade::__private::runtime;
use modal_rust_facade::{App, Registry};

#[test]
fn add_is_a_plain_fn() {
    // The macro emits the user fn verbatim, so it stays directly callable.
    assert_eq!(add(2, 3).unwrap(), 5);
}

#[test]
fn add_generates_named_input_and_output() {
    // Mode B generated nameable `add::Input { a, b }` (Serialize + Deserialize) and
    // `add::Output` (= i64); the input serializes to the frozen named JSON object.
    let json = serde_json::to_string(&add::Input { a: 2, b: 3 }).unwrap();
    assert_eq!(json, r#"{"a":2,"b":3}"#);
    let back: add::Input = serde_json::from_str(r#"{"a":40,"b":2}"#).unwrap();
    assert_eq!((back.a, back.b), (40, 2));
    let _out: add::Output = 5i64;
}

#[test]
fn add_registered_via_inventory_runner_envelope() {
    // The generated spread shim is registered under `add` and dispatches through the
    // UNCHANGED runner: wire input `{"a":2,"b":3}` -> envelope `{"ok":true,"value":5}`.
    assert!(Registry::from_inventory().get("add").is_some());
    let argv: Vec<String> = ["--entrypoint", "add", "--input-json", r#"{"a":2,"b":3}"#]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut buf = Vec::new();
    let code = runtime::run_cli_with_args(Registry::from_inventory(), &argv, &mut buf);
    assert_eq!(code, 0);
    assert_eq!(String::from_utf8(buf).unwrap(), "{\"ok\":true,\"value\":5}\n");
}

#[test]
fn add_typed_app_method_local() {
    // Auto-I/O ergonomics: typed positional method, no input/output type named.
    let app = App::from_inventory();
    let sum: i64 = app.add(2, 3).local().unwrap();
    assert_eq!(sum, 5);
}

#[test]
fn add_explicit_input_path_local() {
    // The generated input stays callable explicitly via the string-keyed path.
    let app = App::from_inventory();
    let sum: i64 = app.function("add").local(add::Input { a: 2, b: 3 }).unwrap();
    assert_eq!(sum, 5);
}

#[test]
fn unknown_entrypoint_error_kind() {
    let argv: Vec<String> = ["--entrypoint", "nope", "--input-json", "{}"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut buf = Vec::new();
    let code = runtime::run_cli_with_args(Registry::from_inventory(), &argv, &mut buf);
    assert_eq!(code, 1);
    let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["kind"], "unknown_entrypoint");
}
```

Notes / risk:
- This is an INTEGRATION test (separate crate). It links `example_add_macro`'s lib
  inventory automatically via the `use example_add_macro::…` imports, so `add`,
  `add_gpu`, `add_extras`, `secret_vol_probe` are all present in
  `Registry::from_inventory()` here. `AddCall` is a public trait re-exported from the
  macro expansion in the lib root, brought in via `use example_add_macro::AddCall;`.
- `add::Input` is `#[doc(hidden)] pub mod add` — reachable as `add::Input` /
  `add::Output` via the `use example_add_macro::add;` import. (Same access pattern
  the old inline test used as `add_plain::Input`, and that `live_auto_io.rs` uses.)
- DROPPED tests (intentionally, per task): the old `macro_path_byte_identical_to_manual`
  ({sum:42}) compared the now-deleted struct `add`; the frozen runner protocol is
  already covered by `crates/modal-rust-runtime` tests + this file's envelope check.
  The decorator-config assertions (`bare_macro_config_is_default`,
  `secrets_and_volumes_decorator_parses_into_config`,
  `user_volume_and_cache_mount_paths_are_distinct`,
  `configured_macro_populates_function_config`,
  `secret_vol_probe_reads_env_and_does_volume_io`) are NOT lost — see §4.
- `modal-rust-facade` is the lib's dep (`package = "modal-rust"`); integration tests
  see it as `modal_rust_facade`. ✔ available (it is a normal `[dependencies]` entry,
  visible to tests).

---

## 4. NEW FILE `examples/add-macro/tests/config.rs` — decorator-config coverage

The old inline tests asserted the decorator parses into the inventory `Registration`
config (`add_gpu` gpu/timeout/cache, `add_extras` secrets/volumes, bare `add`
default, the `secret_vol_probe` body offline). `crates/modal-rust/src/app.rs` already
asserts `config_for("add_gpu")`/`("add_extras")` end-to-end, so this file's job is to
preserve the add-macro-local coverage that those don't (the bare-`add` default config
and the `secret_vol_probe` body). `Write` this file:

```rust
//! Decorator-config + proof-body coverage for `examples/add-macro`, kept in a tests/
//! file so the headline `lib.rs` stays clean. The `add_gpu`/`add_extras` configs are
//! also asserted end-to-end in `crates/modal-rust`'s `App` tests; here we pin the
//! inventory `Registration.config` directly and exercise the offline proof body.

use example_add_macro::proof::{secret_vol_probe, ProbeInput};
use modal_rust_facade::__private::inventory;
use modal_rust_facade::{FunctionConfig, Registration};

fn registration(name: &str) -> Option<&'static Registration> {
    inventory::iter::<Registration>
        .into_iter()
        .find(|r| r.name == name)
}

#[test]
fn bare_macro_config_is_default() {
    let reg = registration("add").expect("macro must register `add`");
    assert_eq!(reg.config, FunctionConfig::default());
    assert!(reg.config.secrets.is_empty());
    assert!(reg.config.volumes.is_empty());
}

#[test]
fn gpu_decorator_parses_into_config() {
    let reg = registration("add_gpu").expect("macro must register `add_gpu`");
    assert_eq!(reg.config.gpu, Some("T4"));
    assert_eq!(reg.config.timeout_secs, Some(1800));
    assert_eq!(reg.config.cache, Some(false));
}

#[test]
fn secrets_and_volumes_decorator_parses_into_config() {
    let reg = registration("add_extras").expect("macro must register `add_extras`");
    assert_eq!(reg.config.secrets, &["my-secret"]);
    assert_eq!(reg.config.volumes, &[("/data", "my-vol")]);
    // The user volume mount never collides with the cargo cache `/cache`.
    for (mount, _name) in reg.config.volumes {
        assert_ne!(*mount, "/cache");
    }
}

#[test]
fn secret_vol_probe_reads_env_and_does_volume_io() {
    let key = "MODAL_RUST_PROBE_UNITTEST_SECRET";
    std::env::set_var(key, "hello-unit");
    let dir = std::env::temp_dir().join("modal_rust_probe_unittest");
    let _ = std::fs::remove_dir_all(&dir);
    let marker = dir.join("marker");
    let marker_path = marker.to_string_lossy().to_string();

    let first = secret_vol_probe(ProbeInput {
        secret_key: key.to_string(),
        marker_path: marker_path.clone(),
        write_value: Some("persisted-value".to_string()),
    })
    .unwrap();
    assert_eq!(first.secret_value.as_deref(), Some("hello-unit"));
    assert!(first.wrote);
    assert_eq!(first.marker_read.as_deref(), Some("persisted-value"));

    let second = secret_vol_probe(ProbeInput {
        secret_key: key.to_string(),
        marker_path: marker_path.clone(),
        write_value: None,
    })
    .unwrap();
    assert!(!second.wrote);
    assert_eq!(second.marker_read.as_deref(), Some("persisted-value"));

    std::env::remove_var(key);
    let _ = std::fs::remove_dir_all(&dir);
}
```

Notes:
- `Registration` + `FunctionConfig` are public re-exports of the facade
  (`modal_rust_facade::{FunctionConfig, Registration}`) — verified by the old inline
  test (`use modal_rust::{FunctionConfig, Registration, Registry};`) and
  `tests/duplicate_rejected.rs`.
- `inventory` via `modal_rust_facade::__private::inventory` — verified same source.
- `secret_vol_probe`/`ProbeInput` are now under `example_add_macro::proof::…`.

### 4.1 Cargo.toml dev-deps for the new test files
`examples/add-macro/Cargo.toml` currently lists `serde`, `serde_json`, `anyhow` in
`[dependencies]` (all visible to integration tests) and `modal_rust_facade`. The new
test files use `serde_json` (already a dep) + `modal_rust_facade` (already a dep).
**No `[dev-dependencies]` change required.** (Confirm `serde_json` is reachable from
tests — it is a normal `[dependencies]` entry, so yes.) If `cargo` warns that
`serde`/`serde_json` are now unused by the lib itself (the lib's generated code uses
`::serde::…` paths, so `serde` stays used; `serde_json` is used only by tests), keep
both — they are needed by `proof.rs` (serde) and the test files (serde_json). No risk.

---

## 5. Consumer rewrites

### 5.1 `crates/modal-rust/tests/live_auto_io.rs`
Rename `add_plain` → `add`, `AddPlainCall` → `AddCall` throughout. Specific edits
(line refs against the current file):
- Doc comment line 5: `fn add_plain(a: i64, b: i64)` → `fn add(a: i64, b: i64)`.
- Doc lines 8–12: `app.add_plain(2, 3)` → `app.add(2, 3)`; `add_plain::Input` →
  `add::Input`; `app.function("add_plain")` → `app.function("add")`.
- Line 35: `use example_add_macro::add_plain;` → `use example_add_macro::add;`
- Line 36: `use example_add_macro::AddPlainCall;` → `use example_add_macro::AddCall;`
- Lines 41–46 doc + `PACKAGE`: replace `add_plain` mentions with `add`. (`PACKAGE =
  "example-add-macro"` UNCHANGED; `MODAL_RUST_PACKAGE` UNCHANGED.)
- Line 59 fn name: `remote_auto_io_add_plain_returns_5` → `remote_auto_io_add_returns_5`.
- Lines 67–71 messages/asserts: `app.add_plain(2,3)` → `app.add(2,3)`;
  `"add_plain"` → `"add"`; keep `== 5`.
- Lines 93–98 doc: `add_plain` → `add`.
- Line 103: `app.add_plain(2, 3).remote()` → `app.add(2, 3).remote()`.
- Line 106: `.function("add_plain")` → `.function("add")`.
- Line 107: `.remote(add_plain::Input { a: 2, b: 3 })` → `.remote(add::Input { a: 2, b: 3 })`.
- Keep `#[ignore]` (line 58) + `#![cfg(feature = "live")]` (line 28).

### 5.2 `examples/orchestrate/src/main.rs`
- Doc line 13: `app.add_plain(2, 3).local()` → `app.add(2, 3).local()`.
- Lines 58–63 doc: `add_plain` → `add`; `add_plain::{Input, Output}` →
  `add::{Input, Output}`.
- Line 64: `use example_add_macro::AddPlainCall;` → `use example_add_macro::AddCall;`
- Line 67 doc: `registers add (and add_plain)` → `registers add`.
- Line 78: `macro_app.add_plain(2, 3).local()?` → `macro_app.add(2, 3).local()?`.
- Line 79: `println!("local (macro auto-I/O):  add_plain(2, 3) -> {plain_sum}");` →
  `println!("local (macro auto-I/O):  add(2, 3) -> {plain_sum}");`
- Line 82 assert msg: `app.add_plain(2,3)` → `app.add(2,3)`.
- Line 162 doc: `app.add_plain(2, 3).local()` → `app.add(2, 3).local()`.
- Line 164 test fn: `local_macro_add_plain_returns_5` → `local_macro_add_returns_5`.
- Line 165: `use example_add_macro::AddPlainCall;` → `use example_add_macro::AddCall;`
- Line 168: `.add_plain(2, 3)` → `.add(2, 3)`.
- **KEEP UNTOUCHED**: the manual `example_add` path — `use example_add::{modal_registry,
  AddInput, AddOutput};` (line 30), `App::new(modal_registry())` (line 43),
  `.function("add")` struct `{sum:42}` (lines 48, 112, 135), the `run_remote` /
  `run_deploy_and_call` fns, and `local_add_returns_42` (line 151). Orchestrate shows
  BOTH paths.
- Note the variable is still named `plain_sum` (lines 78, 81) — fine to leave, or
  rename to `sum` for cleanliness (cosmetic; not required). Recommend leaving to
  minimize churn, OR rename to `sum`; either compiles. (No external coupling.)

### 5.3 `crates/modal-rust/tests/live_secrets_volumes.rs`
- **No symbol rename needed** (it imports none of the renamed/moved add-macro
  symbols; it defines its own `ProbeInput`/`ProbeOutput`/`secret_vol_probe_stub`).
- ONE doc-comment update for accuracy (optional but recommended): line 19
  `the REAL body that runs remotely is example_add_macro::secret_vol_probe` →
  `example_add_macro::proof::secret_vol_probe`. Non-load-bearing (doc only). The
  `PACKAGE = "example-add-macro"`, `MODAL_RUST_PACKAGE`, the stub, and the
  `name = "secret_vol_probe"` entrypoint all stay UNCHANGED.

### 5.4 `crates/modal-rust/src/app.rs` (tests) — NO CODE CHANGE
The comment at lines 501–503 still reads "incl. the decorated `add_gpu`…"; the
assertions on lines 510–562 (`config_for("add_gpu")`, `config_for("add_extras")`,
`config_for("add")`) all still pass because those registrations moved to `proof.rs`
which is compiled into the same lib (`use example_add_macro as _;` still links them).
Optional doc tweak: none required. **Do not edit assertions.**

### 5.5 `crates/modal-rust/src/app.rs` line 504 / `Cargo.toml` line 46 / runner bin —
NO CHANGE. `use example_add_macro as _;` and the path dep are unchanged; the runner
`from_inventory_with_configs()` now also surfaces `add` (Mode B) + the proof fns.

---

## 6. README edits

The macro tutorial (≈104–144) ALREADY leads with `#[function] fn add(a,b)` +
`app.add(2,3).local()/.remote()` and a struct-form alternative — **KEEP IT AS-IS.**
Remaining `add_plain` references to replace (verified by grep — exactly these):

- **Line 480 (Examples table, `examples/add-macro` row).** Reword to:
  `| `examples/add-macro` | **(macro)** The SAME `add` in three lines:
  `#[modal_rust::function] fn add(a, b) -> anyhow::Result<i64>`, called
  `app.add(2, 3).remote().await?` — the macro generates the input struct,
  registration, and typed method. Plus the full decorator config
  (`gpu`/`timeout`/`cache`/`secrets`/`volumes`). |`
- **Line 479 (Examples table, `examples/add` row).** Reword to:
  `| `examples/add` | **(manual / no-macro)** The SAME `add` written by hand — the
  input struct, the `typed!` registration, and `modal_registry()`, i.e. everything
  the macro generates for you. Plus named entrypoints exercising every runner error
  kind. |`
- **Line 481 (Examples table, `examples/orchestrate` row).** `typed `app.add_plain(2, 3)`
  paths` → `typed `app.add(2, 3)` paths`.
- **Line 490** (quickstart snippet): `pub fn add_plain(a: i64, b: i64) -> anyhow::Result<i64>
  { Ok(a + b) }` → `pub fn add(a: i64, b: i64) -> anyhow::Result<i64> { Ok(a + b) }`.
  Also fix the comment on line 489 if it says "auto-I/O from the plain signature" —
  keep, it is accurate.
- **Line 497**: `let five: i64 = app.add_plain(2, 3).local()?;` →
  `let five: i64 = app.add(2, 3).local()?;`
- **Line 498**: `let out = app.add_plain(2, 3).remote().await?;` →
  `let out = app.add(2, 3).remote().await?;`
- **Line 507** (the printed `local tour` output block): `local (macro auto-I/O):
  add_plain(2, 3) -> 5` → `local (macro auto-I/O):  add(2, 3) -> 5`. ⚠ MUST match the
  new `println!` in `orchestrate/src/main.rs` §5.2 (both become `add(2, 3) -> 5`).

After edits, `grep -n add_plain README.md` MUST return zero.

---

## 7. Verification (offline = HARD gate)

Run from repo root, all must be green:
```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo build
cargo test
```
Then:
```
grep -rn "add_plain\|AddPlainCall" examples/ crates/ README.md   # MUST be EMPTY
```
(Note: `crates/modal-rust-macros/src/lib.rs:761` has
`assert_eq!(to_pascal_case("add_plain"), "AddPlain")` — that is an INTERNAL
PascalCase unit test on an arbitrary string, NOT a reference to the example symbol.
Leave it. It will appear in the grep. ⇒ scope the grep to exclude that line, OR
accept the single macro-internal hit. Recommend: `grep -rn "add_plain" examples/
crates/modal-rust crates/modal-rust-runtime README.md` to avoid the macro-crate
unit-test false positive — that hit is legitimate and stays.)

Runner proof:
```
cargo run -q -p example-add-macro --bin modal_runner -- --describe
# expect entrypoints incl. `add`, `add_gpu`, `add_extras`, `secret_vol_probe`
cargo run -q -p example-add-macro --bin modal_runner -- --entrypoint add --input-json '{"a":2,"b":3}'
# expect exactly: {"ok":true,"value":5}
```

Live (best-effort, cheap CPU — never block):
```
cargo test -p modal-rust --features live --test live_auto_io -- --ignored --nocapture
# proves app.add(2,3).remote() == 5
```

---

## 8. Risk register

1. **`add_gpu`/`add_extras` location (HARD).** They MUST stay registered in the
   add-macro LIB inventory (via `pub mod proof;`), NOT in `tests/`, or
   `cargo test -p modal-rust` (`app.rs` config tests) fails. ⇒ §0.1, §2. This is the
   single highest-risk deviation from the task wording; the task's "tests/config.rs OR
   drop" both break `app.rs`. SAFE path chosen.
2. **`add::Input` access from integration tests.** The generated module is
   `#[doc(hidden)] pub mod add` — reachable as `example_add_macro::add::Input`. The old
   inline test reached `add_plain::Input` the same way and `live_auto_io.rs` reaches
   `add_plain::Input` cross-crate today, so cross-crate `add::Input` is proven to work.
3. **`AddCall` re-export.** The trait is `pub trait AddCall` emitted at the lib root
   (sibling of `add`), so `use example_add_macro::AddCall;` works (today's
   `AddPlainCall` proves it). No `pub use` needed.
4. **README output-block / orchestrate `println!` must agree** (both `add(2, 3) -> 5`).
   §5.2 + §6 line 507 are a matched pair — change together or `verify` diverges.
5. **`live_secrets_volumes.rs` doc-only.** No symbol coupling; the only real
   requirement is that the add-macro RUNNER still registers `secret_vol_probe`
   (satisfied by `pub mod proof;`). The local `ProbeInput`/`ProbeOutput`/stub in the
   test stay as-is.
6. **`examples/add` left as-is (deliberate).** Deviates from the optional
   `error_kinds` tidy; chosen because `local.rs` + the crate's own registry test
   couple to the registration + `typed!` `details` behavior of the error-kind fns.
   Low value, non-trivial risk ⇒ skip.
7. **Unused-import / dead-code clippy.** After deleting the inline tests + `serde`
   import from `lib.rs`, ensure `lib.rs` no longer `use serde::…` (it doesn't in the
   new version). `proof.rs` keeps `use serde::{Deserialize, Serialize};` (used by the
   hand-written derives). `serde_json`/`anyhow` stay in Cargo `[dependencies]` (used by
   tests / `anyhow::Result`). Run `cargo clippy --all-targets -D warnings` to catch any
   stray unused import before declaring green.
```
