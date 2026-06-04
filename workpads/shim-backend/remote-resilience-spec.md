# Remote `.remote()` Resilience + Upload-Ignore Spec

Build-ready spec for the two MUST-DO fixes that make the live
`app.function("add").remote(AddInput{a:40,b:2}) -> {sum:42}` round-trip converge,
plus one OPTIONAL (skippable) image-shrink. Status: design only — no code changed.

Primary diagnosis (already verified, restated for grounding):

- **BUG 1 — the source upload includes `references/`.** `ensure_function`
  (`crates/modal-rust/src/remote.rs:217`) uploads `config.local_root` (the
  workspace root) with the default ignore list
  (`crates/modal-rust/src/remote.rs:154-159`):
  `["target", ".git", ".modal-rust", "**/*.rlib"]`. That list does NOT exclude
  `references/`, which holds two full clones (`references/modal-rs` +
  `references/modal-client`) — `du -sh references` = **14 MB** of Go/JS/Python
  source. Every `.remote()` therefore uploads ~14 MB of junk: a slow, reset-prone
  upload. (`workpads/` is 736 KB, `.claude` 144 KB — minor but also non-source.)
- **BUG 2 — no transient-retry on the control-plane unary RPCs.**
  `ensure_function` issues ~7 unary RPCs (plus per-file upload RPCs) with bare
  `.await?`. `Error::is_transient` (`crates/modal-rust-sdk/src/error.rs:63`)
  exists but is used ONLY in the image build-poll reconnect
  (`crates/modal-rust-sdk/src/ops/image.rs:270`). A single transport reset on any
  other RPC fails the whole `.remote()`, and the test's outer 4× retry restarts
  the WHOLE sequence (re-upload + rebuild) so it never converges.

---

## Part A — `retry_transient` helper (BUG 2, PRIMARY)

### A.1 Where it goes

New file: **`crates/modal-rust-sdk/src/retry.rs`**, declared `pub(crate) mod retry;`
in `crates/modal-rust-sdk/src/lib.rs`. It is an SDK-internal helper applied at
each ops call site (the wrapper is centralized; the *application* is per-RPC,
because each RPC closure must rebuild its own owned request — see A.4). This keeps
`crates/modal-rust/src/remote.rs` untouched re: retry: the resilience lives one
layer down in the SDK, exactly like Modal's Python client wraps every unary RPC in
`retry_transient_errors` inside the gRPC layer (`grpc_utils.py`), not in app code.

### A.2 The transient predicate — reuse + extend `Error::is_transient`

Canonical Python set (`references/modal-client/py/modal/_utils/grpc_utils.py:88-94`,
`RETRYABLE_GRPC_STATUS_CODES`):

```
Status.DEADLINE_EXCEEDED
Status.UNAVAILABLE
Status.CANCELLED
Status.INTERNAL
Status.UNKNOWN
```

Plus the transport-layer exceptions Python also catches and retries
(`grpc_utils.py:411-417`): `StreamTerminatedError`, `OSError`,
`asyncio.TimeoutError` (and the grpclib `AttributeError`/`_write_appdata`
work-around — N/A for tonic). The modal-rs precedent
(`references/modal-rs/crates/modal-rs/src/sandbox_filesystem.rs:399`
`is_retryable_status`) uses the SAME five gRPC codes:
`Unavailable | DeadlineExceeded | ResourceExhausted | Internal | Unknown | Cancelled`.

Our current `Error::is_transient` (`error.rs:63-90`) currently treats as transient:

- `Error::Transport(_)` → always (TLS/channel establishment).
- `Error::Status(s)` with code `Unavailable | DeadlineExceeded | ResourceExhausted`.
- `Error::Status(s)` whose message text matches a reset substring
  (`connection reset`, `error reading a body`, `h2 protocol error`,
  `broken pipe`, `transport error`, `socket connection closed`, `goaway`,
  `connection closed`).

**REQUIRED EXTENSION (error.rs:67-87).** Add the two gRPC codes Python/modal-rs
retry that we currently DROP: **`Code::Internal`** and **`Code::Unknown`**. The
live failures arrive as `hyper ConnectionReset` / `h2 protocol error` — tonic
surfaces a mid-stream h2 reset as `Status { code: Unknown | Internal, message:
"... h2 protocol error ..." }`. The substring sniff already catches the common
text, but adding the codes makes it robust to message wording the substring list
misses. Resulting `matches!`:

