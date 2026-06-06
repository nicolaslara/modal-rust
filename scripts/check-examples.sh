#!/usr/bin/env bash
# Validate the README examples by running each one and checking its output.
#
# Three tiers, each gated on what the host can actually do:
#
#   1. OFFLINE   — always run. The in-process `.local()` / `--describe` commands
#                  from README.md's "Examples" section. No credentials, no network.
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

# ───────────────────────── 1. OFFLINE (always) ─────────────────────────

# quickstart — the headline (auto-I/O add)
run "quickstart: add(2, 3)" '{"ok":true,"value":5}' \
  "cd examples/quickstart && cargo run -q --bin modal_runner -- --entrypoint add --input-json '{\"a\":2,\"b\":3}'"

run "quickstart: --describe lists add" '"name":"add"' \
  "cd examples/quickstart && cargo run -q --bin modal_runner -- --describe"

# add-macro — macro path (struct I/O)
run "add-macro: add(40, 2)" '{"ok":true,"value":42}' \
  "cd examples/add-macro && cargo run -q --bin modal_runner -- --entrypoint add --input-json '{\"a\":40,\"b\":2}'"

# custom-types — a function over YOUR OWN structs (macro infers I/O from the signature)
run "custom-types: score(Player) -> Scored" '{"ok":true,"value":{"accuracy_pct":70,"name":"Ada","points":700}}' \
  "cd examples/custom-types && cargo run -q --bin modal_runner -- --entrypoint score --input-json '{\"name\":\"Ada\",\"hits\":7,\"shots\":10}'"

# add — manual / no-macro path ({sum} output)
run "add (manual): add(40, 2)" '{"ok":true,"value":{"sum":42}}' \
  "cd examples/add && cargo run -q --bin modal_runner -- --entrypoint add --input-json '{\"a\":40,\"b\":2}'"

# orchestrate — the local tour (manual + macro/inventory + auto-I/O)
run "orchestrate: local tour" 'add(2, 3) -> 5' \
  "cd examples/orchestrate && cargo run -q --bin orchestrate"

# ways-to-call — one function, four invocation shapes; OFFLINE runs the .local() one
run "ways-to-call: .local() tour" 'local:  square(6) -> 36' \
  "cargo run -q -p example-ways-to-call --bin ways_to_call"

# fan-out-map — embarrassingly-parallel scale-out; OFFLINE runs the local fan-out
run "fan-out-map: local fan-out (results in input order)" 'intro -> 8 words, 1 min' \
  "cargo run -q -p example-fan-out-map --bin fan_out_map"

# cuda-vector-add — decorator-is-config, proven OFFLINE via --describe
run "cuda-vector-add: --describe (gpu rides through inventory)" '"gpu":"T4"' \
  "cd examples/cuda-vector-add && cargo run -q --bin modal_runner -- --describe"

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
  echo "     cuda-vector-add cargo run -p modal-rust-cli -- run vector_add --project examples/cuda-vector-add --input '{\"n\":1024}'"
  echo "     burn-add        (deploy+call on a T4; RUN_GPU=1)"
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

  if [[ "${RUN_GPU:-}" == "1" ]]; then
    # GPU: cuda-vector-add on a T4 via the RUN path (in-body build, Tier 0).
    live "cuda-vector-add: run on a T4 (.remote())" \
      "cargo run -q -p modal-rust-cli -- run vector_add --project examples/cuda-vector-add --input '{\"n\":1024}'" \
      '"valid":true'

    # GPU: burn-add deployed + called on a T4 (CUDA-devel image, Tier 1). The
    # first deploy build is slow; re-deploys reuse the cached image.
    live "burn-add: deploy + call on a T4" \
      "MODAL_RUST_BASE_IMAGE=nvidia/cuda:12.6.3-devel-ubuntu22.04 MODAL_RUST_INSTALL_RUST=1 cargo run -q -p modal-rust-cli -- deploy burn_add --project examples/burn-add --app modal-rust-burn-add-example && cargo run -q -p modal-rust-cli -- call burn_add --app modal-rust-burn-add-example --input '{\"n\":256}'" \
      '"valid":true'
  else
    echo "── GPU tier skipped (set RUN_GPU=1 to run the real T4 examples):"
    echo "     cuda-vector-add cargo run -p modal-rust-cli -- run vector_add --project examples/cuda-vector-add --input '{\"n\":1024}'"
    echo "     burn-add        deploy + call on a T4 (CUDA-devel image)"
    echo
  fi
fi

echo "RESULT: ${pass} passed, ${fail} failed"
[ "${fail}" -eq 0 ]
