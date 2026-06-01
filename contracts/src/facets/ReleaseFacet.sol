// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibRegistryStorage} from "../libraries/LibRegistryStorage.sol";

/// @title ReleaseFacet
/// @notice Recycle a subdomain: the owner gives it up — the NFT is burned
///         and the name is freed for re-registration. The lifecycle piece
///         that was missing (you could register but never release).
///
///         Owner-only. REFUSES your MAIN — releasing it would orphan your
///         identity and any assets consolidated into its TBA. (Other
///         subdomains: the caller confirms via typed confirmation in the
///         UI; if a released name's TBA holds assets they become
///         unreachable, so the UI warns.)
contract ReleaseFacet {
    // Same topic as the ERC-721 Transfer; a burn is Transfer(owner, 0, id).
    event Transfer(address indexed from, address indexed to, uint256 indexed tokenId);
    event NameReleased(uint256 indexed tokenId, string name, address indexed formerOwner);

    error NotOwner();
    error CannotReleaseMain();

    function releaseName(uint256 tokenId) external {
        LibRegistryStorage.Storage storage s = LibRegistryStorage.load();
        address owner = s.ownerOfId[tokenId];
        if (owner != msg.sender) revert NotOwner();

        // Guard: never release your MAIN (self-call to MainIdentityFacet).
        (bool ok, bytes memory ret) =
            address(this).staticcall(abi.encodeWithSignature("mainOf(address)", msg.sender));
        if (ok && ret.length >= 32 && abi.decode(ret, (uint256)) == tokenId) {
            revert CannotReleaseMain();
        }

        string memory name = s.nameOfId[tokenId];
        // Burn the NFT.
        s.ownerOfId[tokenId] = address(0);
        if (s.balanceOf[owner] > 0) {
            s.balanceOf[owner] -= 1;
        }
        delete s.tokenApprovals[tokenId];
        // Free the name for re-registration (a fresh register mints a new id).
        if (bytes(name).length > 0) {
            s.idOfName[name] = 0;
            delete s.nameOfId[tokenId];
        }

        emit Transfer(owner, address(0), tokenId);
        emit NameReleased(tokenId, name, owner);
    }
}
