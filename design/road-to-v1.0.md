# Road to v1.0.0

> Captured 2026-06-27 via a 9-investigator parallel recon (docs · cross-session memory ·
> telemetry · code-health · security · on-chain/economy · infra/ops · product/UX · SDK-API)
> + an adversarial synthesis. The synthesis VERIFIED findings against the live mainnet
> chain — and corrected a phantom "large blocker": `chain.rs:62`'s comment claims the
> economy ladder isn't cut on mainnet, but `postBounty`/`propose`/`vote`/`attest` all
> resolve to real facets on `0x8ab4f3a5…f3a77`. The ladder IS live.

## Framing

localharness is **closer to a safe, honest 1.0 than the design docs imply**: the platform
is already live on Tempo mainnet (chain 4217), the 06-26 audit largely shipped (0.57/0.58),
and the economy ladder is cut. 1.0 here = the deliberate **public-launch moment + an SDK
semver freeze** on the already-live platform — not a giant feature/contract push. The real
work is four buckets:

- **(A) Process/QA gates that have never run** — a structured fresh-user beta + a self
  golden-path walkthrough on a real phone, incl. a *cold* keyless-relay onboarding.
- **(B) Real-money safety gates** — H2 money-transmitter decision, `fiatLockSecs=0`
  (clawback dead), a per-identity spend-velocity cap the proxy admits it lacks, a Stripe
  LIVE-key E2E.
