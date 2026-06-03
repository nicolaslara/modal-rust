# Prototype Knowledge

## Objective

Prove the `modal-rust` core path on the smallest function (`add` →
`{"sum":42}`): write a plain lib fn, run it remotely with a runtime build, deploy
it with a build-time build, and call the deployed Function — with the
run-vs-deploy build boundary observably proven. Build slowly, one boundary per
milestone (M0→M9), recording evidence before depending on it. This file captures
the reasoning, confidence, and what stays user-sensitive; the cockpit lives in
`tasks.md`, the canonical contracts in `../architecture/boundaries.md`, and the
authoritative locked decisions in `../architecture/research-synthesis.md`. The
design stances: the build boundary is the hard, non-negotiable invariant (`run`
builds at function-execution time, `deploy` builds at image-build time and the
deployed runtime never runs `cargo`); direct-execution-first (try a normal
`@app.function`, with a Modal Sandbox as a documented fallback if a Function-body
build proves infeasible); prefer static dispatch.

## Gate Status

MET (2026-06-03, after the M5–M8 loop).

M0–M8 executed and PASS with evidence (see the POC validation section below). The
gate is met because this file records, with evidence: (1) the runtime `cargo build`
AND `{"ok":true,"value":{"sum":42}}` in one `modal run dev_app.py::main` invocation
(M4/M5), reactive to local edits with no redeploy (M5: 42 → 43 → 42); (2)
`{"ok":true,"value":{"sum":42}}` from a deployed Function via `modal run
call_app.py::main` (M7/M8); (3) the build boundary proven both ways — `cargo build`
present in the run function-body log and in the deploy/image-build log, **absent**
from every call log, with the deployed result stable until an explicit redeploy
(edit → still 42; redeploy → 43; checkout + redeploy → 42). M6 (the cache speedup)
is best-effort and did NOT gate; the accepted result is a null/neutral warm speedup.
M9a (CLI wrapper byte-equivalence) and M9b (`doctor`) remain for the next loop and
are outside this gate.

## Decisions

Carried from the synthesis §2 (authoritative). Items marked **[CHANGED]** folded
in a HIGH-severity review `must_fix`. Record prototype-local refinements below as
milestones produce evidence.

- **Design stances (build boundary is the hard invariant; direct-execution-first
  with a Sandbox fallback; prefer static dispatch).** (1)
  **Direct-execution-first; Sandbox is a documented fallback** — try the core path
  on a normal `@app.function` first (runtime compile in the Function body, M4); if
  that proves infeasible for a step, iterate to a Modal Sandbox for that step and
  record the decision. Sandboxes are a fallback explicitly on the table, not a ban
  and not out of scope. (2) **The build boundary is the product** (the hard,
  non-negotiable invariant): `run` builds at function-execution time, `deploy`
  builds at image-build time and the deployed runtime executes only the prebuilt
  binary, never `cargo` — this holds whether the build runs in a Function body or a
  Sandbox. Every deploy task proves `cargo build` in deploy/build logs and ABSENT
  from call logs. (3) **Prefer static dispatch** — favor `enum`/generics/`impl
  Trait` over `dyn Trait` (the registry uses `fn` pointers, not `Box<dyn>`; see the
  Registry decision below).
- **v0 authoring = generated Python + the official `modal` CLI** (the known-good
  control path). `modal-rs` is confined to the later `call` path behind a
  validated `--use-modal-rs` flag — out of scope for this gate (§2.7).
- **The user does not own `main()`.** Library crate + a CLI-owned ~15-line
  `src/bin/modal_runner.rs` whose `main()` is
  `modal_rust_runtime::run_cli(user_crate::modal_registry())`. The user authors
  only `lib.rs` + `pub fn modal_registry() -> Registry` (§2.1).
- **`modal-rust-runtime` has zero Modal/network/Python deps** (serde, serde_json,
  anyhow, a tiny hand-rolled arg parser). `clap` is CLI-only — NOT a runtime
  dependency. Rationale: this crate is recompiled every dev run and baked into
  the deploy image; keep the recompiled/baked artifact minimal (§2.1).
- **Frozen runner seam.** `modal_runner --entrypoint <name> ( --input-json |
  --input-file | --input-stdin )`. stdout carries EXACTLY ONE JSON envelope; all
  cargo/rustc/user diagnostics → stderr. Exit code mirrors `ok` (0/1); the error
  kind lives in the JSON, not the exit code. `--input-file`/`--input-stdin` exist
  so the shim can write large inputs to `/tmp` and avoid argv-length / payload
  limits (§2.2).