```rust
matches!(
    s.code(),
    Code::Unavailable
        | Code::DeadlineExceeded
        | Code::ResourceExhausted
        | Code::Internal     // NEW — h2/transport resets land here (matches Python + modal-rs)
        | Code::Unknown      // NEW — ditto
)
```

Keep the substring sniff as a belt-and-suspenders fallback (it stays correct and
helps when a reset is reported as some other code with recognizable text).

**MUST NOT retry (terminal — surface immediately).** Per WORKING.md:
`Error::Build` (in-band remote build/function failure), `Error::Config`,
`Error::Invalid`, `Error::Codec`, `Error::Runner` (the latter lives in the
`modal-rust` crate, not the SDK), and `Error::Status` with a definite code:
`Unauthenticated`, `PermissionDenied`, `InvalidArgument`, `NotFound`,
`AlreadyExists`, `FailedPrecondition`, `OutOfRange`, `Unimplemented`. These are
NOT transient under `is_transient` and the helper MUST propagate them on the FIRST
occurrence — never masked, never retried into a timeout. (`is_transient` already
returns `false` for all of these because its `_ => false` arm covers every
non-`Transport`/non-`Status` variant and the `Status` arm only returns `true` for
the listed codes/substrings.)

> Note on `ResourceExhausted` / server throttle: Python honors a server-sent
> `RPCRetryPolicy.retry_after_secs` (`grpc_utils.py:get_server_retry_policy`).
> That is an OPTIONAL refinement; v0 may treat `ResourceExhausted` as a normal
> transient with our own backoff (we already classify it transient). Do NOT block
> on parsing `RPCRetryPolicy` details.

### A.3 Backoff + jitter + caps

Mirror Python's `Retry` defaults (`grpc_utils.py:299-309`) and the modal-rs
backoff shape, tuned for the run path's longer per-RPC work (per-file uploads,
`ImageGetOrCreate` initial call):

| Param | Value | Source / rationale |
|---|---|---|
| `base_delay` | `100ms` | Python `base_delay=0.1` (`grpc_utils.py:301`); modal-rs `0.5s` |
| `delay_factor` | `2.0` | Python `delay_factor=2` (`grpc_utils.py:303`) |
| `max_delay` | `5s` | Python uses 1s default but 5s for `connect_channel` (`grpc_utils.py:222`); 5s is safe for control-plane |
| `max_attempts` | `8` | Total tries (1 initial + 7 retries). Python default `max_retries=3`; we use more because a fresh build-window reset is common. Σ delay ≈ 0.1+0.2+0.4+0.8+1.6+3.2+5 ≈ 11.3s of backoff |
| `total_deadline` | `120s` | Hard wall-clock cap per RPC. A single unary control-plane RPC must not hang the process; surface a transient error after this. |
| jitter | full jitter | `sleep = rand(0, computed_delay)` before each retry. Python uses fixed delays; we ADD jitter to avoid thundering-herd on the per-file upload loop (many files retrying in lockstep). Use `fastrand` (already a transitive dep) or a tiny LCG — do NOT pull a new crate if avoidable. |

Stop conditions (whichever first): `attempt == max_attempts` OR
`elapsed + next_delay >= total_deadline`. On stop, return the LAST error
unchanged (preserve `Error::Status`/`Error::Transport` for the caller's
diagnostics). Emit one `eprintln!("[retry] {rpc_name} attempt N/8 after transient: {err}")`
per retry (mirrors the existing image-poll `eprintln!` at `image.rs:271`); do not
add a logging crate.

### A.4 Signature

The helper wraps an async closure that PRODUCES a fresh future each attempt
(tonic requests are consumed by value, so the closure must rebuild its owned
request every call — same reason Python passes `req` + `fn` separately rather than
a pre-bound future):

