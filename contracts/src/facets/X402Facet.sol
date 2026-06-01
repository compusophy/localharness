// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibCreditsStorage} from "../libraries/LibCreditsStorage.sol";
import {LibX402Storage} from "../libraries/LibX402Storage.sol";

interface IERC20Min {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
}

interface IERC1271 {
    function isValidSignature(bytes32 hash, bytes calldata signature) external view returns (bytes4);
}

/// @title X402Facet
/// @notice x402 ("exact" scheme) payment settlement in `$LH`, for
///         agent-to-agent payments. The deployed `$LH` token predates
///         EIP-3009, so settlement lives here: the PAYER signs an
///         EIP-712 `PaymentAuthorization` off-chain (gasless), and the
///         payee (or a facilitator) submits `settle(...)`, which
///         verifies the signature, enforces validity + a one-shot
///         nonce, and pulls `$LH` `from -> to` via `transferFrom`.
///
///         The payer approves the diamond for `$LH` ONCE; thereafter
///         every per-request payment is just a signature — the x402
///         `X-PAYMENT` payload. Signatures are verified for both EOAs
///         (ecrecover, low-s/EIP-2) and contract wallets (EIP-1271), so
///         an agent can pay from its ERC-6551 TBA.
///
///         EIP-712 domain: name "localharness-x402", version "1",
///         chainId, verifyingContract = the diamond.
contract X402Facet {
    event PaymentSettled(
        address indexed from,
        address indexed to,
        uint256 value,
        bytes32 indexed nonce
    );

    error AuthAlreadyUsed();
    error AuthNotYetValid();
    error AuthExpired();
    error BadSignature();
    error NotConfigured();

    bytes32 private constant EIP712_DOMAIN_TYPEHASH = keccak256(
        "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"
    );
    bytes32 private constant PAYMENT_TYPEHASH = keccak256(
        "PaymentAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)"
    );
    bytes4 private constant MAGICVALUE_1271 = 0x1626ba7e;
    // secp256k1n / 2 — reject high-s signatures (EIP-2 malleability).
    uint256 private constant HALF_N =
        0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0;

    /// EIP-712 domain separator clients must use to build the digest.
    function x402DomainSeparator() public view returns (bytes32) {
        return keccak256(
            abi.encode(
                EIP712_DOMAIN_TYPEHASH,
                keccak256(bytes("localharness-x402")),
                keccak256(bytes("1")),
                block.chainid,
                address(this)
            )
        );
    }

    /// Settle an x402 payment: verify `signature` over the authorization
    /// and move `value` `$LH` from `from` to `to`. Idempotent per
    /// (from, nonce) — a replayed authorization reverts `AuthAlreadyUsed`.
    function settle(
        address from,
        address to,
        uint256 value,
        uint256 validAfter,
        uint256 validBefore,
        bytes32 nonce,
        bytes calldata signature
    ) external {
        LibX402Storage.Storage storage s = LibX402Storage.load();
        if (s.authState[from][nonce]) revert AuthAlreadyUsed();
        if (block.timestamp <= validAfter) revert AuthNotYetValid();
        if (block.timestamp >= validBefore) revert AuthExpired();

        bytes32 structHash = keccak256(
            abi.encode(PAYMENT_TYPEHASH, from, to, value, validAfter, validBefore, nonce)
        );
        bytes32 digest = keccak256(
            abi.encodePacked("\x19\x01", x402DomainSeparator(), structHash)
        );
        if (!_isValidSignature(from, digest, signature)) revert BadSignature();

        // Effect before the external token call (CEI / replay safety).
        s.authState[from][nonce] = true;

        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();
        require(IERC20Min(token).transferFrom(from, to, value), "x402: transfer failed");

        emit PaymentSettled(from, to, value, nonce);
    }

    function authorizationState(address from, bytes32 nonce) external view returns (bool) {
        return LibX402Storage.load().authState[from][nonce];
    }

    /// EOA (ecrecover, low-s) or contract wallet (EIP-1271) signature check.
    function _isValidSignature(
        address signer,
        bytes32 digest,
        bytes calldata signature
    ) internal view returns (bool) {
        if (signer.code.length > 0) {
            try IERC1271(signer).isValidSignature(digest, signature) returns (bytes4 mv) {
                return mv == MAGICVALUE_1271;
            } catch {
                return false;
            }
        }
        if (signature.length != 65) return false;
        bytes32 r;
        bytes32 vs;
        uint8 v;
        assembly {
            r := calldataload(signature.offset)
            vs := calldataload(add(signature.offset, 32))
            v := byte(0, calldataload(add(signature.offset, 64)))
        }
        if (uint256(vs) > HALF_N) return false; // reject high-s
        if (v < 27) v += 27;
        if (v != 27 && v != 28) return false;
        address recovered = ecrecover(digest, v, r, vs);
        return recovered != address(0) && recovered == signer;
    }
}
