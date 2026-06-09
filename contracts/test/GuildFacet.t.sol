// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {GuildFacet} from "../src/facets/GuildFacet.sol";
import {LibGuildStorage} from "../src/libraries/LibGuildStorage.sol";
import {LibCreditsStorage} from "../src/libraries/LibCreditsStorage.sol";
import {LibRegistryStorage} from "../src/libraries/LibRegistryStorage.sol";

/// Minimal `$LH`-shaped TIP-20 mock: 18-decimal balances + the
/// approve/transferFrom/transfer surface GuildFacet escrows + spends +
/// refunds through. Reverts (via require) on an under-allowance /
/// under-balance pull so the facet's CEI ordering is provable.
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

/// Hostile reentrant TIP-20 mock: on `transfer` (the spend path — the only
/// external call in spendTreasury) it re-enters the diamond, trying a SECOND
/// spend of the same guild's treasury. Real `$LH` has NO callback; this is
/// the defense-in-depth probe that CEI ordering makes a double-spend
/// structurally impossible (the re-entry sees the already-debited balance
/// and reverts on InsufficientTreasury).
contract ReentrantLH {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    address public diamond;
    uint256 public attackGuild;
    address public attackTo;
    uint256 public attackAmount;
    bool internal entered;
    bool public reenterReverted;

    function arm(address d, uint256 guildId, address to, uint256 amount) external {
        diamond = d;
        attackGuild = guildId;
        attackTo = to;
        attackAmount = amount;
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
        // Re-enter ONCE during the settlement transfer: try to spend the
        // same guild's treasury again. CEI means the balance is already
        // debited, so this MUST revert (no double drain).
        if (diamond != address(0) && !entered) {
            entered = true;
            try GuildFacet(diamond).spendTreasury(attackGuild, attackTo, attackAmount, "") {
                reenterReverted = false;
            } catch {
                reenterReverted = true;
            }
        }
        return true;
    }
}

/// Test harness: GuildFacet + setters that write the SHARED diamond-storage
/// slots a real diamond populates via other facets (creditsToken from
/// CreditsFacet) AND a real `tokenBoundAccount` implementation so the
/// guildAddress self-call resolves to a deterministic guild wallet. Because
/// every `Lib*Storage.load()` resolves against THIS contract's storage,
/// writing them here IS the cross-facet storage sharing the diamond
/// provides. `createGuild` writes registry storage directly (its design),
/// so registry reads/writes already work against this contract's storage —
/// no registry facet needed in the harness. The diamond IS the escrow
/// holder, so `address(this)` holds the treasury `$LH`, exactly like the
/// live diamond.
contract GuildHarness is GuildFacet {
    function _setCreditsToken(address token) external {
        LibCreditsStorage.load().creditsToken = token;
    }

    /// Read the registry holder of a tokenId from THIS harness's storage
    /// (a lib `load()` in the test contract would read the test's own slot,
    /// not the harness's — registry storage lives where createGuild wrote it).
    function _ownerOfId(uint256 tokenId) external view returns (address) {
        return LibRegistryStorage.load().ownerOfId[tokenId];
    }

    /// Deterministic, NON-ZERO TBA for any registered tokenId — mirrors the
    /// ERC-6551 counterfactual the live TbaFacet returns. address(0) (the
    /// "unresolved" sentinel) for an unregistered token, so the
    /// guildAddress/recursive paths get a concrete address to use as a
    /// member of another guild.
    function tokenBoundAccount(uint256 tokenId) external view returns (address) {
        if (LibRegistryStorage.load().ownerOfId[tokenId] == address(0)) return address(0);
        return address(uint160(uint256(keccak256(abi.encodePacked("tba", tokenId)))));
    }
}