```rust
// crates/modal-rust-sdk/src/retry.rs
use std::future::Future;
use std::time::{Duration, Instant};
use crate::error::{Error, Result};

/// Tunables for `retry_transient`. `RetryPolicy::default()` is the control-plane
/// unary default (8 attempts, 100ms→5s exp backoff + full jitter, 120s deadline).
#[derive(Debug, Clone, Copy)]
pub(crate) struct RetryPolicy {
    pub base_delay: Duration,   // 100ms
    pub max_delay: Duration,    // 5s
    pub delay_factor: f64,      // 2.0
    pub max_attempts: u32,      // 8
    pub total_deadline: Duration, // 120s
}

impl Default for RetryPolicy { /* values from A.3 */ }

/// Retry `op` while it fails with a TRANSIENT error (`Error::is_transient`).
/// Non-transient errors propagate on the first occurrence (auth, invalid arg,
/// in-band build/function failure). `name` is for log lines only.
///
/// `op` is an `FnMut` returning a fresh `Future` each call so the wrapped RPC can
/// rebuild its owned tonic request per attempt.
pub(crate) async fn retry_transient<T, F, Fut>(
    name: &str,
    policy: RetryPolicy,
    mut op: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let start = Instant::now();
    let mut delay = policy.base_delay;
    let mut attempt = 1u32;
    loop {
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                let last = attempt >= policy.max_attempts;
                let over_deadline =
                    start.elapsed() + delay >= policy.total_deadline;
                if !e.is_transient() || last || over_deadline {
                    return Err(e);
                }
                eprintln!("[retry] {name} attempt {attempt}/{} after transient: {e}",
                          policy.max_attempts);
                let jittered = jitter(delay);            // full jitter: rand(0, delay)
                tokio::time::sleep(jittered).await;
                delay = (delay.mul_f64(policy.delay_factor)).min(policy.max_delay);
                attempt += 1;
            }
        }
    }
}
```

A convenience alias for the common case keeps call sites terse:

```rust
pub(crate) async fn retry_unary<T, F, Fut>(name: &str, op: F) -> Result<T>
where F: FnMut() -> Fut, Fut: Future<Output = Result<T>> {
    retry_transient(name, RetryPolicy::default(), op).await
}
```

**Borrow note (important).** Every wrapped RPC body does
`self.inner_mut().<rpc>(req.clone()).await?.into_inner()`. Because the closure
borrows `&mut self.inner` mutably AND must run multiple times, write the closure
to borrow the stub fresh each call and `.clone()` the request inside it:

```rust
let req = MountGetOrCreateRequest { /* ... */ };
let resp = retry_unary("mount_get_or_create", || {
    let req = req.clone();
    async { Ok(self.inner_mut().mount_get_or_create(req).await?.into_inner()) }
}).await?;
```

The generated request protos all derive `Clone` (prost), and `&mut self` is
re-borrowed by the closure on each call — this compiles under NLL because the
closure is `FnMut` and each invocation's borrow ends at `.await`. If a specific
call site fights the borrow checker, factor the RPC into a small
`async fn raw_<name>(&mut self, req: Req) -> Result<Resp>` and retry THAT:
`retry_unary("name", || self.raw_name(req.clone()))`.

### A.5 Where to apply it — every control-plane unary RPC + per-file upload

Apply `retry_unary` (default policy) by wrapping the existing `.await?` call. The
EXISTING image join-poll reconnect (`image.rs:252-277`) STAYS as-is — it is a
stream, not a unary RPC, and already reconnects on transient.

