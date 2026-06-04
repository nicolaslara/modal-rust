# P10 — Delete the CLI codegen / Python-shim path (build-ready deletion spec)

Status: build-ready. Authored after reading
`crates/modal-rust-cli/src/{main.rs, templates.rs, doctor.rs, programmatic.rs, workspace.rs, Cargo.toml}`,
`crates/modal-rust-cli/src/templates/*.tmpl`, and a full workspace grep of every deleted symbol.

P10 removes the dead `--use-shim` Python-codegen path. The CLI becomes PURELY
programmatic (the P9 `programmatic.rs` path is the ONLY path). NO behavior of the
programmatic run/deploy/call changes. FROZEN invariants (runner protocol / typed! /
dispatch / build boundary / retry_transient / images / cargo-scoped upload / cache /
secrets / volumes / spawn / map / decorator config) are untouched.

---

## 0. Grep confirmation — deletion leaves NO dangling references

Every reference to a deleted symbol lives in exactly four files, all in the CLI crate:
`main.rs`, `templates.rs`, `templates/*.tmpl`, `doctor.rs`. Nothing in the runner /
facade / SDK / examples references any of them.

- `templates::` / `mod templates` / `use templates::ShimParams` — only `main.rs`
  (lines 30, 39, 311, 341, 361, and the tests at 420/427/434/443/464/482/493, plus
  the two source-scan tests at 533/540). All deleted in this spec.
- `ShimParams` — `templates.rs` (def) + `main.rs` (39, 269/270, 380, 387/395/403). All deleted.
- `cmd_run_shim` / `cmd_deploy_shim` / `cmd_call_shim` — `main.rs` only (def + dispatch + the
  `modal_subprocess_only_in_shim_path` test). All deleted.
- `run_modal` / `write_shim` / `generated_dir` / `shim_params` — `main.rs` only. All deleted.
- `check_modal_cli` — `doctor.rs` only (def 76 + call 296). Deleted.
- `DEFAULT_DEV_APP` / `DEFAULT_CALL_APP` — `main.rs` only, shim-path-only. Deleted.
- `RUST_VER` (the `main.rs:43` const) — `main.rs` only, used solely by `shim_params`. Deleted.
  (NOTE: the IDENTICALLY-NAMED `RUST_VER` in `crates/modal-rust/src/remote.rs:33` and
  `DEFAULT_DEPLOY_APP` in `crates/modal-rust/src/deploy.rs:46` are DIFFERENT symbols in the
  runtime crate — DO NOT touch them.)
- `use-shim` / `use_shim` — `main.rs` + `doctor.rs` only (CLI). All deleted.

Symbols that must be KEPT (referenced by the surviving live path):
- `DEFAULT_DEPLOY_APP` (`main.rs:49`) — used by the live `deploy`/`call` clap
  `default_value` (main.rs:105, 118). KEEP.
- `programmatic.rs` and everything it imports — the path now. KEEP entirely.
- `workspace.rs` (`workspace_root`, `package_name`) — used by `build_and_describe`
  in `programmatic.rs:168,171`. KEEP entirely.

Non-Rust references that are intentionally NOT touched by P10:
- `README.md:69` (`--use-shim` mention) — the orchestrator updates docs separately
  (per task instructions: do NOT edit README).
- `workpads/**` (`shim-backend/{knowledge.md,tasks.md,references.md,p4/p9-build-spec.md}`,
  `WORKPADS.md`, `gpu-compute/knowledge.md`, `prototype/knowledge.md`) — HISTORICAL
  reference notes; do NOT delete or edit workpad files.
- `.gitignore:2` (`.modal-rust/`) — harmless leftover (it ignored the now-removed
  generated dir). OPTIONAL cleanup; leaving it is correct and costs nothing. Recommend LEAVE.
- `TASKS.md:93-94`, `Cargo.toml` description — see §5 (Cargo.toml description reword is in scope).

Baseline confirmed: `cargo build -p modal-rust-cli` is GREEN before deletion.

---

## 1. Files to DELETE outright (4 files)

1. `crates/modal-rust-cli/src/templates.rs` — the `ShimParams` struct + `dev_app`/
   `deploy_app`/`call_app` renderers + the three `include_str!` template consts.
2. `crates/modal-rust-cli/src/templates/dev_app.py.tmpl`
3. `crates/modal-rust-cli/src/templates/deploy_app.py.tmpl`
4. `crates/modal-rust-cli/src/templates/call_app.py.tmpl`

After (3) and (4) the `crates/modal-rust-cli/src/templates/` directory is empty —
remove the empty directory too.