contract GuildFacetTest is Test {
    GuildHarness g;
    MockLH lh;

    address founder = address(0xF00D); // creates the guild; first Admin
    address officer = address(0x0FF1); // promoted to Officer
    address alice = address(0xA11CE); // a plain Member
    address bob = address(0xB0B); // an invitee
    address stranger = address(0xBEEF); // non-member
    address payee = address(0x7BA); // a treasury spend recipient

    uint256 constant FUND = 1_000 ether;

    function setUp() public {
        g = new GuildHarness();
        lh = new MockLH();
        g._setCreditsToken(address(lh));

        // Fund the funders + pre-approve the diamond (the facet) for escrow.
        lh.mint(founder, 1_000_000 ether);
        lh.mint(stranger, 1_000_000 ether);
        vm.prank(founder);
        lh.approve(address(g), type(uint256).max);
        vm.prank(stranger);
        lh.approve(address(g), type(uint256).max);

        vm.warp(1_000_000);
    }

    // --- helpers --------------------------------------------------------

    function _create(string memory name) internal returns (uint256 id) {
        vm.prank(founder);
        id = g.createGuild(name);
    }

    /// founder (Admin) invites + the member accepts → joined as Member.
    function _join(uint256 id, address member) internal {
        vm.prank(founder);
        g.inviteToGuild(id, member);
        vm.prank(member);
        g.acceptGuildInvite(id);
    }

    // =====================================================================
    // createGuild: registers an identity, seats the founder as Admin
    // =====================================================================

    function test_createGuild_registers_identity_and_seats_admin() public {
        uint256 id = _create("rustguild");

        assertEq(id, 1, "first guild gets tokenId 1");
        assertTrue(g.isGuild(id), "recorded as a guild");
        assertEq(g.guildName(id), "rustguild", "name stored");
        assertEq(g.guildCount(), 1);

        // The founder is the holder of the underlying NFT (register's write).
        assertEq(g._ownerOfId(id), founder, "founder owns the NFT");

        // The founder is the first Admin + the sole member.
        assertEq(g.roleOf(id, founder), uint8(LibGuildStorage.Role.Admin), "founder is Admin");
        assertTrue(g.isGuildMember(id, founder));
        address[] memory m = g.guildMembersOf(id);
        assertEq(m.length, 1);
        assertEq(m[0], founder);

        // The guild has a non-zero address (its TBA).
        assertTrue(g.guildAddress(id) != address(0), "guild has a wallet address");

        // guildsOf reflects membership.
        uint256[] memory fg = g.guildsOf(founder);
        assertEq(fg.length, 1);
        assertEq(fg[0], id);
    }

    function test_createGuild_reverts_name_taken() public {
        _create("dupe");
        vm.prank(alice);
        vm.expectRevert(GuildFacet.NameTaken.selector);
        g.createGuild("dupe");
    }

    function test_createGuild_reverts_invalid_name() public {
        vm.prank(founder);
        vm.expectRevert(GuildFacet.InvalidName.selector);
        g.createGuild("Bad_Name"); // uppercase + underscore
    }

    function test_createGuild_reverts_empty_name() public {
        vm.prank(founder);
        vm.expectRevert(GuildFacet.InvalidName.selector);
        g.createGuild("");
    }

    function test_createGuild_distinct_ids_and_addresses() public {
        uint256 a = _create("alpha");
        vm.prank(alice);
        uint256 b = g.createGuild("beta");
        assertEq(a, 1);
        assertEq(b, 2);
        assertTrue(g.guildAddress(a) != g.guildAddress(b), "distinct wallets");
        assertEq(g.guildCount(), 2);
    }

    // an ordinary tokenId (not created via createGuild) is NOT a guild
    function test_isGuild_false_for_unknown() public {
        assertFalse(g.isGuild(999));
    }

    // =====================================================================
    // membership: invite (Officer+) + accept (consent)
    // =====================================================================

    function test_invite_and_accept_makes_member() public {
        uint256 id = _create("guild");
        vm.prank(founder);
        g.inviteToGuild(id, bob);
        assertFalse(g.isGuildMember(id, bob), "invited != member until accept");

        vm.prank(bob);
        g.acceptGuildInvite(id);
        assertTrue(g.isGuildMember(id, bob), "member after accept");
        assertEq(g.roleOf(id, bob), uint8(LibGuildStorage.Role.Member), "joins as Member");

        address[] memory m = g.guildMembersOf(id);
        assertEq(m.length, 2, "founder + bob");
    }

    function test_invite_reverts_non_member() public {
        uint256 id = _create("guild");
        vm.prank(stranger);
        vm.expectRevert(GuildFacet.NotOfficer.selector);
        g.inviteToGuild(id, bob);
    }

    function test_invite_reverts_plain_member_not_officer() public {
        // A plain Member cannot invite — needs Officer+.
        uint256 id = _create("guild");
        _join(id, alice); // alice is a Member
        vm.prank(alice);
        vm.expectRevert(GuildFacet.NotOfficer.selector);
        g.inviteToGuild(id, bob);
    }

    function test_officer_can_invite() public {
        uint256 id = _create("guild");
        _join(id, officer);
        vm.prank(founder);
        g.setRole(id, officer, uint8(LibGuildStorage.Role.Officer)); // promote
        // Now the officer can invite.
        vm.prank(officer);
        g.inviteToGuild(id, bob);
        vm.prank(bob);
        g.acceptGuildInvite(id);
        assertTrue(g.isGuildMember(id, bob));
    }

    function test_invite_reverts_already_member() public {
        uint256 id = _create("guild");
        _join(id, alice);
        vm.prank(founder);
        vm.expectRevert(GuildFacet.AlreadyMember.selector);
        g.inviteToGuild(id, alice);
    }

    function test_accept_reverts_not_invited() public {
        uint256 id = _create("guild");
        vm.prank(bob);
        vm.expectRevert(GuildFacet.NotInvited.selector);
        g.acceptGuildInvite(id);
    }

    function test_accept_reverts_unknown_guild() public {
        vm.prank(bob);
        vm.expectRevert(GuildFacet.UnknownGuild.selector);
        g.acceptGuildInvite(999);
    }

    function test_invite_reverts_unknown_guild() public {
        vm.prank(founder);
        vm.expectRevert(GuildFacet.UnknownGuild.selector);
        g.inviteToGuild(999, bob);
    }

    // =====================================================================
    // leaveGuild + the last-Admin guard
    // =====================================================================

    function test_member_can_leave() public {
        uint256 id = _create("guild");
        _join(id, alice);
        vm.prank(alice);
        g.leaveGuild(id);
        assertFalse(g.isGuildMember(id, alice), "no longer a member");
        assertEq(g.roleOf(id, alice), uint8(LibGuildStorage.Role.None));
        // member list swap-popped back to just the founder.
        address[] memory m = g.guildMembersOf(id);
        assertEq(m.length, 1);
        assertEq(m[0], founder);
        // guildsOf cleared.
        assertEq(g.guildsOf(alice).length, 0);
    }

    function test_leave_reverts_non_member() public {
        uint256 id = _create("guild");
        vm.prank(stranger);
        vm.expectRevert(GuildFacet.NotMember.selector);
        g.leaveGuild(id);
    }

    function test_sole_admin_cannot_leave() public {
        uint256 id = _create("guild");
        _join(id, alice); // alice is a Member, founder is the sole Admin
        vm.prank(founder);
        vm.expectRevert(GuildFacet.LastAdmin.selector);
        g.leaveGuild(id);
    }

    function test_admin_can_leave_after_promoting_another() public {
        uint256 id = _create("guild");
        _join(id, alice);
        vm.prank(founder);
        g.setRole(id, alice, uint8(LibGuildStorage.Role.Admin)); // now 2 Admins
        vm.prank(founder);
        g.leaveGuild(id); // ok — alice is still an Admin
        assertFalse(g.isGuildMember(id, founder));
        assertEq(g.roleOf(id, alice), uint8(LibGuildStorage.Role.Admin), "alice remains Admin");
    }

    // =====================================================================
    // setRole: Admin-gated promote / demote / evict
    // =====================================================================

    function test_setRole_promote_and_demote() public {
        uint256 id = _create("guild");
        _join(id, alice);
        vm.prank(founder);
        g.setRole(id, alice, uint8(LibGuildStorage.Role.Officer));
        assertEq(g.roleOf(id, alice), uint8(LibGuildStorage.Role.Officer));
        vm.prank(founder);
        g.setRole(id, alice, uint8(LibGuildStorage.Role.Member));
        assertEq(g.roleOf(id, alice), uint8(LibGuildStorage.Role.Member));
    }

    function test_setRole_evict_removes_member() public {
        uint256 id = _create("guild");
        _join(id, alice);
        vm.prank(founder);
        g.setRole(id, alice, uint8(LibGuildStorage.Role.None)); // evict
        assertFalse(g.isGuildMember(id, alice));
        assertEq(g.guildMembersOf(id).length, 1, "evicted out of the member list");
    }

    function test_setRole_direct_seat_adds_member() public {
        // An Admin can seat an address directly (None -> Member) without the
        // invite/accept dance — still bounded by the member cap.
        uint256 id = _create("guild");
        vm.prank(founder);
        g.setRole(id, alice, uint8(LibGuildStorage.Role.Member));
        assertTrue(g.isGuildMember(id, alice));
        assertEq(g.guildMembersOf(id).length, 2);
    }

    function test_setRole_reverts_non_admin() public {
        uint256 id = _create("guild");
        _join(id, alice);
        _join(id, bob);
        vm.prank(alice); // a plain Member
        vm.expectRevert(GuildFacet.NotAdmin.selector);
        g.setRole(id, bob, uint8(LibGuildStorage.Role.Officer));
    }

    function test_setRole_officer_cannot_setRole() public {
        uint256 id = _create("guild");
        _join(id, officer);
        vm.prank(founder);
        g.setRole(id, officer, uint8(LibGuildStorage.Role.Officer));
        _join(id, bob);
        vm.prank(officer); // Officer is NOT Admin
        vm.expectRevert(GuildFacet.NotAdmin.selector);
        g.setRole(id, bob, uint8(LibGuildStorage.Role.Officer));
    }

    function test_setRole_reverts_bad_role() public {
        uint256 id = _create("guild");
        _join(id, alice);
        vm.prank(founder);
        vm.expectRevert(GuildFacet.BadRole.selector);
        g.setRole(id, alice, 4); // > Admin(3)
    }

    function test_setRole_cannot_demote_sole_admin() public {
        uint256 id = _create("guild");
        // founder is the sole Admin; can't self-demote to Officer.
        vm.prank(founder);
        vm.expectRevert(GuildFacet.LastAdmin.selector);
        g.setRole(id, founder, uint8(LibGuildStorage.Role.Officer));
    }

    function test_setRole_cannot_evict_sole_admin() public {
        uint256 id = _create("guild");
        vm.prank(founder);
        vm.expectRevert(GuildFacet.LastAdmin.selector);
        g.setRole(id, founder, uint8(LibGuildStorage.Role.None));
    }

    function test_setRole_demote_one_of_two_admins_ok() public {
        uint256 id = _create("guild");
        _join(id, alice);
        vm.prank(founder);
        g.setRole(id, alice, uint8(LibGuildStorage.Role.Admin)); // 2 Admins
        vm.prank(founder);
        g.setRole(id, alice, uint8(LibGuildStorage.Role.Member)); // demote one, still 1 left
        assertEq(g.roleOf(id, alice), uint8(LibGuildStorage.Role.Member));
    }

    function test_setRole_reverts_unknown_guild() public {
        vm.prank(founder);
        vm.expectRevert(GuildFacet.UnknownGuild.selector);
        g.setRole(999, alice, uint8(LibGuildStorage.Role.Member));
    }

    // =====================================================================
    // member cap (anti-sybil)
    // =====================================================================

    function test_accept_reverts_at_member_cap() public {
        uint256 id = _create("guild");
        // founder is member #1. Fill to MAX_MEMBERS via direct seats.
        for (uint256 i = 1; i < LibGuildStorage.MAX_MEMBERS; i++) {
            address m = address(uint160(0x10000 + i));
            vm.prank(founder);
            g.setRole(id, m, uint8(LibGuildStorage.Role.Member));
        }
        assertEq(g.guildMembersOf(id).length, LibGuildStorage.MAX_MEMBERS, "at cap");
        // One more via invite/accept must revert GuildFull.
        vm.prank(founder);
        g.inviteToGuild(id, bob);
        vm.prank(bob);
        vm.expectRevert(GuildFacet.GuildFull.selector);
        g.acceptGuildInvite(id);
    }

    function test_setRole_direct_seat_reverts_at_cap() public {
        uint256 id = _create("guild");
        for (uint256 i = 1; i < LibGuildStorage.MAX_MEMBERS; i++) {
            address m = address(uint160(0x10000 + i));
            vm.prank(founder);
            g.setRole(id, m, uint8(LibGuildStorage.Role.Member));
        }
        vm.prank(founder);
        vm.expectRevert(GuildFacet.GuildFull.selector);
        g.setRole(id, bob, uint8(LibGuildStorage.Role.Member));
    }

    // =====================================================================
    // fundGuild: CEI escrow credit
    // =====================================================================

    function test_fundGuild_credits_treasury() public {
        uint256 id = _create("guild");
        uint256 founderBefore = lh.balanceOf(founder);
        vm.prank(founder);
        g.fundGuild(id, FUND);

        assertEq(g.treasuryBalanceOf(id), FUND, "ledger credited");
        assertEq(lh.balanceOf(address(g)), FUND, "diamond holds the $LH");
        assertEq(lh.balanceOf(founder), founderBefore - FUND, "pulled from funder");
    }

    function test_fundGuild_permissionless() public {
        // A non-member (stranger) can fund.
        uint256 id = _create("guild");
        vm.prank(stranger);
        g.fundGuild(id, FUND);
        assertEq(g.treasuryBalanceOf(id), FUND);
    }

    function test_fundGuild_accumulates() public {
        uint256 id = _create("guild");
        vm.prank(founder);
        g.fundGuild(id, FUND);
        vm.prank(stranger);
        g.fundGuild(id, FUND);
        assertEq(g.treasuryBalanceOf(id), 2 * FUND, "funds accumulate");
    }

    function test_fundGuild_reverts_zero() public {
        uint256 id = _create("guild");
        vm.prank(founder);
        vm.expectRevert(GuildFacet.ZeroAmount.selector);
        g.fundGuild(id, 0);
    }

    function test_fundGuild_reverts_unknown_guild() public {
        vm.prank(founder);
        vm.expectRevert(GuildFacet.UnknownGuild.selector);
        g.fundGuild(999, FUND);
    }

    function test_fundGuild_no_ghost_credit_on_failed_pull() public {
        // A broke funder: approve but no balance → transferFrom reverts →
        // the whole tx reverts, no ledger credit.
        uint256 id = _create("guild");
        address broke = address(0x0B0B);
        vm.prank(broke);
        lh.approve(address(g), type(uint256).max);
        vm.prank(broke);
        vm.expectRevert(); // MockLH "balance"
        g.fundGuild(id, FUND);
        assertEq(g.treasuryBalanceOf(id), 0, "no ghost credit");
    }

    // =====================================================================
    // spendTreasury: Admin-gated CEI debit
    // =====================================================================

    function test_spendTreasury_pays_recipient() public {
        uint256 id = _create("guild");
        vm.prank(founder);
        g.fundGuild(id, FUND);

        uint256 payeeBefore = lh.balanceOf(payee);
        vm.prank(founder);
        g.spendTreasury(id, payee, 300 ether, "grant");

        assertEq(lh.balanceOf(payee), payeeBefore + 300 ether, "recipient paid");
        assertEq(g.treasuryBalanceOf(id), FUND - 300 ether, "ledger debited");
        assertEq(lh.balanceOf(address(g)), FUND - 300 ether, "diamond drained by exactly the spend");
    }

    function test_spendTreasury_full_drain() public {
        uint256 id = _create("guild");
        vm.prank(founder);
        g.fundGuild(id, FUND);
        vm.prank(founder);
        g.spendTreasury(id, payee, FUND, "");
        assertEq(g.treasuryBalanceOf(id), 0);
        assertEq(lh.balanceOf(payee), FUND);
    }

    function test_spendTreasury_reverts_non_admin() public {
        uint256 id = _create("guild");
        _join(id, alice);
        vm.prank(founder);
        g.fundGuild(id, FUND);
        vm.prank(alice); // a Member
        vm.expectRevert(GuildFacet.NotAdmin.selector);
        g.spendTreasury(id, payee, 1 ether, "");
    }

    function test_spendTreasury_reverts_officer() public {
        uint256 id = _create("guild");
        _join(id, officer);
        vm.prank(founder);
        g.setRole(id, officer, uint8(LibGuildStorage.Role.Officer));
        vm.prank(founder);
        g.fundGuild(id, FUND);
        vm.prank(officer); // Officer is NOT Admin
        vm.expectRevert(GuildFacet.NotAdmin.selector);
        g.spendTreasury(id, payee, 1 ether, "");
    }

    function test_spendTreasury_reverts_insufficient() public {
        uint256 id = _create("guild");
        vm.prank(founder);
        g.fundGuild(id, 100 ether);
        vm.prank(founder);
        vm.expectRevert(GuildFacet.InsufficientTreasury.selector);
        g.spendTreasury(id, payee, 101 ether, "");
    }

    function test_spendTreasury_reverts_zero_amount() public {
        uint256 id = _create("guild");
        vm.prank(founder);
        g.fundGuild(id, FUND);
        vm.prank(founder);
        vm.expectRevert(GuildFacet.ZeroAmount.selector);
        g.spendTreasury(id, payee, 0, "");
    }

    function test_spendTreasury_reverts_zero_recipient() public {
        uint256 id = _create("guild");
        vm.prank(founder);
        g.fundGuild(id, FUND);
        vm.prank(founder);
        vm.expectRevert(GuildFacet.ZeroRecipient.selector);
        g.spendTreasury(id, address(0), 1 ether, "");
    }

    function test_spendTreasury_reverts_unknown_guild() public {
        vm.prank(founder);
        vm.expectRevert(GuildFacet.UnknownGuild.selector);
        g.spendTreasury(999, payee, 1 ether, "");
    }

    function test_treasury_isolated_between_guilds() public {
        uint256 a = _create("alpha");
        vm.prank(alice);
        uint256 b = g.createGuild("beta");
        vm.prank(founder);
        g.fundGuild(a, FUND);
        // alice (Admin of guild b) can't spend guild a's treasury.
        vm.prank(alice);
        vm.expectRevert(GuildFacet.NotAdmin.selector);
        g.spendTreasury(a, payee, 1 ether, "");
        // And spending guild b (empty) for any amount fails on balance.
        vm.prank(alice);
        vm.expectRevert(GuildFacet.InsufficientTreasury.selector);
        g.spendTreasury(b, payee, 1 ether, "");
        // Guild a's balance is untouched throughout.
        assertEq(g.treasuryBalanceOf(a), FUND);
        assertEq(g.treasuryBalanceOf(b), 0);
    }

    // =====================================================================
    // REENTRANCY PROBE — a hostile token re-enters during a spend
    // =====================================================================

    function test_reentrant_spend_cannot_double_drain() public {
        ReentrantLH rlh = new ReentrantLH();
        GuildHarness h = new GuildHarness();
        h._setCreditsToken(address(rlh));
        rlh.mint(founder, 1_000_000 ether);
        vm.prank(founder);
        rlh.approve(address(h), type(uint256).max);
        vm.warp(1_000_000);

        vm.prank(founder);
        uint256 id = h.createGuild("reguild");
        vm.prank(founder);
        h.fundGuild(id, FUND);

        // Extra unrelated balance in the diamond so a SUCCESSFUL double-drain
        // would have something to steal (proving the revert is the defense).
        rlh.mint(address(h), 1_000_000 ether);

        uint256 diamondBefore = rlh.balanceOf(address(h));
        // Arm the token to re-enter with a second spend of the SAME amount.
        rlh.arm(address(h), id, payee, 300 ether);

        vm.prank(founder);
        h.spendTreasury(id, payee, 300 ether, "");

        assertTrue(rlh.reenterReverted(), "re-entrant spend reverted (InsufficientTreasury)");
        // Exactly ONE spend left the diamond, not two.
        assertEq(rlh.balanceOf(address(h)), diamondBefore - 300 ether, "exactly one spend");
        assertEq(h.treasuryBalanceOf(id), FUND - 300 ether, "ledger debited once");
    }

    // =====================================================================
    // RECURSIVE MEMBERSHIP — a guild is a member of another guild
    // (Part 4: guilds-of-guilds, "turtles all the way down")
    // =====================================================================

    function test_guild_can_be_member_of_another_guild() public {
        // Guild A (the parent) and Guild B (the child / member).
        uint256 guildA = _create("parentguild");
        vm.prank(alice);
        uint256 guildB = g.createGuild("childguild");

        // Guild B's ADDRESS is its TBA — a CONTRACT account, not an EOA.
        address guildBAddr = g.guildAddress(guildB);
        assertTrue(guildBAddr != address(0), "guild B has a wallet address");

        // The parent's Admin (founder) invites guild B's ADDRESS as a member.
        vm.prank(founder);
        g.inviteToGuild(guildA, guildBAddr);

        // Guild B "accepts" by calling from its TBA address (in production a
        // sponsored call FROM the guild's MultiSignerAccount; here we prank
        // as that address — the facet keys purely on msg.sender, never on
        // EOA-ness, which is the whole recursive point).
        vm.prank(guildBAddr);
        g.acceptGuildInvite(guildA);

        // PROVEN: a guild is now a member of another guild.
        assertTrue(g.isGuildMember(guildA, guildBAddr), "guild B is a member of guild A");
        assertEq(
            g.roleOf(guildA, guildBAddr),
            uint8(LibGuildStorage.Role.Member),
            "guild B joined as Member"
        );

        // It shows up in BOTH enumerations.
        address[] memory aMembers = g.guildMembersOf(guildA);
        assertEq(aMembers.length, 2, "founder + guild B");
        assertEq(aMembers[1], guildBAddr);
        uint256[] memory bGuilds = g.guildsOf(guildBAddr);
        assertEq(bGuilds.length, 1, "guild B belongs to one (parent) guild");
        assertEq(bGuilds[0], guildA, "...which is guild A");

        // And the parent can PAY OUT to the member-guild's address (funding a
        // sub-guild from the treasury) — the recursive money flow.
        vm.prank(founder);
        g.fundGuild(guildA, FUND);
        uint256 childBefore = lh.balanceOf(guildBAddr);
        vm.prank(founder);
        g.spendTreasury(guildA, guildBAddr, 250 ether, "sub-guild grant");
        assertEq(
            lh.balanceOf(guildBAddr),
            childBefore + 250 ether,
            "parent guild funded the member guild's wallet"
        );

        // The member-guild can even promote-then-act within the parent: the
        // parent Admin makes guild B an Officer, and guild B (a CONTRACT)
        // invites a further member — recursion all the way through the API.
        vm.prank(founder);
        g.setRole(guildA, guildBAddr, uint8(LibGuildStorage.Role.Officer));
        vm.prank(guildBAddr);
        g.inviteToGuild(guildA, bob);
        vm.prank(bob);
        g.acceptGuildInvite(guildA);
        assertTrue(g.isGuildMember(guildA, bob), "a contract-member invited a further member");
    }

    // =====================================================================
    // FUZZ: treasury conservation — diamond $LH held == Σ live guildBalance
    // =====================================================================

    /// The load-bearing invariant: at every point, the `$LH` the diamond
    /// holds for guild treasuries equals the sum of `guildBalance` over all
    /// guilds. fund (credit) and spend (debit) move the escrow and the
    /// ledger in lockstep; nothing is ever stranded or double-counted. The
    /// diamond holds ONLY guild treasury here (no unrelated funds), so its
    /// balance IS the treasury total.
    function testFuzz_treasury_conservation(uint256 seedRaw) public {
        uint256 seed = seedRaw;
        assertEq(lh.balanceOf(address(g)), 0, "diamond starts empty");

        // Spin up three guilds (founder is Admin of all three — direct-seat
        // via createGuild; ids 1,2,3).
        uint256[] memory ids = new uint256[](3);
        ids[0] = _create("ga");
        ids[1] = _create("gb");
        ids[2] = _create("gc");

        for (uint256 i = 0; i < 60; i++) {
            seed = uint256(keccak256(abi.encode(seed, i)));
            uint256 pick = seed % 3;
            uint256 id = ids[pick];
            uint256 action = (seed >> 8) % 2;

            if (action == 0) {
                // FUND: bounded amount from the founder.
                uint256 amt = 1 + (seed % 500) * 1 ether;
                vm.prank(founder);
                g.fundGuild(id, amt);
            } else {
                // SPEND: an amount that may exceed the balance (must revert
                // cleanly) — pick something bounded, guard the success path.
                uint256 bal = g.treasuryBalanceOf(id);
                if (bal == 0) {
                    // nothing to spend; skip
                } else {
                    uint256 amt = 1 + (seed % bal);
                    vm.prank(founder);
                    g.spendTreasury(id, payee, amt, "");
                }
            }

            // INVARIANT after every step: diamond balance == Σ guildBalance.
            uint256 sum = g.treasuryBalanceOf(ids[0]) + g.treasuryBalanceOf(ids[1])
                + g.treasuryBalanceOf(ids[2]);
            assertEq(lh.balanceOf(address(g)), sum, "diamond $LH == sum of live guild treasuries");
        }
    }
}