- **[CHANGED — Rust #3] Five frozen error kinds:** `decode_error`,
  `unknown_entrypoint`, `function_error`, `encode_error`, `panic`. Frozen
  precedence: top-level JSON parse → entrypoint lookup → decode `In` → call →
  encode `Out` (malformed JSON + bad entrypoint → `decode_error`). `encode_error`
  was added so an output-serialization failure is never mis-reported as `panic`.
  **User errors are wrapped on the top-level enum:** `function_error` is the
  user's `Err` wrapped on the `RunnerError` enum — `message` = the Display/anyhow
  chain, and the failure envelope gains an **optional additive `details`** field =
  `serde_json::to_value(&e).ok()` when the handler's error type is `Serialize`
  (else `null`):
  `{"ok":false,"error":{"kind":"function_error","message":"<Display/anyhow chain>","details":<serialized user error|null>,"backtrace":"..."}}`.
  Future envelope additions must be additive optional fields (`details` follows
  this; `meta`/`version` reserved) (§2.2).
- **[CHANGED — Rust #5] Codec-neutral, [CHANGED — Rust #4] sync handler — static
  dispatch.** Handler is a bare `fn` pointer:
  `type HandlerFn = fn(&[u8]) -> Result<Vec<u8>, RunnerError>;` — no `dyn`, no
  vtable, no `Box`. `typed!(f)` is a `macro_rules!` that generates a monomorphized
  wrapper `fn` and yields its pointer; the wrapper owns decode/encode via a `Codec`
  (JSON for v0) and wraps the user error on `RunnerError`. `typed_async!` is
  reserved now (same `fn`-pointer shape) so async and CBOR are additive — neither
  touches `HandlerFn`/`Registry` (§2.3).
- **[CHANGED — Rust MED] Registry** = `BTreeMap<&'static str, HandlerFn>`
  (static-str keys, `fn`-pointer values — no allocation, no `dyn`); builder
  `Registry::new().function("add", typed!(add))`. Duplicate names rejected with a
  hard error at startup (not silent last-write-wins). Arguments are always a single
  named JSON object, never a positional array (§2.3).
- **Macro-compatibility invariant:** every entry is a monomorphized `typed!`
  wrapper reduced to the same `HandlerFn` pointer; `run_cli`/`Registry` never
  change shape regardless of registration path. The manual registry and the future
  `inventory`/`#[modal_rust::function]` path (which generates the same wrapper + an
  `inventory` registration of the `fn` pointer, or a static `match` table) converge
  on one static-dispatch code path (§2.3, project.md).
- **[CHANGED — Rust MED] `panic = "unwind"` required** for `catch_unwind` to
  upgrade a panic into the `panic` envelope. `doctor --rust` detects
  `panic = "abort"`; the runner builds under a profile / `--config` override
  forcing unwind. M0 asserts NOT abort (§2.6).
- **run path:** `from_registry('rust:1.<pin>-slim', add_python='3.12').entrypoint([])`,
  `.env({"RUST_BACKTRACE":"1"})`, `add_local_dir(LOCAL_SRC,'/src',copy=False, ignore=[...])`.
  **[CHANGED — Modal #1]** CLI flags routed via `@app.local_entrypoint()` (a bare
  `@app.function` does not auto-bind `modal run` flags). **[CHANGED — Modal #2]**
  Build into a known-writable LOCAL path (`CARGO_TARGET_DIR=/tmp/target`), NOT a
  Volume by default; `CARGO_HOME` may sit on a Volume earlier (lower risk).
  **[CHANGED — Modal MED]** If `/src` is read-only, `cp -a /src /tmp/build`.
  `timeout=1800` (300 s default too low for cold compile); never `vol.reload()`
  mid-build (§2.4).
- **deploy path:** `add_local_dir(src,'/app/src',copy=True)` +
  `run_commands('cd /app/src && cargo build --release --bin modal_runner', 'cp …/release/modal_runner /app/modal_runner && chmod +x …')`
  at image-build time; the deployed body execs ONLY `/app/modal_runner`, mounts no
  source, mounts no cache Volume, and never calls cargo. `add_python` still
  required (the image must host Modal's Python runtime even though the workload is
  native). **[CHANGED — Modal #6]** `call` logic lives in a `@app.local_entrypoint()`
  to avoid a module-scope `NameError` (§2.5).
- **No web endpoint in v0.** The deployed `add` is callable only via
  `Function.from_name().remote()` / the generated call shim. Invoked arg + return
  are plain `str` (the JSON envelope), well under the ~100 MB gRPC limit (§2.5,
  §4 Q2).
- **Recommended pins (defaults; confirm before locking):** `rust:1.83-slim` +
  `add_python='3.12'` as the single image backing run AND deploy; JSON wire format
  for v0; a single shared `modal-rust-cargo-cache` Volume (§4 Q4–Q6).

## Findings

Seeded verified facts most load-bearing for the prototype (full table +
confidence in the synthesis §1). Append empirical spike results per milestone.

- **Runtime compile feasible in principle** (high): a normal `@app.function` can
  run subprocesses (Modal's own `subprocess.run(['nvidia-smi'])` examples); the
  Python requirement is satisfiable on a Rust base via `add_python`; the
  filesystem is writable (`/tmp` guaranteed; default 512 GiB); timeouts go to
  24 h. M4 is the empirical confirmation.
- **`add_local_dir(copy=False)`** adds files at container startup (not an image
  layer), enabling fast redeploy and source-edit reactivity (high). **Open:**
  whether those mounts are read-only or writable in place — the single biggest
  unverified assumption; M2's write-probe resolves it and gates M4.
- **`add_local_dir(copy=True)`** copies into an image layer at build time (Docker
  COPY-like), required so a later `run_commands` cargo build can see the source
  (high). Layers cascade: a change in the `copy=True` source busts that and all
  later layers → full rebuild per deploy (Residual Risk #3; mitigation =
  dependency-prebuild layer, documented).
- **`run_commands`** runs shell at image-build time with verified network access
  (docs show `git clone`/`apt`) — the basis for deploy-time crates.io egress
  (high). Re-confirm on this account in M7; `--vendor` (`cargo vendor`) is the
  recorded fallback if egress is restricted.
- **A bare `rust:` image is an invalid Function image**; `add_python` (3.11/3.12
  documented by example) is mandatory (high). A custom base ENTRYPOINT must exec
  its args; `.entrypoint([])` neutralizes it so Modal's Python runtime starts.
  M3 asserts `which -a python python3` to catch interpreter shadowing.
- **`modal run app.py::fn --flag val` auto-binds CLI flags ONLY for
  `@app.local_entrypoint()`**, NOT a bare `@app.function` (high). Hence M1 proves
  arg routing via a local entrypoint, and the call shim uses one too.
- **Volume caching** (high): pointing a tool-cache env var at a Volume path is
  Modal's first-class pattern (`HF_HOME` etc.), directly analogous to
  `CARGO_HOME`. Background commits run "every few seconds"; `vol.reload()` fails
  "volume busy" if files are open (Cargo holds locks) — avoid on the hot path.
  **Open on Modal:** whether warm-rebuild speedup survives a network FS's many
  small stat/read ops — M6 must benchmark; a null result is acceptable and does
  not block deploy.
- **`.remote()` payload ceiling ~100 MB** gRPC; the scalar JSON envelope for `add`
  is far under it. `--input-file`/`--input-stdin` exist to avoid argv limits for
  larger runner inputs (§2.2, Residual Risk #8).
- **`modal-rs` (0.1.3) is unofficial, pre-1.0, single-maintainer** (high);
  `FunctionCreate` still needs a Python `function_serialized` (does NOT remove the
  Python shim); serde-pickle protocol 2/3 vs cloudpickle 4 caveat. Confirms
  generated-Python-for-authoring; `modal-rs` only for `call` behind a flag.

## Open Questions

Prototype-relevant; each has a recommended default in the synthesis §4 (none block
M0–M3). Record the empirical answer when a milestone resolves it.

- **Mount writability** (gates M4): is `add_local_dir(copy=False)` `/src`
  read-only or writable? → resolved by M2's write-probe; the design sidesteps it
  via copy-to-`/tmp` if read-only.
- **`add_python` coexistence** (M3): does a system `python3` in `rust:slim` shadow
  the standalone `add_python`? → M3 asserts `which -a` + the resolved path.
- **Build-time egress on this account** (M7): does `run_commands` reach crates.io
  here? → verified by docs; re-confirm in M7; `--vendor` fallback recorded.
- **Warm cache speedup vs network FS** (M6): real or erased by small stat/read
  ops? → benchmark in M6; a null result is acceptable and does not block deploy.
- **Cold-start build latency**: can a cold full compile approach the 1800 s
  timeout for larger dep graphs? → record the M4 cold wall-clock as the M6
  baseline.
- **Default `call` invoke mode** (§4 Q3): generated `call_app.py` via `modal run`
  for v0; promote `modal-rs` `Function::from_name().remote()` to default only
  after a non-scalar round-trip smoke. (Out of scope for the gate.)
- **GPU / deploy cost confirmation** (§4 Q1): require an explicit `--yes` for
  persistent deploys and any GPU run, with a per-run cost note. (Cost-sensitive —
  confirm with the user before persistent/GPU spend.)

## POC validation (2026-06-03)

First empirical loop. M0 through M4 executed and all PASS; M5–M9b not run this
loop (M4 was the stopping point). Git untouched throughout.

### CENTRAL VERDICT

**YES — runtime-compile-in-a-Function-body works.** A single
`modal run dev_app.py::main --entrypoint add --input-json '{"a":40,"b":2}'` showed
the in-body `cargo build` (`Compiling example-add` + `Finished in 9.34s`) AND
returned `{"ok":true,"value":{"sum":42}}` from the freshly-compiled-in-Function-body
binary. M4 returned sum=42 from a fresh build. The hypothesis that decides the
architecture is confirmed on the direct path.

**Sandbox fallback: NOT indicated.** Direct `@app.function` execution succeeded; the
documented Sandbox fallback (stance 1) is not needed. The one error during the loop
was a transient `DEADLINE_EXCEEDED` gRPC control-plane auth timeout during teardown
of the `{}` run, which occurred AFTER the envelope was already returned — infra
noise, not a build/runner failure.

### M0 — local runner contract (pass)

Full runner protocol validated locally, no Modal. Gates green: `cargo fmt --check`
clean, `cargo clippy --all-targets --all-features -- -D warnings` clean,
`cargo test --workspace` 13/13 pass.
- Success: `add {"a":40,"b":2}` → `{"ok":true,"value":{"sum":42}}`, exit 0, exactly
  one stdout envelope.
- All five frozen error kinds verified with correct frozen envelopes + non-zero exit:
  `unknown_entrypoint`; `decode_error` (both malformed `not-json` AND wrong-shape
  `{"a":1}`); `function_error` with null `details` (`fail`) AND populated `details`
  (`fail_structured` → `details:{"code":42,...}`, confirming the Serialize-details
  path); `encode_error` (the "key must be a string" case — surfaced as `encode_error`,
  NOT `panic`); `panic` with a non-empty backtrace.
- Precedence: malformed JSON + bad entrypoint → `decode_error` (top-level parse
  precedes entrypoint lookup), as frozen.
- Runner seams: `--input-file` and `--input-stdin` both → 42.
- Release profile confirmed `panic = "unwind"`; a release build also produces the
  `panic` kind (so `catch_unwind` upgrades panics in release, not just debug).

### M1 — Python-shim control path (pass)

`control` target authenticated via `~/.modal.toml` and printed the remote
`uname -a`: `Linux modal 4.4.0 #1 SMP Sun Jan 10 15:06:54 PST 2016 x86_64
GNU/Linux`. The Modal control plane (auth, app authoring, subprocess, result
marshalling) and CLI-arg routing via a `@app.local_entrypoint()` work end to end.
Full `dev_app.py` written at
`/Users/nicolas/devel/modal-rust/workpads/prototype/dev_app.py` carrying all four
milestone targets: `control` (M1), `mount` (M2), `toolchain` (M3), and
`run_entrypoint`/`main` (M4).
- Contract note: the design (tasks.md M1 acceptance) permits proving arg-routing via
  a matching `@app.local_entrypoint()`; here `control` is the bare `@app.function`
  body and `control_main` is the printing local_entrypoint. Acceptance met as
  designed.

### M2 — source mount + writability (pass)

`modal run dev_app.py::mount_main` mounted source at startup (not as an image layer).
- `target/` and `.git` ABSENT remotely — client-side `ignore` applied (only
  `/src/.gitignore` present).
- Remote sha256 of `/src/Cargo.toml` =
  `e5432ef8e279eccd8fc747900adacf37721ab91ac27441dc365150ff274b8bb1`, byte-equal to
  the local `shasum` of `/Users/nicolas/devel/modal-rust/Cargo.toml`.
- **Mount writability: WRITABLE.** The biggest unverified assumption is resolved —
  the `copy=False` mount is writable in place. This unblocks the M4 build location:
  M4 may build in `/src` directly with `CARGO_TARGET_DIR=/tmp/target` (no read-only
  `cp -a /src /tmp/build` detour needed). Modal run URL:
  `https://modal.com/apps/nicolaslara/main/ap-ga93PfaAx0Np8869d0gIHb`.
- Contract divergence (surfaced to orchestrator, acceptance satisfied either way):
  tasks.md M2 describes the probe as `mount_probe` at `/workspace` with
  `ignore=['target','.git']`; the actual fixture implements it as `mount`/`mount_main`
  at `/src` with `ignore=["target",".git",".modal-rust","**/*.rlib"]`. The fixture's
  actual target was run.

### M3 — Rust+Python toolchain image (pass)

Image `rust:1-slim + add_python="3.12"` hosts BOTH the Rust toolchain and Modal's
Python runtime: cargo 1.96.0, rustc 1.96.0, Python 3.12.1 (`python`/`python3` resolve
to `/usr/local/bin`). The Function started cleanly via
`modal run dev_app.py::toolchain_probe`.
- Negative control confirmed: bare `rust:1-slim` WITHOUT `add_python` fails as an
  invalid Function image (`ConflictError`) — `add_python` is mandatory, as expected.
- Probe-shim gotcha discovered and worked around: (a) the `::NAME` selector matches
  the entrypoint's **Python function name**, not its registered `name=` tag; and (b) a
  function + entrypoint sharing the same Python name silently shadows the Function (its
  `.remote` is lost), so a naive collision "completes" with ZERO probe output. Working
  pattern: the `@app.local_entrypoint()` is Python-named `toolchain_probe` and calls a
  distinctly-named bare `@app.function` `_toolchain_probe_fn.remote()`. New file:
  `/Users/nicolas/devel/modal-rust/workpads/prototype/dev_app_no_python.py` (negative
  control). This naming hazard should inform the M9a shim generator.

### M4 — runtime compile in Function body (pass — THE key validation)

See CENTRAL VERDICT above. Detail:
- Invocation: `modal run …/dev_app.py::main --entrypoint add --input-json
  '{"a":40,"b":2}'` (disambiguated with `::main` because the shim hosts multiple
  local_entrypoints — `control_main`, `mount_main`, `toolchain_probe`, `main`; `main`
  is exactly the contract's `main(entrypoint, input_json)`). Single log showed BOTH
  the cargo build AND `{"ok":true,"value":{"sum":42}}`.
- **Build location:** built in-place in the writable `/src` mount (per the M2
  WRITABLE result) with `CARGO_TARGET_DIR=/tmp/target` and `CARGO_HOME=/tmp/cargo` —
  a local writable path, NOT a Volume (Modal review HIGH #2 honored).
- Failure propagation: `--entrypoint will_panic` propagated a structured `panic`
  envelope; `{}` (empty input) propagated `decode_error` — never silent success.
- `timeout=1800` set on `run_entrypoint`.

### Observed image / build timing

- **Cold first-build wall-clock: ~9.34s** (`Compiling example-add` → `Finished in
  9.34s`). This is the M6 cache-speedup baseline. Small dep graph — cold-start build
  latency is far from the 1800s timeout for `add`; larger dep graphs remain the open
  risk M6 should probe.
- Mount applies client-side `ignore` at startup; uploaded bytes small (only source,
  target/.git excluded). Exact byte/wall-clock for the mount upload not separately
  captured this loop (early signal, non-gating per M2).

### Mount-writability result

**WRITABLE** in place (M2 write-probe). Consequence for downstream: M4 built directly
in `/src`; M6's target-dir caching is on the writable-in-place branch (both
`CARGO_HOME` and `CARGO_TARGET_DIR` are candidates for Volume warming) rather than the
read-only `cp -a` cliff branch.

### Not executed this loop (M5–M9b)

M5 (source-edit reactivity), M6 (cache Volume, best-effort), M7 (deploy-time build),
M8 (deploy no-compile invariant), M9a (CLI wrapper byte-equivalence), M9b (`doctor`
preflight + `panic=abort` detection) were not run; the loop stopped after the M4
central validation. The deploy half of the build boundary (cargo in deploy log,
ABSENT from call log) remains UNPROVEN — the gate is not yet met. M5/M6/M7/M9b have
their dependencies satisfied and are ready to run next; M8 and M9a are blocked on M7
and M8 respectively.

### Blocker — Modal workspace task-execution denied (2026-06-03 ~16:00)

The M5–M8 attempt stalled: every `modal run` crash-loops with
`could not fetch task data: 'The caller does not have permission to execute the
specified operation' … App <X> does not have write access to app <X>. Contact your
workspace administrator to update permissions.` Reproduced on a TRIVIAL app
(`return "hello"`, no mount/build) → **workspace-wide, not our code**. Read ops are
healthy (`modal profile current` → `nicolaslara`, `modal app list` works), so auth +
control plane are up; only container/task *execution* is denied. M1–M4 executed
fine ~15:48; execution broke by ~15:58 → a Modal-side account/workspace state change
(likely a usage/credit/billing limit disabling execution, or a plan/permission
change) or a Modal incident.

**Resume path:** workspace admin checks Modal usage/credits/billing + workspace
permissions (and status.modal.com for an incident); when a trivial `modal run`
returns again, re-run M5→M8 (`.claude/workflows/` loop, or resume the saved
`modal-rust-poc-loop-deploy` script). No code change needed — `examples/add` +
`dev_app.py` are validated (M0–M4). M5–M8 remain pending/blocked on Modal.

**RESOLVED (2026-06-03 ~16:15):** transient Modal execution incident, not our code
and not the account. By 16:14 a trivial 1-function app ran fine, and
`modal run workpads/prototype/dev_app.py::main` returned `{"ok":true,"value":{"sum":42}}`
in ~14s (in-body cargo build 6.65s, exit 0). Reads were healthy throughout; only
remote task execution was briefly denied (~15:58–16:14). M5–M8 are unblocked.

### Second loop — M5–M8 executed, deploy half of the boundary PROVEN (2026-06-03 PM)

The blocker cleared and the deploy half of the build boundary was completed. M5–M8
all PASS with real evidence. The prototype gate is now MET (run AND deploy/call both
proven; the build boundary proven both ways). Git was edited then restored via
`git checkout` throughout; the working tree is clean (lib.rs diff empty).

#### M5 — source-edit reactivity (pass)

Proven: the same `dev_app.py::main` run shim reflects local source edits with NO
redeploy and NO image rebuild — `copy=False` re-uploads current source each run.
- Baseline: `modal run …/dev_app.py::main --entrypoint add --input-json
  '{"a":40,"b":2}'` → `{"ok":true,"value":{"sum":42}}` (runtime function-body cargo
  build).
- Edit: changed ONLY `examples/add/src/lib.rs` line 37 `sum: input.a + input.b` →
  `input.a + input.b + 1` (`git diff` = exactly one `-`/one `+` line); re-ran the same
  shim → `{"ok":true,"value":{"sum":43}}`. Source re-uploaded via `copy=False`,
  `example-add` recompiled in the function body (~6.26s). One transient Modal
  `PermissionDenied` recurred and cleared on a single ~20s retry (resilience protocol).
- Revert: `git checkout -- examples/add/src/lib.rs` (tree clean); re-ran the same shim
  → `{"ok":true,"value":{"sum":42}}` (recompiled in function body ~9.60s).
- Build boundary held: every run did a runtime `cargo build` in the function body
  (`Compiling example-add … Finished release profile`), with NO `modal deploy` and NO
  image rebuild between runs. The only thing changing across 42 → 43 → 42 was the
  single source byte (`+ 1`); all three compile cycles came from the mounted `/src`,
  never a baked binary.

#### M6 — cargo-cache Volume, cold-vs-warm timing (pass; accepted NULL/neutral speedup)

Cached run path added: Volume `modal-rust-cargo-cache` mounted at `/cache`, `CARGO_HOME`
on the Volume, `CARGO_TARGET_DIR=/tmp/target` (ephemeral), no `vol.reload()` on the
hot path. Correctness held both runs.
- Cold (empty volume, just created): `{"ok":true,"value":{"sum":42}}`, wall-clock
  **14.40s**.
- Warm (no source change): `{"ok":true,"value":{"sum":42}}`, wall-clock **29.20s**.
- **Net result: NULL / neutral (warm not faster — slower here).** This is the
  documented bound, not a surprise: with `CARGO_TARGET_DIR=/tmp/target` ephemeral every
  run, only `CARGO_HOME` download/index time is warmed (NOT compile time). For this
  12-crate graph that saving is tiny and dominated by compile-time variance. A
  null/neutral speedup is an ACCEPTABLE pass per the milestone contract; a cache miss
  only ever cost time, never correctness.
- `modal volume list` shows `modal-rust-cargo-cache | 2026-06-03 16:27 CEST |
  nicolaslara`; `modal volume ls` shows it contains `cargo` (persisted CARGO_HOME).
  Reset: `modal volume rm modal-rust-cargo-cache`.
- Build boundary intact: this is the `run` (dev) path only; cargo builds in the
  function body at execution time, never on a deploy/runtime path.

#### M7 — deploy-time build, binary baked at IMAGE-BUILD time (pass)

`timeout 360 modal deploy …/deploy_app.py` succeeded ("App deployed in 19.033s").
Build log shows cargo compiling at IMAGE-BUILD time and the cp baking the binary:
- Step 1 `RUN cd /app/src && cargo build --release --bin modal_runner` →
  `Compiling modal-rust-runtime v0.0.0` / `Compiling example-add v0.0.0
  (/app/src/examples/add)` / `Finished \`release\` profile [optimized] target(s) in
  4.84s` (crate downloads present → build-time crates.io egress CONFIRMED). 14
  `Compiling` lines in the build log.
- Step 2 `RUN cp /app/src/target/release/modal_runner /app/modal_runner && chmod +x
  /app/modal_runner` (binary baked into the image layer).
- Throwaway run: `timeout 300 modal run …/deploy_app.py::main` returned
  `{"ok":true,"value":{"sum":42}}` from the baked binary; the call log has ZERO
  cargo/Compiling/Updating-crates lines (the deployed runtime never runs cargo). The
  deployed body sets the input to `/tmp/in.json` and execs ONLY `/app/modal_runner
  --entrypoint <entrypoint> --input-file /tmp/in.json` (no cargo/mount/volume), with
  `@app.local_entrypoint() main(entrypoint="add", input_json='{"a":40,"b":2}')`.

#### M8 — deployed runtime does NOT compile; deploy invariant VERDICT (pass)

Verdict: **the deploy invariant HOLDS — the deployed runtime never recompiles.**
`cargo build` appears ONLY in deploy/build logs and is ABSENT from every call log; the
deployed result is stable under local edits and changes only on an explicit redeploy.
- Call-routing + base call: `modal deploy …/deploy_app.py` then `modal run
  …/call_app.py::main --entrypoint add --input-json '{"a":40,"b":2}'` →
  `{"ok":true,"value":{"sum":42}}`, with NO cargo/Compiling/rustc lines in the call log
  (`from_name(...).remote()` arg path proven through `call_app.py`).
- Stability under local edit: editing local source did NOT change the deployed result
  (still 42) — the deployed binary is frozen until redeploy.
- Redeploy reactivity: edited `lib.rs` to `a+b+1`, `modal deploy …/deploy_app.py`
  (build log: `cargo build --release --bin modal_runner`, `Compiling example-add
  v0.0.0`, `Finished release profile … in 6.27s`), then `modal run …/call_app.py::main`
  → `{"ok":true,"value":{"sum":43}}`, with NO cargo/compiling/rustc in the call log.
  (A stale warm container from the prior deploy briefly served the old image returning
  42; after it scaled down the new image served 43 — a deployment-rollover timing
  effect, not a build-boundary violation. The 43 confirms the new cargo build is baked
  and the call path never recompiles.)
- Restore: `git checkout -- examples/add/src/lib.rs` (restored `a+b`), `modal deploy
  …/deploy_app.py` once more (`Compiling example-add`, `Finished release … in 5.22s`),
  then `modal run …/call_app.py::main` → `{"ok":true,"value":{"sum":42}}`, no cargo in
  the call log. `lib.rs` git diff empty (restored). Full sequence: 42 → (local edit)
  still 42 → (redeploy) 43 → (git checkout + redeploy) 42.
- Files: `/Users/nicolas/devel/modal-rust/workpads/prototype/call_app.py` (created),
  `/Users/nicolas/devel/modal-rust/workpads/prototype/deploy_app.py` (used as-is),
  `/Users/nicolas/devel/modal-rust/examples/add/src/lib.rs` (edited then reverted via
  `git checkout`). Persistent app `modal-rust-add-poc` confirmed deployed.

### GATE STATUS — MET (after the M5–M8 loop)

The prototype gate is now **MET**. Both halves proven with real evidence:
- **run** (dev path): a single `modal run dev_app.py::main` shows the runtime
  function-body `cargo build` AND `{"ok":true,"value":{"sum":42}}` (M4/M5), reactive to
  local edits with no redeploy (M5: 42 → 43 → 42).
- **deploy + call**: `modal deploy …/deploy_app.py` bakes the binary at IMAGE-BUILD
  time (M7: `Compiling example-add` + `Finished release … in 4.84s` + cp), and `modal
  run …/call_app.py::main` returns `{"ok":true,"value":{"sum":42}}` from the deployed
  Function (M7/M8).
- **build boundary proven BOTH ways:** `cargo build` is PRESENT in the run
  function-body log and in the deploy/image-build log, and ABSENT from every call log;
  the deployed result is stable until an explicit redeploy (edit → still 42; redeploy →
  43; checkout + redeploy → 42). M6 (cache speedup) is the accepted best-effort
  null/neutral result and does not gate.

Human commands for the README:
- Deploy:  `modal deploy /Users/nicolas/devel/modal-rust/workpads/prototype/deploy_app.py`
- Call:    `modal run /Users/nicolas/devel/modal-rust/workpads/prototype/call_app.py::main`

### Third loop — M9-build / M9b / M9a executed, modal-rust CLI proven (2026-06-03 PM)

M9-build (CLI build + offline gates), M9b (`doctor`), and M9a (CLI wrapper
byte-equivalence) all PASS with real evidence. The `modal-rust` CLI now exists as a
pure wrapper over the prototype shims; Modal was up and responsive throughout the M9a
live run (NOT blocked — no transient incident this loop). Git was edited then restored
via `git checkout`; the working tree is clean.

#### M9-build — modal-rust CLI builds; offline workspace green (pass)

The `modal-rust` CLI is a **pure wrapper** exposing `doctor`/`run`/`deploy`/`call`, all
with `--help`. It introduces NO new Modal capability — it generates the dev/deploy/call
shims into `<workspace-root>/.modal-rust/generated/` (workspace root detected by walking
up for `[workspace]`, mounted as `/src`), then execs the official `modal` CLI. Public
`--input <json|@file>` lowers to `modal run <shim>::main --entrypoint <e> --input-json
<json>`; `deploy` → `modal deploy <shim>`. The shims are embedded as templates with only
the documented injected params (app name, RUST_VER, source path) substituted —
byte-identical to the prototype shims. Errors surface as the runner-envelope-shaped
structured error (`{"ok":false,"error":{kind,message,details,backtrace}}`) with a
non-zero exit.
- Workspace green: `cargo fmt --check` clean; `cargo clippy --all-targets
  --all-features -- -D warnings` clean; `cargo test --workspace` 6/6 suites OK (16 CLI +
  11 runtime tests, 27 total).
- `cargo run -p modal-rust-cli -- doctor` exits 0 in this env (modal 1.3.2,
  `~/.modal.toml`, rustc/cargo 1.96.0, release `panic=unwind`); all four subcommands
  expose `--help`.
- Generated shims are byte-equivalent to the prototype fixtures (empty `diff`, equal
  sha256 on all three) and correctly gitignored under `.modal-rust/`. No `gpu=` path; no
  Modal calls made.
- **VERDICT: pass** — CLI builds, is a pure wrapper, workspace gates green, doctor exits
  0 offline, shims byte-identical, no Modal calls.

#### M9b — `doctor` preflight + `panic=abort` detection (pass)

`doctor` is its own boundary (structured-error preflight + abort-profile detection in
isolation), reusing the M0 5-kind error model.
- OFFLINE accuracy: reports modal CLI 1.3.2, creds from `~/.modal.toml` (env-var
  `MODAL_TOKEN_ID`+`MODAL_TOKEN_SECRET` also recognized); `--rust` adds cargo/rustc
  1.96.0 and release `panic="unwind"`. The line-scan parser tracks `[profile.release]`
  and ignores `[profile.dev]` (unit-tested).
- **`panic=abort` detection present + correct:** workspace-root/standalone `panic =
  "abort"` → `panic_abort_profile` FAIL exit 1; a member-abort under an unwind root →
  unwind OK (resolved release profile). Default unwind profile passes.
- Simulated missing prerequisite (exit 1): `env -i PATH=<empty> HOME=<no .modal.toml>`
  → `[FAIL] modal CLI not found on $PATH`, with an actionable runner-envelope-shaped
  error on stderr:
  `{"ok":false,"error":{"kind":"missing_prerequisite","message":"...","details":{"prerequisite":"modal CLI","remediation":"Install the Modal CLI..."},"backtrace":""}}`.
- Quality gates: `cargo test -p modal-rust-cli` → 16/16 pass (6 doctor tests); clippy
  `-D warnings` exit 0; `cargo fmt --check` exit 0.
- Minor (non-blocking) note: §8 mentions doctor should also surface pinned
  python/image-builder versions; the bare `doctor` reports modal CLI version + creds
  only (rust toolchain versions appear under `--rust`). A §8-completeness nicety, not in
  the M9b acceptance bullets and not a correctness bug. Source:
  `/Users/nicolas/devel/modal-rust/crates/modal-rust-cli/src/doctor.rs`.
- **VERDICT: pass** — doctor OFFLINE accurate; abort-detection present + correct;
  missing-prereq → `missing_prerequisite` runner-envelope error exit 1; 16/16 tests,
  clippy + fmt clean.

#### M9a — modal-rust CLI is a byte-equivalent wrapper (run/deploy/call) (pass)

The public UX proven live against Modal — one `modal-rust` binary generates the shims
and orchestrates the build stage; the user never touches Modal Python. Modal (client
1.3.2, `~/.modal.toml`) was up and responsive — NO transient incident this loop. Every
`modal run`/`deploy` was wrapped in `timeout`; no `gpu=` path used.
- **(a) `modal-rust run add --input '{"a":40,"b":2}'`** → runtime function-body cargo
  build + `{"ok":true,"value":{"sum":42}}` (reproduces M4/M5).
- **(b) `modal-rust deploy add`** → `cargo build --release` ran at IMAGE-BUILD time
  (Step 1 RUN), binary baked via `cp /app/src/target/release/modal_runner
  /app/modal_runner` (Step 2); deployed under `modal-rust-add-poc` (reproduces M7).
- **(c) `modal-rust call add --input '{"a":40,"b":2}'`** → `{"ok":true,"value":{"sum":42}}`
  (inner value `{"sum":42}`), with NO cargo/compile lines anywhere in the call log
  (reproduces M8, the deploy invariant).
- **Shim equivalence (the anti-divergence guard):** generated
  `.modal-rust/generated/{dev_app,deploy_app,call_app}.py` are byte-for-byte identical
  to `workpads/prototype/{dev_app,deploy_app,call_app}.py` (all three `diff` exit 0;
  matching sha256; CLI defaults match the prototype injected params). All three shims
  are gitignored (private, per §8/§10).
- Offline still stands: `cargo build -p modal-rust-cli` clean; `cargo test -p
  modal-rust-cli` 16/16 green including the three byte-equivalence tests and the
  `panic=abort`/unwind doctor tests (M9-build, M9b).
- **VERDICT: pass** — CLI pure-wrapper reproduces shims: run→runtime build +
  `{"sum":42}`, deploy→build-time bake (`modal-rust-add-poc`), call→`{"sum":42}` with no
  cargo in the call log; all 3 generated shims diff-identical to prototype refs (exit 0)
  and gitignored; offline build + 16/16 tests green.

#### M9 milestone consequence — the modal-rust CLI now exists

With M9-build and M9b (and M9a) all passing, the **`modal-rust` CLI now exists** as a
real binary. The README's `modal-rust run/deploy/call` commands are therefore real for
the `doctor` + offline (shim-generation / byte-equivalence) surfaces. The wrapper
`run`/`deploy`/`call` against live Modal were also exercised and PASS this loop (M9a),
so they are real end-to-end as well — Modal was healthy throughout; no Modal-blocked
note applies. (README CLI commands left for the human to review before adding to the
Try-it section.)

### M0-R — panic-capture robustness review (2026-06-03, completed)

The M0-R follow-up is resolved in `crates/modal-rust-runtime/src/lib.rs`
(panic-capture: lib.rs:338-408). Decisions recorded:

- **Backtrace via `force_capture()` (no env dependency).** The panic hook always
  uses `std::backtrace::Backtrace::force_capture()`, so the `panic` envelope's
  `backtrace` is populated in every context — local `modal_runner --entrypoint
  will_panic` with no `RUST_BACKTRACE` set now yields a full backtrace, not
  `"backtrace":""`. The decision (force_capture vs env-gated) is force_capture.
- **Per-thread capture, hook installed once (no process-global race).** The old
  process-global `Mutex<Option<(String,String)>>` slot + per-call hook swap is
  replaced by a `thread_local! PANIC_SLOT` plus a `std::sync::Once` (`HOOK_INIT`)
  that installs the process-wide hook exactly once. The hook writes the panicking
  thread's `(message, backtrace)` to its own thread-local; `run_handler` clears its
  slot before `catch_unwind` and reads it back after — so concurrent panics on
  different threads (e.g. the parallel test harness) never race.
- **`set_var` removed (edition-2024 safe).** The M0 test no longer calls
  `std::env::set_var("RUST_BACKTRACE", ...)` (was `unsafe` under edition 2024,
  rust-analyzer E0133). force_capture removes the need to mutate the env at all.
- **Test un-ignored + stress-passed.** `panic_captured_with_backtrace` is NOT
  `#[ignore]`d; it asserts `kind=="panic"`, a non-empty message, AND a non-empty
  backtrace with NO env var set. De-flaked across 25× `cargo test -p
  modal-rust-runtime panic_captured_with_backtrace -- --exact` (FLAKED=0) and 5×
  full `cargo test` (concurrent harness, multiple deliberate panics) — 0 flakes.
- **Protocol unchanged.** The 5-kind `RunnerError` taxonomy, the frozen panic
  envelope, exit codes, `typed!`/`Registry`/`HandlerFn`, and the `panic="unwind"`
  profile are all left intact — this is a capture-mechanism hardening, not a
  protocol change. Acceptance command (`env -u RUST_BACKTRACE`, built via
  `-p example-add`) returns `kind:"panic"` with a full symbolicated backtrace and
  exit 1; stderr 0 bytes (default panic message suppressed by the hook).
- **Gates green (default-members, OFFLINE).** `cargo fmt --check` exit 0; `cargo
  clippy --all-targets -- -D warnings` exit 0 (0 warnings); `cargo test` all suites
  green (runtime crate 11 passed, 0 ignored).

### CLI package-qualified shim build + `--gpu` passthrough (2026-06-03, completed)

Two CLI changes landed on top of M9a, both preserving the byte-equivalent-wrapper
invariant. Full GPU-side detail is mirrored in
`workpads/gpu-compute/knowledge.md` (same dated note).

- **Package-qualified shim build (`-p <pkg>` derived from `--project`) — fixes the
  multiple-`modal_runner`-bins regression.** Four workspace members
  (`example-add`, `example-add-macro`, `example-burn-add`,
  `example-cuda-vector-add`) each expose a `modal_runner` bin, so the bare
  `cargo build --release --bin modal_runner` the shims used to emit became
  ambiguous and failed. The CLI now derives `PACKAGE` from `--project` (e.g.
  `examples/add` → `example-add`) and the generated shims build
  `cargo build --release -p <PACKAGE> --bin modal_runner` (in dev `run_commands`
  and deploy `run_commands` alike). The ambiguous bare `--bin modal_runner` was
  removed from both templates AND the prototype `dev_app.py`/`deploy_app.py` refs.
  `example-burn-add` is excluded from `default-members` (CUDA-only) — expected and
  correct; `-p example-burn-add` still resolves on a CUDA host / Modal because it
  stays a workspace member.
- **`run`/`deploy` accept `--gpu <spec>` (verbatim passthrough).** When present,
  the spec lands as a `gpu="<spec>"` kwarg on the work `@app.function` only (no GPU
  catalog re-implemented — a bad type surfaces Modal's own error). When absent, no
  `gpu=` kwarg is emitted, so the no-GPU shims stay byte-identical to the prototype
  refs. The GPU shim differs from no-GPU ONLY by the injected `gpu=` kwarg — no
  other byte changes; runner seam (`--input-file /tmp/in.json`, single stdout
  envelope) and the call-time exec-only-`/app/modal_runner` invariant unchanged.
- **Gates green (default-members, package-qualified).** `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings`, and `cargo test` all exit 0; new
  guards: `dev_shim_injects_package_qualified_build`,
  `deploy_shim_injects_package_qualified_build`,
  `dev_shim_no_gpu_kwarg_when_absent`, `dev_shim_injects_gpu_kwarg_verbatim`,
  `deploy_shim_injects_gpu_kwarg_verbatim`, plus the dev/deploy/call
  byte-equivalence checks.
- **Modal acceptance PASSED (not blocked, not retry-pending).** `run add` built
  `-p example-add` cleanly → `{"ok":true,"value":{"sum":42}}`;
  `run gpu_info --gpu T4` → envelope `exit_code:0` with `nvidia-smi` showing a
  Tesla T4 (Driver 580.95.05, CUDA 13.0). First try, no git touched.
- **Files:** `crates/modal-rust-cli/src/{templates.rs,workspace.rs,main.rs}`,
  `crates/modal-rust-cli/src/templates/{dev_app,deploy_app,call_app}.py.tmpl`.

### M6b — sccache dev cache (2026-06-03)

**Status: COMPLETED experiment** — the sccache artifact-cache spike ran, was
cold/warm/edit-benchmarked, and ends with a recorded DECISION. The honest net
speedup is null/negative on `add`'s tiny graph; that modest/null result is still a
completed experiment (the mechanism is proven correct), not a failure.

**Approach (vs M6's CARGO_HOME-only path):** M6 warmed only `CARGO_HOME`
(crates.io index + downloads) on a Volume, so it saved download time but NEVER
compile time — hence M6's accepted null. M6b instead caches COMPILED ARTIFACTS:
`sccache` as `RUSTC_WRAPPER` with `SCCACHE_DIR` on a dedicated Modal Volume
(`modal-rust-sccache`). sccache is content-addressable, so it sidesteps cargo's
single-writer target-dir + network-FS locking that defeats a Volume-mounted
`CARGO_TARGET_DIR`. `CARGO_TARGET_DIR` stays on local `/tmp`; only the
content-addressed object cache lives on the Volume. sccache installed at
image-build time via the prebuilt static musl binary (v0.15.0), NOT `cargo
install`. Shim: `workpads/prototype/dev_app_sccache.py`.

Goal (tasks.md M6b, boundaries.md §7): cache COMPILED ARTIFACTS so a warm `run`
rebuild skips recompiling unchanged crates — fixing M6's null result (M6 warmed only
CARGO_HOME/downloads, never compile time). Approach: `sccache` as `RUSTC_WRAPPER`,
`SCCACHE_DIR` on a Modal Volume (`modal-rust-sccache`), content-addressable so it
sidesteps cargo's single-writer target-dir + network-FS locking. CARGO_TARGET_DIR
stays on local `/tmp`; only the content-addressed objects live on the Volume.
sccache installed via prebuilt static musl binary (v0.15.0) at image-build time
(fast — NOT `cargo install`). Shim: `workpads/prototype/dev_app_sccache.py`.

Cache RESET first for a true cold run 1: `modal volume delete modal-rust-sccache --yes`
(recreated empty by `create_if_missing=True`). All three runs returned
`{"ok":true,"value":{"sum":42}}`. The run-3 edit was `sum: input.a + input.b + 0`
(forces an `example-add` recompile but keeps the result 42), reverted via
`git checkout -- examples/add/src/lib.rs` (tree clean, verified).

| run | scenario | sccache hits/misses | hit rate | in-body cargo build | total wall-clock (real) | result |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | COLD (empty volume) | 0 / 16 (16 compilations) | 0.00 % | 25.41s | 33.09s | 42 ✓ |
| 2 | WARM, no source change | 16 / 0 (0 compilations) | 100.00 % | 139.13s | 149.21s | 42 ✓ |
| 3 | WARM + 1-line edit | 15 / 1 (1 compilation) | 93.75 % | 7.93s | 15.90s | 42 ✓ |

**Honest verdict: sccache's cache CORRECTNESS is perfect — exactly the predicted
hit pattern (cold all-miss; warm-unchanged 100% hits, 0 recompiles; edit hits the 15
deps + recompiles only the edited crate). But the warm wall-clock speedup is NEGATIVE
for this tiny crate.** Run 2 hit 100% of cache entries yet was ~4.5x SLOWER than the
cold run (139s vs 25s build). Cause: `SCCACHE_DIR` is on a Modal Volume (network FS);
reading 16 cached objects back over the network FS, plus sccache server round-trips,
costs far more than just locally recompiling a 16-crate graph. For `add` (≈16 small
crates, each compiling in well under a second), recompilation is cheaper than a
network-FS cache fetch — so the cache "works" but loses on net time.

Comparison to the M6 baseline (cold ~14s, warm null/neutral 29s, CARGO_HOME-only):
sccache adds real artifact caching (M6 had none) and proves correctness, but on this
graph it is **net-negative warm and only modestly positive on the edit path** (run 3
at 15.9s real beats the cold 33.1s — incremental edits are where it helps, because
only 1 of 16 crates recompiles). Even run 3's win is partly the local recompile being
cheap, not the cache fetch being fast.

**Where this WOULD pay off:** a real dependency-heavy project (hundreds of crates,
multi-minute cold builds) inverts the economics — fetching a cached `.o` over the
network is far cheaper than recompiling a crate that takes seconds-to-minutes, so a
100%-hit warm rebuild would be dramatically faster. The `add` POC is precisely the
worst case for any non-local cache: per-crate compile cost ≈ per-object fetch cost.

**Mitigation that would likely flip this positive even for small graphs:** keep
sccache but point `SCCACHE_DIR` at LOCAL disk and sync the (small) cache dir
to/from the Volume around the build (snapshot-restore), instead of having sccache
read/write each object over the network FS during compilation — same conservative
pattern §7 already prescribes for the target-dir snapshot strategy. Not benchmarked
this loop.

**DECISION: do NOT wire sccache on by default. Wire it behind an explicit
`--cache`/`--sccache` flag, OFF by default, and pair it with the local-SCCACHE_DIR +
Volume-snapshot-sync strategy before promoting it.** Rationale: (1) correctness is
proven and a miss only ever costs time (never a wrong result — `RUSTC_WRAPPER` is
transparent and errors loudly), so it is safe to expose; (2) the direct
Volume-`SCCACHE_DIR` wiring tested here is net-negative for small graphs and is a dev
UX regression if defaulted on; (3) it is plausibly a large win for real
dependency-heavy crates, which is exactly the audience a `--cache` flag targets.
This is an experiment with an honestly-modest/mixed result for the POC crate — not an
overclaim. Reset documented: `modal volume delete modal-rust-sccache --yes`.

- Logs: `/tmp/sccache_run1_cold.log`, `/tmp/sccache_run2_warm.log`,
  `/tmp/sccache_run3_edit.log`. Modal up/responsive throughout; no git committed.
