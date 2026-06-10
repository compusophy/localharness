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
#   Flows asserted (all as the funded `claude` identity unless noted):
#     1. whoami      identity resolves on-chain (registered:true + address)
#     2. discover    a hit returns >=1 agent; a miss returns 0 (both asserted)
#     3. call        a headless turn to a live persona returns non-empty text
#     4. mcp-call    an x402 micro-payment settles + returns non-empty text
#     5. schedule    schedule -> jobCount++ + job in `jobs`; unschedule -> refund
#     6. invite      create -> escrowedOf rose; reclaim REJECTED (not expired);
#                    self-accept -> escrow released (net-zero, no orphan escrow)
#   --- the agent-economy coordination ladder (bounty -> guild -> DAO) ---
#     7. guild       create -> new guildId; fund 0.05 -> treasuryBalanceOf rose;
#                    invite + (member) accept -> isGuildMember true + roster grew
#     8. vote (DAO)  propose -> proposalId; cast `for` -> tallyOf for=1 + hasVoted
#                    true + passing (NOT executed — needs the voting window)
#     9. tba-exec    deploy (idempotent) -> code on-chain; fund the TBA, then
#                    `tba exec` -> the recipient's $LH rose by the sent amount
#    10. reputation  attest (fresh --ref) -> reputationOf count +1 and sum +rating
#    11. colony      OPTIONAL (E2E_RUN_COLONY=1): one full cycle pays the worker's
#                    TBA the reward (CYCLE asserted, not the LLM result text)
#    12. send        OPTIONAL (E2E_RUN_SEND=1): tiny self-send asserts balance flow
#
# The ladder blocks create FRESH test guilds / proposals / attestations each run.
# On testnet that's acceptable for an occasional regression gate (no cleanup of
# the guild/proposal — they're cheap on-chain rows; the reputation attestation
# uses a fresh --ref each run so it's a real write, not a dedup no-op).
#
# A non-zero exit means at least one shipped flow regressed. Run it by hand
# (NOT from verify.sh — that gate must stay network-free):
#     bash scripts/verify-e2e.sh                    # core suite (+ ladder 7-10)
#     E2E_RUN_SEND=1 bash scripts/verify-e2e.sh     # + the optional self-send
#     E2E_RUN_COLONY=1 bash scripts/verify-e2e.sh   # + the full colony cycle (slow/LLM)
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
# the cwd first, else the config home (`$LOCALHARNESS_HOME`, default
# `~/.localharness/keys` — where keys live since the keys-out-of-cwd change).
# Mirror that precedence here for every key the suite reads directly.
# ---------------------------------------------------------------------------
KEY_HOME="${LOCALHARNESS_HOME:-$HOME/.localharness/keys}"
key_path() { # $1 = identity name -> the readable key path (cwd wins, else home)
  local f="$1.localharness.key"
  if [[ -f "$f" ]]; then printf '%s' "$f"; else printf '%s' "$KEY_HOME/$f"; fi
}
KEY="$(key_path "$ME")"
if [[ ! -f "$KEY" ]]; then
  # Legacy case: an isolated git worktree with cwd-local keys in the primary
  # working tree. Resolve it via git and re-home the suite there. (Config-home
  # keys are per-user, so they already work from any checkout.)
  PRIMARY="$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null | sed 's#/\.git$##')"
  if [[ -n "$PRIMARY" && -f "$PRIMARY/${ME}.localharness.key" ]]; then
    note "no $KEY — using the primary checkout at $PRIMARY"
    # Re-resolve the binary as an absolute path BEFORE we leave this dir.
    case "$BIN" in /*|?:*) : ;; *) BIN="$PWD/$BIN" ;; esac
    cd "$PRIMARY"
    KEY="$(key_path "$ME")"
  fi
fi
if [[ ! -f "$KEY" ]]; then
  printf "\n${R}E2E SETUP FAILED${N} — no %s key (cwd of %s, or %s).\n" "$ME" "$PWD" "$KEY_HOME" >&2
  printf "  Run the suite where the claude identity key resolves.\n" >&2
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

# ---------------------------------------------------------------------------
# Agent-economy ladder constants (blocks 7-11). The ladder needs a SECOND
# identity for the member / worker / payout-recipient roles. Prefer a fleet
# agent whose key is present (dex-qa for guild/vote/tba/reputation; vex-qa for
# the colony worker), so the run is fully self-driving as `claude`.
#   - $LH (the credit token) balances are read via `balanceOf` on the TOKEN
#     contract, NOT the diamond — the diamond is the registry, $LH is its own
#     TIP-20 at LH_TOKEN (canonical post-reset address, CLAUDE.md Tempo section).
#   - PEER = the member/recipient identity; WORKER = the colony worker.
# ---------------------------------------------------------------------------
LH_TOKEN="0x90B84c7234Aae89BadA7f69160B9901B9bc37B17"
PEER="dex-qa"     # guild member / vote recipient / tba-exec recipient / attestee
WORKER="vex-qa"   # colony worker (its key must be local — it signs claim+submit)

# Derive PEER's owner address (from its local key) — the address we assert
# membership / balance deltas against. Empty (skipped) if the key is absent.
PEER_KEY="$(key_path "$PEER")"
PEER_ADDR=""
if [[ -f "$PEER_KEY" ]]; then
  PEER_ADDR="$(cast wallet address --private-key "0x$(tr -d '[:space:]' < "$PEER_KEY" | sed 's/^0x//')" 2>/dev/null || true)"
fi

# A `--as <peer>` runner (the member accepts the invite / could vote). Mirrors
# `as` but for the PEER identity. Used only when PEER_KEY is present.
as_peer() { "${CLI[@]}" "$@" --as "$PEER"; }

# Read `balanceOf(addr)` off the $LH token contract (the credit-token balance,
# the source of truth for every $LH delta in the ladder). Empty on RPC failure.
read_lh() { # $1 = holder 0x address
  cast call "$LH_TOKEN" "balanceOf(address)(uint256)" "$1" --rpc-url "$RPC" 2>/dev/null | awk '{print $1}'
}

# Resolve a name's token-bound account address (its on-chain wallet). Empty if
# unregistered / RPC failure. Used to assert the colony worker's TBA payout.
tba_of() { # $1 = subdomain name
  cast call "$DIAMOND" "tokenBoundAccountByName(string)(address)" "$1" --rpc-url "$RPC" 2>/dev/null | awk '{print $1}'
}

# Resolve a name's tokenId (for reputationOf / proposal recipient). Empty/0 if
# unregistered.
id_of() { # $1 = subdomain name
  cast call "$DIAMOND" "idOfName(string)(uint256)" "$1" --rpc-url "$RPC" 2>/dev/null | awk '{print $1}'
}

# Integer delta of two decimal strings via awk (bignum-safe enough for wei here:
# the values fit a double only loosely, so use awk's arbitrary-precision-ish
# string subtraction guard — but $LH deltas in this gate are <= 0.05e18, which a
# double represents exactly, and we compare to an exact expected literal). For
# the exact-equality asserts we prefer python3 when present (as block 6 does).
delta_wei() { # $1 = after  $2 = before  ->  echoes (after-before)
  if command -v python3 >/dev/null 2>&1; then
    python3 -c "print(int('${1:-0}') - int('${2:-0}'))" 2>/dev/null || echo ""
  else
    awk "BEGIN{printf \"%d\", ${1:-0} - ${2:-0}}" 2>/dev/null || echo ""
  fi
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
# 7. guild — PARTY/GUILD rung of the coordination ladder. Create a fresh guild
#    (claude becomes its admin), fund its on-chain treasury, invite the PEER and
#    have the PEER accept. Each step asserts a CONCRETE on-chain delta:
#      create  -> a NEW guildId appears (guildsOf(claude) grew, last id is new)
#      fund    -> treasuryBalanceOf(id) rose by exactly 0.05 $LH
#      invite+accept -> isGuildMember(id, peer)==true AND guildMembersOf grew
#    Fresh guild per run (testnet — a cheap append-only row; not cleaned up).
# ===========================================================================
step "7. guild — create / fund / invite+accept (the guild rung)"
GUILD_NAME="e2e-guild-$(date +%s)"
GUILDS_BEFORE="$(cast call "$DIAMOND" "guildsOf(address)(uint256[])" "$ME_ADDR" --rpc-url "$RPC" 2>/dev/null | tr -d '[] ')"
GC_OUT="$(as guild create "$GUILD_NAME" 2>&1)"; GC_RC=$?
note "$(printf '%s' "$GC_OUT" | grep -E '✓|guild #' | head -1)"
# The new guildId is the LAST entry in the creator's guildsOf index (the CLI
# reads it the same way; we re-read the chain to assert, not the CLI output).
GUILD_ID="$(cast call "$DIAMOND" "guildsOf(address)(uint256[])" "$ME_ADDR" --rpc-url "$RPC" 2>/dev/null | tr -d '[]' | awk '{print $NF}')"
note "new guildId: ${GUILD_ID:-?}  (guildsOf before: [${GUILDS_BEFORE:-}])"
if [[ -n "${GUILD_ID:-}" && "$GUILD_ID" =~ ^[0-9]+$ ]] && [[ ",$GUILDS_BEFORE," != *",$GUILD_ID,"* ]]; then
  pass "guild create -> new guildId #$GUILD_ID (not in guildsOf before)"
else
  fail "guild create did NOT yield a new guildId (got '$GUILD_ID', exit $GC_RC)"
fi
if [[ -n "${GUILD_ID:-}" && "$GUILD_ID" =~ ^[0-9]+$ ]]; then
  # fund: treasuryBalanceOf must rise by exactly 0.05 $LH (5e16 wei).
  TREAS_BEFORE="$(read_uint 'treasuryBalanceOf(uint256)(uint256)' "$GUILD_ID")"
  FUND_OUT="$(as guild fund "$GUILD_ID" 0.05 2>&1)"; FUND_RC=$?
  note "$(printf '%s' "$FUND_OUT" | grep -E '✓|deposited' | head -1)"
  TREAS_AFTER="$(read_uint 'treasuryBalanceOf(uint256)(uint256)' "$GUILD_ID")"
  TREAS_DELTA="$(delta_wei "${TREAS_AFTER:-0}" "${TREAS_BEFORE:-0}")"
  note "treasuryBalanceOf $TREAS_BEFORE -> $TREAS_AFTER  (delta $TREAS_DELTA wei)"
  if [[ "$TREAS_DELTA" == "50000000000000000" ]]; then
    pass "guild fund 0.05 -> treasuryBalanceOf rose by exactly 5e16 wei"
  else
    fail "guild fund: treasury delta=$TREAS_DELTA wei, expected 5e16 (0.05 \$LH) (exit $FUND_RC)"
  fi

  # invite + (peer) accept: assert the PEER joins the on-chain roster. Needs the
  # PEER's key (it signs its own accept). Skip cleanly when the key is absent.
  if [[ -z "$PEER_ADDR" ]]; then
    skip "guild invite/accept ($PEER key not present — cannot sign the member's accept)"
  else
    MEMBERS_BEFORE_RAW="$(cast call "$DIAMOND" "guildMembersOf(uint256)(address[])" "$GUILD_ID" --rpc-url "$RPC" 2>/dev/null | tr ',' '\n' | grep -ciE '0x[0-9a-f]+' || echo 0)"
    INVITE_OUT="$(as guild invite "$GUILD_ID" "$PEER" 2>&1)"; INVITE_RC=$?
    note "$(printf '%s' "$INVITE_OUT" | grep -E '✓|invited' | head -1)"
    ACCEPT_OUT="$(as_peer guild accept "$GUILD_ID" 2>&1)"; GACC_RC=$?
    note "$(printf '%s' "$ACCEPT_OUT" | grep -E '✓|joined' | head -1)"
    IS_MEMBER="$(cast call "$DIAMOND" "isGuildMember(uint256,address)(bool)" "$GUILD_ID" "$PEER_ADDR" --rpc-url "$RPC" 2>/dev/null)"
    if [[ "$IS_MEMBER" == "true" ]]; then
      pass "guild invite+accept -> isGuildMember(#$GUILD_ID, $PEER)==true"
    else
      fail "isGuildMember(#$GUILD_ID, $PEER)=$IS_MEMBER after accept (invite rc $INVITE_RC / accept rc $GACC_RC)"
    fi
    MEMBERS_AFTER_RAW="$(cast call "$DIAMOND" "guildMembersOf(uint256)(address[])" "$GUILD_ID" --rpc-url "$RPC" 2>/dev/null | tr ',' '\n' | grep -ciE '0x[0-9a-f]+' || echo 0)"
    note "guildMembersOf count $MEMBERS_BEFORE_RAW -> $MEMBERS_AFTER_RAW"
    if [[ "${MEMBERS_AFTER_RAW:-0}" -gt "${MEMBERS_BEFORE_RAW:-0}" ]]; then
      pass "guildMembersOf(#$GUILD_ID) grew ($MEMBERS_BEFORE_RAW -> $MEMBERS_AFTER_RAW)"
    else
      fail "guildMembersOf(#$GUILD_ID) did not grow ($MEMBERS_BEFORE_RAW -> $MEMBERS_AFTER_RAW)"
    fi
  fi
fi

# ===========================================================================
# 8. vote (DAO) — the GOVERNANCE rung. A guild MEMBER opens a treasury-spend
#    proposal, then casts a `for` ballot. Asserts the on-chain governance state:
#      propose -> a NEW proposalId appears in proposalsOf(guildId)
#      cast for -> tallyOf(pid).forVotes == 1 AND hasVoted(pid, claude)==true
#                  AND tallyOf(pid).passing == true (quorum=ceil(members/2) met,
#                  for > against)  AND getProposal(pid).status == 0 (Active)
#    We do NOT execute: execute needs the --period window to close (1h here), and
#    execution-when-passed is Foundry-proven on the facet. The point of the LIVE
#    gate is the propose+vote tallying, asserted on-chain.
# ===========================================================================
step "8. vote (DAO) — propose + cast 'for' (tally asserted, not executed)"
if [[ -z "${GUILD_ID:-}" || ! "$GUILD_ID" =~ ^[0-9]+$ ]]; then
  fail "vote: no guild from block 7 to propose into"
else
  PROPS_BEFORE="$(cast call "$DIAMOND" "proposalsOf(uint256,uint256,uint256)(uint256[],uint256)" "$GUILD_ID" 0 100 --rpc-url "$RPC" 2>/dev/null | head -1 | tr -d '[] ')"
  # propose a tiny 0.01 $LH spend to the PEER, voting window 1h (== MIN period).
  VP_OUT="$(as vote propose "$GUILD_ID" "$PEER" 0.01 --period 1h e2e regression proposal 2>&1)"; VP_RC=$?
  note "$(printf '%s' "$VP_OUT" | grep -E '✓|proposal #' | head -1)"
  PROP_ID="$(cast call "$DIAMOND" "proposalsOf(uint256,uint256,uint256)(uint256[],uint256)" "$GUILD_ID" 0 100 --rpc-url "$RPC" 2>/dev/null | head -1 | tr -d '[]' | tr ',' '\n' | grep -E '[0-9]' | tail -1 | tr -d ' ')"
  note "new proposalId: ${PROP_ID:-?}"
  if [[ -n "${PROP_ID:-}" && "$PROP_ID" =~ ^[0-9]+$ && ",$PROPS_BEFORE," != *",$PROP_ID,"* ]]; then
    pass "vote propose -> new proposalId #$PROP_ID"
  else
    fail "vote propose did NOT yield a new proposalId (got '$PROP_ID', exit $VP_RC)"
  fi
  if [[ -n "${PROP_ID:-}" && "$PROP_ID" =~ ^[0-9]+$ ]]; then
    VC_OUT="$(as vote cast "$PROP_ID" for 2>&1)"; VC_RC=$?
    note "$(printf '%s' "$VC_OUT" | grep -E '✓|voted' | head -1)"
    # tallyOf returns (forVotes, againstVotes, quorum, votesCast, passing) — one
    # value per line. forVotes is line 1, passing is line 5.
    TALLY="$(cast call "$DIAMOND" "tallyOf(uint256)(uint256,uint256,uint256,uint256,bool)" "$PROP_ID" --rpc-url "$RPC" 2>/dev/null)"
    FOR_VOTES="$(printf '%s' "$TALLY" | awk 'NR==1{print $1}')"
    PASSING="$(printf '%s' "$TALLY" | awk 'NR==5{print $1}')"
    HAS_VOTED="$(cast call "$DIAMOND" "hasVoted(uint256,address)(bool)" "$PROP_ID" "$ME_ADDR" --rpc-url "$RPC" 2>/dev/null)"
    # getProposal: status is field 6 (uint8) — guild_id, proposer, to, amount,
    # deadline, status, forVotes, againstVotes.
    PSTATUS="$(cast call "$DIAMOND" "getProposal(uint256)(uint256,address,address,uint256,uint64,uint8,uint256,uint256)" "$PROP_ID" --rpc-url "$RPC" 2>/dev/null | awk 'NR==6{print $1}')"
    note "tally: for=$FOR_VOTES passing=$PASSING hasVoted=$HAS_VOTED status=$PSTATUS (cast rc $VC_RC)"
    if [[ "$FOR_VOTES" == "1" ]]; then
      pass "vote cast for -> tallyOf(#$PROP_ID).forVotes == 1"
    else
      fail "tallyOf(#$PROP_ID).forVotes=$FOR_VOTES, expected 1 (cast exit $VC_RC)"
    fi
    if [[ "$HAS_VOTED" == "true" ]]; then
      pass "hasVoted(#$PROP_ID, claude) == true"
    else
      fail "hasVoted(#$PROP_ID, claude)=$HAS_VOTED, expected true"
    fi
    if [[ "$PASSING" == "true" && "$PSTATUS" == "0" ]]; then
      pass "proposal #$PROP_ID is Active (status 0) AND passing (quorum met, for>against)"
    else
      fail "proposal #$PROP_ID passing=$PASSING status=$PSTATUS (expected passing=true, status=0 Active)"
    fi
  fi
fi

# ===========================================================================
# 9. tba-exec — the agent's WALLET acts. Deploy claude's token-bound account
#    (idempotent), fund it a little, then have it EXECUTE a $LH transfer to the
#    PEER. Asserts the EXECUTION had an on-chain effect:
#      deploy -> eth_getCode(claude TBA) is non-empty (the account exists)
#      exec   -> the recipient's $LH balance rose by exactly the sent 0.01 $LH
#                (`tba exec <name> <amt>` sends to the NAME's OWNER address, so we
#                 assert the PEER's owner-EOA balance delta).
# ===========================================================================
step "9. tba-exec — deploy + execute a \$LH transfer from the agent's TBA"
CLAUDE_TBA="$(tba_of "$ME")"
note "claude TBA: ${CLAUDE_TBA:-?}"
DEP_OUT="$(as tba deploy 2>&1)"; DEP_RC=$?
note "$(printf '%s' "$DEP_OUT" | grep -iE '✓|deployed|already' | head -1)"
TBA_CODE="$(cast code "$CLAUDE_TBA" --rpc-url "$RPC" 2>/dev/null)"
if [[ -n "$TBA_CODE" && "$TBA_CODE" != "0x" ]]; then
  pass "tba deploy -> eth_getCode(claude TBA) non-empty (account deployed)"
else
  fail "claude TBA has no code after deploy (code='$TBA_CODE', exit $DEP_RC)"
fi
if [[ -z "$PEER_ADDR" ]]; then
  skip "tba exec ($PEER key not present — cannot resolve/assert the recipient delta)"
else
  # Fund claude's TBA a little so it can pay (a self-send from claude's EOA to
  # its own TBA — the TBA must hold $LH to forward it). Idempotent + cheap.
  as send "$CLAUDE_TBA" 0.02 >/dev/null 2>&1 || true
  PEER_LH_BEFORE="$(read_lh "$PEER_ADDR")"
  EXEC_OUT="$(as tba exec "$PEER" 0.01 2>&1)"; EXEC_RC=$?
  note "$(printf '%s' "$EXEC_OUT" | grep -iE '✓|executed|send' | head -1)"
  PEER_LH_AFTER="$(read_lh "$PEER_ADDR")"
  PEER_LH_DELTA="$(delta_wei "${PEER_LH_AFTER:-0}" "${PEER_LH_BEFORE:-0}")"
  note "$PEER \$LH $PEER_LH_BEFORE -> $PEER_LH_AFTER  (delta $PEER_LH_DELTA wei)"
  if [[ "$PEER_LH_DELTA" == "10000000000000000" ]]; then
    pass "tba exec -> $PEER's \$LH rose by exactly 1e16 wei (0.01 \$LH) — the TBA executed a transfer"
  else
    fail "tba exec: $PEER \$LH delta=$PEER_LH_DELTA wei, expected 1e16 (0.01 \$LH) (exit $EXEC_RC)"
  fi
fi

# ===========================================================================
# 10. reputation — the ERC-8004-style attestation rung. claude attests a rating
#     to the PEER with a FRESH workRef each run (so it's a real write, never a
#     dedup no-op — the facet dedups (attester, subject, workRef)). Asserts the
#     on-chain reputation accumulator moved:
#       reputationOf(peerTokenId).count rose by exactly 1
#       reputationOf(peerTokenId).sum   rose by exactly the rating (5)
# ===========================================================================
step "10. reputation — attest (fresh ref) -> count +1 + sum +rating"
PEER_TOKEN="$(id_of "$PEER")"
note "$PEER tokenId: ${PEER_TOKEN:-?}"
if [[ -z "${PEER_TOKEN:-}" || "$PEER_TOKEN" == "0" ]]; then
  fail "reputation: could not resolve $PEER tokenId on-chain"
else
  REP_BEFORE="$(cast call "$DIAMOND" "reputationOf(uint256)(uint256,uint256)" "$PEER_TOKEN" --rpc-url "$RPC" 2>/dev/null)"
  REP_C_BEFORE="$(printf '%s' "$REP_BEFORE" | awk 'NR==1{print $1}')"
  REP_S_BEFORE="$(printf '%s' "$REP_BEFORE" | awk 'NR==2{print $1}')"
  # A fresh ref each run: the current epoch seconds (so the (attester,subject,ref)
  # dedup key is unique per run — a real on-chain attestation, not a no-op).
  REP_REF="$(date +%s)"
  RATING=5
  ATT_OUT="$(as reputation attest "$PEER" "$RATING" --ref "$REP_REF" 2>&1)"; ATT_RC=$?
  note "$(printf '%s' "$ATT_OUT" | grep -E '✓|attested' | head -1)"
  REP_AFTER="$(cast call "$DIAMOND" "reputationOf(uint256)(uint256,uint256)" "$PEER_TOKEN" --rpc-url "$RPC" 2>/dev/null)"
  REP_C_AFTER="$(printf '%s' "$REP_AFTER" | awk 'NR==1{print $1}')"
  REP_S_AFTER="$(printf '%s' "$REP_AFTER" | awk 'NR==2{print $1}')"
  C_DELTA="$(delta_wei "${REP_C_AFTER:-0}" "${REP_C_BEFORE:-0}")"
  S_DELTA="$(delta_wei "${REP_S_AFTER:-0}" "${REP_S_BEFORE:-0}")"
  note "reputationOf count $REP_C_BEFORE -> $REP_C_AFTER (Δ$C_DELTA)  sum $REP_S_BEFORE -> $REP_S_AFTER (Δ$S_DELTA)"
  if [[ "$C_DELTA" == "1" ]]; then
    pass "reputation attest -> count rose by exactly 1"
  else
    fail "reputation count delta=$C_DELTA, expected 1 (attest exit $ATT_RC — was the ref fresh?)"
  fi
  if [[ "$S_DELTA" == "$RATING" ]]; then
    pass "reputation attest -> sum rose by exactly the rating ($RATING)"
  else
    fail "reputation sum delta=$S_DELTA, expected $RATING (attest exit $ATT_RC)"
  fi
fi

# ===========================================================================
# 11. colony — the FULL autonomous cycle (OPTIONAL, E2E_RUN_COLONY=1). `colony
#     run` composes post->claim->work(LLM)->submit->judge(LLM)->accept->payout->
#     attest into one self-driving turn. It makes TWO headless LLM `call`s (work
#     + judge), so it's SLOW and the result/judge TEXT is non-deterministic — we
#     assert the CYCLE, not the text: exit 0 AND the worker's TBA $LH rose by the
#     escrowed reward (the accept settled the payout to the worker's TBA). Off by
#     default (mirrors how `send` is guarded); keep the reward tiny.
# ===========================================================================
step "11. colony — one full autonomous cycle pays the worker (optional)"
if [[ "${E2E_RUN_COLONY:-0}" != "1" ]]; then
  skip "colony (set E2E_RUN_COLONY=1 to exercise; it makes 2 LLM calls — slow — and pays a tiny reward)"
elif [[ ! -f "$(key_path "$WORKER")" ]]; then
  skip "colony ($WORKER key not present — the worker must sign its own claim+submit)"
else
  WORKER_TBA="$(tba_of "$WORKER")"
  note "worker $WORKER TBA: ${WORKER_TBA:-?}"
  COLONY_REWARD="0.01"
  WORKER_LH_BEFORE="$(read_lh "$WORKER_TBA")"
  COLONY_OUT="$(as colony run "e2e probe: reply with the single word pong" --reward "$COLONY_REWARD" --worker "$WORKER" 2>&1)"; COLONY_RC=$?
  note "$(printf '%s' "$COLONY_OUT" | grep -iE '✓|accepted|settl|payout|attest' | tail -2 | tr '\n' ' ')"
  WORKER_LH_AFTER="$(read_lh "$WORKER_TBA")"
  WORKER_LH_DELTA="$(delta_wei "${WORKER_LH_AFTER:-0}" "${WORKER_LH_BEFORE:-0}")"
  note "$WORKER TBA \$LH $WORKER_LH_BEFORE -> $WORKER_LH_AFTER  (delta $WORKER_LH_DELTA wei)"
  if [[ $COLONY_RC -eq 0 && "$WORKER_LH_DELTA" == "10000000000000000" ]]; then
    pass "colony run -> cycle completed (exit 0) AND worker TBA paid exactly 1e16 wei (0.01 \$LH)"
  else
    fail "colony run: exit $COLONY_RC, worker TBA delta=$WORKER_LH_DELTA wei (expected exit 0 + 1e16 reward)"
  fi
fi

# ===========================================================================
# 12. send — OPTIONAL, off by default (E2E_RUN_SEND=1 to enable). A tiny self-
#    send is wasteful (a no-op on net balance, only burns sponsor gas), so it's
#    guarded. When enabled, assert the CLI reports success (the $LH transfer is
#    a self-transfer; balance is unchanged but the tx must land).
# ===========================================================================
step "12. send — tiny self-transfer (optional)"
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
