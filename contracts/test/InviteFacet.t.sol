// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {InviteFacet} from "../src/facets/InviteFacet.sol";
import {LibInviteStorage} from "../src/libraries/LibInviteStorage.sol";
import {LibCreditsStorage} from "../src/libraries/LibCreditsStorage.sol";

/// Minimal `$LH`-shaped TIP-20 mock: 18-decimal balances + the
/// approve/transferFrom/transfer surface InviteFacet escrows + pays out
/// through. Reverts (via require) on an under-allowance / under-balance
/// pull so we can prove the facet's CEI ordering (a failed escrow leaves
/// no ghost invite).
contract MockLH {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    function mint(address to, uint256 amt) external {
        balanceOf[to] += amt;
    }

    function approve(address spender, uint256 amt) external returns (bool) {
        allowance[msg.sender][spender] = amt;
        return true;
    }

    function transferFrom(address from, address to, uint256 amt) external returns (bool) {
        uint256 a = allowance[from][msg.sender];
        require(a >= amt, "allowance");
        require(balanceOf[from] >= amt, "balance");
        allowance[from][msg.sender] = a - amt;
        balanceOf[from] -= amt;
        balanceOf[to] += amt;
        return true;
    }

    function transfer(address to, uint256 amt) external returns (bool) {
        require(balanceOf[msg.sender] >= amt, "balance");
        balanceOf[msg.sender] -= amt;
        balanceOf[to] += amt;
        return true;
    }
}

/// Test harness: InviteFacet + a tiny setter that writes the SHARED
/// diamond-storage slot a real diamond populates via CreditsFacet
/// (`creditsToken`). Because `Lib*Storage.load()` resolves against THIS
/// contract's storage, writing it here IS the cross-facet storage sharing
/// the diamond provides — the facet under test reads it identically to
/// production. The diamond IS the escrow holder, so `address(this)` (the
/// harness) holds the escrowed `$LH`, exactly like the live diamond.
contract InviteHarness is InviteFacet {
    function _setCreditsToken(address token) external {
        LibCreditsStorage.load().creditsToken = token;
    }
}

