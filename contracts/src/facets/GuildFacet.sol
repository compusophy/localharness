// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibGuildStorage} from "../libraries/LibGuildStorage.sol";
import {LibRegistryStorage} from "../libraries/LibRegistryStorage.sol";
import {LibCreditsStorage} from "../libraries/LibCreditsStorage.sol";

interface IERC20Min {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function transfer(address to, uint256 amount) external returns (bool);
}

/// The TBA-resolution surface GuildFacet reaches for the guild's own
/// address (its wallet — used to be a member of OTHER guilds + to receive
/// payouts). In production this is a SELF-call: the facet runs via the
/// diamond's delegatecall, so `address(this)` is the diamond and routes to
/// `TbaFacet.tokenBoundAccount` (same diamond, same on-chain reads). Kept
/// as an explicit interface so the dependency is legible and the test
/// harness can satisfy it by implementing the one selector.
interface ITbaResolver {
    function tokenBoundAccount(uint256 tokenId) external view returns (address);
}

/// @title GuildFacet
/// @notice Agent GUILDS — Rung 3 of the coordination ladder
///         (design/agent-coordination.md): a PERSISTENT organization with
///         members, roles, and a pooled `$LH` treasury. The standing
///         collective the lower rungs feed (a party claims a bounty; a guild
///         POSTS bounties from a treasury; a DAO votes on which to fund).
///
///         A GUILD *IS* AN IDENTITY (the SAFE MVP design — documented so the
///         ABI is unsurprising):
///           • `createGuild(name)` registers `name` as a NORMAL identity NFT
///             owned by the caller (the exact writes `LocalharnessRegistry
///             Facet.register` makes — replicated through the shared
///             `LibRegistryStorage` slot so the ORIGINAL caller, not the
///             diamond, is recorded as the holder).
///           • `guildId` == that registry tokenId.
///           • the guild's ADDRESS == `tokenBoundAccount(guildId)` — its
///             ERC-6551 wallet, for BEING A MEMBER OF OTHER GUILDS and for
///             receiving payouts. (`guildAddress` view.)
///
///         THE TREASURY is a FACET-BALANCE ESCROW — `guildBalance[guildId]`,
///         `$LH` physically held IN THE DIAMOND, the SAME safe pattern as
///         BountyFacet (NOT a TBA-execute). `fundGuild` credits it
///         (`transferFrom` funder→diamond, CEI); `spendTreasury` debits it
///         (Admin-gated, `transfer` diamond→`to`, CEI + reentrancy-safe).
///         DOCUMENTED UPGRADES: making the guild's TBA the live treasury and
///         vote-gating the spend (VotingFacet, Rung 4) — `spendTreasury`
///         routes through the internal `_spend` precisely so a VotingFacet
///         can vote-gate it later without reshaping storage.
///
///         THE RECURSIVE PROPERTY (Part 4 — "turtles all the way down").
///         Membership keys on `address` and NEVER assumes an EOA. A guild's
///         own TBA is a contract account; nothing here gates it out, so a
///         guild can be `inviteToGuild`'d into another guild → guilds-of-
///         guilds with ZERO new machinery. Proven by the recursive-
///         membership test (guild B's `guildAddress` joins guild A).
///
///         ROLE MODEL (the ABI-pinned enum, strictly ordered for `>=`
///         gating): None(0) Member(1) Officer(2) Admin(3).
///           • Officer+ may `inviteToGuild`.
///           • Admin may `setRole` (promote/demote/EVICT via setRole(.,.,0))
///             and `spendTreasury`.
///           • The founder is the first Admin (set in `createGuild`).
///           • LAST-ADMIN GUARD: the sole Admin can neither `leaveGuild` nor
///             self-demote — a guild can never become un-administrable (its
///             treasury would be frozen forever). They must promote another
///             Admin first, or the guild persists with them at the helm.
///
///         CEI ON EVERY `$LH` MOVE (fund credit / spend debit). The ledger
///         (`guildBalance`) is committed BEFORE the external token transfer,
///         so a hostile re-entrant token re-reads the already-debited
///         balance and a second spend reverts on `InsufficientTreasury` —
///         no double-spend, no drain. Proven by the reentrant-token probe.
///
///         CUTTING IT (diamond owner; mirror script/AddBountyFacet): deploy
///         + diamondCut Add the 16 selectors in script/AddGuildFacet.s.sol.
///         No post-cut config — the credits token is read from the shared
///         CreditsFacet storage slot, the registrar + TBA resolver are the
///         diamond itself (LocalharnessRegistryFacet + TbaFacet must already
///         be cut, which they are on the live diamond).
///
///         SELECTOR NOTE: the member-enumeration view is `guildMembersOf`,
///         NOT `membersOf` — TeamFacet already owns `membersOf(uint256)`
///         (selector 0x0e2aa455) on the live diamond and a diamond can't
///         share a selector (the BountyFacet `bountyTaskOf`-vs-`taskOf`
///         lesson). Every other name in the design ABI is collision-free.
contract GuildFacet {
    // --- Events ---------------------------------------------------------

    event GuildCreated(uint256 indexed guildId, address indexed founder, string name);
    event GuildInvited(uint256 indexed guildId, address indexed member, address indexed by);
    event GuildJoined(uint256 indexed guildId, address indexed member);
    event GuildLeft(uint256 indexed guildId, address indexed member);
    event RoleSet(uint256 indexed guildId, address indexed member, uint8 role, address indexed by);
    event GuildFunded(uint256 indexed guildId, address indexed funder, uint256 amount);
    event TreasurySpent(uint256 indexed guildId, address indexed to, uint256 amount, bytes memo);

    // --- Errors ---------------------------------------------------------

    error NotConfigured(); // credits token unset
    error NameTaken(); // createGuild on an already-registered name
    error InvalidName(); // createGuild with a non-DNS-label name
    error UnknownGuild(); // no such guildId
    error NotMember(); // caller is not a member of the guild
    error NotOfficer(); // caller lacks Officer+ (invite gate)
    error NotAdmin(); // caller lacks Admin (setRole / spendTreasury gate)
    error NotInvited(); // acceptGuildInvite without a pending invite
    error AlreadyMember(); // invite/accept for an existing member
    error GuildFull(); // member cap reached
    error BadRole(); // setRole with role > Admin
    error LastAdmin(); // the sole Admin can't leave / self-demote
    error ZeroAmount(); // fund / spend of 0
    error ZeroRecipient(); // spendTreasury to address(0)
    error InsufficientTreasury(); // spend exceeds guildBalance

    // --- Create (permissionless; the guild IS a registered identity) ----

    /// Register `name` as a normal identity owned by the caller, record it
    /// as a guild, and make the caller its first Admin. Returns the new
    /// `guildId` (== the registry tokenId).
    ///
    /// Replicates `LocalharnessRegistryFacet.register`'s EXACT writes
    /// against the shared `LibRegistryStorage` slot — NOT an external
    /// self-call, because a self-call's `msg.sender` would be the diamond,
    /// recording the DIAMOND as the holder. Writing the lib directly keeps
    /// the ORIGINAL caller as the owner (the guild's founder), so the
    /// resulting NFT + its TBA are indistinguishable from an ordinary
    /// `register`. The name validation + token-id-starts-at-1 invariants are
    /// reproduced verbatim (a guild name is a DNS label like any other; a
    /// token 0 would read as unclaimed).
    function createGuild(string calldata name) external returns (uint256 guildId) {
        if (!_isValidName(name)) revert InvalidName();

        LibRegistryStorage.Storage storage rs = LibRegistryStorage.load();
        if (rs.idOfName[name] != 0) revert NameTaken();

        // --- mint the identity (mirror register()) ----------------------
        if (rs.nextId == 0) rs.nextId = 1; // token ids start at 1
        guildId = rs.nextId++;
        rs.ownerOfId[guildId] = msg.sender;
        rs.idOfName[name] = guildId;
        rs.nameOfId[guildId] = name;
        rs.idOf[msg.sender] = guildId;
        rs.balanceOf[msg.sender] += 1;

        // --- record the guild + seat the founder as Admin ---------------
        LibGuildStorage.Storage storage gs = LibGuildStorage.load();
        LibGuildStorage.Guild storage g = gs.guilds[guildId];
        g.exists = true;
        gs.totalGuilds += 1;
        _setRoleInternal(gs, guildId, msg.sender, LibGuildStorage.Role.Admin);

        emit GuildCreated(guildId, msg.sender, name);
    }

    // --- Membership (consent-gated: Officer+ invites, invitee accepts) --

    /// Officer+ only: invite `member` to the guild. They must
    /// `acceptGuildInvite` themselves (the consent half). `member` may be a
    /// CONTRACT (another guild's TBA) — membership is not gated to EOAs.
    function inviteToGuild(uint256 guildId, address member) external {
        LibGuildStorage.Storage storage gs = LibGuildStorage.load();
        _requireGuild(gs, guildId);
        if (gs.roleOf[guildId][msg.sender] < LibGuildStorage.Role.Officer) revert NotOfficer();
        if (gs.roleOf[guildId][member] != LibGuildStorage.Role.None) revert AlreadyMember();
        gs.invited[guildId][member] = true;
        emit GuildInvited(guildId, member, msg.sender);
    }

    /// Invitee-only: accept a pending invite → become a Member. Both sides
    /// have now agreed (an Officer+ invited, you accepted). Enforces the
    /// member cap. The caller may be a contract — a guild's TBA can call
    /// this to join another guild (the recursive property).
    function acceptGuildInvite(uint256 guildId) external {
        LibGuildStorage.Storage storage gs = LibGuildStorage.load();
        _requireGuild(gs, guildId);
        if (!gs.invited[guildId][msg.sender]) revert NotInvited();
        if (gs.roleOf[guildId][msg.sender] != LibGuildStorage.Role.None) revert AlreadyMember();
        if (gs.guilds[guildId].memberCount >= LibGuildStorage.MAX_MEMBERS) revert GuildFull();

        gs.invited[guildId][msg.sender] = false;
        _setRoleInternal(gs, guildId, msg.sender, LibGuildStorage.Role.Member);
        emit GuildJoined(guildId, msg.sender);
    }

    /// Leave a guild you're a member of. Blocked for the SOLE Admin (a guild
    /// must always have at least one Admin so its treasury can never be
    /// frozen) — promote another Admin first. Anyone else (incl. a
    /// non-sole Admin) may leave freely.
    function leaveGuild(uint256 guildId) external {
        LibGuildStorage.Storage storage gs = LibGuildStorage.load();
        _requireGuild(gs, guildId);
        LibGuildStorage.Role r = gs.roleOf[guildId][msg.sender];
        if (r == LibGuildStorage.Role.None) revert NotMember();
        if (r == LibGuildStorage.Role.Admin && gs.guilds[guildId].adminCount <= 1) {
            revert LastAdmin();
        }
        _setRoleInternal(gs, guildId, msg.sender, LibGuildStorage.Role.None);
        emit GuildLeft(guildId, msg.sender);
    }

    /// Admin-only: set `member`'s role. 0=None (EVICT — removes them),
    /// 1=Member, 2=Officer, 3=Admin (promote). Guards:
    ///   • role must be a valid enum value (<= Admin).
    ///   • can't demote/evict the SOLE Admin (would freeze the treasury);
    ///     an Admin must promote a replacement before stepping down.
    /// Setting a role on a current None address that ISN'T an invitee is the
    /// admin-grant path (an Admin can seat a member directly) — but it still
    /// respects the member cap when it would add a new member.
    function setRole(uint256 guildId, address member, uint8 role) external {
        if (role > uint8(LibGuildStorage.Role.Admin)) revert BadRole();
        LibGuildStorage.Storage storage gs = LibGuildStorage.load();
        _requireGuild(gs, guildId);
        if (gs.roleOf[guildId][msg.sender] != LibGuildStorage.Role.Admin) revert NotAdmin();

        LibGuildStorage.Role newRole = LibGuildStorage.Role(role);
        LibGuildStorage.Role oldRole = gs.roleOf[guildId][member];
        if (newRole == oldRole) return; // no-op

        // Last-Admin guard: demoting/evicting the only Admin is forbidden.
        if (
            oldRole == LibGuildStorage.Role.Admin && newRole != LibGuildStorage.Role.Admin
                && gs.guilds[guildId].adminCount <= 1
        ) {
            revert LastAdmin();
        }
        // Adding a brand-new member (None -> something) respects the cap.
        if (oldRole == LibGuildStorage.Role.None && newRole != LibGuildStorage.Role.None) {
            if (gs.guilds[guildId].memberCount >= LibGuildStorage.MAX_MEMBERS) revert GuildFull();
            gs.invited[guildId][member] = false; // a direct seat clears any pending invite
        }

        _setRoleInternal(gs, guildId, member, newRole);
        emit RoleSet(guildId, member, role, msg.sender);
    }

    // --- Treasury: fund (anyone) + spend (Admin) — CEI escrow -----------

    /// Fund the guild's treasury: pull `amount` `$LH` (`transferFrom`
    /// funder→diamond; approve the diamond first — the bundle batches
    /// approve + fundGuild into one sponsored tx, exactly like
    /// `postBounty` / `createInvite`) and credit `guildBalance[guildId]`.
    /// PERMISSIONLESS — anyone (a member, an outsider, ANOTHER guild's TBA)
    /// may fund. The `$LH` lives in the diamond; the ledger tracks each
    /// guild's share.
    ///
    /// CEI: the ledger credit lands BEFORE the external pull. A failed pull
    /// (under-allowance / under-balance) reverts the whole tx, including the
    /// credit — no ghost balance.
    function fundGuild(uint256 guildId, uint256 amount) external {
        if (amount == 0) revert ZeroAmount();
        LibGuildStorage.Storage storage gs = LibGuildStorage.load();
        _requireGuild(gs, guildId);

        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();

        // CEI: credit the ledger BEFORE the pull. A revert in transferFrom
        // unwinds this with it.
        gs.guildBalance[guildId] += amount;
        require(
            IERC20Min(token).transferFrom(msg.sender, address(this), amount),
            "guild: fund failed"
        );

        emit GuildFunded(guildId, msg.sender, amount);
    }

    /// Spend from the guild's treasury. ADMIN-ONLY (the MVP governance rule
    /// — Rung 4's VotingFacet is the documented vote-gated upgrade; this
    /// routes through the internal `_spend` so the vote-gate slots in there
    /// without a storage change). Debits `guildBalance[guildId]` and
    /// transfers `amount` `$LH` to `to`. `to` may be ANY address — an EOA, a
    /// worker's TBA (paying a bounty out of the treasury), or ANOTHER
    /// guild's address (funding a member-guild). `memo` is an opaque,
    /// unstored note carried in the event for off-chain accounting.
    ///
    /// CEI + reentrancy-safe: the debit lands BEFORE the external transfer,
    /// so a hostile re-entrant token that tries a SECOND spend re-reads the
    /// already-reduced balance and reverts on `InsufficientTreasury`. No
    /// double-spend, no drain (proven by the reentrant-token probe).
    function spendTreasury(uint256 guildId, address to, uint256 amount, bytes calldata memo)
        external
    {
        LibGuildStorage.Storage storage gs = LibGuildStorage.load();
        _requireGuild(gs, guildId);
        if (gs.roleOf[guildId][msg.sender] != LibGuildStorage.Role.Admin) revert NotAdmin();
        _spend(gs, guildId, to, amount, memo);
    }

    /// The single treasury-debit path (calldata-memo entry — what
    /// `spendTreasury` calls). Internal so a future VotingFacet can vote-gate
    /// the spend (Rung 4) by calling the same CEI-safe core. Forwards to
    /// `_spendCore` so the calldata (Admin path) and memory (VotingFacet
    /// path) callers share ONE implementation — a single source of treasury
    /// accounting truth.
    function _spend(
        LibGuildStorage.Storage storage gs,
        uint256 guildId,
        address to,
        uint256 amount,
        bytes calldata memo
    ) internal {
        _spendCore(gs, guildId, to, amount, memo);
    }

    /// The ACTUAL treasury-debit core, `memory`-memo so BOTH the Admin
    /// `spendTreasury` path (via `_spend`, calldata→memory) AND the
    /// vote-gated VotingFacet path (which has only `storage`/`memory` memo at
    /// the call site — `execute` has no calldata bytes argument) reuse it.
    /// CEI: validate → debit ledger → external transfer LAST. The single
    /// place `guildBalance` is debited — same slot, same ordering, same
    /// reentrancy guarantee, whoever calls it.
    function _spendCore(
        LibGuildStorage.Storage storage gs,
        uint256 guildId,
        address to,
        uint256 amount,
        bytes memory memo
    ) internal {
        if (amount == 0) revert ZeroAmount();
        if (to == address(0)) revert ZeroRecipient();
        if (gs.guildBalance[guildId] < amount) revert InsufficientTreasury();

        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();

        // CEI: debit BEFORE the transfer. A re-entrant second spend sees the
        // reduced balance and reverts on InsufficientTreasury.
        gs.guildBalance[guildId] -= amount;
        require(IERC20Min(token).transfer(to, amount), "guild: spend failed");

        emit TreasurySpent(guildId, to, amount, memo);
    }

    // --- Views ----------------------------------------------------------

    /// Enumerable member list. NAMED `guildMembersOf` (not `membersOf`) to
    /// avoid the TeamFacet `membersOf(uint256)` selector collision.
    function guildMembersOf(uint256 guildId) external view returns (address[] memory) {
        return LibGuildStorage.load().members[guildId];
    }

    /// A member's role (0=None … 3=Admin). None for a non-member.
    function roleOf(uint256 guildId, address member) external view returns (uint8) {
        return uint8(LibGuildStorage.load().roleOf[guildId][member]);
    }

    /// True iff `member` has any role above None in the guild.
    function isGuildMember(uint256 guildId, address member) external view returns (bool) {
        return LibGuildStorage.load().roleOf[guildId][member] != LibGuildStorage.Role.None;
    }

    /// The guild's pooled `$LH` treasury balance (the diamond holds it).
    function treasuryBalanceOf(uint256 guildId) external view returns (uint256) {
        return LibGuildStorage.load().guildBalance[guildId];
    }

    /// The guild's ADDRESS = the token-bound account of its identity NFT
    /// (resolved via the diamond's `TbaFacet.tokenBoundAccount`). This is
    /// the guild's wallet — what you `inviteToGuild` to make a guild a
    /// member of ANOTHER guild, and what receives payouts. Reverts for an
    /// unknown guild (the underlying TBA call reverts on a nonexistent
    /// token).
    function guildAddress(uint256 guildId) external view returns (address) {
        if (!LibGuildStorage.load().guilds[guildId].exists) revert UnknownGuild();
        return ITbaResolver(address(this)).tokenBoundAccount(guildId);
    }

    /// The guild's registered name (its identity NFT's name).
    function guildName(uint256 guildId) external view returns (string memory) {
        if (!LibGuildStorage.load().guilds[guildId].exists) revert UnknownGuild();
        return LibRegistryStorage.load().nameOfId[guildId];
    }

    /// Every guild id `member` belongs to.
    function guildsOf(address member) external view returns (uint256[] memory) {
        return LibGuildStorage.load().guildsOf[member];
    }

    /// True iff `tokenId` is a guild (created via `createGuild`) — an
    /// ordinary registered name is NOT a guild.
    function isGuild(uint256 tokenId) external view returns (bool) {
        return LibGuildStorage.load().guilds[tokenId].exists;
    }

    /// Total number of guilds ever created.
    function guildCount() external view returns (uint256) {
        return LibGuildStorage.load().totalGuilds;
    }

    // --- internals ------------------------------------------------------

    function _requireGuild(LibGuildStorage.Storage storage gs, uint256 guildId) internal view {
        if (!gs.guilds[guildId].exists) revert UnknownGuild();
    }

    /// The single role-transition path. Keeps the enumerable member list,
    /// the index map, the per-member `guildsOf`, the member count, and the
    /// admin count all consistent on EVERY transition (add / remove /
    /// promote / demote). Idempotent on a no-op.
    function _setRoleInternal(
        LibGuildStorage.Storage storage gs,
        uint256 guildId,
        address member,
        LibGuildStorage.Role newRole
    ) internal {
        LibGuildStorage.Role oldRole = gs.roleOf[guildId][member];
        if (oldRole == newRole) return;

        bool wasMember = oldRole != LibGuildStorage.Role.None;
        bool isMember = newRole != LibGuildStorage.Role.None;

        // Admin-count accounting (the last-Admin invariant key).
        if (oldRole == LibGuildStorage.Role.Admin && newRole != LibGuildStorage.Role.Admin) {
            gs.guilds[guildId].adminCount -= 1;
        } else if (oldRole != LibGuildStorage.Role.Admin && newRole == LibGuildStorage.Role.Admin) {
            gs.guilds[guildId].adminCount += 1;
        }

        gs.roleOf[guildId][member] = newRole;

        if (!wasMember && isMember) {
            // None -> member: add to the enumerable set.
            gs.members[guildId].push(member);
            gs.memberIndex[guildId][member] = gs.members[guildId].length; // index + 1
            gs.guildsOf[member].push(guildId);
            gs.guilds[guildId].memberCount += 1;
        } else if (wasMember && !isMember) {
            // member -> None: swap-pop out of the enumerable set.
            _removeFromMembers(gs, guildId, member);
            _removeFromGuildsOf(gs, member, guildId);
            gs.guilds[guildId].memberCount -= 1;
        }
        // member -> member (a role change within membership): nothing to do
        // for the enumerable structures.
    }

    function _removeFromMembers(
        LibGuildStorage.Storage storage gs,
        uint256 guildId,
        address member
    ) internal {
        uint256 idx1 = gs.memberIndex[guildId][member];
        if (idx1 == 0) return;
        address[] storage m = gs.members[guildId];
        uint256 i = idx1 - 1;
        uint256 last = m.length - 1;
        if (i != last) {
            address moved = m[last];
            m[i] = moved;
            gs.memberIndex[guildId][moved] = i + 1;
        }
        m.pop();
        gs.memberIndex[guildId][member] = 0;
    }

    function _removeFromGuildsOf(
        LibGuildStorage.Storage storage gs,
        address member,
        uint256 guildId
    ) internal {
        uint256[] storage gids = gs.guildsOf[member];
        for (uint256 j = 0; j < gids.length; j++) {
            if (gids[j] == guildId) {
                gids[j] = gids[gids.length - 1];
                gids.pop();
                break;
            }
        }
    }

    /// A valid DNS label, EXACTLY matching `LocalharnessRegistryFacet.
    /// _isValidName` (1-63 bytes of lowercase a-z / 0-9 / hyphen, no leading
    /// or trailing hyphen) — a guild name must be a routable subdomain like
    /// any other identity.
    function _isValidName(string calldata name) internal pure returns (bool) {
        bytes memory b = bytes(name);
        if (b.length < 1 || b.length > 63) return false;
        if (b[0] == 0x2d || b[b.length - 1] == 0x2d) return false;
        for (uint256 i = 0; i < b.length; i++) {
            bytes1 c = b[i];
            bool ok = (c >= 0x30 && c <= 0x39) // 0-9
                || (c >= 0x61 && c <= 0x7a) // a-z
                || (c == 0x2d); // -
            if (!ok) return false;
        }
        return true;
    }
}
