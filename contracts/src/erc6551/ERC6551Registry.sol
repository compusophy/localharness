// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {IERC6551Registry} from "./IERC6551Registry.sol";

/// @title ERC6551Registry
/// @notice EIP-6551 (token-bound accounts) v0.3.1 reference registry.
///         Deploys ERC-1167 minimal proxies (clones of
///         `implementation`) at CREATE2 addresses derived from
///         (implementation, salt, chainId, tokenContract, tokenId).
///
///         Source: github.com/erc6551/reference (MIT). Vendored
///         verbatim — DO NOT modify the bytecode below or the
///         address-derivation breaks compatibility with other 6551
///         consumers.
contract ERC6551Registry is IERC6551Registry {
    /// Initial bytecode used by all deployed accounts. Concatenated
    /// at runtime with: (implementation, salt, chainId, tokenContract,
    /// tokenId) as immutable args after the ERC-1167 clone code.
    bytes private constant ERC1167_HEADER =
        hex"3d60ad80600a3d3981f3363d3d373d3d3d363d73";
    bytes private constant ERC1167_FOOTER = hex"5af43d82803e903d91602b57fd5bf3";

    function createAccount(
        address implementation,
        bytes32 salt,
        uint256 chainId,
        address tokenContract,
        uint256 tokenId
    ) external returns (address) {
        bytes memory code = _creationCode(implementation, salt, chainId, tokenContract, tokenId);
        address acct = _computeAddress(salt, code);

        if (acct.code.length != 0) {
            // Already deployed — return as-is (idempotent).
            return acct;
        }

        address deployed;
        // solhint-disable-next-line no-inline-assembly
        assembly {
            deployed := create2(0, add(code, 0x20), mload(code), salt)
        }
        require(deployed == acct, "ERC6551Registry: deploy failed");

        emit ERC6551AccountCreated(
            deployed,
            implementation,
            salt,
            chainId,
            tokenContract,
            tokenId
        );
        return deployed;
    }

    function account(
        address implementation,
        bytes32 salt,
        uint256 chainId,
        address tokenContract,
        uint256 tokenId
    ) external view returns (address) {
        bytes memory code = _creationCode(implementation, salt, chainId, tokenContract, tokenId);
        return _computeAddress(salt, code);
    }

    function _creationCode(
        address implementation,
        bytes32 salt,
        uint256 chainId,
        address tokenContract,
        uint256 tokenId
    ) internal pure returns (bytes memory) {
        return
            abi.encodePacked(
                ERC1167_HEADER,
                implementation,
                ERC1167_FOOTER,
                abi.encode(salt, chainId, tokenContract, tokenId)
            );
    }

    function _computeAddress(bytes32 salt, bytes memory code) internal view returns (address) {
        bytes32 codeHash = keccak256(code);
        bytes32 raw = keccak256(abi.encodePacked(bytes1(0xff), address(this), salt, codeHash));
        return address(uint160(uint256(raw)));
    }
}
