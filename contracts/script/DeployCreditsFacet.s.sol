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

    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        // 1. Deploy the credit token. Deployer is initial owner so it can grant
        // ISSUER_ROLE to the diamond + set the global mint cap.
        LocalharnessCredits credits = new LocalharnessCredits(INITIAL_SUPPLY_CAP, vm.addr(pk));

        // 1b. C1 global rolling-window mint cap on the FRESH token, BEFORE any
        // mint path is live (red-team launch gate). 0 = uncapped (fail-open) — a
        // mainnet deploy MUST pass a finite MINT_WINDOW_CAP_WEI. uncapped→finite
        // is an immediate "tighten".
        uint256 cap = vm.envOr("MINT_WINDOW_CAP_WEI", uint256(0));
        if (cap != 0) {
            credits.tightenMintWindow(cap, vm.envOr("MINT_WINDOW_SECS", uint256(1 days)));
        } else {
            console.log("WARNING: MINT_WINDOW_CAP_WEI=0 -> global mint cap DISABLED (set before mainnet)");
        }

        // 2. Cut the facet.
        address facet = _cutCreditsFacet(diamond);

        // 3. Grant the diamond ISSUER_ROLE so claimDaily/redeem/MintGate can mint.
        credits.grantRole(credits.ISSUER_ROLE(), diamond);
        CreditsFacet(diamond).setCreditsToken(address(credits));

        // 4. Daily faucet defaults to 0 (DISABLED) — a free daily mint is a sybil
        // hole on a value-real chain. Pass INITIAL_DAILY_ALLOWANCE for a testnet
        // faucet only.
        uint256 dailyAllowance = vm.envOr("INITIAL_DAILY_ALLOWANCE", uint256(0));
        if (dailyAllowance != 0) CreditsFacet(diamond).setDailyAllowance(dailyAllowance);

        vm.stopBroadcast();

        console.log("--- Credits + CreditsFacet deployed ---");
        console.log("creditsToken:   ", address(credits));
        console.log("creditsFacet:   ", facet);
        console.log("mintWindowCap:  ", cap / 1 ether);
        console.log("dailyAllow (LH):", dailyAllowance / 1 ether);
    }

    function _cutCreditsFacet(address diamond) internal returns (address) {
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
        return address(facet);
    }
}
