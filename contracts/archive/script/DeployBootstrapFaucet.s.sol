// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {BootstrapFaucet} from "../src/BootstrapFaucet.sol";

/// Deploy + pre-fund the BootstrapFaucet on Tempo Moderato.
///
/// Run with:
///   forge script script/DeployBootstrapFaucet.s.sol \
///     --rpc-url tempo_moderato \
///     --private-key $EVM_PRIVATE_KEY \
///     --broadcast \
///     --sig "run(uint256,uint256)" $INITIAL_DRIP_WEI $PREFUND_WEI
///
/// `INITIAL_DRIP_WEI` is the per-recipient drip (e.g.
/// 10000000000000000 = 0.01 ETH). `PREFUND_WEI` is the amount sent
/// to the contract at deploy (cover ~N drips). Both default sensibly
/// if you just call `run()` with no args.
///
/// Prints the deployed contract address. Bake it into
/// `src/registry.rs::BOOTSTRAP_FAUCET_ADDRESS` after deploy.
contract DeployBootstrapFaucet is Script {
    /// Default: 0.01 ETH drip per recipient, 1 ETH pre-fund (100 drips).
    function run() external returns (address faucetAddr) {
        return run(0.01 ether, 1 ether);
    }

    function run(uint256 initialDripWei, uint256 prefundWei)
        public
        returns (address faucetAddr)
    {
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");
        address deployer = vm.addr(pk);

        console.log("deployer:    ", deployer);
        console.log("drip (wei):  ", initialDripWei);
        console.log("prefund (wei):", prefundWei);

        vm.startBroadcast(pk);

        BootstrapFaucet faucet = new BootstrapFaucet{value: prefundWei}(initialDripWei);

        vm.stopBroadcast();

        faucetAddr = address(faucet);
        console.log("BootstrapFaucet deployed at:", faucetAddr);
        console.log("balance (wei):", faucetAddr.balance);
    }
}
