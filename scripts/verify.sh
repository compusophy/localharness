#!/usr/bin/env bash
# scripts/verify.sh — the proof-of-spec gate: ONE command that runs every proof
# we have, end to end, so nothing ships unverified. The release script calls it
# (scripts/release.sh); run it yourself any time for a full conformance check.
#
# Why this exists: `cargo test` NEVER instantiates wasm, so it cannot prove the
# browser app's cartridge runtime or host::compose actually work — that gap is
# how features shipped on "it compiles, therefore it's done". Stages 3-7 do REAL
# wasm instantiation + framebuffer assertions. A release must pass all seven.
#
#   1. native test suite             cargo test
#   2. wasm32 browser-app guardrail   cargo check (the live app must compile)
#   3. compile a real cartridge       rustlite .rl -> .wasm via the CLI
#   4. instantiate + run              validate-cartridge.js (catch runtime traps)
#   5. single-cartridge render        render-cartridge.js  (present-after-frame)
#   6. multi-module composition       render-compose.js    (host::compose isolation)
#   7. worker host-parity            test-worker-host-parity.mjs (the off-main-
#                                    thread cartridge worker's JS host re-impl
#                                    matches the Rust host — guards font/op drift)
set -euo pipefail
cd "$(dirname "$0")/.."

CART_SRC="${1:-bitmask.rl}"
mkdir -p target
CART_WASM="target/.verify-cartridge.wasm"
trap 'rm -f "$CART_WASM"' EXIT

if [[ -t 1 ]]; then B='\033[1m'; G='\033[1;32m'; N='\033[0m'; else B=''; G=''; N=''; fi
step() { printf "\n${B}== %s ==${N}\n" "$1"; }

step "1/7  native test suite"
cargo test --quiet

step "2/7  wasm32 browser-app guardrail"
cargo check --quiet --no-default-features --target wasm32-unknown-unknown --features browser-app

step "3/7  compile a real cartridge ($CART_SRC)"
cargo run --quiet --features wallet --bin localharness -- compile "$CART_SRC" "$CART_WASM"

step "4/7  instantiate + run (catch traps)"
node scripts/validate-cartridge.js "$CART_WASM"

step "5/7  single-cartridge render"
node scripts/render-cartridge.js "$CART_WASM"

step "6/7  multi-module composition (host::compose)"
node scripts/render-compose.js "$CART_WASM"

step "7/7  worker host-parity (off-main-thread cartridge runtime)"
node scripts/test-worker-host-parity.mjs

printf "\n${G}PROOF-OF-SPEC OK${N} — all 7 stages passed.\n"

# Opt-in extensions (NOT run here — both hit the LIVE testnet / proxy and spend
# real sponsor gas, so they must never gate this network-free proof). Run by hand:
#   scripts/verify-onchain.sh  proves the TRUST layer — a sponsored mint LANDS on-chain.
#   scripts/verify-e2e.sh      proves every shipped PLATFORM FLOW end to end
#                              (whoami / discover / call / mcp-call / schedule /
#                              invite / send) against the live chain + credit proxy,
#                              asserting each result via the CLI output or `cast call`.
#                              Self-cleaning + idempotent; tiny live $LH spend.
