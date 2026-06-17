// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {VotingFacet} from "../src/facets/VotingFacet.sol";
import {GuildFacet} from "../src/facets/GuildFacet.sol";
import {LibGuildStorage} from "../src/libraries/LibGuildStorage.sol";
import {LibVotingStorage} from "../src/libraries/LibVotingStorage.sol";
import {LibCreditsStorage} from "../src/libraries/LibCreditsStorage.sol";
import {LibRegistryStorage} from "../src/libraries/LibRegistryStorage.sol";

/// Minimal `$LH`-shaped TIP-20 mock: 18-decimal balances + the
/// approve/transferFrom/transfer surface the facet escrows + spends
/// through. Reverts on an under-allowance / under-balance pull so the CEI
/// ordering is provable.
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

/// Hostile reentrant TIP-20 mock: on `transfer` (the execute->_spend payout -
/// the only external call in the execute path) it re-enters the diamond,
/// trying to `execute` the SAME proposal again. Real `$LH` has NO callback;
/// this is the defense-in-depth probe that the double CEI barrier (status ->
/// Executed before the spend, ledger debit before the transfer) makes a
/// double-spend structurally impossible - the re-entry sees a non-Active
/// proposal and reverts.
contract ReentrantLH {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    address public diamond;
    uint256 public attackProposal;
    bool internal entered;
    bool public reenterReverted;

    function arm(address d, uint256 proposalId) external {
        diamond = d;
        attackProposal = proposalId;
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
        // Re-enter ONCE during the settlement transfer: try to execute the
        // same proposal again. CEI means it's already Executed, so this
        // MUST revert (no double drain).
        if (diamond != address(0) && !entered) {
            entered = true;
            try VotingFacet(diamond).execute(attackProposal) {
                reenterReverted = false;
            } catch {
                reenterReverted = true;
            }
        }
        return true;
    }
}

/// Test harness: VotingFacet (which inherits GuildFacet) + setters that
/// write the SHARED diamond-storage slots a real diamond populates via other
/// facets (creditsToken from CreditsFacet) + a real `tokenBoundAccount` so
/// the guildAddress self-call resolves. Because every `Lib*Storage.load()`
/// resolves against THIS contract's storage, writing them here IS the
/// cross-facet storage sharing the diamond provides - and crucially the
/// inherited `_spend` operates on the SAME LibGuildStorage slot
/// `spendTreasury` does (single accounting source). The diamond IS the
/// escrow holder, so `address(this)` holds the treasury `$LH`.
contract VotingHarness is VotingFacet {
    function _setCreditsToken(address token) external {
        LibCreditsStorage.load().creditsToken = token;
    }

    function tokenBoundAccount(uint256 tokenId) external view returns (address) {
        if (LibRegistryStorage.load().ownerOfId[tokenId] == address(0)) return address(0);
        return address(uint160(uint256(keccak256(abi.encodePacked("tba", tokenId)))));
    }
}

