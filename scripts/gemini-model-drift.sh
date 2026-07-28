#!/usr/bin/env bash
# gemini-model-drift.sh — diff our pinned Gemini model ids against the LIVE catalog.
#
# Model ids flip: `gemini-2.5-flash` started 400ing and
# `gemini-2.0-flash-exp-image-generation` was withdrawn entirely (404) while still
# pinned — generate_image was broken in production until telemetry #77 surfaced it.
# This script is the standing answer to "is there a newer Flash?".
#
#   scripts/gemini-model-drift.sh          # report
#   scripts/gemini-model-drift.sh --check  # non-zero exit if a pin is DEAD (CI-shaped)
#
# Key: $GEMINI_API_KEY, else GEMINI_API_KEY= in ./.env. Never printed.
set -euo pipefail
cd "$(dirname "$0")/.."

CHECK=0
[ "${1:-}" = "--check" ] && CHECK=1

KEY="${GEMINI_API_KEY:-}"
if [ -z "$KEY" ] && [ -f .env ]; then
  KEY=$(grep -E '^GEMINI_API_KEY=' .env | head -1 | cut -d= -f2- | tr -d '\r"' || true)
fi
if [ -z "$KEY" ]; then
  echo "no GEMINI_API_KEY (env or .env) — skipping drift check" >&2
  exit 0
fi

pin_of() { grep -E "^pub const $1" src/types.rs | sed 's/.*= "//;s/".*//'; }
CHAT_PIN=$(pin_of DEFAULT_MODEL)
IMAGE_PIN=$(pin_of DEFAULT_IMAGE_GENERATION_MODEL)

LIVE=$(curl -fsS "https://generativelanguage.googleapis.com/v1beta/models?key=$KEY&pageSize=200" \
  | grep -oE '"name": "models/[^"]+"' | sed 's/.*models\///;s/"//' | sort)

echo "pinned chat : $CHAT_PIN"
echo "pinned image: $IMAGE_PIN"
echo

rc=0
for pin in "$CHAT_PIN" "$IMAGE_PIN"; do
  if printf '%s\n' "$LIVE" | grep -qx "$pin"; then
    echo "OK    $pin is live"
  else
    echo "DEAD  $pin is NOT in the live catalog — fix src/types.rs"
    rc=1
  fi
done

# Version-rank the stable (non-preview) Flash families so "is there a newer one?"
# is answered mechanically. `sort -V` orders gemini-3.5 < gemini-3.6 correctly.
newer() { # $1 = pin, $2 = grep filter for the family
  printf '%s\n' "$LIVE" | grep -E "$2" | grep -v -- '-preview' | sort -V \
    | awk -v pin="$1" 'BEGIN{seen=0} {if(seen)print "  "$0} $0==pin{seen=1}'
}

echo
echo "newer stable chat Flash ids than $CHAT_PIN:"
n=$(newer "$CHAT_PIN" '^gemini-[0-9.]+-flash$'); [ -n "$n" ] && echo "$n" || echo "  (none)"
echo "newer stable image ids than $IMAGE_PIN:"
n=$(newer "$IMAGE_PIN" '^gemini-[0-9.]+(-flash|-pro)-image$'); [ -n "$n" ] && echo "$n" || echo "  (none)"

echo
echo "note: a newer id is a CANDIDATE, not an upgrade — probe tool-calling + SSE"
echo "      against it before flipping the pin (scripts/probe-gemini.ps1)."

[ "$CHECK" = 1 ] && exit $rc
exit 0