| RPC | File:line (current bare `.await?`) | Idempotency note |
|---|---|---|
| `client_hello` | `client.rs:98` | Read-only handshake; safe to repeat. (Optional — connect-time; a reset here just fails connect.) |
| `app_get_or_create` | `client.rs:119`, `app.rs:48` | `CREATE_IF_MISSING` — idempotent by name+env (returns same `app_id`). |
| `app_create` (ephemeral) | `app.rs:71` | Not used by run path; if wrapped, a dropped response could create a duplicate ephemeral app. SKIP or accept (ephemerals are GC'd). Run path uses get-or-create. |
| `mount_get_or_create` (client mount, GLOBAL) | `mount.rs:54` | Pure lookup (`UNSPECIFIED` creation) — fully idempotent. |
| `mount_get_or_create` (source mount, EPHEMERAL) | `local_dir.rs:85` | Keyed by the `files` set (sha-addressed); re-sending yields the same mount. Safe. |
| `mount_put_file` (probe, no data) | `local_dir.rs:180` (`mount_put_file_probe`) | Pure existence read — idempotent. |
| `mount_put_file` (upload, with data) | `local_dir.rs:156` | Server dedups by `sha256_hex`; re-PUT of same bytes is a no-op. Safe. |
| `blob_create` | `blob.rs:41` | Returns a presigned URL for the sha; re-requesting is safe (new URL, same content addressing). |
| blob `PUT` (HTTP, not gRPC) | `blob.rs:69` (`put_blob_bytes`) | Idempotent object-store PUT (same key/bytes). Wrap with a SEPARATE small retry that treats reqwest transport errors + 5xx/429 as transient (see A.6). |
| `image_get_or_create` (INITIAL call) | `image.rs:207` | Modal dedups images by content hash; re-issuing returns the same `image_id`/build. Safe. (The build POLL stays on its own reconnect loop.) |
| `function_precreate` | `function.rs:139` | Re-precreate of same `app_id`+`function_name` returns a usable id; downstream `function_create` reconciles via `existing_function_id`. Safe. |
| `function_create` (FILE mode) | `function.rs:188` | Sent with `existing_function_id = precreate_id` + a fixed definition; re-sending the same definition after a dropped response is idempotent (server reconciles by precreate id). Safe — mirrors Python which retries `FunctionCreate`. |
| `app_publish` | `app.rs:100` | Publishes `DEPLOYED` state with the same `function_ids`/`definition_ids`; re-publish is idempotent (deploy is a set-state, not append). Safe. |
| `function_get` / `from_name` | `function.rs:235` | Pure read. Idempotent. |
| `function_map` | `invoke.rs:112` | **CARE:** `FunctionMap` enqueues an input. A retry could double-enqueue → two executions. Use an idempotency guard: pass a stable `idempotency_key` if the proto supports it; ELSE retry `function_map` ONLY on errors raised BEFORE the call is established (transport reset with no `function_call_id` returned) and treat a post-enqueue reset as "poll with the id we have". For v0, the simplest safe choice is: retry `function_map` on transient (a duplicate `add` is harmless — same `{sum:42}` — and we only read ONE output). Document this; for non-idempotent user functions a future milestone adds the input idempotency token. |
| `function_put_inputs` | `invoke.rs:133` | Same double-enqueue caveat as `function_map`; same v0 stance (retry; harmless for the pure `add`). Carries `idx`/`function_call_id` so the server can dedup within a call. |
| `function_get_outputs` (POLL) | `invoke.rs:167` | Pure read with `last_entry_id` cursor; already in a poll loop. Wrap the single-window `.await?` so a transient reset retries the window instead of failing the whole invoke. (Analogous to the image poll reconnect.) |

**Invoke double-enqueue stance (explicit decision).** For v0 the run path only
invokes the pure `add` (idempotent), and `poll_outputs` reads exactly one output
then returns, so a duplicate enqueue is observably harmless. Therefore wrap
`function_map` and `function_put_inputs` with `retry_unary` like the others.
Record the caveat in code comments: a non-idempotent user function would need a
server idempotency token before enabling map/put retry generally. This matches
Python, which DOES retry these RPCs under `retry_transient_errors` and relies on
server-side input dedup.

### A.6 Blob PUT (HTTP, not gRPC)

`put_blob_bytes` (`blob.rs:67-83`) is plain reqwest. `Error::is_transient` only
classifies gRPC/tonic errors, so add a tiny inline retry in `put_blob_bytes` (or a
`http_is_transient(&reqwest::Error, status)` helper) that retries when:
`err.is_timeout() || err.is_connect()` OR the response status is `5xx` or `429`.
Reuse the same `RetryPolicy` shape (8 attempts, exp backoff + jitter). Non-2xx
that is `4xx` (except 429) is terminal — surface immediately. The run path's
source files are tiny (all inline `MountPutFile`), so this branch is rarely hit,
but wrapping it closes the last reset-prone hole.

### A.7 Tests (offline, no live)

- `retry.rs` unit tests with a closure backed by an `AtomicUsize`:
  (1) transient-then-ok returns `Ok` after N attempts; (2) a non-transient
  `Error::Build`/`Error::Status(InvalidArgument)` returns IMMEDIATELY (attempt
  count == 1 — proves real errors are never retried/masked); (3) all-transient
  exhausts `max_attempts` and returns the last error; (4) deadline cap stops
  early. Use `tokio::time::pause()`/`advance` so tests are instant.
- Extend `error.rs` tests: assert `Code::Internal` and `Code::Unknown` statuses
  are now `is_transient()`, and that `Unauthenticated`/`InvalidArgument`/`NotFound`
  remain non-transient.
- No change to existing ops tests (behavior identical on the happy path —
  `retry_unary` returns `Ok` on the first try).

---

## Part B — Upload IGNORE fix (BUG 1, PRIMARY)

### B.1 The corrected default ignore list

In `RemoteConfig::default()` (`crates/modal-rust/src/remote.rs:154-159`) replace:

```rust
ignore: vec![
    "target".to_string(),
    ".git".to_string(),
    ".modal-rust".to_string(),
    "**/*.rlib".to_string(),
],
```

with (additions in **bold** intent — `references` is the load-bearing fix; the
rest are belt-and-suspenders non-source dirs that should never reach the
container):

```rust
ignore: vec![
    "target".to_string(),       // build artifacts (already pruned early)
    ".git".to_string(),         // VCS (2.4 MB)
    ".modal-rust".to_string(),  // generated scratch / shims
    "references".to_string(),   // FIX: 14 MB of vendored modal-rs + modal-client clones (gitignored)
    "workpads".to_string(),     // planning docs (736 KB) — not build input
    ".github".to_string(),      // CI config — not build input
    ".claude".to_string(),      // agent config — not build input
    ".cursor".to_string(),      // editor config
    ".opencode".to_string(),    // agent config
    "tmp".to_string(),          // matches .gitignore scratch
    ".research".to_string(),    // matches .gitignore scratch
    "**/*.rlib".to_string(),    // stray rust libs
],
```

**Matcher confirmation.** `IgnoreMatcher` (`local_dir.rs:290-343`) treats a bare
pattern (no `*.`/`**/*.` prefix) as a **segment** matched against ANY path
component, and PRUNES the directory early in the `WalkDir::filter_entry`
(`local_dir.rs:222-231`) so we never descend into it. So `"references"` prunes
`references/` and everything under it on first contact — exactly the cheap
behavior we need for a 14 MB tree. The existing tests
(`ignore_matcher_prunes_bare_segments`) already prove bare-segment pruning works;
add a case asserting `references/modal-rs/Cargo.toml` is ignored.

> Minimal-change alternative: if you want the smallest diff, adding ONLY
> `"references"` fixes the verified bug (the 14 MB clone). The extra entries
> (`workpads`, `.github`, `.claude`, `.cursor`, `.opencode`, `tmp`, `.research`)
> are cheap insurance and keep the upload to genuine workspace source. Both are
> acceptable; prefer the fuller list — it is strictly more correct and the cost is
> one `Vec<String>` literal.

### B.2 Confirm the KEPT set still builds on Modal

`cargo build -p example-add --bin modal_runner` (run in the FILE-mode container
against the mounted `/src`) needs every `[workspace].members` path to EXIST
(cargo refuses to load a workspace if a member dir is missing) plus the actual
sources. The kept set after the ignore fix:

- `Cargo.toml` (workspace manifest — KEPT, top-level file, not ignored).
- `Cargo.lock` (KEPT — top-level file; pins the dependency graph so the in-body
  build is reproducible). **Verify it is present** (it is: 145 KB at root).
- `crates/*` — ALL five members KEPT: `modal-rust-sdk`, `modal-rust`,
  `modal-rust-runtime`, `modal-rust-cli`, `modal-rust-macros`.
- `examples/*` — ALL four members KEPT: `add` (== `example-add`, the build
  target), `add-macro`, `cuda-vector-add`, `burn-add`. **`burn-add` MUST be
  uploaded** even though it is excluded from `default-members`: it is still a
  `[workspace].members` entry, so its `Cargo.toml` + `src/` must exist on disk or
  the workspace fails to load. The ignore list does NOT touch `examples/`, so it
  is kept. (We build only `-p example-add`, so `burn-add`'s CUDA deps are never
  compiled — only its manifest is read during workspace resolution.)

None of the new ignore entries (`references`, `workpads`, `.github`, `.claude`,
`.cursor`, `.opencode`, `tmp`, `.research`) overlaps `Cargo.toml`, `Cargo.lock`,
`crates/`, or `examples/`. So the corrected upload still contains the complete,
self-consistent workspace and `cargo build -p example-add --bin modal_runner`
resolves and builds exactly as before — just without 14 MB of dead weight.

### B.3 Tests

- Extend `default_config_has_expected_shape` (`remote.rs:417`): assert the ignore
  list now contains `"references"` (and the other new segments).
- Add an `IgnoreMatcher` test (`local_dir.rs` test mod): `references` prunes
  `references/modal-rs/Cargo.toml` and `references/modal-client/py/x.py`, while
  `crates/modal-rust-sdk/src/lib.rs`, `examples/add/Cargo.toml`, `Cargo.toml`,
  `Cargo.lock` are KEPT.

---

## Part C — OPTIONAL: `add_python` to shrink the image build (SECONDARY, SKIPPABLE)

> SECONDARY. Do NOT block the primary fixes on this. If the client-deps story is
> unclear, KEEP the current `apt-get python3 + pip install --break-system-packages
> modal` image and rely on Part A's retry + Modal layer caching (once one build
> completes it caches; later runs are fast). The retry fix alone makes the live
> run converge.

