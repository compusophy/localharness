# Relay allowlist gaps + found_company overrun

**UPDATE 2026-07-31 (user greenlit "do all"): #1 + #3 SHIPPED LIVE. #2 is the
chunking follow-up (in progress).**
- **#1 (7 relay selectors): DONE + LIVE** — added to `sponsor.ts DIAMOND_WRITE_SIGS`
  (proxy deployed) + `signer.rs` (web bundle `34134d8f0dd5` live). Party/validation/
  governance tools no longer 403. Residual: an on-chain E2E smoke (form a party →
  join → complete) not yet run — needs live setup.
- **#3 (funded-gate setMetadata message): DONE + LIVE** (same proxy deploy).
- **Prevention drift test (below): SHIPPED** — `tests/relay_allowlist_parity.rs`
  pins the registry `*_sponsored` fn→signature table (enumeration-guarded: a new
  `*_sponsored` fn fails the test until classified) and asserts every submitted
  diamond signature appears in BOTH `signer.rs diamond_signable_selectors()` and
  `sponsor.ts DIAMOND_WRITE_SIGS`, plus exact signer↔relay mirror parity. First
  run caught `subscribe(uint256)`/`unsubscribe(uint256)` present in sponsor.ts
  but missing from signer.rs — fixed in the same commit.
- **The 504 fix bundled in the same session BROKE inference** (edge→node = 500 on
  every request) and was reverted — see `design/proxy-504-fix.md`. It is NOT a
  config flip. The relay/price/msg changes were unaffected.
- **#2 (found_company chunking + the batch-tool chunking, #85): still the follow-up.**
  The stale-prefund-field lie was fixed separately (commit b86c4288, live).

---
Original staging note below (kept for the verified detail).

**Status: VERIFIED, staged, NOT applied.** These fixes touch the security-sensitive
sponsor-relay allowlist and/or need a `cd proxy && vercel --prod`, so they ride the
same attended proxy session as `design/proxy-504-fix.md` (the `maxDuration` 504 fix
and the `claude-sonnet-5` price row) — one deploy, with review. Found by the
"own-text-lies" audit (2026-07-31), each confirmed on both sides + re-grepped.

## 1. Relay allowlist gaps — advertised on-chain tools that 403 on mainnet (HIGH)
Three live, session-wired tool families submit diamond selectors missing from BOTH
`proxy/api/sponsor.ts` `DIAMOND_WRITE_SIGS` **and** `src/app/signer.rs`
`diamond_signable_selectors()`. On mainnet every call 403s `LH_RELAY_SELECTOR`
(`sponsor.ts checkAllowlist`), blaming the caller; escrowed `$LH` is stranded and
the rung-4 payout can never fire. Grep-confirmed present: `executeWeighted`,
`formParty`, `fundParty`, `stakeValidation`, `resolveValidation`. **Missing (add
these):**

| selector | tool (session-wired) | facet | impact if missing |
|---|---|---|---|
| `execute(uint256)` | `execute_proposal` (`voting.rs:181`) | VotingFacet:260 | rung-4 guild treasury payout DEAD (propose+vote work, execute dies) |
| `joinParty(uint256)` | `join_party` (`party.rs:145`) | PartyFacet:237 | members can't consent → party un-settleable |
| `completeParty(uint256)` | `complete_party` (`party.rs:203`) | PartyFacet:335 | party can't settle → escrow stranded |
| `disbandParty(uint256)` | `disband_party` (`party.rs:222`) | PartyFacet:399 | party can't dissolve → escrow stranded |
| `challengeValidation(uint256)` | `challenge_validation` (`validation.rs:116`) | ValidationFacet:232 | Challenged state unreachable → `resolveValidation` (allowlisted!) can never fire |
| `reclaimStake(uint256)` | `reclaim_stake` (`validation.rs:154`) | ValidationFacet:329 | unchallenged validator can't reclaim → funds stranded |
| `reclaimUnresolved(uint256)` | `reclaim_unresolved` (`validation.rs:170`) | ValidationFacet:362 | stale validation stake stranded |

**Fix:** add each string to `DIAMOND_WRITE_SIGS` (`sponsor.ts`, near
`executeWeighted`/`formParty`/`stakeValidation`) AND to `diamond_signable_selectors()`
(`signer.rs`, ~766). These are legitimate facet fns gated by their own access
control (e.g. `execute` only runs a PASSED proposal), so sponsoring them restores
intended, safe functionality — not a new abuse vector. Then `cd proxy && vercel
--prod` (relay) + a web deploy (signer.rs).

**Root cause / prevention:** no parity guard between the two allowlists + the
registry's `*_sponsored` selectors. Add a drift test: enumerate the selectors every
`*_sponsored` registry fn submits, assert each appears in BOTH
`diamond_signable_selectors()` and (read as text) `sponsor.ts DIAMOND_WRITE_SIGS`.
Without it this class recurs on every new facet tool.

## 2. found_company over-runs the 8-call relay cap on its DEFAULT config (HIGH)
`company.rs` STEP 4 (462-483) batches ALL role setup into ONE
`run_sponsored_tempo_call`. `build_actor_setup` emits 1 call/persona + 2/prefund
(createTBA + transfer). Default 7 roles + `prefund_each` = 21 calls; any prefund
with ≥3 roles = 9+; the relay hard-rejects >8. On the caught Err it sets
`persona_set=false` but **leaves `prefunded_lh`/`tba` set (company.rs:437-441,
written before the tx)** — so the manifest reports an org staffed + prefunded when
NOTHING landed. STEP 3 has the same shape: >7 roles = >8 register calls (incl. the
approve), aborting AFTER the guild is created (a live guild with zero roles).

**Fix (needs care — money/on-chain multi-tx, verify with a live smoke):** chunk
STEP 3 registrations AND STEP 4 setup into ≤8-call sponsored txs (the same
`chunking` primitive the batch tools need — `design`-level follow-up covering
telemetry #85), clear `prefunded_lh`/`tba` (not just `persona_set`) on a chunk
failure, and don't create the guild before role registration is known to fit. This
is the same chunking follow-up as #85 — do them together.

## 3. Funded-caller >4096B setMetadata error blames the wrong cause (LOW, proxy)
`sponsor.ts:617` returns `LH_RELAY_FUNDED` "self-pay your fees" for a funded
caller's >4096B setMetadata self-edit. But the real disqualifier is SIZE
(`isGateExemptCall` relays ≤4096B same-caller edits fine, 314-321), and "self-pay"
is impossible on mainnet — an agent holds only `$LH`, never the fee token (the
file's own comments say so). Narrow trigger (personas/lessons rarely >4KB).
**Fix:** in the funded-gate branch, detect setMetadata-over-4096 and return a
size-specific message (`"setMetadata payload N B exceeds the 4096-byte relay
self-edit cap — shorten it"`); drop the "self-pay your fees" clause on mainnet.

## The attended proxy-session bundle (one `cd proxy && vercel --prod`)
1. `design/proxy-504-fix.md`: `maxDuration` (drop `runtime:'edge'`).
2. `_prices.ts`: `claude-sonnet-5` row (already committed — inert until deploy).
3. #1 here: the 7 relay selectors (sponsor.ts + signer.rs) + the parity drift test.
4. #3 here: the funded-gate setMetadata message.
Then smoke each (a governance execute, a party join, a validation challenge) on a
funded test identity. #2 (found_company / chunking) is a separate coding follow-up,
not a proxy change.
