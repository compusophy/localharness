// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibRegistryStorage} from "../libraries/LibRegistryStorage.sol";
import {LibMainIdentityStorage} from "../libraries/LibMainIdentityStorage.sol";

/// @title MainIdentityFacet
/// @notice Records which of a holder's subdomain NFTs is their MAIN
///         identity. Auxiliary metadata, not enforcement — the on-chain
///         flag is descriptive ("this is the user's primary identity")
///         and is consumed by the bundle UX (MAIN badge in the agents
///         list, header pill, etc.) plus any downstream reputation
///         facet that wants to address the user-as-such.
///
///         No fee / lock yet. Sybil resistance (cost-locked MAIN,
///         reputation-bound MAIN) is the next layer; see
///         `design/main-identity.md`. This facet just establishes the
///         primitive so the bundle can start surfacing it.
contract MainIdentityFacet {
    event MainRegistered(address indexed holder, uint256 indexed tokenId, string name);
    event MainCleared(address indexed holder, uint256 indexed tokenId);

    error NotOwner(uint256 tokenId, address caller);
    error UnknownToken(uint256 tokenId);

    /// Declare `tokenId` (a registry NFT the caller owns) as the
    /// caller's MAIN. Idempotent — re-calling with the same tokenId
    /// is a no-op. Switching to a different owned tokenId silently
    /// replaces the previous MAIN.
    function registerMain(uint256 tokenId) external {
        LibRegistryStorage.Storage storage rs = LibRegistryStorage.load();
        address owner = rs.ownerOfId[tokenId];
        if (owner == address(0)) revert UnknownToken(tokenId);
        if (owner != msg.sender) revert NotOwner(tokenId, msg.sender);

        LibMainIdentityStorage.Storage storage ms = LibMainIdentityStorage.load();
        if (ms.mainOf[msg.sender] == tokenId) {
            return; // no-op
        }
        ms.mainOf[msg.sender] = tokenId;
        emit MainRegistered(msg.sender, tokenId, rs.nameOfId[tokenId]);
    }

    /// Clear the caller's MAIN flag. Doesn't burn the NFT; just
    /// removes the "this is my primary" pointer. Subsequent
    /// `registerMain` calls can set a new one.
    function clearMain() external {
        LibMainIdentityStorage.Storage storage ms = LibMainIdentityStorage.load();
        uint256 prev = ms.mainOf[msg.sender];
        if (prev == 0) return;
        ms.mainOf[msg.sender] = 0;
        emit MainCleared(msg.sender, prev);
    }

    // --- views -----------------------------------------------------------

    function mainOf(address holder) external view returns (uint256) {
        return LibMainIdentityStorage.load().mainOf[holder];
    }

    /// Convenience: return the MAIN's name string for direct UI use.
    /// Empty string when the holder has no MAIN registered.
    function mainNameOf(address holder) external view returns (string memory) {
        uint256 id = LibMainIdentityStorage.load().mainOf[holder];
        if (id == 0) return "";
        return LibRegistryStorage.load().nameOfId[id];
    }

    /// `true` iff `tokenId` is the registered MAIN for its current
    /// owner. Auto-invalidates on transfer (we look up
    /// `ownerOfId[tokenId]` fresh, then compare to `mainOf[owner]`).
    function isMain(uint256 tokenId) external view returns (bool) {
        LibRegistryStorage.Storage storage rs = LibRegistryStorage.load();
        address owner = rs.ownerOfId[tokenId];
        if (owner == address(0)) return false;
        return LibMainIdentityStorage.load().mainOf[owner] == tokenId;
    }
}
