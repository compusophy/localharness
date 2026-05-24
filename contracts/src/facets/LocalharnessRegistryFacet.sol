// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibRegistryStorage} from "../libraries/LibRegistryStorage.sol";

/// @title LocalharnessRegistryFacet
/// @notice Subdomain registry — port of the flat `LocalharnessRegistry`
///         contract to a Diamond facet. Identical surface:
///         `register / transfer / setMetadata / isTaken / ownerOfName`
///         plus the storage-getter views `ownerOfId / idOfName /
///         nameOfId / idOf / nextId / metadata`.
///
///         Storage lives in `LibRegistryStorage` at a dedicated slot
///         (`keccak256("localharness.registry.storage.v1")`) so
///         future facets — ERC-721 conformance, ERC-8004 reputation,
///         ERC-6551 helpers, MPP payments — can be added without
///         collision.
contract LocalharnessRegistryFacet {
    event Registered(uint256 indexed agentId, address indexed owner, string name);
    event Transferred(uint256 indexed agentId, address indexed from, address indexed to);
    event MetadataSet(uint256 indexed agentId, bytes32 indexed key, bytes value);

    // --- public mutators -------------------------------------------------

    /// Register `name` to `msg.sender`. Reverts if the name is taken
    /// or if the sender already owns a name (loosen via M9 if needed).
    function register(string calldata name) external returns (uint256 agentId) {
        LibRegistryStorage.Storage storage s = LibRegistryStorage.load();
        require(s.idOfName[name] == 0, "name taken");
        require(s.idOf[msg.sender] == 0, "sender already owns one");
        require(_isValidName(name), "invalid name");
        agentId = s.nextId++;
        s.ownerOfId[agentId] = msg.sender;
        s.idOfName[name] = agentId;
        s.nameOfId[agentId] = name;
        s.idOf[msg.sender] = agentId;
        emit Registered(agentId, msg.sender, name);
    }

    function transfer(uint256 agentId, address to) external {
        LibRegistryStorage.Storage storage s = LibRegistryStorage.load();
        require(s.ownerOfId[agentId] == msg.sender, "not owner");
        require(to != address(0), "burn via release");
        require(s.idOf[to] == 0, "recipient already owns one");
        delete s.idOf[msg.sender];
        s.ownerOfId[agentId] = to;
        s.idOf[to] = agentId;
        emit Transferred(agentId, msg.sender, to);
    }

    function setMetadata(uint256 agentId, bytes32 key, bytes calldata value) external {
        LibRegistryStorage.Storage storage s = LibRegistryStorage.load();
        require(s.ownerOfId[agentId] == msg.sender, "not owner");
        s.metadata[agentId][key] = value;
        emit MetadataSet(agentId, key, value);
    }

    // --- views -----------------------------------------------------------

    function isTaken(string calldata name) external view returns (bool) {
        return LibRegistryStorage.load().idOfName[name] != 0;
    }

    function ownerOfName(string calldata name) external view returns (address) {
        LibRegistryStorage.Storage storage s = LibRegistryStorage.load();
        uint256 id = s.idOfName[name];
        return id == 0 ? address(0) : s.ownerOfId[id];
    }

    function ownerOfId(uint256 agentId) external view returns (address) {
        return LibRegistryStorage.load().ownerOfId[agentId];
    }

    function idOfName(string calldata name) external view returns (uint256) {
        return LibRegistryStorage.load().idOfName[name];
    }

    function nameOfId(uint256 agentId) external view returns (string memory) {
        return LibRegistryStorage.load().nameOfId[agentId];
    }

    function idOf(address owner) external view returns (uint256) {
        return LibRegistryStorage.load().idOf[owner];
    }

    function nextId() external view returns (uint256) {
        return LibRegistryStorage.load().nextId;
    }

    function metadata(uint256 agentId, bytes32 key) external view returns (bytes memory) {
        return LibRegistryStorage.load().metadata[agentId][key];
    }

    // --- internals -------------------------------------------------------

    function _isValidName(string memory name) internal pure returns (bool) {
        bytes memory b = bytes(name);
        if (b.length < 3 || b.length > 32) return false;
        if (b[0] == 0x2d || b[b.length - 1] == 0x2d) return false;
        for (uint256 i = 0; i < b.length; i++) {
            bytes1 c = b[i];
            bool ok =
                (c >= 0x30 && c <= 0x39) || // 0-9
                (c >= 0x61 && c <= 0x7a) || // a-z
                (c == 0x2d);                // -
            if (!ok) return false;
        }
        return true;
    }
}
