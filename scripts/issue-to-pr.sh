#!/usr/bin/env bash
# scripts/issue-to-pr.sh — colony rung-2: turn a GitHub issue into a
# VERIFY-GATED pull request. The SAFE HARNESS around an (intentionally
# pluggable) fixer.
#
# THE POINT — and the whole value — is the IMMUNE SYSTEM: a PR is opened ONLY
# IF the project's proof-of-spec gate (scripts/verify.sh) passes on the change.
# That gate (native tests + the wasm32 browser-app guardrail + real cartridge
# instantiate/render/compose) is what makes an autonomous / agent-authored
# change trustworthy enough to propose. No green gate -> no PR. Ever.
#
# HONEST SCOPE: this does NOT fix issues. The fix-GENERATION is a pluggable hook
# ($FIX_CMD) — a one-shot agent can't reliably patch arbitrary issues, so we do
# not pretend it can. What this script guarantees is the trustworthy MECHANISM
# around whatever fixer you wire in:
#   - never opens an EMPTY PR (no fixer wired, or the fixer changed nothing),
#   - never opens a PR on RED (the verify gate must pass first),
#   - states in the PR body that the gate ran, so a reviewer sees the check.
#
# Flow:
#   1. gh issue view <n>        -> read title/body (fail clearly if absent)
#   2. fresh branch fix/issue-<n> off an up-to-date main (fail if dirty)
#   3. $FIX_CMD applies the fix  (ISSUE_NUMBER/ISSUE_TITLE/ISSUE_BODY in env)
#   4. git diff --quiet?         -> abort + clean up (no empty PR)
#   5. bash scripts/verify.sh    -> THE GATE. red => no PR, exit non-zero
#   6. commit + push + gh pr create (body: Closes #<n>, diffstat, "gate passed")
#
# THE PLUGGABLE-FIXER CONTRACT ($FIX_CMD):
#   $FIX_CMD runs once, from the repo root, on the fresh fix/issue-<n> branch.
#   The issue is exposed to it as environment variables:
#       ISSUE_NUMBER   the issue number
#       ISSUE_TITLE    the issue title
#       ISSUE_BODY     the full issue body (may be multi-line / empty)
#   The fixer's job: edit the working tree to address the issue. It MUST exit 0
#   on success. It MUST NOT commit, push, or open a PR — this harness owns the
#   git + verify + PR machinery (so the gate can't be bypassed). If it makes no
#   changes, the harness aborts cleanly (step 4) rather than open an empty PR.
#   This is exactly where an agent plugs in, e.g.:
#       FIX_CMD='claude -p "fix issue #$ISSUE_NUMBER: $ISSUE_TITLE"' \
#         ./scripts/issue-to-pr.sh 42
#       FIX_CMD='localharness call claude "fix #$ISSUE_NUMBER"' \
#         ./scripts/issue-to-pr.sh 42
#       FIX_CMD='./my-patch-script.sh' ./scripts/issue-to-pr.sh 42   # human patch
#   If $FIX_CMD is unset the harness prints that no fixer is wired and exits
#   WITHOUT opening anything.
#
# Usage:
#   FIX_CMD='<command>' ./scripts/issue-to-pr.sh <issue-number> [flags]
#   ./scripts/issue-to-pr.sh --help
#
# Flags:
#   --dry-run        do everything EXCEPT push + gh pr create (still runs the
#                    fixer and the FULL verify gate); prints what it WOULD do.
#   --keep-branch    on gate failure, leave the branch checked out for
#                    inspection (this is the default).
#   --clean-on-fail  on gate failure, switch back to the base branch and delete
#                    the fix branch.
#   --base <ref>     base branch to branch off / target the PR (default: main).
#   -h, --help       this help.
#
# Env:
#   FIX_CMD          the pluggable fixer (see contract above). Required unless
#                    --dry-run is given, in which case a no-op sample fixer that
#                    touches a tracked sentinel file is used so the harness is
#                    testable end-to-end WITHOUT a real fixer or a real PR.
#   REPO             GitHub repo (default: compusophy/localharness).
#
# Conventions match scripts/*.sh: bash, set -euo pipefail, repo-root cwd.
set -euo pipefail
cd "$(dirname "$0")/.."

REPO="${REPO:-compusophy/localharness}"

# ---- pretty output (bold/green only on a tty; same idiom as verify.sh) -------
if [[ -t 1 ]]; then B='\033[1m'; G='\033[1;32m'; R='\033[1;31m'; Y='\033[1;33m'; N='\033[0m'
else B=''; G=''; R=''; Y=''; N=''; fi
say()  { printf "${B}%s${N}\n" "$*"; }
ok()   { printf "${G}%s${N}\n" "$*"; }
warn() { printf "${Y}%s${N}\n" "$*" >&2; }
die()  { printf "${R}error:${N} %s\n" "$*" >&2; exit 1; }

