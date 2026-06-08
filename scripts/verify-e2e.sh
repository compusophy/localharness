#!/usr/bin/env bash
# scripts/verify-e2e.sh — the LIVE end-to-end regression suite (the "proof-of-
# spec" for the SHIPPED PLATFORM FLOWS). Installment 2 of the proof-of-spec
# system: verify.sh proves the wasm framebuffer renders (static, offline);
# verify-onchain.sh proves ONE sponsored write lands. THIS proves every shipped
# user-facing FLOW still works end to end against the LIVE Tempo Moderato testnet
# AND the live credit proxy — so a future regression in any of them is caught.
#
# Why it exists / why it's separate: `cargo test` is network-free and can only
# prove pure logic. The whole bug history of this project is "local says ok,
# chain reverted / proxy 402'd silently". The only real proof a flow works is to
# RUN the CLI command a user would run and then INDEPENDENTLY assert the result —
# grep the CLI output, or (where the CLI's own exit is unreliable) read the chain
# back with `cast call`. The on-chain state is the source of truth.
#
# It spends a TINY amount of live sponsor gas (AlphaUSD) and a TINY amount of the
# test identity's $LH (per-call meter ~0.01, an x402 micro-payment 0.001). It is
# self-cleaning + idempotent: every escrow it opens (a scheduled job, an invite)
# is closed again in the same run (unschedule / self-accept), so re-runs never
# accumulate state and the net $LH change is ~zero beyond per-call metering.
#
#   Flows asserted (all as the funded `claude` identity):
#     1. whoami      identity resolves on-chain (registered:true + address)
#     2. discover    a hit returns >=1 agent; a miss returns 0 (both asserted)
#     3. call        a headless turn to a live persona returns non-empty text
#     4. mcp-call    an x402 micro-payment settles + returns non-empty text
#     5. schedule    schedule -> jobCount++ + job in `jobs`; unschedule -> refund
#     6. invite      create -> escrowedOf rose; reclaim REJECTED (not expired);
#                    self-accept -> escrow released (net-zero, no orphan escrow)
#     7. send        OPTIONAL (off by default): tiny self-send asserts balance flow
#
# A non-zero exit means at least one shipped flow regressed. Run it by hand
# (NOT from verify.sh — that gate must stay network-free):
#     bash scripts/verify-e2e.sh                 # core suite
#     E2E_RUN_SEND=1 bash scripts/verify-e2e.sh  # include the optional send flow
set -uo pipefail
cd "$(dirname "$0")/.."

# ---------------------------------------------------------------------------
# Colors / reporting helpers + a pass/fail accumulator (the on-non-zero-exit
# contract every proof script in this repo follows).
# ---------------------------------------------------------------------------
if [[ -t 1 ]]; then B='\033[1m'; G='\033[1;32m'; R='\033[1;31m'; Y='\033[1;33m'; N='\033[0m'; else B=''; G=''; R=''; Y=''; N=''; fi
PASS_N=0
FAIL_N=0
SKIP_N=0
FAILURES=()

step() { printf "\n${B}== %s ==${N}\n" "$1"; }
pass() { PASS_N=$((PASS_N + 1)); printf "  ${G}PASS${N} %s\n" "$1"; }
fail() { FAIL_N=$((FAIL_N + 1)); FAILURES+=("$1"); printf "  ${R}FAIL${N} %s\n" "$1"; }
skip() { SKIP_N=$((SKIP_N + 1)); printf "  ${Y}SKIP${N} %s\n" "$1"; }
note() { printf "  %s\n" "$1"; }

# ---------------------------------------------------------------------------
# On-chain constants (canonical post-reset addresses — CLAUDE.md "on-chain
# stack"). The diamond is the only durable handle; everything routes through it.
# ---------------------------------------------------------------------------
DIAMOND="0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c"
RPC="https://rpc.moderato.tempo.xyz"
ME="claude"                 # the funded test identity (key in the cwd)

