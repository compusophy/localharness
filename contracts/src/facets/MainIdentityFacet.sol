// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibDiamond} from "../libraries/LibDiamond.sol";
import {LibRegistryStorage} from "../libraries/LibRegistryStorage.sol";
import {LibMainIdentityStorage} from "../libraries/LibMainIdentityStorage.sol";
import {LibMainCostStorage} from "../libraries/LibMainCostStorage.sol";
import {LibCreditsStorage} from "../libraries/LibCreditsStorage.sol";

interface IERC20Min {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
}

/// @title MainIdentityFacet
/// @notice Records which of a holder's subdomain NFTs is their MAIN
///         identity. Auxiliary metadata, not enforcement — the on-chain
///         flag is descriptive ("this is the user's primary identity")
///         and is consumed by the bundle UX (MAIN badge in the agents
///         list, header pill, etc.) plus any downstream reputation
///         facet that wants to address the user-as-such.
///
///         Optional cost-gate: when `mainCost()` is non-zero, every
///         `registerMain` call pulls that much LH from the caller via
///         `transferFrom` into the diamond's treasury — sybil
///         deterrent that scales linearly with identity count. The
///         no-op branch (re-registering the same tokenId) skips the
///         charge so legitimate users don't pay for idempotent retries.
contract MainIdentityFacet {
    event MainRegistered(address indexed holder, uint256 indexed tokenId, string name);
    event MainCleared(address indexed holder, uint256 indexed tokenId);
    event MainCostUpdated(uint256 oldCostWei, uint256 newCostWei);

    error NotOwner(uint256 tokenId, address caller);
    error UnknownToken(uint256 tokenId);

    /// Declare `tokenId` (a registry NFT the caller owns) as the
    /// caller's MAIN. Idempotent — re-calling with the same tokenId
    /// is a no-op. Switching to a different owned tokenId silently
    /// replaces the previous MAIN.
    ///
    /// When `mainCost()` is non-zero AND the call actually changes
    /// state (i.e., the caller is moving to a new MAIN, not
    /// re-registering the existing one), pulls `costWei` LH from the
    /// caller into the diamond. No-op re-registers don't pay.
    function registerMain(uint256 tokenId) external {
        LibRegistryStorage.Storage storage rs = LibRegistryStorage.load();
        address owner = rs.ownerOfId[tokenId];
        if (owner == address(0)) revert UnknownToken(tokenId);
        if (owner != msg.sender) revert NotOwner(tokenId, msg.sender);

        LibMainIdentityStorage.Storage storage ms = LibMainIdentityStorage.load();
        if (ms.mainOf[msg.sender] == tokenId) {
            return; // no-op — skip the charge
        }
        ms.mainOf[msg.sender] = tokenId;
        emit MainRegistered(msg.sender, tokenId, rs.nameOfId[tokenId]);

        _chargeMainCost();
    }

    function _chargeMainCost() internal {
        uint256 costWei = LibMainCostStorage.load().costWei;
        if (costWei == 0) return;
        address creditsToken = LibCreditsStorage.load().creditsToken;
        if (creditsToken == address(0)) return;
        require(
            IERC20Min(creditsToken).transferFrom(msg.sender, address(this), costWei),
            "main: transfer failed"
        );
    }

    /// Owner-only setter for the MAIN registration cost. Zero disables
    /// the gate. Emitted event lets indexers track price changes.
    function setMainCost(uint256 newCostWei) external {
        LibDiamond.enforceIsContractOwner();
        LibMainCostStorage.Storage storage s = LibMainCostStorage.load();
        emit MainCostUpdated(s.costWei, newCostWei);
        s.costWei = newCostWei;
    }

    function mainCost() external view returns (uint256) {
        return LibMainCostStorage.load().costWei;
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
