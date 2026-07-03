#!/usr/bin/env bash
# OPT-IN live money-path smoke — spends REAL $LH on Tempo MAINNET. NEVER CI.
#
# Automates the verification pattern the operator runs by hand after touching a
# money path: prove, to the WEI, that the two live flows conserve funds.
#
#   STAGE A (always)   sponsored bounty escrow round-trip: post a 0.02-$LH
#                      bounty, cancel it, assert the wallet balance is EXACTLY
#                      equal before and after. Net-zero $LH (the relay/sponsor
#                      pays the ~2 txs of gas in the fee token, not you).
#   STAGE B (--spend)  one metered `call` through the credit proxy. The x402
#                      path settles the model price from the caller's WALLET;
#                      assert the wallet dropped by EXACTLY the amount the CLI
#                      printed ("x402: paying X LH per call"). Costs ~1 $LH.
#
# usage: scripts/smoke-money.sh --as <name> [--spend]
#   needs <name>'s mainnet identity key in place (~/.lh_<name>_mainnet.key);
#   LH_BIN overrides the binary (default target/release/localharness[.exe]).
#
# Gotchas this script encodes so you don't rediscover them:
#   - SETTLE LAG IS REAL: a sponsored/relayed tx returns at broadcast; balance
#     reads lag a few seconds behind. We sleep $SETTLE_WAIT (default 8s) before
#     every post-write balance read.
#   - Exact-equality asserts require a QUIET identity: don't run this while a
#     loop/scheduler/colony is spending as the same name.
#   - Wei values overflow bash's 64-bit arithmetic (18 $LH > 2^63 wei), so all
#     balance math here is string big-int (norm/big_ge/big_add).
set -euo pipefail
cd "$(dirname "$0")/.."

usage() { echo "usage: scripts/smoke-money.sh --as <name> [--spend]" >&2; exit 2; }
fail() { echo "FAIL — $1" >&2; exit 1; }
pass() { echo "  ok — $1"; }

NAME=""; SPEND=0
while [ $# -gt 0 ]; do
  case "$1" in
    --as) NAME="${2:-}"; [ -n "$NAME" ] || usage; shift 2 ;;
    --spend) SPEND=1; shift ;;
    *) usage ;;
  esac
done
[ -n "$NAME" ] || usage
SETTLE_WAIT="${SETTLE_WAIT:-8}"

# ---- binary ----------------------------------------------------------------
if [ -n "${LH_BIN:-}" ]; then BIN="$LH_BIN"
elif [ -x target/release/localharness.exe ]; then BIN=target/release/localharness.exe
elif [ -x target/release/localharness ]; then BIN=target/release/localharness
else
  echo "no release binary — building (cargo build --release --features wallet) …"
  cargo build --release --features wallet --bin localharness
  BIN=target/release/localharness
  [ -x "$BIN" ] || BIN=target/release/localharness.exe
fi
echo -n "binary: $BIN — "
"$BIN" --version

# ---- string big-int helpers (wei exceeds bash's 64-bit ints) ---------------
norm() { # strip leading zeros; empty -> 0
  local s; s="$(printf '%s' "$1" | sed 's/^0\{1,\}//')"; printf '%s\n' "${s:-0}"
}
big_ge() { # is A >= B ? (non-negative decimal strings)
  local a b; a="$(norm "$1")"; b="$(norm "$2")"
  if [ "${#a}" -ne "${#b}" ]; then [ "${#a}" -gt "${#b}" ]
  else [ "$a" = "$b" ] || [[ "$a" > "$b" ]]; fi
}
big_add() { # A + B (non-negative decimal strings)
  local a="$1" b="$2" out="" carry=0 da db s
  while [ -n "$a" ] || [ -n "$b" ] || [ "$carry" -ne 0 ]; do
    da="${a: -1}"; db="${b: -1}"
    s=$(( ${da:-0} + ${db:-0} + carry ))
    out="$(( s % 10 ))${out}"; carry=$(( s / 10 ))
    a="${a%?}"; b="${b%?}"
  done
  norm "$out"
}
lh_to_wei() { # "X.YY" (the CLI's 2-dp fmt_lh rendering) -> wei string
  case "$1" in
    *.??) ;;
    *) fail "unparseable LH amount '$1' (expected X.YY)" ;;
  esac
  norm "${1%%.*}${1#*.}0000000000000000" # whole ++ 2 frac digits ++ 16 zeros
}

