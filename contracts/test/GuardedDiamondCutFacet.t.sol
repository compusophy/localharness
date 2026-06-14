// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {Diamond} from "../src/Diamond.sol";
import {GuardedDiamondCutFacet} from "../src/facets/GuardedDiamondCutFacet.sol";
import {DiamondLoupeFacet} from "../src/facets/DiamondLoupeFacet.sol";
import {OwnershipFacet} from "../src/facets/OwnershipFacet.sol";
import {CounterFacet} from "../src/facets/CounterFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";
import {IDiamondLoupe} from "../src/interfaces/IDiamondLoupe.sol";

/// Builds a REAL agent-owned child diamond seeded with the GUARDED cut facet
/// (the §7 on-chain twin of `cut_guard`) plus loupe + ownership, then proves the
/// guard binds even a raw, owner-signed `diamondCut`:
///   - a legit cut of the agent's OWN facet works and routes through the diamond;
///   - a cut touching ANY reserved selector reverts (incl. replacing the guard
///     itself → the constraint is permanent);
///   - a non-zero `_init` / `_calldata` reverts (no init delegatecall);
///   - a non-owner cut reverts.
contract GuardedDiamondCutFacetTest is Test {
    Diamond diamond;
    IDiamondCut cut;
    address stranger = address(0xBAD);

    function setUp() public {
        GuardedDiamondCutFacet guardedCut = new GuardedDiamondCutFacet();
        DiamondLoupeFacet loupe = new DiamondLoupeFacet();
        OwnershipFacet ownership = new OwnershipFacet();

        IDiamond.FacetCut[] memory genesis = new IDiamond.FacetCut[](3);

        bytes4[] memory cutSel = new bytes4[](1);
        cutSel[0] = IDiamondCut.diamondCut.selector;
        genesis[0] = IDiamond.FacetCut(address(guardedCut), IDiamond.FacetCutAction.Add, cutSel);

        bytes4[] memory loupeSel = new bytes4[](5);
        loupeSel[0] = DiamondLoupeFacet.facets.selector;
        loupeSel[1] = DiamondLoupeFacet.facetFunctionSelectors.selector;
        loupeSel[2] = DiamondLoupeFacet.facetAddresses.selector;
        loupeSel[3] = DiamondLoupeFacet.facetAddress.selector;
        loupeSel[4] = DiamondLoupeFacet.supportsInterface.selector;
        genesis[1] = IDiamond.FacetCut(address(loupe), IDiamond.FacetCutAction.Add, loupeSel);

        bytes4[] memory ownSel = new bytes4[](2);
        ownSel[0] = OwnershipFacet.transferOwnership.selector;
        ownSel[1] = OwnershipFacet.owner.selector;
        genesis[2] = IDiamond.FacetCut(address(ownership), IDiamond.FacetCutAction.Add, ownSel);

        diamond = new Diamond(address(this), genesis);
        cut = IDiamondCut(address(diamond));
    }

    // --- a legit cut of the agent's own facet works end to end -----------

    function test_legit_facet_cut_succeeds_and_routes() public {
        CounterFacet counter = new CounterFacet();
        IDiamond.FacetCut[] memory cuts = _counterCut(address(counter));

        cut.diamondCut(cuts, address(0), "");

        // routes through the diamond.
        CounterFacet(address(diamond)).increment();
        assertEq(CounterFacet(address(diamond)).countOf(address(this)), 1, "increment routes via diamond");

        // loupe sees the new facet.
        assertEq(
            IDiamondLoupe(address(diamond)).facetAddress(CounterFacet.increment.selector),
            address(counter),
            "loupe maps increment -> counter facet"
        );
    }

    // --- reserved selectors are refused (any action) ---------------------

    function test_cut_adding_reserved_selector_reverts() public {
        bytes4[] memory sels = new bytes4[](1);
        sels[0] = OwnershipFacet.transferOwnership.selector; // reserved
        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut(address(0xFACE), IDiamond.FacetCutAction.Add, sels);

        vm.expectRevert(bytes("Guarded: reserved selector"));
        cut.diamondCut(cuts, address(0), "");
    }

    function test_cannot_replace_the_guard_itself() public {
        // Replacing diamondCut would let an agent swap in an unguarded cut facet
        // — diamondCut is reserved, so the guard refuses, permanently.
        GuardedDiamondCutFacet evil = new GuardedDiamondCutFacet();
        bytes4[] memory sels = new bytes4[](1);
        sels[0] = IDiamondCut.diamondCut.selector;
        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut(address(evil), IDiamond.FacetCutAction.Replace, sels);

        vm.expectRevert(bytes("Guarded: reserved selector"));
        cut.diamondCut(cuts, address(0), "");
    }

    function test_cannot_remove_the_loupe() public {
        bytes4[] memory sels = new bytes4[](1);
        sels[0] = DiamondLoupeFacet.facets.selector; // reserved
        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut(address(0), IDiamond.FacetCutAction.Remove, sels);

        vm.expectRevert(bytes("Guarded: reserved selector"));
        cut.diamondCut(cuts, address(0), "");
    }

    // --- init delegatecall is forbidden ----------------------------------

    function test_nonzero_init_reverts() public {
        CounterFacet counter = new CounterFacet();
        IDiamond.FacetCut[] memory cuts = _counterCut(address(counter));

        vm.expectRevert(bytes("Guarded: init delegatecall forbidden"));
        cut.diamondCut(cuts, address(counter), "");
    }

    function test_nonempty_calldata_reverts() public {
        CounterFacet counter = new CounterFacet();
        IDiamond.FacetCut[] memory cuts = _counterCut(address(counter));

        vm.expectRevert(bytes("Guarded: init calldata forbidden"));
        cut.diamondCut(cuts, address(0), hex"deadbeef");
    }

    // --- ownership is still enforced -------------------------------------

    function test_nonowner_cut_reverts() public {
        CounterFacet counter = new CounterFacet();
        IDiamond.FacetCut[] memory cuts = _counterCut(address(counter));

        vm.prank(stranger);
        vm.expectRevert(bytes("LibDiamond: not owner"));
        cut.diamondCut(cuts, address(0), "");
    }

    // --- helper ----------------------------------------------------------

    function _counterCut(address counter) internal pure returns (IDiamond.FacetCut[] memory cuts) {
        bytes4[] memory sels = new bytes4[](4);
        sels[0] = CounterFacet.increment.selector;
        sels[1] = CounterFacet.incrementBy.selector;
        sels[2] = CounterFacet.countOf.selector;
        sels[3] = CounterFacet.totalCount.selector;
        cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut(counter, IDiamond.FacetCutAction.Add, sels);
    }
}
