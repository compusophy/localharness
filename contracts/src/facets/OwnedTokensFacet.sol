// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibOwnedTokensStorage} from "../libraries/LibOwnedTokensStorage.sol";
import {LibRegistryStorage} from "../libraries/LibRegistryStorage.sol";
import {LibDiamond} from "../libraries/LibDiamond.sol";

/// @title OwnedTokensFacet
/// @notice On-chain ENUMERABLE owner -> tokenIds index — the "owner address to
///         subdomains" key/value the UI was missing. Without it,
///         `registry::list_owned_tokens` must scan `ownerOfId(1..nextId)` —
///         O(total supply), because the only on-chain maps are id->owner plus a
///         `balanceOf` count (a number, not a list). This facet returns an
///         owner's tokens in ONE call, O(holdings). Same philosophy as
///         DeviceRegistryFacet's `devicesOf`: keep queryable STATE, not a scan.
///
///         === CUTTING IT (diamond owner; mirror script/AddTbaFacet.s.sol) ===
///           1. forge create / deploy OwnedTokensFacet
///           2. diamondCut Add the selectors:
///                tokensOfOwner(address)
///                tokensOfOwnerDetailed(address)
///                ownedCount(address)
///                rebuildOwnedIndex(uint256,uint256)
///           3. call rebuildOwnedIndex(1, nextId()) ONCE to backfill the
///              existing tokens. Idempotent; split into ranges if a single tx
///              is gas-heavy (e.g. (1,500),(500,1000)…).
///
///         === REAL-TIME MAINTENANCE (optional, after the backfill) ===
///         So the index stays correct on new mint/transfer/burn WITHOUT
///         re-running rebuild, add one line to each ownership mutation in the
///         facets that already write `ownerOfId` (share the bodies via a small
///         internal lib, or inline the 3 lines — they touch only
///         LibOwnedTokensStorage):
///           - LocalharnessRegistryFacet.register()  -> _add(msg.sender, agentId)
///           - ERC721Facet._transfer(from,to,id)     -> _remove(id); _add(to,id)
///           - ReleaseFacet burn/_burn(id)            -> _remove(id)
///         Until those land, the index is still usable — just call
///         rebuildOwnedIndex after a batch of registrations. The view is
///         O(holdings) regardless.
contract OwnedTokensFacet {
    /// An owner's tokenIds — one call, no scan. May be empty.
    function tokensOfOwner(address owner) external view returns (uint256[] memory) {
        return LibOwnedTokensStorage.load().owned[owner];
    }

    /// An owner's tokenIds AND their names in a single call (names read from the
    /// registry storage), so the client needs ZERO follow-up reads to render the
    /// agent list. This is the call `list_owned_tokens` should prefer when the
    /// facet is present.
    function tokensOfOwnerDetailed(address owner)
        external
        view
        returns (uint256[] memory ids, string[] memory names)
    {
        ids = LibOwnedTokensStorage.load().owned[owner];
        LibRegistryStorage.Storage storage r = LibRegistryStorage.load();
        names = new string[](ids.length);
        for (uint256 i = 0; i < ids.length; i++) {
            names[i] = r.nameOfId[ids[i]];
        }
    }

    function ownedCount(address owner) external view returns (uint256) {
        return LibOwnedTokensStorage.load().owned[owner].length;
    }

    /// Diamond-owner-only backfill/repair: index every still-owned tokenId in
    /// [fromId, toId) by reading the registry's current `ownerOfId`. Idempotent
    /// (skips ids already indexed via the `indexOf` guard), so it's safe to
    /// re-run and to run in ranges. Run once as rebuildOwnedIndex(1, nextId)
    /// after cutting. NOTE: this backfills the CURRENT owner; if tokens have
    /// been transferred since a prior index without the real-time hooks above,
    /// wire the hooks (which do remove+add) rather than relying on rebuild to
    /// move a stale entry.
    function rebuildOwnedIndex(uint256 fromId, uint256 toId) external {
        LibDiamond.enforceIsContractOwner();
        LibRegistryStorage.Storage storage r = LibRegistryStorage.load();
        LibOwnedTokensStorage.Storage storage s = LibOwnedTokensStorage.load();
        for (uint256 id = fromId; id < toId; id++) {
            if (s.indexOf[id] != 0) continue; // already indexed
            address owner = r.ownerOfId[id];
            if (owner == address(0)) continue; // unminted / burned
            s.owned[owner].push(id);
            s.indexOf[id] = s.owned[owner].length; // store index + 1
        }
    }
}
