#!/usr/bin/env bash
# scripts/add-redeem-codes.sh — owner tool to mint tiered $LH redeem codes.
#
# The $LH credit-funding model: daily-allowance claiming is being disabled as a
# sybil risk; redeem codes + agent-to-agent `send_lh` are the controlled funding
# paths until Tempo mainnet + Stripe. This script generates a batch of one-time
# redeem codes worth a fixed $LH denomination and registers their HASHES on-chain
# via RedeemFacet.addRedeemCodes(bytes32[],uint256). The chain only ever sees the
# keccak hashes — the plaintext codes are SECRETS you distribute off-chain (DMs,
# invites). Whoever holds a plaintext code can redeem it once for its $LH amount.
#
# CRITICAL — hashing must match the contract. RedeemFacet.redeem(string code)
# (contracts/src/facets/RedeemFacet.sol:72) computes:
#     bytes32 h = keccak256(bytes(code));
# i.e. the keccak256 of the raw UTF-8 bytes of the code string. `cast keccak
# "$code"` hashes exactly those bytes, so a code generated here round-trips: the
# hash we register equals what redeem() recomputes from the plaintext. (Restrict
# codes to ASCII so "bytes(code)" == the bytes cast hashes — we do, below.)
#
# Owner-only (EIP-173): addRedeemCodes needs the diamond-owner key (EVM_PRIVATE_KEY
# in ./.env). Requires `cast` (foundry) + `openssl` for CSPRNG randomness.
#
# Usage:
#   scripts/add-redeem-codes.sh <amount_lh> <count> [--send]
#   scripts/add-redeem-codes.sh --help
#
#   <amount_lh>  $LH each generated code is worth (integer or decimal; >0).
#   <count>      how many codes to generate (1–100).
#   --send       BROADCAST the addRedeemCodes tx. DEFAULT is DRY-RUN: print the
#                codes, hashes, wei amount, and the exact `cast send` it WOULD run,
#                writing the plaintext file but sending NOTHING.
#
# Env overrides: DIAMOND, RPC, EVM_PRIVATE_KEY.
set -euo pipefail
cd "$(dirname "$0")/.."

DIAMOND="${DIAMOND:-0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c}"
RPC="${RPC:-https://rpc.moderato.tempo.xyz}"

usage() {
  sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'
}

# --- arg parse ------------------------------------------------------------
SEND=0
POSITIONAL=()
for arg in "$@"; do
  case "$arg" in
    --help|-h) usage; exit 0 ;;
    --send)    SEND=1 ;;
    -*)        echo "add-redeem-codes: unknown flag: $arg" >&2; exit 1 ;;
    *)         POSITIONAL+=("$arg") ;;
  esac
done

if [ "${#POSITIONAL[@]}" -ne 2 ]; then
  echo "add-redeem-codes: need <amount_lh> <count>  (try --help)" >&2
  exit 1
fi
AMOUNT_LH="${POSITIONAL[0]}"
COUNT="${POSITIONAL[1]}"

# --- validation -----------------------------------------------------------
command -v cast    >/dev/null 2>&1 || { echo "add-redeem-codes: needs cast (foundry)" >&2; exit 1; }
command -v openssl >/dev/null 2>&1 || { echo "add-redeem-codes: needs openssl (CSPRNG)" >&2; exit 1; }

# amount: positive number (integer or decimal). Reject 0 / negatives / junk.
case "$AMOUNT_LH" in
  ''|*[!0-9.]*|.) echo "add-redeem-codes: amount_lh must be a positive number, got: $AMOUNT_LH" >&2; exit 1 ;;
esac
# >0 test via awk (handles decimals).
if ! awk -v a="$AMOUNT_LH" 'BEGIN{ exit (a+0 > 0) ? 0 : 1 }'; then
  echo "add-redeem-codes: amount_lh must be > 0, got: $AMOUNT_LH" >&2; exit 1
fi

# count: integer in 1..100.
case "$COUNT" in
  ''|*[!0-9]*) echo "add-redeem-codes: count must be an integer, got: $COUNT" >&2; exit 1 ;;
esac
if [ "$COUNT" -lt 1 ] || [ "$COUNT" -gt 100 ]; then
  echo "add-redeem-codes: count must be in 1..100, got: $COUNT" >&2; exit 1
fi

