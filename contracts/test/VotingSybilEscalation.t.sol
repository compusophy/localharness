// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {VotingFacet} from "../src/facets/VotingFacet.sol";
import {GuildFacet} from "../src/facets/GuildFacet.sol";
import {LibGuildStorage} from "../src/libraries/LibGuildStorage.sol";
import {LibVotingStorage} from "../src/libraries/LibVotingStorage.sol";
import {LibCreditsStorage} from "../src/libraries/LibCreditsStorage.sol";
import {LibRegistryStorage} from "../src/libraries/LibRegistryStorage.sol";

/// $LH-shaped TIP-20 mock (escrow/spend surface the facet uses).
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
        require(a >= amt && balanceOf[from] >= amt, "insufficient");
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

/// VOTE-1 — the sybil-flood privilege-escalation exploit and its fix.
///
/// A compromised Officer can INVITE (Officer+ gate) + self-ACCEPT (free) N
/// sybil Members it controls, all BEFORE proposing — so the propose-time
/// member-count snapshot already counts them (the prior snapshot fix does NOT
/// help). The Officer then proposes a self-serving treasury drain and votes
/// Officer+N FOR, meeting quorum on a bare majority and EXECUTING — bypassing
/// the Admin-only `spendTreasury` gate.
///
/// FIX: a passing treasury spend must carry >= 1 Admin FOR vote
/// (`forAdminVotes > 0`). Sybil Members carry no Admin weight, so the drain
/// can no longer pass without Admin consent.
contract VotingSybilEscalationTest is Test {
    VotingHarness g;
    MockLH lh;

    address founder = address(0xF00D); // the guild's only Admin
    address officer = address(0x0FF1CE); // a (compromised) Officer
    address payee = address(0x7BA); // the attacker's drain target

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

    function _seat(uint256 id, address member, LibGuildStorage.Role role) internal {
        vm.prank(founder);
        g.setRole(id, member, uint8(role));
    }

    /// Officer mints `n` sybil Members it controls, via the real
    /// inviteToGuild (Officer+) + acceptGuildInvite (free) path.
    function _floodSybils(uint256 id, uint256 n) internal returns (address[] memory sybils) {
        sybils = new address[](n);
        for (uint256 i = 0; i < n; i++) {
            address s = address(uint160(0x5B11_0000 + i));
            sybils[i] = s;
            vm.prank(officer);
            g.inviteToGuild(id, s); // Officer+ invite
            vm.prank(s);
            g.acceptGuildInvite(id); // free accept
        }
    }

    // =====================================================================
    // EXPLOIT: rogue Officer floods sybils, passes a bare-majority drain.
    // After the fix the measure FAILS (no Admin FOR vote) — treasury safe.
    // =====================================================================
    function test_officer_sybil_flood_cannot_drain_treasury() public {
        vm.prank(founder);
        uint256 id = g.createGuild("victimdao"); // founder = Admin (member 1)
        _seat(id, officer, LibGuildStorage.Role.Officer); // member 2

        // 5 more HONEST Members (founder + officer + 5 = 7 honest members).
        for (uint256 i = 0; i < 5; i++) {
            _seat(id, address(uint160(0xA000 + i)), LibGuildStorage.Role.Member);
        }
        vm.prank(founder);
        g.fundGuild(id, FUND);

        // Officer mints 6 sybils it controls -> 13 members total.
        address[] memory sybils = _floodSybils(id, 6);
        assertEq(g.guildMembersOf(id).length, 13, "7 honest + 6 sybils");

        // Officer proposes the self-serving drain (snapshot member count = 13,
        // quorum = ceil(13/2) = 7).
        vm.prank(officer);
        uint256 pid = g.propose(id, payee, 500 ether, "drain", PERIOD);

        // Officer + its 6 sybils all vote FOR = 7 FOR, meeting quorum 7 on a
        // bare majority. NONE of them is an Admin.
        vm.prank(officer);
        g.vote(pid, true);
        for (uint256 i = 0; i < sybils.length; i++) {
            vm.prank(sybils[i]);
            g.vote(pid, true);
        }

        (uint256 fv, uint256 av, uint256 q, uint256 cast, bool passing) = g.tallyOf(pid);
        assertEq(fv, 7, "7 FOR votes mustered");
        assertEq(av, 0);
        assertEq(q, 7, "snapshot quorum ceil(13/2)");
        assertEq(cast, 7, "quorum met on the cast tally");
        assertFalse(passing, "but NOT passing - no Admin FOR vote (the fix)");

        // Execute after the deadline: the drain FAILS, treasury untouched.
        vm.warp(block.timestamp + PERIOD + 1);
        g.execute(pid);
        assertEq(g.treasuryBalanceOf(id), FUND, "treasury NOT drained by sybil flood");
        assertEq(lh.balanceOf(payee), 0, "attacker unpaid");
        (,,,,, uint8 st,,) = g.getProposal(pid);
        assertEq(st, uint8(LibVotingStorage.VStatus.Failed), "sybil drain Failed");
    }

    // =====================================================================
    // NO REGRESSION: a legitimate Admin-backed measure still passes. Same
    // 13-member guild, same quorum, but now an Admin (founder) votes FOR.
    // =====================================================================
    function test_admin_backed_proposal_still_passes() public {
        vm.prank(founder);
        uint256 id = g.createGuild("legitdao"); // founder = Admin
        _seat(id, officer, LibGuildStorage.Role.Officer);
        for (uint256 i = 0; i < 5; i++) {
            _seat(id, address(uint160(0xA000 + i)), LibGuildStorage.Role.Member);
        }
        vm.prank(founder);
        g.fundGuild(id, FUND);
        address[] memory extra = _floodSybils(id, 6); // 13 members, quorum 7

        vm.prank(founder);
        uint256 pid = g.propose(id, payee, 300 ether, "grant", PERIOD);

        // The Admin (founder) + officer + 5 honest members back it = 7 FOR,
        // quorum met AND an Admin voted FOR.
        vm.prank(founder); // Admin FOR vote
        g.vote(pid, true);
        vm.prank(officer);
        g.vote(pid, true);
        for (uint256 i = 0; i < 5; i++) {
            vm.prank(address(uint160(0xA000 + i)));
            g.vote(pid, true);
        }

        (,,,, bool passing) = g.tallyOf(pid);
        assertTrue(passing, "Admin-backed quorate majority passes");

        vm.warp(block.timestamp + PERIOD + 1);
        g.execute(pid);
        assertEq(lh.balanceOf(payee), 300 ether, "legit Admin-backed measure executed");
        (,,,,, uint8 st,,) = g.getProposal(pid);
        assertEq(st, uint8(LibVotingStorage.VStatus.Executed));
        extra; // silence unused
    }
}
