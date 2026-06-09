// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {BountyFacet} from "../src/facets/BountyFacet.sol";
import {LibBountyStorage} from "../src/libraries/LibBountyStorage.sol";
import {LibCreditsStorage} from "../src/libraries/LibCreditsStorage.sol";
import {LibRegistryStorage} from "../src/libraries/LibRegistryStorage.sol";

/// Minimal `$LH`-shaped TIP-20 mock: 18-decimal balances + the
/// approve/transferFrom/transfer surface BountyFacet escrows + pays out +
/// refunds through. Reverts (via require) on an under-allowance /
/// under-balance pull so the facet's CEI ordering is provable (a failed
/// escrow leaves no ghost bounty).
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

/// Hostile reentrant TIP-20 mock: on `transfer` (the payout/refund path —
/// the only external call in accept/cancel/reclaim) it re-enters the
/// diamond, trying a SECOND settlement of the same bounty. Real `$LH` has
/// NO callback; this is the defense-in-depth probe that CEI ordering makes
/// a double-payout / double-refund structurally impossible (the re-entry
/// re-reads a terminal status and reverts).
contract ReentrantLH {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    address public diamond;
    uint256 public attackId;
    uint8 public mode; // 0=accept, 1=cancel, 2=reclaim
    bool internal entered;
    bool public reenterReverted;

    function arm(address d, uint256 id, uint8 m) external {
        diamond = d;
        attackId = id;
        mode = m;
        entered = false;
        reenterReverted = false;
    }

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
        // Re-enter ONCE during the settlement transfer: try to settle the
        // same bounty a second time. CEI means the status is already
        // terminal, so this MUST revert (no double drain).
        if (diamond != address(0) && !entered) {
            entered = true;
            if (mode == 0) {
                try BountyFacet(diamond).acceptResult(attackId) {
                    reenterReverted = false;
                } catch {
                    reenterReverted = true;
                }
            } else if (mode == 1) {
                try BountyFacet(diamond).cancelBounty(attackId) {
                    reenterReverted = false;
                } catch {
                    reenterReverted = true;
                }
            } else {
                try BountyFacet(diamond).reclaimExpired(attackId) {
                    reenterReverted = false;
                } catch {
                    reenterReverted = true;
                }
            }
        }
        return true;
    }
}

/// Test harness: BountyFacet + setters that write the SHARED diamond-
/// storage slots a real diamond populates via other facets (creditsToken
/// from CreditsFacet, ownerOfId from the registry) AND a real
/// `tokenBoundAccount` implementation so the acceptResult self-call
/// (`ITbaResolver(address(this)).tokenBoundAccount`) resolves to a
/// deterministic worker wallet. Because every `Lib*Storage.load()` resolves
/// against THIS contract's storage, writing them here IS the cross-facet
/// storage sharing the diamond provides — the facet reads them identically
/// to production. The diamond IS the escrow holder, so `address(this)`
/// holds the escrowed `$LH`, exactly like the live diamond.
contract BountyHarness is BountyFacet {
    // tokenId -> the (test-fixed) TBA address. address(0) = "unresolved".
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

    /// Satisfies ITbaResolver — the selector acceptResult self-calls.
    function tokenBoundAccount(uint256 tokenId) external view returns (address) {
        return _tba[tokenId];
    }
}

