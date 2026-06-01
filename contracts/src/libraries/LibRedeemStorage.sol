// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the redeem-code facet. Diamond storage
///      pattern — fresh slot, no collision with registry / TBA /
///      feedback / main-identity / credits / session storage already
///      cut into the diamond. Add new fields ONLY at the end.
library LibRedeemStorage {
    bytes32 constant POSITION = keccak256("localharness.redeem.storage.v1");

    struct Storage {
        /// keccak256(code) -> $LH amount (18-decimal wei) minted on
        /// redemption. Zero means "not a registered code". Only the
        /// HASH lives on-chain, so loading codes never leaks the
        /// plaintext — the owner hands out the plaintext off-chain.
        mapping(bytes32 => uint256) codeAmount;
        /// keccak256(code) -> already redeemed. One-shot per code.
        mapping(bytes32 => bool) claimed;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
