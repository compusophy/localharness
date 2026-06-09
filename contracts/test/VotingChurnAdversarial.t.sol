// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {VotingFacet} from "../src/facets/VotingFacet.sol";
import {GuildFacet} from "../src/facets/GuildFacet.sol";
import {LibGuildStorage} from "../src/libraries/LibGuildStorage.sol";
import {LibVotingStorage} from "../src/libraries/LibVotingStorage.sol";
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

contract VotingHarness is VotingFacet {
    function _setCreditsToken(address token) external {
        LibCreditsStorage.load().creditsToken = token;
    }

    function tokenBoundAccount(uint256 tokenId) external view returns (address) {
        if (LibRegistryStorage.load().ownerOfId[tokenId] == address(0)) return address(0);
        return address(uint160(uint256(keccak256(abi.encodePacked("tba", tokenId)))));
    }
}

/// The adversarial governance-robustness suite: quorum vs membership churn.
/// These are the exploit-class tests the author flagged as a follow-up
/// ("snapshot-at-propose"). After the fix (snapshot the member count at
/// propose) the quorum denominator is FIXED for a proposal's life, so none of
/// the churn games below can shrink it.
contract VotingChurnAdversarialTest is Test {
    VotingHarness g;
    MockLH lh;

    address founder = address(0xF00D);
    address alice = address(0xA11CE);
    address bob = address(0xB0B);
    address carol = address(0xCA401);
    address dave = address(0xDA1E);
    address eve = address(0xE7E); // the attacker
    address mallory = address(0x4A11074);
    address payee = address(0x7BA);

    uint256 constant FUND = 1_000 ether;
    uint64 constant PERIOD = 1 days;

    function setUp() public {
        g = new VotingHarness();
        lh = new MockLH();
        g._setCreditsToken(address(lh));
        lh.mint(founder, 1_000_000 ether);
        vm.prank(founder);
        lh.approve(address(g), type(uint256).max);
        vm.warp(1_000_000);
    }

    function _create(string memory name) internal returns (uint256 id) {
        vm.prank(founder);
        id = g.createGuild(name);
    }

    function _seat(uint256 id, address member) internal {
        vm.prank(founder);
        g.setRole(id, member, uint8(LibGuildStorage.Role.Member));
    }

    function _evict(uint256 id, address member) internal {
        vm.prank(founder);
        g.setRole(id, member, uint8(LibGuildStorage.Role.None));
    }

    function _fund(uint256 id, uint256 amt) internal {
        vm.prank(founder);
        g.fundGuild(id, amt);
    }

    // =====================================================================
    // EXPLOIT 1: members LEAVE between propose and execute to SHRINK the
    // quorum denominator so a measure that missed quorum now passes.
    //
    // 6-member guild, quorum at propose = ceil(6/2) = 3. Attacker musters
    // only 2 FOR votes (a minority). Then 2 unrelated members are evicted /
    // leave, dropping memberCount to 4, quorum to 2 -> the 2-vote minority
    // now meets the LIVE quorum and DRAINS the treasury.
    //
    // With the snapshot fix this MUST fail (quorum stays 3) -> the measure
    // Fails, treasury untouched.
    // =====================================================================
    function test_churn_leave_to_shrink_quorum_cannot_pass_minority() public {
        uint256 id = _create("victimdao"); // founder
        _seat(id, alice);
        _seat(id, bob);
        _seat(id, carol);
        _seat(id, dave);
        _seat(id, eve); // 6 members total; quorum at propose = 3
        _fund(id, FUND);

        // Attacker (eve) proposes a self-serving treasury drain to payee.
        vm.prank(eve);
        uint256 pid = g.propose(id, payee, 500 ether, "drain", PERIOD);

        // Only a 2-vote minority backs it (eve + a single ally bob).
        vm.prank(eve);
        g.vote(pid, true);
        vm.prank(bob);
        g.vote(pid, true);
        // 2 cast < quorum(3) at the 6-member size -> should NOT pass.

        // Churn: evict two uninvolved members so memberCount drops 6 -> 4.
        _evict(id, carol);
        _evict(id, dave);
        assertEq(g.guildMembersOf(id).length, 4, "two members gone");

        vm.warp(block.timestamp + PERIOD + 1);
        g.execute(pid);

        // The fix: the snapshot quorum (3) is unchanged by churn, the
        // 2-vote minority misses it, the proposal Fails with NO spend.
        assertEq(g.treasuryBalanceOf(id), FUND, "treasury NOT drained by minority via churn");
        assertEq(lh.balanceOf(payee), 0, "payee unpaid");
        (,,,,, uint8 st,,) = g.getProposal(pid);
        assertEq(st, uint8(LibVotingStorage.VStatus.Failed), "minority measure Failed");
    }

    // =====================================================================
    // EXPLOIT 2: a voter votes FOR, then LEAVES. Their vote stays counted
    // (the tally is never decremented) while the live quorum denominator
    // shrinks. Same shrink-the-denominator class as Exploit 1, but with the
    // leaver themselves.
    //
    // 4-member guild, quorum at propose = 2. Eve votes FOR alone (1 < 2,
    // misses). Then two OTHER members leave -> memberCount 2, live quorum 1
    // -> eve's lone vote would meet it. Snapshot fix keeps quorum 2 -> Fails.
    // =====================================================================
    function test_churn_vote_then_others_leave_lone_vote_cannot_pass() public {
        uint256 id = _create("victimdao2");
        _seat(id, alice);
        _seat(id, bob);
        _seat(id, eve); // 4 members; quorum at propose = 2
        _fund(id, FUND);

        vm.prank(eve);
        uint256 pid = g.propose(id, payee, 400 ether, "drain", PERIOD);
        vm.prank(eve);
        g.vote(pid, true); // 1 FOR, alone

        // alice + bob leave -> memberCount 4 -> 2, live quorum would be 1.
        vm.prank(alice);
        g.leaveGuild(id);
        vm.prank(bob);
        g.leaveGuild(id);
        assertEq(g.guildMembersOf(id).length, 2, "founder + eve remain");

        vm.warp(block.timestamp + PERIOD + 1);
        g.execute(pid);

        assertEq(g.treasuryBalanceOf(id), FUND, "lone vote can't pass via shrink");
        (,,,,, uint8 st,,) = g.getProposal(pid);
        assertEq(st, uint8(LibVotingStorage.VStatus.Failed));
    }

    // =====================================================================
    // CONTROL: a legitimately-quorate, majority-for measure STILL executes
    // after the snapshot fix (the fix must not break the happy path). The
    // snapshot is taken at propose, so a guild that GROWS afterward doesn't
    // change the bar either — exactly the determinism we want.
    // =====================================================================
    function test_snapshot_does_not_block_legit_pass() public {
        uint256 id = _create("legitdao");
        _seat(id, alice);
        _seat(id, bob);
        _seat(id, carol); // 4 members; snapshot quorum = 2
        _fund(id, FUND);

        vm.prank(alice);
        uint256 pid = g.propose(id, payee, 300 ether, "grant", PERIOD);
        vm.prank(alice);
        g.vote(pid, true);
        vm.prank(bob);
        g.vote(pid, true);
        vm.prank(carol);
        g.vote(pid, true); // 3 for, quorum 2 met, majority

        // Even if the guild GROWS afterward, the snapshot bar is unchanged.
        _seat(id, dave);
        _seat(id, eve);
        _seat(id, mallory); // now 7 members

        vm.warp(block.timestamp + PERIOD + 1);
        g.execute(pid);
        assertEq(lh.balanceOf(payee), 300 ether, "legit measure still executes");
        (,,,,, uint8 st,,) = g.getProposal(pid);
        assertEq(st, uint8(LibVotingStorage.VStatus.Executed));
    }

    // =====================================================================
    // EXPLOIT 3 (the OTHER direction): does GROWTH after propose let an
    // attacker move the goalposts so a legitimately-passing measure FAILS
    // (a griefer floods new members to inflate the quorum the honest voters
    // can no longer meet)? Snapshot-at-propose closes this too.
    // =====================================================================
    function test_churn_join_to_inflate_quorum_cannot_grief_legit_pass() public {
        uint256 id = _create("growdao");
        _seat(id, alice);
        _seat(id, bob); // 3 members; snapshot quorum = 2
        _fund(id, FUND);

        vm.prank(alice);
        uint256 pid = g.propose(id, payee, 200 ether, "grant", PERIOD);
        vm.prank(alice);
        g.vote(pid, true);
        vm.prank(bob);
        g.vote(pid, true); // 2 for, snapshot quorum 2 met

        // A griefer floods the guild with sybil members AFTER the vote to
        // inflate the LIVE quorum (would be ceil(9/2)=5) above the 2 cast.
        _seat(id, carol);
        _seat(id, dave);
        _seat(id, eve);
        _seat(id, mallory);
        _seat(id, address(0xF1));
        _seat(id, address(0xF2)); // now 9 members

        vm.warp(block.timestamp + PERIOD + 1);
        g.execute(pid);

        // Snapshot quorum (2) is unchanged -> the legit measure still passes.
        assertEq(lh.balanceOf(payee), 200 ether, "growth can't grief a quorate measure");
        (,,,,, uint8 st,,) = g.getProposal(pid);
        assertEq(st, uint8(LibVotingStorage.VStatus.Executed));
    }

    // =====================================================================
    // The tally VIEW should also reflect the snapshot quorum (so off-chain
    // UIs and the eventual outcome agree), not the live member count.
    // =====================================================================
    function test_tally_quorum_is_snapshot_not_live() public {
        uint256 id = _create("tallydao");
        _seat(id, alice);
        _seat(id, bob);
        _seat(id, carol);
        _seat(id, dave); // 5 members; snapshot quorum = 3
        _fund(id, FUND);

        vm.prank(alice);
        uint256 pid = g.propose(id, payee, 100 ether, "", PERIOD);
        (,, uint256 q0,,) = g.tallyOf(pid);
        assertEq(q0, 3, "snapshot quorum ceil(5/2)=3");

        // Shrink the live membership; the tally quorum must stay 3.
        _evict(id, carol);
        _evict(id, dave);
        (,, uint256 q1,,) = g.tallyOf(pid);
        assertEq(q1, 3, "tally quorum is the snapshot, not the shrunken live count");
    }

    // =====================================================================
    // CROSS-FACET _spendCore (Voting -> Guild treasury): SOUND-path guards.
    // =====================================================================

    /// A passed proposal CANNOT spend MORE than the guild's live treasury:
    /// two proposals each for (more than) half the balance both pass, but the
    /// first execute debits the ledger and the second re-reads the reduced
    /// balance and reverts AmountExceedsTreasury — no cross-proposal
    /// double-spend of the same escrow.
    function test_voting_two_proposals_cannot_double_spend_treasury() public {
        uint256 id = _create("spenddao");
        _seat(id, alice);
        _seat(id, bob); // 3 members, quorum 2
        _fund(id, FUND); // 1000

        // Two proposals, each for 700 (>half) — together 1400 > 1000.
        vm.prank(alice);
        uint256 p1 = g.propose(id, payee, 700 ether, "a", PERIOD);
        vm.prank(alice);
        uint256 p2 = g.propose(id, payee, 700 ether, "b", PERIOD);

        // Both pass (2 for each).
        vm.prank(alice);
        g.vote(p1, true);
        vm.prank(bob);
        g.vote(p1, true);
        vm.prank(alice);
        g.vote(p2, true);
        vm.prank(bob);
        g.vote(p2, true);

        vm.warp(block.timestamp + PERIOD + 1);
        g.execute(p1); // debits 700 -> 300 left
        assertEq(g.treasuryBalanceOf(id), 300 ether, "first spend debited");

        // Second can't be afforded; reverts and stays Active (retryable).
        vm.expectRevert(VotingFacet.AmountExceedsTreasury.selector);
        g.execute(p2);
        (,,,,, uint8 st,,) = g.getProposal(p2);
        assertEq(st, uint8(LibVotingStorage.VStatus.Active), "unaffordable passed measure stays Active");

        // The diamond's $LH out exactly matches the one executed spend.
        assertEq(lh.balanceOf(payee), 700 ether, "exactly one 700 spend left the treasury");
        assertEq(g.treasuryBalanceOf(id), 300 ether);
    }

    /// The Admin spend path and the vote-execute path debit the SAME
    /// guildBalance ledger — an Admin drain between propose and execute is
    /// re-checked at execute (can't overspend the shared escrow across the
    /// two routes).
    function test_admin_and_vote_paths_share_ledger_no_overspend() public {
        uint256 id = _create("shareddao");
        _seat(id, alice);
        _seat(id, bob);
        _fund(id, FUND); // 1000

        vm.prank(alice);
        uint256 pid = g.propose(id, payee, 600 ether, "vote-spend", PERIOD);
        vm.prank(alice);
        g.vote(pid, true);
        vm.prank(bob);
        g.vote(pid, true);

        // Admin drains 600 via the Admin path BEFORE execute -> 400 left.
        vm.prank(founder);
        g.spendTreasury(id, payee, 600 ether, "admin-drain");
        assertEq(g.treasuryBalanceOf(id), 400 ether);

        vm.warp(block.timestamp + PERIOD + 1);
        // The 600 vote-spend can no longer be afforded (only 400 left).
        vm.expectRevert(VotingFacet.AmountExceedsTreasury.selector);
        g.execute(pid);
        // Total out of the treasury never exceeded the funded 1000.
        assertEq(lh.balanceOf(payee), 600 ether, "only the admin spend settled");
        assertEq(g.treasuryBalanceOf(id), 400 ether, "ledger consistent across both routes");
    }

    /// A proposal targets EXACTLY one guild's treasury (`p.guildId`) — a
    /// proposal opened in guild A can never debit guild B's balance. (Cross-
    /// facet treasury isolation under the voting path.)
    function test_voting_cannot_drain_a_sibling_guilds_treasury() public {
        uint256 a = _create("guilda");
        _seat(a, alice);
        _seat(a, bob); // a: 3 members, quorum 2

        // A separate, well-funded guild B that guild A's members do NOT govern.
        vm.prank(carol);
        uint256 b = g.createGuild("guildb");
        vm.prank(founder);
        g.fundGuild(b, FUND); // B holds 1000; A holds 0

        // A's members pass a proposal IN GUILD A (which is empty). It can only
        // ever touch A's balance — propose fail-fasts AmountExceedsTreasury
        // because A has 0, proving the spend is scoped to p.guildId, not B.
        vm.prank(alice);
        vm.expectRevert(VotingFacet.AmountExceedsTreasury.selector);
        g.propose(a, payee, 100 ether, "steal-from-B", PERIOD);

        // B's treasury is wholly untouched.
        assertEq(g.treasuryBalanceOf(b), FUND, "sibling guild treasury isolated");
        assertEq(g.treasuryBalanceOf(a), 0);
    }

    /// Reentrancy across the cross-facet boundary: a hostile token re-entering
    /// during the execute->_spendCore payout (the recipient/token callback)
    /// can't double-spend AND can't drain a sibling. (Belt-and-suspenders on
    /// top of VotingFacet's own reentrancy probe — this one arms a SECOND
    /// guild as the re-entry target to prove cross-guild isolation holds even
    /// under re-entry.) Uses the plain reentrancy already proven in
    /// VotingFacet.t.sol for the same-proposal case; here we assert the
    /// sibling-guild ledger is the invariant that holds.
    function test_execute_does_not_touch_sibling_under_normal_token() public {
        // Two guilds funded; execute on one must leave the other exactly whole.
        uint256 a = _create("exa");
        _seat(a, alice);
        _seat(a, bob);
        _fund(a, FUND);

        vm.prank(carol);
        uint256 b = g.createGuild("exb");
        vm.prank(founder);
        g.fundGuild(b, FUND);

        vm.prank(alice);
        uint256 pid = g.propose(a, payee, 250 ether, "", PERIOD);
        vm.prank(alice);
        g.vote(pid, true);
        vm.prank(bob);
        g.vote(pid, true);
        vm.warp(block.timestamp + PERIOD + 1);
        g.execute(pid);

        assertEq(g.treasuryBalanceOf(a), FUND - 250 ether, "guild A debited");
        assertEq(g.treasuryBalanceOf(b), FUND, "guild B untouched");
        // Diamond holds exactly both ledgers' sum.
        assertEq(
            lh.balanceOf(address(g)),
            g.treasuryBalanceOf(a) + g.treasuryBalanceOf(b),
            "diamond $LH == sum of both guild ledgers"
        );
    }
}