### C.1 What `add_python` actually does (mechanism, verified from refs)

`modal.Image.from_registry("rust:1-slim", add_python="3.12")`
(`references/modal-client/py/modal/_image.py:2084-2155`) does NOT add a slow `RUN`
step. It:

1. Resolves a hosted GLOBAL mount by name:
   `python_standalone_mount_name("3.12")` →
   `"python-build-standalone.20240107.3.12.1-gnu"`
   (`mount.py:72-89`, table at `mount.py:45-52`). Resolution is the SAME
   `_Mount.from_name(..., namespace=DEPLOYMENT_NAMESPACE_GLOBAL)` pattern we
   already use for the client mount (`ops/mount.rs`) — a `MountGetOrCreate`
   lookup, NO build step.
2. Attaches that mount as the image's **`context_mount`** (NOT a runtime
   `Function.mount_ids` mount) and emits two Dockerfile commands
   (`_image.py:2047-2050`):
   ```
   COPY /python/. /usr/local
   ENV TERMINFO_DIRS=/etc/terminfo:/lib/terminfo:/usr/share/terminfo:/usr/lib/terminfo
   ```
   For Python < 3.13 it also inserts `RUN ln -s /usr/local/bin/python3
   /usr/local/bin/python` (`_image.py:2059`). The `COPY` pulls the standalone
   interpreter from the context mount into `/usr/local` at build time — a fast
   file copy, not an `apt-get`/`pip` network install.

