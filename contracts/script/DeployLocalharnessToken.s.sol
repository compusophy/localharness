// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {LocalharnessToken} from "../src/LocalharnessToken.sol";

/// Deploy LocalharnessToken on Tempo Moderato.
///
/// Run with:
///   EVM_PRIVATE_KEY=<key> forge script script/DeployLocalharnessToken.s.sol \
///     --rpc-url tempo_moderato --broadcast
///
/// Constructor takes no args (everything's tunable post-deploy via
/// owner functions). Prints the deployed address — bake it into
/// `src/registry.rs::LOCALHARNESS_TOKEN_ADDRESS`.
contract DeployLocalharnessToken is Script {
    function run() external returns (address tokenAddr) {
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");
        address deployer = vm.addr(pk);
        console.log("deployer:", deployer);

        vm.startBroadcast(pk);
        LocalharnessToken token = new LocalharnessToken();
        vm.stopBroadcast();

        tokenAddr = address(token);
        console.log("LocalharnessToken deployed at:", tokenAddr);
        console.log("name:        ", token.name());
        console.log("symbol:      ", token.symbol());
        console.log("decimals:    ", token.decimals());
        console.log("faucetAmount:", token.faucetAmount());
    }
}
