// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script} from "forge-std/Script.sol";

interface IMintGate {
    function mintFromFiat(
        address to,
        uint256 amount,
        bytes32 receiptId,
        uint256 validBefore,
        bytes calldata signature
    ) external;
    function fiatMintDomainSeparator() external view returns (bytes32);
}

/// One-off recovery: mint the `$LH` a CONFIRMED-but-unfulfilled Stripe payment
/// owes when the proxy's automated mint (finalize + webhook) failed. Uses the
/// SAME receiptId the proxy derives (`keccak256("localharness.fiatmint:"+piId)`),
/// so it's idempotent — if a backstop ever fires it's a clean no-op (ReceiptUsed).
/// Env: DIAMOND, MINT_TO, MINT_WEI, RECEIPT_ID, ISSUER_PK, SUBMITTER_PK.
contract MintForReceipt is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        address to = vm.envAddress("MINT_TO");
        uint256 amount = vm.envUint("MINT_WEI");
        bytes32 receiptId = vm.envBytes32("RECEIPT_ID");
        uint256 issuerPk = vm.envUint("ISSUER_PK");
        uint256 submitterPk = vm.envUint("SUBMITTER_PK");
        uint256 validBefore = block.timestamp + 86400;

        bytes32 typehash =
            keccak256("FiatMint(address to,uint256 amount,bytes32 receiptId,uint256 validBefore)");
        bytes32 structHash = keccak256(abi.encode(typehash, to, amount, receiptId, validBefore));
        bytes32 domainSep = IMintGate(diamond).fiatMintDomainSeparator();
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", domainSep, structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(issuerPk, digest);
        bytes memory sig = abi.encodePacked(r, s, v);

        vm.startBroadcast(submitterPk);
        IMintGate(diamond).mintFromFiat(to, amount, receiptId, validBefore, sig);
        vm.stopBroadcast();
    }
}
