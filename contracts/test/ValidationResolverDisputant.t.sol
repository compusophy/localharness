// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {ValidationFacet} from "../src/facets/ValidationFacet.sol";
import {LibCreditsStorage} from "../src/libraries/LibCreditsStorage.sol";
import {LibRegistryStorage} from "../src/libraries/LibRegistryStorage.sol";
import {LibBountyStorage} from "../src/libraries/LibBountyStorage.sol";
import {LibDiamond} from "../src/libraries/LibDiamond.sol";

/// $LH-shaped TIP-20 mock — the same escrow/payout surface the facet pulls.
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

/// Harness exposes the diamond-storage writes a real cut would perform (token
/// wiring, a registered subject, the bounty poster, the diamond owner) so the
/// resolve path can be exercised in isolation.
contract ValidationHarness is ValidationFacet {
    function _setCreditsToken(address token) external {
        LibCreditsStorage.load().creditsToken = token;
    }

    function _setSubjectOwner(uint256 id, address owner) external {
        LibRegistryStorage.load().ownerOfId[id] = owner;
    }

    function _setBountyPoster(uint256 bountyId, address poster) external {
        LibBountyStorage.load().bounties[bountyId].poster = poster;
    }

    function _setDiamondOwner(address owner) external {
        LibDiamond.setContractOwner(owner);
    }
}

/// THE FIX (defense-in-depth): the resolver of a Challenged validation must
/// never be a disputant. The legit resolver is the work's bounty POSTER — a
/// neutral third party. If that poster is ALSO the validator or challenger, the
/// resolve is a self-deal (judge-is-a-party) and must revert, forcing the
/// owner-arbiter / draw path instead. The diamond owner (the platform arbiter
/// of last resort) stays exempt. These tests pin that guard.
contract ValidationResolverDisputantTest is Test {
    ValidationHarness v;
    MockLH lh;

    address validator = address(0x1111);
    address challenger = address(0x2222);
    address subjectOwner = address(0x3333);
    address neutralPoster = address(0x4444);
    address owner = address(0x0DAD);

    uint256 constant SUBJECT_ID = 7;
    uint256 constant BOUNTY_ID = 42;
    uint256 constant STAKE = 10 ether;
    bytes32 constant WORK_REF = bytes32(BOUNTY_ID); // _posterOf reads bounties[uint256(workRef)]

    function setUp() public {
        v = new ValidationHarness();
        lh = new MockLH();
        v._setCreditsToken(address(lh));
        v._setSubjectOwner(SUBJECT_ID, subjectOwner);
        v._setDiamondOwner(owner);

        lh.mint(validator, 1_000 ether);
        lh.mint(challenger, 1_000 ether);
        vm.prank(validator);
        lh.approve(address(v), type(uint256).max);
        vm.prank(challenger);
        lh.approve(address(v), type(uint256).max);

        vm.warp(1_000_000);
    }

    /// Stake + challenge so a validation lands in the Challenged state ready to
    /// resolve. Returns the id.
    function _challengedValidation() internal returns (uint256 id) {
        vm.prank(validator);
        id = v.stakeValidation(WORK_REF, SUBJECT_ID, true, STAKE);
        vm.prank(challenger);
        v.challengeValidation(id);
    }

    /// The poster who is ALSO the validator cannot self-resolve in their favor.
    function test_poster_equals_validator_cannot_self_resolve() public {
        uint256 id = _challengedValidation();
        v._setBountyPoster(BOUNTY_ID, validator); // poster IS the validator

        vm.prank(validator);
        vm.expectRevert(ValidationFacet.ResolverIsDisputant.selector);
        v.resolveValidation(id, true);
    }

    /// The poster who is ALSO the challenger cannot self-resolve in their favor.
    function test_poster_equals_challenger_cannot_self_resolve() public {
        uint256 id = _challengedValidation();
        v._setBountyPoster(BOUNTY_ID, challenger); // poster IS the challenger

        vm.prank(challenger);
        vm.expectRevert(ValidationFacet.ResolverIsDisputant.selector);
        v.resolveValidation(id, false);
    }

    /// A NEUTRAL poster (not a disputant) resolves normally — the legit path is
    /// untouched and the winner is paid both stakes.
    function test_neutral_poster_resolves_and_pays_winner() public {
        uint256 id = _challengedValidation();
        v._setBountyPoster(BOUNTY_ID, neutralPoster);

        uint256 before = lh.balanceOf(validator);
        vm.prank(neutralPoster);
        v.resolveValidation(id, true); // validator wins

        assertEq(lh.balanceOf(validator), before + STAKE * 2, "winner paid both stakes");
        assertEq(lh.balanceOf(challenger), 1_000 ether - STAKE, "loser stake forfeited");
    }

    /// The diamond owner (platform arbiter of last resort) is EXEMPT — it can
    /// resolve even when it happens to also be a disputant.
    function test_owner_arbiter_exempt_even_if_disputant() public {
        // Make the owner the validator-disputant, then have it resolve.
        v._setDiamondOwner(validator);
        uint256 id = _challengedValidation();
        v._setBountyPoster(BOUNTY_ID, neutralPoster); // poster is neutral; auth via owner branch

        uint256 before = lh.balanceOf(validator);
        vm.prank(validator); // validator == diamond owner
        v.resolveValidation(id, true);

        assertEq(lh.balanceOf(validator), before + STAKE * 2, "owner-disputant resolved");
    }
}
