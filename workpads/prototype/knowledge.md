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

Not passed yet.

No milestones executed. The gate passes only when this file records, with
evidence: (1) `modal-rust run add --input '{"a":40,"b":2}'` shows the runtime
cargo build AND `{"ok":true,"value":{"sum":42}}` in one `modal run` invocation;
(2) `modal-rust call add --input '{"a":40,"b":2}'` returns `{"sum":42}` from a
deployed Function; (3) the build boundary is proven both ways — `cargo build` in
the run function-body log and in the deploy/image-build log, **absent** from the
call log, with the deployed result stable until an explicit redeploy. M6 (the
cache speedup) is best-effort and does NOT gate; a null cache result is
acceptable.

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
