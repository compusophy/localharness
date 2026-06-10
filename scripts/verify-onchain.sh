#!/usr/bin/env bash
# scripts/verify-onchain.sh — the TRUST-LAYER proof: the one stage scripts/verify.sh
# deliberately does NOT cover. verify.sh proves the wasm framebuffer renders; it
# never touches the chain. This proves a SPONSORED on-chain write actually LANDS.
#
# Why this is separate / opt-in: it hits the LIVE Tempo Moderato testnet and
# spends the embedded sponsor key's AlphaUSD gas — it mints a real (disposable)
# subdomain NFT. So it is NEVER part of the default `verify.sh` and must be run
# by hand.
#
# Why it exists: the entire bug history of this project is "local says ok, chain
# reverted silently" — a flat gas cap OOG-ing on submitFeedback / setMetadata /
# release while the CLI prints success. The ONLY real proof is an INDEPENDENT
# on-chain READ asserting the write landed. This script does exactly that:
#
#   1. derive a unique disposable name      qa-onchain-<unix-ts | $RANDOM | $1>
#   2. create it on-chain (sponsored mint)  cargo run … -- create <name>
#   3. ASSERT it registered, via a fresh    cargo run … -- whoami --json <name>
#      read-only RPC ("registered": true)   (independent of create's own check)
#   4. clean up — attempt                   cargo run … -- release <name> --confirm <name>
#      EXPECTED to refuse for a fresh name: `create` auto-sets the new name as
#      the fresh wallet's MAIN identity, and `release` REFUSES the caller's MAIN
#      (client-side exit 2 + the facet refuses on-chain too). The fallback logs
#      the leak and KEEPS the key file so future cleanup stays possible.
#
# A non-zero exit means the sponsored tx did NOT land on-chain (the OOG/revert
# case) — the failure mode this whole script exists to catch.
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ -t 1 ]]; then B='\033[1m'; G='\033[1;32m'; R='\033[1;31m'; N='\033[0m'; else B=''; G=''; R=''; N=''; fi
step() { printf "\n${B}== %s ==${N}\n" "$1"; }
fail() { printf "\n${R}ON-CHAIN PROOF FAILED${N} — %s\n" "$1" >&2; exit 1; }

# A unique, disposable throwaway name. Prefer an explicit arg ($1) for a
# repeatable run; else derive uniqueness from the unix timestamp + $RANDOM so
# back-to-back runs never collide on an already-taken name.
NAME="${1:-qa-onchain-$(date +%s)-${RANDOM}}"
CLI=(cargo run --quiet --features wallet --bin localharness --)

step "trust-layer proof against the live testnet (name: $NAME)"
printf "  this spends the embedded sponsor key's AlphaUSD gas (a real mint).\n"

step "1/3  create $NAME on-chain (sponsored Tempo tx)"
if ! "${CLI[@]}" create "$NAME"; then
  fail "the CLI 'create' returned non-zero — the sponsored mint did not complete (likely an out-of-gas/revert or RPC error)."
fi

step "2/3  ASSERT on-chain landing (independent read-only RPC)"
# whoami is pure read-only RPC — a SEPARATE round trip from create's own verify,
# so a 'registered: true' here is the real proof the tx landed (not just that the
# CLI printed ok before the chain reverted). --json gives a stable field to grep.
PROFILE="$("${CLI[@]}" whoami --json "$NAME")" || fail "the on-chain read (whoami) errored — could not confirm the write."
printf '%s\n' "$PROFILE"
if ! printf '%s' "$PROFILE" | grep -q '"registered": *true'; then
  fail "whoami reports $NAME is NOT registered on-chain — the sponsored create silently failed to land (the classic OOG/revert)."
fi

step "3/3  clean up the disposable name"
# `localharness release <name> --confirm <name>` exists (ReleaseFacet burn), so
# try it. CAVEAT: `create` just auto-set $NAME as this fresh wallet's MAIN
# identity, and `release` REFUSES the caller's MAIN (client-side exit 2; the
# facet also refuses on-chain), so for a brand-new disposable wallet this is
# EXPECTED to fall through to the leak-note below. It still cleans up when the
# name is NOT the key's MAIN (e.g. a rerun against a key with another MAIN).
KEY_FILE="${NAME}.localharness.key"
if "${CLI[@]}" release --as "$NAME" "$NAME" --confirm "$NAME"; then
  printf "  released '%s' on-chain — nothing leaked.\n" "$NAME"
else
  printf "  release refused (the name is this wallet's MAIN) — '%s' is LEFT\n" "$NAME"
  printf "  REGISTERED on-chain. KEEPING the key file so future cleanup stays\n"
  printf "  possible; it lives at one of:\n"
  printf "    %s/%s   (config home — current CLIs)\n" "${LOCALHARNESS_HOME:-$HOME/.localharness/keys}" "$KEY_FILE"
  printf "    ./%s   (cwd — older CLIs)\n" "$KEY_FILE"
  printf "  manual cleanup: re-point the wallet's MAIN, then\n"
  printf "    localharness release --as %s %s --confirm %s\n" "$NAME" "$NAME" "$NAME"
fi

printf "\n${G}ON-CHAIN PROOF OK${N} — sponsored create landed + verified on-chain (%s).\n" "$NAME"
