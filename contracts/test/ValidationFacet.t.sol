// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {ValidationFacet} from "../src/facets/ValidationFacet.sol";
import {LibValidationStorage} from "../src/libraries/LibValidationStorage.sol";
import {LibBountyStorage} from "../src/libraries/LibBountyStorage.sol";
import {LibCreditsStorage} from "../src/libraries/LibCreditsStorage.sol";
import {LibRegistryStorage} from "../src/libraries/LibRegistryStorage.sol";
import {LibDiamond} from "../src/libraries/LibDiamond.sol";

/// Minimal `$LH`-shaped TIP-20 mock (same shape as the BountyFacet suite's):
/// 18-decimal balances + the approve/transferFrom/transfer surface the facet
/// escrows + pays out + refunds through. Reverts on an under-allowance /
/// under-balance pull so CEI ordering is provable (a failed escrow leaves no
/// ghost validation).
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
/// the only outbound external call in resolve/reclaimStake/reclaimUnresolved)
/// it re-enters the diamond, trying a SECOND settlement of the same
/// validation. Real `$LH` has NO callback; this is the defense-in-depth probe
/// that CEI ordering makes a double payout / double refund structurally
/// impossible (the re-entry re-reads a terminal status and reverts).
contract ReentrantLH {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    address public diamond;
    uint256 public attackId;
    uint8 public mode; // 0=resolve, 1=reclaimStake, 2=reclaimUnresolved
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
        // same validation a second time. CEI means the status is already
        // terminal, so this MUST revert (no double drain).
        if (diamond != address(0) && !entered) {
            entered = true;
            if (mode == 0) {
                try ValidationFacet(diamond).resolveValidation(attackId, true) {
                    reenterReverted = false;
                } catch {
                    reenterReverted = true;
                }
            } else if (mode == 1) {
                try ValidationFacet(diamond).reclaimStake(attackId) {
                    reenterReverted = false;
                } catch {
                    reenterReverted = true;
                }
            } else {
                try ValidationFacet(diamond).reclaimUnresolved(attackId) {
                    reenterReverted = false;
                } catch {
                    reenterReverted = true;
                }
            }
        }
        return true;
    }
}

/// Test harness: ValidationFacet + setters that write the SHARED diamond-
/// storage slots a real diamond populates via other facets (creditsToken
/// from CreditsFacet, ownerOfId from the registry, bounty posters from
/// BountyFacet for the resolver coupling, and the LibDiamond contract owner
/// for the arbiter fallback). Because every `Lib*Storage.load()` resolves
/// against THIS contract's storage, writing them here IS the cross-facet
/// storage sharing the diamond provides. The diamond IS the escrow holder,
/// so `address(this)` holds the staked `$LH`, exactly like the live diamond.
contract ValidationHarness is ValidationFacet {
    function _setCreditsToken(address token) external {
        LibCreditsStorage.load().creditsToken = token;
    }

    function _registerIdentity(uint256 id, address owner) external {
        LibRegistryStorage.load().ownerOfId[id] = owner;
    }

    function _setBountyPoster(uint256 bountyId, address poster) external {
        LibBountyStorage.load().bounties[bountyId].poster = poster;
    }

    function _setDiamondOwner(address owner) external {
        LibDiamond.diamondStorage().contractOwner = owner;
    }
}

