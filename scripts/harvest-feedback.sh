#!/usr/bin/env bash
# Pull every `FeedbackSubmitted` event from the registry diamond and
# print one row per submission: <iso-ts>  <sender>  <text>
#
# Usage: ./scripts/harvest-feedback.sh
# Env overrides: DIAMOND, RPC.
#
# Requires `cast` (foundry).

set -euo pipefail

DIAMOND="${DIAMOND:-0x6f2858b4b10bf8d4ea372a446e69bea8fbce2930}"
RPC="${RPC:-https://rpc.moderato.tempo.xyz}"

# Tempo caps eth_getLogs to a 100k-block window. Default to scanning
# the most recent 100k blocks; set FROM_BLOCK explicitly if you need
# more history (or paginate manually).
LATEST=$(cast block-number --rpc-url "$RPC")
FROM_BLOCK="${FROM_BLOCK:-$((LATEST - 99000))}"
if [ "$FROM_BLOCK" -lt 0 ]; then FROM_BLOCK=0; fi

cast logs \
    --address "$DIAMOND" \
    --rpc-url "$RPC" \
    --from-block "$FROM_BLOCK" \
    --to-block "$LATEST" \
    'event FeedbackSubmitted(address indexed sender, uint256 timestamp, string text)'
