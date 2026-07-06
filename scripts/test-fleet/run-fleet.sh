#!/usr/bin/env bash
# scripts/test-fleet/run-fleet.sh — drive the localharness test-user fleet.
#
# A standing fleet of 12 persistent on-chain agent identities (personas.json),
# each a distinct personality, that dogfood localharness and file GROUNDED
# feedback. For each selected persona:
#
#   1. create it on-chain with its persona (sponsored mint; idempotent — a
#      persona that already exists is reused, not re-minted)
#   2. send its `probe` to a live agent (a REAL interaction, real response)
#   3. reflect on that ACTUAL experience IN PERSONA and write exactly one
#      [BUG] / [FEATURE] / [FEEDBACK] item (anchored to the real probe + reply,
#      never hallucinated)
#   4. file it off-chain (`localharness feedback` → the proxy telemetry
#      endpoint → a GitHub issue in the telemetry repo)
#
# Read the harvest in the telemetry repo's issues (label `feedback`).
#
# Usage:
#   scripts/test-fleet/run-fleet.sh                  # all 12 personas
#   scripts/test-fleet/run-fleet.sh nova-qa pip-qa   # just these (a sample)
#   LOCALHARNESS_BIN=/path/to/localharness scripts/test-fleet/run-fleet.sh ...
#
# Cost: spends the sponsor's gas for one mint per NEW persona (feedback itself
# is off-chain + free). Model calls are
# NOT free: the proxy meters ~1 $LH per call (it gates on an active session
# OR a meter balance >= the cost, 402 otherwise) and the CLI deliberately does
# NOT auto-open the 10-$LH/hr session. A fresh persona holds 0 $LH AND on mainnet
# `create` itself costs ~1 $LH pulled from the wallet, so this script funds each
# persona's ADDRESS ~4 $LH from the funded `claude` identity BEFORE create (probe +
# reflect are ~1 $LH each; warns + continues if the send fails).
# Needs `node` (for JSON parsing) and a built `localharness`.
set -uo pipefail
cd "$(dirname "$0")/../.."

CLI="${LOCALHARNESS_BIN:-./target/debug/localharness.exe}"
[ -x "$CLI" ] || CLI="./target/debug/localharness"
[ -x "$CLI" ] || CLI="cargo run --quiet --features wallet --bin localharness --"
command -v node >/dev/null 2>&1 || { echo "run-fleet: needs node on PATH (for JSON parsing)" >&2; exit 1; }

JSON="$(dirname "$0")/personas.json"
# CLI stderr goes here, NOT into captured payloads (a filed feedback body must
# not start with '· localharness on Tempo…' chatter); errors still get echoed.
ERRLOG="$(mktemp)"; trap 'rm -f "$ERRLOG"' EXIT

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

# fund_persona <name> <amount>: fund the local-keyed identity's ADDRESS from claude.
# `send` by NAME fails for an UNREGISTERED name, so resolve the 0x address from the
# identity's own `status` (works off the local key even pre-registration) and send to
# the address. Needed because on mainnet `create` COSTS ~1 $LH pulled from the
# claimer's wallet, so the persona must hold funds BEFORE create (not after).
fund_persona() {
  local name="$1" amt="$2" addr
  addr="$($CLI status --as "$name" 2>/dev/null | grep -iE 'your wallet' | grep -oiE '0x[0-9a-f]{40}' | head -1)"
  if [ -z "$addr" ]; then echo "  !! could not resolve $name's address — funding skipped"; return 1; fi
  if $CLI send --as claude "$addr" "$amt" >/dev/null 2>&1; then
    echo "  · funded $amt \$LH (from claude → $addr)"; return 0
  else
    echo "  !! could not fund $name ($addr) from claude — create/calls may 402"; return 1
  fi
}

