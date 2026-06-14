# `contracts/` — Localharness on-chain registry

The **EIP-2535 Diamond** under `src/{Diamond,facets,interfaces,
libraries,upgradeInitializers}/` is the LIVE deployment:
`0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c` on Tempo Moderato
(chain 42431) — what `src/registry/mod.rs::REGISTRY_ADDRESS` in the
Rust crate reads. New capability lands as facets cut into that one
stable address without redeploying-the-world each time: identity +
ERC-721 + ERC-6551, credits/sessions/x402 payments, scheduling,
bounties — and, live since 0.30.0, ERC-8004-flavored reputation
(`ReputationFacet`) plus guild DAO governance (`GuildFacet` +
`VotingFacet`). The ERC-8004 *validation* half (validator stake
escrow) now exists as `ValidationFacet` — source-complete + tested,
awaiting its cut (see its section below).

The flat `LocalharnessRegistry` monolith at
`src/LocalharnessRegistry.sol` (~110 lines) is HISTORICAL reference
only — the diamond's predecessor. Its pre-reset deployment was
abandoned in the 2026-06-01 full reset (every prior address dropped;
see CLAUDE.md "Canonical addresses"); nothing reads it.

> Facet SOURCE here can be ahead of the facets CUT on the live
> diamond: a source fix takes effect only on a future re-cut
> (`FacetCutAction.Replace`). Such gaps are called out in the facet
> sections below.

## Deploy (Tempo Moderato testnet)

The canonical diamond is already deployed (address above). These
steps are for a FRESH deployment (a new testnet, or post-reset):

- `foundry` installed (`forge --version` works).
- An EVM private key with some testnet TMP for gas.
- `forge-std` installed: `forge install foundry-rs/forge-std --no-git`
  from this directory (one-time).

```sh
cd contracts
export EVM_PRIVATE_KEY=0x...your-funded-testnet-key
forge script script/DeployDiamond.s.sol \
    --rpc-url tempo_moderato \
    --private-key $EVM_PRIVATE_KEY \
    --broadcast
```

Prints the diamond address + each facet's address. Bake the
**diamond** address into `src/registry/mod.rs::REGISTRY_ADDRESS`,
rebuild + deploy the wasm bundle, then cut the remaining facets via
their `script/Add<Facet>.s.sol` scripts.

## Diamond architecture

The diamond proxy (`src/Diamond.sol`) holds storage and dispatches
every external call to the facet that owns its selector. Selectors
are wired in/out via `diamondCut` — the only way to upgrade.

```
contracts/src/
├── Diamond.sol                       proxy: fallback delegatecalls
│                                     to the facet that owns msg.sig
├── facets/
│   ├── DiamondCutFacet.sol           owner-only upgrade entry point
│   ├── DiamondLoupeFacet.sol         introspection (facets, selectors,
│   │                                 supportsInterface)
│   ├── OwnershipFacet.sol            EIP-173 owner() + transferOwnership
│   ├── LocalharnessRegistryFacet.sol register / transfer / setMetadata
│   │                                 / isTaken / ownerOfName / ...
│   ├── PartyFacet.sol                ad-hoc squads — consent-gated
│   │                                 bps split of an escrowed pot
│   ├── GuildFacet.sol                agent guilds — members/roles +
│   │                                 pooled $LH treasury escrow
│   ├── VotingFacet.sol               guild DAO — propose / vote /
│   │                                 execute a treasury spend
│   ├── ReputationFacet.sol           attestation-based reputation —
│   │                                 attest / reputationOf / ...
│   └── ValidationFacet.sol           ERC-8004-style validation staking —
│                                     stake / challenge / resolve / reclaim
├── interfaces/
│   ├── IDiamond.sol                  FacetCut + DiamondCut event
│   ├── IDiamondCut.sol               diamondCut(...)
│   ├── IDiamondLoupe.sol             facets / facetFunctionSelectors / ...
│   ├── IERC173.sol                   ownership
│   └── IERC165.sol                   supportsInterface
├── libraries/
│   ├── LibDiamond.sol                THE library — storage slot,
│   │                                 enforceIsContractOwner,
│   │                                 diamondCut implementation
│   ├── LibRegistryStorage.sol        isolated registry storage at
│   │                                 keccak256("localharness.registry.
│   │                                 storage.v1")
│   ├── LibPartyStorage.sol           party storage ("localharness.
│   │                                 party.storage.v1")
│   ├── LibGuildStorage.sol           guild storage ("localharness.
│   │                                 guild.storage.v1")
│   ├── LibVotingStorage.sol          voting storage ("localharness.
│   │                                 voting.storage.v1")
│   ├── LibReputationStorage.sol      reputation storage ("localharness.
│   │                                 reputation.storage.v1")
│   └── LibValidationStorage.sol      validation storage ("localharness.
│                                     validation.storage.v1")
└── upgradeInitializers/
    └── DiamondInit.sol               one-shot init: sets ERC-165 flags
                                      and `nextId = 1`
```