Exact commands:
```
git rm crates/modal-rust-cli/src/templates.rs \
       crates/modal-rust-cli/src/templates/dev_app.py.tmpl \
       crates/modal-rust-cli/src/templates/deploy_app.py.tmpl \
       crates/modal-rust-cli/src/templates/call_app.py.tmpl
rmdir crates/modal-rust-cli/src/templates   # now-empty dir
```

---

## 2. `crates/modal-rust-cli/src/main.rs` edits

### 2a. Module + imports

- Line 30: delete `mod templates;`.
- Line 39: delete `use templates::ShimParams;`.
- Line 33: change `use std::path::{Path, PathBuf};` → `use std::path::PathBuf;`
  (`Path` is used ONLY by deleted shim fns/tests; `PathBuf` is still used by the clap
  `project: PathBuf` fields at 73/85/103. The dispatcher's `&project` derefs to `&Path`
  at the `programmatic::*` call sites without `Path` needing to be in scope.)
- Line 34: delete `use std::process::Command;` (`Command` is used ONLY by `run_modal`,
  which is deleted; doctor.rs has its own `use std::process::Command;` and is unaffected).
- Line 36: `use anyhow::{Context, Result};` — KEEP UNCHANGED (`Context` still used by
  `InputArg::resolve` (150) and `runtime` (178)).

### 2b. Constants (lines 41-49)

- Delete `RUST_VER` (41-43) — only `shim_params` used it.
- Delete `DEFAULT_DEV_APP` (44-45) — shim-only.
- Delete `DEFAULT_CALL_APP` (46-47) — shim-only.
- KEEP `DEFAULT_DEPLOY_APP` (48-49) — used by the live `deploy`/`call` clap defaults.

### 2c. Module-doc comment (lines 1-26)

Rewrite the header doc so it no longer documents the fallback path. Concretely:
- Delete the "## Fallback path (`--use-shim` — KEPT, P10 removes)" section (lines 12-17).
- Delete the "## Default path (programmatic — P9)" heading wording that contrasts with a
  fallback; keep the substance (programmatic: build runner, `--describe`, drive the same
  `App` methods, emits no `.py`, spawns no `modal`).
- In the subcommand list (19-26): drop every `[--use-shim]` and the "or generate …
  (`--use-shim`)" alternatives, leaving the programmatic-only description per command
  (e.g. `run <entrypoint>` — programmatic ephemeral run; `deploy <entrypoint>` —
  programmatic persistent deploy; `call <entrypoint>` — programmatic `from_name` + invoke;
  `doctor [--rust]` — OFFLINE preflight).

### 2d. Clap `Commands` enum (lines 62-127)

- `Doctor` (67-77): delete the `use_shim: bool` field (74-76). Update the doc on `Doctor`
  (64-66) to drop "checked ONLY with --use-shim" (state: auth always; cargo/rustc/panic
  under `--rust`). Keep `rust` (68-70) and `project` (71-73).
- `Run` (80-95): delete the `use_shim: bool` field (91-94). Update the `Run` doc (78-79)
  to drop the `--use-shim` alternative. Keep `entrypoint`, `project`, `input`, `timeout`.
  (`timeout` doc at 88-89 says "the shim pins timeout=1800" — reword to match the
  programmatic note, e.g. "informational; the decorator/run-path timeout applies".)
- `Deploy` (98-111): delete the `use_shim: bool` field (107-110). Update the `Deploy`
  doc (96-97) to programmatic-only. Keep `entrypoint`, `project`, `app`.
- `Call` (114-126): delete the `use_shim: bool` field (122-125). Update the `Call` doc
  (112-113) to programmatic-only. Keep `entrypoint`, `app`, `input`.

### 2e. `run(cli)` dispatcher (lines 181-241) — collapse each branch to the programmatic arm

- `Doctor` arm (183-187): remove `use_shim` from the destructure; change the call to
  `Ok(doctor::run(rust, &project))` (drop the `use_shim` arg — see §3).
- `Run` arm (188-206): remove `use_shim` from the destructure; delete the
  `if use_shim { cmd_run_shim(...) } else { … }` (196-205), keeping ONLY the
  programmatic arm:
  ```
  let input_json = input.resolve()?;
  runtime()?.block_on(programmatic::cmd_run_programmatic(
      &entrypoint, &project, input_json, timeout,
  ))
  ```
- `Deploy` arm (207-222): remove `use_shim`; collapse to
  `runtime()?.block_on(programmatic::cmd_deploy_programmatic(&entrypoint, &project, &app))`.