contract BountyFacetTest is Test {
    BountyHarness b;
    MockLH lh;

    address poster = address(0xF00D); // escrows the reward
    address workerEoa = address(0xA11CE); // calls claim/submit (the worker's key)
    address stranger = address(0xBEEF); // poker / non-poster
    address workerTba = address(0x7BA); // the worker identity's TBA (payout target)

    uint256 constant WORKER_ID = 7; // the claimant's registered tokenId
    uint128 constant REWARD = 100 ether; // 100 $LH
    uint64 constant TTL = 24 hours;

    bytes constant TASK = bytes("ipfs://bafy.../fix-the-rustlite-bug");
    bytes constant RESULT = bytes("ipfs://bafy.../the-patch");

    function setUp() public {
        b = new BountyHarness();
        lh = new MockLH();
        b._setCreditsToken(address(lh));

        // Register the worker identity + bind its TBA (the payout target).
        b._registerIdentity(WORKER_ID, address(0xCAFE));
        b._setTba(WORKER_ID, workerTba);

        // Fund the poster + pre-approve the diamond (the facet) for escrow.
        lh.mint(poster, 1_000_000 ether);
        vm.prank(poster);
        lh.approve(address(b), type(uint256).max);

        // Pin a stable timestamp so expiry math is deterministic.
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
    // postBounty: escrow + storage + validation
    // =====================================================================

    function test_postBounty_escrows_and_stores() public {
        uint256 posterBefore = lh.balanceOf(poster);
        uint256 id = _post();

        assertEq(id, 1, "first bounty id is 1");
        assertEq(lh.balanceOf(poster), posterBefore - REWARD, "reward escrowed from poster");
        assertEq(lh.balanceOf(address(b)), REWARD, "diamond holds the escrow");

        (address p, uint128 rw, uint64 exp, uint8 st, uint256 cid) = b.getBounty(id);
        assertEq(p, poster, "poster recorded");
        assertEq(rw, REWARD, "reward recorded");
        assertEq(exp, uint64(block.timestamp) + TTL, "expiry = now + ttl");
        assertEq(st, uint8(LibBountyStorage.Status.Open), "status Open");
        assertEq(cid, 0, "no claimant yet");

        assertEq(string(b.bountyTaskOf(id)), string(TASK), "task stored");
        assertEq(b.resultOf(id).length, 0, "no result yet");
        assertEq(b.bountyCount(), 1);
        assertEq(b.activeBountyCountOf(poster), 1, "active count bumped");

        uint256[] memory mine = b.bountiesOf(poster);
        assertEq(mine.length, 1);
        assertEq(mine[0], id);
    }

    function test_postBounty_reverts_zero_reward() public {
        vm.prank(poster);
        vm.expectRevert(BountyFacet.ZeroReward.selector);
        b.postBounty(TASK, 0, TTL);
    }

    function test_postBounty_reverts_ttl_too_short() public {
        vm.prank(poster);
        vm.expectRevert(BountyFacet.BadTtl.selector);
        b.postBounty(TASK, REWARD, LibBountyStorage.MIN_TTL - 1);
    }

    function test_postBounty_reverts_ttl_too_long() public {
        vm.prank(poster);
        vm.expectRevert(BountyFacet.BadTtl.selector);
        b.postBounty(TASK, REWARD, LibBountyStorage.MAX_TTL + 1);
    }

    function test_postBounty_accepts_ttl_bounds() public {
        vm.prank(poster);
        uint256 idMin = b.postBounty(TASK, REWARD, LibBountyStorage.MIN_TTL);
        vm.prank(poster);
        uint256 idMax = b.postBounty(TASK, REWARD, LibBountyStorage.MAX_TTL);
        (, , uint64 expMin, , ) = b.getBounty(idMin);
        (, , uint64 expMax, , ) = b.getBounty(idMax);
        assertEq(expMin, uint64(block.timestamp) + LibBountyStorage.MIN_TTL);
        assertEq(expMax, uint64(block.timestamp) + LibBountyStorage.MAX_TTL);
    }

    function test_postBounty_reverts_task_too_large() public {
        bytes memory big = new bytes(LibBountyStorage.MAX_TASK_BYTES + 1);
        vm.prank(poster);
        vm.expectRevert(BountyFacet.TaskTooLarge.selector);
        b.postBounty(big, REWARD, TTL);
    }

    function test_postBounty_accepts_task_at_cap() public {
        bytes memory atCap = new bytes(LibBountyStorage.MAX_TASK_BYTES);
        vm.prank(poster);
        uint256 id = b.postBounty(atCap, REWARD, TTL);
        assertEq(b.bountyTaskOf(id).length, LibBountyStorage.MAX_TASK_BYTES);
    }

    function test_postBounty_reverts_not_configured() public {
        // A fresh harness with no credits token set.
        BountyHarness fresh = new BountyHarness();
        vm.prank(poster);
        vm.expectRevert(BountyFacet.NotConfigured.selector);
        fresh.postBounty(TASK, REWARD, TTL);
    }

    function test_postBounty_no_ghost_when_escrow_fails() public {
        // A broke poster: approve but no balance → transferFrom reverts →
        // the whole tx reverts, no bounty persisted, no id consumed.
        address broke = address(0x0B0B);
        vm.prank(broke);
        lh.approve(address(b), type(uint256).max);
        vm.prank(broke);
        vm.expectRevert(); // MockLH "balance"
        b.postBounty(TASK, REWARD, TTL);

        (address p, uint128 rw, , uint8 st, ) = b.getBounty(1);
        assertEq(p, address(0), "no ghost bounty");
        assertEq(rw, 0, "no ghost reward");
        assertEq(st, uint8(LibBountyStorage.Status.Open), "default (unset) status");
        assertEq(b.bountyCount(), 0, "no id consumed on a failed escrow");
        assertEq(b.activeBountyCountOf(broke), 0, "no ghost active count");
    }

    // --- anti-sybil: per-poster active cap ------------------------------

    function test_postBounty_reverts_at_active_cap() public {
        // Use minimal rewards to stay well within the balance.
        for (uint256 i = 0; i < LibBountyStorage.MAX_ACTIVE_PER_POSTER; i++) {
            vm.prank(poster);
            b.postBounty(TASK, 1, TTL);
        }
        assertEq(b.activeBountyCountOf(poster), LibBountyStorage.MAX_ACTIVE_PER_POSTER, "at cap");
        vm.prank(poster);
        vm.expectRevert(BountyFacet.TooManyActiveBounties.selector);
        b.postBounty(TASK, 1, TTL);
    }

    function test_active_cap_no_escrow_pulled_on_cap_revert() public {
        for (uint256 i = 0; i < LibBountyStorage.MAX_ACTIVE_PER_POSTER; i++) {
            vm.prank(poster);
            b.postBounty(TASK, 1, TTL);
        }
        uint256 balBefore = lh.balanceOf(poster);
        vm.prank(poster);
        vm.expectRevert(BountyFacet.TooManyActiveBounties.selector);
        b.postBounty(TASK, 5 ether, TTL);
        assertEq(lh.balanceOf(poster), balBefore, "cap revert pulls no escrow");
    }

    function test_active_cap_frees_a_slot_on_terminal_exit() public {
        uint256[] memory ids = new uint256[](LibBountyStorage.MAX_ACTIVE_PER_POSTER);
        for (uint256 i = 0; i < LibBountyStorage.MAX_ACTIVE_PER_POSTER; i++) {
            vm.prank(poster);
            ids[i] = b.postBounty(TASK, 1, TTL);
        }
        // At cap -> next reverts.
        vm.prank(poster);
        vm.expectRevert(BountyFacet.TooManyActiveBounties.selector);
        b.postBounty(TASK, 1, TTL);

        // Cancel one -> count drops -> a new post succeeds.
        vm.prank(poster);
        b.cancelBounty(ids[0]);
        assertEq(
            b.activeBountyCountOf(poster),
            LibBountyStorage.MAX_ACTIVE_PER_POSTER - 1,
            "cancel decremented"
        );
        vm.prank(poster);
        uint256 fresh = b.postBounty(TASK, 1, TTL);
        assertGt(fresh, 0, "post allowed after a slot frees");
        assertEq(b.activeBountyCountOf(poster), LibBountyStorage.MAX_ACTIVE_PER_POSTER, "back at cap");
    }

    // =====================================================================
    // claimBounty
    // =====================================================================

    function test_claimBounty_reserves_and_records_identity() public {
        uint256 id = _post();
        _claim(id);
        (, , , uint8 st, uint256 cid) = b.getBounty(id);
        assertEq(st, uint8(LibBountyStorage.Status.Claimed), "status Claimed");
        assertEq(cid, WORKER_ID, "claimant identity recorded");
    }

    function test_claimBounty_reverts_unknown_bounty() public {
        vm.prank(workerEoa);
        vm.expectRevert(BountyFacet.UnknownBounty.selector);
        b.claimBounty(999, WORKER_ID);
    }

    function test_claimBounty_reverts_when_not_open() public {
        uint256 id = _post();
        _claim(id); // now Claimed
        vm.prank(stranger);
        vm.expectRevert(BountyFacet.NotOpen.selector);
        b.claimBounty(id, WORKER_ID); // already claimed (first-come reserve)
    }

    function test_claimBounty_reverts_after_expiry() public {
        uint256 id = _post();
        vm.warp(block.timestamp + TTL + 1);
        vm.prank(workerEoa);
        vm.expectRevert(BountyFacet.Expired.selector);
        b.claimBounty(id, WORKER_ID);
    }

    function test_claimBounty_at_exact_expiry_still_ok() public {
        uint256 id = _post();
        (, , uint64 exp, , ) = b.getBounty(id);
        vm.warp(exp); // now == expiry is still claimable (now <= expiry)
        vm.prank(workerEoa);
        b.claimBounty(id, WORKER_ID);
        (, , , uint8 st, ) = b.getBounty(id);
        assertEq(st, uint8(LibBountyStorage.Status.Claimed));
    }

    function test_claimBounty_reverts_unknown_claimant_identity() public {
        uint256 id = _post();
        vm.prank(workerEoa);
        vm.expectRevert(BountyFacet.UnknownClaimant.selector);
        b.claimBounty(id, 4242); // tokenId 4242 not registered
    }

    function test_claim_as_someone_else_pays_that_someone_else() public {
        // The claim-squat / trust note in code: claiming with a DIFFERENT
        // identity's tokenId binds the payout to THAT identity's TBA — no
        // theft, just a gift. Register a second identity with its own TBA;
        // a stranger claims AS that identity; the accept pays that TBA.
        uint256 otherId = 9;
        address otherTba = address(0x0DEF);
        b._registerIdentity(otherId, address(0xBEAD));
        b._setTba(otherId, otherTba);

        uint256 id = _post();
        // A stranger (not the other identity's owner) claims AS otherId.
        vm.prank(stranger);
        b.claimBounty(id, otherId);
        vm.prank(workerEoa);
        b.submitResult(id, RESULT);

        uint256 otherBefore = lh.balanceOf(otherTba);
        vm.prank(poster);
        b.acceptResult(id);
        // The reward landed on otherId's TBA — the recorded claimant — NOT
        // the stranger who called claim.
        assertEq(lh.balanceOf(otherTba), otherBefore + REWARD, "payout bound to recorded identity TBA");
        assertEq(lh.balanceOf(stranger), 0, "the claim caller gains nothing");
    }

    // =====================================================================
    // submitResult
    // =====================================================================

    function test_submitResult_stores_and_advances() public {
        uint256 id = _post();
        _claim(id);
        _submit(id);
        (, , , uint8 st, ) = b.getBounty(id);
        assertEq(st, uint8(LibBountyStorage.Status.Submitted), "status Submitted");
        assertEq(string(b.resultOf(id)), string(RESULT), "result stored");
    }

    function test_submitResult_reverts_unknown_bounty() public {
        vm.prank(workerEoa);
        vm.expectRevert(BountyFacet.UnknownBounty.selector);
        b.submitResult(999, RESULT);
    }

    function test_submitResult_reverts_when_not_claimed() public {
        uint256 id = _post(); // still Open, never claimed
        vm.prank(workerEoa);
        vm.expectRevert(BountyFacet.NotClaimed.selector);
        b.submitResult(id, RESULT);
    }

    function test_submitResult_reverts_double_submit() public {
        uint256 id = _post();
        _claim(id);
        _submit(id); // Submitted
        vm.prank(workerEoa);
        vm.expectRevert(BountyFacet.NotClaimed.selector);
        b.submitResult(id, RESULT); // can't submit twice
    }

    function test_submitResult_reverts_result_too_large() public {
        uint256 id = _post();
        _claim(id);
        bytes memory big = new bytes(LibBountyStorage.MAX_RESULT_BYTES + 1);
        vm.prank(workerEoa);
        vm.expectRevert(BountyFacet.ResultTooLarge.selector);
        b.submitResult(id, big);
    }

    // =====================================================================
    // acceptResult: payout to the worker TBA
    // =====================================================================

    function test_acceptResult_pays_worker_tba_and_finalizes() public {
        uint256 id = _post();
        _claim(id);
        _submit(id);

        uint256 tbaBefore = lh.balanceOf(workerTba);
        vm.prank(poster);
        b.acceptResult(id);

        // Reward moved diamond -> the WORKER IDENTITY's TBA (not the EOA).
        assertEq(lh.balanceOf(workerTba), tbaBefore + REWARD, "worker TBA paid the reward");
        assertEq(lh.balanceOf(workerEoa), 0, "the claiming EOA is NOT the payee");
        assertEq(lh.balanceOf(address(b)), 0, "diamond drained the escrow");

        (, , , uint8 st, ) = b.getBounty(id);
        assertEq(st, uint8(LibBountyStorage.Status.Paid), "status Paid");
        assertEq(b.activeBountyCountOf(poster), 0, "active count decremented on accept");
    }

    function test_acceptResult_only_poster() public {
        uint256 id = _post();
        _claim(id);
        _submit(id);
        vm.prank(stranger);
        vm.expectRevert(BountyFacet.NotPoster.selector);
        b.acceptResult(id);
    }

    function test_acceptResult_reverts_unknown_bounty() public {
        vm.prank(poster);
        vm.expectRevert(BountyFacet.UnknownBounty.selector);
        b.acceptResult(999);
    }

    function test_acceptResult_reverts_when_not_submitted() public {
        uint256 id = _post();
        _claim(id); // Claimed, no submit
        vm.prank(poster);
        vm.expectRevert(BountyFacet.NotSubmitted.selector);
        b.acceptResult(id);
    }

    function test_acceptResult_reverts_double_accept() public {
        uint256 id = _post();
        _claim(id);
        _submit(id);
        vm.prank(poster);
        b.acceptResult(id); // Paid
        vm.prank(poster);
        vm.expectRevert(BountyFacet.NotSubmitted.selector);
        b.acceptResult(id); // can't pay twice
    }

    function test_acceptResult_reverts_when_tba_unresolved() public {
        // A claimant identity whose TBA resolves to address(0) — accept must
        // refuse (never burn the reward to the zero address). The bounty
        // stays Submitted so the poster can retry once the TBA is deployed.
        uint256 noTbaId = 11;
        b._registerIdentity(noTbaId, address(0xAB1E)); // registered, but no _setTba
        uint256 id = _post();
        vm.prank(workerEoa);
        b.claimBounty(id, noTbaId);
        _submit(id);
        vm.prank(poster);
        vm.expectRevert(BountyFacet.TbaUnresolved.selector);
        b.acceptResult(id);
        // Unchanged: still Submitted, escrow intact, count intact.
        (, , , uint8 st, ) = b.getBounty(id);
        assertEq(st, uint8(LibBountyStorage.Status.Submitted), "stays Submitted on a TBA revert");
        assertEq(lh.balanceOf(address(b)), REWARD, "escrow untouched");
        assertEq(b.activeBountyCountOf(poster), 1, "active count untouched");
    }

    // =====================================================================
    // cancelBounty: poster refund while Open
    // =====================================================================

    function test_cancelBounty_refunds_poster() public {
        uint256 id = _post();
        uint256 posterBefore = lh.balanceOf(poster);
        vm.prank(poster);
        b.cancelBounty(id);

        assertEq(lh.balanceOf(poster), posterBefore + REWARD, "poster refunded 100%");
        assertEq(lh.balanceOf(address(b)), 0, "diamond drained");
        (, , , uint8 st, ) = b.getBounty(id);
        assertEq(st, uint8(LibBountyStorage.Status.Cancelled), "status Cancelled");
        assertEq(b.activeBountyCountOf(poster), 0, "active count decremented");
    }

    function test_cancelBounty_only_poster() public {
        uint256 id = _post();
        vm.prank(stranger);
        vm.expectRevert(BountyFacet.NotPoster.selector);
        b.cancelBounty(id);
    }

    function test_cancelBounty_reverts_after_claim() public {
        // A claimed bounty can't be cancelled out from under the worker —
        // the poster must wait for expiry + reclaim.
        uint256 id = _post();
        _claim(id);
        vm.prank(poster);
        vm.expectRevert(BountyFacet.NotOpen.selector);
        b.cancelBounty(id);
    }

    function test_cancelBounty_reverts_unknown() public {
        vm.prank(poster);
        vm.expectRevert(BountyFacet.UnknownBounty.selector);
        b.cancelBounty(999);
    }

    function test_cancelBounty_reverts_double_cancel() public {
        uint256 id = _post();
        vm.prank(poster);
        b.cancelBounty(id);
        vm.prank(poster);
        vm.expectRevert(BountyFacet.NotOpen.selector);
        b.cancelBounty(id);
    }

    // =====================================================================
    // reclaimExpired: poster refund after deadline
    // =====================================================================

    function test_reclaimExpired_refunds_poster_from_open() public {
        uint256 id = _post();
        vm.warp(block.timestamp + TTL + 1);
        uint256 posterBefore = lh.balanceOf(poster);
        // Permissionless: a stranger pokes it; the POSTER gets the money.
        vm.prank(stranger);
        b.reclaimExpired(id);

        assertEq(lh.balanceOf(poster), posterBefore + REWARD, "poster refunded 100%");
        assertEq(lh.balanceOf(stranger), 0, "the poker gains nothing");
        (, , , uint8 st, ) = b.getBounty(id);
        assertEq(st, uint8(LibBountyStorage.Status.Reclaimed), "status Reclaimed");
        assertEq(b.activeBountyCountOf(poster), 0, "active count decremented");
    }

    function test_reclaimExpired_works_from_claimed() public {
        // The worker-griefs-poster case: claimed but never delivered.
        uint256 id = _post();
        _claim(id);
        vm.warp(block.timestamp + TTL + 1);
        uint256 posterBefore = lh.balanceOf(poster);
        vm.prank(stranger);
        b.reclaimExpired(id);
        assertEq(lh.balanceOf(poster), posterBefore + REWARD, "refund from Claimed");
    }

    function test_reclaimExpired_works_from_submitted() public {
        // Submitted but the poster never accepted (deadlock) → after expiry
        // the escrow refunds to the poster, not trapped forever.
        uint256 id = _post();
        _claim(id);
        _submit(id);
        vm.warp(block.timestamp + TTL + 1);
        uint256 posterBefore = lh.balanceOf(poster);
        vm.prank(stranger);
        b.reclaimExpired(id);
        assertEq(lh.balanceOf(poster), posterBefore + REWARD, "refund from Submitted");
    }

    function test_reclaimExpired_reverts_before_expiry() public {
        uint256 id = _post();
        vm.prank(stranger);
        vm.expectRevert(BountyFacet.NotYetExpired.selector);
        b.reclaimExpired(id);
    }

    function test_reclaimExpired_reverts_at_exact_expiry() public {
        uint256 id = _post();
        (, , uint64 exp, , ) = b.getBounty(id);
        vm.warp(exp); // now == expiry: still claimable, NOT yet reclaimable
        vm.prank(stranger);
        vm.expectRevert(BountyFacet.NotYetExpired.selector);
        b.reclaimExpired(id);
    }

    function test_reclaimExpired_reverts_unknown() public {
        vm.warp(block.timestamp + TTL + 1);
        vm.prank(stranger);
        vm.expectRevert(BountyFacet.UnknownBounty.selector);
        b.reclaimExpired(999);
    }

    function test_reclaimExpired_reverts_on_paid_bounty() public {
        // A settled (Paid) bounty is not reclaimable even after expiry —
        // the escrow already left.
        uint256 id = _post();
        _claim(id);
        _submit(id);
        vm.prank(poster);
        b.acceptResult(id); // Paid
        vm.warp(block.timestamp + TTL + 1);
        vm.prank(stranger);
        vm.expectRevert(BountyFacet.NotReclaimable.selector);
        b.reclaimExpired(id);
    }

    function test_reclaimExpired_reverts_double_reclaim() public {
        uint256 id = _post();
        vm.warp(block.timestamp + TTL + 1);
        vm.prank(stranger);
        b.reclaimExpired(id);
        vm.prank(stranger);
        vm.expectRevert(BountyFacet.NotReclaimable.selector);
        b.reclaimExpired(id);
    }

    // --- the settled bounty can't ALSO be reclaimed (disjoint windows) --

    function test_paid_bounty_cannot_be_cancelled_or_reclaimed() public {
        uint256 id = _post();
        _claim(id);
        _submit(id);
        vm.prank(poster);
        b.acceptResult(id);
        // cancel: not Open
        vm.prank(poster);
        vm.expectRevert(BountyFacet.NotOpen.selector);
        b.cancelBounty(id);
        // reclaim: not reclaimable
        vm.warp(block.timestamp + TTL + 1);
        vm.prank(poster);
        vm.expectRevert(BountyFacet.NotReclaimable.selector);
        b.reclaimExpired(id);
    }

    // =====================================================================
    // REENTRANCY PROBES — a hostile token re-enters during settlement
    // =====================================================================

    function _reentrantHarness() internal returns (BountyHarness h, ReentrantLH rlh) {
        rlh = new ReentrantLH();
        h = new BountyHarness();
        h._setCreditsToken(address(rlh));
        h._registerIdentity(WORKER_ID, address(0xCAFE));
        h._setTba(WORKER_ID, workerTba);
        rlh.mint(poster, 1_000_000 ether);
        vm.prank(poster);
        rlh.approve(address(h), type(uint256).max);
        // Extra balance in the diamond so a SUCCESSFUL double-drain would
        // have something to steal (proving the revert is what saves it).
        rlh.mint(address(h), 1_000_000 ether);
        vm.warp(1_000_000);
    }

    function test_reentrant_accept_cannot_double_pay() public {
        (BountyHarness h, ReentrantLH rlh) = _reentrantHarness();
        vm.prank(poster);
        uint256 id = h.postBounty(TASK, REWARD, TTL);
        vm.prank(workerEoa);
        h.claimBounty(id, WORKER_ID);
        vm.prank(workerEoa);
        h.submitResult(id, RESULT);

        uint256 diamondBefore = rlh.balanceOf(address(h));
        rlh.arm(address(h), id, 0); // mode 0 = re-enter acceptResult

        vm.prank(poster);
        h.acceptResult(id);

        assertTrue(rlh.reenterReverted(), "re-entrant acceptResult reverted (NotSubmitted)");
        // Exactly ONE reward left the diamond, not two.
        assertEq(rlh.balanceOf(address(h)), diamondBefore - REWARD, "exactly one payout");
        assertEq(uint8(_status(h, id)), uint8(LibBountyStorage.Status.Paid));
    }

    function test_reentrant_cancel_cannot_double_refund() public {
        (BountyHarness h, ReentrantLH rlh) = _reentrantHarness();
        vm.prank(poster);
        uint256 id = h.postBounty(TASK, REWARD, TTL);

        uint256 diamondBefore = rlh.balanceOf(address(h));
        rlh.arm(address(h), id, 1); // mode 1 = re-enter cancelBounty

        vm.prank(poster);
        h.cancelBounty(id);

        assertTrue(rlh.reenterReverted(), "re-entrant cancelBounty reverted (NotOpen)");
        assertEq(rlh.balanceOf(address(h)), diamondBefore - REWARD, "exactly one refund");
        assertEq(uint8(_status(h, id)), uint8(LibBountyStorage.Status.Cancelled));
    }

    function test_reentrant_reclaim_cannot_double_refund() public {
        (BountyHarness h, ReentrantLH rlh) = _reentrantHarness();
        vm.prank(poster);
        uint256 id = h.postBounty(TASK, REWARD, TTL);
        vm.warp(block.timestamp + TTL + 1);

        uint256 diamondBefore = rlh.balanceOf(address(h));
        rlh.arm(address(h), id, 2); // mode 2 = re-enter reclaimExpired

        vm.prank(stranger);
        h.reclaimExpired(id);

        assertTrue(rlh.reenterReverted(), "re-entrant reclaimExpired reverted (NotReclaimable)");
        assertEq(rlh.balanceOf(address(h)), diamondBefore - REWARD, "exactly one refund");
        assertEq(uint8(_status(h, id)), uint8(LibBountyStorage.Status.Reclaimed));
    }

    function _status(BountyHarness h, uint256 id) internal view returns (uint8 st) {
        (, , , st, ) = h.getBounty(id);
    }

    // =====================================================================
    // openBounties pagination (Open + unexpired, index-window paging)
    // =====================================================================

    function test_openBounties_filters_and_pages() public {
        // Post 5. Their expiry = now + TTL.
        uint256[] memory ids = new uint256[](5);
        for (uint256 i = 0; i < 5; i++) {
            vm.prank(poster);
            ids[i] = b.postBounty(TASK, 1 ether, TTL);
        }
        b._registerIdentity(WORKER_ID, address(0xCAFE)); // ensure claimable

        // Take id[1] to Claimed (filtered out), cancel id[3] (filtered out).
        vm.prank(workerEoa);
        b.claimBounty(ids[1], WORKER_ID);
        vm.prank(poster);
        b.cancelBounty(ids[3]);

        // Full scan: only Open + unexpired remain → ids 0,2,4.
        (uint256[] memory all, uint256 cur) = b.openBounties(0, 100);
        assertEq(cur, 5, "cursor at end after a full scan");
        assertEq(all.length, 3, "claimed + cancelled filtered out");
        assertEq(all[0], ids[0]);
        assertEq(all[1], ids[2]);
        assertEq(all[2], ids[4]);

        // Page in windows of 2 over the index.
        (uint256[] memory p1, uint256 c1) = b.openBounties(0, 2); // window [0,2): ids 0 (Open), 1 (Claimed)
        assertEq(c1, 2);
        assertEq(p1.length, 1);
        assertEq(p1[0], ids[0]);

        (uint256[] memory p2, uint256 c2) = b.openBounties(c1, 2); // window [2,4): ids 2 (Open), 3 (Cancelled)
        assertEq(c2, 4);
        assertEq(p2.length, 1);
        assertEq(p2[0], ids[2]);

        (uint256[] memory p3, uint256 c3) = b.openBounties(c2, 2); // window [4,5): id 4 (Open)
        assertEq(c3, 5);
        assertEq(p3.length, 1);
        assertEq(p3[0], ids[4]);

        // Past the end: empty, cursor clamps to total.
        (uint256[] memory p4, uint256 c4) = b.openBounties(c3, 2);
        assertEq(p4.length, 0);
        assertEq(c4, 5);
    }

    function test_openBounties_excludes_expired() public {
        vm.prank(poster);
        b.postBounty(TASK, 1 ether, TTL);
        // Before expiry: present.
        (uint256[] memory before, ) = b.openBounties(0, 100);
        assertEq(before.length, 1);
        // After expiry: excluded (Open but expired).
        vm.warp(block.timestamp + TTL + 1);
        (uint256[] memory after_, ) = b.openBounties(0, 100);
        assertEq(after_.length, 0, "expired Open bounties drop out of the open list");
    }

    // =====================================================================
    // FUZZ: escrow conservation — sum(live escrows) == diamond $LH balance
    // =====================================================================

    /// The load-bearing invariant: at every point, the `$LH` the diamond
    /// holds for bounties equals the sum of `rewardWei` over all LIVE
    /// (Open/Claimed/Submitted) bounties. A payout (accept) or refund
    /// (cancel/reclaim) removes both the escrow and the live record in
    /// lockstep; nothing is ever stranded or double-counted.
    function testFuzz_escrow_conservation(uint256 seedRaw) public {
        uint256 seed = seedRaw;
        // A clean diamond holding ONLY bounty escrow (so its balance IS the
        // escrow total — no unrelated funds to confuse the invariant).
        assertEq(lh.balanceOf(address(b)), 0, "diamond starts empty");

        uint256 liveIdsLen = 0;
        uint256[] memory liveIds = new uint256[](40);

        for (uint256 i = 0; i < 40; i++) {
            seed = uint256(keccak256(abi.encode(seed, i)));
            uint256 action = seed % 4;

            if (action == 0) {
                // POST: bounded reward, respect the per-poster cap.
                if (b.activeBountyCountOf(poster) >= LibBountyStorage.MAX_ACTIVE_PER_POSTER) {
                    // skip — at cap
                } else {
                    uint128 reward = uint128(1 + (seed % 1000) * 1 ether);
                    vm.prank(poster);
                    uint256 id = b.postBounty(TASK, reward, TTL);
                    liveIds[liveIdsLen++] = id;
                }
            } else if (liveIdsLen > 0) {
                uint256 pick = seed % liveIdsLen;
                uint256 id = liveIds[pick];
                (, , , uint8 st, ) = b.getBounty(id);

                if (action == 1 && st == uint8(LibBountyStorage.Status.Open)) {
                    // CLAIM then SUBMIT (advance toward a payout).
                    vm.prank(workerEoa);
                    b.claimBounty(id, WORKER_ID);
                    vm.prank(workerEoa);
                    b.submitResult(id, RESULT);
                } else if (action == 2 && st == uint8(LibBountyStorage.Status.Submitted)) {
                    // ACCEPT → pays the worker TBA, leaves the live set.
                    vm.prank(poster);
                    b.acceptResult(id);
                    _removeLive(liveIds, liveIdsLen, pick);
                    liveIdsLen--;
                } else if (action == 3 && st == uint8(LibBountyStorage.Status.Open)) {
                    // CANCEL → refunds the poster, leaves the live set.
                    vm.prank(poster);
                    b.cancelBounty(id);
                    _removeLive(liveIds, liveIdsLen, pick);
                    liveIdsLen--;
                }
            }

            // INVARIANT after every step: diamond balance == sum of live
            // escrows (recomputed straight from on-chain state).
            assertEq(
                lh.balanceOf(address(b)),
                _sumLiveEscrow(),
                "diamond $LH == sum of live bounty rewards"
            );
        }
    }

    /// Sum `rewardWei` over every non-terminal bounty (Open/Claimed/
    /// Submitted) — these are the ones whose escrow is still in the diamond.
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

    function _removeLive(uint256[] memory arr, uint256 len, uint256 idx) internal pure {
        arr[idx] = arr[len - 1];
    }
}
