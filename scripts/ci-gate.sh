#!/usr/bin/env bash
# ci-gate.sh — the canonical local CI gate.
#
# Mirrors .github/workflows/ci.yml COMMAND-FOR-COMMAND (same flags, same
# exclusions) so "passes locally" and "passes CI" cannot diverge. If you edit
# ci.yml, edit this script in the same commit (and vice versa).
#
# CI context mirrored here:
#   - RUST_TOOLCHAIN is pinned in ci.yml and must match rust-toolchain.toml.
#   - No --workspace / --all-features anywhere: that would pull the CUDA-only
#     GPU examples (example-burn-add, example-cuda-vector-add); default-members
#     (set in Cargo.toml) excludes them.
#
# Usage: scripts/ci-gate.sh
# Exits non-zero on the first failing step.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

RUST_TOOLCHAIN="1.96.0" # keep in lockstep with ci.yml + rust-toolchain.toml

run() {
  echo "==> $*"
  "$@"
}

# ci.yml step: Check Formatting
run cargo +"$RUST_TOOLCHAIN" fmt --check

# ci.yml step: Lint (default-members exclude the CUDA-only examples)
run cargo +"$RUST_TOOLCHAIN" clippy --all-targets -- -D warnings

# ci.yml step: Lint (light facade — default features, no client). Lib-only on
# purpose: --all-targets pulls the facade's self dev-dep back in and lights
# `client`; the light surface IS the lib.
run cargo +"$RUST_TOOLCHAIN" clippy -p modal-rust -p modal-rust-macros -- -D warnings

# ci.yml step: Test
run cargo +"$RUST_TOOLCHAIN" test

# ci.yml step: Check Whitespace
run git diff --check

echo "ci-gate: all steps green"
