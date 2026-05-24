// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibDiamond} from "../libraries/LibDiamond.sol";
import {LibRegistryStorage} from "../libraries/LibRegistryStorage.sol";
import {IDiamondCut} from "../interfaces/IDiamondCut.sol";
import {IDiamondLoupe} from "../interfaces/IDiamondLoupe.sol";
import {IERC165} from "../interfaces/IERC165.sol";
import {IERC173} from "../interfaces/IERC173.sol";

/// @dev One-shot initialiser. Called via `delegatecall` from
///      `LibDiamond.diamondCut` during the constructor cut. Sets
///      ERC-165 flags + seeds registry state (nextId = 1, since 0 is
///      the "unregistered" sentinel in `idOfName`).
contract DiamondInit {
    function init() external {
        LibDiamond.DiamondStorage storage ds = LibDiamond.diamondStorage();
        ds.supportedInterfaces[type(IERC165).interfaceId] = true;
        ds.supportedInterfaces[type(IDiamondCut).interfaceId] = true;
        ds.supportedInterfaces[type(IDiamondLoupe).interfaceId] = true;
        ds.supportedInterfaces[type(IERC173).interfaceId] = true;

        LibRegistryStorage.Storage storage rs = LibRegistryStorage.load();
        if (rs.nextId == 0) {
            rs.nextId = 1;
        }
    }
}
