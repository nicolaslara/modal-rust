#!/usr/bin/env bash
# Validate the README examples by running each one and checking its output.
#
# Three tiers, each gated on what the host can actually do:
#
#   1. OFFLINE   — always run. In-process tests / `cargo test` / `doctor` + the
#                  named `add-runner` bin for the manual reference. No credentials,
#                  no network. Every pure-library example is exercised through the
#                  `modal-rust` CLI or its `cargo test` suite (NOT via per-example
#                  `cargo run --bin modal_runner`).
#   2. LIVE      — run automatically WHEN Modal credentials are present
#                  (`~/.modal.toml` or MODAL_TOKEN_ID + MODAL_TOKEN_SECRET). The
#                  CPU `.remote()` + deploy + call round-trips. Cheap and fast.
#   3. GPU       — run when credentials are present AND `RUN_GPU=1`. Real T4 runs
#                  (cuda-vector-add via `run`, burn-add via deploy+call). These cost
#                  a little and the first burn build is slow, so they are opt-in.
#
# Escape hatches: `SKIP_LIVE=1` forces offline-only even with credentials;
# `RUN_GPU=1` adds the GPU tier.
#
#   bash scripts/check-examples.sh           # offline (+ live if creds present)
#   RUN_GPU=1 bash scripts/check-examples.sh # also the real T4 runs
#   SKIP_LIVE=1 bash scripts/check-examples.sh
#
set -uo pipefail
cd "$(dirname "$0")/.."

pass=0
fail=0

# run "<description>" "<expected substring>" "<bash command>"
run() {
  local desc="$1" expect="$2" cmd="$3" out
  echo "── $desc"
  echo "   \$ $cmd"
  out="$(eval "$cmd" 2>/dev/null)"
  if [[ "$out" == *"$expect"* ]]; then
    echo "   ✓ $out"
    pass=$((pass + 1))
  else
    echo "   ✗ expected to contain: $expect"
    echo "     got: $out"
    fail=$((fail + 1))
  fi
  echo
}

# live "<description>" "<bash command>" "<expected substr>" ["<expected substr>" …]
# Runs a real Modal command (timeout-wrapped against transient capacity blips) and
# asserts every expected substring appears in its combined stdout+stderr.
live() {
  local desc="$1" cmd="$2"; shift 2
  echo "── $desc"
  echo "   \$ $cmd"
  local out ok=1
  out="$(timeout 1800 bash -c "$cmd" 2>&1)"
  local expect
  for expect in "$@"; do
    if [[ "$out" == *"$expect"* ]]; then
      echo "   ✓ $expect"
    else
      echo "   ✗ expected to contain: $expect"
      ok=0
    fi
  done
  if [[ "$ok" -eq 1 ]]; then
    pass=$((pass + 1))
  else
    echo "     --- output ---"
    echo "$out" | sed 's/^/     /'
    fail=$((fail + 1))
  fi
  echo
}

# Credentials present? (~/.modal.toml or the token env pair.)
has_creds() {
  [[ -f "${HOME}/.modal.toml" ]] || { [[ -n "${MODAL_TOKEN_ID:-}" ]] && [[ -n "${MODAL_TOKEN_SECRET:-}" ]]; }
}

# ───────────────────────── Build the CLI once ─────────────────────────
# All `modal-rust run/deploy/call/doctor` invocations below use this binary.
echo "── Building modal-rust-cli …"
if ! cargo build -q -p modal-rust-cli 2>/dev/null; then
  echo "FATAL: cargo build -p modal-rust-cli failed" >&2
  exit 1
fi
CLI="target/debug/modal-rust"
echo "   built: ${CLI}"
echo

# ───────────────────────── 1. OFFLINE (always) ─────────────────────────

