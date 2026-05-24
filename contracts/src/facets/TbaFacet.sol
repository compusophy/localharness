// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibDiamond} from "../libraries/LibDiamond.sol";
import {LibTbaConfigStorage} from "../libraries/LibTbaConfigStorage.sol";
import {LibRegistryStorage} from "../libraries/LibRegistryStorage.sol";
import {IERC6551Registry} from "../erc6551/IERC6551Registry.sol";

/// @title TbaFacet
/// @notice Diamond facet that wraps EIP-6551 so the token-bound
///         account for any registered name is a single call away.
///
///         Every name minted by `LocalharnessRegistryFacet.register`
///         gets a deterministic, counterfactual smart-contract
///         account at the address returned by `tokenBoundAccount`.
///         The account is the agent's wallet — can hold tokens,
///         sign messages, settle x402/MPP payments. Anyone may call
///         `createTokenBoundAccount` to actually deploy the account
///         the first time it's needed.
contract TbaFacet {
    /// Owner-only one-time setter for the registry + account-impl
    /// addresses. Both are deployed once and never change.
    function setTbaConfig(address registry, address accountImpl) external {
        LibDiamond.enforceIsContractOwner();
        LibTbaConfigStorage.Storage storage s = LibTbaConfigStorage.load();
        s.registry = registry;
        s.accountImpl = accountImpl;
    }

    function tbaRegistry() external view returns (address) {
        return LibTbaConfigStorage.load().registry;
    }

    function tbaAccountImpl() external view returns (address) {
        return LibTbaConfigStorage.load().accountImpl;
    }

    /// Deterministic address of the token-bound account for `tokenId`.
    /// Reverts if the name isn't registered. Always returns a value,
    /// whether the account has been deployed yet or not (counterfactual).
    function tokenBoundAccount(uint256 tokenId) external view returns (address) {
        LibRegistryStorage.Storage storage rs = LibRegistryStorage.load();
        require(rs.ownerOfId[tokenId] != address(0), "TBA: nonexistent token");
        LibTbaConfigStorage.Storage storage cs = LibTbaConfigStorage.load();
        require(cs.registry != address(0), "TBA: registry unset");
        return
            IERC6551Registry(cs.registry).account(
                cs.accountImpl,
                bytes32(0),
                block.chainid,
                address(this),
                tokenId
            );
    }

    /// Same lookup but by name — convenience for the apex chrome.
    function tokenBoundAccountByName(string calldata name) external view returns (address) {
        LibRegistryStorage.Storage storage rs = LibRegistryStorage.load();
        uint256 id = rs.idOfName[name];
        require(id != 0, "TBA: name unregistered");
        LibTbaConfigStorage.Storage storage cs = LibTbaConfigStorage.load();
        require(cs.registry != address(0), "TBA: registry unset");
        return
            IERC6551Registry(cs.registry).account(
                cs.accountImpl,
                bytes32(0),
                block.chainid,
                address(this),
                id
            );
    }

    /// Actually deploy the token-bound account at its precomputed
    /// address. Idempotent — re-call returns the same address.
    /// Anyone may call (the ERC-6551 registry doesn't gate creation).
    function createTokenBoundAccount(uint256 tokenId) external returns (address) {
        LibRegistryStorage.Storage storage rs = LibRegistryStorage.load();
        require(rs.ownerOfId[tokenId] != address(0), "TBA: nonexistent token");
        LibTbaConfigStorage.Storage storage cs = LibTbaConfigStorage.load();
        require(cs.registry != address(0), "TBA: registry unset");
        return
            IERC6551Registry(cs.registry).createAccount(
                cs.accountImpl,
                bytes32(0),
                block.chainid,
                address(this),
                tokenId
            );
    }
}
