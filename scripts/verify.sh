#!/usr/bin/env bash
# scripts/verify.sh — the proof-of-spec gate: ONE command that runs every proof
# we have, end to end, so nothing ships unverified. The release script calls it
# (scripts/release.sh); run it yourself any time for a full conformance check.
#
# Why this exists: `cargo test` NEVER instantiates wasm, so it cannot prove the
# browser app's cartridge runtime or host::compose actually work — that gap is
# how features shipped on "it compiles, therefore it's done". Stages 3-10 do REAL
# wasm instantiation + framebuffer assertions. A release must pass all ten.
#
#   1. native test suites            cargo test for EVERY feature config that
#                                    carries tests: default, anthropic, wallet
#                                    (wallet alone holds the 111 CLI tests)
#   2. wasm32 guardrails             cargo check x3: bare SDK, wallet, browser-app
#                                    (gated modules don't trip the bare check)
#   3. compile a real cartridge       rustlite .rl -> .wasm via the CLI
#   4. instantiate + run              validate-cartridge.js (catch runtime traps)
#   5. single-cartridge render        render-cartridge.js  (present-after-frame)
#   6. multi-module composition       render-compose.js    (host::compose isolation)
#   7. worker host-parity            test-worker-host-parity.mjs (the off-main-
#                                    thread cartridge worker's JS host re-impl
#                                    matches the Rust host — guards font/op drift)
#   8. cartridge corpus              test-cartridges.mjs (compile -> instantiate
#                                    -> run -> assert each examples/cartridges/*.rl;
#                                    the codegen regression gate — proves complex
#                                    cartridges produce valid, non-trapping wasm
#                                    that draws/computes the right answer)
#   9. variable resolution           test-variable-resolution.mjs (a dims()
#                                    cartridge resizes the worker framebuffer;
#                                    a no-dims() cartridge stays 256×144;
#                                    clamp range [16,1024] enforced)
#  10. compose wiring               test-compose-wiring.mjs (host::compose: a
#                                    parent composites a child cartridge into a
#                                    sub-rect through the worker host — scaled +
#                                    isolated blit, focus-gated pointer routing,
#                                    blitChild/mapPointer JS<->Rust parity, the
#                                    ComposeBudget child-count cap)
set -euo pipefail
cd "$(dirname "$0")/.."

CART_SRC="${1:-bitmask.rl}"
mkdir -p target
CART_WASM="target/.verify-cartridge.wasm"
trap 'rm -f "$CART_WASM"' EXIT

if [[ -t 1 ]]; then B='\033[1m'; G='\033[1;32m'; N='\033[0m'; else B=''; G=''; N=''; fi
step() { printf "\n${B}== %s ==${N}\n" "$1"; }

step "1/10 native test suites (default + anthropic + wallet)"
cargo test --quiet
cargo test --quiet --features anthropic
cargo test --quiet --features wallet

step "2/10 wasm32 guardrails (bare SDK + wallet + browser-app)"
cargo check --quiet --no-default-features --target wasm32-unknown-unknown
cargo check --quiet --no-default-features --features wallet --target wasm32-unknown-unknown
cargo check --quiet --no-default-features --target wasm32-unknown-unknown --features browser-app

step "3/10 compile a real cartridge ($CART_SRC)"
cargo run --quiet --features wallet --bin localharness -- compile "$CART_SRC" "$CART_WASM"

step "4/10 instantiate + run (catch traps)"
node scripts/validate-cartridge.js "$CART_WASM"

step "5/10 single-cartridge render"
node scripts/render-cartridge.js "$CART_WASM"

step "6/10 multi-module composition (host::compose)"
node scripts/render-compose.js "$CART_WASM"

step "7/10 worker host-parity (off-main-thread cartridge runtime)"
node scripts/test-worker-host-parity.mjs

step "8/10 cartridge corpus (compile -> instantiate -> run -> assert)"
node scripts/test-cartridges.mjs

step "9/10 variable framebuffer resolution (dims() convention)"
node scripts/test-variable-resolution.mjs

step "10/10 cartridge-in-cartridge composition (host::compose wiring)"
node scripts/test-compose-wiring.mjs

printf "\n${G}PROOF-OF-SPEC OK${N} — all 10 stages passed.\n"

# Opt-in extensions (NOT run here — both hit the LIVE testnet / proxy and spend
# real sponsor gas, so they must never gate this network-free proof). Run by hand:
#   scripts/verify-onchain.sh  proves the TRUST layer — a sponsored mint LANDS on-chain.
#   scripts/verify-e2e.sh      proves every shipped PLATFORM FLOW end to end
#                              (whoami / discover / call / mcp-call / schedule /
#                              invite / send) against the live chain + credit proxy,
#                              asserting each result via the CLI output or `cast call`.
#                              Self-cleaning + idempotent; tiny live $LH spend.
