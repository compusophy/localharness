// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

interface IERC6551Registry {
    /// Emitted when a token-bound account is created.
    event ERC6551AccountCreated(
        address account,
        address indexed implementation,
        bytes32 salt,
        uint256 chainId,
        address indexed tokenContract,
        uint256 indexed tokenId
    );

    /// Creates a token-bound account for the given token. Reverts if
    /// creation fails (CREATE2 collision). Idempotent: re-call returns
    /// the same already-deployed address.
    function createAccount(
        address implementation,
        bytes32 salt,
        uint256 chainId,
        address tokenContract,
        uint256 tokenId
    ) external returns (address account);

    /// Computes the deterministic account address for the given token.
    /// Does not check whether the account is actually deployed yet.
    function account(
        address implementation,
        bytes32 salt,
        uint256 chainId,
        address tokenContract,
        uint256 tokenId
    ) external view returns (address account);
}
