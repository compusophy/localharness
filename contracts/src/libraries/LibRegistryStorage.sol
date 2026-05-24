// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the LocalharnessRegistry facet.
///      Diamond storage pattern: every facet that owns state stamps
///      out its own struct at a deterministic slot so facets can't
///      collide. Add new fields ONLY at the end of the struct;
///      never reorder or delete (storage layout is immutable).
library LibRegistryStorage {
    bytes32 constant REGISTRY_POSITION = keccak256("localharness.registry.storage.v1");

    struct Storage {
        // agentId -> owner address
        mapping(uint256 => address) ownerOfId;
        // name -> agentId (0 means unregistered; agentId starts at 1)
        mapping(string => uint256) idOfName;
        // agentId -> name
        mapping(uint256 => string) nameOfId;
        // address -> agentId  (one of the addr's tokens; if the owner
        // has multiple post-ERC-721, this is just "the most recent
        // they registered" — kept for back-compat, not authoritative)
        mapping(address => uint256) idOf;
        // agentId -> key -> bytes (ERC-8004 / ERC-8048-style metadata)
        mapping(uint256 => mapping(bytes32 => bytes)) metadata;
        // Monotonic agentId counter — initialised to 1 by DiamondInit.
        uint256 nextId;
        // --- ERC-721 storage (append-only; never reorder above) ----
        // owner -> token count
        mapping(address => uint256) balanceOf;
        // tokenId -> approved single-token spender
        mapping(uint256 => address) tokenApprovals;
        // owner -> operator -> all-token approval
        mapping(address => mapping(address => bool)) operatorApprovals;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = REGISTRY_POSITION;
        assembly {
            s.slot := position
        }
    }
}
