// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {GuildFacet} from "../src/facets/GuildFacet.sol";
import {LibGuildStorage} from "../src/libraries/LibGuildStorage.sol";
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

contract GuildHarness is GuildFacet {
    function _setCreditsToken(address token) external {
        LibCreditsStorage.load().creditsToken = token;
    }

    function tokenBoundAccount(uint256 tokenId) external view returns (address) {
        if (LibRegistryStorage.load().ownerOfId[tokenId] == address(0)) return address(0);
        return address(uint160(uint256(keccak256(abi.encodePacked("tba", tokenId)))));
    }

    /// Expose the raw admin/member counters so the invariant tests can assert
    /// on the storage that backs the last-Admin freeze guard.
    function _adminCount(uint256 guildId) external view returns (uint32) {
        return LibGuildStorage.load().guilds[guildId].adminCount;
    }

    function _memberCount(uint256 guildId) external view returns (uint64) {
        return LibGuildStorage.load().guilds[guildId].memberCount;
    }
}

/// Adversarial governance/role suite for GuildFacet: the "guild can never
/// become un-administrable (treasury freeze)" invariant + the count integrity
/// that backs it, plus the role-gate-bypass attempts. These pin the SOUND
/// paths so the regression gate covers them.
contract GuildRoleAdversarialTest is Test {
    GuildHarness g;
    MockLH lh;

    address founder = address(0xF00D);
    address alice = address(0xA11CE);
    address bob = address(0xB0B);
    address carol = address(0xCA401);
    address mallory = address(0x4A11074); // the attacker
    address payee = address(0x7BA);

    uint256 constant FUND = 1_000 ether;

    function setUp() public {
        g = new GuildHarness();
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

    function _seat(uint256 id, address m, LibGuildStorage.Role r) internal {
        vm.prank(founder);
        g.setRole(id, m, uint8(r));
    }

    // =====================================================================
    // The treasury-FREEZE invariant: a guild ALWAYS retains >= 1 Admin, so
    // its treasury can never be locked forever. Every path that could remove
    // the last Admin must revert LastAdmin.
    // =====================================================================

    function test_sole_admin_cannot_self_demote_to_member() public {
        uint256 id = _create("freeze1");
        vm.prank(founder);
        vm.expectRevert(GuildFacet.LastAdmin.selector);
        g.setRole(id, founder, uint8(LibGuildStorage.Role.Member));
        assertEq(g._adminCount(id), 1, "still one Admin");
    }

    function test_sole_admin_cannot_self_demote_to_officer() public {
        uint256 id = _create("freeze2");
        vm.prank(founder);
        vm.expectRevert(GuildFacet.LastAdmin.selector);
        g.setRole(id, founder, uint8(LibGuildStorage.Role.Officer));
    }

    function test_sole_admin_cannot_self_evict() public {
        uint256 id = _create("freeze3");
        vm.prank(founder);
        vm.expectRevert(GuildFacet.LastAdmin.selector);
        g.setRole(id, founder, uint8(LibGuildStorage.Role.None));
    }

    function test_sole_admin_cannot_leave() public {
        uint256 id = _create("freeze4");
        vm.prank(founder);
        vm.expectRevert(GuildFacet.LastAdmin.selector);
        g.leaveGuild(id);
    }

    /// The full handoff dance: promote a replacement Admin, THEN the original
    /// can leave — and the guild still has an Admin (never frozen).
    function test_admin_handoff_then_leave_keeps_one_admin() public {
        uint256 id = _create("handoff");
        _seat(id, alice, LibGuildStorage.Role.Admin); // 2 admins
        assertEq(g._adminCount(id), 2);
        vm.prank(founder);
        g.leaveGuild(id); // now 1 admin (alice)
        assertEq(g._adminCount(id), 1, "alice remains Admin");
        assertEq(g.roleOf(id, founder), uint8(LibGuildStorage.Role.None));
        // alice (now sole) is herself frozen from leaving.
        vm.prank(alice);
        vm.expectRevert(GuildFacet.LastAdmin.selector);
        g.leaveGuild(id);
        // ...and can still spend (treasury never frozen).
        vm.prank(founder);
        g.fundGuild(id, FUND);
        vm.prank(alice);
        g.spendTreasury(id, payee, 100 ether, "");
        assertEq(lh.balanceOf(payee), 100 ether, "admin can spend -> treasury never frozen");
    }

    /// One of TWO admins CAN be demoted/evicted (adminCount stays >= 1).
    function test_one_of_two_admins_can_be_demoted() public {
        uint256 id = _create("twoadmin");
        _seat(id, alice, LibGuildStorage.Role.Admin); // 2 admins
        vm.prank(founder);
        g.setRole(id, alice, uint8(LibGuildStorage.Role.Member)); // demote alice
        assertEq(g._adminCount(id), 1, "one admin remains");
        assertEq(g.roleOf(id, alice), uint8(LibGuildStorage.Role.Member));
    }

    // =====================================================================
    // ROLE-GATE bypass attempts (the privilege-escalation surface).
    // =====================================================================

    /// A plain Member cannot invite (Officer+ gate).
    function test_member_cannot_invite() public {
        uint256 id = _create("gate1");
        _seat(id, alice, LibGuildStorage.Role.Member);
        vm.prank(alice);
        vm.expectRevert(GuildFacet.NotOfficer.selector);
        g.inviteToGuild(id, bob);
    }

    /// An Officer cannot setRole (Admin-only) — no self-promotion to Admin.
    function test_officer_cannot_self_promote_to_admin() public {
        uint256 id = _create("gate2");
        _seat(id, alice, LibGuildStorage.Role.Officer);
        vm.prank(alice);
        vm.expectRevert(GuildFacet.NotAdmin.selector);
        g.setRole(id, alice, uint8(LibGuildStorage.Role.Admin));
        assertEq(g.roleOf(id, alice), uint8(LibGuildStorage.Role.Officer), "no escalation");
    }

    /// A non-member cannot spend the treasury (Admin-only) — the headline
    /// economic gate.
    function test_nonmember_cannot_spend_treasury() public {
        uint256 id = _create("gate3");
        vm.prank(founder);
        g.fundGuild(id, FUND);
        vm.prank(mallory);
        vm.expectRevert(GuildFacet.NotAdmin.selector);
        g.spendTreasury(id, mallory, FUND, "steal");
        assertEq(g.treasuryBalanceOf(id), FUND, "treasury intact");
    }

    /// An Officer cannot spend the treasury (Admin-only).
    function test_officer_cannot_spend_treasury() public {
        uint256 id = _create("gate4");
        _seat(id, alice, LibGuildStorage.Role.Officer);
        vm.prank(founder);
        g.fundGuild(id, FUND);
        vm.prank(alice);
        vm.expectRevert(GuildFacet.NotAdmin.selector);
        g.spendTreasury(id, payee, 1 ether, "");
    }

    // =====================================================================
    // COUNT INTEGRITY: memberCount / adminCount can't be corrupted by
    // idempotent or churning role sets (the underflow / double-count guard).
    // =====================================================================

    /// Re-seating the same member at the same role is a no-op (no double
    /// member-count increment, no enumerable-list duplication).
    function test_idempotent_setRole_no_double_count() public {
        uint256 id = _create("count1");
        _seat(id, alice, LibGuildStorage.Role.Member);
        uint64 mc = g._memberCount(id);
        // Set the SAME role again — must be a no-op.
        _seat(id, alice, LibGuildStorage.Role.Member);
        assertEq(g._memberCount(id), mc, "no double increment");
        assertEq(g.guildMembersOf(id).length, mc, "enumerable list matches count");
    }

    /// A promote-then-demote round trip leaves the counts exactly where they
    /// started (no adminCount drift that could fake-satisfy the freeze guard).
    function test_promote_demote_roundtrip_count_integrity() public {
        uint256 id = _create("count2");
        _seat(id, alice, LibGuildStorage.Role.Member);
        _seat(id, bob, LibGuildStorage.Role.Member);
        assertEq(g._memberCount(id), 3); // founder + alice + bob
        assertEq(g._adminCount(id), 1);

        _seat(id, alice, LibGuildStorage.Role.Admin); // promote
        assertEq(g._adminCount(id), 2);
        _seat(id, alice, LibGuildStorage.Role.Officer); // demote (2 admins -> ok)
        assertEq(g._adminCount(id), 1, "adminCount back to 1");
        assertEq(g._memberCount(id), 3, "memberCount unchanged through role churn");
    }

    /// Evicting a member then re-seating them keeps the enumerable set + the
    /// count consistent (the swap-pop bookkeeping under churn).
    function test_evict_reseat_churn_keeps_set_consistent() public {
        uint256 id = _create("count3");
        _seat(id, alice, LibGuildStorage.Role.Member);
        _seat(id, bob, LibGuildStorage.Role.Member);
        _seat(id, carol, LibGuildStorage.Role.Member);
        assertEq(g._memberCount(id), 4);

        _seat(id, bob, LibGuildStorage.Role.None); // evict the MIDDLE one
        assertEq(g._memberCount(id), 3);
        assertEq(g.guildMembersOf(id).length, 3, "enumerable shrank");
        assertFalse(g.isGuildMember(id, bob), "bob gone");
        assertTrue(g.isGuildMember(id, carol), "carol still present (swap-pop intact)");

        _seat(id, bob, LibGuildStorage.Role.Member); // re-seat
        assertEq(g._memberCount(id), 4);
        assertTrue(g.isGuildMember(id, bob), "bob re-added cleanly");

        // guildsOf bookkeeping is consistent too.
        uint256[] memory bobGuilds = g.guildsOf(bob);
        assertEq(bobGuilds.length, 1, "bob in exactly one guild after the churn");
        assertEq(bobGuilds[0], id);
    }
}