# quickstart — pure-library example (no runner bin; the CLI generates it). Offline
# coverage: in-process tests prove add(2,3)==5; doctor --rust verifies the CLI
# resolves the project without a hand-written runner bin (the banner is always
# printed to stdout; credentials optional for the offline tier).
run "quickstart: typed_local_add_returns_5 (in-process)" 'test result: ok' \
  "cargo test -q -p quickstart -- typed_local_add_returns_5 2>&1"

run "quickstart: doctor --rust (CLI resolves pure-library project)" 'modal-rust doctor — preflight (OFFLINE)' \
  "${CLI} doctor --rust --project examples/quickstart"

# add-macro — macro path (struct I/O). Proven OFFLINE via cargo test: the
# `add_registered_via_inventory_runner_envelope` test dispatches through the frozen
# runner CLI, proving the same `{"ok":true,"value":42}` the runner would print.
run "add-macro: runner envelope (in-process via cargo test)" 'test result: ok' \
  "cargo test -q -p example-add-macro -- add_registered_via_inventory_runner_envelope 2>&1"

# custom-types — a function over YOUR OWN structs (macro infers I/O from the
# signature). Proven OFFLINE via cargo test: `score_round_trips_through_user_structs`
# drives the same in-process dispatch path.
run "custom-types: score(Player) -> Scored (in-process via cargo test)" 'test result: ok' \
  "cargo test -q -p example-custom-types 2>&1"

# add — manual / no-macro path ({sum} output). MANUAL reference: hand-built Registry,
# so it keeps a hand-written runner whose bin is `add-runner` (NOT `modal_runner`).
run "add (manual): add(40, 2)" '{"ok":true,"value":{"sum":42}}' \
  "cd examples/add && cargo run -q --bin add-runner -- --entrypoint add --input-json '{\"a\":40,\"b\":2}'"

# orchestrate — the local tour (manual + macro/inventory + auto-I/O)
run "orchestrate: local tour" 'add(2, 3) -> 5' \
  "cd examples/orchestrate && cargo run -q --bin orchestrate"

# ways-to-call — one function, four invocation shapes; OFFLINE runs the .local() one
run "ways-to-call: .local() tour" 'local:  square(6) -> 36' \
  "cargo run -q -p example-ways-to-call --bin ways_to_call"

# fan-out-map — embarrassingly-parallel scale-out; OFFLINE runs the local fan-out
run "fan-out-map: local fan-out (results in input order)" 'intro -> 8 words, 1 min' \
  "cargo run -q -p example-fan-out-map --bin fan_out_map"

# background-jobs — fire-and-forget with .spawn()+.get(timeout); OFFLINE runs the
# job in-process (the deterministic result a spawned run converges to)
run "background-jobs: local job (result a spawn converges to)" "job 'nightly-report' done -> 250000 rounds, digest 17267777379177717202" \
  "cargo run -q -p example-background-jobs --bin background_jobs"

# spawn-map-foreach — the rest of the map family: side-effect maps (.for_each, waits
# + discards) and fire-and-forget fan-out (.spawn_map). OFFLINE runs the local mirror
# of .for_each (run every recipient's side effect in-process, discard the receipts).
run "spawn-map-foreach: local for_each mirror (side effects, results discarded)" 'for_each (local mirror): notified 3 recipients, results discarded' \
  "cargo run -q -p example-spawn-map-foreach --bin spawn_map_foreach"

# error-handling — how a failure crosses the boundary. The driver runs BOTH failing
# functions offline and prints the wire envelope each produces (the structured one
# carries machine-readable `details` the caller branches on).
run "error-handling: structured error -> details + branch" 'branch:     short by 50 cents -> prompt a top-up' \
  "cargo run -q -p example-error-handling --bin error_handling"

# error-handling — the plain anyhow path lands on `function_error` with `details:null`.
# Proven OFFLINE via cargo test: `anyhow_error_is_opaque_with_null_details` dispatches
# through the in-process facade and asserts `details == None` on the RunnerError.
run "error-handling: anyhow error -> function_error, details:null (in-process via cargo test)" 'test result: ok' \
  "cargo test -q -p example-error-handling -- anyhow_error_is_opaque_with_null_details 2>&1"

