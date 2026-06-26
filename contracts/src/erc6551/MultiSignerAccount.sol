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
///         authorized. Additional signers (device EOAs) let a user link
///         multiple devices to the same on-chain identity without
///         sharing the master seed — each device holds its own key and
///         the MAIN's TBA recognises all of them.
///
///         SECURITY — additional signers are bound to the holder that
///         enrolled them. Signer management (`addSigner` / `removeSigner`)
///         is restricted to the current NFT holder (`owner()`), and a
///         signer is authorized ONLY while its enroller still holds the
///         NFT. So when the NFT changes hands the previous holder's device
///         signers go dormant automatically (no cleanup call, no storage
///         iteration), and they can't re-enroll themselves — only the new
///         holder can. This closes the "stale signers survive NFT
///         transfer" and "any signer can add signers" holes.
///
///         The "owner" returned by `owner()` is still the NFT holder
///         (per the ERC-6551 contract); the extra signers are surfaced
///         via `isAuthorizedSigner` / `isValidSigner` / `isValidSignature`.
///         `execute` accepts any currently-authorized signer.
///
///         CALL-only — DELEGATECALL is explicitly disabled to avoid
///         the self-destruct footgun (same as the vanilla impl). Storage
///         lives in the clone's slots, not the impl's, so each TBA
///         maintains its own independent signer set.
contract MultiSignerAccount is IERC6551Account, IERC6551Executable, IERC1271 {
    /// secp256k1 group order / 2 — the EIP-2 low-s ceiling. Signatures
    /// with `s` above this are malleable and rejected by `isValidSignature`.
    uint256 private constant _HALF_ORDER =
        0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0;

    uint256 private _state;
    /// The NFT holder that enrolled each additional signer. A signer is
    /// authorized ONLY while its enroller is still the current `owner()`,
    /// so an NFT transfer transparently invalidates the previous holder's
    /// device signers (no storage iteration, no cleanup call) — and the
    /// new holder must enroll their own. A signer enrolled by holder X
    /// becomes dormant the moment X stops holding the NFT and is only ever
    /// live again if X re-acquires it (i.e. it always tracks the current
    /// holder's own enrollments, never a former holder's).
    mapping(address => address) private _signerEnroller;

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

    /// Add `signer` to the authorized set. Restricted to the current NFT
    /// holder — a linked device cannot enroll further devices, and a past
    /// holder can't act after transfer. Idempotent.
    function addSigner(address signer) external {
        address holder = owner();
        require(msg.sender == holder, "MultiSigner: only owner");
        require(signer != address(0), "MultiSigner: zero address");
        if (_signerEnroller[signer] != holder) {
            _signerEnroller[signer] = holder;
            _state++;
            emit SignerAdded(signer, msg.sender);
        }
    }

    /// Remove `signer` enrolled by the current holder. Restricted to the
    /// current NFT holder. The holder themselves can't be removed (they're
    /// authorized implicitly by holding the NFT — transfer it to revoke).
    function removeSigner(address signer) external {
        address holder = owner();
        require(msg.sender == holder, "MultiSigner: only owner");
        if (_signerEnroller[signer] == holder) {
            _signerEnroller[signer] = address(0);
            _state++;
            emit SignerRemoved(signer, msg.sender);
        }
    }

    function isAuthorizedSigner(address signer) external view returns (bool) {
        return _isAuthorized(signer);
    }

    function _isAuthorized(address signer) internal view returns (bool) {
        if (signer == address(0)) return false;
        address holder = owner();
        // Fail CLOSED when there is no holder — `owner()` returns the zero
        // address on a chainId mismatch (the clone running on the wrong chain),
        // and an unenrolled signer's `_signerEnroller` slot is also zero, so
        // without this guard `_signerEnroller[signer] == holder` (0 == 0) would
        // spuriously authorize ANY unenrolled signer.
        if (holder == address(0)) return false;
        if (signer == holder) return true;
        // An additional signer is authorized only while the holder who
        // enrolled it still holds the NFT — so a transfer silently revokes
        // the previous holder's signers.
        return _signerEnroller[signer] == holder;
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

    function owner() public view virtual override returns (address) {
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
        // Reject malleable (high-s) signatures — EIP-2.
        if (uint256(s) > _HALF_ORDER) {
            return 0xffffffff;
        }
        if (v < 27) {
            v += 27;
        }
        if (v != 27 && v != 28) {
            return 0xffffffff;
        }
        address recovered = ecrecover(hash, v, r, s);
        if (recovered != address(0) && _isAuthorized(recovered)) {
            // EIP-1271 magic value.
            return 0x1626ba7e;
        }
        return 0xffffffff;
    }
}