- `Call` arm (223-239): remove `use_shim`; collapse to
  ```
  let input_json = input.resolve()?;
  runtime()?.block_on(programmatic::cmd_call_programmatic(&entrypoint, &app, input_json))
  ```

### 2f. Delete the shim helper functions (lines 243-374)

Delete the entire block, contiguous, lines 243-374:
- `generated_dir` (243-246)
- `write_shim` (248-257)
- `shim_params` (259-278)
- `run_modal` (280-289)
- `cmd_run_shim` (291-331)
- `cmd_deploy_shim` (333-349)
- `cmd_call_shim` (351-374)

After this, the only items left between `run()` and `#[cfg(test)]` are: nothing
(the programmatic helpers all live in `programmatic.rs`).

### 2g. Tests (lines 376-581) — delete the dead shim/source-scan tests, keep the live ones

KEEP (they test surviving code — `InputArg`):
- `input_defaults_to_prototype_default` (501-505)
- `input_inline_passthrough` (507-513)
- `input_at_file_read` (515-524)

DELETE (test deleted code):
- `proto_params` helper (380-412) — builds `ShimParams`.
- `dev_shim_byte_equivalent_to_prototype` (416-421)
- `deploy_shim_byte_equivalent_to_prototype` (423-428)
- `call_shim_byte_equivalent_to_prototype` (430-435)
- `dev_shim_injects_package_qualified_build` (437-459)
- `deploy_shim_injects_package_qualified_build` (461-473)
- `dev_shim_never_emits_gpu_kwarg` (475-488)
- `deploy_shim_never_emits_gpu_kwarg` (490-499)
- `generated_dir_is_under_modal_rust` (526-530) — tests deleted `generated_dir`.
- `programmatic_path_has_no_codegen_or_modal_subprocess` (532-558) — a source-scan test
  asserting the programmatic path has no `templates::`/`write_shim`/`run_modal`/
  `Command::new("modal")`. With the shim path GONE there is no other path to contrast
  against; this is dead and references the deleted-symbol names in string form. DELETE.
- `modal_subprocess_only_in_shim_path` (560-580) — asserts `fn cmd_run_shim` etc. exist
  (they no longer do) and `fn run_modal` exists (deleted). DELETE.

Result: the `#[cfg(test)] mod tests` block keeps only `use super::*;` + the three
`InputArg` tests. (`use super::*;` still needed for `InputArg`.)

> Note: the two source-scan tests are FROM P9 and exist to prove the P9 separation
> between the programmatic and shim paths. P10 removes the shim path, so the property
> they guarded ("the default path has no codegen / no modal subprocess") is now trivially
> true and structurally enforced (there is no codegen code left in the crate). Deleting
> them is correct, not a coverage regression. The real coverage — that the programmatic
> path drives cargo + `--describe` and never spawns `modal` — is now intrinsic to
> `programmatic.rs` (which is the entire run/deploy/call surface) and exercised by the
> live re-confirm + `programmatic.rs`'s own unit tests (schema / manifest / envelope).

---

## 3. `crates/modal-rust-cli/src/doctor.rs` edits

Drop the `modal`-CLI requirement and the shim branch; keep auth (always) + `--rust`
(cargo/rustc/panic-abort).

- Delete `check_modal_cli` (lines 75-99) — the only function that checked `modal` on `$PATH`.
- `run(...)` signature (line 279): change
  `pub fn run(with_rust: bool, with_shim: bool, project_dir: &std::path::Path) -> i32`
  → `pub fn run(with_rust: bool, project_dir: &std::path::Path) -> i32` (drop `with_shim`).
- Banner (281-285): delete the `if with_shim { … } else { … }` block; replace with a
  single line, e.g.
  `println!("(programmatic path; the modal CLI is not required — auth + --rust cargo/rustc only)");`
  Keep the `--rust` banner line (286-288).
- Check vector (294-297): delete the `if with_shim { checks.insert(0, check_modal_cli()); }`
  block (295-297). The vector starts as `vec![check_modal_credentials()]` and only the
  `--rust` block (298-302) appends cargo/rustc/panic. KEEP all of that.
- Module doc (1-13): drop the first bullet (`modal` on `$PATH`) from the "Checks, in order"
  list (4); keep credentials + `--rust` (cargo/rustc/panic). Optionally tidy the
  `run(...)` doc (265-278) to drop the `with_shim` description.
- `use std::process::Command;` (line 16): KEEP — still used by `capture_version` (60),
  which `check_cargo`/`check_rustc` use. (Only `check_modal_cli` is removed; `capture_version`
  stays.)

