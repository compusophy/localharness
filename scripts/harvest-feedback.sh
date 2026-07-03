#!/usr/bin/env bash
# THIN DELEGATING SHIM over scripts/check-feedback.mjs (the build-web.ps1
# pattern: logic lives in ONE maintained script). This script used to read the
# FeedbackFacet itself via `cast`, but it stayed pinned to the TESTNET diamond
# after the mainnet migration (stale chain) and required foundry.
# check-feedback.mjs is the replacement: node-only, reads BOTH chains, and
# knows the per-chain resolved-index files.
#
# Usage: ./scripts/harvest-feedback.sh [--unresolved]
#   --unresolved  hide resolved indices (mapped to check-feedback's --open)
# The old DIAMOND/RPC/RESOLVED env overrides no longer apply.

set -euo pipefail

args=()
for a in "$@"; do
    if [ "$a" = "--unresolved" ]; then args+=("--open"); else args+=("$a"); fi
done
exec node "$(dirname "$0")/check-feedback.mjs" ${args[@]+"${args[@]}"}
