#!/usr/bin/env bash
# Read every feedback submission from CONTRACT STATE on the registry
# diamond and print one row per submission: <index>  <unix-ts>  <sender>
# <text>. Reads via view functions (no event-log scraping, so the Tempo
# 100k-block log window no longer hides older notes).
#
# Usage: ./scripts/harvest-feedback.sh
# Env overrides: DIAMOND, RPC.
#
# Requires `cast` (foundry).

set -euo pipefail

DIAMOND="${DIAMOND:-0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c}"
RPC="${RPC:-https://rpc.moderato.tempo.xyz}"

COUNT=$(cast call "$DIAMOND" "feedbackCount()(uint256)" --rpc-url "$RPC")
# cast may append units in parentheses (e.g. "3 [3e0]"); keep the integer.
COUNT="${COUNT%% *}"

if [ "$COUNT" -eq 0 ]; then
    echo "no feedback yet"
    exit 0
fi

i=0
while [ "$i" -lt "$COUNT" ]; do
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
