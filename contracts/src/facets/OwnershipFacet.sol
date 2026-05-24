// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {IERC173} from "../interfaces/IERC173.sol";
import {LibDiamond} from "../libraries/LibDiamond.sol";

/// @dev EIP-173 ownership facet — `owner()` + `transferOwnership`.
///      Backs LibDiamond's `enforceIsContractOwner` checks.
contract OwnershipFacet is IERC173 {
    function transferOwnership(address _newOwner) external override {
        LibDiamond.enforceIsContractOwner();
        LibDiamond.setContractOwner(_newOwner);
    }

    function owner() external view override returns (address owner_) {
        owner_ = LibDiamond.contractOwner();
    }
}
