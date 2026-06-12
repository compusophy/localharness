// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {PartyFacet} from "../src/facets/PartyFacet.sol";
import {LibPartyStorage} from "../src/libraries/LibPartyStorage.sol";
import {LibCreditsStorage} from "../src/libraries/LibCreditsStorage.sol";
import {LibRegistryStorage} from "../src/libraries/LibRegistryStorage.sol";

/// Minimal `$LH`-shaped TIP-20 mock: 18-decimal balances + the
/// approve/transferFrom/transfer surface PartyFacet escrows + splits +
/// refunds through. Reverts (via require) on an under-allowance /
/// under-balance pull so the facet's CEI ordering is provable (a failed
/// escrow leaves no ghost contribution).
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

/// Hostile reentrant TIP-20 mock: on `transfer` (the split/refund path —
/// the only external calls in complete/disband) it re-enters the diamond,
/// trying a SECOND settlement of the same party. Real `$LH` has NO
/// callback; this is the defense-in-depth probe that CEI ordering makes a
/// double-split / double-refund structurally impossible (the re-entry
/// re-reads a terminal status and reverts).
contract ReentrantLH {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    address public diamond;
    uint256 public attackId;
    uint8 public mode; // 0=completeParty, 1=disbandParty
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
        // same party a second time. CEI means the status is already
        // terminal, so this MUST revert (no double drain).
        if (diamond != address(0) && !entered) {
            entered = true;
            if (mode == 0) {
                try PartyFacet(diamond).completeParty(attackId) {
                    reenterReverted = false;
                } catch {
                    reenterReverted = true;
                }
            } else {
                try PartyFacet(diamond).disbandParty(attackId) {
                    reenterReverted = false;
                } catch {
                    reenterReverted = true;
                }
            }
        }
        return true;
    }
}

/// Test harness: PartyFacet + setters that write the SHARED diamond-storage
/// slots a real diamond populates via other facets (creditsToken from
/// CreditsFacet, ownerOfId from the registry) AND a `tokenBoundAccount`
/// implementation so the completeParty self-call resolves each member's
/// payout wallet. Because every `Lib*Storage.load()` resolves against THIS
/// contract's storage, writing them here IS the cross-facet storage sharing
/// the diamond provides. The diamond IS the escrow holder, so
/// `address(this)` holds the pooled `$LH`, exactly like the live diamond.
contract PartyHarness is PartyFacet {
    mapping(uint256 => address) internal _tba; // tokenId -> TBA; 0 = unresolved

    function _setCreditsToken(address token) external {
        LibCreditsStorage.load().creditsToken = token;
    }

    function _registerIdentity(uint256 id, address owner) external {
        LibRegistryStorage.load().ownerOfId[id] = owner;
    }

    function _setTba(uint256 tokenId, address tba) external {
        _tba[tokenId] = tba;
    }

    /// Satisfies ITbaResolver — the selector completeParty self-calls.
    function tokenBoundAccount(uint256 tokenId) external view returns (address) {
        return _tba[tokenId];
    }
}

