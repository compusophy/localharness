// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Minimal IERC721 surface — just `ownerOf` — for the account
///      to verify "is msg.sender the holder of my owning NFT?".
interface IERC721Min {
    function ownerOf(uint256 tokenId) external view returns (address);
}

interface IERC6551Account {
    receive() external payable;
    function token() external view returns (uint256 chainId, address tokenContract, uint256 tokenId);
    function owner() external view returns (address);
    function state() external view returns (uint256);
    function isValidSigner(address signer, bytes calldata context) external view returns (bytes4 magicValue);
}

interface IERC6551Executable {
    function execute(
        address to,
        uint256 value,
        bytes calldata data,
        uint8 operation
    ) external payable returns (bytes memory);
}

interface IERC1271 {
    function isValidSignature(bytes32 hash, bytes memory signature) external view returns (bytes4 magicValue);
}

/// @title MultiSignerAccount
/// @notice ERC-6551 account impl with an authorized-signer set on top
///         of the NFT holder. The NFT holder is ALWAYS implicitly
///         authorized; additional signers (device EOAs) can be added
///         via `addSigner` from any already-authorized address. Lets
///         a user link multiple devices to the same on-chain identity
///         without sharing the master seed — each device holds its own
///         key, and the MAIN's TBA recognises all of them.
///
///         The "owner" returned by `owner()` is still the NFT holder
///         (per the ERC-6551 contract); the extra signers are surfaced
///         via `isAuthorizedSigner` / `isValidSigner` / `isValidSignature`.
///         `execute` and signer-management ops accept any authorized
///         signer as `msg.sender`.
///
///         CALL-only — DELEGATECALL is explicitly disabled to avoid
///         the self-destruct footgun (same as the vanilla impl). Storage
///         lives in the clone's slots, not the impl's, so each TBA
///         maintains its own independent signer set.
///
///         Caveat — NFT transfer does NOT clear additional signers.
///         Acceptable on testnet; revisit before mainnet.
contract MultiSignerAccount is IERC6551Account, IERC6551Executable, IERC1271 {
    uint256 private _state;
    mapping(address => bool) private _authorizedSigners;

    event SignerAdded(address indexed signer, address indexed addedBy);
    event SignerRemoved(address indexed signer, address indexed removedBy);

    receive() external payable override {}

    function execute(
        address to,
        uint256 value,
        bytes calldata data,
        uint8 operation
    ) external payable override returns (bytes memory result) {
        require(_isAuthorized(msg.sender), "MultiSigner: not authorised");
        require(operation == 0, "MultiSigner: only CALL supported");
        _state++;
        bool ok;
        (ok, result) = to.call{value: value}(data);
        if (!ok) {
            // Bubble underlying revert reason up.
            assembly {
                revert(add(result, 32), mload(result))
            }
        }
    }

    /// Add `signer` to the authorized set. Callable by any already-
    /// authorized address (NFT owner or an existing additional signer).
    /// Idempotent — re-adding is a no-op.
    function addSigner(address signer) external {
        require(_isAuthorized(msg.sender), "MultiSigner: not authorised");
        require(signer != address(0), "MultiSigner: zero address");
        if (!_authorizedSigners[signer]) {
            _authorizedSigners[signer] = true;
            _state++;
            emit SignerAdded(signer, msg.sender);
        }
    }

    /// Remove `signer` from the additional-signer set. The NFT holder
    /// CANNOT be removed via this path (they're authorized implicitly
    /// by holding the NFT — transfer the NFT to revoke). Callable by
    /// any authorized address.
    function removeSigner(address signer) external {
        require(_isAuthorized(msg.sender), "MultiSigner: not authorised");
        if (_authorizedSigners[signer]) {
            _authorizedSigners[signer] = false;
            _state++;
            emit SignerRemoved(signer, msg.sender);
        }
    }

    function isAuthorizedSigner(address signer) external view returns (bool) {
        return _isAuthorized(signer);
    }

    function _isAuthorized(address signer) internal view returns (bool) {
        if (signer == address(0)) return false;
        return signer == owner() || _authorizedSigners[signer];
    }

    function token()
        public
        view
        override
        returns (uint256 chainId, address tokenContract, uint256 tokenId)
    {
        bytes memory footer = new bytes(0x60);
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
        if (_isAuthorized(signer)) {
            // ERC-6551 magic value for "authorised signer".
            return 0x523e3260;
        }
        return 0xffffffff;
    }

    /// EIP-1271 signature validation. Recovers the address from
    /// `(hash, signature)` and returns the magic value iff the
    /// recovered address is authorized. Lets off-chain protocols
    /// verify that a signature "comes from" the MAIN identity by
    /// asking the TBA contract, regardless of which device signed.
    function isValidSignature(bytes32 hash, bytes memory signature)
        external
        view
        override
        returns (bytes4)
    {
        if (signature.length != 65) {
            return 0xffffffff;
        }
        bytes32 r;
        bytes32 s;
        uint8 v;
        assembly {
            r := mload(add(signature, 0x20))
            s := mload(add(signature, 0x40))
            v := byte(0, mload(add(signature, 0x60)))
        }
        if (v < 27) {
            v += 27;
        }
        address recovered = ecrecover(hash, v, r, s);
        if (recovered != address(0) && _isAuthorized(recovered)) {
            // EIP-1271 magic value.
            return 0x1626ba7e;
        }
        return 0xffffffff;
    }
}
