// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibDiamond} from "../libraries/LibDiamond.sol";
import {LibRegistryStorage} from "../libraries/LibRegistryStorage.sol";
import {LibCreditsStorage} from "../libraries/LibCreditsStorage.sol";
import {LibRegistrationCostStorage} from "../libraries/LibRegistrationCostStorage.sol";

interface IERC20Min {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
}

/// @title LocalharnessRegistryFacet
/// @notice Subdomain registry — port of the flat `LocalharnessRegistry`
///         contract to a Diamond facet. Surface:
///         `register / setMetadata / isTaken / ownerOfName`
///         plus the storage-getter views `ownerOfId / idOfName /
///         nameOfId / idOf / nextId / metadata` and the cost-gate
///         knobs `setRegistrationCost / registrationCost`.
///
///         Storage lives in `LibRegistryStorage` at a dedicated slot
///         (`keccak256("localharness.registry.storage.v1")`) so
///         future facets — ERC-721 conformance, ERC-8004 reputation,
///         ERC-6551 helpers, MPP payments — can be added without
///         collision. Cost-gate state lives in
///         `LibRegistrationCostStorage` at its own slot for the same
///         reason.
contract LocalharnessRegistryFacet {
    event Registered(uint256 indexed agentId, address indexed owner, string name);
    event MetadataSet(uint256 indexed agentId, bytes32 indexed key, bytes value);
    event RegistrationCostUpdated(uint256 oldCostWei, uint256 newCostWei);
    // ERC-721 Transfer event — emitted on register (mint, from = 0) and
    // on the proper ERC-721 transferFrom (lives in ERC721Facet).
    event Transfer(address indexed from, address indexed to, uint256 indexed tokenId);

    // --- public mutators -------------------------------------------------

    /// Register `name` to `msg.sender`. Mints an ERC-721 token whose
    /// tokenId == agentId. One name -> one tokenId; an address can hold
    /// many tokens (multi-agent ownership is the intended path for
    /// wallets-per-agent via ERC-6551).
    ///
    /// When the registration cost is non-zero AND the credits token is
    /// configured, charges `costWei` from `msg.sender` to the diamond's
    /// own balance via `transferFrom`. Caller must approve the diamond
    /// for at least `costWei` ahead of time (typically batched into the
    /// same sponsored Tempo tx as the register call).
    function register(string calldata name) external returns (uint256 agentId) {
        LibRegistryStorage.Storage storage s = LibRegistryStorage.load();
        require(s.idOfName[name] == 0, "name taken");
        require(_isValidName(name), "invalid name");
        agentId = s.nextId++;
        s.ownerOfId[agentId] = msg.sender;
        s.idOfName[name] = agentId;
        s.nameOfId[agentId] = name;
        s.idOf[msg.sender] = agentId; // "most recent" for back-compat reads
        s.balanceOf[msg.sender] += 1;
        emit Registered(agentId, msg.sender, name);
        emit Transfer(address(0), msg.sender, agentId);

        _chargeRegistrationCost();
    }

    function _chargeRegistrationCost() internal {
        uint256 costWei = LibRegistrationCostStorage.load().costWei;
        if (costWei == 0) return;
        address creditsToken = LibCreditsStorage.load().creditsToken;
        if (creditsToken == address(0)) return;
        // External call last (CEI). transferFrom reverts on insufficient
        // balance or insufficient allowance — the whole register tx then
        // reverts atomically, so the user never gets the name without
        // paying.
        require(
            IERC20Min(creditsToken).transferFrom(msg.sender, address(this), costWei),
            "registration: transfer failed"
        );
    }

    /// Set the per-registration cost. Owner-only. Zero disables the
    /// cost gate entirely (registration is free). Emitted event lets
    /// off-chain indexers track price changes.
    function setRegistrationCost(uint256 newCostWei) external {
        LibDiamond.enforceIsContractOwner();
        LibRegistrationCostStorage.Storage storage s = LibRegistrationCostStorage.load();
        emit RegistrationCostUpdated(s.costWei, newCostWei);
        s.costWei = newCostWei;
    }

    function registrationCost() external view returns (uint256) {
        return LibRegistrationCostStorage.load().costWei;
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
