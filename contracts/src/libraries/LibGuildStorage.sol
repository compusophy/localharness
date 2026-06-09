// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the GUILD facet — Rung 3 of the coordination
///      ladder (design/agent-coordination.md): a PERSISTENT organization
///      with members, roles, and a pooled `$LH` treasury. Diamond storage
///      pattern — fresh slot, no collision with the registry / TBA / team /
///      bounty / invite / schedule / credits storage already cut into the
///      diamond. Add new fields ONLY at the end of the struct (and only at
///      the end of `Guild` / `Membership`) — diamond storage layout is
///      positional and immutable (the "append-only rule").
///
///      THE RECURSIVE VISION (Part 4 of the design doc — "turtles all the
///      way down"). Membership keys on `address`, never on "is this an
///      EOA / a human." A guild's own ADDRESS is the token-bound account of
///      its identity NFT (`TbaFacet.tokenBoundAccount(guildId)`), which is a
///      contract account — so a guild can itself be `inviteToGuild`'d into
///      ANOTHER guild, voted on, paid out to, with ZERO new machinery.
///      Guilds-of-guilds (and, later, DAOs-of-DAOs) emerge for free the
///      moment one guild points its TBA at another guild's
///      `acceptGuildInvite`. The single discipline that keeps that door
///      open: this storage and the facet NEVER gate a member to an EOA.
///
///      TREASURY = facet-balance escrow, NOT a TBA-execute (the SAFE MVP).
///      `guildBalance[guildId]` accounts the `$LH` the diamond physically
///      holds on a guild's behalf — the SAME pattern as BountyFacet's escrow
///      (the `$LH` lives in the diamond; the mapping is the ledger). The
///      documented upgrade is to make the guild's TBA the live treasury and
///      vote-gate the spend (VotingFacet, Rung 4); the facet routes spend
///      through an internal `_spend` precisely so a future VotingFacet can
///      vote-gate it without reshaping the storage.
library LibGuildStorage {
    bytes32 constant POSITION = keccak256("localharness.guild.storage.v1");

    // --- Bounds (anti-sybil / anti-grief circuit-breakers). The per-guild
    //     member cap mirrors ScheduleFacet's MAX_ACTIVE_JOBS_PER_OWNER and
    //     BountyFacet's MAX_ACTIVE_PER_POSTER: it bounds the `guildMembersOf`
    //     array scan + keeps a single guild's membership from being flooded
    //     into an unbounded SSTORE chain. 128 is generous for a real
    //     collective yet small enough to keep the member enumeration cheap.
    uint256 internal constant MAX_MEMBERS = 128;

    /// Member role (the ABI-pinned enum — None=0 … Admin=3). Strictly
    /// ordered so a gate can compare numerically (`role >= Officer`):
    ///   None    (0) — not a member; the default for any unseen address.
    ///   Member  (1) — joined; can leave + receive payouts; no privileges.
    ///   Officer (2) — Member + may `inviteToGuild` new members.
    ///   Admin   (3) — Officer + may `setRole` (incl. promote/demote/evict)
    ///                 + `spendTreasury`. The founder is the first Admin.
    enum Role {
        None,    // 0
        Member,  // 1
        Officer, // 2
        Admin    // 3
    }

    /// One guild = one registered identity. `guildId` IS the registry
    /// tokenId of the guild's name (so the guild's wallet/address is
    /// `tokenBoundAccount(guildId)`), and `exists` is the "is this tokenId a
    /// guild" sentinel (a name registered the ordinary way is NOT a guild).
    struct Guild {
        bool exists;       // true once createGuild recorded this tokenId
        uint64 memberCount; // live member count (the MAX_MEMBERS cap key)
        uint32 adminCount;  // live Admin count — guards the "last Admin can't leave / self-demote" invariant
    }

    struct Storage {
        /// guildId (== registry tokenId) -> guild record. `exists == false`
        /// is the unknown-guild guard.
        mapping(uint256 => Guild) guilds;
        /// guildId -> member address -> role. Role.None (0) is "not a
        /// member" — the default for any address. KEYED ON ADDRESS, never on
        /// EOA-ness: a member may be a contract (another guild's TBA) — the
        /// recursive-composability property (Part 4 of the design doc).
        mapping(uint256 => mapping(address => Role)) roleOf;
        /// guildId -> member address -> has a pending invite (cleared on
        /// accept/leave). Consent-gated membership: an Officer+ invites, the
        /// invitee must `acceptGuildInvite` themselves — the TeamFacet
        /// pattern.
        mapping(uint256 => mapping(address => bool)) invited;
        /// guildId -> enumerable member list (for `guildMembersOf`). Removal
        /// is O(1) swap-pop via `memberIndex`.
        mapping(uint256 => address[]) members;
        /// guildId -> member address -> (index + 1) into `members[id]`,
        /// 0 = absent. The swap-pop bookkeeping.
        mapping(uint256 => mapping(address => uint256)) memberIndex;
        /// member address -> the guild ids it belongs to (for `guildsOf`).
        mapping(address => uint256[]) guildsOf;
        /// guildId -> the `$LH` (18-dec wei) the diamond escrows for this
        /// guild's treasury. `fundGuild` credits it (after a `transferFrom`
        /// into the diamond); `spendTreasury` debits it (before a `transfer`
        /// out). The diamond's real `$LH` balance >= sum over all guilds of
        /// this field (treasury-conservation invariant). The SAME safe
        /// escrow ledger as BountyFacet — NOT a TBA-execute.
        mapping(uint256 => uint256) guildBalance;
        /// Count of guilds ever created (monotonic). A guild's id is its
        /// registry tokenId (NOT this counter), so this is a population stat
        /// for `guildCount`, not an id source.
        uint256 totalGuilds;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
