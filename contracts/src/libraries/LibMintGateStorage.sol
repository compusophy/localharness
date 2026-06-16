// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the fiat on-ramp mint gate (`MintGateFacet`).
///      Diamond storage pattern — fresh slot, no collision with any other
///      facet. Add new fields ONLY at the end of a struct.
///
///      `fiatLocked` is read+written by BOTH `MintGateFacet` (mint / clawback)
///      and `CreditMeterFacet` (lock-aware withdraw / meter), so it lives here
///      and both facets import this lib.
library LibMintGateStorage {
    bytes32 constant POSITION = keccak256("localharness.mintgate.storage.v1");

    /// One fiat mint, keyed by an immutable Stripe-derived `receiptId`. `used`
    /// is the one-shot idempotency guard (set on mint, never cleared); `clawed`
    /// guards double-clawback. `to`/`amount` let `clawbackFiatMint` know whom to
    /// claw and how much was minted under this receipt.
    struct Receipt {
        address to;
        uint256 amount;
        bool used;
        /// True once the receipt is FULLY clawed (`clawedWei >= amount`).
        bool clawed;
        /// Cumulative wei clawed back so far — lets partial Stripe refunds claw
        /// proportionally and sum correctly across multiple refund events.
        uint256 clawedWei;
    }

    /// The portion of a user's `creditOf` balance that is fiat-origin and still
    /// LOCKED: spendable on compute (`meter`) but NOT withdrawable/transferable
    /// to wallet `$LH` until `unlockAt`, and clawable on chargeback. Aggregated
    /// per user; a new mint extends `unlockAt` to the latest (conservative).
    struct FiatLock {
        uint256 amount;
        uint256 unlockAt;
    }

    struct Storage {
        /// EIP-712 signer the proxy authorizes fiat mints with (a dedicated hot
        /// EOA, distinct from the meter key — see `design/custody-security.md`).
        /// The key NEVER mints directly; it only signs `FiatMint` digests.
        address fiatIssuerSigner;
        /// Address allowed to call `clawbackFiatMint` (the proxy clawback key);
        /// the diamond owner may also clawback.
        address clawbacker;
        /// Max fiat wei mintable per rolling window (0 = uncapped). A sub-ceiling
        /// under the token-wide global cap; both are enforced.
        uint256 windowCapWei;
        /// Fiat rolling-window length in seconds (must be > 0 when cap is set).
        uint256 windowSecs;
        /// Unix start of the current fiat window; rolls forward in `mintFromFiat`.
        uint256 windowStart;
        /// Fiat wei minted so far in the current window.
        uint256 mintedInWindow;
        /// Max wei a single receipt may mint (0 = unbounded). First-buyer/
        /// bill-shock guard.
        uint256 perReceiptMaxWei;
        /// Lock duration applied to each fiat mint (seconds). Should exceed the
        /// Stripe dispute window for full chargeback coverage (red-team H1).
        uint256 fiatLockSecs;
        mapping(bytes32 => Receipt) receipts;
        mapping(address => FiatLock) fiatLocked;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