contract PartyFacetTest is Test {
    PartyHarness pf;
    MockLH lh;

    address creator = address(0xF00D); // forms + settles the party
    address aliceEoa = address(0xA11CE); // owns ALICE_ID
    address bobEoa = address(0xB0B); // owns BOB_ID
    address funder = address(0xFEED); // a third-party funder
    address stranger = address(0xBEEF); // owns nothing, pokes things

    address aliceTba = address(0x7BA1);
    address bobTba = address(0x7BA2);
    address creatorTba = address(0x7BA3);

    uint256 constant ALICE_ID = 7;
    uint256 constant BOB_ID = 8;
    uint256 constant CREATOR_ID = 9; // an identity the CREATOR owns

    uint64 constant TTL = 24 hours;
    uint128 constant POT = 100 ether; // 100 $LH

    function setUp() public {
        pf = new PartyHarness();
        lh = new MockLH();
        pf._setCreditsToken(address(lh));

        pf._registerIdentity(ALICE_ID, aliceEoa);
        pf._registerIdentity(BOB_ID, bobEoa);
        pf._registerIdentity(CREATOR_ID, creator);
        pf._setTba(ALICE_ID, aliceTba);
        pf._setTba(BOB_ID, bobTba);
        pf._setTba(CREATOR_ID, creatorTba);

        // Fund + pre-approve the would-be funders.
        lh.mint(creator, 1_000_000 ether);
        lh.mint(funder, 1_000_000 ether);
        vm.prank(creator);
        lh.approve(address(pf), type(uint256).max);
        vm.prank(funder);
        lh.approve(address(pf), type(uint256).max);

        vm.warp(1_000_000); // deterministic expiry math
    }

    // --- helpers ---------------------------------------------------------

    function _ids2() internal pure returns (uint256[] memory m) {
        m = new uint256[](2);
        m[0] = ALICE_ID;
        m[1] = BOB_ID;
    }

    function _bps2(uint16 a, uint16 b) internal pure returns (uint16[] memory s) {
        s = new uint16[](2);
        s[0] = a;
        s[1] = b;
    }

    /// Form the canonical 2-member (alice 60% / bob 40%) party as `creator`.
    function _form() internal returns (uint256 id) {
        vm.prank(creator);
        id = pf.formParty(_ids2(), _bps2(6000, 4000), TTL);
    }

    /// Form + both members consent → Active.
    function _formActive() internal returns (uint256 id) {
        id = _form();
        vm.prank(aliceEoa);
        pf.joinParty(id);
        vm.prank(bobEoa);
        pf.joinParty(id);
    }

    function _status(uint256 id) internal view returns (uint8 st) {
        (,, st,,,) = pf.getParty(id);
    }

    function _escrow(uint256 id) internal view returns (uint128 e) {
        (,,, e,,) = pf.getParty(id);
    }

    // =====================================================================
    // formParty: proposal + validation + auto-consent
    // =====================================================================

    function test_formParty_stores_and_starts_forming() public {
        uint256 id = _form();
        assertEq(id, 1, "first party id is 1");

        (address c, uint64 exp, uint8 st, uint128 esc, uint256 n, uint256 acc) = pf.getParty(id);
        assertEq(c, creator, "creator recorded");
        assertEq(exp, uint64(block.timestamp) + TTL, "expiry = now + ttl");
        assertEq(st, uint8(LibPartyStorage.Status.Forming), "status Forming");
        assertEq(esc, 0, "no escrow yet");
        assertEq(n, 2, "two members");
        assertEq(acc, 0, "no consent yet (creator owns neither seat)");

        uint256[] memory mem = pf.partyMembersOf(id);
        assertEq(mem.length, 2);
        assertEq(mem[0], ALICE_ID);
        assertEq(mem[1], BOB_ID);
        uint16[] memory bps = pf.partySharesOf(id);
        assertEq(bps[0], 6000);
        assertEq(bps[1], 4000);
        assertFalse(pf.partyConsentOf(id, ALICE_ID));
        assertFalse(pf.partyConsentOf(id, BOB_ID));

        assertEq(pf.partyCount(), 1);
        assertEq(pf.activePartyCountOf(creator), 1, "active count bumped");
        uint256[] memory mine = pf.partiesOf(creator);
        assertEq(mine.length, 1);
        assertEq(mine[0], id);
    }

    function test_formParty_autoconsents_creator_owned_seats() public {
        // alice + the creator's own identity: the creator's seat consents
        // at formParty (forming is consenting), alice's does not.
        uint256[] memory m = new uint256[](2);
        m[0] = ALICE_ID;
        m[1] = CREATOR_ID;
        vm.prank(creator);
        uint256 id = pf.formParty(m, _bps2(5000, 5000), TTL);

        (,, uint8 st,,, uint256 acc) = pf.getParty(id);
        assertEq(st, uint8(LibPartyStorage.Status.Forming), "still Forming (alice pending)");
        assertEq(acc, 1, "creator's seat auto-consented");
        assertTrue(pf.partyConsentOf(id, CREATOR_ID));
        assertFalse(pf.partyConsentOf(id, ALICE_ID));
    }

    function test_formParty_all_creator_seats_goes_straight_active() public {
        uint256[] memory m = new uint256[](1);
        m[0] = CREATOR_ID;
        uint16[] memory s = new uint16[](1);
        s[0] = 10_000;
        vm.prank(creator);
        uint256 id = pf.formParty(m, s, TTL);
        assertEq(_status(id), uint8(LibPartyStorage.Status.Active), "fully consented at form");
    }

    function test_formParty_reverts_empty_members() public {
        vm.prank(creator);
        vm.expectRevert(PartyFacet.BadMembers.selector);
        pf.formParty(new uint256[](0), new uint16[](0), TTL);
    }

    function test_formParty_reverts_length_mismatch() public {
        vm.prank(creator);
        vm.expectRevert(PartyFacet.BadMembers.selector);
        pf.formParty(_ids2(), new uint16[](1), TTL);
    }

    function test_formParty_reverts_too_many_members() public {
        uint256 n = LibPartyStorage.MAX_PARTY_MEMBERS + 1;
        uint256[] memory m = new uint256[](n);
        uint16[] memory s = new uint16[](n);
        for (uint256 i = 0; i < n; i++) {
            m[i] = 100 + i;
            s[i] = 1;
        }
        vm.prank(creator);
        vm.expectRevert(PartyFacet.BadMembers.selector);
        pf.formParty(m, s, TTL);
    }

    function test_formParty_reverts_zero_share() public {
        vm.prank(creator);
        vm.expectRevert(PartyFacet.BadShares.selector);
        pf.formParty(_ids2(), _bps2(10_000, 0), TTL);
    }

    function test_formParty_reverts_sum_under_10000() public {
        vm.prank(creator);
        vm.expectRevert(PartyFacet.BadShares.selector);
        pf.formParty(_ids2(), _bps2(5000, 4999), TTL);
    }

    function test_formParty_reverts_sum_over_10000() public {
        vm.prank(creator);
        vm.expectRevert(PartyFacet.BadShares.selector);
        pf.formParty(_ids2(), _bps2(5000, 5001), TTL);
    }

    function test_formParty_reverts_duplicate_member() public {
        uint256[] memory m = new uint256[](2);
        m[0] = ALICE_ID;
        m[1] = ALICE_ID;
        vm.prank(creator);
        vm.expectRevert(PartyFacet.DuplicateMember.selector);
        pf.formParty(m, _bps2(5000, 5000), TTL);
    }

    function test_formParty_reverts_unregistered_member() public {
        uint256[] memory m = new uint256[](2);
        m[0] = ALICE_ID;
        m[1] = 4242; // never registered
        vm.prank(creator);
        vm.expectRevert(PartyFacet.UnknownMember.selector);
        pf.formParty(m, _bps2(5000, 5000), TTL);
    }

    function test_formParty_reverts_bad_ttl() public {
        vm.prank(creator);
        vm.expectRevert(PartyFacet.BadTtl.selector);
        pf.formParty(_ids2(), _bps2(5000, 5000), LibPartyStorage.MIN_TTL - 1);
        vm.prank(creator);
        vm.expectRevert(PartyFacet.BadTtl.selector);
        pf.formParty(_ids2(), _bps2(5000, 5000), LibPartyStorage.MAX_TTL + 1);
    }

    function test_formParty_accepts_ttl_bounds() public {
        vm.prank(creator);
        uint256 idMin = pf.formParty(_ids2(), _bps2(5000, 5000), LibPartyStorage.MIN_TTL);
        vm.prank(creator);
        uint256 idMax = pf.formParty(_ids2(), _bps2(5000, 5000), LibPartyStorage.MAX_TTL);
        (, uint64 expMin,,,,) = pf.getParty(idMin);
        (, uint64 expMax,,,,) = pf.getParty(idMax);
        assertEq(expMin, uint64(block.timestamp) + LibPartyStorage.MIN_TTL);
        assertEq(expMax, uint64(block.timestamp) + LibPartyStorage.MAX_TTL);
    }

    function test_formParty_reverts_at_active_cap() public {
        for (uint256 i = 0; i < LibPartyStorage.MAX_ACTIVE_PER_CREATOR; i++) {
            _form();
        }
        assertEq(pf.activePartyCountOf(creator), LibPartyStorage.MAX_ACTIVE_PER_CREATOR);
        vm.prank(creator);
        vm.expectRevert(PartyFacet.TooManyActiveParties.selector);
        pf.formParty(_ids2(), _bps2(5000, 5000), TTL);
    }

    function test_active_cap_frees_on_terminal_exit() public {
        uint256 first = _form();
        for (uint256 i = 1; i < LibPartyStorage.MAX_ACTIVE_PER_CREATOR; i++) {
            _form();
        }
        vm.prank(creator);
        vm.expectRevert(PartyFacet.TooManyActiveParties.selector);
        pf.formParty(_ids2(), _bps2(5000, 5000), TTL);

        // Disband one → a slot frees.
        vm.prank(creator);
        pf.disbandParty(first);
        assertEq(pf.activePartyCountOf(creator), LibPartyStorage.MAX_ACTIVE_PER_CREATOR - 1);
        uint256 fresh = _form();
        assertGt(fresh, 0, "form allowed after a slot frees");
    }

    // =====================================================================
    // joinParty: the consent gate
    // =====================================================================

    function test_joinParty_consents_only_callers_seats() public {
        uint256 id = _form();
        vm.prank(aliceEoa);
        pf.joinParty(id);

        assertTrue(pf.partyConsentOf(id, ALICE_ID), "alice's seat consented");
        assertFalse(pf.partyConsentOf(id, BOB_ID), "bob's seat untouched");
        (,, uint8 st,,, uint256 acc) = pf.getParty(id);
        assertEq(acc, 1);
        assertEq(st, uint8(LibPartyStorage.Status.Forming), "not Active until everyone consents");
    }

    function test_joinParty_full_consent_activates() public {
        uint256 id = _form();
        vm.prank(aliceEoa);
        pf.joinParty(id);
        vm.prank(bobEoa);
        pf.joinParty(id);
        assertEq(_status(id), uint8(LibPartyStorage.Status.Active), "everyone consented -> Active");
        (,,,,, uint256 acc) = pf.getParty(id);
        assertEq(acc, 2);
    }

    function test_joinParty_multi_seat_owner_consents_all_at_once() public {
        // bob owns BOTH listed identities → one joinParty consents both.
        uint256 secondBobId = 11;
        pf._registerIdentity(secondBobId, bobEoa);
        uint256[] memory m = new uint256[](2);
        m[0] = BOB_ID;
        m[1] = secondBobId;
        vm.prank(creator);
        uint256 id = pf.formParty(m, _bps2(7000, 3000), TTL);

        vm.prank(bobEoa);
        pf.joinParty(id);
        assertEq(_status(id), uint8(LibPartyStorage.Status.Active), "both seats consented in one call");
    }

    function test_joinParty_reverts_stranger() public {
        uint256 id = _form();
        vm.prank(stranger); // owns no listed identity
        vm.expectRevert(PartyFacet.NothingToConsent.selector);
        pf.joinParty(id);
    }

    function test_joinParty_reverts_double_join() public {
        uint256 id = _form();
        vm.prank(aliceEoa);
        pf.joinParty(id);
        vm.prank(aliceEoa);
        vm.expectRevert(PartyFacet.NothingToConsent.selector);
        pf.joinParty(id); // her only seat is already consented
    }

    function test_joinParty_reverts_unknown_party() public {
        vm.prank(aliceEoa);
        vm.expectRevert(PartyFacet.UnknownParty.selector);
        pf.joinParty(999);
    }

    function test_joinParty_reverts_after_expiry() public {
        uint256 id = _form();
        vm.warp(block.timestamp + TTL + 1);
        vm.prank(aliceEoa);
        vm.expectRevert(PartyFacet.Expired.selector);
        pf.joinParty(id);
    }

    function test_joinParty_at_exact_expiry_still_ok() public {
        uint256 id = _form();
        (, uint64 exp,,,,) = pf.getParty(id);
        vm.warp(exp); // now == expiry is still consentable (now <= expiry)
        vm.prank(aliceEoa);
        pf.joinParty(id);
        assertTrue(pf.partyConsentOf(id, ALICE_ID));
    }

    function test_joinParty_reverts_when_already_active() public {
        uint256 id = _formActive();
        vm.prank(aliceEoa);
        vm.expectRevert(PartyFacet.NotForming.selector);
        pf.joinParty(id);
    }

    // =====================================================================
    // fundParty: pot escrow
    // =====================================================================

    function test_fundParty_escrows_and_ledgers() public {
        uint256 id = _formActive();
        uint256 funderBefore = lh.balanceOf(funder);
        vm.prank(funder);
        pf.fundParty(id, POT);

        assertEq(lh.balanceOf(funder), funderBefore - POT, "pot pulled from funder");
        assertEq(lh.balanceOf(address(pf)), POT, "diamond holds the escrow");
        assertEq(_escrow(id), POT, "escrow ledgered");
        assertEq(pf.partyContributionOf(id, funder), POT, "contribution recorded");
        address[] memory fs = pf.partyFundersOf(id);
        assertEq(fs.length, 1);
        assertEq(fs[0], funder);
    }

    function test_fundParty_while_forming_is_allowed() public {
        uint256 id = _form(); // still Forming
        vm.prank(funder);
        pf.fundParty(id, POT);
        assertEq(_escrow(id), POT, "a proposal is fundable (refundable on disband)");
    }

    function test_fundParty_multiple_funders_and_repeat_contributions() public {
        uint256 id = _formActive();
        vm.prank(funder);
        pf.fundParty(id, 10 ether);
        vm.prank(creator);
        pf.fundParty(id, 5 ether);
        vm.prank(funder);
        pf.fundParty(id, 2 ether); // repeat: same slot, ledger grows

        assertEq(_escrow(id), 17 ether);
        assertEq(pf.partyContributionOf(id, funder), 12 ether);
        assertEq(pf.partyContributionOf(id, creator), 5 ether);
        assertEq(pf.partyFundersOf(id).length, 2, "repeat funder takes no second slot");
    }

    function test_fundParty_reverts_zero_amount() public {
        uint256 id = _formActive();
        vm.prank(funder);
        vm.expectRevert(PartyFacet.ZeroAmount.selector);
        pf.fundParty(id, 0);
    }

    function test_fundParty_reverts_unknown_party() public {
        vm.prank(funder);
        vm.expectRevert(PartyFacet.UnknownParty.selector);
        pf.fundParty(999, POT);
    }

    function test_fundParty_reverts_after_expiry() public {
        uint256 id = _formActive();
        vm.warp(block.timestamp + TTL + 1);
        vm.prank(funder);
        vm.expectRevert(PartyFacet.Expired.selector);
        pf.fundParty(id, POT);
    }

    function test_fundParty_reverts_on_terminal_party() public {
        uint256 id = _formActive();
        vm.prank(creator);
        pf.disbandParty(id); // → Disbanded
        vm.prank(funder);
        vm.expectRevert(PartyFacet.NotLive.selector);
        pf.fundParty(id, POT);
    }

    function test_fundParty_reverts_not_configured() public {
        PartyHarness fresh = new PartyHarness();
        fresh._registerIdentity(ALICE_ID, aliceEoa);
        fresh._registerIdentity(BOB_ID, bobEoa);
        vm.prank(creator);
        uint256 id = fresh.formParty(_ids2(), _bps2(5000, 5000), TTL);
        vm.prank(funder);
        vm.expectRevert(PartyFacet.NotConfigured.selector);
        fresh.fundParty(id, POT);
    }

    function test_fundParty_no_ghost_on_failed_pull() public {
        // A broke funder: approve but no balance → transferFrom reverts →
        // the whole tx reverts: no ghost ledger, no ghost funder slot.
        uint256 id = _formActive();
        address broke = address(0x0B0B5);
        vm.prank(broke);
        lh.approve(address(pf), type(uint256).max);
        vm.prank(broke);
        vm.expectRevert(); // MockLH "balance"
        pf.fundParty(id, POT);

        assertEq(_escrow(id), 0, "no ghost escrow");
        assertEq(pf.partyContributionOf(id, broke), 0, "no ghost contribution");
        assertEq(pf.partyFundersOf(id).length, 0, "no ghost funder slot");
    }

    function test_fundParty_reverts_at_funders_cap() public {
        uint256 id = _formActive();
        for (uint256 i = 0; i < LibPartyStorage.MAX_FUNDERS; i++) {
            address f = address(uint160(0xF0000 + i));
            lh.mint(f, 1 ether);
            vm.prank(f);
            lh.approve(address(pf), type(uint256).max);
            vm.prank(f);
            pf.fundParty(id, 1 ether);
        }
        assertEq(pf.partyFundersOf(id).length, LibPartyStorage.MAX_FUNDERS, "at cap");
        vm.prank(funder); // a NEW funder needs a slot → reverts
        vm.expectRevert(PartyFacet.TooManyFunders.selector);
        pf.fundParty(id, 1 ether);
        // … but an EXISTING funder can still top up (no new slot needed).
        address f0 = address(uint160(0xF0000));
        lh.mint(f0, 1 ether);
        vm.prank(f0);
        pf.fundParty(id, 1 ether);
    }

    // =====================================================================
    // completeParty: the split
    // =====================================================================

    function test_completeParty_splits_to_member_tbas_by_shares() public {
        uint256 id = _formActive();
        vm.prank(funder);
        pf.fundParty(id, POT); // 100 $LH; alice 60% / bob 40%

        vm.prank(creator);
        pf.completeParty(id);

        assertEq(lh.balanceOf(aliceTba), 60 ether, "alice's TBA got 60%");
        assertEq(lh.balanceOf(bobTba), 40 ether, "bob's TBA got 40%");
        assertEq(lh.balanceOf(aliceEoa), 0, "the owner EOA is NOT the payee");
        assertEq(lh.balanceOf(address(pf)), 0, "diamond drained the pot");
        assertEq(_status(id), uint8(LibPartyStorage.Status.Completed));
        assertEq(pf.activePartyCountOf(creator), 0, "active count decremented");
    }

    function test_completeParty_remainder_goes_to_last_member() public {
        // An odd pot that doesn't divide: 100 wei at 33.33%/66.67% →
        // alice floor(100*3333/10000)=33, bob takes the remainder 67.
        vm.prank(creator);
        uint256 id = pf.formParty(_ids2(), _bps2(3333, 6667), TTL);
        vm.prank(aliceEoa);
        pf.joinParty(id);
        vm.prank(bobEoa);
        pf.joinParty(id);
        vm.prank(funder);
        pf.fundParty(id, 100); // 100 wei

        vm.prank(creator);
        pf.completeParty(id);

        assertEq(lh.balanceOf(aliceTba), 33, "floor share");
        assertEq(lh.balanceOf(bobTba), 67, "last member takes the remainder");
        assertEq(lh.balanceOf(address(pf)), 0, "NOTHING stranded - split == escrow exactly");
    }

    function test_completeParty_only_creator() public {
        uint256 id = _formActive();
        vm.prank(funder);
        pf.fundParty(id, POT);
        vm.prank(aliceEoa);
        vm.expectRevert(PartyFacet.NotCreator.selector);
        pf.completeParty(id);
    }

    function test_completeParty_reverts_unknown() public {
        vm.prank(creator);
        vm.expectRevert(PartyFacet.UnknownParty.selector);
        pf.completeParty(999);
    }

    function test_completeParty_requires_full_consent() public {
        uint256 id = _form(); // Forming — bob/alice never consented
        vm.prank(funder);
        pf.fundParty(id, POT);
        vm.prank(creator);
        vm.expectRevert(PartyFacet.NotActive.selector);
        pf.completeParty(id); // no split without consent over the money
    }

    function test_completeParty_reverts_double_complete() public {
        uint256 id = _formActive();
        vm.prank(funder);
        pf.fundParty(id, POT);
        vm.prank(creator);
        pf.completeParty(id);
        vm.prank(creator);
        vm.expectRevert(PartyFacet.NotActive.selector);
        pf.completeParty(id); // can't split twice
    }

    function test_completeParty_reverts_after_expiry() public {
        // Past the deadline the refund window owns the escrow — the
        // complete/disband windows are disjoint (no settle-vs-refund race).
        uint256 id = _formActive();
        vm.prank(funder);
        pf.fundParty(id, POT);
        vm.warp(block.timestamp + TTL + 1);
        vm.prank(creator);
        vm.expectRevert(PartyFacet.Expired.selector);
        pf.completeParty(id);
    }

    function test_completeParty_zero_escrow_is_pure_dissolution() public {
        uint256 id = _formActive(); // never funded
        vm.prank(creator);
        pf.completeParty(id);
        assertEq(_status(id), uint8(LibPartyStorage.Status.Completed));
        assertEq(lh.balanceOf(aliceTba), 0);
        assertEq(lh.balanceOf(bobTba), 0);
    }

    function test_completeParty_reverts_when_a_tba_is_unresolved() public {
        // A member whose TBA resolves to address(0) — complete must refuse
        // (never burn a share). The party stays Active so the creator can
        // deploy the TBA and retry.
        uint256 noTbaId = 12;
        pf._registerIdentity(noTbaId, aliceEoa); // registered, but no _setTba
        uint256[] memory m = new uint256[](2);
        m[0] = noTbaId;
        m[1] = BOB_ID;
        vm.prank(creator);
        uint256 id = pf.formParty(m, _bps2(5000, 5000), TTL);
        vm.prank(aliceEoa);
        pf.joinParty(id);
        vm.prank(bobEoa);
        pf.joinParty(id);
        vm.prank(funder);
        pf.fundParty(id, POT);

        vm.prank(creator);
        vm.expectRevert(PartyFacet.TbaUnresolved.selector);
        pf.completeParty(id);
        // Unchanged: still Active, escrow intact, count intact.
        assertEq(_status(id), uint8(LibPartyStorage.Status.Active), "stays Active on a TBA revert");
        assertEq(lh.balanceOf(address(pf)), POT, "escrow untouched");
        assertEq(pf.activePartyCountOf(creator), 1, "active count untouched");
    }

    // =====================================================================
    // disbandParty: refund exit
    // =====================================================================

    function test_disbandParty_creator_refunds_every_funder_exactly() public {
        uint256 id = _formActive();
        vm.prank(funder);
        pf.fundParty(id, 30 ether);
        vm.prank(creator);
        pf.fundParty(id, 12 ether);
        uint256 funderBefore = lh.balanceOf(funder);
        uint256 creatorBefore = lh.balanceOf(creator);

        vm.prank(creator);
        pf.disbandParty(id);

        assertEq(lh.balanceOf(funder), funderBefore + 30 ether, "funder refunded exactly");
        assertEq(lh.balanceOf(creator), creatorBefore + 12 ether, "creator-funder refunded exactly");
        assertEq(lh.balanceOf(address(pf)), 0, "diamond drained - nothing stranded");
        assertEq(_status(id), uint8(LibPartyStorage.Status.Disbanded));
        assertEq(pf.activePartyCountOf(creator), 0, "active count decremented");
    }

    function test_disbandParty_creator_may_abort_while_forming() public {
        uint256 id = _form();
        vm.prank(funder);
        pf.fundParty(id, POT);
        uint256 before = lh.balanceOf(funder);
        vm.prank(creator);
        pf.disbandParty(id);
        assertEq(lh.balanceOf(funder), before + POT, "abort+reclaim (the MVP answer)");
    }

    function test_disbandParty_stranger_reverts_before_expiry() public {
        uint256 id = _formActive();
        vm.prank(stranger);
        vm.expectRevert(PartyFacet.NotDisbandable.selector);
        pf.disbandParty(id);
    }

    function test_disbandParty_stranger_reverts_at_exact_expiry() public {
        uint256 id = _formActive();
        (, uint64 exp,,,,) = pf.getParty(id);
        vm.warp(exp); // now == expiry: still the creator's window
        vm.prank(stranger);
        vm.expectRevert(PartyFacet.NotDisbandable.selector);
        pf.disbandParty(id);
    }

    function test_disbandParty_anyone_after_expiry_refunds_funders() public {
        uint256 id = _formActive();
        vm.prank(funder);
        pf.fundParty(id, POT);
        vm.warp(block.timestamp + TTL + 1);
        uint256 before = lh.balanceOf(funder);

        vm.prank(stranger); // permissionless poke
        pf.disbandParty(id);

        assertEq(lh.balanceOf(funder), before + POT, "the FUNDER is refunded");
        assertEq(lh.balanceOf(stranger), 0, "the poker gains nothing");
        assertEq(_status(id), uint8(LibPartyStorage.Status.Disbanded));
    }

    function test_disbandParty_reverts_unknown() public {
        vm.prank(creator);
        vm.expectRevert(PartyFacet.UnknownParty.selector);
        pf.disbandParty(999);
    }

    function test_disbandParty_reverts_double_disband() public {
        uint256 id = _formActive();
        vm.prank(creator);
        pf.disbandParty(id);
        vm.prank(creator);
        vm.expectRevert(PartyFacet.NotLive.selector);
        pf.disbandParty(id);
    }

    function test_disbandParty_reverts_after_complete() public {
        // A settled party can't ALSO be refunded (disjoint outcomes).
        uint256 id = _formActive();
        vm.prank(funder);
        pf.fundParty(id, POT);
        vm.prank(creator);
        pf.completeParty(id);
        vm.warp(block.timestamp + TTL + 1);
        vm.prank(stranger);
        vm.expectRevert(PartyFacet.NotLive.selector);
        pf.disbandParty(id);
    }

    // =====================================================================
    // REENTRANCY PROBES — a hostile token re-enters during settlement
    // =====================================================================

    function _reentrantHarness() internal returns (PartyHarness h, ReentrantLH rlh) {
        rlh = new ReentrantLH();
        h = new PartyHarness();
        h._setCreditsToken(address(rlh));
        h._registerIdentity(ALICE_ID, aliceEoa);
        h._registerIdentity(BOB_ID, bobEoa);
        h._setTba(ALICE_ID, aliceTba);
        h._setTba(BOB_ID, bobTba);
        rlh.mint(funder, 1_000_000 ether);
        vm.prank(funder);
        rlh.approve(address(h), type(uint256).max);
        // Extra balance in the diamond so a SUCCESSFUL double-drain would
        // have something to steal (proving the revert is what saves it).
        rlh.mint(address(h), 1_000_000 ether);
        vm.warp(1_000_000);
    }

    function test_reentrant_complete_cannot_double_split() public {
        (PartyHarness h, ReentrantLH rlh) = _reentrantHarness();
        vm.prank(creator);
        uint256 id = h.formParty(_ids2(), _bps2(6000, 4000), TTL);
        vm.prank(aliceEoa);
        h.joinParty(id);
        vm.prank(bobEoa);
        h.joinParty(id);
        vm.prank(funder);
        h.fundParty(id, POT);

        uint256 diamondBefore = rlh.balanceOf(address(h));
        rlh.arm(address(h), id, 0); // re-enter completeParty mid-payout

        vm.prank(creator);
        h.completeParty(id);

        assertTrue(rlh.reenterReverted(), "re-entrant completeParty reverted (NotActive)");
        // Exactly ONE pot left the diamond, not two.
        assertEq(rlh.balanceOf(address(h)), diamondBefore - POT, "exactly one split");
    }

    function test_reentrant_disband_cannot_double_refund() public {
        (PartyHarness h, ReentrantLH rlh) = _reentrantHarness();
        vm.prank(creator);
        uint256 id = h.formParty(_ids2(), _bps2(6000, 4000), TTL);
        vm.prank(funder);
        h.fundParty(id, POT);

        uint256 diamondBefore = rlh.balanceOf(address(h));
        rlh.arm(address(h), id, 1); // re-enter disbandParty mid-refund

        vm.prank(creator);
        h.disbandParty(id);

        assertTrue(rlh.reenterReverted(), "re-entrant disbandParty reverted (NotLive)");
        assertEq(rlh.balanceOf(address(h)), diamondBefore - POT, "exactly one refund");
    }

    // =====================================================================
    // liveParties pagination (live + unexpired, index-window paging)
    // =====================================================================

    function test_liveParties_filters_and_pages() public {
        uint256[] memory ids = new uint256[](5);
        for (uint256 i = 0; i < 5; i++) {
            ids[i] = _form();
        }
        // Terminal-ize ids[1] (disband) and settle ids[3] (consent+complete).
        vm.prank(creator);
        pf.disbandParty(ids[1]);
        vm.prank(aliceEoa);
        pf.joinParty(ids[3]);
        vm.prank(bobEoa);
        pf.joinParty(ids[3]);
        vm.prank(creator);
        pf.completeParty(ids[3]);

        // Full scan: only live remain → ids 0,2,4.
        (uint256[] memory all, uint256 cur) = pf.liveParties(0, 100);
        assertEq(cur, 5, "cursor at end after a full scan");
        assertEq(all.length, 3, "disbanded + completed filtered out");
        assertEq(all[0], ids[0]);
        assertEq(all[1], ids[2]);
        assertEq(all[2], ids[4]);

        // Page in windows of 2 over the index.
        (uint256[] memory p1, uint256 c1) = pf.liveParties(0, 2);
        assertEq(c1, 2);
        assertEq(p1.length, 1);
        assertEq(p1[0], ids[0]);
        (uint256[] memory p2, uint256 c2) = pf.liveParties(c1, 2);
        assertEq(c2, 4);
        assertEq(p2.length, 1);
        assertEq(p2[0], ids[2]);
        (uint256[] memory p3, uint256 c3) = pf.liveParties(c2, 2);
        assertEq(c3, 5);
        assertEq(p3.length, 1);
        assertEq(p3[0], ids[4]);
        // Past the end: empty, cursor clamps to total.
        (uint256[] memory p4, uint256 c4) = pf.liveParties(c3, 2);
        assertEq(p4.length, 0);
        assertEq(c4, 5);
    }

    function test_liveParties_excludes_expired() public {
        _form();
        (uint256[] memory before,) = pf.liveParties(0, 100);
        assertEq(before.length, 1);
        vm.warp(block.timestamp + TTL + 1);
        (uint256[] memory after_,) = pf.liveParties(0, 100);
        assertEq(after_.length, 0, "expired live parties drop off the board");
    }

    function test_getParty_unknown_returns_zeros() public view {
        (address c, uint64 exp, uint8 st, uint128 esc, uint256 n, uint256 acc) = pf.getParty(404);
        assertEq(c, address(0));
        assertEq(exp, 0);
        assertEq(st, 0);
        assertEq(esc, 0);
        assertEq(n, 0);
        assertEq(acc, 0);
    }

    // =====================================================================
    // FUZZ 1: the shares vector must sum to exactly 10000 bps
    // =====================================================================

    /// Any 2-share vector forms IFF both shares are nonzero AND they sum to
    /// exactly 10000 — the whole accept/reject boundary in one fuzz.
    function testFuzz_formParty_shares_must_sum_to_10000(uint16 a, uint16 b) public {
        uint256 sum = uint256(a) + uint256(b);
        vm.prank(creator);
        if (a == 0 || b == 0 || sum != LibPartyStorage.TOTAL_SHARES_BPS) {
            vm.expectRevert(PartyFacet.BadShares.selector);
            pf.formParty(_ids2(), _bps2(a, b), TTL);
        } else {
            uint256 id = pf.formParty(_ids2(), _bps2(a, b), TTL);
            uint16[] memory got = pf.partySharesOf(id);
            assertEq(got[0], a);
            assertEq(got[1], b);
        }
    }

    /// A valid random split of a random pot CONSERVES the escrow exactly:
    /// the member TBAs receive precisely the pot, the diamond is drained,
    /// nothing minted, nothing stranded (the remainder-to-last rule).
    function testFuzz_complete_split_conserves_escrow(uint16 aRaw, uint128 potRaw) public {
        uint16 a = uint16(bound(uint256(aRaw), 1, LibPartyStorage.TOTAL_SHARES_BPS - 1));
        uint16 b = uint16(LibPartyStorage.TOTAL_SHARES_BPS - a);
        uint128 pot = uint128(bound(uint256(potRaw), 1, 1_000_000 ether));

        vm.prank(creator);
        uint256 id = pf.formParty(_ids2(), _bps2(a, b), TTL);
        vm.prank(aliceEoa);
        pf.joinParty(id);
        vm.prank(bobEoa);
        pf.joinParty(id);
        lh.mint(funder, pot);
        vm.prank(funder);
        pf.fundParty(id, pot);

        vm.prank(creator);
        pf.completeParty(id);

        assertEq(
            lh.balanceOf(aliceTba) + lh.balanceOf(bobTba),
            uint256(pot),
            "split == escrow EXACTLY (nothing minted, nothing stranded)"
        );
        assertEq(lh.balanceOf(address(pf)), 0, "diamond fully drained");
        assertEq(lh.balanceOf(aliceTba), (uint256(pot) * a) / 10_000, "floor share to non-last");
    }

    // =====================================================================
    // FUZZ 2: escrow conservation across a random op sequence
    // =====================================================================

    /// The load-bearing invariant: at every point, the `$LH` the diamond
    /// holds for parties equals the sum of `escrowWei` over all LIVE
    /// (Forming/Active) parties. A split (complete) or refund (disband)
    /// removes both the escrow and the live record in lockstep; nothing is
    /// ever stranded or double-counted.
    function testFuzz_escrow_conservation(uint256 seedRaw) public {
        uint256 seed = seedRaw;
        assertEq(lh.balanceOf(address(pf)), 0, "diamond starts empty");

        for (uint256 i = 0; i < 40; i++) {
            seed = uint256(keccak256(abi.encode(seed, i)));
            uint256 action = seed % 5;

            if (action == 0) {
                // FORM (respect the per-creator cap).
                if (pf.activePartyCountOf(creator) < LibPartyStorage.MAX_ACTIVE_PER_CREATOR) {
                    _form();
                }
            } else if (pf.partyCount() > 0) {
                uint256 id = 1 + (seed % pf.partyCount());
                uint8 st = _status(id);
                bool live = st == uint8(LibPartyStorage.Status.Forming)
                    || st == uint8(LibPartyStorage.Status.Active);

                if (action == 1 && live) {
                    // FUND a bounded random amount.
                    uint128 amt = uint128(1 + (seed % 1000) * 1 ether);
                    vm.prank(funder);
                    pf.fundParty(id, amt);
                } else if (action == 2 && st == uint8(LibPartyStorage.Status.Forming)) {
                    // CONSENT both members → Active.
                    vm.prank(aliceEoa);
                    pf.joinParty(id);
                    vm.prank(bobEoa);
                    pf.joinParty(id);
                } else if (action == 3 && st == uint8(LibPartyStorage.Status.Active)) {
                    // COMPLETE → splits to TBAs, leaves the live set.
                    vm.prank(creator);
                    pf.completeParty(id);
                } else if (action == 4 && live) {
                    // DISBAND → refunds funders, leaves the live set.
                    vm.prank(creator);
                    pf.disbandParty(id);
                }
            }

            // INVARIANT after every step: diamond balance == sum of live
            // party escrows (recomputed straight from on-chain state).
            assertEq(
                lh.balanceOf(address(pf)),
                _sumLiveEscrow(),
                "diamond $LH == sum of live party escrows"
            );
        }
    }

    /// Sum `escrowWei` over every non-terminal party (Forming/Active) —
    /// the ones whose pot is still in the diamond.
    function _sumLiveEscrow() internal view returns (uint256 sum) {
        uint256 n = pf.partyCount();
        for (uint256 id = 1; id <= n; id++) {
            (,, uint8 st, uint128 esc,,) = pf.getParty(id);
            if (
                st == uint8(LibPartyStorage.Status.Forming)
                    || st == uint8(LibPartyStorage.Status.Active)
            ) {
                sum += esc;
            }
        }
    }
}