usage() {
  # Print the leading comment block: every line after the shebang up to (but
  # not including) the first line that doesn't start with '#', with the leading
  # '# ' / '#' stripped.
  while IFS= read -r line; do
    case "$line" in
      '#!'*) continue ;;          # shebang
      '#') printf '\n' ;;         # bare comment line -> blank
      '# '*) printf '%s\n' "${line#'# '}" ;;
      '#'*) printf '%s\n' "${line#'#'}" ;;
      *) break ;;                 # first non-comment line ends the block
    esac
  done < "$0"
}

# ---- arg parse ---------------------------------------------------------------
DRY_RUN=0
CLEAN_ON_FAIL=0
BASE="main"
ISSUE=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help)      usage; exit 0 ;;
    --dry-run)      DRY_RUN=1; shift ;;
    --keep-branch)  CLEAN_ON_FAIL=0; shift ;;
    --clean-on-fail) CLEAN_ON_FAIL=1; shift ;;
    --base)         BASE="${2:-}"; [[ -n "$BASE" ]] || die "--base needs a ref"; shift 2 ;;
    --base=*)       BASE="${1#*=}"; shift ;;
    -*)             die "unknown flag: $1 (try --help)" ;;
    *)
      [[ -z "$ISSUE" ]] || die "unexpected extra argument: $1"
      ISSUE="$1"; shift ;;
  esac
done

[[ -n "$ISSUE" ]] || { usage; die "missing <issue-number>"; }
[[ "$ISSUE" =~ ^[0-9]+$ ]] || die "issue number must be a positive integer, got: $ISSUE"

BRANCH="fix/issue-${ISSUE}"

# ---- preflight: environment is sane ------------------------------------------
command -v git >/dev/null 2>&1 || die "git not found on PATH"
command -v gh  >/dev/null 2>&1 || die "the GitHub CLI (gh) is required but not on PATH — https://cli.github.com"
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || die "not inside a git repository"

# Detached HEAD has no current branch to return to — refuse so we never strand
# the user on a dangling commit.
if ! git symbolic-ref -q HEAD >/dev/null; then
  die "HEAD is detached — check out a branch (e.g. '$BASE') before running"
fi
START_BRANCH="$(git rev-parse --abbrev-ref HEAD)"

# Clean tree required: the fixer's diff must be the ONLY diff, so we can detect
# "no change" and produce an honest, minimal PR.
if ! git diff --quiet || ! git diff --cached --quiet; then
  die "working tree is dirty — commit or stash your changes first"
fi
if [[ -n "$(git ls-files --others --exclude-standard)" ]]; then
  die "untracked files present — clean them or add to .gitignore first (would muddy the fix diff)"
fi

# ---- step 1: read the issue --------------------------------------------------
say "== 1/6  read issue #$ISSUE ($REPO) =="
# Confirm the issue exists (and gh is authed) with one round-trip, then pull
# each field via gh's bundled --jq (no separate jq dependency).
gh issue view "$ISSUE" --repo "$REPO" --json number >/dev/null 2>&1 \
  || die "issue #$ISSUE not found in $REPO (or gh is unauthenticated — try 'gh auth status')"

ISSUE_TITLE="$(gh issue view "$ISSUE" --repo "$REPO" --json title --jq .title)"
ISSUE_BODY="$(gh issue view "$ISSUE" --repo "$REPO" --json body  --jq .body)"
ISSUE_STATE="$(gh issue view "$ISSUE" --repo "$REPO" --json state --jq .state)"
ISSUE_URL="$(gh issue view "$ISSUE" --repo "$REPO" --json url   --jq .url)"
export ISSUE_NUMBER="$ISSUE" ISSUE_TITLE ISSUE_BODY

printf '  title: %s\n' "$ISSUE_TITLE"
printf '  state: %s\n' "$ISSUE_STATE"
printf '  url:   %s\n' "$ISSUE_URL"
[[ "$ISSUE_STATE" == "OPEN" ]] || warn "issue #$ISSUE is $ISSUE_STATE (proceeding anyway)"

# ---- step 2: fresh branch off up-to-date base --------------------------------
say "== 2/6  branch $BRANCH off $BASE =="
# Refresh the base ref from the remote (best-effort; offline -> keep local).
if git remote get-url origin >/dev/null 2>&1; then
  git fetch --quiet origin "$BASE" 2>/dev/null \
    && say "  fetched origin/$BASE" \
    || warn "  could not fetch origin/$BASE — using local $BASE"
