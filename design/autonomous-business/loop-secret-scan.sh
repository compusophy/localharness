#!/bin/sh
# loop-secret-scan.sh — the autonomous-business loop's pre-commit secret gate.
#
# Scans the given paths for token/key/secret patterns and EXITS NON-ZERO on any
# hit, so a loop tick can gate its commit on it (LOOP-PROTOCOL.md §5.2). It is the
# enforceable backstop behind "no git add -A / never commit secrets" (RISKS.md
# b.3/d.5): even if a credential reaches staging, this refuses the commit.
#
# Detects: sk- LLM keys, GitHub tokens (ghp_/gho_/ghu_/ghs_/ghr_/github_pat_),
# AWS access-key ids (AKIA…), PEM PRIVATE KEY blocks, raw 0x 64-hex
# private-keys/seeds, Slack tokens (xox?-…), and .env-style assignments of
# secret-named vars to a non-placeholder value.
#
# Self-contained POSIX sh — no bashisms, no pipefail, no external tools beyond
# git + grep. Conventional style matches scripts/ (set -u, step/fail helpers).
#
# Usage:
#   sh loop-secret-scan.sh [path ...]
#     no args  → scans STAGED files under design/autonomous-business/ + src/
#                (the loop's own write surface), reading the STAGED blob content.
#     path...  → scans those files/dirs directly (working tree).
#
# False positives: a line that legitimately contains one of these patterns (a
# committed template, a test vector, a docs example) may carry the inline pragma
#   secret-scan:allow
# anywhere on the line to exempt THAT line. Placeholder values (__foo__, <foo>,
# ${foo}, your_…, changeme, example, REDACTED, …) are auto-exempt for the .env rule
# so committed *.template / .env.example files pass cleanly.
#
# Exit: 0 = clean · 1 = secret(s) found · 2 = usage/setup error.

set -u

# ---- presentation (TTY-aware, like scripts/audit-tech-debt.sh) ----
if [ -t 1 ]; then B='\033[1m'; G='\033[1;32m'; R='\033[1;31m'; N='\033[0m'; else B=''; G=''; R=''; N=''; fi
say()  { printf "%b\n" "$*"; }
fail() { printf "${R}SECRET-SCAN FAIL:${N} %s\n" "$1" >&2; }

SELF_BASE=$(basename "$0")

# ---- pattern bank (POSIX ERE) -------------------------------------------------
# High-confidence credential shapes. Prefix-only mentions (e.g. the word "AKIA"
# in prose) do NOT match — each requires its full token body, so this very file
# and LOOP-PROTOCOL.md scan clean.
SECRET_RE='sk-(ant-|proj-|live-)?[A-Za-z0-9_-]{20,}'
SECRET_RE="$SECRET_RE"'|(ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36,}'
SECRET_RE="$SECRET_RE"'|github_pat_[A-Za-z0-9_]{22,}'
SECRET_RE="$SECRET_RE"'|AKIA[0-9A-Z]{16}'
SECRET_RE="$SECRET_RE"'|xox[baprs]-[A-Za-z0-9-]{10,}'
SECRET_RE="$SECRET_RE"'|-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----'
# raw 0x 64-hex (private key / seed) — bounded so a 128-hex blob or an address
# (40-hex) does not match; only an isolated 64-hex run.
SECRET_RE="$SECRET_RE"'|(^|[^0-9a-fA-Fx])0x[0-9a-fA-F]{64}([^0-9a-fA-F]|$)'

# .env-style assignment: a secret-NAMED var set to some value.
ENV_RE='(API_?KEY|SECRET|TOKEN|PASSWORD|PASSWD|PRIVATE_?KEY|ACCESS_?KEY|ACCESS_?TOKEN|CLIENT_SECRET|MNEMONIC|SEED_?PHRASE|BEARER)[A-Z0-9_]*[[:space:]]*[:=][[:space:]]*[^[:space:]]'
# …unless the value is an obvious placeholder (keeps templates/.env.example clean).
PLACEHOLDER_RE='__[A-Za-z0-9_]+__|<[^>]+>|\$\{?[A-Za-z_]|your[-_]|change[-_]?me|placeholder|example|redacted|optional|^[A-Za-z_][A-Za-z0-9_]*[[:space:]]*[:=][[:space:]]*(""|'"''"'|x{3,})$'