- **(C) SDK API-freeze** — cheap but mandatory *before* 1.0 (the fixes are themselves
  breaking, so they can't be done after).
- **(D) Honest scoping decisions** — iOS, the paid-only front door, spec/reality drift.

The planned **pre-1.0 RESET** is a real lever: it makes the I6/I7 re-cuts, dead-weight
removal (`chargeFromWallet`/`SessionFacet`/`fiatLock`), the off-chain notifications move,
sybil/`fiatLockSecs` economics, and one-canonical-address-set all **free** (do them in a
fresh deploy instead of N money-critical live `diamondCut`s).

Explicitly **NOT** 1.0 blockers despite being DoD checkboxes: forkable templates, the
autonomy dial (spec defaults it off), multiplayer/teams/Gemma, the economy pilot (spec
routes it to Track B).

## Launch blockers

1. **Fresh-user beta + golden-path walkthrough on a real phone (incl. cold relay onboarding)**
   — _medium._ The single biggest real gap and the spec's own hard gate (`beta-plan.md`):
   no beta has run; the phone golden-path "has not been done". The 06-26 browser-E2E ran as
   *existing funded* identities, not a cold stranger; visitor-pay / multiplayer / `?rpc=1` /
   app-io remain unverified. Covers the one CLAUDE.md-pending seam (cold relay onboarding).
2. **Panic-hardening + recovery path for every golden-path failure** — _small (mostly
   verification)._ Code-health found this substantially met in source (no non-test
   `todo!`/`unimplemented!`, panics guarded). Walk the path; confirm RPC-down / claim-fail /
   compile-error / out-of-gas / bad-key each render a readable error with a way forward.
3. **H2 money-transmitter / MTL decision before selling $LH for cards** — _large, maintainer-
   owned, at-reset._ $LH is open-loop today (`settle`/`withdrawCredits`/`send_lh`) → fiat-bought
   transferable $LH is likely stored-value/MSB territory. Clean fix: make fiat-origin $LH a
   permanently non-transferable spend-on-compute balance class (free at the reset).
4. **Stripe LIVE-key payment→mint E2E + webhook backstop + fix `fiatLockSecs=0`** — _medium,
   at-reset._ Proven with TEST keys only; a real card→mint never confirmed; `fiatLockSecs()=0`
   verified on mainnet (clawback non-functional); prior charged-card-no-mint history.
5. **Per-identity spend-velocity cap / circuit breaker on the credit path** — _large._
   `proxy/README.md` admits there is NO per-identity sliding-window limit — a leaked auth
   key can drain an identity's whole $LH balance. Acceptable for an invited beta; a direct
   bill-shock/abuse exposure for public traffic. Needs a stateful (KV) counter.
6. **Lock the SDK public API for the semver promise** — _medium, NEW/undocumented._ These
   CANNOT be done after 1.0 (applying them is breaking): (1) `#[non_exhaustive]` on growing
   enums (`Error`, `BuiltinTool`, `Step*`, `StreamChunk` — ZERO crate-wide today); (2) seal
   config struct fields (`AgentConfig`/`GeminiBackendConfig` all-pub); (3) demote
   rustlite/soliditylite internals (~15.5K SLOC) to `pub(crate)`; (4) seal the provider wire
   modules. (3)+(4) also erase ~630 of 790 missing-doc warnings at zero consumer cost.
7. **Reconcile "what 1.0 means" + canonicalize ONE address set** — _small, at-reset,
   NEW/undocumented._ `launch-1.0.md` still says "on testnet at 0.x" though mainnet is live at
   0.58.0; CLAUDE.md's "Canonical addresses (post-reset)" table lists the *testnet* diamond
   under a mainnet-sounding heading; `chain.rs:62`'s comment is false. Pin one set from
   `docs_manifest.rs` at the reset.
8. **Deliberate, messaged decision on iOS + the paid-only front door** — _medium,
   NEW/undocumented._ **iOS half RESOLVED 2026-07-07**: the "not available on iOS"
   gate is removed — WebKit OPFS writes broker through a worker
   `createSyncAccessHandle` path (`web/opfs-worker.js`; root cause was Safari
   lacking `createWritable` pre-Safari-26). Residual: real-device verification on
   an actual iPhone. The paid-entry-only ($2) front-door question still stands.

## Should-have (strongly wanted, not strictly blocking)

- **Admin-panel complete refactor** (telemetry #36) — the admin surface (`#header-admin-panel`
  / `.admin-dialog`) is a dense settings grab-bag that clashes with the chat-native design
  (maintainer: "horrendous artifact that doesn't fit"); re-think it chat-native + coherent
  (keep the no-DOM / one-box / centered-overlay rules). A UX-coherence gate for a
  not-embarrassing launch.
- Notifications fire FAILING on-chain push-sub writes on mainnet (address-keyed path bypasses
  the relay) → confusing failed-tx in the bell (frank #27/#32). Move push subs off-chain;
  retire the on-chain path at the reset (also closes a plaintext-bearer-cap privacy hole).
- Diagnose the recurring mainnet sponsored-write "insufficient funds for gas" (#32 open) — the
  zero-balance promise is the core UX; needs a deliberate mainnet zero-balance smoke + the
  failing selector added to tx-error telemetry. (Reset does NOT fix this.)
- Fund the mainnet sponsor float (~1.46 USDC.e today, no monitor) + add float/meter-key/RPC/
  cron monitoring + alerting on the one SPOF.
- Provision dedicated GitHub PATs (one shared token backs 6 subsystems = de-facto DB SPOF) +
  a global/per-IP cap (per-address is defeated by free keypair rotation).
- Deploy-time assertion that prod proxy is NOT on testnet defaults (the FEE_TOKEN outage class)
  + commit the proxy lockfile (reproducible money-service builds).
- Focused real-money security pass once economy facets + scoped keys settle — confirm M4 (dust
  buys a 20-$LH Opus call via `min(cost,avail)`), I5 (HTML faces served no-CSP = phishing),
  L6/L47 (welcome amplifies a meter-key gas drain), L9 body-binding.
- Supply-chain CVE gate (`cargo audit`/`cargo-deny`) + `cargo-semver-checks` API-diff in CI.
- Wire the `circulatingSupply()`-vs-Stripe reconciliation alarm (issuer-leak detector; H1 gate).
- Fix docs.rs landing inaccuracies ("6 fs tools" → 8; stale "Agent gated behind native") +
  write a STABILITY/MSRV scope statement.
- Re-cut I6 (fail-closed MultiSigner) + I7 (chain-bound Signaling); **write the I7 client
  digest binding** (signaling.rs still signs the old preimage — independent of the reset) +
  a fresh same-selector `ReplaceSignalingFacet` script.

## Post-1.0 (1.x track)

Forkable templates + one-click fork · autonomy dial / scoped runtime keys (spec defaults off;
the spend-velocity *cap* is pulled forward) · protocol-surface spec doc · x402 `settleUpto` +
metering flip (margin, not safety) · economy pilot · multiplayer/teams E2E + TURN + SessionRoom
ph2 · Local Gemma · OpenAI keep-or-delete · MCP SSE/HTTP · cleanup-cut dead weight (free at
reset) · derived tool/CLI doc lists · `?explore` directory UI entry · public-face fast paint.

## Newly surfaced (undocumented finds)

- **Phantom blocker killed:** the economy ladder IS cut on mainnet (selectors verified live);
  `chain.rs:62`'s comment is stale/false and propagated into 3 recon reports as the #1 blocker.
- SDK API stability completely unprepared (zero `non_exhaustive`, all-pub config, ~15.5K SLOC
  needlessly `pub`) — the cheapest high-leverage 1.0 work, in no design doc.
- `fiatLockSecs()=0` live (verified) → clawback non-functional today.
- Zero proxy monitoring/alerting on the SPOF; one shared PAT backs 6 subsystems; metering
  correct-by-env-only with no startup assertion; single unmonitored RPC.
- No public open-issue tracker; ALL recent feedback from two internal dogfooders → empty
  backlog means "under-tested by outsiders", not "de-risked".
- No `SECURITY.md` / disclosure policy / key-rotation+incident runbook for a seed-custodying,
  money-moving crate.
- `registrationCost()=1 $LH` live (verified) while CLAUDE.md/README say register is FREE — the
  sybil gate is already on, undocumented as such.
- `?explore` agent directory exists but has NO UI entry point (marketplace invisible).

## Open decisions (maintainer-owned)

- **H2:** legal read on transferable $LH mechanics, OR commit to non-transferable
  fiat-origin $LH at the reset. Gates selling credits for money.
- **What 1.0 means** now that mainnet is live: pin "1.0 = public launch + SDK freeze on the
  live platform, DoD actually met" (vs the stale "1.0 = go to mainnet").
- **iOS:** genuinely fix the WebKit OOM/OPFS path, or message it as out-of-scope.
- **Front door:** keep the $2 paid-only entry (sybil gate) or add a free demo turn.
- **Sybil sufficiency:** is `1 $LH` reg cost + $2 paywall + relay onboarding caps enough, or is
  stake/proof-of-persistence needed? (Cheap to redo at the reset.)
- **SDK stability tiers:** is `feature=wallet`'s `registry::` surface covered by the 1.0
  promise, or a documented separate track?
- **Multiplayer at 1.0?** If headlined, TURN must be provisioned; else keep dormant.
- **OpenAI backend:** keep-for-parity (fix L22/L23) or delete the parked code.
- **Versioning:** confirm 1.0.0 is reserved for the launch moment → freeze + beta + safety
  gates land BEFORE the tag.

## Recommended sequence

0. **Reconcile reality + decide** (days, mostly non-code): fix `chain.rs:62` + the CLAUDE.md
   address table; reconcile `launch-1.0.md` vs live mainnet + re-tick its DoD; make the open
   decisions the rest depends on. Unblocks everything, ~free.
1. **SDK API freeze** (1-2 days, source-only, chain-independent): the four mandatory stability
   changes + docs.rs fixes + STABILITY/MSRV statement + a `Connection` custom-impl example +
   `cargo-semver-checks`/`cargo-audit` in CI. Do early so the public surface stops moving.
2. **Money & abuse safety gates:** spend-velocity cap; Stripe LIVE E2E + webhook + `fiatLockSecs`
   + circulating-supply alarm; H2 in code; fund sponsor float; dedicated PATs + global cap;
   prod-not-testnet assertion + lockfile; proxy monitoring/alerting; `SECURITY.md` + runbook.
3. **THE RESET** (one atomic deploy): fresh diamond + token + full audited ladder + I6/I7 fixes,
   drop dead weight, set reg/fiatLock/sybil economics per Phase-0, push subs off-chain; move
   crate + bundle + proxy env to new canonical addresses atomically + regenerate docs; fix the
   I7 client digest + notifications-off-chain in the same window.
4. **Golden-path + fresh-user beta on the post-reset platform:** self-walk twice on desktop AND
   a real phone (cold onboarding → claim → build → publish → share → visitor-on-phone),
   panic-harden every on-path failure, diagnose #32. Exit: 3+ consecutive strangers complete
   the magic moment unaided on a phone.
5. **Focused security pass + ship 1.0:** targeted real-money pass (M4/I5/L6/L47/L9), close
   beta blockers, tag `1.0.0` + publish + marketing apex/quickstart/showcase. Defer the 1.x
   track.
