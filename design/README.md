# design/

This folder is the project's design record. Most of what was once "the plan"
is now **shipped and live** — the authoritative map of the running system is
[`../CLAUDE.md`](../CLAUDE.md) (see *What's pending* and *The on-chain stack*)
and the per-version log is [`../CHANGELOG.md`](../CHANGELOG.md).

What lives here:

- **Forward / open design** (`design/*.md`) — docs whose subject is **not yet
  built**. Each carries a `STATUS: open` header. These are the only docs worth
  reading as a roadmap.
- **Shipped design** (`design/shipped/*.md`) — the architectural reasoning
  behind capabilities that are now live, kept for the *why*. Each carries a
  `STATUS: SHIPPED` header naming when/where it went live. These are history,
  not plans.

---

## What's built (one line each → the doc that designed it)

- **Agent scheduling** — tab-free recurring jobs on `ScheduleFacet` + a
  Vercel-Cron worker, multi-agent ping-pong, cross-tick recursion
  (`scheduleChildJob`), per-tick spend caps, and the `/goal` ralph-on-chain
  self-terminating loop. → [`shipped/agent-scheduling.md`](shipped/agent-scheduling.md)
- **Economy ladder (rungs 1–4)** — bounty board (`BountyFacet`), guilds
  (`GuildFacet` + TBA treasuries), DAO voting (`VotingFacet`), reputation
  (`ReputationFacet`), and DAOs-of-DAOs ("turtles", proven live at the contract
  level). → [`shipped/agent-coordination.md`](shipped/agent-coordination.md)
- **Module revenue + on-chain reputation/attestations** — proof-of-transaction
  attestation gate; value flowing one hop down the call graph.
  → [`shipped/economy-reputation.md`](shipped/economy-reputation.md)
- **The colony** — feedback → GitHub issue → escrowed `$LH` bounty → agent PR →
  verify gate → merge → on-chain settlement to the worker's TBA (colony-authored
  code shipped in 0.32.0). Designed in
  [`autonomous-loop.md`](autonomous-loop.md) (still `open` — the jailed QA
  execution surface + autonomy dial are partial).
- **MAIN identity + multi-device** — `MainIdentityFacet` + the multi-signer
  ERC-6551 `MultiSignerAccount`; device linking now via QR seed-adoption.
  → [`shipped/main-identity.md`](shipped/main-identity.md)
- **User-funded refundable invites** — bearer `InviteFacet` codes (create /
  accept / reclaim), the growth on-ramp. → [`shipped/invites.md`](shipped/invites.md)
- **Sponsored writes (paymaster)** — superseded by Tempo's native AA (tx `0x76`,
  embedded `fee_payer` sponsor). The original paymaster analysis is obsolete and
  was removed; the live wire format lives in `../CLAUDE.md` and
  `../examples/tempo_tx_live.rs`.
- **Model-agnostic backends** — the `Connection`/`ConnectionStrategy` seam with
  Gemini + Anthropic + Mock backends and `$LH`-proxy routing to either provider.
  Designed in [`model-agnostic.md`](model-agnostic.md) (still `open` — local
  WebGPU finish + own coding model, Phases D–F).
- **rustlite + agent-authored cartridges** — the Rust-subset → wasm compiler and
  `create_and_publish_app`. Designed in
  [`agent-writes-rust.md`](agent-writes-rust.md) (still `open` — the
  neural-net-as-compiler north star).

For the full historical sequencing that got us here, see
[`shipped/roadmap.md`](shipped/roadmap.md).

---

## Forward / open design

Genuinely-unbuilt docs, each with a `STATUS: open` header:

- [`launch-1.0.md`](launch-1.0.md) — the **mainnet 1.0** launch spec (real `$LH`
  value + Stripe + sybil gate + sponsor rewrite). The project is still on Tempo
  Moderato **testnet** at `0.x`; `1.0.0` is reserved for this moment.
- [`beta-plan.md`](beta-plan.md) — the operational private-beta plan that runs
  on testnet as ordinary `0.x` bumps before the 1.0 mainnet launch.
- [`model-agnostic.md`](model-agnostic.md) — backends D–F: finish the in-browser
  local model (Gemma/WebGPU, native-validated but not shipped to the browser
  app), an own coding model, and decentralized compute.
- [`agent-writes-rust.md`](agent-writes-rust.md) — the long-arc dream beyond the
  shipped rustlite compiler: neural-net-as-compiler / a model trained to write
  Rust; cartridge composition + macros.
- [`host-compose.md`](host-compose.md) — framebuffer-resident cartridge
  composition (a window manager over one shared RGBA buffer, no iframes). The
  concept is sound; the `host::compose` ABI is unbuilt. Subsumes the open
  cartridge host-import / rich-context-cartridge frontier.
- [`autonomous-loop.md`](autonomous-loop.md) — the agent "immune system". The
  colony loop shipped, but the jailed `qa_tools` execution surface, the
  three-rung autonomy dial, and the unattended fix-agent (`propose` rung)
  remain open.

### Open items tracked in CLAUDE.md *What's pending* (no dedicated doc)

- **Stripe / fiat MPP** — a fiat agent-payments rail beside the live x402 `$LH`
  path (touched by `launch-1.0.md` and `economy-reputation.md`).
- **ERC-8004 validation staking** — validators stake to re-execute claims. The
  attestation half ships (`ReputationFacet`); the stake-escrow / slashing half
  does not (designed in `shipped/economy-reputation.md` §validation).
- **At-rest OPFS encryption** — a wallet-derived symmetric key over OPFS.
- **P2P teams** — the 2-device E2E test, SDP sealing, mutable shared-FS, team UI
  (`SignalingFacet` + `TeamFacet` are cut and live).
- **DAOs-of-DAOs UX / party rung** — nesting works at the contract level; the
  browser UX and the ad-hoc "party" squad rung are unbuilt
  (`shipped/agent-coordination.md` §rung-2 + §recursion).
- **Public colony board, inter-agent notify consent, off-chain telemetry,
  lessons auditing/deprecation** — operational follow-ups with no dedicated doc.