ALLOW_PRAGMA='secret-scan:allow'

# ---- build the target list ----------------------------------------------------
# Each target is "displayname<TAB>scanpath". In staged mode scanpath is a temp
# file holding the staged blob; in path mode scanpath == displayname.
TMPDIR_SCAN=$(mktemp -d 2>/dev/null) || { fail "cannot mktemp"; exit 2; }
trap 'rm -rf "$TMPDIR_SCAN"' EXIT INT TERM
TARGETS="$TMPDIR_SCAN/targets"; : > "$TARGETS"
i=0

add_path() { # $1 = file on disk (display == scan)
  printf '%s\t%s\n' "$1" "$1" >> "$TARGETS"
}
add_staged() { # $1 = staged repo path → snapshot blob to a temp file
  i=$((i + 1)); blob="$TMPDIR_SCAN/blob.$i"
  if git show ":$1" > "$blob" 2>/dev/null; then
    printf '%s\t%s\n' "$1" "$blob" >> "$TARGETS"
  fi
}

if [ "$#" -gt 0 ]; then
  for arg in "$@"; do
    if [ -d "$arg" ]; then
      find "$arg" -type f | while IFS= read -r f; do add_path "$f"; done
    elif [ -f "$arg" ]; then
      add_path "$arg"
    else
      say "${B}skip${N} (not found): $arg" >&2
    fi
  done
else
  git rev-parse --is-inside-work-tree >/dev/null 2>&1 || { fail "no args and not in a git repo"; exit 2; }
  staged=$(git diff --cached --name-only --diff-filter=ACMR -- design/autonomous-business src 2>/dev/null)
  [ -n "$staged" ] || { say "${G}no staged files under design/autonomous-business or src — nothing to scan.${N}"; exit 0; }
  printf '%s\n' "$staged" | while IFS= read -r f; do
    [ -n "$f" ] && add_staged "$f"
  done
fi

[ -s "$TARGETS" ] || { say "${G}no scannable files — clean.${N}"; exit 0; }

# ---- scan ---------------------------------------------------------------------
hits=0
while IFS=$(printf '\t') read -r display scan; do
  [ -f "$scan" ] || continue
  case "$display" in */"$SELF_BASE"|"$SELF_BASE") continue ;; esac   # never flag self
  grep -Iq . "$scan" 2>/dev/null || continue                         # skip binary/empty

  # class 1: high-confidence secret shapes
  found=$(grep -nE "$SECRET_RE" "$scan" 2>/dev/null | grep -v "$ALLOW_PRAGMA")
  # class 2: secret-named env assignment with a real (non-placeholder) value
  envf=$(grep -nE "$ENV_RE" "$scan" 2>/dev/null | grep -vEi "$PLACEHOLDER_RE" | grep -v "$ALLOW_PRAGMA")

  if [ -n "$found" ] || [ -n "$envf" ]; then
    hits=$((hits + 1))
    fail "$display"
    [ -n "$found" ] && printf '%s\n' "$found" | sed 's/^/    [key]  /' >&2
    [ -n "$envf" ]  && printf '%s\n' "$envf"  | sed 's/^/    [env]  /' >&2
  fi
done < "$TARGETS"

# NB: the while-loop ran in this shell (input redirected from a file, not a pipe),
# so $hits survives. Report and exit on it.
if [ "$hits" -gt 0 ]; then
  printf "${R}==> %s file(s) contain secret-like content — COMMIT BLOCKED.${N}\n" "$hits" >&2
  printf "    Remove the secret, or add the inline pragma '%s' to a verified-safe line.\n" "$ALLOW_PRAGMA" >&2
  exit 1
fi
say "${G}SECRET-SCAN OK — no token/key/secret patterns found.${N}"
exit 0
