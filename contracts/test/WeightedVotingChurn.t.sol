// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {WeightedVotingFacet} from "../src/facets/WeightedVotingFacet.sol";
import {LibGuildStorage} from "../src/libraries/LibGuildStorage.sol";
import {LibCreditsStorage} from "../src/libraries/LibCreditsStorage.sol";
import {LibRegistryStorage} from "../src/libraries/LibRegistryStorage.sol";

/// $LH-shaped TIP-20 mock (same surface the facet escrows/spends through).
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

contract WeightedHarness is WeightedVotingFacet {
    function _setCreditsToken(address token) external {
        LibCreditsStorage.load().creditsToken = token;
    }

    function tokenBoundAccount(uint256 tokenId) external view returns (address) {
        if (LibRegistryStorage.load().ownerOfId[tokenId] == address(0)) return address(0);
        return address(uint160(uint256(keccak256(abi.encodePacked("tba", tokenId)))));
    }
}

/// Adversarial coverage for the SHARE-WEIGHTED board (the facet shipped with zero
/// tests). The exploit: the quorum DENOMINATOR is snapshotted at propose, but each
/// ballot's WEIGHT is read live — so without a guard an Admin could re-weight a
/// friendly voter AFTER propose and blow past the frozen quorum. The fix freezes
/// the cap table (`setShares` reverts `SharesLockedDuringVote`) for the voting
/// window. These tests pin that lock.
contract WeightedVotingChurnTest is Test {
    WeightedHarness g;
    MockLH lh;

    address founder = address(0xF00D);
    address alice = address(0xA11CE);
    address payee = address(0x7BA);

    uint256 constant FUND = 1_000 ether;
    uint256 constant PERIOD = 1 days;

    function setUp() public {
        g = new WeightedHarness();
        lh = new MockLH();
        g._setCreditsToken(address(lh));
        lh.mint(founder, 1_000_000 ether);
        vm.prank(founder);
        lh.approve(address(g), type(uint256).max);
        vm.warp(1_000_000);
    }

    function _guildWithCapTable() internal returns (uint256 id) {
        vm.startPrank(founder);
        id = g.createGuild("captabledao"); // founder is Admin
        g.setRole(id, alice, uint8(LibGuildStorage.Role.Member));
        g.setShares(id, founder, 10);
        g.setShares(id, alice, 10); // total 20; quorum needs 2*cast > 20
        g.fundGuild(id, FUND);
        vm.stopPrank();
    }

    /// THE FIX: re-weighting a voter while a weighted proposal is open reverts.
    function test_setShares_reverts_during_open_weighted_vote() public {
        uint256 id = _guildWithCapTable();
        vm.prank(founder);
        uint256 pid = g.proposeWeighted(id, payee, 500 ether, PERIOD, "drain");
        assertTrue(pid != 0);

        // Admin tries to inflate a friendly voter past the snapshot quorum.
        vm.prank(founder);
        vm.expectRevert(WeightedVotingFacet.SharesLockedDuringVote.selector);
        g.setShares(id, alice, 1000);
    }

    /// The lock is scoped to the voting window — it auto-releases at the deadline.
    function test_setShares_allowed_after_deadline() public {
        uint256 id = _guildWithCapTable();
        vm.prank(founder);
        g.proposeWeighted(id, payee, 500 ether, PERIOD, "drain");

        vm.warp(block.timestamp + PERIOD + 1); // voting closed
        vm.prank(founder);
        g.setShares(id, alice, 1000); // no revert — the cap table is editable again
    }

    /// End-to-end: a minority CANNOT drain the treasury by re-weighting mid-vote.
    /// founder(10) + alice(10), total 20 → quorum needs 2*cast > 20 (i.e. > 10
    /// shares FOR). founder alone (10) can't meet it; the only way to pass solo is
    /// to inflate, which the lock blocks → after the deadline the measure fails
    /// quorum and the treasury is untouched.
    function test_midvote_reweight_cannot_pass_minority_drain() public {
        uint256 id = _guildWithCapTable();
        vm.prank(founder);
        uint256 pid = g.proposeWeighted(id, payee, 500 ether, PERIOD, "drain");

        vm.prank(founder);
        g.voteWeighted(pid, true); // 10 FOR — exactly half, NOT > half

        // The inflate-to-pass-solo move is blocked.
        vm.prank(founder);
        vm.expectRevert(WeightedVotingFacet.SharesLockedDuringVote.selector);
        g.setShares(id, founder, 1000);

        vm.warp(block.timestamp + PERIOD + 1);
        g.executeWeighted(pid);

        assertEq(g.treasuryBalanceOf(id), FUND, "treasury NOT drained by mid-vote re-weight");
        assertEq(lh.balanceOf(payee), 0, "payee unpaid");
    }
}