contract VotingFacetTest is Test {
    VotingHarness g;
    MockLH lh;

    address founder = address(0xF00D); // creates the guild; first Admin + member
    address alice = address(0xA11CE); // a member / voter
    address bob = address(0xB0B); // a member / voter
    address carol = address(0xCA401); // a member / voter
    address dave = address(0xDA1E); // a member / voter
    address stranger = address(0xBEEF); // non-member
    address payee = address(0x7BA); // a treasury spend recipient

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

    // --- helpers --------------------------------------------------------

    function _create(string memory name) internal returns (uint256 id) {
        vm.prank(founder);
        id = g.createGuild(name);
    }

    /// founder (Admin) direct-seats `member` as a Member (skips the
    /// invite/accept dance - same end state, fewer pranks).
    function _seat(uint256 id, address member) internal {
        vm.prank(founder);
        g.setRole(id, member, uint8(LibGuildStorage.Role.Member));
    }

    function _fund(uint256 id, uint256 amt) internal {
        vm.prank(founder);
        g.fundGuild(id, amt);
    }

    /// A guild with `founder` + alice + bob + carol = 4 members, funded.
    function _guild4() internal returns (uint256 id) {
        id = _create("dao");
        _seat(id, alice);
        _seat(id, bob);
        _seat(id, carol);
        _fund(id, FUND);
    }

    function _propose(uint256 id, address proposer, uint256 amount) internal returns (uint256 pid) {
        vm.prank(proposer);
        pid = g.propose(id, payee, amount, "grant", PERIOD);
    }

    // =====================================================================
    // FULL LIFECYCLE - propose -> vote -> pass -> execute pays + debits
    // =====================================================================

    function test_lifecycle_pass_pays_payee_and_debits_treasury() public {
        uint256 id = _guild4(); // 4 members, quorum = 2
        uint256 pid = _propose(id, alice, 300 ether);

        // 4 for, 0 against (quorum 2 met, strict majority, founder=Admin FOR
        // satisfies the admin-consent gate).
        vm.prank(founder);
        g.vote(pid, true);
        vm.prank(alice);
        g.vote(pid, true);
        vm.prank(bob);
        g.vote(pid, true);
        vm.prank(carol);
        g.vote(pid, true);

        (,,,,, uint8 st, uint256 fv, uint256 av) = g.getProposal(pid);
        assertEq(st, uint8(LibVotingStorage.VStatus.Active), "still active before deadline");
        assertEq(fv, 4);
        assertEq(av, 0);

        // Can't execute before the deadline.
        vm.expectRevert(VotingFacet.VotingNotEnded.selector);
        g.execute(pid);

        vm.warp(block.timestamp + PERIOD + 1);

        uint256 payeeBefore = lh.balanceOf(payee);
        uint256 treasBefore = g.treasuryBalanceOf(id);
        g.execute(pid); // permissionless

        assertEq(lh.balanceOf(payee), payeeBefore + 300 ether, "payee paid from treasury");
        assertEq(g.treasuryBalanceOf(id), treasBefore - 300 ether, "treasury debited via _spend");
        assertEq(lh.balanceOf(address(g)), FUND - 300 ether, "diamond drained by exactly the spend");

        (,,,,, uint8 st2,,) = g.getProposal(pid);
        assertEq(st2, uint8(LibVotingStorage.VStatus.Executed), "terminal Executed");
    }

    function test_execute_is_idempotent() public {
        uint256 id = _guild4();
        uint256 pid = _propose(id, alice, 100 ether);
        vm.prank(alice);
        g.vote(pid, true);
        vm.prank(bob);
        g.vote(pid, true);
        vm.warp(block.timestamp + PERIOD + 1);
        g.execute(pid);
        // Second execute must revert (terminal).
        vm.expectRevert(VotingFacet.ProposalNotActive.selector);
        g.execute(pid);
    }

    // =====================================================================
    // QUORUM not met -> no spend (fails)
    // =====================================================================

    function test_quorum_not_met_fails_no_spend() public {
        uint256 id = _guild4(); // 4 members, quorum = 2
        uint256 pid = _propose(id, alice, 300 ether);
        // Only ONE member votes for -> 1 < quorum(2) -> fails.
        vm.prank(alice);
        g.vote(pid, true);

        vm.warp(block.timestamp + PERIOD + 1);
        uint256 treasBefore = g.treasuryBalanceOf(id);
        g.execute(pid);

        assertEq(g.treasuryBalanceOf(id), treasBefore, "no spend - quorum missed");
        assertEq(lh.balanceOf(payee), 0, "payee unpaid");
        (,,,,, uint8 st,,) = g.getProposal(pid);
        assertEq(st, uint8(LibVotingStorage.VStatus.Failed), "Failed terminal");
    }

    // =====================================================================
    // MAJORITY against -> no spend (fails); a TIE also fails
    // =====================================================================

    function test_majority_against_fails_no_spend() public {
        uint256 id = _guild4(); // 4 members, quorum 2
        uint256 pid = _propose(id, alice, 300 ether);
        // 1 for, 2 against -> quorum met (3 cast) but majority against.
        vm.prank(alice);
        g.vote(pid, true);
        vm.prank(bob);
        g.vote(pid, false);
        vm.prank(carol);
        g.vote(pid, false);

        vm.warp(block.timestamp + PERIOD + 1);
        g.execute(pid);

        assertEq(g.treasuryBalanceOf(id), FUND, "no spend - majority against");
        (,,,,, uint8 st,,) = g.getProposal(pid);
        assertEq(st, uint8(LibVotingStorage.VStatus.Failed));
    }

    function test_tie_fails_no_spend() public {
        // Add a 4th non-founder member so we can cast an even split that
        // still meets quorum: founder+alice+bob+carol+dave = 5 members,
        // quorum = 3. Cast 2 for / 2 against = 4 cast (quorum met), tie.
        uint256 id = _guild4();
        _seat(id, dave); // now 5 members
        uint256 pid = _propose(id, alice, 100 ether);
        vm.prank(alice);
        g.vote(pid, true);
        vm.prank(bob);
        g.vote(pid, true);
        vm.prank(carol);
        g.vote(pid, false);
        vm.prank(dave);
        g.vote(pid, false);
        vm.warp(block.timestamp + PERIOD + 1);
        g.execute(pid);
        assertEq(g.treasuryBalanceOf(id), FUND, "tie fails - strict majority required");
        (,,,,, uint8 st,,) = g.getProposal(pid);
        assertEq(st, uint8(LibVotingStorage.VStatus.Failed));
    }

    // =====================================================================
    // 1-MEMBER guild - divide-by-zero guard (quorum = 1)
    // =====================================================================

    function test_one_member_guild_quorum_is_one() public {
        uint256 id = _create("solo"); // founder is the sole member, quorum 1
        _fund(id, FUND);
        uint256 pid = _propose(id, founder, 200 ether);

        (,, uint256 q,, bool passingBefore) = g.tallyOf(pid);
        assertEq(q, 1, "1-member quorum is 1 (divide-by-zero guard)");
        assertFalse(passingBefore, "no votes yet -> not passing");

        vm.prank(founder);
        g.vote(pid, true);
        (,,, uint256 cast2, bool passing2) = g.tallyOf(pid);
        assertEq(cast2, 1);
        assertTrue(passing2, "sole member voting for -> passes");

        vm.warp(block.timestamp + PERIOD + 1);
        g.execute(pid);
        assertEq(lh.balanceOf(payee), 200 ether, "solo DAO executed");
        assertEq(g.treasuryBalanceOf(id), FUND - 200 ether);
    }

    function test_one_member_guild_no_vote_fails() public {
        uint256 id = _create("solo2");
        _fund(id, FUND);
        uint256 pid = _propose(id, founder, 200 ether);
        // No one votes -> 0 cast < quorum(1) -> fails, no spend.
        vm.warp(block.timestamp + PERIOD + 1);
        g.execute(pid);
        assertEq(g.treasuryBalanceOf(id), FUND, "no vote -> no spend");
        (,,,,, uint8 st,,) = g.getProposal(pid);
        assertEq(st, uint8(LibVotingStorage.VStatus.Failed));
    }

    // =====================================================================
    // PROPOSE reverts
    // =====================================================================

    function test_propose_reverts_non_member() public {
        uint256 id = _guild4();
        vm.prank(stranger);
        vm.expectRevert(VotingFacet.NotGuildMember.selector);
        g.propose(id, payee, 100 ether, "", PERIOD);
    }

    function test_propose_reverts_unknown_guild() public {
        vm.prank(founder);
        vm.expectRevert(GuildFacet.UnknownGuild.selector);
        g.propose(999, payee, 100 ether, "", PERIOD);
    }

    function test_propose_reverts_zero_amount() public {
        uint256 id = _guild4();
        vm.prank(alice);
        vm.expectRevert(VotingFacet.ZeroProposalAmount.selector);
        g.propose(id, payee, 0, "", PERIOD);
    }

    function test_propose_reverts_zero_recipient() public {
        uint256 id = _guild4();
        vm.prank(alice);
        vm.expectRevert(VotingFacet.ZeroProposalRecipient.selector);
        g.propose(id, address(0), 100 ether, "", PERIOD);
    }

    function test_propose_reverts_amount_exceeds_treasury() public {
        uint256 id = _guild4(); // funded FUND
        vm.prank(alice);
        vm.expectRevert(VotingFacet.AmountExceedsTreasury.selector);
        g.propose(id, payee, FUND + 1, "", PERIOD);
    }

    function test_propose_reverts_bad_period_too_short() public {
        uint256 id = _guild4();
        vm.prank(alice);
        vm.expectRevert(VotingFacet.BadVotingPeriod.selector);
        g.propose(id, payee, 100 ether, "", uint64(LibVotingStorage.MIN_VOTING_PERIOD) - 1);
    }

    function test_propose_reverts_bad_period_too_long() public {
        uint256 id = _guild4();
        vm.prank(alice);
        vm.expectRevert(VotingFacet.BadVotingPeriod.selector);
        g.propose(id, payee, 100 ether, "", uint64(LibVotingStorage.MAX_VOTING_PERIOD) + 1);
    }

    function test_propose_reverts_memo_too_large() public {
        uint256 id = _guild4();
        bytes memory big = new bytes(LibVotingStorage.MAX_MEMO_BYTES + 1);
        vm.prank(alice);
        vm.expectRevert(VotingFacet.MemoTooLarge.selector);
        g.propose(id, payee, 100 ether, big, PERIOD);
    }

    // =====================================================================
    // VOTE reverts
    // =====================================================================

    function test_vote_reverts_non_member() public {
        uint256 id = _guild4();
        uint256 pid = _propose(id, alice, 100 ether);
        vm.prank(stranger);
        vm.expectRevert(VotingFacet.NotGuildMember.selector);
        g.vote(pid, true);
    }

    function test_vote_reverts_double_vote() public {
        uint256 id = _guild4();
        uint256 pid = _propose(id, alice, 100 ether);
        vm.prank(alice);
        g.vote(pid, true);
        vm.prank(alice);
        vm.expectRevert(VotingFacet.AlreadyVoted.selector);
        g.vote(pid, true);
    }

    function test_vote_reverts_double_vote_even_flipping() public {
        // Voting again with the OPPOSITE support is still a double-vote.
        uint256 id = _guild4();
        uint256 pid = _propose(id, alice, 100 ether);
        vm.prank(alice);
        g.vote(pid, true);
        vm.prank(alice);
        vm.expectRevert(VotingFacet.AlreadyVoted.selector);
        g.vote(pid, false);
    }

    function test_vote_reverts_after_deadline() public {
        uint256 id = _guild4();
        uint256 pid = _propose(id, alice, 100 ether);
        vm.warp(block.timestamp + PERIOD + 1);
        vm.prank(alice);
        vm.expectRevert(VotingFacet.VotingClosed.selector);
        g.vote(pid, true);
    }

    function test_vote_reverts_unknown_proposal() public {
        vm.prank(alice);
        vm.expectRevert(VotingFacet.UnknownProposal.selector);
        g.vote(999, true);
    }

    function test_vote_reverts_on_terminal_proposal() public {
        uint256 id = _guild4();
        uint256 pid = _propose(id, alice, 100 ether);
        vm.prank(alice);
        g.vote(pid, true);
        vm.prank(bob);
        g.vote(pid, true);
        vm.warp(block.timestamp + PERIOD + 1);
        g.execute(pid); // now Executed
        vm.prank(carol);
        vm.expectRevert(VotingFacet.ProposalNotActive.selector);
        g.vote(pid, true);
    }

    // =====================================================================
    // EXECUTE reverts
    // =====================================================================

    function test_execute_reverts_before_deadline() public {
        uint256 id = _guild4();
        uint256 pid = _propose(id, alice, 100 ether);
        vm.prank(alice);
        g.vote(pid, true);
        vm.prank(bob);
        g.vote(pid, true);
        vm.expectRevert(VotingFacet.VotingNotEnded.selector);
        g.execute(pid);
    }

    function test_execute_reverts_unknown_proposal() public {
        vm.expectRevert(VotingFacet.UnknownProposal.selector);
        g.execute(999);
    }

    function test_execute_reverts_amount_exceeds_treasury_drained_midvote() public {
        // Pass a proposal, but the Admin drains the treasury (via the Admin
        // spend path) before execute -> execute re-checks the LIVE balance,
        // reverts AmountExceedsTreasury, leaves the proposal Active to retry.
        uint256 id = _guild4(); // FUND
        uint256 pid = _propose(id, alice, 800 ether);
        vm.prank(founder); // Admin FOR (satisfies the admin-consent gate)
        g.vote(pid, true);
        vm.prank(alice);
        g.vote(pid, true);
        vm.prank(bob);
        g.vote(pid, true);
        // Admin drains 500 -> only 500 left, < the 800 proposal.
        vm.prank(founder);
        g.spendTreasury(id, payee, 500 ether, "drain");

        vm.warp(block.timestamp + PERIOD + 1);
        vm.expectRevert(VotingFacet.AmountExceedsTreasury.selector);
        g.execute(pid);
        // Still Active (retryable) - not burned.
        (,,,,, uint8 st,,) = g.getProposal(pid);
        assertEq(st, uint8(LibVotingStorage.VStatus.Active), "stays Active to retry after refund");
        // Refund the treasury and execute succeeds.
        _fund(id, 500 ether); // now 500 + 500 = 1000 >= 800
        g.execute(pid);
        (,,,,, uint8 st2,,) = g.getProposal(pid);
        assertEq(st2, uint8(LibVotingStorage.VStatus.Executed), "retried + executed");
    }

    // =====================================================================
    // VIEWS - getProposal / proposalsOf / hasVoted / tallyOf
    // =====================================================================

    function test_getProposal_records_fields() public {
        uint256 id = _guild4();
        vm.prank(alice);
        uint256 pid = g.propose(id, payee, 123 ether, "memo", PERIOD);
        (
            uint256 gid,
            address proposer,
            address to,
            uint256 amount,
            uint64 deadline,
            uint8 status,
            uint256 fv,
            uint256 av
        ) = g.getProposal(pid);
        assertEq(gid, id);
        assertEq(proposer, alice);
        assertEq(to, payee);
        assertEq(amount, 123 ether);
        assertEq(deadline, uint64(block.timestamp) + PERIOD);
        assertEq(status, uint8(LibVotingStorage.VStatus.Active));
        assertEq(fv, 0);
        assertEq(av, 0);
        assertEq(g.proposalMemoOf(pid), "memo");
        assertEq(g.proposalCount(), 1);
    }

    function test_getProposal_reverts_unknown() public {
        vm.expectRevert(VotingFacet.UnknownProposal.selector);
        g.getProposal(999);
    }

    function test_proposalsOf_paginates() public {
        uint256 id = _guild4();
        uint256 p1 = _propose(id, alice, 1 ether);
        uint256 p2 = _propose(id, bob, 2 ether);
        uint256 p3 = _propose(id, carol, 3 ether);

        (uint256[] memory page1, uint256 cur1) = g.proposalsOf(id, 0, 2);
        assertEq(page1.length, 2);
        assertEq(page1[0], p1);
        assertEq(page1[1], p2);
        assertEq(cur1, 2);

        (uint256[] memory page2, uint256 cur2) = g.proposalsOf(id, cur1, 2);
        assertEq(page2.length, 1);
        assertEq(page2[0], p3);
        assertEq(cur2, 3);

        // past the end -> empty.
        (uint256[] memory page3,) = g.proposalsOf(id, 3, 2);
        assertEq(page3.length, 0);
    }

    function test_hasVoted_tracks_ballots() public {
        uint256 id = _guild4();
        uint256 pid = _propose(id, alice, 100 ether);
        assertFalse(g.hasVoted(pid, alice));
        vm.prank(alice);
        g.vote(pid, true);
        assertTrue(g.hasVoted(pid, alice));
        assertFalse(g.hasVoted(pid, bob));
    }

    function test_tallyOf_projects_outcome() public {
        uint256 id = _guild4(); // 4 members, quorum 2
        uint256 pid = _propose(id, alice, 100 ether);
        vm.prank(alice);
        g.vote(pid, true);
        (uint256 fv, uint256 av, uint256 q, uint256 cast, bool passing) = g.tallyOf(pid);
        assertEq(fv, 1);
        assertEq(av, 0);
        assertEq(q, 2, "ceil(4/2)");
        assertEq(cast, 1);
        assertFalse(passing, "1 < quorum 2 -> not passing yet");
        vm.prank(bob);
        g.vote(pid, true);
        (,,, uint256 cast2, bool passing2) = g.tallyOf(pid);
        assertEq(cast2, 2);
        assertFalse(passing2, "quorum + majority met but no Admin FOR yet -> not passing");
        vm.prank(founder); // Admin FOR satisfies the admin-consent gate
        g.vote(pid, true);
        (,,, uint256 cast3, bool passing3) = g.tallyOf(pid);
        assertEq(cast3, 3);
        assertTrue(passing3, "quorum + majority + Admin FOR -> passing");
    }

    // =====================================================================
    // REENTRANCY PROBE - a hostile token re-enters during execute's payout
    // =====================================================================

    function test_reentrant_execute_cannot_double_spend() public {
        ReentrantLH rlh = new ReentrantLH();
        VotingHarness h = new VotingHarness();
        h._setCreditsToken(address(rlh));
        rlh.mint(founder, 1_000_000 ether);
        vm.prank(founder);
        rlh.approve(address(h), type(uint256).max);
        vm.warp(1_000_000);

        vm.prank(founder);
        uint256 id = h.createGuild("reguild"); // founder sole member, quorum 1
        vm.prank(founder);
        h.fundGuild(id, FUND);

        // Extra unrelated balance so a SUCCESSFUL double-drain would have
        // something to steal (proving the revert is the defense).
        rlh.mint(address(h), 1_000_000 ether);

        vm.prank(founder);
        uint256 pid = h.propose(id, payee, 300 ether, "", PERIOD);
        vm.prank(founder);
        h.vote(pid, true);
        vm.warp(block.timestamp + PERIOD + 1);

        uint256 diamondBefore = rlh.balanceOf(address(h));
        rlh.arm(address(h), pid);
        h.execute(pid);

        assertTrue(rlh.reenterReverted(), "re-entrant execute reverted (ProposalNotActive)");
        assertEq(rlh.balanceOf(address(h)), diamondBefore - 300 ether, "exactly one spend");
        assertEq(h.treasuryBalanceOf(id), FUND - 300 ether, "treasury debited once");
    }

    // =====================================================================
    // RECURSIVE - a guild's TBA (a CONTRACT) is a member + votes
    // (Part 4: DAOs-of-DAOs, "turtles all the way down")
    // =====================================================================

    function test_contract_member_can_vote_the_turtles_property() public {
        // Parent DAO with founder + alice as members, funded.
        uint256 parent = _create("parentdao");
        _seat(parent, alice);
        _fund(parent, FUND);

        // A child guild whose ADDRESS (its TBA - a CONTRACT account) becomes
        // a member of the parent and CASTS A VOTE.
        vm.prank(bob);
        uint256 child = g.createGuild("childdao");
        address childAddr = g.guildAddress(child);
        assertTrue(childAddr != address(0), "child guild has a contract wallet");

        // Seat the child's TBA as a parent member -> 3 members, quorum = 2.
        vm.prank(founder);
        g.setRole(parent, childAddr, uint8(LibGuildStorage.Role.Member));
        assertTrue(g.isGuildMember(parent, childAddr), "contract is a member");

        // Open a proposal; the CONTRACT member votes (pranked as its address
        // - in production a sponsored call FROM the MultiSignerAccount; the
        // facet keys purely on msg.sender, never on EOA-ness, the whole
        // recursive point).
        uint256 pid = _propose(parent, alice, 400 ether);
        vm.prank(founder); // Admin FOR (satisfies the admin-consent gate)
        g.vote(pid, true);
        vm.prank(alice);
        g.vote(pid, true);
        vm.prank(childAddr); // <-- a CONTRACT votes
        g.vote(pid, true);
        assertTrue(g.hasVoted(pid, childAddr), "the contract-member voted");

        (,, uint256 q, uint256 cast, bool passing) = g.tallyOf(pid);
        assertEq(q, 2, "3-member quorum");
        assertEq(cast, 3, "founder + alice + the child-DAO");
        assertTrue(passing, "quorum met + Admin FOR -> passes with a contract voter");

        vm.warp(block.timestamp + PERIOD + 1);
        g.execute(pid);
        assertEq(lh.balanceOf(payee), 400 ether, "executed - a contract member helped pass it");

        // And the parent can spend its treasury TO the child-DAO (recursive
        // money flow) via a proposal whose recipient is the child contract.
        uint256 pid2 = _propose(parent, alice, 100 ether); // recipient = payee for simplicity
        // re-target by proposing to the child directly:
        vm.prank(alice);
        uint256 pid3 = g.propose(parent, childAddr, 100 ether, "sub-dao grant", PERIOD);
        vm.prank(founder); // Admin FOR (satisfies the admin-consent gate)
        g.vote(pid3, true);
        vm.prank(alice);
        g.vote(pid3, true);
        vm.prank(childAddr);
        g.vote(pid3, true);
        vm.warp(block.timestamp + PERIOD + 1);
        uint256 childBefore = lh.balanceOf(childAddr);
        g.execute(pid3);
        assertEq(
            lh.balanceOf(childAddr), childBefore + 100 ether, "parent DAO funded the child DAO's wallet"
        );
        // pid2 left unvoted/unexecuted intentionally (only referenced to
        // show multiple proposals coexist); silence the unused warning.
        pid2;
    }

    // =====================================================================
    // FUZZ - treasury conservation across propose/vote/execute cycles
    // =====================================================================

    /// The diamond's `$LH` held for the guild treasury always equals the
    /// guild's `guildBalance` ledger - every executed proposal moves the
    /// escrow and the ledger in lockstep through the shared `_spend`. The
    /// diamond holds ONLY this guild's treasury here.
    function testFuzz_treasury_conservation(uint256 seedRaw) public {
        uint256 seed = seedRaw;
        uint256 id = _create("fuzzdao");
        _seat(id, alice);
        _seat(id, bob); // 3 members, quorum 2

        for (uint256 i = 0; i < 25; i++) {
            seed = uint256(keccak256(abi.encode(seed, i)));
            // Always keep the treasury topped so proposals are affordable.
            _fund(id, 100 ether);

            uint256 amt = 1 + (seed % 50) * 1 ether;
            vm.prank(alice);
            uint256 pid = g.propose(id, payee, amt, "", PERIOD);

            // Random support pattern across the 3 members.
            bool a = (seed & 1) == 1;
            bool b = (seed & 2) == 2;
            vm.prank(alice);
            g.vote(pid, a);
            vm.prank(bob);
            g.vote(pid, b);
            // founder swings it sometimes.
            if ((seed & 4) == 4) {
                vm.prank(founder);
                g.vote(pid, true);
            }

            vm.warp(block.timestamp + PERIOD + 1);
            g.execute(pid); // passes or fails; either way conserves

            // INVARIANT: diamond $LH == the guild's ledger balance.
            assertEq(
                lh.balanceOf(address(g)),
                g.treasuryBalanceOf(id),
                "diamond $LH == guild treasury ledger"
            );
        }
    }
}
