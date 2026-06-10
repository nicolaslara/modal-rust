# modal-rust — Roadmap & Tech Debt

Living backlog. Source for the ergonomics/docs items: `docs/local/ergonomics-and-docs-review.md`
(prioritized table + 7 sub-30-min quick wins). Update this file as items land.

---

## DONE: newcomer ergonomics + real README examples (the P0s)

All six landed via the ergonomics-hardening + runner-bin-removal workflows (the CLI
generates the runner for bin-less crates; README snippets are extracted-from real,
tested crates; the `AddCall` import is documented in the snippet itself). Kept for
history — details below describe the PRE-fix state.

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
- ~~Web endpoints (`@fastapi_endpoint`)~~ — DONE (v0, `#[endpoint]`, FUNCTION type):
  `#[modal_rust::endpoint(method = "POST", <any #[function] config>)]` exposes a plain
  `#[function]`-shaped handler over HTTP on a **deployed** app — `webhook_config{type:
  FUNCTION, method, requires_proxy_auth}` + the ASGI data formats ride the DEPLOY
  `FunctionCreate` only (RUN stays wire-identical; the URL is deploy-only, D5). The
  deploy image auto-adds `fastapi[standard]`; the baked wrapper gains a per-endpoint
  `(request: Request)` adapter reusing the SAME `--serve` child (so `#[cls]`
  load-once + memory snapshot compose with endpoints). Public by default,
  `requires_proxy_auth = true` opts into Modal proxy-auth; `method` is required and
  validated at compile time. DUAL surface: the fn keeps its typed
  `.local()`/`.remote()` path alongside the URL. `examples/web-endpoint`. Named
  follow-ups:
  - **`#[web_server]`** — the full-app shape (a Rust `Router`/real HTTP server behind
    `WEBHOOK_TYPE_WEB_SERVER`): routing, multiple methods, streaming, websockets.
    Reuses v0's `webhook_config` plumbing; needs the new `--web` runtime mode.
    (`#[asgi_app]` stays reserved-only — a Rust fn cannot return a Python ASGI app.)
  - **Custom domains / `requested_suffix` / `web_endpoint_docs`** — the remaining
    `WebhookConfig` knobs (v0 takes Modal's default URL label, no OpenAPI docs page).
  - **`#[endpoint]` on a `#[cls]` method** (stateful endpoints) — currently a
    compile error.
  - **Ephemeral dev URL** — the `modal serve`-style URL on the RUN boundary (v0 is
    deploy-only).
  - **Surface the assigned `web_url`** — print it from `modal-rust deploy` / carry it
    on `DeployedApp` (the SDK already returns it on the create:
    `CreatedFunction.web_url`).
- Next big subsystems: `Dict`/`Queue`/`Sandbox`/NFS.
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
- ~~The two `DEFAULT_DEPLOY_APP` constants hold different strings~~ — FIXED: the CLI
  re-exports the facade's single constant.
- ~~Runner-bin boilerplate~~ — FIXED: the CLI generates the runner for bin-less crates
  (runner-bin-removal).
- Large files: the M1 mechanical splits landed — sdk `ops/function/{parse,spec,rpc}.rs`,
  `ops/image/{render,build}.rs`, macros `src/{args,cls,emit,specs}.rs` (all public paths
  preserved via re-exports). Remaining: `runtime/lib.rs` ~1620, `control_plane.rs` ~1420
  (deliberately unsplit — one cohesive provision pipeline, pending review items M12/M13),
  `remote.rs` ~1110. Both wrappers are real `.py` files (`remote/wrapper.py`,
  `deploy/wrapper.py` via `include_str!`).
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