contract ValidationFacetTest is Test {
    ValidationHarness v;
    MockLH lh;

    address validator = address(0xF00D); // stakes the verdict
    address challenger = address(0xC4A1); // counter-stakes the opposite
    address subjectOwner = address(0xCAFE); // owns the SUBJECT identity
    address poster = address(0x9057); // posted the bounty behind WORK_REF
    address arbiter = address(0xD1A0); // the diamond owner (fallback resolver)
    address stranger = address(0xBEEF); // poker / non-party

    uint256 constant SUBJECT_ID = 7; // the subject's registered tokenId
    uint256 constant BOUNTY_ID = 42; // the bounty behind WORK_REF
    uint128 constant STAKE = 100 ether; // 100 $LH
    // The platform convention: workRef = bytes32(bountyId).
    bytes32 constant WORK_REF = bytes32(uint256(BOUNTY_ID));
    // A non-bounty workRef (a commit-hash-style ref) — owner-only resolution.
    bytes32 constant FREE_REF = keccak256("git:deadbeef");

    function setUp() public {
        v = new ValidationHarness();
        lh = new MockLH();
        v._setCreditsToken(address(lh));
        v._setDiamondOwner(arbiter);

        // The subject identity + the bounty whose poster is the resolver.
        v._registerIdentity(SUBJECT_ID, subjectOwner);
        v._setBountyPoster(BOUNTY_ID, poster);

        // Fund both sides + pre-approve the diamond (the facet) for escrow.
        lh.mint(validator, 10_000_000 ether);
        lh.mint(challenger, 10_000_000 ether);
        vm.prank(validator);
        lh.approve(address(v), type(uint256).max);
        vm.prank(challenger);
        lh.approve(address(v), type(uint256).max);

        // Pin a stable timestamp so window math is deterministic.
        vm.warp(1_000_000);
    }

    function _stake() internal returns (uint256 id) {
        vm.prank(validator);
        id = v.stakeValidation(WORK_REF, SUBJECT_ID, true, STAKE);
    }

    function _challenge(uint256 id) internal {
        vm.prank(challenger);
        v.challengeValidation(id);
    }

    function _status(uint256 id) internal view returns (uint8 st) {
        (, , , , , , , st, ) = v.getValidation(id);
    }

    // =====================================================================
    // stakeValidation: escrow + storage + validation
    // =====================================================================

    function test_stake_escrows_and_stores() public {
        uint256 valBefore = lh.balanceOf(validator);
        uint256 id = _stake();

        assertEq(id, 1, "first validation id is 1");
        assertEq(lh.balanceOf(validator), valBefore - STAKE, "stake escrowed from validator");
        assertEq(lh.balanceOf(address(v)), STAKE, "diamond holds the escrow");

        (
            address val,
            address chal,
            uint256 subj,
            bytes32 wref,
            uint128 stake,
            uint64 cDeadline,
            uint64 rDeadline,
            uint8 st,
            bool verdict
        ) = v.getValidation(id);
        assertEq(val, validator, "validator recorded");
        assertEq(chal, address(0), "no challenger yet");
        assertEq(subj, SUBJECT_ID, "subject recorded");
        assertEq(wref, WORK_REF, "workRef recorded");
        assertEq(stake, STAKE, "stake recorded");
        assertEq(
            cDeadline,
            uint64(block.timestamp) + LibValidationStorage.CHALLENGE_WINDOW,
            "challenge deadline = now + window"
        );
        assertEq(rDeadline, 0, "no resolve deadline until challenged");
        assertEq(st, uint8(LibValidationStorage.Status.Open), "status Open");
        assertTrue(verdict, "verdict recorded");

        assertEq(v.validationCount(), 1);
        assertEq(v.validationStakedOf(validator), STAKE, "stakedOf bumped");
        assertEq(v.activeValidationCountOf(validator), 1, "active count bumped");
        assertTrue(v.hasValidated(validator, SUBJECT_ID, WORK_REF), "dedup flag set");

        uint256[] memory ofWork = v.validationsOfWork(WORK_REF);
        assertEq(ofWork.length, 1);
        assertEq(ofWork[0], id);
        uint256[] memory mine = v.validationsOf(validator);
        assertEq(mine.length, 1);
        assertEq(mine[0], id);
    }

    function test_stake_reverts_zero_stake() public {
        vm.prank(validator);
        vm.expectRevert(ValidationFacet.ZeroStake.selector);
        v.stakeValidation(WORK_REF, SUBJECT_ID, true, 0);
    }

    function test_stake_reverts_over_uint128() public {
        vm.prank(validator);
        vm.expectRevert(ValidationFacet.StakeCapExceeded.selector);
        v.stakeValidation(WORK_REF, SUBJECT_ID, true, uint256(type(uint128).max) + 1);
    }

    function test_stake_reverts_over_max_staked() public {
        vm.prank(validator);
        vm.expectRevert(ValidationFacet.StakeCapExceeded.selector);
        v.stakeValidation(WORK_REF, SUBJECT_ID, true, LibValidationStorage.MAX_STAKED + 1);
    }

    function test_stake_reverts_unknown_subject() public {
        vm.prank(validator);
        vm.expectRevert(ValidationFacet.UnknownSubject.selector);
        v.stakeValidation(WORK_REF, 4242, true, STAKE); // tokenId 4242 not registered
    }

    function test_stake_reverts_self_validation() public {
        // The SUBJECT's owner cannot stake about their own work (the
        // documented self-validation rule — mirrors SelfAttestation).
        lh.mint(subjectOwner, STAKE);
        vm.prank(subjectOwner);
        lh.approve(address(v), type(uint256).max);
        vm.prank(subjectOwner);
        vm.expectRevert(ValidationFacet.SelfValidation.selector);
        v.stakeValidation(WORK_REF, SUBJECT_ID, true, STAKE);
    }

    function test_stake_reverts_duplicate_verdict() public {
        _stake();
        // Same (validator, subject, workRef) — even a DIFFERENT verdict/stake.
        vm.prank(validator);
        vm.expectRevert(ValidationFacet.AlreadyValidated.selector);
        v.stakeValidation(WORK_REF, SUBJECT_ID, false, STAKE * 2);
    }

    function test_stake_dedup_survives_reclaim() public {
        // One verdict per (validator, subject, workRef) EVER: a reclaimed
        // stake doesn't reopen the slot.
        uint256 id = _stake();
        vm.warp(block.timestamp + LibValidationStorage.CHALLENGE_WINDOW + 1);
        v.reclaimStake(id);
        vm.prank(validator);
        vm.expectRevert(ValidationFacet.AlreadyValidated.selector);
        v.stakeValidation(WORK_REF, SUBJECT_ID, true, STAKE);
    }

    function test_stake_distinct_workrefs_allowed() public {
        _stake();
        vm.prank(validator);
        uint256 id2 = v.stakeValidation(FREE_REF, SUBJECT_ID, false, STAKE);
        assertEq(id2, 2, "distinct workRef = a fresh verdict slot");
        assertEq(v.validationStakedOf(validator), 2 * uint256(STAKE));
    }

    function test_stake_reverts_at_active_cap() public {
        for (uint256 i = 0; i < LibValidationStorage.MAX_ACTIVE_PER_VALIDATOR; i++) {
            vm.prank(validator);
            v.stakeValidation(bytes32(uint256(1000 + i)), SUBJECT_ID, true, 1);
        }
        assertEq(
            v.activeValidationCountOf(validator),
            LibValidationStorage.MAX_ACTIVE_PER_VALIDATOR,
            "at cap"
        );
        vm.prank(validator);
        vm.expectRevert(ValidationFacet.TooManyActiveValidations.selector);
        v.stakeValidation(bytes32(uint256(9999)), SUBJECT_ID, true, 1);
    }

    function test_active_cap_frees_a_slot_on_terminal_exit() public {
        uint256 firstId;
        for (uint256 i = 0; i < LibValidationStorage.MAX_ACTIVE_PER_VALIDATOR; i++) {
            vm.prank(validator);
            uint256 id = v.stakeValidation(bytes32(uint256(1000 + i)), SUBJECT_ID, true, 1);
            if (i == 0) firstId = id;
        }
        vm.warp(block.timestamp + LibValidationStorage.CHALLENGE_WINDOW + 1);
        v.reclaimStake(firstId);
        assertEq(
            v.activeValidationCountOf(validator),
            LibValidationStorage.MAX_ACTIVE_PER_VALIDATOR - 1,
            "reclaim decremented"
        );
        vm.prank(validator);
        uint256 fresh = v.stakeValidation(bytes32(uint256(9999)), SUBJECT_ID, true, 1);
        assertGt(fresh, 0, "stake allowed after a slot frees");
    }

    function test_stake_no_ghost_when_escrow_fails() public {
        // A broke validator: approve but no balance → transferFrom reverts →
        // the whole tx reverts, no record, no consumed id, no dedup flag.
        address broke = address(0x0B0B);
        vm.prank(broke);
        lh.approve(address(v), type(uint256).max);
        vm.prank(broke);
        vm.expectRevert(); // MockLH "balance"
        v.stakeValidation(WORK_REF, SUBJECT_ID, true, STAKE);

        (address val, , , , , , , , ) = v.getValidation(1);
        assertEq(val, address(0), "no ghost validation");
        assertEq(v.validationCount(), 0, "no id consumed on a failed escrow");
        assertEq(v.validationStakedOf(broke), 0, "no ghost stakedOf");
        assertEq(v.activeValidationCountOf(broke), 0, "no ghost active count");
        assertFalse(v.hasValidated(broke, SUBJECT_ID, WORK_REF), "no ghost dedup flag");
    }

    function test_stake_reverts_not_configured() public {
        ValidationHarness fresh = new ValidationHarness();
        fresh._registerIdentity(SUBJECT_ID, subjectOwner);
        vm.prank(validator);
        vm.expectRevert(ValidationFacet.NotConfigured.selector);
        fresh.stakeValidation(WORK_REF, SUBJECT_ID, true, STAKE);
    }

    // =====================================================================
    // challengeValidation: equal counter-stake, disjoint windows
    // =====================================================================

    function test_challenge_escrows_equal_stake_and_advances() public {
        uint256 id = _stake();
        uint256 chalBefore = lh.balanceOf(challenger);
        _challenge(id);

        assertEq(lh.balanceOf(challenger), chalBefore - STAKE, "equal counter-stake escrowed");
        assertEq(lh.balanceOf(address(v)), 2 * uint256(STAKE), "diamond holds both stakes");

        (, address chal, , , , , uint64 rDeadline, uint8 st, ) = v.getValidation(id);
        assertEq(chal, challenger, "challenger recorded");
        assertEq(st, uint8(LibValidationStorage.Status.Challenged), "status Challenged");
        assertEq(
            rDeadline,
            uint64(block.timestamp) + LibValidationStorage.RESOLUTION_WINDOW,
            "resolve deadline = now + window"
        );
        assertEq(v.validationStakedOf(challenger), STAKE, "challenger stakedOf bumped");
    }

    function test_challenge_reverts_unknown() public {
        vm.prank(challenger);
        vm.expectRevert(ValidationFacet.UnknownValidation.selector);
        v.challengeValidation(999);
    }

    function test_challenge_reverts_when_not_open() public {
        uint256 id = _stake();
        _challenge(id); // Challenged
        vm.prank(stranger);
        vm.expectRevert(ValidationFacet.NotOpen.selector);
        v.challengeValidation(id); // only ONE challenger (first-come)
    }

    function test_challenge_reverts_after_deadline() public {
        uint256 id = _stake();
        vm.warp(block.timestamp + LibValidationStorage.CHALLENGE_WINDOW + 1);
        vm.prank(challenger);
        vm.expectRevert(ValidationFacet.ChallengeWindowClosed.selector);
        v.challengeValidation(id);
    }

    function test_challenge_at_exact_deadline_still_ok() public {
        uint256 id = _stake();
        (, , , , , uint64 cDeadline, , , ) = v.getValidation(id);
        vm.warp(cDeadline); // now == deadline is still challengeable
        _challenge(id);
        assertEq(_status(id), uint8(LibValidationStorage.Status.Challenged));
    }

    function test_challenge_reverts_self_challenge() public {
        uint256 id = _stake();
        vm.prank(validator);
        vm.expectRevert(ValidationFacet.SelfChallenge.selector);
        v.challengeValidation(id);
    }

    function test_subject_owner_can_challenge() public {
        // The documented rule: the subject's owner CAN defend their own work
        // with a counter-stake (only STAKING about it is blocked).
        uint256 id = _stake();
        lh.mint(subjectOwner, STAKE);
        vm.prank(subjectOwner);
        lh.approve(address(v), type(uint256).max);
        vm.prank(subjectOwner);
        v.challengeValidation(id);
        (, address chal, , , , , , , ) = v.getValidation(id);
        assertEq(chal, subjectOwner, "subject owner challenged");
    }

    function test_challenge_no_flip_when_counter_stake_fails() public {
        // A broke challenger: the failed pull reverts the status flip — the
        // validation stays cleanly Open (CEI, no ghost challenge).
        uint256 id = _stake();
        address broke = address(0x0B0B);
        vm.prank(broke);
        lh.approve(address(v), type(uint256).max);
        vm.prank(broke);
        vm.expectRevert(); // MockLH "balance"
        v.challengeValidation(id);
        assertEq(_status(id), uint8(LibValidationStorage.Status.Open), "stays Open");
        assertEq(v.validationStakedOf(broke), 0, "no ghost stakedOf");
    }

    // =====================================================================
    // resolveValidation: the oracle picks the winner; loser pays winner
    // =====================================================================

    function test_resolve_validator_wins_pays_both_stakes() public {
        uint256 id = _stake();
        _challenge(id);
        uint256 valBefore = lh.balanceOf(validator);
        uint256 chalBefore = lh.balanceOf(challenger);

        vm.prank(poster);
        v.resolveValidation(id, true);

        assertEq(
            lh.balanceOf(validator), valBefore + 2 * uint256(STAKE), "validator paid both stakes"
        );
        assertEq(lh.balanceOf(challenger), chalBefore, "challenger's stake is forfeit");
        assertEq(lh.balanceOf(address(v)), 0, "diamond drained the escrow");
        assertEq(_status(id), uint8(LibValidationStorage.Status.ValidatorWon));
        assertEq(v.validationStakedOf(validator), 0, "validator stakedOf cleared");
        assertEq(v.validationStakedOf(challenger), 0, "challenger stakedOf cleared");
        assertEq(v.activeValidationCountOf(validator), 0, "active count cleared");
    }

    function test_resolve_challenger_wins_pays_both_stakes() public {
        uint256 id = _stake();
        _challenge(id);
        uint256 chalBefore = lh.balanceOf(challenger);

        vm.prank(poster);
        v.resolveValidation(id, false);

        assertEq(
            lh.balanceOf(challenger), chalBefore + 2 * uint256(STAKE), "challenger paid both stakes"
        );
        assertEq(lh.balanceOf(address(v)), 0, "diamond drained");
        assertEq(_status(id), uint8(LibValidationStorage.Status.ChallengerWon));
    }

    function test_resolve_by_diamond_owner_arbiter() public {
        // The arbiter fallback: the diamond owner may always resolve.
        uint256 id = _stake();
        _challenge(id);
        vm.prank(arbiter);
        v.resolveValidation(id, false);
        assertEq(_status(id), uint8(LibValidationStorage.Status.ChallengerWon));
    }

    function test_resolve_non_bounty_workref_owner_only() public {
        // FREE_REF maps to no bounty → resolverOf is zero → ONLY the diamond
        // owner can resolve.
        vm.prank(validator);
        uint256 id = v.stakeValidation(FREE_REF, SUBJECT_ID, true, STAKE);
        _challenge(id);

        assertEq(v.validationResolverOf(id), address(0), "no bounty poster for a free ref");
        vm.prank(poster); // the (unrelated) bounty poster has no say here
        vm.expectRevert(ValidationFacet.NotResolver.selector);
        v.resolveValidation(id, true);
        vm.prank(arbiter);
        v.resolveValidation(id, true);
        assertEq(_status(id), uint8(LibValidationStorage.Status.ValidatorWon));
    }

    function test_resolve_reverts_stranger() public {
        uint256 id = _stake();
        _challenge(id);
        vm.prank(stranger);
        vm.expectRevert(ValidationFacet.NotResolver.selector);
        v.resolveValidation(id, true);
        // The disputants themselves are not resolvers either (unless one
        // happens to BE the poster/owner — the documented trust boundary).
        vm.prank(validator);
        vm.expectRevert(ValidationFacet.NotResolver.selector);
        v.resolveValidation(id, true);
        vm.prank(challenger);
        vm.expectRevert(ValidationFacet.NotResolver.selector);
        v.resolveValidation(id, false);
    }

    function test_resolve_reverts_unknown() public {
        vm.prank(arbiter);
        vm.expectRevert(ValidationFacet.UnknownValidation.selector);
        v.resolveValidation(999, true);
    }

    function test_resolve_reverts_when_not_challenged() public {
        uint256 id = _stake(); // Open, never challenged
        vm.prank(poster);
        vm.expectRevert(ValidationFacet.NotChallenged.selector);
        v.resolveValidation(id, true);
    }

    function test_resolve_reverts_double_resolve() public {
        uint256 id = _stake();
        _challenge(id);
        vm.prank(poster);
        v.resolveValidation(id, true); // ValidatorWon (terminal)
        vm.prank(poster);
        vm.expectRevert(ValidationFacet.NotChallenged.selector);
        v.resolveValidation(id, true); // can't pay twice
        vm.prank(arbiter);
        vm.expectRevert(ValidationFacet.NotChallenged.selector);
        v.resolveValidation(id, false); // nor flip the outcome
    }

    function test_resolve_reverts_after_resolve_deadline() public {
        // Past the resolve window it is a DRAW, not a late ruling — the
        // resolve and draw windows are disjoint.
        uint256 id = _stake();
        _challenge(id);
        vm.warp(block.timestamp + LibValidationStorage.RESOLUTION_WINDOW + 1);
        vm.prank(poster);
        vm.expectRevert(ValidationFacet.ResolveWindowClosed.selector);
        v.resolveValidation(id, true);
    }

    function test_resolve_at_exact_deadline_still_ok() public {
        uint256 id = _stake();
        _challenge(id);
        (, , , , , , uint64 rDeadline, , ) = v.getValidation(id);
        vm.warp(rDeadline); // now == deadline is still resolvable
        vm.prank(poster);
        v.resolveValidation(id, true);
        assertEq(_status(id), uint8(LibValidationStorage.Status.ValidatorWon));
    }

    // =====================================================================
    // reclaimStake: unchallenged stake comes home (disjoint with challenge)
    // =====================================================================

    function test_reclaimStake_refunds_validator() public {
        uint256 id = _stake();
        vm.warp(block.timestamp + LibValidationStorage.CHALLENGE_WINDOW + 1);
        uint256 valBefore = lh.balanceOf(validator);
        // Permissionless poke: a stranger calls; the VALIDATOR gets the money.
        vm.prank(stranger);
        v.reclaimStake(id);

        assertEq(lh.balanceOf(validator), valBefore + STAKE, "validator refunded 100%");
        assertEq(lh.balanceOf(stranger), 0, "the poker gains nothing");
        assertEq(lh.balanceOf(address(v)), 0, "diamond drained");
        assertEq(_status(id), uint8(LibValidationStorage.Status.Reclaimed));
        assertEq(v.validationStakedOf(validator), 0, "stakedOf cleared");
        assertEq(v.activeValidationCountOf(validator), 0, "active count cleared");
    }

    function test_reclaimStake_reverts_before_deadline() public {
        uint256 id = _stake();
        vm.prank(stranger);
        vm.expectRevert(ValidationFacet.ChallengeWindowStillOpen.selector);
        v.reclaimStake(id);
    }

    function test_reclaimStake_reverts_at_exact_deadline() public {
        // now == deadline: still CHALLENGEABLE, not yet reclaimable — the
        // windows are disjoint with no overlap second.
        uint256 id = _stake();
        (, , , , , uint64 cDeadline, , , ) = v.getValidation(id);
        vm.warp(cDeadline);
        vm.prank(stranger);
        vm.expectRevert(ValidationFacet.ChallengeWindowStillOpen.selector);
        v.reclaimStake(id);
    }

    function test_reclaimStake_reverts_unknown() public {
        vm.expectRevert(ValidationFacet.UnknownValidation.selector);
        v.reclaimStake(999);
    }

    function test_reclaimStake_reverts_double_reclaim() public {
        uint256 id = _stake();
        vm.warp(block.timestamp + LibValidationStorage.CHALLENGE_WINDOW + 1);
        v.reclaimStake(id);
        vm.expectRevert(ValidationFacet.NotOpen.selector);
        v.reclaimStake(id);
    }

    function test_reclaimStake_reverts_on_challenged() public {
        // A challenged validation is NOT reclaimable via the unchallenged
        // path, even after the challenge deadline passes.
        uint256 id = _stake();
        _challenge(id);
        vm.warp(block.timestamp + LibValidationStorage.CHALLENGE_WINDOW + 1);
        vm.expectRevert(ValidationFacet.NotOpen.selector);
        v.reclaimStake(id);
    }

    function test_challenged_then_resolved_cannot_be_reclaimed() public {
        // Terminal states reject BOTH reclaim paths.
        uint256 id = _stake();
        _challenge(id);
        vm.prank(poster);
        v.resolveValidation(id, true);
        vm.warp(block.timestamp + 365 days);
        vm.expectRevert(ValidationFacet.NotOpen.selector);
        v.reclaimStake(id);
        vm.expectRevert(ValidationFacet.NotChallenged.selector);
        v.reclaimUnresolved(id);
    }

    // =====================================================================
    // reclaimUnresolved: the AWOL-resolver hard stop (both refunded)
    // =====================================================================

    function test_reclaimUnresolved_refunds_both_sides() public {
        uint256 id = _stake();
        _challenge(id);
        uint256 valBefore = lh.balanceOf(validator);
        uint256 chalBefore = lh.balanceOf(challenger);
        vm.warp(block.timestamp + LibValidationStorage.RESOLUTION_WINDOW + 1);

        vm.prank(stranger);
        v.reclaimUnresolved(id);

        assertEq(lh.balanceOf(validator), valBefore + STAKE, "validator refunded own stake");
        assertEq(lh.balanceOf(challenger), chalBefore + STAKE, "challenger refunded own stake");
        assertEq(lh.balanceOf(stranger), 0, "the poker gains nothing");
        assertEq(lh.balanceOf(address(v)), 0, "diamond drained, nothing stranded");
        assertEq(_status(id), uint8(LibValidationStorage.Status.Drawn));
        assertEq(v.validationStakedOf(validator), 0);
        assertEq(v.validationStakedOf(challenger), 0);
        assertEq(v.activeValidationCountOf(validator), 0);
    }

    function test_reclaimUnresolved_reverts_before_deadline() public {
        uint256 id = _stake();
        _challenge(id);
        vm.expectRevert(ValidationFacet.ResolveWindowStillOpen.selector);
        v.reclaimUnresolved(id);
    }

    function test_reclaimUnresolved_reverts_at_exact_deadline() public {
        // now == resolveDeadline: still RESOLVABLE, not yet drawable.
        uint256 id = _stake();
        _challenge(id);
        (, , , , , , uint64 rDeadline, , ) = v.getValidation(id);
        vm.warp(rDeadline);
        vm.expectRevert(ValidationFacet.ResolveWindowStillOpen.selector);
        v.reclaimUnresolved(id);
    }

    function test_reclaimUnresolved_reverts_on_open() public {
        uint256 id = _stake();
        vm.warp(block.timestamp + 365 days);
        vm.expectRevert(ValidationFacet.NotChallenged.selector);
        v.reclaimUnresolved(id);
    }

    function test_reclaimUnresolved_reverts_double_draw() public {
        uint256 id = _stake();
        _challenge(id);
        vm.warp(block.timestamp + LibValidationStorage.RESOLUTION_WINDOW + 1);
        v.reclaimUnresolved(id);
        vm.expectRevert(ValidationFacet.NotChallenged.selector);
        v.reclaimUnresolved(id);
    }

    function test_resolve_after_draw_reverts() public {
        // A drawn dispute can't be late-ruled (resolved XOR drawn).
        uint256 id = _stake();
        _challenge(id);
        vm.warp(block.timestamp + LibValidationStorage.RESOLUTION_WINDOW + 1);
        v.reclaimUnresolved(id);
        vm.prank(arbiter);
        vm.expectRevert(ValidationFacet.NotChallenged.selector);
        v.resolveValidation(id, true);
    }

    // =====================================================================
    // resolver coupling views
    // =====================================================================

    function test_validationResolverOf_bounty_coupling() public {
        uint256 id = _stake();
        assertEq(v.validationResolverOf(id), poster, "bounty workRef resolves to the bounty's poster");
        assertEq(v.validationResolverOf(999), address(0), "unknown id resolves to zero");
    }

    // =====================================================================
    // REENTRANCY PROBES — a hostile token re-enters during settlement
    // =====================================================================

    function _reentrantHarness() internal returns (ValidationHarness h, ReentrantLH rlh) {
        rlh = new ReentrantLH();
        h = new ValidationHarness();
        h._setCreditsToken(address(rlh));
        h._setDiamondOwner(arbiter);
        h._registerIdentity(SUBJECT_ID, subjectOwner);
        h._setBountyPoster(BOUNTY_ID, poster);
        rlh.mint(validator, 1_000_000 ether);
        rlh.mint(challenger, 1_000_000 ether);
        vm.prank(validator);
        rlh.approve(address(h), type(uint256).max);
        vm.prank(challenger);
        rlh.approve(address(h), type(uint256).max);
        // Extra balance in the diamond so a SUCCESSFUL double-drain would
        // have something to steal (proving the revert is what saves it).
        rlh.mint(address(h), 1_000_000 ether);
        vm.warp(1_000_000);
    }

    function test_reentrant_resolve_cannot_double_pay() public {
        (ValidationHarness h, ReentrantLH rlh) = _reentrantHarness();
        vm.prank(validator);
        uint256 id = h.stakeValidation(WORK_REF, SUBJECT_ID, true, STAKE);
        vm.prank(challenger);
        h.challengeValidation(id);

        uint256 diamondBefore = rlh.balanceOf(address(h));
        rlh.arm(address(h), id, 0); // mode 0 = re-enter resolveValidation

        vm.prank(poster);
        h.resolveValidation(id, true);

        assertTrue(rlh.reenterReverted(), "re-entrant resolve reverted (NotChallenged)");
        // Exactly ONE 2x payout left the diamond, not two.
        assertEq(rlh.balanceOf(address(h)), diamondBefore - 2 * uint256(STAKE), "exactly one payout");
        (, , , , , , , uint8 st, ) = h.getValidation(id);
        assertEq(st, uint8(LibValidationStorage.Status.ValidatorWon));
    }

    function test_reentrant_reclaimStake_cannot_double_refund() public {
        (ValidationHarness h, ReentrantLH rlh) = _reentrantHarness();
        vm.prank(validator);
        uint256 id = h.stakeValidation(WORK_REF, SUBJECT_ID, true, STAKE);
        vm.warp(block.timestamp + LibValidationStorage.CHALLENGE_WINDOW + 1);

        uint256 diamondBefore = rlh.balanceOf(address(h));
        rlh.arm(address(h), id, 1); // mode 1 = re-enter reclaimStake

        vm.prank(stranger);
        h.reclaimStake(id);

        assertTrue(rlh.reenterReverted(), "re-entrant reclaimStake reverted (NotOpen)");
        assertEq(rlh.balanceOf(address(h)), diamondBefore - STAKE, "exactly one refund");
        (, , , , , , , uint8 st, ) = h.getValidation(id);
        assertEq(st, uint8(LibValidationStorage.Status.Reclaimed));
    }

    function test_reentrant_reclaimUnresolved_cannot_double_refund() public {
        (ValidationHarness h, ReentrantLH rlh) = _reentrantHarness();
        vm.prank(validator);
        uint256 id = h.stakeValidation(WORK_REF, SUBJECT_ID, true, STAKE);
        vm.prank(challenger);
        h.challengeValidation(id);
        vm.warp(block.timestamp + LibValidationStorage.RESOLUTION_WINDOW + 1);

        uint256 diamondBefore = rlh.balanceOf(address(h));
        rlh.arm(address(h), id, 2); // mode 2 = re-enter reclaimUnresolved

        vm.prank(stranger);
        h.reclaimUnresolved(id);

        assertTrue(rlh.reenterReverted(), "re-entrant reclaimUnresolved reverted (NotChallenged)");
        // Exactly one stake to EACH side — 2x total — not 4x.
        assertEq(rlh.balanceOf(address(h)), diamondBefore - 2 * uint256(STAKE), "exactly one draw refund");
        (, , , , , , , uint8 st, ) = h.getValidation(id);
        assertEq(st, uint8(LibValidationStorage.Status.Drawn));
    }

    // =====================================================================
    // FUZZ: escrow conservation — sum(live escrows) == diamond $LH balance
    // =====================================================================

    /// The load-bearing invariant: at every point, the `$LH` the diamond
    /// holds for validations equals the sum over all LIVE validations of
    /// (stakeWei while Open, 2*stakeWei while Challenged). Every exit
    /// (resolve / reclaim / draw) removes the escrow and the live record in
    /// lockstep; nothing is ever stranded, double-counted, or minted.
    function testFuzz_escrow_conservation(uint256 seedRaw) public {
        uint256 seed = seedRaw;
        assertEq(lh.balanceOf(address(v)), 0, "diamond starts empty");

        uint256 liveIdsLen = 0;
        uint256[] memory liveIds = new uint256[](48);
        uint256 refSalt = 0;

        for (uint256 i = 0; i < 40; i++) {
            seed = uint256(keccak256(abi.encode(seed, i)));
            uint256 action = seed % 6;

            if (action == 0 || liveIdsLen == 0) {
                // STAKE: a fresh workRef each time (the dedup is per-work),
                // bounded stake, respect the per-validator cap.
                if (
                    v.activeValidationCountOf(validator)
                        < LibValidationStorage.MAX_ACTIVE_PER_VALIDATOR
                ) {
                    uint256 stake = 1 + (seed % 1000) * 1 ether;
                    bytes32 wref = bytes32(uint256(100_000 + refSalt++));
                    vm.prank(validator);
                    uint256 id = v.stakeValidation(wref, SUBJECT_ID, (seed >> 8) % 2 == 0, stake);
                    liveIds[liveIdsLen++] = id;
                }
            } else {
                uint256 pick = seed % liveIdsLen;
                uint256 id = liveIds[pick];
                uint8 st = _status(id);

                if (action == 1 && st == uint8(LibValidationStorage.Status.Open)) {
                    // CHALLENGE (counter-stake; stays in the live set) — only
                    // if the warps below haven't already expired its window.
                    (, , , , , uint64 cDeadline, , , ) = v.getValidation(id);
                    if (block.timestamp <= cDeadline) {
                        vm.prank(challenger);
                        v.challengeValidation(id);
                    }
                } else if (action == 2 && st == uint8(LibValidationStorage.Status.Challenged)) {
                    // RESOLVE → winner takes both; leaves the live set.
                    // These fuzz workRefs map to no bounty, so only the
                    // arbiter (the diamond owner) can resolve — and only
                    // while the resolve window is still open (the warps
                    // below may have expired it; action 4 drains those).
                    (, , , , , , uint64 rDeadline, , ) = v.getValidation(id);
                    if (block.timestamp <= rDeadline) {
                        vm.prank(arbiter);
                        v.resolveValidation(id, (seed >> 16) % 2 == 0);
                        liveIds[pick] = liveIds[--liveIdsLen];
                    }
                } else if (action == 3 && st == uint8(LibValidationStorage.Status.Open)) {
                    // RECLAIM unchallenged: warp past the challenge window.
                    // (Warping also expires other Open windows — fine: the
                    // invariant doesn't depend on time, and later challenge
                    // attempts are guarded by status, not asserted here.)
                    (, , , , , uint64 cDeadline, , , ) = v.getValidation(id);
                    if (block.timestamp <= cDeadline) vm.warp(uint256(cDeadline) + 1);
                    v.reclaimStake(id);
                    liveIds[pick] = liveIds[--liveIdsLen];
                } else if (action == 4 && st == uint8(LibValidationStorage.Status.Challenged)) {
                    // DRAW: warp past the resolve window, both refunded.
                    (, , , , , , uint64 rDeadline, , ) = v.getValidation(id);
                    if (block.timestamp <= rDeadline) vm.warp(uint256(rDeadline) + 1);
                    v.reclaimUnresolved(id);
                    liveIds[pick] = liveIds[--liveIdsLen];
                } else if (action == 5 && st == uint8(LibValidationStorage.Status.Open)) {
                    // CHALLENGE only if its window is still open (the warps
                    // above may have expired it) — else skip this step.
                    (, , , , , uint64 cDeadline, , , ) = v.getValidation(id);
                    if (block.timestamp <= cDeadline) {
                        vm.prank(challenger);
                        v.challengeValidation(id);
                    }
                }
            }

            // INVARIANT after every step: diamond balance == sum of live
            // escrows (recomputed straight from on-chain state), AND the
            // stakedOf ledgers agree with it.
            uint256 liveSum = _sumLiveEscrow();
            assertEq(lh.balanceOf(address(v)), liveSum, "diamond $LH == sum of live escrows");
            assertEq(
                v.validationStakedOf(validator) + v.validationStakedOf(challenger),
                liveSum,
                "stakedOf ledgers == live escrow"
            );
        }
    }

    /// Sum the escrow still inside the diamond over every validation:
    /// stake while Open, both stakes while Challenged, zero once terminal.
    function _sumLiveEscrow() internal view returns (uint256 sum) {
        uint256 n = v.validationCount();
        for (uint256 id = 1; id <= n; id++) {
            (, , , , uint128 stake, , , uint8 st, ) = v.getValidation(id);
            if (st == uint8(LibValidationStorage.Status.Open)) {
                sum += stake;
            } else if (st == uint8(LibValidationStorage.Status.Challenged)) {
                sum += 2 * uint256(stake);
            }
        }
    }

    /// Fuzz the stake amount itself across the full accepted range — the
    /// boundary partner to the action-sequence fuzz above.
    function testFuzz_stake_roundtrip_any_amount(uint128 stakeRaw) public {
        uint256 stake = bound(uint256(stakeRaw), 1, LibValidationStorage.MAX_STAKED);
        lh.mint(validator, stake);
        uint256 valBefore = lh.balanceOf(validator);

        vm.prank(validator);
        uint256 id = v.stakeValidation(WORK_REF, SUBJECT_ID, true, stake);
        assertEq(lh.balanceOf(address(v)), stake, "escrowed exactly");

        vm.warp(block.timestamp + LibValidationStorage.CHALLENGE_WINDOW + 1);
        v.reclaimStake(id);
        assertEq(lh.balanceOf(validator), valBefore, "round-trips to the wei");
        assertEq(lh.balanceOf(address(v)), 0, "nothing stranded");
    }
}