# secrets — decorator-is-config: a named secret rides through inventory. Proven OFFLINE
# via cargo test (tests/manifest.rs): `named_secret_rides_into_function_create` uses
# App::dry_run to assert the secret id rides into the FunctionCreate manifest.
run "secrets: named secret rides through inventory (cargo test)" 'test result: ok' \
  "cargo test -q -p example-secrets 2>&1"

# volumes — decorator-is-config: a named volume mounts at /data. Proven OFFLINE via
# cargo test (tests/manifest.rs): `mounted_volume_rides_into_function_create` uses
# App::dry_run to assert the volume mount rides into the FunctionCreate manifest.
run "volumes: mounted volume rides through inventory (cargo test)" 'test result: ok' \
  "cargo test -q -p example-volumes 2>&1"

# timeout-and-cache — decorator-is-config: timeout + cache knobs ride through
# inventory. Proven OFFLINE via cargo test (tests/manifest.rs): asserts both
# timeout_secs and the cargo-cache volume mount ride into the FunctionCreate manifest.
run "timeout-and-cache: timeout + cache ride through inventory (cargo test)" 'test result: ok' \
  "cargo test -q -p example-timeout-and-cache 2>&1"

# cpu-memory — decorator-is-config: right-size compute by requesting CPU cores + RAM.
# Proven OFFLINE via cargo test (tests/manifest.rs): asserts milli_cpu + memory_mb
# ride into the FunctionCreate manifest's resources.
run "cpu-memory: cpu + memory ride through inventory (cargo test)" 'test result: ok' \
  "cargo test -q -p example-cpu-memory 2>&1"

# retries — decorator-is-config: a retry policy makes a flaky function self-heal.
# Proven OFFLINE via cargo test (tests/manifest.rs): asserts `retries == 5` rides
# into the FunctionCreate manifest's retry_policy.
run "retries: retry count rides through inventory (cargo test)" 'test result: ok' \
  "cargo test -q -p example-retries 2>&1"

# scheduled-job — decorator-is-config: a cron schedule runs the function on a cadence.
# Proven OFFLINE via cargo test (tests/manifest.rs): asserts the cron spec rides into
# the FunctionCreate manifest's schedule field.
run "scheduled-job: cron schedule rides through inventory (cargo test)" 'test result: ok' \
  "cargo test -q -p example-scheduled-job 2>&1"

# autoscaling — decorator-is-config: control warm capacity + scale-to-zero with the
# autoscaler knobs (`min_containers`/`max_containers`/`buffer_containers` +
# `scaledown_window`). Proven OFFLINE via tests/manifest.rs (App::dry_run asserts all
# four knobs ride into FunctionCreate's autoscaler_settings). The crate is a pure
# library run via the modal-rust CLI; the offline doctor preflight proves the CLI finds
# the project without credentials or a runner bin.
run "autoscaling: doctor offline preflight (pure library, no runner bin)" 'modal-rust doctor — preflight (OFFLINE)' \
  "${CLI} doctor --project examples/autoscaling"

# stateful-class — load-once stateful class with `#[modal_rust::cls]`: `#[enter]` builds
# the expensive state (an embedding model) ONCE per warm container, every `#[method]`
# reuses the same singleton. Proven OFFLINE via cargo test: tests/local.rs asserts
# `#[enter]` runs exactly once across many `.local()` calls AND the embedding is real
# (right width, deterministic, unit-norm); tests/manifest.rs asserts each method rides
# into its own dotted FunctionCreate entrypoint (Embedder.embed gpu=A10G override,
# Embedder.dim gpu=T4 inherited; both timeout=600). Pure library run via the CLI; the
# offline doctor preflight proves the CLI finds the project with no creds or runner bin.
run "stateful-class: load-once #[enter] + real embedding + dotted entrypoints (cargo test)" 'test result: ok' \
  "cargo test -q -p stateful-class 2>&1"

