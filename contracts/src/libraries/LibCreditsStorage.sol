// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the credits facet. Diamond storage
///      pattern — fresh slot, no collision with the registry / TBA /
///      feedback / main-identity storage already cut into the diamond.
///      Add new fields ONLY at the end of the struct.
library LibCreditsStorage {
    bytes32 constant POSITION = keccak256("localharness.credits.storage.v1");

    struct Storage {
        /// LocalharnessCredits token contract — the diamond holds
        /// ISSUER_ROLE on it so `claimDaily` can mint. One-time
        /// setter via `setCreditsToken` for owner.
        address creditsToken;
        /// Tokens minted to a caller of `claimDaily`. In 18-decimal
        /// token wei. Default 0 (no allowance) until owner sets it.
        uint256 dailyAllowance;
        /// Last UTC day index a given address claimed on. Day index =
        /// block.timestamp / 86400. Lets us reset claims at UTC
        /// midnight without any cron / external trigger.
        mapping(address => uint64) lastClaimDay;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