# ---- CLI plumbing -----------------------------------------------------------
run() { # run <arg…> — echo the command + its FULL output (tx hashes included)
  local rc=0
  echo "\$ localharness $*"
  OUT="$("$BIN" "$@" 2>&1)" || rc=$?
  printf '%s\n' "$OUT" | sed 's/^/  | /'
  return $rc
}
wallet_wei() { # <name>'s owner-EOA $LH wallet balance, exact wei (whoami --json)
  local out wei
  out="$("$BIN" whoami "$NAME" --json 2>/dev/null)" ||
    fail "whoami $NAME failed — is the RPC reachable?"
  wei="$(printf '%s\n' "$out" | sed -n 's/.*"walletLhWei": *"\([0-9][0-9]*\)".*/\1/p')"
  [ -n "$wei" ] || fail "could not read walletLhWei for $NAME (owner balance read failed?)"
  printf '%s\n' "$wei"
}

# ---- STAGE A: sponsored bounty escrow round-trip (net-zero $LH) -------------
REWARD="0.02"
REWARD_WEI="$(lh_to_wei "$REWARD")"

echo
echo "[A] bounty post + cancel as '$NAME' — wallet must be net-zero to the wei"
START_A="$(wallet_wei)"
echo "  wallet before: $START_A wei"
# Guard: if the wallet can't cover the escrow, `bounty post` auto-bridges the
# shortfall from the chat meter — which would break the net-zero assert.
big_ge "$START_A" "$REWARD_WEI" ||
  fail "wallet ($START_A wei) is under the $REWARD-LH escrow — the meter auto-bridge would break net-zero; fund the wallet first"

run bounty --as "$NAME" post "smoke: money-path (auto-cancel)" --reward "$REWARD" --ttl 1h
BOUNTY_ID="$(printf '%s\n' "$OUT" | sed -n 's/.*bounty #\([0-9][0-9]*\) posted.*/\1/p' | head -n1)"
[ -n "$BOUNTY_ID" ] ||
  fail "posted but could not parse the bounty id — $REWARD LH IS ESCROWED; recover with: localharness bounty mine --as $NAME && localharness bounty cancel --as $NAME <id>"
pass "posted bounty #$BOUNTY_ID ($REWARD LH escrowed)"

run bounty --as "$NAME" cancel "$BOUNTY_ID"
pass "cancelled bounty #$BOUNTY_ID (escrow refunded)"

echo "  waiting ${SETTLE_WAIT}s for settlement (balance reads lag broadcast) …"
sleep "$SETTLE_WAIT"
END_A="$(wallet_wei)"
echo "  wallet after:  $END_A wei"
if [ "$(norm "$START_A")" != "$(norm "$END_A")" ]; then
  echo "STAGE A FAIL — wallet NOT net-zero after post+cancel:" >&2
  echo "  before: $START_A wei" >&2
  echo "  after:  $END_A wei" >&2
  exit 1
fi
pass "STAGE A: wallet exactly unchanged ($END_A wei)"

# ---- STAGE B (--spend): metered call, wallet drops by the printed x402 amount
if [ "$SPEND" -eq 1 ]; then
  echo
  echo "[B] metered call as '$NAME' (--spend: costs ~1 \$LH for real)"
  START_B="$(wallet_wei)"
  echo "  wallet before: $START_B wei"

  run call --as "$NAME" "$NAME" "Reply: ok"
  PAID_LH="$(printf '%s\n' "$OUT" | sed -n 's/.*x402: paying \([0-9][0-9]*\.[0-9][0-9]\) LH per call.*/\1/p' | head -n1)"
  [ -n "$PAID_LH" ] ||
    fail "no 'x402: paying X LH per call' line in the output — the proxy took the METER path (x402 payTo off, or wallet under the model price); the wallet-drop assert only holds on the x402 path"
  PAID_WEI="$(lh_to_wei "$PAID_LH")"
  pass "call replied; x402 printed $PAID_LH LH ($PAID_WEI wei)"

  echo "  waiting ${SETTLE_WAIT}s for the x402 settle …"
  sleep "$SETTLE_WAIT"
  END_B="$(wallet_wei)"
  echo "  wallet after:  $END_B wei"
  if [ "$(norm "$START_B")" != "$(big_add "$END_B" "$PAID_WEI")" ]; then
    echo "STAGE B FAIL — wallet drop != the printed x402 amount:" >&2
    echo "  before: $START_B wei" >&2
    echo "  after:  $END_B wei" >&2
    echo "  paid:   $PAID_WEI wei ($PAID_LH LH) — expected after+paid == before" >&2
    exit 1
  fi
  pass "STAGE B: wallet dropped exactly $PAID_LH LH ($PAID_WEI wei)"
else
  echo
  echo "[B] skipped (pass --spend to run the ~1-\$LH metered-call stage)"
fi

echo
echo "ALL MONEY-PATH SMOKE CHECKS PASSED"
echo "  stage A: net-zero escrow round-trip confirmed at $END_A wei"
[ "$SPEND" -eq 1 ] && echo "  stage B: metered call debited exactly $PAID_LH LH ($START_B -> $END_B wei)"
exit 0
