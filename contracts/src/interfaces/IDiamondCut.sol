// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {IDiamond} from "./IDiamond.sol";

/// @dev EIP-2535: facet upgrade interface.
interface IDiamondCut is IDiamond {
    /// Add/replace/remove any number of functions and optionally
    /// execute a delegatecall on `_init` with `_calldata`. Typically
    /// `_init` is a one-shot initializer contract; pass `address(0)`
    /// and empty calldata if there's no initialisation.
    function diamondCut(
        FacetCut[] calldata _diamondCut,
        address _init,
        bytes calldata _calldata
    ) external;
}
