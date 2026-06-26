// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Remove the inert `chargeFromWallet(address,uint256)` selector from the live
/// diamond (audit L13). The wallet-primary billing model it implemented was
/// REJECTED, and the function let the meter key pull a user's full standing $LH
/// allowance (commonly set large/unlimited for deposit/x402/invite) straight into
/// the diamond. It is no longer in the Solidity source, so it must also stop being
/// routable on-chain. Remove-only cut: the old facet bytecode stays deployed but
/// the selector is dropped from the diamond, so the call path is gone.
///
/// chargeFromWallet is only cut on MAINNET (it was never cut on Moderato — the
/// loupe returns address(0) there).
///
/// Run with:
///   DIAMOND=0x8ab4f3a57643410cdf4022cdaf1faeef234f3a77 \
///   EVM_PRIVATE_KEY=0x<diamond owner key> \
///   forge script script/RemoveChargeFromWallet.s.sol --rpc-url tempo_mainnet --broadcast
contract RemoveChargeFromWallet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        bytes4[] memory sel = new bytes4[](1);
        sel[0] = bytes4(keccak256("chargeFromWallet(address,uint256)")); // 0xae5d2b3e

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(0), // Remove cuts MUST carry facetAddress == address(0)
            action: IDiamond.FacetCutAction.Remove,
            functionSelectors: sel
        });

        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- chargeFromWallet removed from diamond ---");
        console.log("diamond:", diamond);
        console.logBytes4(sel[0]);
    }
}
