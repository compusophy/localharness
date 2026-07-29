#!/usr/bin/env bash
# Fast, OFFLINE smoke test of the `localharness` CLI — run before a release.
#
# Verifies the binary builds and its core, network-free commands behave: help,
# version, unknown-command exit code, the cartridge compile gate (valid
# cartridge compiles; an entry-less one is rejected), and every command
# family's OFFLINE usage-error path — bad/missing args print usage to STDERR
# and exit 2 BEFORE any signer/RPC work (incl. the `release` typed-confirmation
# guard). No network, no on-chain writes, no $LH — safe to run anytime. The
# live commands (create/call/publish/whoami/list) need an identity + chain and
# are intentionally NOT exercised here.
set -euo pipefail
cd "$(dirname "$0")/.."

BIN=(cargo run -q --features wallet --bin localharness --)
pass() { echo "  ok — $1"; }
fail() { echo "  FAIL — $1"; exit 1; }

# Assert a usage error: the command must exit 2 (never reaching signer/RPC
# work) and its output must contain <pattern>. Usage lines print to STDERR,
# so capture with 2>&1.
usage_err() { # usage_err <label> <pattern> <arg…>
  local label="$1" pat="$2" out rc=0; shift 2
  out="$("${BIN[@]}" "$@" 2>&1)" || rc=$?
  [ "$rc" -eq 2 ] || fail "$label (exit $rc, want 2): $out"
  echo "$out" | grep -q -- "$pat" || fail "$label (no '$pat' in: $out)"
  pass "$label"
}

echo "[1/22] --version prints a version"
"${BIN[@]}" --version | grep -qE 'localharness [0-9]+\.[0-9]+\.[0-9]+' && pass "version" || fail "version"

echo "[2/22] help lists the create command"
"${BIN[@]}" help | grep -q 'localharness create' && pass "help" || fail "help"

echo "[3/22] unknown command exits non-zero"
if "${BIN[@]}" definitely-not-a-command >/dev/null 2>&1; then fail "unknown cmd exited 0"; else pass "unknown cmd rejected"; fi

echo "[4/22] a valid cartridge compiles"
"${BIN[@]}" compile examples/cartridges/bitmask.rl >/dev/null && pass "compile bitmask.rl" || fail "compile bitmask.rl"

echo "[5/22] an entry-less cartridge is rejected"
tmp="$(mktemp)"; printf 'fn helper(n: i32) -> i32 { n + 1 }\n' > "$tmp"
if "${BIN[@]}" compile "$tmp" >/dev/null 2>&1; then rm -f "$tmp"; fail "no-entry cartridge accepted"; else rm -f "$tmp"; pass "no-entry rejected"; fi

echo "[6/22] release without --confirm is refused (typed-confirmation guard)"
usage_err "release guard" '--confirm must exactly match' release foo

echo "[7/22] call without a message exits 2"
usage_err "call usage" 'usage: localharness call' call some-target

echo "[8/22] mcp-call without args exits 2"
usage_err "mcp-call usage" 'usage: localharness mcp-call' mcp-call

echo "[9/22] schedule without args exits 2"
usage_err "schedule usage" 'usage: localharness schedule' schedule

echo "[10/22] status with too many args exits 2"
usage_err "status usage" 'usage: localharness status' status x y

echo "[11/22] bounty without a subcommand exits 2"
usage_err "bounty usage" 'usage: localharness bounty' bounty

echo "[12/22] guild without a subcommand exits 2"
usage_err "guild usage" 'usage: localharness guild' guild

echo "[13/22] vote without a subcommand exits 2"
usage_err "vote usage" 'usage: localharness vote' vote

echo "[14/22] invite without a subcommand exits 2"
usage_err "invite usage" 'usage: localharness invite' invite

echo "[15/22] reputation without a subcommand exits 2"
usage_err "reputation usage" 'usage: localharness reputation' reputation

echo "[16/22] tba without a subcommand exits 2"
usage_err "tba usage" 'usage: localharness tba' tba

# Phase-2 nested-division wrappers: `--tba <subguild>` routes through the
# sponsored tba-execute path. A missing positional must still fail OFFLINE (exit
# 2, usage to STDERR) before any RPC — proving the flag is parsed, not swallowed.
echo "[17/22] guild accept --tba <name> without a guildId exits 2"
usage_err "guild accept --tba usage" 'guild accept' guild accept --tba subguildx

echo "[18/22] vote cast --tba <name> <id> without a ballot exits 2"
usage_err "vote cast --tba usage" 'vote cast' vote cast --tba subguildx 7

# Agent-state commands (telemetry #78): a missing <name> must fail OFFLINE,
# before any RPC read — these are the terminal's only write path for what an
# agent has learned, so their arg gates matter.
echo "[19/22] lessons without a name exits 2"
usage_err "lessons usage" 'usage: localharness lessons' lessons

echo "[20/22] skills without a name exits 2"
usage_err "skills usage" 'usage: localharness skills' skills

echo "[21/22] state without a name exits 2"
usage_err "state usage" 'usage: localharness state' state

echo "[22/22] acp with a dangling --model exits 2"
usage_err "acp usage" 'usage: localharness acp' acp --model

echo "ALL SMOKE CHECKS PASSED"
