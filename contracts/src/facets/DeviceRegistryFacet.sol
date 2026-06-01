// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibDeviceRegistryStorage} from "../libraries/LibDeviceRegistryStorage.sol";

/// @title DeviceRegistryFacet
/// @notice On-chain ENUMERABLE index of the devices linked to an
///         identity (a MAIN tokenId). The point: the UI reads the linked
///         set in ONE `eth_call` (`devicesOf`) instead of scraping
///         `SignerAdded` logs across the chain (which Tempo's RPC caps at
///         100k blocks anyway). The blockchain is the database — so keep
///         queryable state, not just events.
///
///         This is the DISPLAY/identity index. Actual signing AUTHORITY
///         still lives on the ERC-6551 TBA (`addSigner` / EIP-1271). The
///         pairing flow writes both (one sponsored tx). Only the MAIN's
///         current NFT owner can link/unlink.
contract DeviceRegistryFacet {
    event DeviceLinked(uint256 indexed mainId, address indexed device);
    event DeviceUnlinked(uint256 indexed mainId, address indexed device);

    error NotIdentityOwner();
    error ZeroDevice();

    function linkDevice(uint256 mainId, address device) external {
        _requireOwner(mainId);
        if (device == address(0)) revert ZeroDevice();
        LibDeviceRegistryStorage.Storage storage s = LibDeviceRegistryStorage.load();
        if (s.slot[mainId][device] != 0) return; // already linked (idempotent)
        s.devices[mainId].push(device);
        s.slot[mainId][device] = s.devices[mainId].length; // index + 1
        emit DeviceLinked(mainId, device);
    }

    function unlinkDevice(uint256 mainId, address device) external {
        _requireOwner(mainId);
        LibDeviceRegistryStorage.Storage storage s = LibDeviceRegistryStorage.load();
        uint256 idx1 = s.slot[mainId][device];
        if (idx1 == 0) return; // not linked
        uint256 i = idx1 - 1;
        address[] storage arr = s.devices[mainId];
        uint256 last = arr.length - 1;
        if (i != last) {
            address moved = arr[last];
            arr[i] = moved;
            s.slot[mainId][moved] = i + 1;
        }
        arr.pop();
        s.slot[mainId][device] = 0;
        emit DeviceUnlinked(mainId, device);
    }

    /// The identity's linked devices — one call, no log scraping.
    function devicesOf(uint256 mainId) external view returns (address[] memory) {
        return LibDeviceRegistryStorage.load().devices[mainId];
    }

    function isDeviceLinked(uint256 mainId, address device) external view returns (bool) {
        return LibDeviceRegistryStorage.load().slot[mainId][device] != 0;
    }

    /// Only the MAIN tokenId's current NFT holder may link/unlink. Resolved
    /// via a self-call to the registry facet's `ownerOfId` on this diamond.
    function _requireOwner(uint256 mainId) internal view {
        (bool ok, bytes memory ret) =
            address(this).staticcall(abi.encodeWithSignature("ownerOfId(uint256)", mainId));
        if (!ok || ret.length < 32) revert NotIdentityOwner();
        address owner = abi.decode(ret, (address));
        if (owner == address(0) || owner != msg.sender) revert NotIdentityOwner();
    }
}