So `add_python` replaces minutes of `apt-get install python3 + pip install modal`
with a `MountGetOrCreate` lookup + a `COPY` layer.

### C.2 Why this is genuinely SECONDARY (the plumbing cost)

The standalone python mount must be attached as the image's **context mount**,
which means `ImageGetOrCreateRequest` must carry the mount id in its
`Image.context_mount_id` field (see `_image.py:636` `context_mount_id=...`). Our
SDK's `image_get_or_create` (`ops/image.rs:202-214`) currently sends
`Image { dockerfile_commands, ..Default::default() }` with NO `context_mount_id`,
and `ImageSpec` (`image.rs:50-70`) has no field for it. Implementing `add_python`
therefore requires:

1. A new resolver `python_standalone_mount_id(version)` in `ops/mount.rs` (mirror
   `client_mount_id`: `MountGetOrCreate`, GLOBAL, `UNSPECIFIED`, name =
   `python-build-standalone.{release}.{full}-gnu` from a small version table
   ported from `mount.py:45-52`).
2. An `ImageSpec::with_add_python(version)` that (a) records the version, (b) on
   render prepends the `COPY /python/. /usr/local` + `ENV TERMINFO_DIRS=...`
   (+ `ln -s` for <3.13) commands, and (c) carries the resolved mount id.