doctor.rs tests (335-408): KEEP ALL — they exercise `release_profile_panic`,
`manifest_declares_workspace`, `fail_envelope`, `check_panic_profile`. None reference
`check_modal_cli` or `with_shim`. No change needed.

---

## 4. `crates/modal-rust-cli/src/programmatic.rs` — NO CHANGES

Keep entirely as-is. This is the path now. (Self-check after the edits: `programmatic.rs`
imports `crate::workspace` (KEEP) and `modal_rust::{App, DeployConfig, FunctionConfig,
RemoteConfig}` (runtime crate, untouched). It does NOT import `templates` or any deleted
symbol — confirmed by grep.)

---

## 5. `crates/modal-rust-cli/Cargo.toml`

No DEPENDENCY removals: the shim path used NO template engine — `templates.rs` was a
hand-rolled `.replace()` renderer over `include_str!`-embedded `.tmpl` files (no
`tera`/`handlebars`/`askama`/etc.). Every current dep is still used by the programmatic
path + clap surface:
- `clap` — the CLI commands. KEEP.
- `serde` + `serde_json` — manifest parse (programmatic.rs) + doctor envelopes. KEEP.
- `anyhow` — `Result`/`Context` throughout. KEEP.
- `modal-rust` — the facade the programmatic path drives. KEEP.
- `tokio` (`rt-multi-thread`, `macros`) — `runtime()` in main.rs + `block_on`. KEEP.

The ONLY edit is the `description` (line 5), which still says "The legacy Python-shim
path is retained behind `--use-shim`." Reword to drop that clause, e.g.:
`"The `modal-rust` binary: drives the programmatic SDK/facade (run/deploy/call via `modal_runner --describe` + crates/modal-rust) — no codegen, no `modal` CLI. clap lives here (CLI-only), never in the runtime."`
(Cosmetic; not load-bearing for the build.)

---

## 6. Post-deletion verification (gates on default-members, per WORKING.md)

Run, in order, all must be GREEN:
1. `cargo fmt --check`
2. `cargo clippy --all-targets -- -D warnings`  (catches any now-unused import/const:
   the spec already removes `Path`, `Command`, `RUST_VER`, `DEFAULT_DEV_APP`,
   `DEFAULT_CALL_APP` — clippy/`-D warnings` is the backstop.)
3. `cargo build`
4. `cargo test`  (the three `InputArg` tests + all `programmatic.rs`/`doctor.rs`/
   `workspace.rs` tests must still pass; the deleted shim/source-scan tests are gone.)

Then the light LIVE re-confirm (CPU, ephemeral, cheap; the programmatic path is
unchanged so it should just work, NO `--use-shim` flag exists anymore):
- `modal-rust run add --input '{"a":40,"b":2}'` → envelope `{"ok":true,"value":{"sum":42}}`
  (or the equivalent example entrypoint). Modal flakiness ⇒ RETRY (transient), not a code fault.
- Optionally `deploy` + `call` for full coverage; `run` alone satisfies the light re-confirm.

---

## 7. Self-consistency checklist (no dangling refs after edits)

- [ ] `mod templates;` and `use templates::ShimParams;` removed from main.rs.
- [ ] `use std::path::{Path, PathBuf};` → `use std::path::PathBuf;` (Path no longer referenced).
- [ ] `use std::process::Command;` removed from main.rs (Command no longer referenced there).
- [ ] `RUST_VER` / `DEFAULT_DEV_APP` / `DEFAULT_CALL_APP` consts removed; `DEFAULT_DEPLOY_APP` kept.
- [ ] `use_shim` removed from all four clap commands + all four dispatcher arms.
- [ ] `generated_dir` / `write_shim` / `shim_params` / `run_modal` / `cmd_*_shim` removed.
- [ ] Shim/source-scan/gpu-kwarg tests removed; three `InputArg` tests kept.
- [ ] `templates.rs` + `templates/*.tmpl` + empty `templates/` dir removed.
- [ ] doctor.rs: `check_modal_cli` removed; `run` drops `with_shim`; banner + check-vector
      updated; `capture_version` + `use std::process::Command;` kept.
- [ ] programmatic.rs + workspace.rs untouched.
- [ ] Cargo.toml: no dep changes; description reworded only.
- [ ] Runner / facade / SDK / examples / workpads / README untouched.
- [ ] fmt / clippy -D warnings / build / test all green; live `run` → `{"sum":42}`.

RESULT: SPEC_DONE — wrote p10-spec.md