# ---------------------------------------------------------------------------
# Resolve the CLI binary. Prefer a prebuilt target/{debug,release} binary
# (`.exe` on Windows / MSYS, no suffix elsewhere). Build it if missing — the
# binary needs the wallet (chain) + anthropic (call --model claude) features.
# Using the binary directly (not `cargo run`) keeps each invocation fast.
# ---------------------------------------------------------------------------
case "$(uname -s 2>/dev/null || echo unknown)" in
  MINGW*|MSYS*|CYGWIN*|Windows_NT) EXE=".exe" ;;
  *)                               EXE="" ;;
esac
BIN=""
for cand in "target/debug/localharness${EXE}" "target/release/localharness${EXE}"; do
  if [[ -x "$cand" ]]; then BIN="$cand"; break; fi
done
if [[ -z "$BIN" ]]; then
  step "building the localharness CLI (no prebuilt binary found)"
  if cargo build --features wallet,anthropic --bin localharness; then
    BIN="target/debug/localharness${EXE}"
  else
    printf "\n${R}E2E SETUP FAILED${N} — could not build the localharness binary.\n" >&2
    exit 1
  fi
fi
note "binary: $BIN"

# ---------------------------------------------------------------------------
# Resolve the funded identity key. The CLI reads `<name>.localharness.key` from
# the cwd (or the config home), so a key must be present. In a clean worktree
# the keys live in the main checkout — fall back to a sibling that has it, and
# run the suite from there (so the CLI's cwd key resolution finds it).
# ---------------------------------------------------------------------------
KEY="${ME}.localharness.key"
if [[ ! -f "$KEY" ]]; then
  # Common case: this is an isolated git worktree; the keys are in the primary
  # working tree. Resolve it via git and re-home the suite there.
  PRIMARY="$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null | sed 's#/\.git$##')"
  if [[ -n "$PRIMARY" && -f "$PRIMARY/$KEY" ]]; then
    note "no $KEY in the cwd — using the primary checkout at $PRIMARY"
    # Re-resolve the binary as an absolute path BEFORE we leave this dir.
    case "$BIN" in /*|?:*) : ;; *) BIN="$PWD/$BIN" ;; esac
    cd "$PRIMARY"
  fi
fi
if [[ ! -f "$KEY" ]]; then
  printf "\n${R}E2E SETUP FAILED${N} — no %s in %s (the funded test identity).\n" "$KEY" "$PWD" >&2
  printf "  Run the suite from a checkout that holds the claude identity key.\n" >&2
  exit 1
fi

# The CLI is resolved by name from the cwd, so `--as claude` Just Works here.
CLI=("$BIN")
# Two `--as` conventions in the CLI: most commands accept `--as` ANYWHERE
# (`take_as_flag`), but `call`/`mcp-call` only consume it as a LEADING flag
# before the target (`parse_call_args`/`parse_mcp_call_args`). So:
#   `as <cmd> ...`     -> appends `--as claude` (fine for take_as_flag commands)
#   `as_lead <cmd> ...`-> inserts `--as claude` right AFTER the subcommand
# This dir holds many identity keys, so `--as` is mandatory (no inference).
as() { "${CLI[@]}" "$@" --as "$ME"; }
as_lead() { local cmd="$1"; shift; "${CLI[@]}" "$cmd" --as "$ME" "$@"; }

# A `cast call` reader. All asserts that read the chain go through this so a
# transient RPC blip is reported as a fail with context, not a crash.
ME_ADDR="$(cast wallet address --private-key "0x$(tr -d '[:space:]' < "$KEY" | sed 's/^0x//')" 2>/dev/null || true)"
if [[ -z "$ME_ADDR" ]]; then
  printf "\n${R}E2E SETUP FAILED${N} — could not derive the address for %s.\n" "$ME" >&2
  exit 1
fi
note "identity: $ME ($ME_ADDR)"

# Read a uint256 view off the diamond, stripping cast's trailing "[scientific]"
# annotation so the bare decimal is returned (empty on RPC failure).
read_uint() { # $1 = full "sig(args)(uint256)" ; $2.. = args
  local sig="$1"; shift
  cast call "$DIAMOND" "$sig" "$@" --rpc-url "$RPC" 2>/dev/null | awk '{print $1}'
}

# ===========================================================================
# 1. whoami — the identity resolves on-chain (read-only RPC, the trust anchor).
# ===========================================================================
step "1. whoami — identity resolves on-chain"
WHOAMI_JSON="$("${CLI[@]}" whoami --json "$ME" 2>&1)"
note "$(printf '%s' "$WHOAMI_JSON" | tr '\n' ' ')"
if printf '%s' "$WHOAMI_JSON" | grep -q '"registered": *true'; then
  pass "whoami reports registered:true for $ME"
else
  fail "whoami did not report registered:true for $ME"
fi
# The on-chain owner must be the key we hold (the proof the identity is OURS).
OWNER="$(cast call "$DIAMOND" "ownerOfName(string)(address)" "$ME" --rpc-url "$RPC" 2>/dev/null)"
if [[ -n "$OWNER" && "${OWNER,,}" == "${ME_ADDR,,}" ]]; then
  pass "ownerOfName($ME) == our key ($ME_ADDR)"
else
  fail "ownerOfName($ME)=$OWNER != our key $ME_ADDR"
fi

# ===========================================================================
# 2. discover — the Agent Yellow Pages. A real capability returns >=1 agent;
#    nonsense returns 0. BOTH assertions matter (a discover that always returns
#    everything, or always nothing, is broken — assert the discriminator works).
# ===========================================================================
step "2. discover — a hit returns agents, a miss returns none"
HIT="$("${CLI[@]}" discover "security" 2>&1)"
if printf '%s' "$HIT" | grep -qE '[0-9]+ agent\(s\) matching'; then
  pass "discover \"security\" returned >=1 agent"
else
  # Tolerate the legitimate empty case but flag it — the registry may simply
  # have no security-tagged personas in the scan window.
  if printf '%s' "$HIT" | grep -q 'no agents match'; then
    fail "discover \"security\" found 0 agents (expected >=1 — is the registry populated?)"
  else
    fail "discover \"security\" produced unexpected output: $(printf '%s' "$HIT" | head -1)"
  fi
fi
MISS="$("${CLI[@]}" discover "zzznope-$(date +%s)" 2>&1)"
if printf '%s' "$MISS" | grep -q 'no agents match'; then
  pass "discover \"zzznope…\" returned 0 agents"
else
  fail "discover for a nonsense query unexpectedly matched: $(printf '%s' "$MISS" | head -1)"
fi

# ===========================================================================
# 3. call — a HEADLESS turn to a live persona via the credit proxy. Asserts the
#    full path: proxy auth (signed by our key) + meter funding + the model
#    answering as the target. Success = non-empty reply text on stdout, exit 0.
#    We call our OWN persona ($ME) so the target is guaranteed live + funded.
# ===========================================================================
step "3. call — a headless turn returns non-empty text"
CALL_OUT="$(as_lead call --fresh "$ME" "Reply with the single word: pong" 2>&1)"; CALL_RC=$?
note "reply: $(printf '%s' "$CALL_OUT" | tr '\n' ' ' | cut -c1-120)"
# Strip the known non-answer noise lines so "non-empty" means real model text.
CALL_TEXT="$(printf '%s' "$CALL_OUT" | grep -vE '^(warning:|claimed |deposited |session )' | tr -d '[:space:]')"
if [[ $CALL_RC -eq 0 && -n "$CALL_TEXT" ]]; then
  pass "call returned non-empty text (exit 0)"
else
  fail "call returned empty text or non-zero exit ($CALL_RC): $(printf '%s' "$CALL_OUT" | head -1)"
fi

# ===========================================================================
# 4. mcp-call --pay — the HOSTED MCP-over-HTTP + TRUE x402 settlement path. A
#    tiny $LH micro-payment (0.001) is signed to the target's TBA, settled by
#    the proxy via X402Facet, and the agent answers. Success = non-empty reply,
#    exit 0. (mcp-call to ourselves: payer==payee owner, the $LH round-trips.)
# ===========================================================================
step "4. mcp-call --pay — x402 settles + returns text"
MCP_OUT="$(as_lead mcp-call --pay 0.001 "$ME" "Reply with the single word: pong" 2>&1)"; MCP_RC=$?
note "reply: $(printf '%s' "$MCP_OUT" | tr '\n' ' ' | cut -c1-120)"
MCP_TEXT="$(printf '%s' "$MCP_OUT" | grep -vE '^(approving |  approved)' | tr -d '[:space:]')"
if [[ $MCP_RC -eq 0 && -n "$MCP_TEXT" ]]; then
  pass "mcp-call --pay settled + returned non-empty text (exit 0)"
else
  fail "mcp-call --pay failed (exit $MCP_RC): $(printf '%s' "$MCP_OUT" | head -2 | tr '\n' ' ')"
fi

# ===========================================================================
# 5. schedule lifecycle — escrow a job, assert it landed (jobCount++ + the id is
#    in `jobs`), then cancel it (refund) and assert it's no longer Active. The
#    chain is the source of truth: we read jobCount() / jobsOf() / getJob() with
#    `cast`, not the CLI's own exit (which can false-negative on a slow mine).
#    FAST: a 60s cadence + 1 run + 0.05 budget; we do NOT wait for the cron.
# ===========================================================================
step "5. schedule lifecycle — schedule then unschedule (refund)"
JOBS_BEFORE="$(read_uint 'jobCount()(uint256)')"
note "jobCount before: ${JOBS_BEFORE:-?}"
SCHED_OUT="$(as schedule "$ME" "e2e regression probe" --every 60s --budget 0.05 --runs 1 2>&1)"; SCHED_RC=$?
note "$(printf '%s' "$SCHED_OUT" | grep -E '^✓|job #|scheduling' | head -1)"
JOBS_AFTER="$(read_uint 'jobCount()(uint256)')"
note "jobCount after:  ${JOBS_AFTER:-?}"
NEW_JOB_ID=""
if [[ -n "${JOBS_BEFORE:-}" && -n "${JOBS_AFTER:-}" && "$JOBS_AFTER" -gt "$JOBS_BEFORE" ]]; then
  pass "jobCount incremented ($JOBS_BEFORE -> $JOBS_AFTER) — the escrow landed"
  # The new job id is jobCount() itself (ids are 1-based, monotonic).
  NEW_JOB_ID="$JOBS_AFTER"
else
  fail "jobCount did NOT increment ($JOBS_BEFORE -> $JOBS_AFTER) — schedule did not land (exit $SCHED_RC)"
fi
# Assert the new id appears in the owner's jobsOf index.
if [[ -n "$NEW_JOB_ID" ]]; then
  JOBS_LIST="$(cast call "$DIAMOND" "jobsOf(address)(uint256[])" "$ME_ADDR" --rpc-url "$RPC" 2>/dev/null)"
  if printf '%s' "$JOBS_LIST" | grep -qE "(^|[^0-9])$NEW_JOB_ID([^0-9]|$)"; then
    pass "job #$NEW_JOB_ID appears in jobsOf($ME)"
  else
    fail "job #$NEW_JOB_ID NOT in jobsOf($ME): $JOBS_LIST"
  fi
  # The CLI `jobs` listing must also show it (the user-facing read).
  if as jobs 2>&1 | grep -qE "#$NEW_JOB_ID(\b|[^0-9])"; then
    pass "\`jobs\` lists job #$NEW_JOB_ID"
  else
    fail "\`jobs\` did not list job #$NEW_JOB_ID"
  fi
fi
# --- cleanup: cancel the job (refunds the remaining budget). Assert the job
# leaves the Active state (status byte 0). getJob returns a tuple; the status is
# the 3rd field (0 Active / 1 Paused / 2 Cancelled / 3 Exhausted).
if [[ -n "$NEW_JOB_ID" ]]; then
  CANCEL_OUT="$(as unschedule "$NEW_JOB_ID" 2>&1)"; CANCEL_RC=$?
  note "$(printf '%s' "$CANCEL_OUT" | grep -E '^✓|cancelled|refund' | head -1)"
  JOB_TUPLE="$(cast call "$DIAMOND" "getJob(uint256)((address,uint64,uint8,uint64,uint128,uint32,uint64))" "$NEW_JOB_ID" --rpc-url "$RPC" 2>/dev/null)"
  # Parse the comma-separated tuple -> field 3 (1-based) = status.
  STATUS="$(printf '%s' "$JOB_TUPLE" | tr -d '()' | awk -F',' '{gsub(/ /,"",$3); print $3}')"
  if [[ "$STATUS" == "2" ]]; then
    pass "job #$NEW_JOB_ID cancelled (status=2) — budget refunded"
  else
    fail "job #$NEW_JOB_ID not Cancelled after unschedule (status=$STATUS, exit $CANCEL_RC) — escrow may be orphaned"
  fi
fi

# ===========================================================================
# 6. invite lifecycle — create (escrow rises) -> reclaim REJECTED (not expired)
#    -> self-accept (escrow released). Net-zero: the $LH leaves our balance into
#    escrow, then comes back to us on accept, leaving no orphaned escrow. The
#    escrow delta is read with `cast escrowedOf` (the chain truth), and the code
#    is hashed with `cast keccak` (== the CLI's invite_code_hash).
# ===========================================================================
step "6. invite lifecycle — create / reclaim-rejected / self-accept (net-zero)"
ESCROW_BEFORE="$(read_uint 'escrowedOf(address)(uint256)' "$ME_ADDR")"
note "escrowedOf before: ${ESCROW_BEFORE:-?}"
INV_OUT="$(as invite create --amount 0.02 --ttl 1h 2>&1)"; INV_RC=$?
# The CLI prints the bearer code as a line `  code:  <code>`. Capture it.
INV_CODE="$(printf '%s' "$INV_OUT" | grep -oE 'code: *[A-Za-z0-9._-]+' | head -1 | awk '{print $2}')"
note "code: ${INV_CODE:-<none captured>}"
if [[ -z "$INV_CODE" ]]; then
  fail "invite create did not print a code (exit $INV_RC): $(printf '%s' "$INV_OUT" | head -2 | tr '\n' ' ')"
else
  # Assert the escrow rose by 0.02 $LH (2e16 wei). Use the on-chain getInvite to
  # confirm the code is funded + the escrowedOf total rose by the amount.
  ESCROW_AFTER="$(read_uint 'escrowedOf(address)(uint256)' "$ME_ADDR")"
  note "escrowedOf after:  ${ESCROW_AFTER:-?}"
  DELTA="$(python3 -c "print(int('${ESCROW_AFTER:-0}') - int('${ESCROW_BEFORE:-0}'))" 2>/dev/null || echo "")"
  if [[ "$DELTA" == "20000000000000000" ]]; then
    pass "escrowedOf rose by exactly 0.02 \$LH (2e16 wei)"
  else
    fail "escrowedOf delta=$DELTA wei, expected 2e16 (0.02 \$LH) — escrow accounting off"
  fi
  # getInvite must show our address as the funder for this code's hash.
  CODE_HASH="$(cast keccak "$INV_CODE")"
  INV_REC="$(cast call "$DIAMOND" "getInvite(bytes32)(address,uint128,uint64,uint8)" "$CODE_HASH" --rpc-url "$RPC" 2>/dev/null)"
  INV_FUNDER="$(printf '%s' "$INV_REC" | head -1 | awk '{print $1}')"
  if [[ -n "$INV_FUNDER" && "${INV_FUNDER,,}" == "${ME_ADDR,,}" ]]; then
    pass "getInvite(code) funder == $ME ($ME_ADDR)"
  else
    fail "getInvite(code) funder=$INV_FUNDER != $ME_ADDR"
  fi

  # reclaim must be REJECTED while the invite is still Open + unexpired (the 1h
  # TTL is fresh). The facet reverts; the CLI surfaces a non-zero exit.
  RECLAIM_OUT="$(as invite reclaim "$INV_CODE" 2>&1)"; RECLAIM_RC=$?
  if [[ $RECLAIM_RC -ne 0 ]]; then
    pass "invite reclaim REJECTED on a fresh (unexpired) invite (exit $RECLAIM_RC)"
  else
    fail "invite reclaim SUCCEEDED on an unexpired invite — TTL guard regressed"
  fi

  # self-accept recovers the escrow back into our wallet (net-zero) + releases
  # the escrowedOf lock. Assert escrowedOf returns to the pre-create level.
  ACCEPT_OUT="$(as invite accept "$INV_CODE" 2>&1)"; ACCEPT_RC=$?
  note "$(printf '%s' "$ACCEPT_OUT" | grep -E '^✓|accepted' | head -1)"
  ESCROW_FINAL="$(read_uint 'escrowedOf(address)(uint256)' "$ME_ADDR")"
  note "escrowedOf final:  ${ESCROW_FINAL:-?}"
  if [[ -n "${ESCROW_FINAL:-}" && "$ESCROW_FINAL" == "${ESCROW_BEFORE:-x}" ]]; then
    pass "self-accept released the escrow (escrowedOf back to $ESCROW_BEFORE) — net-zero, no orphan"
  else
    fail "escrowedOf=$ESCROW_FINAL after accept, expected $ESCROW_BEFORE (exit $ACCEPT_RC) — escrow may be orphaned"
  fi
fi

# ===========================================================================
# 7. send — OPTIONAL, off by default (E2E_RUN_SEND=1 to enable). A tiny self-
#    send is wasteful (a no-op on net balance, only burns sponsor gas), so it's
#    guarded. When enabled, assert the CLI reports success (the $LH transfer is
#    a self-transfer; balance is unchanged but the tx must land).
# ===========================================================================
step "7. send — tiny self-transfer (optional)"
if [[ "${E2E_RUN_SEND:-0}" == "1" ]]; then
  SEND_OUT="$(as send "$ME_ADDR" 0.001 2>&1)"; SEND_RC=$?
  note "$(printf '%s' "$SEND_OUT" | head -1)"
  if [[ $SEND_RC -eq 0 ]] && printf '%s' "$SEND_OUT" | grep -qiE 'sent .* \$LH|tx:'; then
    pass "send 0.001 \$LH landed (exit 0)"
  else
    fail "send failed (exit $SEND_RC): $(printf '%s' "$SEND_OUT" | head -1)"
  fi
else
  skip "send (set E2E_RUN_SEND=1 to exercise; a self-send is net-zero + only burns sponsor gas)"
fi

# ---------------------------------------------------------------------------
# Final summary + exit contract: non-zero iff any flow failed.
# ---------------------------------------------------------------------------
TOTAL=$((PASS_N + FAIL_N))
printf "\n${B}== summary ==${N}\n"
printf "  %d/%d assertions passed" "$PASS_N" "$TOTAL"
if [[ $SKIP_N -gt 0 ]]; then printf "  (%d skipped)" "$SKIP_N"; fi
printf "\n"
if [[ $FAIL_N -eq 0 ]]; then
  printf "\n${G}LIVE E2E OK${N} — every shipped platform flow works end to end.\n"
  exit 0
fi
printf "\n${R}LIVE E2E FAILED${N} — %d flow(s) regressed:\n" "$FAIL_N"
for f in "${FAILURES[@]}"; do printf "  ${R}-${N} %s\n" "$f"; done
exit 1
