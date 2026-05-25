// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibDiamond} from "../libraries/LibDiamond.sol";
import {LibRegistryStorage} from "../libraries/LibRegistryStorage.sol";

/// @title WipeFacet
/// @notice Owner-only registry reset. Iterates 1..nextId and zeroes
///         out every per-token mapping in `LibRegistryStorage`, then
///         resets nextId to 1 so the next `register` mints token #1
///         again. Intended for testnet-only use — wipes ERC-721
///         ownership state alongside the registry's own bookkeeping.
///
///         Gas-bounded by `maxIds` so callers can chunk if the diamond
///         has grown past the per-tx limit. Pass `maxIds = 0` to wipe
///         everything in one call (fine for the small testnet state we
///         actually have).
contract WipeFacet {
    /// Emitted once per wipe call so off-chain indexers can pick it up.
    event RegistryWiped(uint256 from, uint256 to);

    function wipeRegistry(uint256 maxIds) external {
        LibDiamond.enforceIsContractOwner();
        LibRegistryStorage.Storage storage s = LibRegistryStorage.load();
        uint256 limit = s.nextId;
        if (maxIds != 0 && maxIds + 1 < limit) {
            limit = maxIds + 1;
        }
        for (uint256 i = 1; i < limit; i++) {
            string memory name = s.nameOfId[i];
            if (bytes(name).length > 0) {
                delete s.idOfName[name];
            }
            address prevOwner = s.ownerOfId[i];
            if (prevOwner != address(0) && s.balanceOf[prevOwner] > 0) {
                s.balanceOf[prevOwner] -= 1;
            }
            delete s.ownerOfId[i];
            delete s.nameOfId[i];
            delete s.tokenApprovals[i];
        }
        emit RegistryWiped(1, limit);
        // Only reset nextId when we've wiped everything; partial wipes
        // shouldn't make the counter lie about previously-allocated ids.
        if (limit == s.nextId) {
            s.nextId = 1;
        }
    }
}
