// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev EIP-165 standard interface detection.
interface IERC165 {
    function supportsInterface(bytes4 interfaceId) external view returns (bool);
}
