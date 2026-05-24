// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibRegistryStorage} from "../libraries/LibRegistryStorage.sol";

/// @title ERC721Facet
/// @notice ERC-721 conformance over the registry's existing
///         tokenId == agentId mapping. Mints happen in
///         LocalharnessRegistryFacet.register (emits the (0, owner,
///         tokenId) Transfer there); this facet owns the standard
///         post-mint surface: balanceOf / ownerOf / approve /
///         transferFrom / safeTransferFrom + the Metadata extension.
///
///         Storage is shared with the registry facet via
///         `LibRegistryStorage` — same diamond storage slot. ERC-721
///         fields were appended to the struct so the layout stays
///         backward-compatible.
///
///         Permissionless ERC-6551 deployments automatically derive
///         a token-bound account for every tokenId here, so each
///         registered name gets its own EVM wallet "for free."
interface IERC721Receiver {
    function onERC721Received(
        address operator,
        address from,
        uint256 tokenId,
        bytes calldata data
    ) external returns (bytes4);
}

contract ERC721Facet {
    event Transfer(address indexed from, address indexed to, uint256 indexed tokenId);
    event Approval(address indexed owner, address indexed approved, uint256 indexed tokenId);
    event ApprovalForAll(address indexed owner, address indexed operator, bool approved);

    // --- ERC-721 core ---------------------------------------------------

    function balanceOf(address owner) external view returns (uint256) {
        require(owner != address(0), "ERC721: zero owner");
        return LibRegistryStorage.load().balanceOf[owner];
    }

    function ownerOf(uint256 tokenId) external view returns (address) {
        address o = LibRegistryStorage.load().ownerOfId[tokenId];
        require(o != address(0), "ERC721: nonexistent token");
        return o;
    }

    function approve(address to, uint256 tokenId) external {
        LibRegistryStorage.Storage storage s = LibRegistryStorage.load();
        address owner = s.ownerOfId[tokenId];
        require(owner != address(0), "ERC721: nonexistent token");
        require(
            owner == msg.sender || s.operatorApprovals[owner][msg.sender],
            "ERC721: not approved"
        );
        s.tokenApprovals[tokenId] = to;
        emit Approval(owner, to, tokenId);
    }

    function getApproved(uint256 tokenId) external view returns (address) {
        LibRegistryStorage.Storage storage s = LibRegistryStorage.load();
        require(s.ownerOfId[tokenId] != address(0), "ERC721: nonexistent token");
        return s.tokenApprovals[tokenId];
    }

    function setApprovalForAll(address operator, bool approved) external {
        require(operator != msg.sender, "ERC721: approve to caller");
        LibRegistryStorage.load().operatorApprovals[msg.sender][operator] = approved;
        emit ApprovalForAll(msg.sender, operator, approved);
    }

    function isApprovedForAll(address owner, address operator) external view returns (bool) {
        return LibRegistryStorage.load().operatorApprovals[owner][operator];
    }

    function transferFrom(address from, address to, uint256 tokenId) public {
        _transfer(from, to, tokenId);
    }

    function safeTransferFrom(address from, address to, uint256 tokenId) external {
        _safeTransfer(from, to, tokenId, "");
    }

    function safeTransferFrom(
        address from,
        address to,
        uint256 tokenId,
        bytes calldata data
    ) external {
        _safeTransfer(from, to, tokenId, data);
    }

    // --- ERC-721 Metadata extension --------------------------------------

    function name() external pure returns (string memory) {
        return "Localharness Names";
    }

    function symbol() external pure returns (string memory) {
        return "LH";
    }

    /// `tokenURI(tokenId)` returns a stable HTTPS URL pointing at the
    /// agent's public profile on the apex. Off-chain renderers (block
    /// explorers, NFT galleries) can fetch this for metadata. Returning
    /// empty string for nonexistent tokens matches OZ behaviour.
    function tokenURI(uint256 tokenId) external view returns (string memory) {
        LibRegistryStorage.Storage storage s = LibRegistryStorage.load();
        string memory name_ = s.nameOfId[tokenId];
        if (bytes(name_).length == 0) {
            return "";
        }
        return string(abi.encodePacked("https://", name_, ".localharness.xyz/"));
    }

    // --- internals -------------------------------------------------------

    function _transfer(address from, address to, uint256 tokenId) internal {
        LibRegistryStorage.Storage storage s = LibRegistryStorage.load();
        address owner = s.ownerOfId[tokenId];
        require(owner == from, "ERC721: wrong from");
        require(to != address(0), "ERC721: zero to");

        bool authorised = msg.sender == owner
            || s.operatorApprovals[owner][msg.sender]
            || s.tokenApprovals[tokenId] == msg.sender;
        require(authorised, "ERC721: not approved");

        // Clear per-token approval before transfer (ERC-721 spec).
        delete s.tokenApprovals[tokenId];
        s.balanceOf[from] -= 1;
        s.balanceOf[to] += 1;
        s.ownerOfId[tokenId] = to;
        // Update the "most recent" reverse pointer for the recipient;
        // the sender's stays pointing at a token they no longer own
        // (the field's just a convenience hint — read `ownerOfId` for
        // authoritative answers).
        s.idOf[to] = tokenId;

        emit Transfer(from, to, tokenId);
    }

    function _safeTransfer(
        address from,
        address to,
        uint256 tokenId,
        bytes memory data
    ) internal {
        _transfer(from, to, tokenId);
        if (to.code.length > 0) {
            bytes4 retval = IERC721Receiver(to).onERC721Received(msg.sender, from, tokenId, data);
            require(
                retval == IERC721Receiver.onERC721Received.selector,
                "ERC721: receiver rejected"
            );
        }
    }
}