### Adding a new facet (e.g. ERC-721, ERC-8004, ERC-6551 helpers, x402)

1. Write `src/facets/MyNewFacet.sol`. Use the diamond-storage
   pattern for any new state: define `LibMyNewStorage` with a
   `keccak256("localharness.mynew.storage.v1")` slot, never touch
   `LibRegistryStorage` directly.
2. `forge build`.
3. Cut it in via a one-off forge script (see `DeployDiamond.s.sol`
   for the template — same pattern, just one `FacetCut`):
   ```sh
   forge script script/AddMyNewFacet.s.sol \
       --rpc-url tempo_moderato \
       --private-key $EVM_PRIVATE_KEY \
       --broadcast
   ```
4. If the new facet needs initialisation, deploy a one-shot
   `MyNewInit.sol` and pass `(myNewInit, abi.encodeWithSelector(MyNewInit.init.selector))`
   to the cut.

### Upgrading a facet

Same as add, but with `FacetCutAction.Replace`. The selectors map
from the old facet to the new one; storage is preserved.

### Removing a facet

`FacetCutAction.Remove` with `facetAddress = address(0)`. The
selectors are removed from the dispatch table.

## Coordination + trust facets (live since 0.30.0)

Per-facet addresses are deliberately NOT pinned here — facets churn
via `diamondCut`. The diamond address is the only durable handle;
resolve a facet live via `DiamondLoupeFacet` (`facets()` /
`facetAddress(selector)`).

### PartyFacet — ad-hoc squads (escrowed pot, consent-gated split)

Ephemeral squads formed around ONE objective — rung 2 of the
coordination ladder. NOT yet cut on the live diamond (built +
tested; `script/AddPartyFacet.s.sol` is ready). Storage:
`LibPartyStorage` at `keccak256("localharness.party.storage.v1")`.
Bounds: `MIN_TTL = 1 hours`, `MAX_TTL = 90 days`,
`MAX_PARTY_MEMBERS = 16`, `MAX_FUNDERS = 64`,
`MAX_ACTIVE_PER_CREATOR = 32`.

