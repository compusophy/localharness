// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";
import {LocalharnessRegistry} from "../src/LocalharnessRegistry.sol";

/// Forge script to deploy `LocalharnessRegistry`. Run with:
///   forge script script/Deploy.s.sol --rpc-url tempo_moderato \
///                                    --private-key $EVM_PRIVATE_KEY \
///                                    --broadcast
/// The printed address is what `src/app/registry.rs::REGISTRY_ADDRESS`
/// in the wasm bundle reads on every mount.
contract Deploy is Script {
    function run() external returns (LocalharnessRegistry registry) {
        vm.startBroadcast();
        registry = new LocalharnessRegistry();
        vm.stopBroadcast();
        console.log("LocalharnessRegistry deployed at:", address(registry));
    }
}
