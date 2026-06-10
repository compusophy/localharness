# Browser smoke drive — the canonical click-path

Re-runnable journey for driving the live deploy in a real Chrome tab
(Claude-in-Chrome MCP or a human). First proven 2026-06-10. Run it after any
deploy that touches `src/app/`, before claiming UX works.

**Pre-flight (CLI, ~2 min)**

```sh
localharness whoami claude                 # identity resolves
localharness credits --as claude           # wallet + meter balances
localharness invite create --as claude --amount 5   # fresh invite code for step 2
curl -s https://localharness.xyz/llms.txt | head -3 # prod deploy is live
```

Keys live at `~/.localharness/keys/` (or `$LOCALHARNESS_HOME`); the CLI is
`cargo run --features wallet --bin localharness --` from the repo root.

**Journey** — screenshot + console (`error|warn|fail`) + network at every step.
Hard reload (`Ctrl+Shift+R`) after any redeploy — a loaded tab never sees new
wasm (session staleness).

| # | Step | Expect |
|---|------|--------|
| 1 | `localharness.xyz/` fresh | roster (returning) or hero + claim form (fresh). NO wallet silently created. |
| 2 | `/?invite=<code>` | status line acknowledges the redeem (`#status` node); escrow clears on-chain (`invite list --as claude` shows it gone). |
| 3 | claim form → type name → CREATE | sponsored mint lands; redirect to `<name>.localharness.xyz` studio. |
| 4 | studio: send "hello" in terminal | turn streams; stop-square during stream; markdown renders; no console errors. |
| 5 | ADMIN → ACCOUNT | name/owner/wallet/balance/credits paint; version footer matches the release. |
| 6 | ADMIN → AGENT: set prompt + SAVE | "✓ saved + published on-chain"; `whoami <name>` shows `persona published`. |
| 7 | ADMIN → AGENT: x402 price `0.1` + SAVE | "saved" (decimals MUST work — regression: u128-only parse). |
| 8 | `/?view=public` | public face paints (directory/app/html per choice). |
| 9 | visitor: `claude.localharness.xyz` | directory face, MAIN badge, read-only (no studio leak). |
| 10 | CLI: `call --as claude --pay 1 <name> "ping"` | answer AND settle tx succeeds; target TBA balance += 1e18 (`cast call $LH balanceOf <tba>`). Regression: 400k gas cap reverted cold-TBA settles. |

**Known traps**

- Sponsor "fee-token LOW" warning: AlphaUSD has 6 decimals — verify with
  `cast call` before believing any balance warning.
- A reverted settle still returns the agent's ANSWER (CLI settles after the
  reply) — always check the TBA balance, not just the reply.
- This Chrome profile holds the dev's real seed — NEVER clear OPFS/site data,
  never test "create identity" here; cold-start needs a throwaway profile.
- Cleanup: release smoke-test names after the run so they don't pollute
  public directories (the e2e-guild-* litter lesson).