fi

# Pick the freshest available base: origin/<base> if we just fetched it, else
# the local branch.
if git rev-parse --verify --quiet "origin/$BASE" >/dev/null; then
  BASE_REF="origin/$BASE"
elif git rev-parse --verify --quiet "$BASE" >/dev/null; then
  BASE_REF="$BASE"
else
  die "base ref '$BASE' not found locally or on origin"
fi
say "  base ref: $BASE_REF ($(git rev-parse --short "$BASE_REF"))"

# Existing branch name: suffix with -2, -3, ... rather than clobber.
if git rev-parse --verify --quiet "refs/heads/$BRANCH" >/dev/null; then
  warn "  branch $BRANCH already exists"
  n=2
  while git rev-parse --verify --quiet "refs/heads/${BRANCH}-${n}" >/dev/null; do n=$((n+1)); done
  BRANCH="${BRANCH}-${n}"
  warn "  using $BRANCH instead"
fi

git switch --quiet --create "$BRANCH" "$BASE_REF"
ok "  created + switched to $BRANCH"

# From here on, restore the user's starting branch on any unexpected exit.
# (Normal success paths set BRANCH_KEPT=1 so we DON'T discard a finished branch.)
BRANCH_KEPT=0
restore_branch() {
  [[ "$BRANCH_KEPT" -eq 1 ]] && return 0
  # Best-effort: discard partial work and go home.
  git reset --hard --quiet 2>/dev/null || true
  git switch --quiet "$START_BRANCH" 2>/dev/null || true
  git branch -D "$BRANCH" --quiet 2>/dev/null || true
}
trap restore_branch EXIT

# ---- step 3: pluggable fixer -------------------------------------------------
say "== 3/6  apply fix via \$FIX_CMD =="
SAMPLE_FIXER=0
if [[ -z "${FIX_CMD:-}" ]]; then
  if [[ "$DRY_RUN" -eq 1 ]]; then
    # No real fixer + --dry-run => use a harmless sample fixer so the harness is
    # exercisable end-to-end (reaches the gate) without a real fix or a real PR.
    SAMPLE_FIXER=1
    FIX_CMD='printf "harness self-test for issue #%s — %s\n" "$ISSUE_NUMBER" "$ISSUE_TITLE" >> .issue-to-pr-smoke'
    warn "  no \$FIX_CMD set; --dry-run -> using a no-op SAMPLE fixer (writes .issue-to-pr-smoke)"
    warn "  wire a real fixer via FIX_CMD to address the issue for real (see --help)."
  else
    # No fixer + a real run would either error in the gate or, worse, open an
    # empty PR. Refuse, loudly, with the contract.
    BRANCH_KEPT=0  # let the trap clean up the empty branch
    die "no fixer wired: set \$FIX_CMD to a command that edits the tree to fix issue #$ISSUE.
       e.g.  FIX_CMD='claude -p \"fix #$ISSUE: \$ISSUE_TITLE\"' $0 $ISSUE
       (the issue is in ISSUE_NUMBER/ISSUE_TITLE/ISSUE_BODY; see --help). Not opening an empty PR."
  fi
fi

say "  running: $FIX_CMD"
# Run the fixer in a subshell so a non-zero exit is catchable (set -e off here).
set +e
( eval "$FIX_CMD" )
FIX_RC=$?
set -e
[[ "$FIX_RC" -eq 0 ]] || die "fixer exited $FIX_RC — aborting (branch will be cleaned up)"
ok "  fixer finished (exit 0)"

# ---- step 4: no-change guard (never an empty PR) -----------------------------
say "== 4/6  did the fixer change anything? =="
# Stage everything (incl. new files) so the check + later commit see all of it.
git add -A
if git diff --cached --quiet; then
  BRANCH_KEPT=0  # trap cleans up the no-op branch
  die "the fixer produced NO changes — nothing to propose. Not opening an empty PR."
fi
CHANGED_FILES="$(git diff --cached --name-only)"
DIFFSTAT="$(git diff --cached --stat)"
ok "  changes detected:"
printf '%s\n' "$DIFFSTAT" | sed 's/^/    /'

# ---- step 5: THE GATE — verify before any PR ---------------------------------
say "== 5/6  RUN THE VERIFY GATE (scripts/verify.sh) — the immune system =="
say "  a PR is opened ONLY if this passes."
# The gate runs on the working tree (changes already applied). Don't let the
# EXIT trap fire from inside the gate's failure; capture its rc explicitly.
set +e
bash scripts/verify.sh
GATE_RC=$?
set -e

