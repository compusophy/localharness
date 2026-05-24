// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Minimal IERC721 surface — just `ownerOf` — for the account
///      to verify "is msg.sender the holder of my owning NFT?".
interface IERC721Min {
    function ownerOf(uint256 tokenId) external view returns (address);
}

interface IERC6551Account {
    receive() external payable;

    /// Returns the identifier of the NFT that owns this account.
    function token() external view returns (uint256 chainId, address tokenContract, uint256 tokenId);

    /// Returns the EOA that holds the owning NFT — i.e. the address
    /// allowed to call `execute` on this account.
    function owner() external view returns (address);

    /// Returns a monotonic state counter for replay protection.
    function state() external view returns (uint256);

    /// Returns true if `signer` is authorised to act for this account.
    function isValidSigner(address signer, bytes calldata context) external view returns (bytes4 magicValue);
}

interface IERC6551Executable {
    /// Operations: 0 = CALL, 1 = DELEGATECALL, 2 = CREATE, 3 = CREATE2.
    function execute(
        address to,
        uint256 value,
        bytes calldata data,
        uint8 operation
    ) external payable returns (bytes memory);
}

/// @title ERC6551Account
/// @notice EIP-6551 reference token-bound account (CALL-only variant).
///         Minimal: receive funds, expose token(), let the owning
///         NFT's holder execute arbitrary CALL ops. DELEGATECALL is
///         disabled because misuse self-destructs the account.
///
///         The owning-NFT identifier is read from immutable args
///         appended to the proxy's bytecode by the registry, so this
///         contract holds zero state of its own beyond a state nonce.
contract ERC6551Account is IERC6551Account, IERC6551Executable {
    uint256 private _state;

    receive() external payable override {}

    function execute(
        address to,
        uint256 value,
        bytes calldata data,
        uint8 operation
    ) external payable override returns (bytes memory result) {
        require(_isValidSigner(msg.sender), "ERC6551: not authorised");
        require(operation == 0, "ERC6551: only CALL supported");
        _state++;
        bool ok;
        (ok, result) = to.call{value: value}(data);
        if (!ok) {
            // bubble the underlying revert reason up
            assembly {
                revert(add(result, 32), mload(result))
            }
        }
    }

    function token()
        public
        view
        override
        returns (uint256 chainId, address tokenContract, uint256 tokenId)
    {
        bytes memory footer = new bytes(0x60);
        // The registry appends `abi.encode(salt, chainId, tokenContract, tokenId)`
        // after the ERC-1167 clone. The clone's bytecode is exactly 45 bytes
        // (0x2d). Skip those + the 32-byte salt to land on the
        // (chainId, tokenContract, tokenId) tuple = 96 bytes.
        assembly {
            extcodecopy(address(), add(footer, 0x20), 0x4d, 0x60)
        }
        (chainId, tokenContract, tokenId) = abi.decode(footer, (uint256, address, uint256));
    }

    function owner() public view override returns (address) {
        (uint256 chainId, address tokenContract, uint256 tokenId) = token();
        if (chainId != block.chainid) {
            return address(0);
        }
        return IERC721Min(tokenContract).ownerOf(tokenId);
    }

    function state() external view override returns (uint256) {
        return _state;
    }

    function isValidSigner(address signer, bytes calldata)
        external
        view
        override
        returns (bytes4)
    {
        if (_isValidSigner(signer)) {
            // ERC-6551 magic value for "authorised signer".
            return 0x523e3260;
        }
        return 0xffffffff;
    }

    function _isValidSigner(address signer) internal view returns (bool) {
        return signer == owner();
    }
}
