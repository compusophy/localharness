// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {MultiSignerAccount} from "../src/erc6551/MultiSignerAccount.sol";
import {TbaFacet} from "../src/facets/TbaFacet.sol";

/// Deploy a fresh MultiSignerAccount and swap the diamond's TBA
/// account-impl pointer to it. The ERC-6551 registry is unchanged.
///
/// Effects:
/// - All TBAs minted from this tx forward resolve to addresses derived
///   from the new impl (different counterfactual address than before).
/// - Already-deployed TBAs at the OLD impl continue to function but
///   are no longer reachable via the diamond's `tokenBoundAccount`
///   helper, which now derives the address against the new impl.
/// - Acceptable on testnet; mainnet would need an address-stable
///   upgrade path (e.g. a proxy in front of the impl).
///
/// Run with:
///   DIAMOND=0x6f2858b4b10bf8d4ea372a446e69bea8fbce2930 \
///   ERC6551_REGISTRY=0xc7cadc487eeb06fe8807104443b2f76b45c041d6 \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/SwapTbaImplToMultiSigner.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract SwapTbaImplToMultiSigner is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        address registry = vm.envAddress("ERC6551_REGISTRY");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        MultiSignerAccount accountImpl = new MultiSignerAccount();

        // Keep the same ERC-6551 registry; swap only the impl. Owner-
        // only — broadcaster must be the diamond owner.
        TbaFacet(diamond).setTbaConfig(registry, address(accountImpl));

        vm.stopBroadcast();

        console.log("--- TBA account impl swapped to MultiSignerAccount ---");
        console.log("diamond:        ", diamond);
        console.log("registry:       ", registry);
        console.log("newAccountImpl: ", address(accountImpl));
    }
}
