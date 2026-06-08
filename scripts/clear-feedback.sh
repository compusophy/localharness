#!/usr/bin/env bash
# scripts/clear-feedback.sh — GC the on-chain feedback inbox (owner-only).
#
# The FeedbackFacet's storage array is append-only and grows unbounded — every
# fleet run + probe appends an entry that costs storage gas and lengthens the
# `feedbackRange` reads forever. Treat on-chain feedback as a TRANSIENT inbox:
# once you've harvested the notes off-chain (scripts/test-fleet/feedback-to-issues.mjs,
# or scripts/harvest-feedback.sh), GC the storage with this. The FeedbackSubmitted
# EVENT log is immutable but naturally windows out of the RPC's 100k-block
# `eth_getLogs` cap (so `localharness feedback` still shows the recent ones after
# a clear); the durable record should live off-chain (GitHub issues) after harvest.
#
# Owner-only (EIP-173): needs the diamond-owner key (EVM_PRIVATE_KEY in ./.env).
# Requires `cast` (foundry).
#
# Usage: scripts/clear-feedback.sh
set -euo pipefail
cd "$(dirname "$0")/.."

DIAMOND="${DIAMOND:-0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c}"
RPC="${RPC:-https://rpc.moderato.tempo.xyz}"
KEY="${EVM_PRIVATE_KEY:-$(grep -E '^EVM_PRIVATE_KEY=' .env 2>/dev/null | head -1 | cut -d= -f2- | tr -d "\"' ")}"
[ -n "$KEY" ] || { echo "clear-feedback: need the diamond-owner key (EVM_PRIVATE_KEY in ./.env)" >&2; exit 1; }
command -v cast >/dev/null 2>&1 || { echo "clear-feedback: needs cast (foundry)" >&2; exit 1; }

before=$(cast call "$DIAMOND" "feedbackCount()(uint256)" --rpc-url "$RPC")
echo "on-chain feedback storage entries: $before"
if [ "$before" = "0" ]; then
  echo "already empty — nothing to GC."
  exit 0
fi
echo "clearing (owner-only clearFeedback)…"
cast send "$DIAMOND" "clearFeedback()" --private-key "$KEY" --rpc-url "$RPC" >/dev/null
after=$(cast call "$DIAMOND" "feedbackCount()(uint256)" --rpc-url "$RPC")
echo "done — storage entries: $before -> $after"
echo "(recent feedback is still readable via the event log: localharness feedback)"
