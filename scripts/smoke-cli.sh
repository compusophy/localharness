#!/usr/bin/env bash
# Fast, OFFLINE smoke test of the `localharness` CLI — run before a release.
#
# Verifies the binary builds and its core, network-free commands behave: help,
# version, unknown-command exit code, and the cartridge compile gate (valid
# cartridge compiles; an entry-less one is rejected). No network, no on-chain
# writes, no $LH — safe to run anytime. The live commands (create/call/publish/
# whoami/list) need an identity + chain and are intentionally NOT exercised here.
set -euo pipefail
cd "$(dirname "$0")/.."

BIN=(cargo run -q --features wallet --bin localharness --)
pass() { echo "  ok — $1"; }
fail() { echo "  FAIL — $1"; exit 1; }

echo "[1/5] --version prints a version"
"${BIN[@]}" --version | grep -qE 'localharness [0-9]+\.[0-9]+\.[0-9]+' && pass "version" || fail "version"

echo "[2/5] help lists the create command"
"${BIN[@]}" help | grep -q 'localharness create' && pass "help" || fail "help"

echo "[3/5] unknown command exits non-zero"
if "${BIN[@]}" definitely-not-a-command >/dev/null 2>&1; then fail "unknown cmd exited 0"; else pass "unknown cmd rejected"; fi

echo "[4/5] a valid cartridge compiles"
"${BIN[@]}" compile bitmask.rl >/dev/null && pass "compile bitmask.rl" || fail "compile bitmask.rl"

echo "[5/5] an entry-less cartridge is rejected"
tmp="$(mktemp)"; printf 'fn helper(n: i32) -> i32 { n + 1 }\n' > "$tmp"
if "${BIN[@]}" compile "$tmp" >/dev/null 2>&1; then rm -f "$tmp"; fail "no-entry cartridge accepted"; else rm -f "$tmp"; pass "no-entry rejected"; fi

echo "ALL SMOKE CHECKS PASSED"
