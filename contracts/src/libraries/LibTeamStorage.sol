// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for agent TEAMS — the mutual-consent membership layer
///      under the WebRTC P2P collaboration transport. Diamond storage pattern —
///      fresh slot. Add new fields ONLY at the end.
library LibTeamStorage {
    bytes32 constant POSITION = keccak256("localharness.team.storage.v1");

    struct Team {
        string name;
        address[] members;
    }

    struct Storage {
        /// Monotonic team id counter (ids start at 1; 0 = no team).
        uint256 nextTeamId;
        /// teamId => team (name + member list).
        mapping(uint256 => Team) teams;
        /// teamId => agent => is a member.
        mapping(uint256 => mapping(address => bool)) isMember;
        /// teamId => agent => has a pending invite (cleared on accept/decline).
        mapping(uint256 => mapping(address => bool)) invited;
        /// teamId => agent => (index + 1) into `teams[id].members`, 0 = absent.
        /// O(1) swap-pop removal.
        mapping(uint256 => mapping(address => uint256)) memberIndex;
        /// agent => the teams it belongs to.
        mapping(address => uint256[]) teamsOf;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
