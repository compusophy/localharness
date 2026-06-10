#!/usr/bin/env bash
# scripts/test-fleet/run-fleet.sh — drive the localharness test-user fleet.
#
# A standing fleet of 12 persistent on-chain agent identities (personas.json),
# each a distinct personality, that dogfood localharness and file GROUNDED
# feedback on-chain. For each selected persona:
#
#   1. create it on-chain with its persona (sponsored mint; idempotent — a
#      persona that already exists is reused, not re-minted)
#   2. send its `probe` to a live agent (a REAL interaction, real response)
#   3. reflect on that ACTUAL experience IN PERSONA and write exactly one
#      [BUG] / [FEATURE] / [FEEDBACK] item (anchored to the real probe + reply,
#      never hallucinated)
#   4. submit it on-chain (FeedbackFacet)
#
# Read the harvest with:  scripts/harvest-feedback.sh   (or  localharness feedback)
#
# Usage:
#   scripts/test-fleet/run-fleet.sh                  # all 12 personas
#   scripts/test-fleet/run-fleet.sh nova-qa pip-qa   # just these (a sample)
#   LOCALHARNESS_BIN=/path/to/localharness scripts/test-fleet/run-fleet.sh ...
#
# Cost: spends the sponsor's AlphaUSD gas (one mint + one feedback write per
# NEW persona; reused personas only pay the feedback write). Model calls are
# free in the beta — a $LH session opens automatically for any identity.
# Needs `node` (for JSON parsing) and a built `localharness`.
set -uo pipefail
cd "$(dirname "$0")/../.."

CLI="${LOCALHARNESS_BIN:-./target/debug/localharness.exe}"
[ -x "$CLI" ] || CLI="./target/debug/localharness"
[ -x "$CLI" ] || CLI="cargo run --quiet --features wallet --bin localharness --"
command -v node >/dev/null 2>&1 || { echo "run-fleet: needs node on PATH (for JSON parsing)" >&2; exit 1; }

JSON="$(dirname "$0")/personas.json"

# pj <persona-name> <field>  → the field's value (focus|persona|probe), or
# with name "" and field "target"/"names" → the top-level target / all names.
pj() {
  node -e '(() => {
    const fs = require("fs");
    const d = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
    const name = process.argv[2], field = process.argv[3];
    if (field === "names") { d.personas.forEach(p => console.log(p.name)); return; }
    if (field === "target") { process.stdout.write(d.target || ""); return; }
    const p = d.personas.find(x => x.name === name);
    process.stdout.write(p ? (p[field] || "") : "");
  })();' "$JSON" "$1" "$2"
}

TARGET="$(pj "" target)"

if [ "$#" -gt 0 ]; then
  SELECT=("$@")
else
  mapfile -t SELECT < <(pj "" names)
fi

submitted=0
for NAME in "${SELECT[@]}"; do
  PERSONA="$(pj "$NAME" persona)"
  PROBE="$(pj "$NAME" probe)"
  FOCUS="$(pj "$NAME" focus)"
  [ -n "$PERSONA" ] || { echo "?? unknown persona '$NAME' — skipping"; continue; }
  echo "== $NAME — $FOCUS =="

  # 1. create on-chain identity + persona. Idempotent on the LOCAL KEY: we
  # control a persona iff we hold its key (`whoami` is a read that returns
  # registered:false with exit 0, so it can't tell ownership).
  if [ -f "${NAME}.localharness.key" ] || [ -f "${LOCALHARNESS_HOME:-$HOME/.localharness/keys}/${NAME}.localharness.key" ]; then
    echo "  · identity exists (reusing local key)"
  else
    echo "  · creating on-chain identity + persona…"
    $CLI create "$NAME" --persona "$PERSONA" >/dev/null 2>&1 || { echo "  ✗ create failed (name taken?) — skipping"; continue; }
  fi

  # 2. real interaction with a live agent
  echo "  · probing $TARGET…"
  EXPERIENCE="$($CLI call --as "$NAME" --fresh "$TARGET" "$PROBE" 2>&1)" \
    || { echo "  ✗ probe failed: $EXPERIENCE"; continue; }

  # 3. reflect IN PERSONA on the ACTUAL experience → one grounded item
  REFLECT="You just used localharness. You asked: \"$PROBE\" and the agent replied: \"$EXPERIENCE\". Based on your personality and this REAL experience, write exactly ONE concrete piece of feedback for the localharness maintainers. Start it with [BUG], [FEATURE], or [FEEDBACK]. One item only, under 280 characters, in your own voice, no preamble or sign-off."
  FEEDBACK="$($CLI call --as "$NAME" --fresh "$NAME" "$REFLECT" 2>&1)" \
    || { echo "  ✗ reflect failed: $FEEDBACK"; continue; }
  FEEDBACK="$(printf '%s' "$FEEDBACK" | tr -d '\r' | tr '\n' ' ' | sed 's/  */ /g' | head -c 2000)"
  echo "  · ${FEEDBACK}"

  # 4. submit on-chain
  if $CLI feedback --as "$NAME" "$FEEDBACK" >/dev/null 2>&1; then
    echo "  ✓ submitted on-chain"
    submitted=$((submitted + 1))
  else
    echo "  ✗ feedback submit failed"
  fi
done

echo
echo "fleet run complete — $submitted item(s) submitted on-chain."
echo "read them:  scripts/harvest-feedback.sh   (or  $CLI feedback)"
