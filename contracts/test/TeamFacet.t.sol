// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {TeamFacet} from "../src/facets/TeamFacet.sol";

/// Pins the CURRENT LIVE behavior of TeamFacet — the facet is cut + live on
/// the canonical diamond with ZERO prior Foundry coverage. These tests are a
/// CHARACTERIZATION suite: they assert what the deployed facet DOES,
/// including three known quirks (each named + pinned as-is below for
/// fidelity with the deployed bytecode — do NOT "fix" them here without a
/// deliberate source change + re-cut):
///
///   (a) ORPHANED-TEAM INVITES: a pending invite survives ALL members
///       leaving — `accept()` still seats the invitee into the empty team.
///   (b) PHANTOM LEAVE: `leave()` by a never-member silently succeeds
///       (`_removeMember` early-returns on index 0) and still emits a
///       misleading `Left` event.
///   (c) UNBOUNDED MEMBERSHIP: no MAX_MEMBERS cap (unlike GuildFacet's 128)
///       — the member list can grow without bound.
///
/// TeamFacet's storage (`LibTeamStorage`) is fully self-contained — no
/// cross-facet slots (credits token, registry) to arrange — so unlike the
/// GuildFacet harness no storage setters are needed: a bare facet instance
/// behaves exactly like the diamond-routed one.
contract TeamFacetTest is Test {
    TeamFacet t;

    address creator = address(0xC0FFEE); // creates teams; first member
    address alice = address(0xA11CE); // an invitee / member
    address bob = address(0xB0B); // another invitee / member
    address carol = address(0xCA801); // a third member (swap-pop probes)
    address stranger = address(0xBEEF); // never a member

    function setUp() public {
        t = new TeamFacet();
    }

    // --- helpers ---------------------------------------------------------

    function _team(string memory name) internal returns (uint256 id) {
        vm.prank(creator);
        id = t.createTeam(name);
    }

    /// creator invites + the agent accepts → joined.
    function _join(uint256 id, address agent) internal {
        vm.prank(creator);
        t.invite(id, agent);
        vm.prank(agent);
        t.accept(id);
    }

    // =====================================================================
    // createTeam
    // =====================================================================

    function test_createTeam_creator_is_first_member() public {
        uint256 id = _team("alpha");

        assertEq(id, 1, "team ids start at 1");
        assertEq(t.nextTeamId(), 1, "counter advanced");
        assertEq(t.teamName(id), "alpha", "name stored");
        assertTrue(t.isMember(id, creator), "creator seated");

        address[] memory m = t.membersOf(id);
        assertEq(m.length, 1);
        assertEq(m[0], creator);

        uint256[] memory ts = t.teamsOf(creator);
        assertEq(ts.length, 1);
        assertEq(ts[0], id);
    }

    function test_createTeam_ids_increment() public {
        uint256 a = _team("alpha");
        vm.prank(alice);
        uint256 b = t.createTeam("beta");
        assertEq(a, 1);
        assertEq(b, 2);
        assertEq(t.nextTeamId(), 2);
        assertEq(t.teamName(b), "beta");
        assertEq(t.membersOf(b).length, 1);
    }

    function test_createTeam_emits_TeamCreated() public {
        vm.expectEmit(true, true, false, true, address(t));
        emit TeamFacet.TeamCreated(1, creator, "alpha");
        vm.prank(creator);
        t.createTeam("alpha");
    }

    /// PINNED: team names are NOT deduplicated (unlike registry names) —
    /// two teams may share a name; identity is the id, not the name.
    function test_createTeam_duplicate_names_allowed() public {
        uint256 a = _team("same");
        uint256 b = _team("same");
        assertTrue(a != b);
        assertEq(t.teamName(a), "same");
        assertEq(t.teamName(b), "same");
    }

    // =====================================================================
    // invite / accept / decline — the mutual-consent lifecycle
    // =====================================================================

    function test_invite_and_accept_makes_member() public {
        uint256 id = _team("alpha");

        vm.expectEmit(true, true, true, true, address(t));
        emit TeamFacet.Invited(id, alice, creator);
        vm.prank(creator);
        t.invite(id, alice);

        assertTrue(t.isInvited(id, alice), "invite pending");
        assertFalse(t.isMember(id, alice), "invited != member until accept");

        vm.expectEmit(true, true, false, true, address(t));
        emit TeamFacet.Joined(id, alice);
        vm.prank(alice);
        t.accept(id);

        assertTrue(t.isMember(id, alice), "member after accept");
        assertFalse(t.isInvited(id, alice), "invite consumed");
        assertEq(t.membersOf(id).length, 2, "creator + alice");
        assertEq(t.teamsOf(alice).length, 1);
        assertEq(t.teamsOf(alice)[0], id);
    }

    function test_invite_reverts_non_member() public {
        uint256 id = _team("alpha");
        vm.prank(stranger);
        vm.expectRevert(TeamFacet.NotMember.selector);
        t.invite(id, alice);
    }

    /// Any member (not just the creator) may invite — there are no roles.
    function test_any_member_can_invite() public {
        uint256 id = _team("alpha");
        _join(id, alice);
        vm.prank(alice);
        t.invite(id, bob);
        vm.prank(bob);
        t.accept(id);
        assertTrue(t.isMember(id, bob));
    }

    function test_accept_reverts_not_invited() public {
        uint256 id = _team("alpha");
        vm.prank(stranger);
        vm.expectRevert(TeamFacet.NotInvited.selector);
        t.accept(id);
    }

    /// A nonexistent team is just a team nobody is a member of: invite
    /// reverts NotMember, accept reverts NotInvited (no UnknownTeam error
    /// exists on this facet — pinned).
    function test_nonexistent_team_invite_accept_revert() public {
        vm.prank(creator);
        vm.expectRevert(TeamFacet.NotMember.selector);
        t.invite(999, alice);

        vm.prank(alice);
        vm.expectRevert(TeamFacet.NotInvited.selector);
        t.accept(999);
    }

    function test_decline_clears_invite() public {
        uint256 id = _team("alpha");
        vm.prank(creator);
        t.invite(id, alice);
        assertTrue(t.isInvited(id, alice));

        vm.prank(alice);
        t.decline(id);
        assertFalse(t.isInvited(id, alice), "invite cleared");

        // A declined invite can no longer be accepted.
        vm.prank(alice);
        vm.expectRevert(TeamFacet.NotInvited.selector);
        t.accept(id);
    }

    /// PINNED: decline without a pending invite is a silent no-op (it just
    /// writes false over false; no revert, no event).
    function test_decline_without_invite_is_noop() public {
        uint256 id = _team("alpha");
        vm.prank(stranger);
        t.decline(id); // does not revert
        assertFalse(t.isInvited(id, stranger));
    }

    /// PINNED: accept by an EXISTING member (re-invited) consumes the invite
    /// but `_addMember` is idempotent — no duplicate list entry. (The Joined
    /// event still fires again; only the membership structures are guarded.)
    function test_accept_idempotent_for_existing_member() public {
        uint256 id = _team("alpha");
        _join(id, alice);
        assertEq(t.membersOf(id).length, 2);

        vm.prank(creator);
        t.invite(id, alice); // invite does NOT check AlreadyMember
        vm.prank(alice);
        t.accept(id);

        assertEq(t.membersOf(id).length, 2, "no duplicate member entry");
        assertEq(t.teamsOf(alice).length, 1, "no duplicate teamsOf entry");
        assertFalse(t.isInvited(id, alice), "invite consumed");
    }

    // =====================================================================
    // leave + swap-pop consistency
    // =====================================================================

    function test_leave_removes_member() public {
        uint256 id = _team("alpha");
        _join(id, alice);

        vm.expectEmit(true, true, false, true, address(t));
        emit TeamFacet.Left(id, alice);
        vm.prank(alice);
        t.leave(id);

        assertFalse(t.isMember(id, alice));
        assertEq(t.membersOf(id).length, 1, "back to just the creator");
        assertEq(t.membersOf(id)[0], creator);
        assertEq(t.teamsOf(alice).length, 0, "teamsOf cleared");
    }

    /// Swap-pop consistency of membersOf after a MID-LIST removal: with
    /// [creator, alice, bob, carol], alice (index 1) leaving must move carol
    /// (the last) into her slot — and every remaining member must still be
    /// removable (i.e. the moved member's index was rewritten correctly).
    function test_membersOf_swap_pop_mid_list_removal() public {
        uint256 id = _team("alpha");
        _join(id, alice);
        _join(id, bob);
        _join(id, carol);
        assertEq(t.membersOf(id).length, 4);

        vm.prank(alice); // mid-list (index 1 of 0..3)
        t.leave(id);

        address[] memory m = t.membersOf(id);
        assertEq(m.length, 3);
        assertEq(m[0], creator, "head untouched");
        assertEq(m[1], carol, "last member swap-popped into the hole");
        assertEq(m[2], bob, "tail shrunk");
        assertFalse(t.isMember(id, alice));

        // The MOVED member's index must have been rewritten: carol can still
        // leave cleanly (a stale index would corrupt the list / underflow).
        vm.prank(carol);
        t.leave(id);
        m = t.membersOf(id);
        assertEq(m.length, 2);
        assertEq(m[0], creator);
        assertEq(m[1], bob);
        assertFalse(t.isMember(id, carol));
        assertTrue(t.isMember(id, bob), "bob unaffected throughout");
    }

    /// Swap-pop consistency of teamsOf: an agent in three teams leaving the
    /// MIDDLE one gets the last team swapped into its place.
    function test_teamsOf_swap_pop_mid_list_removal() public {
        uint256 t1 = _team("one");
        uint256 t2 = _team("two");
        uint256 t3 = _team("three");
        _join(t1, alice);
        _join(t2, alice);
        _join(t3, alice);
        assertEq(t.teamsOf(alice).length, 3);

        vm.prank(alice);
        t.leave(t2); // the middle entry

        uint256[] memory ts = t.teamsOf(alice);
        assertEq(ts.length, 2);
        assertEq(ts[0], t1);
        assertEq(ts[1], t3, "last team swapped into the middle slot");
        assertTrue(t.isMember(t1, alice));
        assertFalse(t.isMember(t2, alice));
        assertTrue(t.isMember(t3, alice));
    }

    /// A member who left can be re-invited and re-join cleanly.
    function test_leave_then_rejoin() public {
        uint256 id = _team("alpha");
        _join(id, alice);
        vm.prank(alice);
        t.leave(id);
        _join(id, alice);
        assertTrue(t.isMember(id, alice));
        assertEq(t.membersOf(id).length, 2);
        assertEq(t.teamsOf(alice).length, 1);
    }

    // =====================================================================
    // KNOWN QUIRKS — pinned as-is for fidelity with the DEPLOYED facet.
    // Changing any of these requires a deliberate source change + re-cut;
    // these tests exist so such a change is loud, not accidental.
    // =====================================================================

    /// KNOWN QUIRK (a): a pending invite SURVIVES all members leaving.
    /// `leave` never clears outstanding invites, and `accept` checks only
    /// `invited[teamId][msg.sender]` — so the invitee is seated into an
    /// otherwise-orphaned (zero-member) team as its sole member.
    function test_quirk_pending_invite_survives_orphaned_team() public {
        uint256 id = _team("ghost");
        vm.prank(creator);
        t.invite(id, alice);

        // The ONLY member leaves — the team is now empty (orphaned).
        vm.prank(creator);
        t.leave(id);
        assertEq(t.membersOf(id).length, 0, "team fully orphaned");

        // The stale invite still works: alice is seated into the husk.
        vm.prank(alice);
        t.accept(id);
        assertTrue(t.isMember(id, alice), "invitee seated into the orphaned team");
        assertEq(t.membersOf(id).length, 1);
        assertEq(t.membersOf(id)[0], alice);
    }

    /// KNOWN QUIRK (b): `leave()` by a NEVER-member silently succeeds —
    /// `_removeMember` early-returns on memberIndex 0 — and the function
    /// still emits a misleading `Left(teamId, agent)` event for an agent who
    /// was never in the team. Event consumers cannot trust Left alone.
    function test_quirk_leave_by_never_member_succeeds_and_emits_Left() public {
        uint256 id = _team("alpha");

        vm.expectEmit(true, true, false, true, address(t));
        emit TeamFacet.Left(id, stranger); // the misleading event
        vm.prank(stranger);
        t.leave(id); // no revert

        // State is untouched (the no-op half of the quirk).
        assertEq(t.membersOf(id).length, 1);
        assertEq(t.membersOf(id)[0], creator);
        assertFalse(t.isMember(id, stranger));
    }

    /// KNOWN QUIRK (c): membership is UNBOUNDED — no MAX_MEMBERS cap (unlike
    /// GuildFacet's 128). Seat 160 members (> 128) to pin that no cap
    /// exists; the enumeration just grows.
    function test_quirk_membership_unbounded_no_cap() public {
        uint256 id = _team("horde");
        // 160 joiners on top of the creator — comfortably past GuildFacet's
        // 128 cap, where a capped facet would revert.
        for (uint256 i = 0; i < 160; i++) {
            address m = address(uint160(0x20000 + i));
            vm.prank(creator);
            t.invite(id, m);
            vm.prank(m);
            t.accept(id);
        }
        assertEq(t.membersOf(id).length, 161, "no cap: 1 creator + 160 joiners");
        assertTrue(t.isMember(id, address(uint160(0x20000 + 159))));
    }

    // =====================================================================
    // full lifecycle smoke — create → invite → accept → decline → leave
    // =====================================================================

    function test_full_lifecycle() public {
        uint256 id = _team("life");

        // invite two, one accepts, one declines.
        vm.prank(creator);
        t.invite(id, alice);
        vm.prank(creator);
        t.invite(id, bob);
        vm.prank(alice);
        t.accept(id);
        vm.prank(bob);
        t.decline(id);

        assertTrue(t.isMember(id, alice));
        assertFalse(t.isMember(id, bob));
        assertFalse(t.isInvited(id, bob));
        assertEq(t.membersOf(id).length, 2);

        // the member invites a third; they accept; the creator leaves.
        vm.prank(alice);
        t.invite(id, carol);
        vm.prank(carol);
        t.accept(id);
        vm.prank(creator);
        t.leave(id);

        address[] memory m = t.membersOf(id);
        assertEq(m.length, 2, "alice + carol remain");
        assertFalse(t.isMember(id, creator));
        assertEq(t.teamsOf(creator).length, 0);
        assertTrue(t.isMember(id, alice));
        assertTrue(t.isMember(id, carol));
        assertEq(t.teamName(id), "life", "team persists past the creator");
    }
}
