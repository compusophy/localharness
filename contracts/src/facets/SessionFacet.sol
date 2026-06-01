// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibDiamond} from "../libraries/LibDiamond.sol";
import {LibSessionStorage} from "../libraries/LibSessionStorage.sol";
import {LibCreditsStorage} from "../libraries/LibCreditsStorage.sol";

interface IERC20Min {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
}

/// @title SessionFacet
/// @notice Coarse, time-bounded credit sessions — the metering
///         substrate for the `$LH` credit proxy. Spending `$LH` opens
///         a session valid for `duration` seconds; the off-chain Vercel
///         Edge proxy reads `sessionExpiryOf(caller)` and forwards
///         Gemini requests only while the session is live. Because
///         metering is by TIME (not per-request), the proxy never has
///         to write per request and stays stateless — it just reads
///         this expiry. This is the v1 model; per-token metering /
///         streaming / x402 layer on top later.
///
///         `$LH` is pulled caller -> diamond via `transferFrom` (same
///         cost-gate pattern as `register`), so the caller must approve
///         the diamond for `priceWei` first — the bundle batches
///         approve + openSession into one sponsored Tempo tx.
///
///         NOTE (cost caveat): a session is all-you-can-use within its
///         window, capped only by Gemini's own rate limits. Owner tunes
///         `priceWei` / `duration` to balance UX vs the platform key's
///         token spend.
contract SessionFacet {
    event SessionOpened(address indexed user, uint256 expiry, uint256 priceWei);
    event SessionPriceUpdated(uint256 oldPriceWei, uint256 newPriceWei);
    event SessionDurationUpdated(uint256 oldDuration, uint256 newDuration);

    error SessionsDisabled();
    error NotConfigured();

    // --- Public ---------------------------------------------------------

    /// Open (or renew) the caller's credit session. Pulls `priceWei`
    /// `$LH` from the caller to the diamond when the price is non-zero,
    /// then sets the caller's expiry to `now + duration`. Reverts if
    /// sessions are disabled (`duration == 0`) or the credits token is
    /// needed but unconfigured.
    function openSession() external returns (uint256 expiry) {
        LibSessionStorage.Storage storage s = LibSessionStorage.load();
        uint256 duration = s.duration;
        if (duration == 0) revert SessionsDisabled();

        // CEI: write the session expiry BEFORE any external call. A
        // failed payment reverts the whole tx (and this write with it),
        // so writing first costs nothing and removes the reentrancy
        // surface even if a future credits token grows transfer hooks.
        expiry = block.timestamp + duration;
        s.sessionExpiry[msg.sender] = expiry;

        uint256 price = s.priceWei;
        if (price > 0) {
            address token = LibCreditsStorage.load().creditsToken;
            if (token == address(0)) revert NotConfigured();
            require(
                IERC20Min(token).transferFrom(msg.sender, address(this), price),
                "session: transfer failed"
            );
        }
        emit SessionOpened(msg.sender, expiry, price);
    }

    // --- Owner-only setters ---------------------------------------------

    function setSessionPrice(uint256 newPriceWei) external {
        LibDiamond.enforceIsContractOwner();
        LibSessionStorage.Storage storage s = LibSessionStorage.load();
        emit SessionPriceUpdated(s.priceWei, newPriceWei);
        s.priceWei = newPriceWei;
    }

    function setSessionDuration(uint256 newDuration) external {
        LibDiamond.enforceIsContractOwner();
        LibSessionStorage.Storage storage s = LibSessionStorage.load();
        emit SessionDurationUpdated(s.duration, newDuration);
        s.duration = newDuration;
    }

    // --- Views ----------------------------------------------------------

    /// Unix-seconds expiry of `account`'s session. 0 (or any past
    /// value) means no active session. This is the selector the credit
    /// proxy calls on every request.
    function sessionExpiryOf(address account) external view returns (uint256) {
        return LibSessionStorage.load().sessionExpiry[account];
    }

    function sessionPrice() external view returns (uint256) {
        return LibSessionStorage.load().priceWei;
    }

    function sessionDuration() external view returns (uint256) {
        return LibSessionStorage.load().duration;
    }
}