run "stateful-class: doctor offline preflight (pure library, no runner bin)" 'modal-rust doctor — preflight (OFFLINE)' \
  "${CLI} doctor --project examples/stateful-class"

# custom-base — pick the RUN base image + install the Rust toolchain through the
# EXPOSED build-config knobs (RemoteConfig.base_image / .install_rust, or the
# MODAL_RUST_BASE_IMAGE / MODAL_RUST_INSTALL_RUST env vars — NOT decorator config).
# Proven OFFLINE: the driver dry-runs a CUDA-devel base config and prints the rendered
# image dockerfile FROM line (a mock test in tests/manifest.rs asserts the full
# dockerfile — the FROM + the rustup install RUN). No new feature, only exposed knobs.
run "custom-base: dry-run renders a CUDA-devel base FROM line" 'base:   FROM nvidia/cuda:12.6.3-devel-ubuntu22.04' \
  "cargo run -q -p example-custom-base --bin custom_base"

# pip-apt-image — add arbitrary system/Python deps with real image-builder STEPS
# (RemoteConfig.image_steps: ImageStep::apt / ::pip / ::run, mirroring Modal's
# apt_install / pip_install / run_commands — NOT decorator config). Proven OFFLINE: the
# driver dry-runs a config with the three steps and prints the rendered image dockerfile
# pip line (a mock test in tests/manifest.rs asserts the full dockerfile — apt/pip/run
# lines, in chain order, after provisioning and before the build).
run "pip-apt-image: dry-run renders a pip install image-builder step" 'pip: RUN python3 -m pip install --no-cache-dir numpy pillow' \
  "cargo run -q -p example-pip-apt-image --bin pip_apt_image"

# deploy-and-call — the run-vs-deploy build boundary (the production model). The
# offline driver dry-runs BOTH manifests and prints where each builds: .remote()
# builds IN the body (RUN image carries no cargo build); deploy bakes the binary at
# image-build time (top layer `cargo build --release`, client-mount-only
# FunctionCreate, persistent publish). A mock-backed test (tests/manifest.rs) drives
# a REAL deploy + call and asserts the deploy manifest AND that call resolves the
# function with no rebuild. Modal-requiring deploy/call commands are compile-only.
run "deploy-and-call: deploy builds once, call invokes with no rebuild" 'boundary: deploy builds ONCE at image-build, call invokes with no rebuild' \
  "cargo run -q -p example-deploy-and-call --bin deploy_and_call"

# cli-workflow — drive a crate from the generic `modal-rust` CLI, no driver binary.
# The OFFLINE-tested verb is `doctor`: the preflight that needs no Modal (it prints
# its banner to stdout regardless of credentials, then checks them). The run/deploy/
# call verbs require Modal and are documented compile/listed-only (their build is
# covered by `cargo build`). Asserts the OFFLINE preflight banner the doctor prints.
run "cli-workflow: doctor offline preflight (no driver binary)" 'modal-rust doctor — preflight (OFFLINE)' \
  "${CLI} doctor --rust --project examples/cli-workflow"

# cuda-vector-add — decorator-is-config: gpu="T4" rides through the macro. Proven
# OFFLINE via the CLI doctor preflight: the CLI resolves the project (finds the
# workspace root, confirms cargo is available). The GPU decorator config is proven via
# the live RUN_GPU tier below which runs the actual T4 kernel. (No tests/ dir in this
# crate; the gpu= decorator contract is exercised end-to-end in the GPU tier.)
run "cuda-vector-add: doctor offline preflight (gpu decorator via CLI path)" 'modal-rust doctor — preflight (OFFLINE)' \
  "${CLI} doctor --project examples/cuda-vector-add"

