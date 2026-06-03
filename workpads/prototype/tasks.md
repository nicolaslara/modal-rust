# Prototype Tasks

## Objective

Prove the whole `modal-rust` core path on the smallest possible function — `add`
— by validating one boundary at a time (M0→M9), transcribing the corrected
M0–M9 milestone plan from `../architecture/research-synthesis.md` §3 (synthesis
M9 is split here into M9a — the pure-wrapper claim — and M9b — `doctor` — so each
proves exactly one new assumption). The
deliverable is a working walking skeleton, not a complete product: `add` written
as an ordinary Rust library function, run remotely with a **runtime build**,
deployed with a **build-time build**, and called on the deployed Function
returning `{"sum":42}` — with the run-vs-deploy build boundary observably
proven. Validate the happy path on a normal `@app.function` (direct-execution-first);
if a Function-body build proves infeasible at M4, a Modal Sandbox is the documented
fallback (record it) — the build boundary holds either way. The build boundary is
the hard invariant: `run` builds at function-execution time, `deploy` builds at
image-build time and the deployed runtime never invokes `cargo`. Each milestone
fails for exactly one reason and proves exactly one new boundary. Do not build a
later task before the current boundary is proven with evidence.

## Gate

The prototype gate passes when `knowledge.md` records, with evidence: `add` runs
via **`modal-rust run add --input '{"a":40,"b":2}'`** — a single `modal run`
invocation showing the **runtime build** (cargo output on stderr) AND
`{"ok":true,"value":{"sum":42}}` from the freshly built `modal_runner` — and
`add` is callable via **`modal-rust call add --input '{"a":40,"b":2}'`** against
a deployed Function returning `{"sum":42}`, **with the build boundary proven both
ways:** `cargo build` appears in the `run` function-body log and in the
`deploy`/image-build log, and is **ABSENT from the `call` log**; the deployed
result is stable until an explicit redeploy (edit local source → call still
returns the old value; redeploy → new value, with the new cargo build only in the
new deploy's build log). Best-effort items (the M6 cache speedup) do NOT block the
gate; a null cache result is acceptable.

DAG (verified acyclic, §3): `M0 → M1`; `M1 → {M2, M3}`; `{M0, M2, M3} → M4`;
`M4 → {M5, M6, M7}`; `M7 → M8`; `{M5, M8} → M9a`; `M0 → M9b`. `M6` (cache) is
best-effort and **not** a dependency of M7, M9a, or M9b. (M9 is split into M9a —
the pure-wrapper claim — and M9b — `doctor` — so each fails for exactly one reason;
see those tasks.)

### Flag mapping (authoritative — fixed once, used everywhere)

Two distinct CLI surfaces must not be conflated:

- **Public `modal-rust` CLI:** `modal-rust run|call <entrypoint> --input <json|@file>`
  (and `--gpu`/`--timeout`/`--app-name` per §2.7). `--input` accepts inline JSON or
  `@file`.
- **Generated shim, invoked via the official `modal` CLI:** the shim defines a
  single `@app.local_entrypoint() main(entrypoint, input_json=...)` (synthesis
  §2.4/§2.5). `modal run` auto-binds flags **only** to a `@app.local_entrypoint()`
  by parameter name — so the raw flags are `--entrypoint <name> --input-json <json>`,
  and the binder forwards them via `.remote()` to the bare `@app.function` body. A
  bare `@app.function` does **not** bind `modal run` flags (Modal review HIGH #1,
  §1.5/§2.4).

**Lowering rule (the M9a anti-divergence guard):** the public
`modal-rust run/call <entry> --input <json|@file>` lowers to the shim invocation
`modal run <shim>.py --entrypoint <entry> --input-json <json>`; a large or `@file`
input is written to `/tmp/in.json` and the runner is fed `--input-file` (the runner
seam, §2.2). Therefore: **every raw `modal run` evidence line below uses
`--input-json` against the `main` local_entrypoint, never `--input`, and never a
`::<inner_fn>` selector that names a bare `@app.function`.** `--input` appears only
on `modal-rust` wrapper lines (M9a). Probe/diagnostic targets selected with
`::<name>` (M2/M3/M7) name a `@app.local_entrypoint()` of that name, not a bare
`@app.function`.

## M0 - Local dispatcher + runner contract (no Modal) [scaffold]

Status: completed

risk: low. depends_on: []

Validates: the full runner protocol locally — name → monomorphized `typed!`
wrapper (fn pointer) → bytes/JSON in → JSON out, with all five error kinds (the
`details`-carrying envelope), exit codes, panic capture, and the frozen envelope —
before any network. Lays the cargo workspace scaffold (`crates/modal-rust-runtime`
+ `examples/add`).

Acceptance:
- Workspace exists with `crates/modal-rust-runtime` (providing `Registry`, the
  `typed!` macro, `HandlerFn = fn(&[u8]) -> Result<Vec<u8>, RunnerError>`
  (static dispatch — no `Box<dyn>`/`Handler` trait), the JSON `Codec`, the runner
  protocol, and `run_cli`) and `examples/add` defining
  `add(AddInput) -> anyhow::Result<AddOutput>`, `modal_registry()` registering it
  via `Registry::new().function("add", typed!(add))`, and `src/bin/modal_runner.rs`.
- `examples/add` ALSO registers named test entrypoints used to exercise the
  remaining error kinds (each is a real registry entry so the exact commands below
  are reproducible, and `will_panic` is the SAME entrypoint M4 reuses so M0 and M4
  stay consistent):
  - `fail` — returns `Err(anyhow!(...))` → `function_error`.
  - `bad_encode` — returns an `Out` that fails to serialize (e.g. a map with
    non-string keys, or an `f64::NAN` field) → `encode_error` (must NOT surface as
    `panic`).
  - `will_panic` — `panic!(...)` in the handler body → `panic` (captured via the
    panic hook + `catch_unwind`).
- `modal_runner --entrypoint add --input-json '{"a":40,"b":2}'` prints exactly
  `{"ok":true,"value":{"sum":42}}` on stdout with exit code 0; stdout carries
  exactly one JSON envelope (all diagnostics on stderr).
- All FIVE error kinds are exercised, each with the exact frozen schema and a
  non-zero exit: `unknown_entrypoint`; `decode_error` (malformed JSON AND
  wrong-shape JSON); `function_error` (handler returned `Err`); `encode_error`
  (output failed to serialize); `panic` (handler unwound, captured via panic hook
  + `catch_unwind`).
- Precedence test: malformed JSON + bad entrypoint yields `decode_error` (top-level
  JSON parse precedes entrypoint lookup), not `unknown_entrypoint`.
- The release build is asserted NOT `panic = "abort"` (the unwind profile /
  `--config` override is in place so `catch_unwind` produces the `panic` kind).
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  and `cargo test --workspace` all pass.

Evidence:
- Captured stdout + exit code for the success case and for each of the five error
  kinds; the precedence-test output (→ `decode_error`).
- `cargo build -p modal-rust-runtime --bin modal_runner`
- `…/target/debug/modal_runner --entrypoint add --input-json '{"a":40,"b":2}'` →
  `{"ok":true,"value":{"sum":42}}`, `echo "exit=$?"` → `exit=0`
- `…/modal_runner --entrypoint nope --input-json '{}'; echo "exit=$?"` →
  `unknown_entrypoint`, non-zero exit
- `…/modal_runner --entrypoint add --input-json 'not-json'; echo "exit=$?"` →
  `decode_error` (malformed JSON), non-zero exit
- `…/modal_runner --entrypoint add --input-json '{"a":1}'; echo "exit=$?"` →
  `decode_error` (wrong-shape JSON — missing field `b`), non-zero exit
- `…/modal_runner --entrypoint fail --input-json '{"a":40,"b":2}'; echo "exit=$?"` →
  `function_error`, non-zero exit
- `…/modal_runner --entrypoint bad_encode --input-json '{"a":40,"b":2}'; echo "exit=$?"` →
  `encode_error` (NOT `panic`), non-zero exit
- `…/modal_runner --entrypoint will_panic --input-json '{}'; echo "exit=$?"` →
  `panic` (message + backtrace populated via `RUST_BACKTRACE=1`), non-zero exit
- Precedence test: `…/modal_runner --entrypoint nope --input-json 'not-json'; echo "exit=$?"`
  → `decode_error` (top-level JSON parse precedes entrypoint lookup), NOT
  `unknown_entrypoint`, non-zero exit
- `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --workspace`
  (green; clippy/fmt clean)

## M1 - Generated Modal Function runs a shell command (control path, no Rust)

Status: completed

risk: low. depends_on: [M0]

Validates: the Python-shim control plane end to end (Modal auth, app authoring,
subprocess, result marshalling) AND that a CLI-passed argument reaches the
function body via a `@app.local_entrypoint()` (a bare `@app.function` does NOT
auto-bind `modal run` flags — Modal review HIGH #1).

Acceptance:
- Generated `dev_app.py` defines a normal `@app.function` (no web endpoint, no
  Sandbox) whose body runs `subprocess.run(['uname','-a'])` and returns its
  stdout; a `@app.local_entrypoint()` parses `--cmd` and forwards it via `.remote()`.
- `modal run dev_app.py --cmd 'uname -a'` prints the remote `uname -a` output
  (`Linux … x86_64`).
- A CLI-passed value is echoed back from the body, proving argument routing reaches
  the function.
- A failing-command variant (`--cmd 'false'`) shows the captured non-zero exit +
  stderr (not silently dropped).

Evidence:
- `modal run /Users/nicolas/devel/modal-rust/workpads/prototype/dev_app.py --cmd 'uname -a'`
  (console shows `Linux … x86_64`; echoed CLI arg)
- `modal run /Users/nicolas/devel/modal-rust/workpads/prototype/dev_app.py --cmd 'false'`
  (captured non-zero exit + stderr)
- The generated shim contents + the exact CLI invocation, recorded in `knowledge.md`.
- Save the final `dev_app.py` shim as the M1 **reference fixture** (e.g.
  `workpads/prototype/shim-fixtures/dev_app.py`) for the M9a byte-equivalence diff.

## M2 - Source mount via `add_local_dir(copy=False)`

Status: completed

risk: medium. depends_on: [M1]

Validates: local source mounts at startup (not as an image layer), is visible at
the remote path, `ignore` patterns are applied client-side, content is
byte-identical; AND resolves mount writability — the single biggest unverified
assumption, which gates M4's build location.

Acceptance:
- Shim uses `add_local_dir(local_src, '/workspace', copy=False, ignore=['target','.git'])`;
  the probe BODY is a bare `@app.function` (`mount_probe`) that runs
  `find /workspace -maxdepth 2` and `sha256sum /workspace/Cargo.toml`; a
  `@app.local_entrypoint()` ALSO named `mount_probe` calls `mount_probe.remote()`
  (selected by `modal run …::mount_probe` — `modal run` binds the `::` selector and
  any flags only to a `@app.local_entrypoint()`, never to the bare `@app.function`,
  per the Flag-mapping note above and §1.5).
- Remote `find` lists the source tree with `target/` and `.git` ABSENT (client-side
  ignore applied).
- Remote `sha256` of `Cargo.toml` equals the local `shasum -a 256`.
- A **write-probe** in the same run (`touch /workspace/.write_probe`) records
  writable vs read-only (EROFS) — this result gates M4's build location.
- Wall-clock and approximate uploaded bytes recorded as an **early signal, not a
  gate**.

Evidence:
- `shasum -a 256 /Users/nicolas/devel/modal-rust/examples/add/Cargo.toml` (local hash)
- `modal run /Users/nicolas/devel/modal-rust/workpads/prototype/dev_app.py::mount_probe`
  (remote `find` output with `target/`/`.git` absent; side-by-side sha256 equal;
  write-probe writable | EROFS result; recorded wall-clock + approx uploaded bytes)

## M3 - Rust toolchain image with Modal's Python requirement satisfied

Status: completed

risk: medium. depends_on: [M1]

Validates: one image hosts both the Rust toolchain AND Modal's Python runtime; the
base ENTRYPOINT does not break the runtime; `add_python` and any system `python3`
coexist cleanly.

Acceptance:
- Image is `from_registry('rust:1.<pinned>-slim', add_python='3.12')`; a normal
  bare `@app.function` (`toolchain_probe`) returns `cargo --version`, `rustc
  --version`, `python --version`, AND `which -a python python3` with the resolved
  interpreter path; a `@app.local_entrypoint()` ALSO named `toolchain_probe` calls
  `toolchain_probe.remote()` so `modal run …::toolchain_probe` binds and runs it
  (flags bind only to the local_entrypoint, never the bare `@app.function`).
- All version strings are non-empty; `cargo` and `python` are both on `$PATH`.
- The Function STARTS cleanly (else the `entrypoint([])` workaround is recorded).
- Negative control: bare `from_registry('rust:1.<ver>-slim')` WITHOUT `add_python`
  is shown to fail as a Function image (or, if it unexpectedly works, that is
  recorded).
- Pins recorded: the rust tag, the `add_python` value, and `MODAL_IMAGE_BUILDER_VERSION`.

Evidence:
- `modal run /Users/nicolas/devel/modal-rust/workpads/prototype/dev_app.py::toolchain_probe`
  (cargo/rustc/python versions + `which -a`; clean start or recorded `entrypoint([])` workaround)
- `modal run /Users/nicolas/devel/modal-rust/workpads/prototype/dev_app_no_python.py::toolchain_probe`
  (negative-control result)
- Recorded pins: rust tag, `add_python` value, `MODAL_IMAGE_BUILDER_VERSION`.

## M4 - RUNTIME COMPILE in a Function body (the key validation)

Status: completed

risk: high. depends_on: [M0, M2, M3]

Validates: THE central claim — a normal `@app.function` can `cargo build` the
mounted source in its body, exec the freshly built `modal_runner`, and `add(40,2)`
→ 42, end to end, on the direct path (no Sandbox on the happy path). This is the
hypothesis that decides whether direct Function execution suffices or the Sandbox
fallback is needed (stance 1).

The `dev_app.py` shim is the §2.4 form: a bare `@app.function`
`run_entrypoint(entrypoint, input_json)` (the body that builds + execs) and a single
`@app.local_entrypoint() main(entrypoint, input_json="{}")` that calls
`run_entrypoint.remote(entrypoint, input_json)`. `modal run` binds `--entrypoint` /
`--input-json` to `main` by parameter name (per the Flag-mapping note); there is NO
`::run_add` target — `run_add` was a bare `@app.function` and would not bind flags.

Acceptance (ordered — the egress pre-check PRECEDES the expensive compile):
- On the M3 image with the M2 mount: the build location is derived from the M2
  write-probe (in-place if writable; else `cp -a /src /tmp/build` and/or set
  `CARGO_TARGET_DIR=/tmp/target`) — **local-writable, NOT a Volume** (Modal review
  HIGH #2).
- **Egress pre-check FIRST (gates the build):** before the full `cargo build`, the
  body runs a cheap egress probe (e.g. `cargo search anyhow` or a `curl -sfI
  https://static.crates.io/`) and records an explicit "egress confirmed" line. If it
  FAILS, switch to the `cargo vendor` path (the same fallback documented for M7) and
  record that — do NOT proceed into a long cold compile that will fail mid-download.
  This is an ordered acceptance bullet that precedes the build, not an artifact of it.
- The body then runs `cargo build --release --bin modal_runner` (logs → stderr), and
  execs it via the M0 protocol (`--input-file /tmp/in.json`).
- `modal run dev_app.py --entrypoint add --input-json '{"a":40,"b":2}'` →
  `{"ok":true,"value":{"sum":42}}`, with cargo build log lines appearing in the
  SAME invocation (compile at execution time).
- A deliberate failing entrypoint (`--entrypoint will_panic`, the M0-registered
  panic handler) propagates as a Modal failure / structured `panic` envelope (not
  silent success).
- `timeout=1800` recorded; the cold first-build wall-clock recorded (the M6 baseline).
- **Sandbox fallback branch (stance 1):** if the Function-body build proves
  infeasible for a hard reason (not merely a read-only mount, which the build-location
  step already handles) — e.g. the build cannot run or complete in a normal Function
  — do NOT declare the project blocked: evaluate a Modal **Sandbox**-based build for
  the `run` path, record the decision + why, and confirm the run-vs-deploy boundary is
  unchanged. The happy path remains direct Function execution; the Sandbox is the
  documented fallback.

Evidence:
- Egress-confirmed line (from the pre-check above) appears BEFORE any `cargo build`
  output, or the recorded `cargo vendor` fallback.
- `modal run /Users/nicolas/devel/modal-rust/workpads/prototype/dev_app.py --entrypoint add --input-json '{"a":40,"b":2}'`
  (single log showing BOTH cargo output AND the `{"sum":42}` result; build-location
  decision tied to the M2 probe; cold wall-clock + the `timeout=1800` used)
- `modal run /Users/nicolas/devel/modal-rust/workpads/prototype/dev_app.py --entrypoint will_panic --input-json '{}'`
  (failure-propagation evidence — not silent success)
- Save the final `dev_app.py` (runtime-build form) as the M4 **reference fixture**
  for the M9a byte-equivalence diff (supersedes M1's control-path fixture for the
  `run` shim).

## M5 - Source-edit reactivity

Status: completed

risk: low. depends_on: [M4]

Validates: the dev loop reflects local edits with no redeploy — `copy=False`
re-uploads the current source on each run.

Acceptance:
- `add(40,2)` → 42; edit `add` locally (return `a+b+1`), re-run → 43; revert,
  re-run → 42.
- No `modal deploy`, no image rebuild, and no manual cache busting between the three
  runs.

Evidence (raw `modal run` against the `main` local_entrypoint — `--input-json`, no
`::run_add` selector; see the Flag-mapping note):
- `modal run …/dev_app.py --entrypoint add --input-json '{"a":40,"b":2}'` → 42
- `sed -i '' 's/a + b/a + b + 1/' …/examples/add/src/lib.rs && modal run …/dev_app.py --entrypoint add --input-json '{"a":40,"b":2}'`
  → 43
- `git -C /Users/nicolas/devel/modal-rust checkout -- examples/add/src/lib.rs && modal run …/dev_app.py --entrypoint add --input-json '{"a":40,"b":2}'`
  → 42
- The three consecutive outputs (42 / 43 / 42) with timestamps; `git diff`
  confirming only source bytes changed between runs.

## M6 - Cargo-cache Volume (best-effort dev-iteration speedup)

Status: completed (best-effort; cold/warm benchmarked, correctness held both runs; warm speedup is the accepted null/neutral result — CARGO_HOME-only warming on the ephemeral target-dir path)

risk: medium. depends_on: [M4]

Validates: a Volume holding `CARGO_HOME` (+ optionally `CARGO_TARGET_DIR`) persists
across invocations so a warm rebuild is materially faster — and a cache miss only
costs time, never a wrong result. **Best-effort; NOT a dependency of M7 or M9
(neither M9a nor M9b) and does NOT block the gate.**

The cache expectation is BRANCHED on the M2 write-probe outcome (the read-only path
structurally defeats target-dir caching — be honest about it up front, not as a
surprise "null result"):
- **If M2 found the mount writable-in-place:** build in-place with `CARGO_TARGET_DIR`
  on a writable local path adjacent to the source, and benchmark promoting it to the
  Volume. Both `CARGO_HOME` (index/downloads) and `CARGO_TARGET_DIR` (compiled
  artifacts) can plausibly be warmed.
- **If M2 found the mount read-only:** every run does `cp -a /src /tmp/build` with
  `CARGO_TARGET_DIR` on ephemeral `/tmp` — so the target dir is COLD every run and
  ONLY `CARGO_HOME` (index/downloads via the Volume) can be warmed. Record UP FRONT
  that target-dir caching is out of reach for v0 on this path and that the
  warm-rebuild speedup is therefore bounded to `CARGO_HOME` hits (download time
  saved, NOT compile time). This is a real dev-iteration UX cliff, not a neutral null.

Acceptance:
- `Volume.from_name('modal-rust-cargo-cache', create_if_missing=True)` mounted at a
  STABLE path; `CARGO_HOME` on the Volume; `CARGO_TARGET_DIR` promotion follows the
  branch above (Volume only if writable-in-place AND it benchmarks net-positive and
  lock-safe; default stays `/tmp/target`).
- The chosen branch (writable vs read-only) is recorded explicitly, with the bound on
  expected speedup stated before the benchmark.
- On the read-only path ONLY, also test `CARGO_TARGET_DIR` on the Volume (the one case
  where it could conceivably help), accepting the lock/commit caveats, before
  declaring a null result. Capture the `cp -a /src /tmp/build` wall-clock SEPARATELY so
  the iteration-cost story is honest.
- Toolchain + mount path held constant; relies on automatic background/shutdown
  commits; **no `vol.reload()` on the hot path**.
- Cold (empty volume) vs second-run (no source change) wall-clocks recorded; warm is
  meaningfully faster OR the (branch-explained) bounded/null result is recorded as the
  deliverable.
- Correctness unchanged (42 on both runs).
- Documented cache reset (`modal volume rm` / a new name); single-writer /
  low-concurrency documented; parallel shared-cache writes out of scope for v0
  (last-write-wins noted).

Evidence:
- `modal volume create modal-rust-cargo-cache`
- `modal run …/dev_app.py --entrypoint add --input-json '{"a":40,"b":2}'` (×2)
  (two wall-clocks: cold vs warm + speedup or recorded bounded/null; both returned 42)
- The recorded M2-branch decision and the `cp -a` wall-clock (read-only path).
- `modal volume list` + the documented reset command.

## M7 - Deploy-time build (`copy=True` + `run_commands` cargo build, bake `/app/modal_runner`)

Status: completed

risk: high. depends_on: [M4]

Validates: source copied into an image LAYER, `cargo build --release` at
IMAGE-BUILD time via `run_commands` with crates.io egress, and the binary baked into
the image.

Acceptance:
- Deploy image is `add_local_dir(src,'/app/src',copy=True).run_commands('cd /app/src && cargo build --release --bin modal_runner','cp …/release/modal_runner /app/modal_runner && chmod +x /app/modal_runner')`
  with `add_python='3.12'` present and `entrypoint([])` neutralizing the base
  ENTRYPOINT.
- **Egress pre-check FIRST (gates the image-build compile, reusing the M4 guard):**
  the first `run_commands` step is a cheap egress probe (e.g. `cargo search anyhow`
  or `curl -sfI https://static.crates.io/`) that records an "egress confirmed" line
  BEFORE the `cargo build` layer. If it FAILS, the `--vendor` (`cargo vendor`)
  fallback is applied and recorded immediately — not after a deep, billed image
  build dies mid-download.
- The image build SUCCEEDS, proving build-time crates.io egress (verified-by-docs;
  re-confirm on this account). If it fails, the `--vendor` (`cargo vendor`) fallback
  is applied and recorded.
- A throwaway run confirms `/app/modal_runner` exists, is executable, and returns 42
  when exec'd directly. The `probe_binary` BODY is a bare `@app.function`; a
  `@app.local_entrypoint()` ALSO named `probe_binary` calls `probe_binary.remote()`
  so `modal run …::probe_binary` binds it (per the Flag-mapping note).
- The dependency-prebuild caching trick is documented as the cascading-rebuild
  mitigation.

Evidence:
- The "egress confirmed" line appears in the build log BEFORE the `cargo build`
  layer, or the recorded `cargo vendor` fallback.
- `modal deploy /Users/nicolas/devel/modal-rust/workpads/prototype/deploy_app.py`
  (image-build log showing `cargo build` compiling at BUILD time + the `cp`; crate
  downloads in the build log proving egress, or the recorded vendoring workaround)
- `modal run /Users/nicolas/devel/modal-rust/workpads/prototype/deploy_app.py::probe_binary`
  (`/app/modal_runner` exists, executable, → 42)
- Save the final `deploy_app.py` as the M7 **reference fixture** for the M9a
  byte-equivalence diff.

## M8 - Deployed runtime does NOT compile (the deploy invariant)

Status: completed

risk: medium. depends_on: [M7]

Validates: the deployed body only EXECs `/app/modal_runner` and never invokes cargo
— `cargo build` appears in deploy/build logs and is ABSENT from call logs; the
result is stable until an explicit redeploy.

> **Call-shim dependency (made explicit):** M8's evidence invokes the deployed
> Function through `call_app.py`, whose arg-routing path is the
> `@app.local_entrypoint() main(entrypoint, input_json) ->
> Function.from_name(APP,'call_entrypoint').remote(entrypoint, input_json)` form
> (§2.5, Modal review HIGH #6). This `from_name(...).remote()` cross-app lookup of a
> persisted function is a DIFFERENT code path from the M1 fresh-`modal run`
> arg-routing proof. The call-routing acceptance bullet below isolates and proves it
> (analogous to M1, but for the `from_name` path) BEFORE the no-cargo invariant is
> asserted, so a `from_name`/`.remote()` routing bug cannot masquerade as a deploy-
> invariant failure.

Acceptance:
- `modal deploy`; the deployed body execs only `/app/modal_runner --entrypoint add
  --input-file /tmp/in.json` — no cargo, no source mount, no cache Volume.
- **Call-routing proof (isolated first; gated on M7's deployed function):** the
  `call_app.py` `@app.local_entrypoint() main(entrypoint, input_json)` routes a
  CLI-passed arg through `Function.from_name(APP, 'call_entrypoint').remote(entrypoint,
  input_json)` to the deployed function and back. Demonstrated by passing a value the
  body echoes (or by `--entrypoint add --input-json '{"a":40,"b":2}'` → 42), proving
  the `from_name`/`.remote()` arg path works before relying on it for the invariant.
- A call (via the proven `call_app.py` local_entrypoint) → 42; the CALL logs contain
  NO compilation / cargo lines.
- Stability: repeated calls → 42; editing local source does NOT change the deployed
  result.
- Redeploy reactivity: change to `a+b+1`, `modal deploy`, calls → 43, with the new
  cargo build appearing only in the NEW deploy's build log.
- Negative check: `which cargo` in the runtime body fails, or (if the toolchain image
  still carries cargo) the body provably never calls it.

Evidence (raw `modal run` against the `call_app.py` `main` local_entrypoint —
`--entrypoint`/`--input-json`, no `::call_deployed --input`; see the Flag-mapping note):
- Call-routing proof: a CLI-passed arg observably round-trips through
  `Function.from_name(APP,'call_entrypoint').remote(...)` (echoed value or the 42
  result), recorded BEFORE the invariant evidence below.
- Deploy/build log WITH `cargo build` vs CALL log WITHOUT it (side by side — the
  build-boundary proof: present in deploy, ABSENT from call).
- `modal deploy …/deploy_app.py` then `modal run …/call_app.py --entrypoint add --input-json '{"a":40,"b":2}'`
  → 42
- `sed -i '' 's/a + b/a + b + 1/' …/examples/add/src/lib.rs && modal run …/call_app.py --entrypoint add --input-json '{"a":40,"b":2}'`
  → still 42 (deployed result unchanged by local edit)
- `modal deploy …/deploy_app.py && modal run …/call_app.py --entrypoint add --input-json '{"a":40,"b":2}'`
  → 43 (the sequence 42 → (edit) 42 → (redeploy) 43 with timestamps;
  runtime-body cargo-not-invoked check)
- Save the final `call_app.py` as the M8 **reference fixture** for the M9a
  byte-equivalence diff.

## M9a - modal-rust CLI is a byte-equivalent wrapper of the shims (run/deploy/call)

Status: blocked (depends_on M5 and M8; M8 not yet executed, so the wrapper-equivalence claim cannot be validated yet)

risk: medium. depends_on: [M5, M8]

Validates: the public UX — one `modal-rust` binary generates the shims and
orchestrates the build stage; the user never touches Modal Python. Introduces NO new
Modal capability: it is a **pure wrapper** that lowers `modal-rust run/deploy/call`
to the M1/M4/M7/M8 shims. This task fails for exactly ONE reason — the CLI-generated
shim diverging from the prototype shim (it carries no preflight/`doctor` behavior;
that is M9b).

Acceptance:
- `modal-rust run add --input '{"a":40,"b":2}'` reproduces M4/M5 (runtime build +
  42, source-edit reactivity).
- `modal-rust deploy add` reproduces M7/M8 (build-time build; deployed runtime does
  not compile).
- `modal-rust call add --input '{"a":40,"b":2}'` → 42.
- Flag lowering matches the authoritative Flag-mapping note: public `--input
  <json|@file>` lowers to the shim's `--entrypoint <entry> --input-json <json>` (large
  / `@file` inputs written to `/tmp/in.json` + `--input-file`). `--input` is a
  `modal-rust`-only surface; the generated shim parses `--input-json`.
- Generated shims remain private (gitignored under `.modal-rust/generated/`).
- **Byte-equivalence check (concrete, anti-divergence — the one guard that M9a does
  not become a second control path):** M1/M4/M7/M8 each save their shim as a recorded
  reference fixture (e.g. under `workpads/prototype/shim-fixtures/`). In M9a, generate
  the shim, blank out the known injected params via a documented normalization
  (entrypoint name, input path, app name, gpu/timeout, RUST_VER pin, local/manifest
  source path), and `diff` the normalized CLI-generated shim against the normalized
  reference; the diff MUST be empty. Record the exact `diff` command and its empty
  output.
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  and `cargo test` are clean for the CLI crate.

Evidence:
- `cargo run -p modal-rust-cli -- run add --input '{"a":40,"b":2}'` → runtime build + 42
- `cargo run -p modal-rust-cli -- deploy add` → build-time build; deployed runtime
  does not compile
- `cargo run -p modal-rust-cli -- call add --input '{"a":40,"b":2}'` → 42
- The documented normalization + the exact `diff` command (e.g.
  `diff <(normalize generated.py) <(normalize fixtures/dev_app.py)`) returning an
  EMPTY diff, for each of dev/deploy/call shims against the M1/M4/M7/M8 references.
- Transcripts of `run`/`deploy`/`call`.

## M9b - `modal-rust doctor` preflight + `panic=abort` detection

Status: in_progress (M0 dependency satisfied; doctor preflight + panic=abort detection not executed in the 2026-06-03 POC loop)

risk: low. depends_on: [M0]

Validates: `doctor` as its OWN boundary — a brand-new behavior (no earlier milestone
validates structured-error preflight or `panic=abort` detection in isolation). It
fails for exactly ONE reason: a preflight / abort-profile detection bug, never
conflated with the M9a wrapper-equivalence claim. `doctor` reuses the M0 structured
error model (hence `depends_on [M0]`); the `panic=abort` check guards the frozen
`panic` kind (§2.6 / the §2.2 5-kind taxonomy).

Acceptance:
- `modal-rust doctor` performs a creds/CLI/version preflight: detects
  `~/.modal.toml` / `MODAL_TOKEN_*`, the `modal` CLI on `$PATH`, and the pinned
  rust/python/image-builder versions; `--rust` adds `cargo`/`rustc`/`target` checks.
- A missing prerequisite produces an actionable **structured error reusing the M0
  error model** (not an ad-hoc string), demonstrated via a simulated-missing-prereq
  run.
- **`panic=abort` detection (the correctness gate for the `panic` kind):** `doctor
  --rust` detects `[profile.release] panic = "abort"` in the resolved release profile
  and warns/fails, since abort would silently degrade the `panic` envelope into a raw
  process abort (§2.6). Demonstrated against a crate configured `panic = "abort"`
  (flagged) and the default unwind profile (passes).
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  and `cargo test` are clean for the doctor code.

Evidence:
- `cargo run -p modal-rust-cli -- doctor` (healthy environment — all checks pass)
- A simulated-missing-prereq run showing the actionable structured error (M0 model).
- `cargo run -p modal-rust-cli -- doctor --rust` against a `panic = "abort"` crate
  (flagged/failed) vs the default unwind profile (passes) — the abort-detection proof.

## M0-R - Review: panic-capture robustness (follow-up)

Status: pending

risk: low. depends_on: [M0]

Raised during M0–M8 manual validation (2026-06-03); review the `panic` error kind:

- **Backtrace is empty unless `RUST_BACKTRACE=1`.** Capture is env-gated, so a local
  `modal_runner --entrypoint will_panic` (without the env var) yields
  `"backtrace":""`. The shims set `RUST_BACKTRACE=1` remotely so dev/deploy runs
  populate it, but consider `std::backtrace::Backtrace::force_capture()` (always
  captures, ignores the env var) for a consistent `panic` envelope in every context.
- **`std::env::set_var` is `unsafe` on edition 2024.** The M0 test sets
  `RUST_BACKTRACE` via `std::env::set_var` (fine on edition 2021; rust-analyzer flags
  E0133 under 2024). `force_capture()` would remove the need to mutate the env at all.
  Revisit before any edition-2024 migration.
- **Decode precedes the handler (correct — document it).** `will_panic` with `{}`
  returns `decode_error` (input is decoded before the handler runs); valid input is
  required to reach the `panic` path. Correct precedence; note it so it isn't
  mistaken for a panic-capture failure.

Acceptance: a recorded decision (force_capture vs env-gated), the edition-2024
`set_var` risk resolved or tracked, and a test that exercises the `panic` envelope
without a process-global env mutation.

Evidence: `crates/modal-rust-runtime/src/lib.rs` (panic hook + `catch_unwind`);
`workpads/prototype/knowledge.md` POC validation notes.
