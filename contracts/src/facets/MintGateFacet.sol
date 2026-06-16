// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibDiamond} from "../libraries/LibDiamond.sol";
import {LibCreditsStorage} from "../libraries/LibCreditsStorage.sol";
import {LibCreditMeterStorage} from "../libraries/LibCreditMeterStorage.sol";
import {LibMintGateStorage} from "../libraries/LibMintGateStorage.sol";

interface ILocalharnessCreditsMint {
    function mintWithMemo(address to, uint256 amount, bytes32 memo) external;
    function burn(uint256 amount) external;
    function totalSupply() external view returns (uint256);
    function balanceOf(address account) external view returns (uint256);
}

interface IERC1271 {
    function isValidSignature(bytes32 hash, bytes calldata signature) external view returns (bytes4);
}

/// @title MintGateFacet
/// @notice The SOLE on-chain path that mints `$LH` against settled fiat — the
///         Stripe → Tempo on-ramp's money valve. The credit proxy, on a
///         verified `checkout.session.completed` webhook, EIP-712-signs a
///         `FiatMint` with the dedicated `fiatIssuerSigner` key; anyone may
///         submit `mintFromFiat(...)`, which verifies the signature, enforces a
///         one-shot `receiptId` + per-receipt + rolling-window caps, and mints.
///
///         Money safety (see `design/custody-security.md` + stripe-mainnet §7):
///         - **C1** the issued wei also passes the token-wide rolling cap in
///           `LocalharnessCredits._mint`, so even a leaked signer (or a rogue
///           second facet) is bounded by a real ceiling, not `supplyCap`.
///         - **C2** the mint lands in the DIAMOND'S OWN escrow (mint-to-self);
///           the buyer gets a spendable `creditOf` credit + a `fiatLocked`
///           entry. `CreditMeterFacet` makes withdraw/meter lock-aware, and
///           `clawbackFiatMint` BURNS the diamond-held escrow on a chargeback —
///           a clawback can recover only what is still locked + unspent.
///
///         EIP-712 domain: name "localharness-mintgate", version "1", chainId,
///         verifyingContract = the diamond. The fiat-issuer signer may be an
///         EOA (ecrecover, low-s / EIP-2) or a contract wallet (EIP-1271).
contract MintGateFacet {
    event FiatMinted(
        address indexed to,
        uint256 amount,
        bytes32 indexed receiptId,
        uint256 unlockAt
    );
    event FiatClawedBack(address indexed to, uint256 recovered, bytes32 indexed receiptId);
    event FiatIssuerSignerSet(address indexed signer);
    event ClawbackerSet(address indexed clawbacker);
    event PerReceiptMaxSet(uint256 maxWei);
    event FiatLockSecsSet(uint256 lockSecs);
    event FiatMintWindowSet(uint256 capWei, uint256 windowSecs);

    error InvalidRecipient();
    error InvalidAmount();
    error ReceiptUsed();
    error UnknownReceipt();
    error AlreadyClawed();
    error AuthExpired();
    error BadSignature();
    error PerReceiptCapExceeded();
    error FiatWindowCapExceeded();
    error NotClawbacker();
    error NotConfigured();
    error InvalidWindow();

    bytes32 private constant EIP712_DOMAIN_TYPEHASH = keccak256(
        "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"
    );
    bytes32 private constant FIAT_MINT_TYPEHASH = keccak256(
        "FiatMint(address to,uint256 amount,bytes32 receiptId,uint256 validBefore)"
    );
    bytes4 private constant MAGICVALUE_1271 = 0x1626ba7e;
    // secp256k1n / 2 — reject high-s signatures (EIP-2 malleability).
    uint256 private constant HALF_N =
        0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0;
    bytes32 private constant MEMO_FIAT = "LH-FIAT-MINT";

    /// EIP-712 domain separator clients build the `FiatMint` digest against.
    function fiatMintDomainSeparator() public view returns (bytes32) {
        return keccak256(
            abi.encode(
                EIP712_DOMAIN_TYPEHASH,
                keccak256(bytes("localharness-mintgate")),
                keccak256(bytes("1")),
                block.chainid,
                address(this)
            )
        );
    }

    // --- The money valve -------------------------------------------------

    /// Mint `amount` `$LH` for `to` against a settled fiat receipt. Verifies
    /// `signature` from `fiatIssuerSigner` over the `FiatMint` struct, enforces
    /// one-shot `receiptId`, per-receipt + rolling-window caps, then mints into
    /// the diamond escrow and credits `to` a LOCKED `creditOf` balance.
    /// Idempotent per `receiptId` — a replayed webhook reverts `ReceiptUsed`.
    function mintFromFiat(
        address to,
        uint256 amount,
        bytes32 receiptId,
        uint256 validBefore,
        bytes calldata signature
    ) external {
        if (to == address(0)) revert InvalidRecipient();
        if (amount == 0) revert InvalidAmount();
        if (block.timestamp >= validBefore) revert AuthExpired();

        LibMintGateStorage.Storage storage s = LibMintGateStorage.load();
        if (s.fiatIssuerSigner == address(0)) revert NotConfigured();

        LibMintGateStorage.Receipt storage r = s.receipts[receiptId];
        if (r.used) revert ReceiptUsed();
        if (s.perReceiptMaxWei != 0 && amount > s.perReceiptMaxWei) revert PerReceiptCapExceeded();

        bytes32 structHash = keccak256(
            abi.encode(FIAT_MINT_TYPEHASH, to, amount, receiptId, validBefore)
        );
        bytes32 digest = keccak256(
            abi.encodePacked("\x19\x01", fiatMintDomainSeparator(), structHash)
        );
        if (!_isValidSignature(s.fiatIssuerSigner, digest, signature)) revert BadSignature();

        // Fiat-specific rolling window (sub-ceiling under the token-wide cap).
        if (s.windowCapWei != 0) {
            if (block.timestamp >= s.windowStart + s.windowSecs) {
                s.windowStart = block.timestamp;
                s.mintedInWindow = 0;
            }
            uint256 windowTotal = s.mintedInWindow + amount;
            if (windowTotal > s.windowCapWei) revert FiatWindowCapExceeded();
            s.mintedInWindow = windowTotal;
        }

        // EFFECTS before the external mint (replay / reentrancy safety).
        r.used = true;
        r.to = to;
        r.amount = amount;

        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();
        // Mint into the diamond's OWN balance (escrow). Also passes the token's
        // global rolling-window cap — the C1 backstop.
        ILocalharnessCreditsMint(token).mintWithMemo(address(this), amount, MEMO_FIAT);

        // Spendable-on-compute credit for the buyer …
        LibCreditMeterStorage.load().creditOf[to] += amount;
        // … but LOCKED against withdraw/transfer until `unlockAt`.
        LibMintGateStorage.FiatLock storage lock = s.fiatLocked[to];
        lock.amount += amount;
        uint256 newUnlock = block.timestamp + s.fiatLockSecs;
        if (newUnlock > lock.unlockAt) lock.unlockAt = newUnlock;

        emit FiatMinted(to, amount, receiptId, lock.unlockAt);
    }

    /// Burn the STILL-LOCKED escrow for a fiat receipt on a refund/chargeback.
    /// `maxWei` is the CUMULATIVE wei that should have been clawed by now —
    /// `0` means the full receipt (a dispute / full refund); a partial refund
    /// passes Stripe's cumulative refunded amount in wei, so successive partial
    /// refunds claw the delta and never over-burn the buyer's still-backed
    /// credit. Recovers `min(target − clawedSoFar, still-locked)`; already
    /// spent/withdrawn fiat-`$LH` is the accepted, lock-window-bounded residual
    /// (red-team H1) and yields a 0-recovery (NOT a revert) so a retry is clean.
    /// Callable by the clawbacker key or the diamond owner.
    function clawbackFiatMint(bytes32 receiptId, uint256 maxWei) external returns (uint256 recovered) {
        LibMintGateStorage.Storage storage s = LibMintGateStorage.load();
        if (msg.sender != s.clawbacker) LibDiamond.enforceIsContractOwner();

        LibMintGateStorage.Receipt storage r = s.receipts[receiptId];
        if (!r.used) revert UnknownReceipt();

        // Cumulative target, capped at the minted amount. maxWei==0 ⇒ full.
        uint256 target = (maxWei == 0 || maxWei > r.amount) ? r.amount : maxWei;
        if (target <= r.clawedWei) revert AlreadyClawed();
        uint256 want = target - r.clawedWei;

        address user = r.to;
        LibMintGateStorage.FiatLock storage lock = s.fiatLocked[user];
        recovered = want < lock.amount ? want : lock.amount;
        if (recovered > 0) {
            lock.amount -= recovered;
            LibCreditMeterStorage.load().creditOf[user] -= recovered;
            r.clawedWei += recovered;
            address token = LibCreditsStorage.load().creditsToken;
            // Burn the diamond's own escrow tokens (CEI: ledger effects first).
            ILocalharnessCreditsMint(token).burn(recovered);
        }
        if (r.clawedWei >= r.amount) r.clawed = true;
        emit FiatClawedBack(user, recovered, receiptId);
    }

    // --- Owner config ----------------------------------------------------

    function setFiatIssuerSigner(address signer) external {
        LibDiamond.enforceIsContractOwner();
        LibMintGateStorage.load().fiatIssuerSigner = signer;
        emit FiatIssuerSignerSet(signer);
    }

    function setClawbacker(address who) external {
        LibDiamond.enforceIsContractOwner();
        LibMintGateStorage.load().clawbacker = who;
        emit ClawbackerSet(who);
    }

    function setPerReceiptMaxWei(uint256 maxWei) external {
        LibDiamond.enforceIsContractOwner();
        LibMintGateStorage.load().perReceiptMaxWei = maxWei;
        emit PerReceiptMaxSet(maxWei);
    }

    function setFiatLockSecs(uint256 lockSecs) external {
        LibDiamond.enforceIsContractOwner();
        LibMintGateStorage.load().fiatLockSecs = lockSecs;
        emit FiatLockSecsSet(lockSecs);
    }

    /// Set the fiat-specific rolling-window cap. This is a sub-ceiling under the
    /// token-wide cap (which carries the time-locked raise — red-team M), so
    /// raising this can never exceed the global limit; immediate is acceptable.
    function setFiatMintWindow(uint256 capWei, uint256 windowSecs) external {
        LibDiamond.enforceIsContractOwner();
        if (capWei != 0 && windowSecs == 0) revert InvalidWindow();
        LibMintGateStorage.Storage storage s = LibMintGateStorage.load();
        s.windowCapWei = capWei;
        s.windowSecs = windowSecs;
        emit FiatMintWindowSet(capWei, windowSecs);
    }

    // --- Views -----------------------------------------------------------

    function fiatIssuerSigner() external view returns (address) {
        return LibMintGateStorage.load().fiatIssuerSigner;
    }

    function clawbacker() external view returns (address) {
        return LibMintGateStorage.load().clawbacker;
    }

    function perReceiptMaxWei() external view returns (uint256) {
        return LibMintGateStorage.load().perReceiptMaxWei;
    }

    function fiatLockSecs() external view returns (uint256) {
        return LibMintGateStorage.load().fiatLockSecs;
    }

    function fiatLockedOf(address user) external view returns (uint256 amount, uint256 unlockAt) {
        LibMintGateStorage.FiatLock storage lock = LibMintGateStorage.load().fiatLocked[user];
        return (lock.amount, lock.unlockAt);
    }

    function receiptUsed(bytes32 receiptId) external view returns (bool) {
        return LibMintGateStorage.load().receipts[receiptId].used;
    }

    function receiptInfo(bytes32 receiptId)
        external
        view
        returns (address to, uint256 amount, bool used, bool clawed, uint256 clawedWei)
    {
        LibMintGateStorage.Receipt storage r = LibMintGateStorage.load().receipts[receiptId];
        return (r.to, r.amount, r.used, r.clawed, r.clawedWei);
    }

    /// (capWei, windowSecs, windowStart, mintedInWindow) — `mintedInWindow` is
    /// reported as 0 once the current window has rolled over (it resets lazily
    /// on the next mint).
    function fiatMintWindow()
        external
        view
        returns (uint256 capWei, uint256 windowSecs, uint256 windowStart, uint256 mintedInWindow)
    {
        LibMintGateStorage.Storage storage s = LibMintGateStorage.load();
        uint256 minted = s.mintedInWindow;
        if (s.windowCapWei != 0 && block.timestamp >= s.windowStart + s.windowSecs) {
            minted = 0;
        }
        return (s.windowCapWei, s.windowSecs, s.windowStart, minted);
    }

    /// `$LH` held OUTSIDE the diamond escrow = `totalSupply − balanceOf(diamond)`.
    /// This is the portion that has left platform custody (in user wallets, TBAs,
    /// etc.) and could be cashed out — the figure the off-chain reconciliation
    /// alarm compares against settled USD (`circulating ≤ usd_held / peg`).
    function circulatingSupply() external view returns (uint256) {
        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) return 0;
        uint256 total = ILocalharnessCreditsMint(token).totalSupply();
        uint256 escrow = ILocalharnessCreditsMint(token).balanceOf(address(this));
        return total > escrow ? total - escrow : 0;
    }

    // --- Signature verification (EOA low-s or EIP-1271) ------------------

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