# own-runner-bin — the "bring-your-own runner" escape hatch. This is the SINGLE
# workspace member that ships its own `modal_runner` bin (a hand-written one-liner
# wrapping modal_runner!(own_runner_bin)). The CLI auto-detects this bin via cargo
# metadata and uses it as-is (no shadow runner is generated). Proven OFFLINE via
# cargo test: the manifest.rs tests prove the entrypoint is registered (the
# --describe / registry view) and dispatch works through the frozen runner CLI.
# This exercises the "auto-detect: use the existing bin" path end-to-end.
run "own-runner-bin: auto-detect existing modal_runner (cargo test)" 'test result: ok' \
  "cargo test -q -p own-runner-bin 2>&1"

# ───────────────────────── 2 & 3. LIVE / GPU ─────────────────────────

if [[ "${SKIP_LIVE:-}" == "1" ]]; then
  echo "── Live tiers skipped (SKIP_LIVE=1)."
  echo
elif ! has_creds; then
  echo "── Live tiers skipped — no Modal credentials (~/.modal.toml or"
  echo "   MODAL_TOKEN_ID + MODAL_TOKEN_SECRET). With credentials they run"
  echo "   automatically. The commands they would run:"
  echo "     orchestrate     RUN_REMOTE=1 cargo run -p example-orchestrate"
  echo "     ways-to-call    RUN_REMOTE=1 cargo run -p example-ways-to-call --bin ways_to_call"
  echo "     fan-out-map     RUN_REMOTE=1 cargo run -p example-fan-out-map --bin fan_out_map"
  echo "     background-jobs RUN_REMOTE=1 cargo run -p example-background-jobs --bin background_jobs"
  echo "     spawn-map-foreach RUN_REMOTE=1 cargo run -p example-spawn-map-foreach --bin spawn_map_foreach"
  echo "     own-runner-bin  ${CLI} run extract_metrics --project examples/own-runner-bin --input '{\"lines\":[\"INFO source=api a\",\"ERROR source=api b\"]}'"
  echo "     stateful-class  (GPU; RUN_GPU=1) ${CLI} deploy Embedder.embed --project examples/stateful-class --app modal-rust-stateful-class"
  echo "                     ${CLI} call Embedder.embed --app modal-rust-stateful-class --input '{\"text\":\"hello\"}'"
  echo "     cuda-vector-add ${CLI} run vector_add --project examples/cuda-vector-add --input '{\"n\":1024}'"
  echo "     burn-add        (deploy+call on a T4; RUN_GPU=1)"
  echo "     cli-workflow    ${CLI} run summarize --project examples/cli-workflow --input '{\"text\":\"the quick brown fox\"}'"
  echo "                     ${CLI} deploy summarize --project examples/cli-workflow --app modal-rust-cli-workflow-example"
  echo "                     ${CLI} call summarize --app modal-rust-cli-workflow-example --input '{\"text\":\"the quick brown fox\"}'"
  echo
