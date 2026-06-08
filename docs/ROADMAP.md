# modal-rust — Roadmap & Tech Debt

Living backlog. Source for the ergonomics/docs items: `docs/local/ergonomics-and-docs-review.md`
(prioritized table + 7 sub-30-min quick wins). Update this file as items land.

---

## Active: newcomer ergonomics + real README examples (the P0s)

Being addressed by the ergonomics-hardening workflow. Verify each claimed bug against the
code before fixing.

1. **`modal_runner!()` macro.** Every crate hand-writes `src/bin/modal_runner.rs` + a
   `[[bin]]` stanza, and the macro-path version reaches into `__private::runtime`, which
   `crates/modal-rust/src/lib.rs:81` explicitly marks "NOT a stable public API." Ship a
   one-liner macro so users write zero boilerplate and never touch `__private`.
2. **`.remote()` package auto-detection.** `remote.rs` hardcodes `package = "example-add"`
   in `RemoteConfig::default()` unless `MODAL_RUST_PACKAGE` is set. The CLI auto-detects via
   `--project`, but the documented library path `App::connect(...).remote()` would try to
   build `example-add` for anyone's crate — a footgun. Likely fixable by the macro capturing
   `env!("CARGO_PKG_NAME")` in the *user's* crate and threading it into the config.
3. **README install/authoring contradicts every example.** README shows `modal-rust` +
   `use modal_rust::function;`, but all four macro examples need a `package = "modal-rust"`
   rename + `extern crate modal_rust_facade as modal_rust;`. Resolve the `modal_rust` import
   collision so the clean form in the README actually compiles in a real crate.
4. **README headline snippet won't compile.** `app.add(2, 3)` needs the generated `AddCall`
   trait in scope; the snippet omits it. Make the typed method auto-available (a prelude or
   glob) so no extra import is needed.
5. **No getting-started/tutorial.** `architecture.html` is maintainer-depth; the Modal-token
   prerequisite is understated. Need a zero→local→remote→deploy walkthrough + concepts page +
   Python→Rust cheat sheet + troubleshooting.
6. **README examples must be REAL, compiled, tested crates** (user requirement). The simple
   README snippets are liked precisely because they're clean — so make them their own crates
   that read exactly as a real user would write them, build + test them in the workspace, and
   keep the README snippets extracted-from / guarded-against the real crates so they can't
   drift. The cleaner the better. (This depends on #1–#4 making the user code actually minimal.)

**Refuted (confirmed fine):** the `App::local()/connect()` rename, the `Error` enum, and the
deploy/call-vs-connect naming — one edge: a deployed `.call()` loses the typed `app.add()` sugar.

---

## Next steps (features)

### Cheap parity (the mock makes these testable offline now)
- DONE: `cpu`/`memory` (`FunctionResources`), `retries` (int + `Retries(..)` struct),
  image builders (`pip_install`/`apt_install`/`run_commands`) + a per-function custom
  `image = Image(base/install_rust/apt/pip/run)` decorator field, `schedule` (Cron/Period),
  inline `env={..}` secrets-from-dict + `required_keys`, autoscaling, `starmap`. Each
  shipped with mock tests.
- Still cheap & open: `Secret.from_dotenv()` / `.env` file parsing; `cpu`/`memory` as a
  `(request, limit)` tuple.

### Big parity (need real design)
- ~~`Cls` (stateful classes: load-once `@enter` + `@method`)~~ — DONE (v0, Shape A):
  `#[modal_rust::cls]` on an `impl` block with `#[enter]` (load-once `OnceLock` +
  `modal_runner --serve`) and per-method dotted `"<Class>.<method>"` entrypoints with
  merged class/method config; live-confirmed on a T4. `examples/stateful-class`.
  Deferred to Shape B: `#[exit]` (marker reserved, emits `compile_error`) +
  `modal.parameter` class params (use `#[cls(secrets=[..])]` + `std::env` for now).
- Web endpoints (`@fastapi_endpoint`/`@asgi_app`) — now the largest remaining gap —
  then `Dict`/`Queue`/`Sandbox`/NFS.
- ~~**`enable_memory_snapshot` / Cls memory-checkpointing** (high value, `snapshot.py`)~~
  — DONE (v0, CPU-only, `#[cls]`-only): `#[cls(enable_memory_snapshot = true)]` makes the
  expensive `#[enter]` load run **once ever** on a *deployed* app — Modal snapshots the
  loaded process and restores it on every (even cold) container start, extending
  load-once-serve-many across the cold-start path. DEPLOY-ONLY (the flag rides into
  `Function.checkpointing_enabled`/`is_checkpointing_function` only at the deploy boundary;
  RUN stays wire-identical), built on a typed `prime` lifecycle frame on the `--serve` loop
  (degrades to lazy `#[enter]` if the prime fails). `examples/snapshot-class`. Named
  follow-ups:
  - **GPU snap/restore split** — a CPU snapshot blocks GPU access in the snap window, so a
    GPU `#[cls]` must load on CPU inside the snapshot window and move to the GPU *after*
    restore. This lands as the documented-but-not-built `restore` lifecycle frame + a
    `#[restore]` (post-restore) macro hook — additive on top of v0's typed frames.
  - **`enable_gpu_snapshot`** — Modal's GPU-memory snapshot variant, gated on the split above.
  - **`#[function(enable_memory_snapshot)]`** — `#[function]`-level snapshot support (v0 is
    `#[cls]`-only; the macro currently `compile_error`s the function form).

### Infra / quality
- **Benchmarks runnable** — wire the plan-only A/B-vs-Python harness (cold/warm build, deploy,
  invoke latency, `.map` at N, spawn, GPU cold-start). Currently `workpads/benchmarks` is plan-only.
- **Publish prep** — if crates.io is ever a goal: version the path deps, lock the public API.

---

## Tech debt

### Real (worth fixing)
- The two `DEFAULT_DEPLOY_APP` constants hold **different strings** (`modal-rust-add-poc` in the
  CLI vs `modal-rust-add-deploy` in the facade) — flagged in the architecture review, never
  reconciled.
- Runner-bin boilerplate (→ Active #1).
- Large files: `runtime/lib.rs` ~1113, `remote.rs` ~980, `image.rs` ~935;
  the RUN wrapper has been extracted to `remote/wrapper.py`, while the smaller
  `DEPLOY_WRAPPER_SRC` is still an inline Python heredoc embedded in a Rust string.
- The testkit duplicates the 4129-line proto + a 201-RPC server (its own `build_server`) —
  acceptable for a dev crate, but heavy.

### Cosmetic / mild
- Workpad + workflow sprawl: ~10 `*-spec.md` files and ~8 one-shot `.claude/workflows/*.js` now
  committed — process history, but clutter. Could prune the one-shot ones.
- The `testkit`-feature `connect_at*` seam is a test-only hook living in shipped `app.rs`
  (gated + documented, but still shipped surface).
- `add-macro/src/proof.rs` is a small "kitchen-sink" module (the relocated config/secrets demos).
- Local-only working notes (testing-strategy, ergonomics review, architecture-issues, data-user-flows,
  examples-catalogue) live in the gitignored `docs/local/` — intentionally not committed.
