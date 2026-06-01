// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibDiamond} from "../libraries/LibDiamond.sol";
import {LibRedeemStorage} from "../libraries/LibRedeemStorage.sol";
import {LibCreditsStorage} from "../libraries/LibCreditsStorage.sol";

interface ILocalharnessCredits {
    function mintWithMemo(address to, uint256 amount, bytes32 memo) external;
}

/// @title RedeemFacet
/// @notice Owner-gated bootstrap: hand out one-time redeem codes that
///         mint `$LH` platform credits to whoever redeems them. The
///         owner loads only `keccak256(code)` hashes on-chain (via
///         `addRedeemCodes`), so the code table never leaks — the
///         plaintext codes are distributed off-chain (DMs, invites,
///         etc.). Redeeming mints fresh `$LH` to the caller; the
///         diamond holds ISSUER_ROLE on the credits token, so this is
///         a controlled supply path (same as `CreditsFacet.claimDaily`).
///
///         Mirrors the codeHash pattern already used by PairingFacet.
contract RedeemFacet {
    event CodesAdded(uint256 count, uint256 amountWei);
    event CodesDisabled(uint256 count);
    event Redeemed(address indexed user, uint256 amount, bytes32 indexed codeHash);

    error NotConfigured();
    error InvalidCode();
    error CodeAlreadyUsed();

    /// Memo stamped on the mint so off-chain indexers can identify
    /// redeem flows in `MintWithMemo` logs without a side database.
    bytes32 internal constant MEMO = "LH-REDEEM";

    // --- Owner-only -----------------------------------------------------

    /// Register a batch of code hashes, each worth `amountWei` `$LH`.
    /// Pass `keccak256(bytes(code))` for each code. Call again with a
    /// different `amountWei` for codes of a different denomination.
    /// Re-adding an existing hash just overwrites its amount (until
    /// it's claimed; claimed codes stay claimed).
    function addRedeemCodes(bytes32[] calldata codeHashes, uint256 amountWei) external {
        LibDiamond.enforceIsContractOwner();
        if (amountWei == 0) revert InvalidCode();
        LibRedeemStorage.Storage storage s = LibRedeemStorage.load();
        for (uint256 i = 0; i < codeHashes.length; i++) {
            s.codeAmount[codeHashes[i]] = amountWei;
        }
        emit CodesAdded(codeHashes.length, amountWei);
    }

    /// Neutralize leaked-but-unclaimed codes without minting anything:
    /// marks each hash claimed so it can never be redeemed. The revoke
    /// path for when a batch of plaintext codes leaks before use.
    function disableRedeemCodes(bytes32[] calldata codeHashes) external {
        LibDiamond.enforceIsContractOwner();
        LibRedeemStorage.Storage storage s = LibRedeemStorage.load();
        for (uint256 i = 0; i < codeHashes.length; i++) {
            s.claimed[codeHashes[i]] = true;
        }
        emit CodesDisabled(codeHashes.length);
    }

    // --- Public ---------------------------------------------------------

    /// Redeem a plaintext code for its `$LH` denomination. One-shot:
    /// the code is burned (marked claimed) before the mint (CEI), so
    /// it can never be redeemed twice. Reverts on unknown / already-
    /// used codes, or if the credits token isn't configured.
    function redeem(string calldata code) external returns (uint256) {
        bytes32 h = keccak256(bytes(code));
        LibRedeemStorage.Storage storage s = LibRedeemStorage.load();
        uint256 amount = s.codeAmount[h];
        if (amount == 0) revert InvalidCode();
        if (s.claimed[h]) revert CodeAlreadyUsed();
        s.claimed[h] = true;

        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();

        ILocalharnessCredits(token).mintWithMemo(msg.sender, amount, MEMO);
        emit Redeemed(msg.sender, amount, h);
        return amount;
    }

    // --- Views ----------------------------------------------------------

    /// `$LH` amount a given code hash is worth (0 = unknown code).
    function redeemAmountOf(bytes32 codeHash) external view returns (uint256) {
        return LibRedeemStorage.load().codeAmount[codeHash];
    }

    function isRedeemed(bytes32 codeHash) external view returns (bool) {
        return LibRedeemStorage.load().claimed[codeHash];
    }
}