else
  # LIVE (CPU): one orchestrate run drives .remote() + deploy + call.
  live "orchestrate: live .remote() + deploy + call (CPU)" \
    "RUN_REMOTE=1 cargo run -q -p example-orchestrate --bin orchestrate" \
    'remote: add(40, 2) -> {sum: 42}' \
    'call: add(40, 2) -> {sum: 42}'

  # LIVE (CPU): ways-to-call drives the three remote shapes (.remote/.spawn/.map).
  live "ways-to-call: live .remote() + .spawn() + .map() (CPU)" \
    "RUN_REMOTE=1 cargo run -q -p example-ways-to-call --bin ways_to_call" \
    'remote: square(6) -> 36' \
    'spawn:  square(7) -> 49' \
    'map:    square([2, 3, 4]) -> [4, 9, 16]'

  # LIVE (CPU): fan-out-map fans the per-record analyze() out over N docs via .map().
  live "fan-out-map: live .map([..]) fan-out (CPU)" \
    "RUN_REMOTE=1 cargo run -q -p example-fan-out-map --bin fan_out_map" \
    'remote .map([..]) over 3 docs (results in input order):' \
    'intro -> 8 words, 1 min'

  # LIVE (CPU): background-jobs fires the job with .spawn() and polls the handle.
  live "background-jobs: live .spawn() + .get(timeout) (CPU)" \
    "RUN_REMOTE=1 cargo run -q -p example-background-jobs --bin background_jobs" \
    'spawn: job fired -> handle ' \
    "get:   job 'nightly-report' done -> 250000 rounds, digest 17267777379177717202"

  # LIVE (CPU): spawn-map-foreach drives .for_each([..]) (waits, discards) then
  # .spawn_map([..]) (fire-and-forget, returns a handle).
  live "spawn-map-foreach: live .for_each([..]) + .spawn_map([..]) (CPU)" \
    "RUN_REMOTE=1 cargo run -q -p example-spawn-map-foreach --bin spawn_map_foreach" \
    'for_each (live): notified 3 recipients across containers, results discarded' \
    'spawn_map (live): fired fan-out, handle '

  # LIVE (CPU): own-runner-bin — exercises the auto-detect "use the existing bin"
  # path through the CLI. The CLI detects the `modal_runner` bin in cargo metadata
  # and builds + uses IT (not a generated shadow runner). The function crunches a
  # small log batch and returns a Metrics struct.
  live "own-runner-bin: auto-detect existing modal_runner via CLI run (CPU)" \
    "${CLI} run extract_metrics --project examples/own-runner-bin --input '{\"lines\":[\"INFO source=api a\",\"ERROR source=api b\"]}'" \
    '"ok":true' \
    '"total":2'

  if [[ "${RUN_GPU:-}" == "1" ]]; then
    # GPU: cuda-vector-add on a T4 via the RUN path (in-body build, Tier 0).
    live "cuda-vector-add: run on a T4 (.remote())" \
      "${CLI} run vector_add --project examples/cuda-vector-add --input '{\"n\":1024}'" \
      '"valid":true'

    # GPU: burn-add deployed + called on a T4 (CUDA-devel image, Tier 1). The
    # first deploy build is slow; re-deploys reuse the cached image.
    live "burn-add: deploy + call on a T4" \
      "MODAL_RUST_BASE_IMAGE=nvidia/cuda:12.6.3-devel-ubuntu22.04 MODAL_RUST_INSTALL_RUST=1 ${CLI} deploy burn_add --project examples/burn-add --app modal-rust-burn-add-example && ${CLI} call burn_add --app modal-rust-burn-add-example --input '{\"n\":256}'" \
      '"valid":true'

    # GPU: stateful-class deploy + call on a T4. The methods are GPU functions
    # (`embed`=A10G, `dim`=T4 inherited), so this rides the GPU tier. The CLI takes the
    # registered DOTTED entrypoint name — `Embedder.embed` — which is the live proof that
    # Modal accepts a `.`-containing object tag for a per-method Cls entrypoint.
    live "stateful-class: deploy + call Cls method on a T4 (dotted entrypoint)" \
      "${CLI} deploy Embedder.embed --project examples/stateful-class --app modal-rust-stateful-class && ${CLI} call Embedder.embed --app modal-rust-stateful-class --input '{\"text\":\"hello\"}'" \
      '"ok":true'
  else
    echo "── GPU tier skipped (set RUN_GPU=1 to run the real T4 examples):"
    echo "     cuda-vector-add ${CLI} run vector_add --project examples/cuda-vector-add --input '{\"n\":1024}'"
    echo "     burn-add        deploy + call on a T4 (CUDA-devel image)"
    echo "     stateful-class  deploy + call Embedder.embed on a T4 (dotted entrypoint)"
    echo
  fi
fi

echo "RESULT: ${pass} passed, ${fail} failed"
[ "${fail}" -eq 0 ]
