#!/usr/bin/env bash
# Validate the README examples by running each one and checking its output.
#
# These are the exact OFFLINE commands shown in README.md's "Examples" section —
# run them one by one and assert stdout. No Modal credentials, no network.
# The Modal-requiring commands (.remote() / deploy / GPU) are listed but NOT run.
#
#   bash scripts/check-examples.sh
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

# quickstart — the headline (auto-I/O add)
run "quickstart: add(2, 3)" '{"ok":true,"value":5}' \
  "cd examples/quickstart && cargo run -q --bin modal_runner -- --entrypoint add --input-json '{\"a\":2,\"b\":3}'"

run "quickstart: --describe lists add" '"name":"add"' \
  "cd examples/quickstart && cargo run -q --bin modal_runner -- --describe"

# add-macro — macro path (struct I/O)
run "add-macro: add(40, 2)" '{"ok":true,"value":42}' \
  "cd examples/add-macro && cargo run -q --bin modal_runner -- --entrypoint add --input-json '{\"a\":40,\"b\":2}'"

# add — manual / no-macro path ({sum} output)
run "add (manual): add(40, 2)" '{"ok":true,"value":{"sum":42}}' \
  "cd examples/add && cargo run -q --bin modal_runner -- --entrypoint add --input-json '{\"a\":40,\"b\":2}'"

# orchestrate — the local tour (manual + macro/inventory + auto-I/O)
run "orchestrate: local tour" 'add(2, 3) -> 5' \
  "cd examples/orchestrate && cargo run -q --bin orchestrate"

# cuda-vector-add — decorator-is-config, proven OFFLINE via --describe
run "cuda-vector-add: --describe (gpu rides through inventory)" '"gpu":"T4"' \
  "cd examples/cuda-vector-add && cargo run -q --bin modal_runner -- --describe"

echo "── Modal-required (NOT run here — need credentials + a GPU):"
echo "     quickstart  .remote()      RUN_REMOTE=1 cargo run -p example-orchestrate"
echo "     cuda-vector-add / burn-add  on a T4 via .remote() / deploy+call"
echo
echo "RESULT: ${pass} passed, ${fail} failed"
[ "${fail}" -eq 0 ]
