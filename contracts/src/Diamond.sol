// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibDiamond} from "./libraries/LibDiamond.sol";
import {IDiamondCut} from "./interfaces/IDiamondCut.sol";
import {IDiamond} from "./interfaces/IDiamond.sol";

/// @dev EIP-2535 Diamond proxy. All external calls fall through to
///      whichever facet implements the called selector. The cut +
///      loupe + ownership facets are installed at construction.
///
///      Reference: github.com/mudgen/diamond-3-hardhat (MIT).
contract Diamond {
    constructor(address _contractOwner, IDiamond.FacetCut[] memory _diamondCut) payable {
        LibDiamond.setContractOwner(_contractOwner);
        LibDiamond.diamondCut(_diamondCut, address(0), "");
    }

    /// Fallback dispatches by selector to the facet that owns it.
    /// Delegatecall keeps the diamond's storage as the persistent layer.
    // solhint-disable-next-line no-complex-fallback
    fallback() external payable {
        LibDiamond.DiamondStorage storage ds;
        bytes32 position = LibDiamond.DIAMOND_STORAGE_POSITION;
        assembly {
            ds.slot := position
        }
        address facet = ds.selectorToFacetAndPosition[msg.sig].facetAddress;
        require(facet != address(0), "Diamond: function not found");
        assembly {
            calldatacopy(0, 0, calldatasize())
            let result := delegatecall(gas(), facet, 0, calldatasize(), 0, 0)
            returndatacopy(0, 0, returndatasize())
            switch result
            case 0 {
                revert(0, returndatasize())
            }
            default {
                return(0, returndatasize())
            }
        }
    }

    receive() external payable {}
}
