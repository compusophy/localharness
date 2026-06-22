#!/usr/bin/env bash
# NOTE: feedback is now OFF-CHAIN by default — the primary task list is GitHub
# Issues in the private telemetry repo (`gh issue list -R
# compusophy/localharness-telemetry`). This script only reads the OPT-IN on-chain
# mirror (`lh_feedback_onchain`), which most agents no longer write.
#
# Read every feedback submission from CONTRACT STATE on the registry
# diamond and print one row per submission: <index>  <unix-ts>  <sender>
# <text>. Reads via view functions (no event-log scraping, so the Tempo
# 100k-block log window no longer hides older notes).
#
# Usage: ./scripts/harvest-feedback.sh [--unresolved]
#   --unresolved  hide indices listed in docs/feedback-resolved.txt
# Env overrides: DIAMOND, RPC, RESOLVED (path to the resolved-index file).
#
# Requires `cast` (foundry).

set -euo pipefail

DIAMOND="${DIAMOND:-0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c}"
RPC="${RPC:-https://rpc.moderato.tempo.xyz}"

UNRESOLVED=0
[ "${1:-}" = "--unresolved" ] && UNRESOLVED=1
RESOLVED="${RESOLVED:-"$(dirname "$0")/../docs/feedback-resolved.txt"}"

# True when --unresolved is set AND index $1 appears (as the first whitespace
# token of a non-comment, non-blank line) in the resolved-index file. Missing
# file => empty set => nothing hidden.
is_resolved() {
    [ "$UNRESOLVED" -eq 1 ] || return 1
    [ -f "$RESOLVED" ] || return 1
    awk -v idx="$1" '
        /^[[:space:]]*#/ { next }
        /^[[:space:]]*$/ { next }
        { if ($1 == idx) found=1 }
        END { exit(found?0:1) }
    ' "$RESOLVED"
}

COUNT=$(cast call "$DIAMOND" "feedbackCount()(uint256)" --rpc-url "$RPC")
# cast may append units in parentheses (e.g. "3 [3e0]"); keep the integer.
COUNT="${COUNT%% *}"

if [ "$COUNT" -eq 0 ]; then
    echo "no feedback yet"
    exit 0
fi

i=0
while [ "$i" -lt "$COUNT" ]; do
    # Skip resolved indices up front (also avoids the feedbackAt RPC).
    if is_resolved "$i"; then i=$((i + 1)); continue; fi
    # feedbackAt returns (address sender, uint64 timestamp, string text),
    # one value per line.
    OUT=$(cast call "$DIAMOND" "feedbackAt(uint256)(address,uint64,string)" "$i" --rpc-url "$RPC")
    SENDER=$(printf '%s\n' "$OUT" | sed -n '1p')
    TS=$(printf '%s\n' "$OUT" | sed -n '2p')
    TS="${TS%% *}"
    TEXT=$(printf '%s\n' "$OUT" | sed -n '3,$p')
    printf '%s\t%s\t%s\t%s\n' "$i" "$TS" "$SENDER" "$TEXT"
    i=$((i + 1))
done
