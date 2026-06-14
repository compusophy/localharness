// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibDiamond} from "../libraries/LibDiamond.sol";
import {LibRegistryStorage} from "../libraries/LibRegistryStorage.sol";
import {LibMainIdentityStorage} from "../libraries/LibMainIdentityStorage.sol";
import {LibGuildStorage} from "../libraries/LibGuildStorage.sol";

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
///
///         Also hosts the diamond-owner (EIP-173) admin reset:
///         `adminBurnNames` / `adminResetAll` burn names regardless of
///         holder to wipe the registry to a clean slate on testnet.
contract ReleaseFacet {
    // Same topic as the ERC-721 Transfer; a burn is Transfer(owner, 0, id).
    event Transfer(address indexed from, address indexed to, uint256 indexed tokenId);
    event NameReleased(uint256 indexed tokenId, string name, address indexed formerOwner);

    error NotOwner();
    error CannotReleaseMain();
    error CannotReleaseGuild();

    function releaseName(uint256 tokenId) external {
        LibRegistryStorage.Storage storage s = LibRegistryStorage.load();
        address owner = s.ownerOfId[tokenId];
        if (owner != msg.sender) revert NotOwner();

        // Guard: never release your MAIN. Read the MAIN pointer DIRECTLY from
        // diamond storage rather than via a self-`staticcall` to
        // MainIdentityFacet — a cross-facet call returns ok=false (not a revert)
        // if that facet is ever cut out, which would silently BYPASS this guard.
        if (LibMainIdentityStorage.load().mainOf[msg.sender] == tokenId) {
            revert CannotReleaseMain();
        }

        // Guard: never release a GUILD identity. A guild is minted as a normal
        // identity NFT held by its founder (GuildFacet.createGuild), but its
        // treasury ($LH escrowed in the diamond, ledgered in guildBalance) and
        // membership rows live in LibGuildStorage, which `_burn` does NOT clear.
        // Burning it would zombie the guild and strand its funds forever. Force
        // the treasury to be drained + the guild wound down before release.
        if (LibGuildStorage.load().guilds[tokenId].exists) {
            revert CannotReleaseGuild();
        }

        _burn(s, tokenId, owner);
    }

    // --- admin reset (diamond owner / EIP-173, testnet clean slate) -------

    /// Diamond-owner-only force-burn of arbitrary names, regardless of who
    /// holds them. Bypasses the per-holder gate and the MAIN guard — this
    /// is the admin reset path for wiping the registry on testnet. Each
    /// burned name is freed for clean re-registration.
    function adminBurnNames(uint256[] calldata tokenIds) external {
        LibDiamond.enforceIsContractOwner();
        LibRegistryStorage.Storage storage s = LibRegistryStorage.load();
        for (uint256 i = 0; i < tokenIds.length; i++) {
            uint256 tokenId = tokenIds[i];
            address owner = s.ownerOfId[tokenId];
            if (owner == address(0)) continue; // already burned / never minted
            _burn(s, tokenId, owner);
        }
    }

    /// Diamond-owner-only nuke: burn every still-registered name in the
    /// `1..nextId` range for a complete clean slate. Gas-bounded — fine for
    /// testnet's small set; for a large set, page via `adminBurnNames`.
    function adminResetAll() external {
        LibDiamond.enforceIsContractOwner();
        LibRegistryStorage.Storage storage s = LibRegistryStorage.load();
        uint256 last = s.nextId;
        for (uint256 tokenId = 1; tokenId < last; tokenId++) {
            address owner = s.ownerOfId[tokenId];
            if (owner == address(0)) continue;
            _burn(s, tokenId, owner);
        }
    }

    // --- internal ---------------------------------------------------------

    /// Burn `tokenId` held by `owner` and clear EXACTLY the storage that
    /// `register()` writes so the name can be cleanly re-registered:
    /// name<->id mappings, ownerOfId, ERC-721 owner/balance/approval, and
    /// the MAIN pointer if this id is the holder's MAIN. (`isTaken`/`idOf`
    /// are derived — `isTaken(name)` reads `idOfName[name] != 0`, cleared
    /// here; `idOf` is the non-authoritative "most recent" back-compat
    /// pointer, left as-is like `releaseName` does.)
    function _burn(LibRegistryStorage.Storage storage s, uint256 tokenId, address owner) internal {
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

        // Clear the MAIN pointer if this id is the holder's MAIN, so the
        // burned name doesn't leave a dangling primary-identity reference.
        LibMainIdentityStorage.Storage storage ms = LibMainIdentityStorage.load();
        if (ms.mainOf[owner] == tokenId) {
            ms.mainOf[owner] = 0;
        }

        emit Transfer(owner, address(0), tokenId);
        emit NameReleased(tokenId, name, owner);
    }
}
