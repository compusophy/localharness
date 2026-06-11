// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibDiamond} from "../libraries/LibDiamond.sol";
import {LibCreditMeterStorage} from "../libraries/LibCreditMeterStorage.sol";
import {LibCreditsStorage} from "../libraries/LibCreditsStorage.sol";

interface IERC20Min {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function transfer(address to, uint256 amount) external returns (bool);
}

/// @title CreditMeterFacet
/// @notice Per-request (x402-style) metering, alongside the coarse time
///         sessions of `SessionFacet`. Users prepay `$LH` into a metered
///         balance; the credit proxy debits exact cost per request.
///
///         Flow: the proxy GATES a request with a cheap `creditOf(user)`
///         read (serves immediately if funded), then submits `meter(user,
///         cost)` asynchronously (sponsored) — so per-request metering
///         adds ~no latency. Only the owner-set `meter` address can
///         debit, and it can ONLY subtract from a balance, never move
///         funds out — same trust envelope as the proxy already holding
///         the Gemini key.
contract CreditMeterFacet {
    event CreditsDeposited(address indexed user, uint256 amount, uint256 newBalance);
    event CreditsWithdrawn(address indexed user, uint256 amount, uint256 newBalance);
    event Metered(address indexed user, uint256 amount, uint256 newBalance);
    event MeterUpdated(address indexed meter);

    error NotConfigured();
    error NotMeter();
    error InsufficientCredits();

    // --- Funding ---------------------------------------------------------

    /// Prepay `amount` `$LH` into the caller's metered credit balance.
    /// Pulls `$LH` caller -> diamond via `transferFrom` (approve the
    /// diamond first; the bundle batches approve + deposit).
    function depositCredits(uint256 amount) external {
        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();
        require(
            IERC20Min(token).transferFrom(msg.sender, address(this), amount),
            "deposit: transfer failed"
        );
        LibCreditMeterStorage.Storage storage s = LibCreditMeterStorage.load();
        s.creditOf[msg.sender] += amount;
        emit CreditsDeposited(msg.sender, amount, s.creditOf[msg.sender]);
    }

    /// Pull `amount` of the caller's UNSPENT metered credits back out as
    /// wallet `$LH`. Unspent credits are caller-owned escrow (every ledger
    /// credit is backed 1:1 by `$LH` `depositCredits` pulled into the
    /// diamond; `meter()` only finalizes spend by shrinking the ledger) —
    /// so the two pots are one balance in practice: deposit to chat,
    /// withdraw to pay agents (x402) or transfer. Ledger debit BEFORE the
    /// token transfer (CEI).
    function withdrawCredits(uint256 amount) external {
        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();
        LibCreditMeterStorage.Storage storage s = LibCreditMeterStorage.load();
        uint256 bal = s.creditOf[msg.sender];
        if (bal < amount) revert InsufficientCredits();
        unchecked {
            s.creditOf[msg.sender] = bal - amount;
        }
        require(IERC20Min(token).transfer(msg.sender, amount), "withdraw: transfer failed");
        emit CreditsWithdrawn(msg.sender, amount, bal - amount);
    }

    // --- Metering (proxy only) ------------------------------------------

    /// Debit `amount` from `user`'s metered balance. Callable only by the
    /// owner-set meter address (the credit proxy). Reverts if the balance
    /// is short, so the proxy can treat a revert as "out of credit".
    function meter(address user, uint256 amount) external {
        LibCreditMeterStorage.Storage storage s = LibCreditMeterStorage.load();
        if (msg.sender != s.meter) revert NotMeter();
        uint256 bal = s.creditOf[user];
        if (bal < amount) revert InsufficientCredits();
        unchecked {
            s.creditOf[user] = bal - amount;
        }
        emit Metered(user, amount, bal - amount);
    }

    // --- Owner ----------------------------------------------------------

    function setMeter(address newMeter) external {
        LibDiamond.enforceIsContractOwner();
        LibCreditMeterStorage.load().meter = newMeter;
        emit MeterUpdated(newMeter);
    }

    // --- Views ----------------------------------------------------------

    function creditOf(address user) external view returns (uint256) {
        return LibCreditMeterStorage.load().creditOf[user];
    }

    function meterAddress() external view returns (address) {
        return LibCreditMeterStorage.load().meter;
    }
}
