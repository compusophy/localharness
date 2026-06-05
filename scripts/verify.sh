#!/usr/bin/env bash
# scripts/verify.sh — the proof-of-spec gate: ONE command that runs every proof
# we have, end to end, so nothing ships unverified. The release script calls it
# (scripts/release.sh); run it yourself any time for a full conformance check.
#
# Why this exists: `cargo test` NEVER instantiates wasm, so it cannot prove the
# browser app's cartridge runtime or host::compose actually work — that gap is
# how features shipped on "it compiles, therefore it's done". Stages 3-6 do REAL
# wasm instantiation + framebuffer assertions. A release must pass all six.
#
#   1. native test suite             cargo test
#   2. wasm32 browser-app guardrail   cargo check (the live app must compile)
#   3. compile a real cartridge       rustlite .rl -> .wasm via the CLI
#   4. instantiate + run              validate-cartridge.js (catch runtime traps)
#   5. single-cartridge render        render-cartridge.js  (present-after-frame)
#   6. multi-module composition       render-compose.js    (host::compose isolation)
set -euo pipefail
cd "$(dirname "$0")/.."

CART_SRC="${1:-bitmask.rl}"
mkdir -p target
CART_WASM="target/.verify-cartridge.wasm"
trap 'rm -f "$CART_WASM"' EXIT

if [[ -t 1 ]]; then B='\033[1m'; G='\033[1;32m'; N='\033[0m'; else B=''; G=''; N=''; fi
step() { printf "\n${B}== %s ==${N}\n" "$1"; }

step "1/6  native test suite"
cargo test --quiet

step "2/6  wasm32 browser-app guardrail"
cargo check --quiet --no-default-features --target wasm32-unknown-unknown --features browser-app

step "3/6  compile a real cartridge ($CART_SRC)"
cargo run --quiet --features wallet --bin localharness -- compile "$CART_SRC" "$CART_WASM"

step "4/6  instantiate + run (catch traps)"
node scripts/validate-cartridge.js "$CART_WASM"

step "5/6  single-cartridge render"
node scripts/render-cartridge.js "$CART_WASM"

step "6/6  multi-module composition (host::compose)"
node scripts/render-compose.js "$CART_WASM"

printf "\n${G}PROOF-OF-SPEC OK${N} — all 6 stages passed.\n"

# Opt-in extension: scripts/verify-onchain.sh proves the TRUST / on-chain layer
# (a sponsored mint actually lands on-chain). NOT run here — it spends live
# testnet sponsor gas. Run it by hand when you need that proof.