if [[ "$GATE_RC" -ne 0 ]]; then
  printf "\n${R}VERIFY GATE FAILED${N} (exit $GATE_RC) — the change did NOT pass the immune system.\n" >&2
  printf "NO pull request will be opened. This is the safety guarantee working as intended.\n" >&2
  if [[ "$CLEAN_ON_FAIL" -eq 1 ]]; then
    warn "  --clean-on-fail: discarding $BRANCH and returning to $START_BRANCH"
    BRANCH_KEPT=0   # trap restores
  else
    warn "  branch '$BRANCH' left checked out with the (failing) change for inspection."
    warn "  inspect: git -C '$(pwd)' diff $BASE_REF"
    warn "  discard: git switch $START_BRANCH && git branch -D $BRANCH"
    BRANCH_KEPT=1   # don't let the trap nuke it
  fi
  exit "$GATE_RC"
fi
ok "  VERIFY GATE PASSED — all proof-of-spec stages green."

# ---- step 6: commit + (push + PR | dry-run report) ---------------------------
say "== 6/6  open the verify-gated PR =="

COMMIT_TITLE="fix: address issue #$ISSUE — $ISSUE_TITLE"
# Keep the subject line reasonable.
[[ ${#COMMIT_TITLE} -le 100 ]] || COMMIT_TITLE="${COMMIT_TITLE:0:97}..."

COMMIT_MSG="$(cat <<EOF
$COMMIT_TITLE

Fixes #$ISSUE via the issue-to-pr harness. The proof-of-spec verify gate
(scripts/verify.sh) passed on this change before the PR was opened.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"

PR_BODY="$(cat <<EOF
Closes #$ISSUE

## What

Automated, **verify-gated** change for issue #$ISSUE (_${ISSUE_TITLE}_),
produced by \`scripts/issue-to-pr.sh\` with a pluggable fixer.

## Files changed

\`\`\`
$DIFFSTAT
\`\`\`

## Immune system — the verify gate PASSED ✅

Before this PR was opened, the project's proof-of-spec gate ran and **passed**:

\`\`\`
bash scripts/verify.sh
\`\`\`

That gate runs the native test suite, the wasm32 browser-app guardrail, and real
cartridge instantiate / render / compose checks. This PR exists **only because**
that gate was green — no green gate, no PR. A reviewer can re-run it to confirm.

> Honest scope: the fix was generated by a pluggable \`\$FIX_CMD\`, not by the
> harness itself. The harness's guarantee is the safety mechanism around it —
> verify-gated, never an empty PR, never a PR on red.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"

if [[ "$DRY_RUN" -eq 1 ]]; then
  printf "\n${B}-- DRY RUN -- would now:${N}\n"
  printf "  1. commit on %s with message:\n" "$BRANCH"
  printf '%s\n' "$COMMIT_MSG" | sed 's/^/       | /'
  printf "  2. git push -u origin %s\n" "$BRANCH"
  printf "  3. gh pr create --repo %s --base %s --head %s --title %q --body <<...>>\n" \
    "$REPO" "$BASE" "$BRANCH" "$COMMIT_TITLE"
  printf "     PR body:\n"
  printf '%s\n' "$PR_BODY" | sed 's/^/       | /'
  if [[ "$SAMPLE_FIXER" -eq 1 ]]; then
    warn "  (sample fixer was used — discarding its scratch change + branch)"
    BRANCH_KEPT=0   # trap cleans up the smoke branch + sentinel file
  else
    warn "  branch '$BRANCH' kept (your fixer's real change). To discard:"
    warn "    git switch $START_BRANCH && git branch -D $BRANCH"
    BRANCH_KEPT=1
  fi
  ok "\nDRY RUN COMPLETE — gate ran, nothing pushed, no PR opened."
  exit 0
fi

# --- real run: commit, push, open PR ---
git commit --quiet -m "$COMMIT_MSG"
ok "  committed."

git remote get-url origin >/dev/null 2>&1 || die "no 'origin' remote to push to"
say "  pushing $BRANCH -> origin..."
git push --quiet -u origin "$BRANCH" || die "git push failed"
ok "  pushed."

say "  opening PR via gh..."
PR_URL="$(gh pr create --repo "$REPO" --base "$BASE" --head "$BRANCH" \
  --title "$COMMIT_TITLE" --body "$PR_BODY")" \
  || die "gh pr create failed (branch is pushed; you can open the PR manually)"

BRANCH_KEPT=1   # success — keep the branch + the open PR
ok "\nPR OPENED (verify gate passed): $PR_URL"
printf '%s\n' "$PR_URL"
