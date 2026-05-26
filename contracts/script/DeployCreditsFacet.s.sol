// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {LocalharnessCredits} from "../src/LocalharnessCredits.sol";
import {CreditsFacet} from "../src/facets/CreditsFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys `LocalharnessCredits` (TIP-20-shaped, currency = "credits"),
/// cuts `CreditsFacet` into the diamond, grants the diamond
/// ISSUER_ROLE on the token, and seeds the initial daily allowance.
///
/// Run with:
///   DIAMOND=0x6f2858b4b10bf8d4ea372a446e69bea8fbce2930 \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/DeployCreditsFacet.s.sol \
///       --rpc-url tempo_moderato --broadcast
///
/// Initial parameters baked in below — tweak before re-running on
/// mainnet:
/// - supplyCap = 1e27 (1B tokens)
/// - dailyAllowance = 100e18 (100 LH per user per UTC day)
contract DeployCreditsFacet is Script {
    uint256 constant INITIAL_SUPPLY_CAP = 1_000_000_000 ether;
    uint256 constant INITIAL_DAILY_ALLOWANCE = 100 ether;

    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");
        address deployer = vm.addr(pk);

        vm.startBroadcast(pk);

        // 1. Deploy the credit token. Deployer is initial owner so it
        // can grant ISSUER_ROLE to the diamond in the next step.
        LocalharnessCredits credits = new LocalharnessCredits(
            INITIAL_SUPPLY_CAP,
            deployer
        );

        // 2. Cut the facet.
        CreditsFacet facet = new CreditsFacet();
        bytes4[] memory selectors = new bytes4[](7);
        selectors[0] = CreditsFacet.setCreditsToken.selector;
        selectors[1] = CreditsFacet.setDailyAllowance.selector;
        selectors[2] = CreditsFacet.claimDaily.selector;
        selectors[3] = CreditsFacet.creditsToken.selector;
        selectors[4] = CreditsFacet.dailyAllowance.selector;
        selectors[5] = CreditsFacet.lastClaimDay.selector;
        selectors[6] = CreditsFacet.canClaim.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(facet),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        // 3. Grant the diamond ISSUER_ROLE on the token so claimDaily
        // can mint. Deployer (owner) is the only one who can do this.
        credits.grantRole(credits.ISSUER_ROLE(), diamond);

        // 4. Configure the facet via cast-through-diamond calls. Both
        // are owner-only on the facet side.
        CreditsFacet(diamond).setCreditsToken(address(credits));
        CreditsFacet(diamond).setDailyAllowance(INITIAL_DAILY_ALLOWANCE);

        vm.stopBroadcast();

        console.log("--- Credits + CreditsFacet deployed ---");
        console.log("diamond:        ", diamond);
        console.log("creditsToken:   ", address(credits));
        console.log("creditsFacet:   ", address(facet));
        console.log("supplyCap (LH): ", INITIAL_SUPPLY_CAP / 1 ether);
        console.log("dailyAllow (LH):", INITIAL_DAILY_ALLOWANCE / 1 ether);
    }
}
