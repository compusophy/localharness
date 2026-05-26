// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibDiamond} from "../libraries/LibDiamond.sol";
import {LibCreditsStorage} from "../libraries/LibCreditsStorage.sol";

interface ILocalharnessCredits {
    function mintWithMemo(address to, uint256 amount, bytes32 memo) external;
    function balanceOf(address account) external view returns (uint256);
    function supplyCap() external view returns (uint256);
    function totalSupply() external view returns (uint256);
}

/// @title CreditsFacet
/// @notice Distribution layer for `LocalharnessCredits`. The diamond
///         holds ISSUER_ROLE on the token, so the only path to fresh
///         supply is through these methods — owner can tune issuance
///         rules without touching the token contract.
///
///         v1 surface: per-address daily allowance, claimable once
///         per UTC day. Action-gating (spend credits to register a
///         subdomain, etc.) is a later layer that lives on the
///         consuming facets (`LocalharnessRegistryFacet.register`
///         could become payable in $LH, etc.).
///
///         Day boundary: `block.timestamp / 86400`. UTC-aligned,
///         resets at 00:00 UTC, no cron required.
contract CreditsFacet {
    event CreditsTokenSet(address indexed token);
    event DailyAllowanceUpdated(uint256 oldAllowance, uint256 newAllowance);
    event DailyClaim(address indexed claimer, uint256 amount, uint64 indexed day);

    error AlreadyClaimedToday(uint64 day);
    error NotConfigured();
    error InvalidAddress();

    /// Memo prefix used on all `mintWithMemo` calls so off-chain
    /// indexers can identify daily-allowance flows. Format:
    /// `bytes32("LH-DAILY        ")` + lower 8 bytes packed with the
    /// day index. Recoverable from logs without a side database.
    bytes32 internal constant MEMO_PREFIX = "LH-DAILY-";

    // --- Owner-only setters ---------------------------------------------

    /// Bind the diamond to the credits token. Diamond must already
    /// have been granted `ISSUER_ROLE` on the token (typically by the
    /// deploy script via `lh.grantRole(ISSUER_ROLE, diamond)`).
    function setCreditsToken(address token) external {
        LibDiamond.enforceIsContractOwner();
        if (token == address(0)) revert InvalidAddress();
        LibCreditsStorage.load().creditsToken = token;
        emit CreditsTokenSet(token);
    }

    /// Per-claim allowance in token wei (18 decimals). Setting to zero
    /// effectively pauses the daily faucet.
    function setDailyAllowance(uint256 amountWei) external {
        LibDiamond.enforceIsContractOwner();
        LibCreditsStorage.Storage storage s = LibCreditsStorage.load();
        emit DailyAllowanceUpdated(s.dailyAllowance, amountWei);
        s.dailyAllowance = amountWei;
    }

    // --- Public claim ----------------------------------------------------

    /// Claim today's allowance. One claim per address per UTC day —
    /// the day index is `block.timestamp / 86400`. Reverts if the
    /// caller already claimed this day, or if allowance is 0
    /// (effectively-paused), or if the token isn't configured.
    function claimDaily() external returns (uint256) {
        LibCreditsStorage.Storage storage s = LibCreditsStorage.load();
        if (s.creditsToken == address(0)) revert NotConfigured();
        if (s.dailyAllowance == 0) revert NotConfigured();

        uint64 today = uint64(block.timestamp / 86400);
        if (s.lastClaimDay[msg.sender] == today) {
            revert AlreadyClaimedToday(today);
        }
        s.lastClaimDay[msg.sender] = today;

        uint256 amount = s.dailyAllowance;
        bytes32 memo = bytes32(uint256(MEMO_PREFIX) | uint256(today));
        ILocalharnessCredits(s.creditsToken).mintWithMemo(msg.sender, amount, memo);
        emit DailyClaim(msg.sender, amount, today);
        return amount;
    }

    // --- Views ----------------------------------------------------------

    function creditsToken() external view returns (address) {
        return LibCreditsStorage.load().creditsToken;
    }

    function dailyAllowance() external view returns (uint256) {
        return LibCreditsStorage.load().dailyAllowance;
    }

    function lastClaimDay(address account) external view returns (uint64) {
        return LibCreditsStorage.load().lastClaimDay[account];
    }

    /// Convenience: returns true iff `account` can claim right now
    /// (token configured, allowance > 0, not yet claimed today).
    function canClaim(address account) external view returns (bool) {
        LibCreditsStorage.Storage storage s = LibCreditsStorage.load();
        if (s.creditsToken == address(0)) return false;
        if (s.dailyAllowance == 0) return false;
        uint64 today = uint64(block.timestamp / 86400);
        return s.lastClaimDay[account] != today;
    }
}