# key only needed to actually broadcast. The `|| true` keeps a missing/empty
# .env from aborting under `set -e` (dry-run needs no key).
KEY="${EVM_PRIVATE_KEY:-$(grep -E '^EVM_PRIVATE_KEY=' .env 2>/dev/null | head -1 | cut -d= -f2- | tr -d "\"' " || true)}"
if [ "$SEND" -eq 1 ] && [ -z "$KEY" ]; then
  echo "add-redeem-codes: --send needs the diamond-owner key (EVM_PRIVATE_KEY in ./.env)" >&2
  exit 1
fi

# --- wei conversion -------------------------------------------------------
# $LH is 18-decimal; cast --to-wei treats "ether" as 10^18.
AMOUNT_WEI="$(cast --to-wei "$AMOUNT_LH" ether)"
AMOUNT_WEI="${AMOUNT_WEI%% *}"  # strip any trailing units cast may append

# --- generate codes + hashes ---------------------------------------------
# Code shape: lh-<amount>-<10 url-safe random chars>. Randomness from openssl
# (CSPRNG), mapped to [A-Za-z0-9] so the code is ASCII (=> bytes(code) == the
# bytes cast keccak hashes) and human-distributable. NOT $RANDOM.
gen_token() {
  # Pull plenty of base64 entropy, keep only [A-Za-z0-9], take 10 chars.
  openssl rand -base64 48 | tr -dc 'A-Za-z0-9' | head -c 10
}

CODES=()
HASHES=()
i=0
while [ "$i" -lt "$COUNT" ]; do
  tok="$(gen_token)"
  # Guard against a short token if entropy filtering trimmed too much.
  [ "${#tok}" -eq 10 ] || continue
  code="lh-${AMOUNT_LH}-${tok}"
  hash="$(cast keccak "$code")"   # == keccak256(bytes(code)), matches redeem()
  CODES+=("$code")
  HASHES+=("$hash")
  i=$((i + 1))
done

# Build the [h1,h2,...] array literal cast expects for a bytes32[] arg.
ARR="$(IFS=,; echo "${HASHES[*]}")"
ARR="[$ARR]"

# --- write plaintext file (gitignored) -----------------------------------
TS="$(date -u +%Y%m%d-%H%M%S)"
OUT="redeem-codes-${AMOUNT_LH}lh-${TS}.txt"
{
  echo "# \$LH redeem codes — ${AMOUNT_LH} \$LH each (${AMOUNT_WEI} wei)"
  echo "# generated $(date -u +%Y-%m-%dT%H:%M:%SZ) — DISTRIBUTE OFF-CHAIN, these are SECRETS"
  echo "# diamond=$DIAMOND  count=$COUNT"
  echo "#"
  echo "# code<TAB>keccak256(bytes(code))"
  j=0
  while [ "$j" -lt "${#CODES[@]}" ]; do
    printf '%s\t%s\n' "${CODES[$j]}" "${HASHES[$j]}"
    j=$((j + 1))
  done
} > "$OUT"
chmod 600 "$OUT" 2>/dev/null || true

# --- report ---------------------------------------------------------------
echo "generated $COUNT redeem code(s), ${AMOUNT_LH} \$LH each = ${AMOUNT_WEI} wei:"
echo
j=0
while [ "$j" -lt "${#CODES[@]}" ]; do
  printf '  %-24s %s\n' "${CODES[$j]}" "${HASHES[$j]}"
  j=$((j + 1))
done
echo
echo "plaintext written to: $OUT  (gitignored — these codes are secrets)"
echo
echo "on-chain hashing: redeem(code) computes keccak256(bytes(code))"
echo "  (RedeemFacet.sol:72) — each hash above is cast keccak \"<code>\", identical."
echo

if [ "$SEND" -eq 1 ]; then
  echo "broadcasting addRedeemCodes (owner-only)…"
  cast send "$DIAMOND" "addRedeemCodes(bytes32[],uint256)" "$ARR" "$AMOUNT_WEI" \
    --private-key "$KEY" --rpc-url "$RPC" >/dev/null
  echo "done — $COUNT code(s) now redeemable for ${AMOUNT_LH} \$LH each."
else
  echo "DRY-RUN (default) — nothing broadcast. Re-run with --send to register on-chain."
  echo "would run:"
  printf "  cast send %s 'addRedeemCodes(bytes32[],uint256)' '%s' %s --private-key <EVM_PRIVATE_KEY> --rpc-url %s\n" \
    "$DIAMOND" "$ARR" "$AMOUNT_WEI" "$RPC"
fi
