// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the credit-session facet. Diamond storage
///      pattern — fresh slot, no collision with any other facet's
///      storage already cut into the diamond. Add new fields ONLY at
///      the end.
library LibSessionStorage {
    bytes32 constant POSITION = keccak256("localharness.session.storage.v1");

    struct Storage {
        /// `$LH` (18-decimal wei) pulled from the caller to open one
        /// session. Zero = sessions are free (still gated by a valid
        /// identity at the proxy). Owner-tunable.
        uint256 priceWei;
        /// Session lifetime in seconds. A freshly opened session is
        /// valid until `block.timestamp + duration`. Zero = sessions
        /// disabled (openSession reverts). Owner-tunable.
        uint256 duration;
        /// address -> unix-seconds expiry of that account's current
        /// session. The credit proxy reads this via `sessionExpiryOf`
        /// and serves Gemini only while `expiry > now`. Coarse,
        /// time-bounded metering — no per-request on-chain write, so
        /// the proxy stays stateless and reads-only.
        mapping(address => uint256) sessionExpiry;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