submitted=0
for NAME in "${SELECT[@]}"; do
  PERSONA="$(pj "$NAME" persona)"
  PROBE="$(pj "$NAME" probe)"
  FOCUS="$(pj "$NAME" focus)"
  [ -n "$PERSONA" ] || { echo "?? unknown persona '$NAME' — skipping"; continue; }
  echo "== $NAME — $FOCUS =="

  # 1. create on-chain identity + persona. A LOCAL KEY alone doesn't prove the
  # name is registered (pre-reset keys, released names) — verify on-chain and
  # re-claim when stale; `create` is idempotent and REUSES an existing key.
  KEYDIR="${LOCALHARNESS_HOME:-$HOME/.localharness/keys}"
  HAS_KEY=0
  { [ -f "${NAME}.localharness.key" ] || [ -f "$KEYDIR/${NAME}.localharness.key" ]; } && HAS_KEY=1
  if [ "$HAS_KEY" = 1 ] && ! $CLI whoami "$NAME" 2>/dev/null | grep -q "unregistered"; then
    # Registered already — just top up so the probe + reflect (~1 $LH each) don't 402.
    echo "  · identity registered (reusing local key) — topping up"
    fund_persona "$NAME" 3
  else
    # Unregistered (or no key) — on mainnet create COSTS ~1 $LH pulled from the wallet,
    # so FUND FIRST (4 = 1 claim + ~2 probe/reflect + buffer), THEN create + persona.
    echo "  · claiming '$NAME' on-chain (fund-then-create; persona attached)…"
    fund_persona "$NAME" 4
    $CLI create "$NAME" --persona "$PERSONA" >/dev/null 2>&1 \
      || { echo "  ✗ create failed (unfunded / name taken?) — skipping"; continue; }
  fi

  # 2. real interaction with a live agent. On a 402 (existing persona with an
  # empty meter — the create-branch funding above only covers NEW personas),
  # best-effort fund from claude and retry ONCE; a funded persona never pays
  # the extra read, and a failed send degrades to today's failure.
  echo "  · probing $TARGET…"
  EXPERIENCE="$($CLI call --as "$NAME" --fresh "$TARGET" "$PROBE" 2>"$ERRLOG")"
  RC=$?
  if [ $RC -ne 0 ] && grep -q "402" "$ERRLOG"; then
    echo "  · probe 402'd (empty meter) — topping up 2 \$LH + retrying"
    if fund_persona "$NAME" 2; then
      EXPERIENCE="$($CLI call --as "$NAME" --fresh "$TARGET" "$PROBE" 2>"$ERRLOG")" \
        || { echo "  ✗ probe failed after funding: ${EXPERIENCE:-$(cat "$ERRLOG")}"; continue; }
    else
      echo "  ✗ probe failed (and could not fund from claude): ${EXPERIENCE:-$(cat "$ERRLOG")}"; continue
    fi
  elif [ $RC -ne 0 ]; then
    echo "  ✗ probe failed: ${EXPERIENCE:-$(cat "$ERRLOG")}"; continue
  fi

  # 3. reflect IN PERSONA on the ACTUAL experience → one grounded item
  REFLECT="You just used localharness. You asked: \"$PROBE\" and the agent replied: \"$EXPERIENCE\". Based on your personality and this REAL experience, write exactly ONE concrete piece of feedback for the localharness maintainers. Start it with [BUG], [FEATURE], or [FEEDBACK]. One item only, under 280 characters, in your own voice, no preamble or sign-off."
  FEEDBACK="$($CLI call --as "$NAME" --fresh "$NAME" "$REFLECT" 2>"$ERRLOG")" \
    || { echo "  ✗ reflect failed: ${FEEDBACK:-$(cat "$ERRLOG")}"; continue; }
  FEEDBACK="$(printf '%s' "$FEEDBACK" | tr -d '\r' | tr '\n' ' ' | sed 's/  */ /g' | head -c 2000)"
  echo "  · ${FEEDBACK}"

  # 4. file via the telemetry endpoint (GitHub issue in the telemetry repo)
  if $CLI feedback --as "$NAME" "$FEEDBACK" >/dev/null 2>&1; then
    echo "  ✓ filed (telemetry → GitHub issue)"
    submitted=$((submitted + 1))
  else
    echo "  ✗ feedback submit failed"
  fi
done

echo
echo "fleet run complete — $submitted item(s) filed."
echo "read them in the telemetry repo's issues (label: feedback)."