3. Plumbing `context_mount_id` onto the `Image` proto inside `image_get_or_create`
   (resolve the standalone mount, set `image.context_mount_id`). Confirm the
   generated `Image` proto exposes `context_mount_id` (it does in modal-rs's
   `api.proto`; verify in our generated `proto::api::Image`).

**Client-deps caveat (the unclear part).** `add_python` provides the Python
*interpreter* but, on builder versions ≥ 2024.10, the modal *client* deps
(`typing_extensions`, `grpclib`, `protobuf`, `aiohttp`, `cbor2`, …) are "mounted
at runtime" (`_image.py:2064-2065` comment) — i.e. supplied by the hosted client
mount we ALREADY attach. The live finding in `ops/image.rs:11-29` is that a bare
base + client mount still crash-loops on `ModuleNotFoundError: typing_extensions`,
which is why we currently `pip install modal`. It is UNCLEAR whether `add_python`'s
standalone interpreter + our client mount alone satisfies the client dep closure
on the current builder version, OR whether a `pip install` (or `uv pip install`)
of the client deps is still required. **Resolution path if attempted:** build the
`add_python` image WITHOUT pip, attempt one live invoke, and check the container
logs for `ModuleNotFoundError`. If clean → drop the apt+pip lines entirely
(big win). If it still fails on a client dep → keep a single fast
`pip install --break-system-packages modal` (still much faster than apt+pip on a
bare base, since the python interpreter is now COPY'd not apt-installed) OR abandon
add_python and keep the current image. Either way the retry fix (Part A) is what
makes the run converge; add_python only narrows the build's reset window.

### C.3 Recommendation

Implement Parts A + B first and prove the live run green. Attempt Part C ONLY as a
follow-up, gated on the C.2 live check. If the deps story stays unclear, ship A+B
and leave the apt+pip image — Modal layer caching makes the second build onward
fast, and the retry fix rides out any reset during the first (cold) build.

---

## Citations (quick index)

- Python transient codes: `references/modal-client/py/modal/_utils/grpc_utils.py:88-94`
  (`RETRYABLE_GRPC_STATUS_CODES`: DEADLINE_EXCEEDED, UNAVAILABLE, CANCELLED,
  INTERNAL, UNKNOWN); retried transport exceptions `grpc_utils.py:411-417`;
  `Retry` defaults `grpc_utils.py:299-309`; `connect_channel` 18-attempt/63s
  backoff `grpc_utils.py:222`.
- modal-rs retry precedent: `references/modal-rs/crates/modal-rs/src/sandbox_filesystem.rs:399`
  (`is_retryable_status` — same six codes) + `:350-377` (backoff loop);
  `queue.rs:35-36` (put backoff 100ms→30s).
- Our `is_transient`: `crates/modal-rust-sdk/src/error.rs:63-90` (EXTEND with
  `Code::Internal`/`Code::Unknown`).
- Image build-poll reconnect (the ONE existing transient handler, stays):
  `crates/modal-rust-sdk/src/ops/image.rs:252-277` (uses `is_transient` at `:270`).
- RPC call sites to wrap: `client.rs:98,119`; `app.rs:48,71,100`;
  `mount.rs:54`; `local_dir.rs:85,156,180`; `blob.rs:41` (+ HTTP `:69`);
  `image.rs:207` (initial); `function.rs:139,188,235`; `invoke.rs:112,133,167`.
- Ignore fix site: `crates/modal-rust/src/remote.rs:154-159`
  (`RemoteConfig::default().ignore`); upload call `remote.rs:217`;
  matcher `crates/modal-rust-sdk/src/ops/local_dir.rs:290-343`, early prune
  `:222-231`.
- Workspace members (kept set): `Cargo.toml [workspace].members` (5 crates + 4
  examples; `example-burn-add` is a member but not a default-member — still must
  be uploaded so the workspace loads).
- `add_python` mechanism: `references/modal-client/py/modal/_image.py:2036-2081`
  (`_registry_setup_commands`: `COPY /python/. /usr/local`, `ENV TERMINFO_DIRS`,
  `ln -s` for <3.13) and `:2084-2155` (`from_registry` context_mount_function);
  standalone mount name + version table `references/modal-client/py/modal/mount.py:45-52,72-89`;
  context_mount_id wiring `_image.py:636`.