**Membership keys on TOKEN IDS** (unlike GuildFacet's addresses):
each member is an agent identity whose share settles to ITS TBA —
the BountyFacet payout precedent. Lifecycle:

- `formParty(uint256[] memberTokenIds, uint16[] sharesBps,
  uint64 ttlSeconds) → uint256 partyId` — shares MUST sum to exactly
  10000 bps, no zero share, every member a registered identity,
  listed once. Shares are FIXED here, before consent — a joining
  member is signing this exact split. Creator-owned seats
  auto-consent; a fully creator-owned party starts Active.
- `joinParty(partyId)` — consents every seat whose tokenId the
  CALLER owns (`NothingToConsent` otherwise); the last consent flips
  Forming → Active. The GuildFacet consent precedent: no one is
  conscripted into a split.
- `fundParty(partyId, uint128 amount)` — PERMISSIONLESS escrow
  (`transferFrom` funder→diamond, CEI), Forming or Active,
  pre-expiry only. Contributions are ledgered per funder.
- `completeParty(partyId)` — CREATOR-ONLY (the MVP oracle, mirroring
  the bounty poster), Active-only, `now <= expiry`. Splits the pot
  to member TBAs by bps with the REMAINDER to the LAST member —
  payouts sum to the escrow EXACTLY. All TBAs resolved + zero-checked
  before the status flip.
- `disbandParty(partyId)` — creator any time while live; ANYONE once
  `now > expiry` (refunds always go to the FUNDERS, never the
  caller). Every funder gets their exact contribution back. The
  complete/permissionless-disband windows are DISJOINT (the
  InviteFacet discipline).

CEI on every `$LH` move; double-complete / double-disband and
reentrant double-settlement are structurally impossible (terminal
status committed before transfers; reentrant-token probes + a
40-step escrow-conservation fuzz + a split-conservation fuzz in
`test/PartyFacet.t.sol`, 59 tests).

**Views** (all `party`-prefixed — the `bountyTaskOf`-vs-`taskOf`
selector lesson): `getParty(id)`, `partyMembersOf(id)`,
`partySharesOf(id)`, `partyConsentOf(id, tokenId)`,
`partyFundersOf(id)`, `partyContributionOf(id, funder)`,
`partiesOf(creator)`, `partyCount()`, `activePartyCountOf(creator)`,
`liveParties(startAfter, limit)` (index-window paging).

**Events:** `PartyFormed`, `PartyJoined`, `PartyActivated`,
`PartyFunded`, `PartyMemberPaid`, `PartyCompleted`, `PartyDisbanded`.

Cut via `script/AddPartyFacet.s.sol` (15 selectors). No post-cut
config: credits token from the shared CreditsFacet slot; TbaFacet
must already be cut (it is).

### GuildFacet — agent guilds (members, roles, pooled treasury)

Durable on-chain organizations of agents — rung 3 of the
coordination ladder (`design/agent-coordination.md`). Storage:
`LibGuildStorage` at `keccak256("localharness.guild.storage.v1")`;
per-guild member cap `MAX_MEMBERS = 128` (anti-grief bound on the
member enumeration).

**A guild IS an identity.** `createGuild(string name) → uint256
guildId` registers `name` as a normal identity NFT owned by the
caller — it replicates `LocalharnessRegistryFacet.register`'s exact
STORAGE writes against the shared `LibRegistryStorage` slot (an
external self-call would record the DIAMOND as holder), so `guildId`
== the registry tokenId, name validation is the same DNS-label rule,
and the guild's ADDRESS is its ERC-6551 token-bound account
(`guildAddress(guildId)` resolves `TbaFacet.tokenBoundAccount` via a
self-call). The founder is seated as the first Admin.

The "indistinguishable from an ordinary `register`" claim holds for
STORAGE only on the currently-cut facet: the live GuildFacet emits
neither `Transfer(0, owner, id)` (ERC-721 requires it on every mint)
nor `Registered(id, owner, name)`, so event consumers / indexers do
NOT see guild mints. It also skips `register`'s trailing
`registrationCost()` pull — latent today (the cost knob is 0 / not
armed on the canonical diamond) but a free-mint bypass if the gate
is ever armed. BOTH are fixed in source (`GuildFacet.sol` now emits
the two mint events and mirrors `_chargeRegistrationCost`, pinned by
Foundry tests) and take effect on the next re-cut.

**Roles** (`LibGuildStorage.Role`, strictly ordered for `>=`
gating): `None(0)` · `Member(1)` · `Officer(2)` · `Admin(3)`.

- `inviteToGuild(uint256 guildId, address member)` — Officer+ only;
  the invitee must `acceptGuildInvite(uint256 guildId)` themselves
  (consent-gated; member cap enforced on accept).
- `leaveGuild(uint256 guildId)` — any member EXCEPT the sole Admin.
- `setRole(uint256 guildId, address member, uint8 role)` — Admin
  only; promote / demote / evict (`role = 0`). Seating a brand-new
  member directly respects the cap and clears any pending invite.
- **Last-Admin guard:** the sole Admin can neither leave nor
  self-demote nor be demoted/evicted (`LastAdmin`) — a guild can
  never become un-administrable with its treasury frozen forever.

**Treasury = facet-balance escrow** — `guildBalance[guildId]`, `$LH`
physically held IN THE DIAMOND (the same safe pattern as
BountyFacet's escrow; NOT a TBA-execute):

- `fundGuild(uint256 guildId, uint256 amount)` — PERMISSIONLESS
  (anyone, including another guild's TBA); `transferFrom`
  funder→diamond, approve the diamond first.
- `spendTreasury(uint256 guildId, address to, uint256 amount, bytes
  memo)` — Admin-only; routes through the internal `_spend` /
  `_spendCore` precisely so VotingFacet can vote-gate the SAME debit
  core. `memo` is opaque and unstored, carried in the event.
- **CEI on every `$LH` move:** the ledger is committed BEFORE the
  external token transfer, so a hostile re-entrant token re-reads
  the already-debited balance and a second spend reverts
  `InsufficientTreasury` (proven by a reentrant-token probe).

**The recursive property:** membership keys on `address`, never on
EOA-ness — a guild's own TBA is a contract account, so a guild can
be invited into ANOTHER guild → guilds-of-guilds with zero new
machinery.

**Views:** `guildMembersOf(id)` (NOT `membersOf` — TeamFacet already
owns `membersOf(uint256)` and a diamond can't share a selector),
`roleOf(id, member)`, `isGuildMember(id, member)`,
`treasuryBalanceOf(id)`, `guildAddress(id)`, `guildName(id)`,
`guildsOf(member)`, `isGuild(tokenId)`, `guildCount()`.

**Events:** `GuildCreated`, `GuildInvited`, `GuildJoined`,
`GuildLeft`, `RoleSet`, `GuildFunded`, `TreasurySpent` — plus, from
the next re-cut, the mint mirrors `Registered` + `Transfer`
(identical signatures to the registry facet's, so identical topic0).

Cut via `script/AddGuildFacet.s.sol` (16 selectors). No post-cut
config: the credits token is read from the shared CreditsFacet slot;
LocalharnessRegistryFacet + TbaFacet must already be cut (they are).

### VotingFacet — guild DAO governance (propose / vote / execute)

Rung 4, the DAO apex: turns a guild from Admin-controlled into
MEMBER-GOVERNED. The MVP measure is a treasury spend
`(guildId, to, amount, memo)`; generic arbitrary-measure execution
is the documented follow-up. Storage: `LibVotingStorage` at
`keccak256("localharness.voting.storage.v1")`. Bounds:
`MIN_VOTING_PERIOD = 1 hours`, `MAX_VOTING_PERIOD = 30 days`,
`MAX_MEMO_BYTES = 4096`.

`contract VotingFacet is GuildFacet` — a passed `execute` calls the
INHERITED CEI-safe `_spendCore` directly: the exact same
treasury-debit path as `spendTreasury` (same `LibGuildStorage`
ledger, same ordering, same reentrancy guarantee), gated on a passed
vote instead of the Admin role. Only VotingFacet's OWN 8 selectors
are cut (`script/AddVotingFacet.s.sol`); the inherited GuildFacet
externals stay routed to the live GuildFacet.

- `propose(uint256 guildId, address to, uint256 amount, bytes memo,
  uint64 votingPeriod) → uint256 proposalId` — guild MEMBERS only.
  Fail-fast affordability check against the live treasury
  (re-checked at execute). SNAPSHOTS the guild's member count into
  the proposal as the frozen quorum denominator.
- `vote(uint256 proposalId, bool support)` — CURRENT members only,
  one-member-one-vote (weight 1), one ballot per address
  (`AlreadyVoted`), closes at the deadline.
- `execute(uint256 proposalId)` — PERMISSIONLESS after the deadline
  (the outcome is deterministic from the tally; anyone may poke it).
  On a PASS: status flips to `Executed` BEFORE the spend (CEI
  barrier 1), then `_spendCore` debits the ledger before its
  transfer (barrier 2) — proven by a reentrant-token probe.
  Affordability is re-checked live; an unaffordable passed measure
  reverts and stays Active for retry after a refund. Otherwise the
  proposal goes `Failed` with no spend. Terminal either way — a
  second `execute` reverts `ProposalNotActive` (idempotent).

**Quorum / threshold:** quorum = `ceil(snapshotMemberCount / 2)`
distinct voters, minimum 1 (a 0/1-member guild always needs a vote,
and a zero-member guild can never pass); threshold = STRICT majority
of cast votes (`for > against`; a tie FAILS). **The quorum
denominator is SNAPSHOTTED at propose** — the 0.30.0
governance-robustness fix: membership churn between propose and
execute (leave-to-shrink-the-bar, sybil-flood-to-inflate-it) can't
move the quorum; covered by +29 adversarial tests.

**Views:** `getProposal(id)` (status is the raw `VStatus`: 0=Active,
2=Failed, 3=Executed; 1=Passed / 4=Expired reserved),
`proposalMemoOf(id)`, `proposalsOf(guildId, startAfter, limit)`
(index-window paging like BountyFacet's `openBounties`),
`hasVoted(id, voter)`, `tallyOf(id)` (live `passing` projection),
`proposalCount()`.

**Events:** `ProposalCreated`, `VoteCast`, `ProposalExecuted`,
`ProposalFailed`.

A voter is an `address` — nothing gates it to an EOA, so a
member-guild's TBA can cast a ballot in a parent DAO
(DAOs-of-DAOs; proven live end-to-end).

### ReputationFacet — attestation-based agent trust

The trust rung — ERC-8004-FLAVORED on-chain reputation (the
ERC-8004 *validation* half is `ValidationFacet`, next section).
NON-FINANCIAL: no `$LH` escrow / payout / refund, and `attest` makes
no external call, so re-entrancy is structurally impossible
(Checks-Effects throughout). Storage: `LibReputationStorage` at
`keccak256("localharness.reputation.storage.v1")`.

- `attest(uint256 subjectTokenId, uint8 rating, bytes32 workRef)` —
  PERMISSIONLESS; `msg.sender` is the attester. Appends
  `{attester, rating, workRef}` to the subject's append-only audit
  trail and bumps the O(1) aggregate (`count++`, `sumRating +=
  rating`). `workRef` is an opaque hash / off-chain pointer (a
  bounty-id hash, a commit, a CID) — never interpreted, only used as
  the per-work dedup discriminator.

**Anti-abuse guards (all enforced in `attest`):**

1. **Dedup** on `(attester, subject, workRef)` (`AlreadyAttested`) —
   one address may attest a subject for many DISTINCT works but
   never the SAME `workRef` twice (anti-inflation).
2. **No self-attestation** (`SelfAttestation`) — reverts when the
   subject token's owner IS `msg.sender`.
3. **Rating range** 1..5 (`BadRating`); plus `UnknownSubject` for a
   tokenId with no registered owner.

Noted follow-ups, deliberately NOT built (additive cuts later; the
seam is the validation gate, not the storage shape):
attester-reputation WEIGHTING, and BOUNTY-PAYMENT COUPLING (require
`workRef` to map to a bounty actually accepted + paid to the
subject's TBA — the strong sybil defense).

**Views:** `reputationOf(tokenId) → (attestationCount, ratingSum)`
(average = sum/count, computed OFF-CHAIN — no on-chain division),
`attestationsOf(tokenId, start, limit)` (paged parallel arrays +
`nextCursor`), `hasAttested(attester, subjectTokenId, workRef)`.

**Event:** `Attested(subjectTokenId, attester, rating, workRef)`.

Cut via `script/AddReputationFacet.s.sol` (4 selectors). No post-cut
config — it only READS `ownerOfId` from the shared registry storage
slot.

### ValidationFacet — ERC-8004-style validation staking

The MONEY-BACKED half of the reputation system (ReputationFacet's
attestations are the free-signal half). **Source-complete + tested;
NOT yet cut into the live diamond.** Storage: `LibValidationStorage`
at `keccak256("localharness.validation.storage.v1")`. FINANCIAL —
the InviteFacet/BountyFacet escrow state-machine with TWO escrow
legs (`transferFrom` staker→diamond on stake AND on challenge; the
diamond escrows, NO minting; CEI before every payout/refund).

**Lifecycle:**

- `stakeValidation(bytes32 workRef, uint256 subjectTokenId, bool
  valid, uint256 stakeWei) → id` — escrow a stake behind a verdict
  about a subject's work. Open for a fixed `CHALLENGE_WINDOW`
  (3 days; protocol-fixed so a validator can't pick an
  unchallengeable 1-second window).
- `challengeValidation(id)` — anyone but the validator counter-
  stakes EXACTLY `stakeWei` behind the implicit opposite verdict
  (while `now <= challengeDeadline`); starts the
  `RESOLUTION_WINDOW` (7 days) clock.
- `resolveValidation(id, validatorWins)` — RESOLVER-ONLY (while
  `now <= resolveDeadline`): the POSTER of bounty
  `uint256(workRef)` when one exists (the platform convention is
  `workRef = bytes32(bountyId)` — the work's natural oracle, the
  same trust model as `acceptResult`), or the DIAMOND OWNER as
  arbiter fallback (the only resolver for non-bounty refs). The
  winner is paid BOTH stakes.
- `reclaimStake(id)` — permissionless poke, Open + past the
  challenge deadline: the validator is refunded 100% (the verdict
  stands unchallenged).
- `reclaimUnresolved(id)` — permissionless poke, Challenged + past
  the resolve deadline: BOTH sides take their own stake back (a
  draw — the AWOL-resolver hard stop; nothing strands).

**Windows are disjoint** (challenged XOR reclaimed; resolved XOR
drawn). **Self-validation rules:** the subject's owner cannot STAKE
about their own work (mirrors `SelfAttestation`) but CAN CHALLENGE
a validation of it; the validator cannot challenge themself; one
verdict per `(validator, subject, workRef)` EVER (the dedup
survives reclaim/loss). Caps: `MAX_ACTIVE_PER_VALIDATOR = 64`,
`MAX_STAKED = 1_000_000 ether` per address.

**Views:** `getValidation(id)` (full record),
`validationResolverOf(id)` (the bounty-poster half of the gate),
`hasValidated(validator, subject, workRef)`,
`validationsOfWork(workRef)`, `validationsOf(validator)`,
`validationCount()`, `validationStakedOf(addr)`,
`activeValidationCountOf(addr)`.

**Events:** `ValidationStaked / ValidationChallenged /
ValidationResolved / StakeReclaimed / ValidationDrawn`.

50 Foundry tests incl. a 256-run escrow-conservation fuzz
(diamond `$LH` == stake while Open + 2×stake while Challenged,
asserted after every step) and reentrant-token probes on all three
settlement paths. Cut via `script/AddValidationFacet.s.sol`
(13 selectors). No post-cut config — credits token, identity
owners, bounty posters, and the diamond owner are all shared
storage already populated on the live diamond. V1-simple by
design: the poster/owner is the oracle; staked juries,
reputation-weighted resolution, and resolver fees are additive
cuts later (the seam is the `resolveValidation` gate).

### SessionRoomFacet — encrypted on-chain shared key/value state (#22)

**Cut live.** The substrate for durable shared agent state: a room is
a member-gated, append-only log of OPAQUE ciphertext ops. An agent
persists state across turns/devices (or shares it with enrolled
members) by appending sealed ops instead of re-sending full context.
Storage: `LibSessionRoomStorage` at
`keccak256("localharness.sessionroom.storage.v1")` — a monotonic
`nextRoomId`, per-room `{creator, exists, epoch}`, an `isMember`
mapping + enumerable `memberList`, and the append-only `Op[]` log
(`Op = (address writer, uint64 ts, bytes blob)` — identical to
SignalingFacet's `Signal`, so off-chain decoders are shared).

Self-contained — no dependency on other facets' storage. The chain
enforces only WHO may write; the blobs are undecryptable without the
off-chain room key, so reads are open. **All KV/CRDT/crypto is
off-chain** (`src/kv_reduce.rs` LWW-CRDT fold + `src/kv_room.rs`
AES-256-GCM op-seal inside a writer-signed, room-bound envelope); the
room key is derived from the owner's identity secret + roomId, so a
single identity's devices share a room with no key exchange (v1).

**API:** `createRoom() → roomId` (caller = creator + first member);
`roomAddMember/roomRemoveMember(roomId, addr)` (creator-only);
`appendOp(roomId, blob) → index` (member-gated, ≤2048-byte blob);
`clearRoom(roomId) → epoch` (creator-only; wipes the log, bumps the
epoch so readers re-poll from 0); views `opsOf(roomId, fromIndex)`,
`opCount`, `roomEpoch`, `roomCreator`, `roomIsMember`,
`roomMembersOf`. Selectors are `room`-prefixed where a bare name
would collide with TeamFacet/GuildFacet `membersOf` (a diamond can't
share a selector). Cut via `script/AddSessionRoomFacet.s.sol`
(11 selectors); 8 Foundry tests (member-gate, creator-gate,
clear-epoch, paging, blob bounds). Multi-identity rooms (ECIES key
grant to enrolled members) are phase 2 — the membership is already
on-chain.

## Why a Diamond

The flat contract works fine for a single-purpose registry. But the
M9–M12 roadmap layers in:

- **ERC-721 conformance** — every name becomes a tradable NFT, which
  the permissionless ERC-6551 singleton registry then derives a
  token-bound account for (the agent's wallet).
- **ERC-8004-flavored reputation + DAO governance** — now LIVE as
  `ReputationFacet` (attestation-based trust) and `GuildFacet` +
  `VotingFacet` (member-governed treasuries); see the facet sections
  above. The ERC-8004 *validation* half is `ValidationFacet`
  (stake / challenge / resolve escrow) — built + tested, not yet cut.
- **MPP / x402 payment hooks** — per-call settlement layer.
- **Whatever else comes up.**

Each one of those would be a whole new flat contract under the
monolithic model — separate addresses, separate state, separate
migrations. With the diamond they're facets sharing the registry's
storage layout, addressable at one stable address. The bundle's
`REGISTRY_ADDRESS` constant doesn't change for the lifetime of the
project; only the facet selectors behind it do.

## Files (top-level summary)

- `foundry.toml` — Solidity 0.8.24, optimizer on, Tempo RPC alias
- `src/LocalharnessRegistry.sol` — legacy flat contract (~110 lines)
- `src/Diamond.sol` + `src/{facets,interfaces,libraries,upgradeInitializers}/`
  — the diamond stack
- `script/Deploy.s.sol` — legacy flat deploy
- `script/DeployDiamond.s.sol` — diamond deploy (atomic: facets +
  proxy + cut + init in one transaction sequence)
- `.gitignore` — `out/`, `cache/`, `broadcast/`, `lib/`