contract InviteFacetTest is Test {
    InviteHarness inv;
    MockLH lh;

    address funder = address(0xF00D); // escrows their own $LH
    address newcomer = address(0xA11CE); // accepts an invite
    address poker = address(0xBEEF); // permissionless reclaim caller

    uint128 constant AMOUNT = 100 ether; // 100 $LH
    uint64 constant TTL = 24 hours;

    // Plaintext code + its on-chain key (the same hash acceptInvite uses).
    string constant CODE = "inv-100-Ab3xZ9qLmN";
    bytes32 CODE_HASH = keccak256(bytes(CODE));

    function setUp() public {
        inv = new InviteHarness();
        lh = new MockLH();
        inv._setCreditsToken(address(lh));

        // Fund the funder and pre-approve the diamond (the facet) for escrow.
        lh.mint(funder, 1_000_000 ether);
        vm.prank(funder);
        lh.approve(address(inv), type(uint256).max);

        // Pin a stable timestamp so expiry math is deterministic.
        vm.warp(1_000_000);
    }

    function _create() internal {
        vm.prank(funder);
        inv.createInvite(CODE_HASH, AMOUNT, TTL);
    }

    // --- createInvite: escrow + storage ---------------------------------

    function test_createInvite_escrows_and_stores() public {
        uint256 funderBefore = lh.balanceOf(funder);
        _create();

        // $LH moved funder -> diamond (the facet).
        assertEq(lh.balanceOf(funder), funderBefore - AMOUNT, "amount escrowed from funder");
        assertEq(lh.balanceOf(address(inv)), AMOUNT, "diamond holds the escrow");

        (address f, uint128 amt, uint64 exp, uint8 status) = inv.getInvite(CODE_HASH);
        assertEq(f, funder, "funder recorded");
        assertEq(amt, AMOUNT, "amount recorded");
        assertEq(exp, uint64(block.timestamp) + TTL, "expiry = now + ttl");
        assertEq(status, uint8(LibInviteStorage.Status.Open), "status Open");

        assertEq(inv.escrowedOf(funder), AMOUNT, "escrowedOf bumped");
    }

    // --- createInvite: reverts ------------------------------------------

    function test_createInvite_reverts_duplicate_codehash() public {
        _create();
        vm.prank(funder);
        vm.expectRevert(InviteFacet.CodeTaken.selector);
        inv.createInvite(CODE_HASH, AMOUNT, TTL); // same hash, still Open
    }

    function test_createInvite_reverts_duplicate_even_after_terminal() public {
        // A used (Accepted) code's funder is still non-zero, so the hash is
        // permanently taken — a code can never be re-funded after use.
        _create();
        vm.prank(newcomer);
        inv.acceptInvite(CODE);
        vm.prank(funder);
        vm.expectRevert(InviteFacet.CodeTaken.selector);
        inv.createInvite(CODE_HASH, AMOUNT, TTL);
    }

    function test_createInvite_reverts_zero_amount() public {
        vm.prank(funder);
        vm.expectRevert(InviteFacet.ZeroAmount.selector);
        inv.createInvite(CODE_HASH, 0, TTL);
    }

    function test_createInvite_reverts_ttl_too_short() public {
        vm.prank(funder);
        vm.expectRevert(InviteFacet.BadTtl.selector);
        inv.createInvite(CODE_HASH, AMOUNT, LibInviteStorage.MIN_TTL - 1);
    }

    function test_createInvite_reverts_ttl_too_long() public {
        vm.prank(funder);
        vm.expectRevert(InviteFacet.BadTtl.selector);
        inv.createInvite(CODE_HASH, AMOUNT, LibInviteStorage.MAX_TTL + 1);
    }

    function test_createInvite_accepts_ttl_bounds() public {
        // Exactly MIN_TTL and exactly MAX_TTL are both valid (inclusive).
        vm.prank(funder);
        inv.createInvite(keccak256("min"), AMOUNT, LibInviteStorage.MIN_TTL);
        vm.prank(funder);
        inv.createInvite(keccak256("max"), AMOUNT, LibInviteStorage.MAX_TTL);
        (, , uint64 expMin, ) = inv.getInvite(keccak256("min"));
        (, , uint64 expMax, ) = inv.getInvite(keccak256("max"));
        assertEq(expMin, uint64(block.timestamp) + LibInviteStorage.MIN_TTL);
        assertEq(expMax, uint64(block.timestamp) + LibInviteStorage.MAX_TTL);
    }

    function test_createInvite_reverts_escrow_cap_exceeded() public {
        // Fund + approve enough to TRY exceeding the cap, then a single
        // create over MAX_ESCROWED reverts.
        uint256 over = LibInviteStorage.MAX_ESCROWED + 1;
        lh.mint(funder, over); // ensure balance isn't the blocker
        vm.prank(funder);
        vm.expectRevert(InviteFacet.EscrowCapExceeded.selector);
        inv.createInvite(CODE_HASH, over, TTL);
    }

    function test_createInvite_escrow_cap_is_cumulative() public {
        // First create at the cap succeeds; a second that pushes the
        // running sum over the cap reverts (the sum is per-funder).
        lh.mint(funder, LibInviteStorage.MAX_ESCROWED);
        vm.prank(funder);
        inv.createInvite(keccak256("a"), uint128(LibInviteStorage.MAX_ESCROWED), TTL);
        vm.prank(funder);
        vm.expectRevert(InviteFacet.EscrowCapExceeded.selector);
        inv.createInvite(keccak256("b"), 1, TTL);
    }

    // --- CEI: a reverting escrow leaves no ghost invite -----------------

    function test_createInvite_no_ghost_when_escrow_fails() public {
        // A fresh under-funded funder: approve but no balance →
        // transferFrom reverts → the whole tx reverts, no invite persisted.
        address broke = address(0x0B0B);
        vm.prank(broke);
        lh.approve(address(inv), type(uint256).max);
        vm.prank(broke);
        vm.expectRevert(); // MockLH "balance"
        inv.createInvite(CODE_HASH, AMOUNT, TTL);

        (address f, uint128 amt, , uint8 status) = inv.getInvite(CODE_HASH);
        assertEq(f, address(0), "no ghost invite funder");
        assertEq(amt, 0, "no ghost amount");
        assertEq(status, uint8(LibInviteStorage.Status.Open), "default (unset) status");
        assertEq(inv.escrowedOf(broke), 0, "no ghost escrow accounting");
    }

    // --- acceptInvite: pays the accepter --------------------------------

    function test_acceptInvite_pays_accepter() public {
        _create();
        uint256 newcomerBefore = lh.balanceOf(newcomer);

        vm.prank(newcomer);
        uint256 paid = inv.acceptInvite(CODE);

        assertEq(paid, AMOUNT, "returns the amount");
        // $LH moved diamond -> accepter (NOT the funder).
        assertEq(lh.balanceOf(newcomer), newcomerBefore + AMOUNT, "accepter credited");
        assertEq(lh.balanceOf(address(inv)), 0, "diamond drained the escrow");

        (, , , uint8 status) = inv.getInvite(CODE_HASH);
        assertEq(status, uint8(LibInviteStorage.Status.Accepted), "status Accepted");
        assertEq(inv.escrowedOf(funder), 0, "escrowedOf decremented");
    }

    function test_acceptInvite_anyone_holding_code_accepts() public {
        // Bearer: a third party (not the named newcomer) accepts just by
        // submitting the plaintext.
        _create();
        vm.prank(poker);
        inv.acceptInvite(CODE);
        assertEq(lh.balanceOf(poker), AMOUNT, "any bearer of the code accepts");
    }

    function test_acceptInvite_reverts_unknown_code() public {
        vm.prank(newcomer);
        vm.expectRevert(InviteFacet.UnknownInvite.selector);
        inv.acceptInvite("never-created");
    }

    function test_acceptInvite_reverts_double_accept() public {
        _create();
        vm.prank(newcomer);
        inv.acceptInvite(CODE);
        // Second accept re-reads status != Open → NotOpen.
        vm.prank(poker);
        vm.expectRevert(InviteFacet.NotOpen.selector);
        inv.acceptInvite(CODE);
    }

    function test_acceptInvite_reverts_after_expiry() public {
        _create();
        // Move strictly past expiry: accept window is now <= expiry.
        vm.warp(block.timestamp + TTL + 1);
        vm.prank(newcomer);
        vm.expectRevert(InviteFacet.Expired.selector);
        inv.acceptInvite(CODE);
    }

    function test_acceptInvite_at_exact_expiry_still_ok() public {
        _create();
        ( , , uint64 exp, ) = inv.getInvite(CODE_HASH);
        vm.warp(exp); // now == expiry is still acceptable (now <= expiry)
        vm.prank(newcomer);
        inv.acceptInvite(CODE);
        assertEq(lh.balanceOf(newcomer), AMOUNT, "accept at the exact expiry second");
    }

    // --- reclaimInvite: refunds the FUNDER ------------------------------

    function test_reclaimInvite_refunds_funder() public {
        _create();
        uint256 funderBefore = lh.balanceOf(funder);

        // Expire it, then a permissionless poker reclaims.
        vm.warp(block.timestamp + TTL + 1);
        vm.prank(poker);
        inv.reclaimInvite(CODE_HASH);

        // Refund goes to the FUNDER, never the caller (the poker).
        assertEq(lh.balanceOf(funder), funderBefore + AMOUNT, "funder refunded 100%");
        assertEq(lh.balanceOf(poker), 0, "permissionless caller gains nothing");
        assertEq(lh.balanceOf(address(inv)), 0, "diamond drained the escrow");

        (, , , uint8 status) = inv.getInvite(CODE_HASH);
        assertEq(status, uint8(LibInviteStorage.Status.Reclaimed), "status Reclaimed");
        assertEq(inv.escrowedOf(funder), 0, "escrowedOf decremented");
    }

    function test_reclaimInvite_is_permissionless() public {
        // Even the funder isn't required to call — anyone can poke it, and
        // the funder still gets the money. (Covered above via the poker;
        // here assert the funder calling it themselves also works.)
        _create();
        vm.warp(block.timestamp + TTL + 1);
        uint256 funderBefore = lh.balanceOf(funder);
        vm.prank(funder);
        inv.reclaimInvite(CODE_HASH);
        assertEq(lh.balanceOf(funder), funderBefore + AMOUNT);
    }

    function test_reclaimInvite_reverts_before_expiry() public {
        _create();
        // Reclaim window is now > expiry; before that it's NotYetExpired.
        vm.prank(poker);
        vm.expectRevert(InviteFacet.NotYetExpired.selector);
        inv.reclaimInvite(CODE_HASH);
    }

    function test_reclaimInvite_reverts_at_exact_expiry() public {
        _create();
        ( , , uint64 exp, ) = inv.getInvite(CODE_HASH);
        vm.warp(exp); // now == expiry: still acceptable, NOT yet reclaimable
        vm.prank(poker);
        vm.expectRevert(InviteFacet.NotYetExpired.selector);
        inv.reclaimInvite(CODE_HASH);
    }

    function test_reclaimInvite_reverts_unknown_code() public {
        vm.warp(block.timestamp + TTL + 1);
        vm.prank(poker);
        vm.expectRevert(InviteFacet.UnknownInvite.selector);
        inv.reclaimInvite(keccak256("nope"));
    }

    function test_reclaimInvite_reverts_double_reclaim() public {
        _create();
        vm.warp(block.timestamp + TTL + 1);
        vm.prank(poker);
        inv.reclaimInvite(CODE_HASH);
        // Second reclaim re-reads status != Open → NotOpen.
        vm.prank(poker);
        vm.expectRevert(InviteFacet.NotOpen.selector);
        inv.reclaimInvite(CODE_HASH);
    }

    // --- The two windows are disjoint (accept XOR reclaim) --------------

    function test_accepted_invite_cannot_be_reclaimed() public {
        _create();
        vm.prank(newcomer);
        inv.acceptInvite(CODE);
        // Expire it; still can't reclaim — it's already Accepted.
        vm.warp(block.timestamp + TTL + 1);
        vm.prank(funder);
        vm.expectRevert(InviteFacet.NotOpen.selector);
        inv.reclaimInvite(CODE_HASH);
    }

    function test_reclaimed_invite_cannot_be_accepted() public {
        _create();
        vm.warp(block.timestamp + TTL + 1);
        vm.prank(poker);
        inv.reclaimInvite(CODE_HASH);
        // Reclaimed is terminal; a late accepter (even with the code) is
        // blocked by status (the Expired guard would also catch it).
        vm.prank(newcomer);
        vm.expectRevert(InviteFacet.NotOpen.selector);
        inv.acceptInvite(CODE);
    }

    // --- escrowedOf accounting across the full lifecycle ----------------

    function test_escrowedOf_accounting_across_lifecycle() public {
        assertEq(inv.escrowedOf(funder), 0, "starts at 0");

        // Two open invites accumulate. c1 uses a real plaintext so it can
        // be accepted by code; c2 is keyed by a raw hash (reclaim path).
        string memory c1Code = "inv-100-First00001";
        vm.prank(funder);
        inv.createInvite(keccak256(bytes(c1Code)), AMOUNT, TTL);
        vm.prank(funder);
        inv.createInvite(keccak256("c2"), 50 ether, TTL);
        assertEq(inv.escrowedOf(funder), AMOUNT + 50 ether, "sum of open invites");

        // Accepting c1 decrements the running sum by c1's amount.
        vm.prank(newcomer);
        inv.acceptInvite(c1Code);
        assertEq(inv.escrowedOf(funder), 50 ether, "accept decremented c1's amount");

        // Reclaim c2 after expiry decrements by c2's amount -> back to 0.
        vm.warp(block.timestamp + TTL + 1);
        vm.prank(poker);
        inv.reclaimInvite(keccak256("c2"));
        assertEq(inv.escrowedOf(funder), 0, "all escrow released");
    }
}
