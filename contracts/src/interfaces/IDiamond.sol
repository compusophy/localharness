// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev EIP-2535 Diamond Standard: shared types + DiamondCut event.
/// Reference: github.com/mudgen/diamond-3-hardhat (MIT).
interface IDiamond {
    enum FacetCutAction {
        Add,
        Replace,
        Remove
    }

    struct FacetCut {
        address facetAddress;
        FacetCutAction action;
        bytes4[] functionSelectors;
    }

    event DiamondCut(FacetCut[] _diamondCut, address _init, bytes _calldata);
}
