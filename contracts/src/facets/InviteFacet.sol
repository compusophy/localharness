// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibInviteStorage} from "../libraries/LibInviteStorage.sol";
import {LibCreditsStorage} from "../libraries/LibCreditsStorage.sol";

interface IERC20Min {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function transfer(address to, uint256 amount) external returns (bool);
}

/// @title InviteFacet
/// @notice User-funded, refundable-on-expiry invite codes — the GROWTH
///         primitive (design/invites.md). ANY holder ESCROWS their OWN
///         `$LH` to back a shareable invite code keyed by
///         `keccak256(bytes(code))` (the same bearer-code hashing as
///         RedeemFacet, so the existing `?invite=CODE` auto-onboard path
///         carries over). The escrow pays out to whoever ACCEPTS the code
///         (a newcomer), or refunds the FUNDER 100% after expiry if it's
///         never claimed.
///
///         SIBLING OF RedeemFacet, NOT a replacement (§1.1): RedeemFacet
///         is owner-only and MINTS fresh `$LH` (`ISSUER_ROLE`); InviteFacet
///         is PERMISSIONLESS and ESCROWS EXISTING `$LH` (`transferFrom`
///         funder→diamond), later paying it out or refunding it. No
///         minting, no `ISSUER_ROLE`. So invites are SUPPLY-NEUTRAL — they
///         redistribute, never inflate — which is why they DON'T reopen
///         the infinite-credit sybil hole that disabled the daily
///         allowance (§4.5): a sybil ring passing invites among themselves
///         just round-trips their own `$LH` minus sponsor gas.
///
///         LIFECYCLE (§3.1): createInvite (escrow, Open) → acceptInvite
///         (pay accepter, Accepted) XOR reclaimInvite (refund funder after
///         expiry, Reclaimed). Open's accept window (`now <= expiry`) and
///         reclaim window (`now > expiry`) are DISJOINT, so an invite is
///         accepted XOR reclaimed, never both.
///
///         CEI throughout (§4.1/§4.3): every state mutation (status,
///         escrowedOf) lands BEFORE the token transfer. createInvite
///         writes the whole record + bumps escrowedOf BEFORE the inbound
///         `transferFrom`, so a failed pull reverts the whole tx and
///         leaves NO ghost invite. accept/reclaim flip status + decrement
///         escrowedOf BEFORE the payout.
///
///         BEARER MVP (§4.2 / §7.1): the plaintext code is a bearer
///         secret — anyone who submits it first accepts. Bound vouchers
///         (an optional named recipient that defeats mempool front-running)
///         are Phase 2 and deliberately NOT built here. Short TTLs are the
///         MVP mitigation; the code is meant for one trusted recipient.
///
///         CUTTING IT (diamond owner; mirror script/AddRedeemFacet):
///         deploy + diamondCut Add the 5 selectors in
///         script/AddInviteFacet.s.sol. No post-cut config — the credits
///         token is read from the shared CreditsFacet storage slot.
contract InviteFacet {
    // --- Events (indexed for off-chain harvest; §1.3) -------------------

    event InviteCreated(bytes32 indexed codeHash, address indexed funder, uint256 amount, uint64 expiry);
    event InviteAccepted(
        bytes32 indexed codeHash, address indexed accepter, address indexed funder, uint256 amount
    );
    event InviteReclaimed(bytes32 indexed codeHash, address indexed funder, uint256 amount);

    // --- Errors ---------------------------------------------------------

    error NotConfigured(); // credits token unset
    error CodeTaken(); // codeHash already has a funder
    error ZeroAmount();
    error BadTtl(); // ttl < MIN_TTL || ttl > MAX_TTL
    error EscrowCapExceeded(); // funder's open escrow would exceed MAX_ESCROWED
    error UnknownInvite(); // no such codeHash
    error NotOpen(); // already accepted/reclaimed
    error Expired(); // accept after expiry
    error NotYetExpired(); // reclaim before expiry

    // --- Create (permissionless; funder escrows their own $LH) ----------

    /// Create an invite backing `amount` `$LH` under `codeHash`
    /// (`keccak256(bytes(code))`), acceptable for `ttlSeconds`. ESCROWS
    /// `amount` from the caller into the diamond (`transferFrom`; approve
    /// the diamond first — the bundle batches approve + createInvite into
    /// one sponsored tx, exactly like `depositCredits`).
    ///
    /// Rejects a duplicate codeHash (an existing non-zero funder), zero
    /// amount, a ttl outside [MIN_TTL, MAX_TTL], and an amount that would
    /// push the funder's open escrow past MAX_ESCROWED.
    ///
    /// CEI: the WHOLE invite record + the escrowedOf bump are written
    /// BEFORE the external `transferFrom`, so a failed pull reverts the
    /// whole tx and leaves no ghost invite (§4.3).
    function createInvite(bytes32 codeHash, uint256 amount, uint64 ttlSeconds) external {
        LibInviteStorage.Storage storage s = LibInviteStorage.load();

        // Dup guard: a taken code already has a funder.
        if (s.invites[codeHash].funder != address(0)) revert CodeTaken();
        if (amount == 0) revert ZeroAmount();
        if (ttlSeconds < LibInviteStorage.MIN_TTL || ttlSeconds > LibInviteStorage.MAX_TTL) {
            revert BadTtl();
        }
        // amount fits uint128 (checked here so the struct write is safe);
        // $LH supply << 2^128 so this never bites legitimately.
        if (amount > type(uint128).max) revert EscrowCapExceeded();
        // Per-funder open-escrow cap (§2.4 circuit-breaker).
        uint256 newEscrow = s.escrowedOf[msg.sender] + amount;
        if (newEscrow > LibInviteStorage.MAX_ESCROWED) revert EscrowCapExceeded();

        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();

        uint64 expiry = uint64(block.timestamp) + ttlSeconds;
        s.invites[codeHash] = LibInviteStorage.Invite({
            funder: msg.sender,
            expiry: expiry,
            status: LibInviteStorage.Status.Open,
            amount: uint128(amount)
        });
        s.escrowedOf[msg.sender] = newEscrow;

        // CEI: escrow LAST. State fully committed above; a failed pull
        // reverts everything (and these writes with it) → no ghost invite.
        require(
            IERC20Min(token).transferFrom(msg.sender, address(this), amount),
            "invite: escrow failed"
        );

        emit InviteCreated(codeHash, msg.sender, amount, expiry);
    }

    // --- Accept (the recipient redeems the plaintext code) --------------

    /// Accept an invite by its plaintext `code` (`keccak256(bytes(code))`
    /// is the on-chain key — the same hash as `redeem`). Requires the
    /// invite be Open AND unexpired (`now <= expiry`). Pays the escrowed
    /// `$LH` to the ACCEPTER (`msg.sender`). Bearer: anyone holding the
    /// code accepts (§4.2 MVP).
    ///
    /// CEI: status flips to Accepted + escrowedOf[funder] is decremented
    /// BEFORE the payout `transfer`, so a re-entrant / replayed accept
    /// re-reads `status != Open` and reverts (§4.1). The Open+unexpired
    /// guard is also disjoint from reclaim's Open+expired guard, so an
    /// invite is accepted XOR reclaimed, never both.
    function acceptInvite(string calldata code) external returns (uint256) {
        bytes32 codeHash = keccak256(bytes(code));
        LibInviteStorage.Storage storage s = LibInviteStorage.load();
        LibInviteStorage.Invite storage inv = s.invites[codeHash];

        if (inv.funder == address(0)) revert UnknownInvite();
        if (inv.status != LibInviteStorage.Status.Open) revert NotOpen();
        if (block.timestamp > inv.expiry) revert Expired();

        address funder = inv.funder;
        uint128 amount = inv.amount;

        // CEI: terminal state + escrow accounting BEFORE the payout.
        inv.status = LibInviteStorage.Status.Accepted;
        s.escrowedOf[funder] -= amount;

        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();
        require(IERC20Min(token).transfer(msg.sender, amount), "invite: payout failed");

        emit InviteAccepted(codeHash, msg.sender, funder, amount);
        return amount;
    }

    // --- Reclaim (funder gets the escrow back after expiry, unclaimed) --

    /// Reclaim an expired, unaccepted invite. PERMISSIONLESS to CALL
    /// (anyone can poke it), but the refund ALWAYS goes to the FUNDER,
    /// never `msg.sender` (§3.2 decision) — a third-party caller gains
    /// nothing, the funder gets 100% back. Requires the invite be Open AND
    /// expired (`now > expiry`).
    ///
    /// CEI: status flips to Reclaimed + escrowedOf[funder] is decremented
    /// BEFORE the refund `transfer`. Double-reclaim and accept-after-
    /// reclaim are both blocked by the `status != Open` re-read.
    function reclaimInvite(bytes32 codeHash) external {
        LibInviteStorage.Storage storage s = LibInviteStorage.load();
        LibInviteStorage.Invite storage inv = s.invites[codeHash];

        if (inv.funder == address(0)) revert UnknownInvite();
        if (inv.status != LibInviteStorage.Status.Open) revert NotOpen();
        if (block.timestamp <= inv.expiry) revert NotYetExpired();

        address funder = inv.funder;
        uint128 amount = inv.amount;

        // CEI: terminal state + escrow accounting BEFORE the refund.
        inv.status = LibInviteStorage.Status.Reclaimed;
        s.escrowedOf[funder] -= amount;

        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();
        require(IERC20Min(token).transfer(funder, amount), "invite: refund failed");

        emit InviteReclaimed(codeHash, funder, amount);
    }

    // --- Views ----------------------------------------------------------

    /// Full invite record by code hash. Returns zeros for an unknown code
    /// (funder == address(0)).
    function getInvite(bytes32 codeHash)
        external
        view
        returns (address funder, uint128 amount, uint64 expiry, uint8 status)
    {
        LibInviteStorage.Invite storage inv = LibInviteStorage.load().invites[codeHash];
        return (inv.funder, inv.amount, inv.expiry, uint8(inv.status));
    }

    /// Total `$LH` a funder currently has locked in OPEN invites.
    function escrowedOf(address funder) external view returns (uint256) {
        return LibInviteStorage.load().escrowedOf[funder];
    }
}
