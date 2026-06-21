#!/usr/bin/env bash
# scripts/audit-tech-debt.sh — the HYGIENE gate (complements scripts/verify.sh,
# which proves BEHAVIOR). This answers "is the codebase clean?": all-targets/
# all-features warnings, clippy, doc-integrity drift, and the proxy typecheck —
# the checks that silently rot because the default `cargo check` only compiles the
# native build and never touches the proxy. Run it any time; it spends no gas and
# hits no network. See design/tech-debt-unused-code-report-2026-06-21.md.
set -uo pipefail
cd "$(dirname "$0")/.."

if [[ -t 1 ]]; then B='\033[1m'; G='\033[1;32m'; R='\033[1;31m'; N='\033[0m'; else B=''; G=''; R=''; N=''; fi
step() { printf "\n${B}== %s ==${N}\n" "$1"; }
fail() { printf "${R}FAIL:${N} %s\n" "$1"; exit 1; }

step "1/5 cargo check --all-targets --all-features (feature-gated code hides debt)"
out=$(cargo check --all-targets --all-features --message-format=short 2>&1) || { echo "$out"; fail "check errored"; }
if echo "$out" | grep -q "warning:"; then echo "$out" | grep "warning:"; fail "warnings in all-targets/all-features build"; fi
echo "  clean."

step "2/5 cargo clippy --all-targets --all-features -D warnings"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5 || fail "clippy found lints"

step "3/5 doc-integrity drift (gen-docs --check)"
cargo run --quiet --bin gen-docs --features wallet -- --check \
    || fail "doc drift: run 'cargo run --bin gen-docs --features wallet', commit, retry"

step "4/5 proxy typecheck (tsc --noEmit)"
if [[ -x proxy/node_modules/.bin/tsc ]]; then
    ( cd proxy && npm run --silent typecheck ) || fail "proxy typecheck failed"
else
    printf "  ${R}skipped${N} — run 'cd proxy && npm install' first.\n"
fi

step "5/5 un-reasoned broad allow() suppressions (informational)"
# A bare allow(dead_code|unused_imports|deprecated) with no trailing reason hides
# the warning signal. Not a hard gate yet — surfaced so the count trends down.
n=$(grep -rnE 'allow\((dead_code|unused_imports|deprecated)\)' src/ \
      | grep -vE '//.*(PARKED|reason|wasm|legacy|gated)' | wc -l | tr -d ' ')
echo "  $n bare allow(dead_code|unused_imports|deprecated) without a reason in src/"

printf "\n${G}TECH-DEBT AUDIT OK${N}\n"
