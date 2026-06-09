// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {BountyFacet} from "../src/facets/BountyFacet.sol";
import {LibBountyStorage} from "../src/libraries/LibBountyStorage.sol";
import {LibCreditsStorage} from "../src/libraries/LibCreditsStorage.sol";
import {LibRegistryStorage} from "../src/libraries/LibRegistryStorage.sol";

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

contract BountyHarness is BountyFacet {
    mapping(uint256 => address) internal _tba;

    function _setCreditsToken(address token) external {
        LibCreditsStorage.load().creditsToken = token;
    }

    function _registerIdentity(uint256 id, address owner) external {
        LibRegistryStorage.load().ownerOfId[id] = owner;
    }

    function _setTba(uint256 tokenId, address tba) external {
        _tba[tokenId] = tba;
    }

    function tokenBoundAccount(uint256 tokenId) external view returns (address) {
        return _tba[tokenId];
    }
}

/// Adversarial suite for the Bounty REFUND/RECLAIM exits — the worker-griefs-
/// poster hard stop. Pins the lifecycle exclusivity (a bounty settles via
/// EXACTLY one terminal exit) + the per-poster active-count integrity across
/// the reclaim path (the path the main escrow-conservation fuzz omitted), and
/// adds a reclaim-inclusive conservation fuzz.
contract BountyReclaimAdversarialTest is Test {
    BountyHarness b;
    MockLH lh;

    address poster = address(0xF00D);
    address workerEoa = address(0xA11CE);
    address stranger = address(0xBEEF);
    address workerTba = address(0x7BA);

    uint256 constant WORKER_ID = 7;
    uint128 constant REWARD = 100 ether;
    uint64 constant TTL = 24 hours;

    bytes constant TASK = bytes("ipfs://task");
    bytes constant RESULT = bytes("ipfs://result");

    function setUp() public {
        b = new BountyHarness();
        lh = new MockLH();
        b._setCreditsToken(address(lh));
        b._registerIdentity(WORKER_ID, address(0xCAFE));
        b._setTba(WORKER_ID, workerTba);
        lh.mint(poster, 1_000_000 ether);
        vm.prank(poster);
        lh.approve(address(b), type(uint256).max);
        vm.warp(1_000_000);
    }

    function _post() internal returns (uint256 id) {
        vm.prank(poster);
        id = b.postBounty(TASK, REWARD, TTL);
    }

    function _claim(uint256 id) internal {
        vm.prank(workerEoa);
        b.claimBounty(id, WORKER_ID);
    }

    function _submit(uint256 id) internal {
        vm.prank(workerEoa);
        b.submitResult(id, RESULT);
    }

    // =====================================================================
    // RECLAIM exclusivity: a reclaimed bounty is terminal — no second
    // refund, no accept-after-reclaim, no cancel-after-reclaim. Refund goes
    // to the POSTER, never the poker.
    // =====================================================================

    /// Reclaim refunds the POSTER (not the permissionless poker), exactly
    /// once, and frees the active-count slot.
    function test_reclaim_from_claimed_refunds_poster_once_and_frees_slot() public {
        uint256 id = _post();
        _claim(id); // worker squats then never delivers
        assertEq(b.activeBountyCountOf(poster), 1);

        vm.warp(block.timestamp + TTL + 1);
        uint256 posterBefore = lh.balanceOf(poster);
        // A STRANGER pokes it; the refund still goes to the poster.
        vm.prank(stranger);
        b.reclaimExpired(id);
        assertEq(lh.balanceOf(poster), posterBefore + REWARD, "poster refunded, not the poker");
        assertEq(lh.balanceOf(stranger), 0, "poker gains nothing");
        assertEq(b.activeBountyCountOf(poster), 0, "slot freed");
        assertEq(lh.balanceOf(address(b)), 0, "escrow fully released");
    }

    function test_double_reclaim_no_second_refund() public {
        uint256 id = _post();
        vm.warp(block.timestamp + TTL + 1);
        vm.prank(stranger);
        b.reclaimExpired(id);
        vm.prank(stranger);
        vm.expectRevert(BountyFacet.NotReclaimable.selector);
        b.reclaimExpired(id);
    }

    function test_accept_after_reclaim_reverts() public {
        uint256 id = _post();
        _claim(id);
        _submit(id); // Submitted, but...
        vm.warp(block.timestamp + TTL + 1);
        vm.prank(stranger);
        b.reclaimExpired(id); // ...reclaimed first (refunds poster)
        // The poster can no longer "accept" and double-pay the worker.
        vm.prank(poster);
        vm.expectRevert(BountyFacet.NotSubmitted.selector);
        b.acceptResult(id);
        assertEq(lh.balanceOf(workerTba), 0, "worker never paid after a reclaim");
    }

    function test_cancel_after_reclaim_reverts() public {
        uint256 id = _post();
        vm.warp(block.timestamp + TTL + 1);
        vm.prank(stranger);
        b.reclaimExpired(id);
        vm.prank(poster);
        vm.expectRevert(BountyFacet.NotOpen.selector);
        b.cancelBounty(id);
    }

    /// A claimed-then-reclaimed bounty cannot be re-claimed (the slot is
    /// terminal — no zombie claim that strands escrow).
    function test_reclaimed_bounty_cannot_be_reclaimed_or_claimed_again() public {
        uint256 id = _post();
        _claim(id);
        vm.warp(block.timestamp + TTL + 1);
        vm.prank(stranger);
        b.reclaimExpired(id);
        // Can't claim a terminal bounty.
        vm.prank(workerEoa);
        vm.expectRevert(BountyFacet.NotOpen.selector);
        b.claimBounty(id, WORKER_ID);
    }

    /// Accept BEFORE expiry still wins over a (not-yet-valid) reclaim — and
    /// once Paid, reclaim is refused (no refund-after-payout double-spend).
    function test_paid_then_reclaim_refused_no_double_spend() public {
        uint256 id = _post();
        _claim(id);
        _submit(id);
        vm.prank(poster);
        b.acceptResult(id); // Paid, worker got REWARD
        assertEq(lh.balanceOf(workerTba), REWARD);

        vm.warp(block.timestamp + TTL + 1);
        vm.prank(stranger);
        vm.expectRevert(BountyFacet.NotReclaimable.selector);
        b.reclaimExpired(id);
        // Diamond holds nothing extra to refund — the escrow already paid out.
        assertEq(lh.balanceOf(address(b)), 0, "no escrow left to double-refund");
    }

    // =====================================================================
    // FUZZ: escrow conservation INCLUDING the reclaim path (the exit the
    // BountyFacet.t.sol fuzz omits) — the diamond's $LH always equals the
    // sum of live (Open/Claimed/Submitted) escrows across post / claim /
    // submit / accept / cancel / RECLAIM cycles.
    // =====================================================================
    function testFuzz_escrow_conservation_with_reclaim(uint256 seedRaw) public {
        uint256 seed = seedRaw;
        assertEq(lh.balanceOf(address(b)), 0, "diamond starts empty");

        for (uint256 i = 0; i < 50; i++) {
            seed = uint256(keccak256(abi.encode(seed, i)));
            uint256 action = seed % 6;
            uint256 n = b.bountyCount();

            if (action == 0) {
                // POST (respect the per-poster cap).
                if (b.activeBountyCountOf(poster) < LibBountyStorage.MAX_ACTIVE_PER_POSTER) {
                    uint128 reward = uint128(1 + (seed % 500) * 1 ether);
                    vm.prank(poster);
                    b.postBounty(TASK, reward, TTL);
                }
            } else if (n > 0) {
                uint256 id = 1 + (seed % n);
                (, , , uint8 st, ) = b.getBounty(id);

                if (action == 1 && st == uint8(LibBountyStorage.Status.Open)) {
                    // May revert Expired (time was warped by a prior reclaim
                    // action) — a legitimate facet revert; swallow it, the
                    // conservation invariant must hold either way.
                    vm.prank(workerEoa);
                    try b.claimBounty(id, WORKER_ID) {} catch {}
                } else if (action == 2 && st == uint8(LibBountyStorage.Status.Claimed)) {
                    vm.prank(workerEoa);
                    b.submitResult(id, RESULT);
                } else if (action == 3 && st == uint8(LibBountyStorage.Status.Submitted)) {
                    vm.prank(poster);
                    b.acceptResult(id);
                } else if (action == 4 && st == uint8(LibBountyStorage.Status.Open)) {
                    vm.prank(poster);
                    b.cancelBounty(id);
                } else if (action == 5) {
                    // RECLAIM: warp past expiry first, then poke. Only the
                    // non-terminal states are reclaimable; others revert and
                    // we swallow it (the invariant must still hold).
                    vm.warp(block.timestamp + TTL + 2);
                    try b.reclaimExpired(id) {} catch {}
                }
            }

            // INVARIANT after every step.
            assertEq(
                lh.balanceOf(address(b)),
                _sumLiveEscrow(),
                "diamond $LH == sum of live bounty rewards (incl. reclaim path)"
            );
        }
    }

    function _sumLiveEscrow() internal view returns (uint256 sum) {
        uint256 n = b.bountyCount();
        for (uint256 id = 1; id <= n; id++) {
            (, uint128 rw, , uint8 st, ) = b.getBounty(id);
            if (
                st == uint8(LibBountyStorage.Status.Open)
                    || st == uint8(LibBountyStorage.Status.Claimed)
                    || st == uint8(LibBountyStorage.Status.Submitted)
            ) {
                sum += rw;
            }
        }
    }
}
