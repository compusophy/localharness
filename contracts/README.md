# `contracts/` — Localharness on-chain registry

Two contract stacks live here:

1. **Flat `LocalharnessRegistry`** at `src/LocalharnessRegistry.sol`
   — the original ~110-line monolith. Currently deployed at
   `0x42c8D4EaF99bA80F6B6FCA8E163E077D9FC2F9db` on Tempo Moderato.
   This is what `src/app/registry.rs::REGISTRY_ADDRESS` in the wasm
   bundle reads.

2. **EIP-2535 Diamond** under `src/{Diamond,facets,interfaces,
   libraries,upgradeInitializers}/` — the live architecture.
   Replaces the flat contract so new capability lands as facets
   without redeploying-the-world each time: ERC-721, ERC-6551
   helpers, payments — and, live since 0.30.0, ERC-8004-flavored
   reputation (`ReputationFacet`) plus guild DAO governance
   (`GuildFacet` + `VotingFacet`). The ERC-8004 *validation* half
   (validator stake escrow) remains future work.

The flat contract stays in-tree as historical reference. The
diamond is the path forward; the cutover is "deploy diamond → swap
the address constant in the wasm bundle → redeploy bundle." Names
registered against the flat contract are NOT migrated automatically
(small enough population that this is fine for testnet).

## Deploy (Tempo Moderato testnet)

Requirements:

- `foundry` installed (`forge --version` works).
- An EVM private key with some testnet TMP for gas. Faucet via
  `tempo_fundAddress` RPC: see `src/app/registry.rs::request_faucet_funds`
  for the exact JSON-RPC shape.
- `forge-std` installed: `forge install foundry-rs/forge-std --no-git`
  from this directory (one-time).

### Diamond (new)

```sh
cd contracts
export EVM_PRIVATE_KEY=0x...your-funded-testnet-key
forge script script/DeployDiamond.s.sol \
    --rpc-url tempo_moderato \
    --private-key $EVM_PRIVATE_KEY \
    --broadcast
```

Prints the diamond address + each facet's address. Bake the
**diamond** address into `src/app/registry.rs::REGISTRY_ADDRESS`,
rebuild + deploy the wasm bundle.

### Flat (legacy)

```sh
forge script script/Deploy.s.sol \
    --rpc-url tempo_moderato \
    --private-key $EVM_PRIVATE_KEY \
    --broadcast
```

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
│   ├── GuildFacet.sol                agent guilds — members/roles +
│   │                                 pooled $LH treasury escrow
│   ├── VotingFacet.sol               guild DAO — propose / vote /
│   │                                 execute a treasury spend
│   └── ReputationFacet.sol           attestation-based reputation —
│                                     attest / reputationOf / ...
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
│   ├── LibGuildStorage.sol           guild storage ("localharness.
│   │                                 guild.storage.v1")
│   ├── LibVotingStorage.sol          voting storage ("localharness.
│   │                                 voting.storage.v1")
│   └── LibReputationStorage.sol      reputation storage ("localharness.
│                                     reputation.storage.v1")
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

### GuildFacet — agent guilds (members, roles, pooled treasury)

Durable on-chain organizations of agents — rung 3 of the
coordination ladder (`design/agent-coordination.md`). Storage:
`LibGuildStorage` at `keccak256("localharness.guild.storage.v1")`;
per-guild member cap `MAX_MEMBERS = 128` (anti-grief bound on the
member enumeration).

**A guild IS an identity.** `createGuild(string name) → uint256
guildId` registers `name` as a normal identity NFT owned by the
caller — it replicates `LocalharnessRegistryFacet.register`'s exact
writes against the shared `LibRegistryStorage` slot (an external
self-call would record the DIAMOND as holder), so `guildId` == the
registry tokenId, name validation is the same DNS-label rule, and
the guild's ADDRESS is its ERC-6551 token-bound account
(`guildAddress(guildId)` resolves `TbaFacet.tokenBoundAccount` via a
self-call). The founder is seated as the first Admin.

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
`GuildLeft`, `RoleSet`, `GuildFunded`, `TreasurySpent`.

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
ERC-8004 *validation* half, validator stake escrow, is NOT built).
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

## Why a Diamond

The flat contract works fine for a single-purpose registry. But the
M9–M12 roadmap layers in:

- **ERC-721 conformance** — every name becomes a tradable NFT, which
  the permissionless ERC-6551 singleton registry then derives a
  token-bound account for (the agent's wallet).
- **ERC-8004-flavored reputation + DAO governance** — now LIVE as
  `ReputationFacet` (attestation-based trust) and `GuildFacet` +
  `VotingFacet` (member-governed treasuries); see the facet sections
  above. The ERC-8004 *validation* half (validator stake escrow to
  re-execute claims) remains future work.
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
