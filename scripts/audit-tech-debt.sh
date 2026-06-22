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

step "1/7 cargo check --all-targets --all-features (feature-gated code hides debt)"
out=$(cargo check --all-targets --all-features --message-format=short 2>&1) || { echo "$out"; fail "check errored"; }
if echo "$out" | grep -q "warning:"; then echo "$out" | grep "warning:"; fail "warnings in all-targets/all-features build"; fi
echo "  clean."

step "2/7 cargo clippy --all-targets --all-features -D warnings"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5 || fail "clippy found lints"

step "3/7 doc-integrity drift (gen-docs --check)"
cargo run --quiet --bin gen-docs --features wallet -- --check \
    || fail "doc drift: run 'cargo run --bin gen-docs --features wallet', commit, retry"

step "4/7 proxy typecheck (tsc --noEmit)"
if [[ -x proxy/node_modules/.bin/tsc ]]; then
    ( cd proxy && npm run --silent typecheck ) || fail "proxy typecheck failed"
else
    printf "  ${R}skipped${N} — run 'cd proxy && npm install' first.\n"
fi

step "5/7 unused dependencies (cargo machete)"
# Unused crate deps are real trash (slower builds, bigger supply-chain surface).
# Build/link-level deps with no source `use` (e.g. getrandom_v04 for Burn's wasm
# backend) are ignore-listed in Cargo.toml [package.metadata.cargo-machete].
if command -v cargo-machete >/dev/null 2>&1; then
    cargo machete 2>&1 | tail -3 || fail "cargo machete found unused dependencies"
else
    printf "  ${R}skipped${N} — 'cargo install cargo-machete' to enable.\n"
fi

step "6/7 un-justified broad allow() suppressions (HARD GATE)"
# Every allow(dead_code|unused_imports|deprecated) must carry a justification: an
# inline `//` comment, an explanatory comment on the line ABOVE, or a cfg_attr
# conditional (self-documenting). A bare one re-hides the warning signal the way
# the old CLI `use crate::*` did — fail so it can't creep back in. (String
# literals and `#[cfg_attr(... allow ...)]` are excluded: the matcher only fires
# on a line that, trimmed, STARTS with `#[allow(...)]` / `#![allow(...)]`.)
bare=$(awk '
  FNR==1 { prev="" }
  {
    t=$0; sub(/^[ \t]+/,"",t)
    if (t ~ /^#!?\[allow\((dead_code|unused_imports|deprecated)\)\]/) {
      if (($0 !~ /\/\//) && (prev !~ /\/\//)) print FILENAME":"FNR
    }
    prev=$0
  }
' $(grep -rl "allow(" src --include=*.rs) 2>/dev/null)
if [[ -n "$bare" ]]; then
  echo "$bare"
  fail "un-justified allow() — add a one-line reason, cfg-gate it, or remove the dead code"
fi
echo "  none — every broad allow() carries a justification."

step "7/7 proxy .env.example currency (the original complaint)"
# .env.example going stale was the user's first report. Gate it: every real
# process.env.<NAME> used in proxy/api/ must be documented in proxy/.env.example.
# The 2+-char name regex naturally skips single-letter comment placeholders
# (e.g. `process.env.X` in _chain.ts's explanatory comment).
missing=""
for v in $(grep -rhoE "process\.env\.[A-Z][A-Z0-9_]+" proxy/api/ --include=*.ts \
            | sed -E 's/process\.env\.//' | sort -u); do
  grep -q "^$v=" proxy/.env.example || missing="$missing $v"
done
if [[ -n "$missing" ]]; then
  echo "  undocumented:$missing"
  fail "proxy env var(s) used in code but missing from proxy/.env.example — document them"
fi
echo "  all proxy env vars documented."

printf "\n${G}TECH-DEBT AUDIT OK${N}\n"
