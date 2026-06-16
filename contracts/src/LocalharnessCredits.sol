// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @title LocalharnessCredits
/// @notice In-system credits — TIP-20-shaped, NOT fee-token-eligible
///         (currency = "credits", not "USD"). Replaces the standalone
///         ERC-20 `LocalharnessToken.sol` (orphaned at
///         `0xcC8A300658…`). The credit semantics layer (daily
///         allowance, action-gating) lives in `CreditsFacet` on the
///         registry diamond; this contract is just the token surface
///         the facet drives.
///
///         What's implemented:
///         - Full ERC-20 (transfer / approve / allowance + events).
///         - TIP-20 memo extensions (transferWithMemo, mintWithMemo,
///           burnWithMemo + TransferWithMemo event). Useful for
///           tagging issuance/spend with a 32-byte purpose marker.
///         - TIP-20 metadata: currency() == "credits", paused()
///           returning false, supplyCap settable by owner.
///         - Minimal role-based access: ISSUER_ROLE gates mint. No
///           pause/unpause/permit yet (defer to a later TIP-20 pass
///           if we ever need fee-eligibility or DEX integration —
///           currency would need to flip to "USD" for that).
///
///         Deliberate omissions vs. full TIP-20:
///         - EIP-2612 permit (gasless approvals) — sponsored txs
///           already give us gasless UX.
///         - quoteToken / nextQuoteToken / DEX hooks — no DEX.
///         - systemTransferFrom / transferFeePreTx / transferFeePostTx —
///           precompile-callable for fee tokens; not applicable.
///         - PAUSE_ROLE / UNPAUSE_ROLE — emergency pause is overkill
///           for a credit system; revisit if abuse appears.
contract LocalharnessCredits {
    // --- ERC-20 metadata --------------------------------------------------

    string public constant name = "localharness credits";
    string public constant symbol = "LH";
    uint8 public constant decimals = 18;

    // --- ERC-20 state -----------------------------------------------------

    mapping(address => uint256) private _balances;
    mapping(address => mapping(address => uint256)) private _allowances;
    uint256 public totalSupply;

    // --- TIP-20 state -----------------------------------------------------

    /// Issuance ceiling. `mint` reverts if it would push totalSupply past
    /// this. Owner can raise/lower with `setSupplyCap` (cannot go below
    /// current totalSupply).
    uint256 public supplyCap;

    /// Currency identifier — TIP-20 fee-eligibility key. NON-USD means
    /// the chain will reject this token as a fee_token (which is what
    /// we want: credits are not for paying gas).
    string public constant currency = "credits";

    // --- Global mint rate-limit (rolling window) --------------------------
    //
    // Defense-in-depth against ISSUER_ROLE being diamond-wide (red-team C1):
    // EVERY mint path — `CreditsFacet.claimDaily`, `RedeemFacet.redeem`,
    // `MintGateFacet.mintFromFiat`, and ANY future or owner-cut malicious
    // facet (all of which run as `msg.sender == diamond`) — finalizes through
    // `_mint`. A ceiling enforced HERE therefore bounds TOTAL issuance
    // regardless of which facet calls, so a leaked fiat-issuer signer (or a
    // rogue second facet) cannot mint past it: the blast radius is this cap,
    // NOT `supplyCap`. NOTE it is a FIXED/tumbling window — across a boundary an
    // attacker can mint the full cap at the end of one window and again at the
    // start of the next, so the true worst case is <=2x cap per `windowSecs`.
    // Size the cap at HALF the tolerable per-interval loss.

    /// Max wei mintable per rolling window. `0` = uncapped (legacy behaviour).
    /// A value-real (mainnet) deploy MUST set a finite cap — it is a launch
    /// gate (see `design/custody-security.md`).
    uint256 public mintWindowCapWei;
    /// Rolling-window length in seconds. Must be > 0 whenever the cap is set.
    uint256 public mintWindowSecs;
    /// Unix start of the current window; rolls forward in `_mint`.
    uint256 public mintWindowStart;
    /// Wei minted so far in the current window.
    uint256 public mintedInWindow;

    /// Loosening the rate-limit — RAISING the cap, going uncapped, or
    /// SHORTENING the window (each raises max throughput) — is time-locked so
    /// an owner-key compromise cannot do a same-block `setCap(∞)` + drain
    /// (red-team M). TIGHTENING (lower cap / longer window) is immediate.
    uint256 public constant CAP_LOOSEN_TIMELOCK = 2 days;
    uint256 public pendingWindowCapWei;
    uint256 public pendingWindowSecs;
    /// Unix time the pending loosen may be applied; `0` = none pending.
    uint256 public pendingWindowEffectiveAt;

    // --- Roles ------------------------------------------------------------

    /// Caller can `mint`. Granted to the registry diamond's
    /// `CreditsFacet` so the facet's `claimDaily` is the only path
    /// to fresh supply.
    bytes32 public constant ISSUER_ROLE = keccak256("LH.ISSUER_ROLE");

    /// Owner — single admin for grant/revoke roles + setSupplyCap.
    /// Distinct from ISSUER_ROLE: owner can grant roles but cannot mint
    /// directly unless self-granted.
    address public owner;

    mapping(bytes32 => mapping(address => bool)) private _roles;

    // --- Events -----------------------------------------------------------

    event Transfer(address indexed from, address indexed to, uint256 amount);
    event Approval(address indexed owner, address indexed spender, uint256 amount);
    /// TIP-20 memo event — emitted alongside the standard Transfer when
    /// the memo variant is used. Memo is indexed so off-chain indexers
    /// can filter by purpose marker.
    event TransferWithMemo(
        address indexed from,
        address indexed to,
        uint256 amount,
        bytes32 indexed memo
    );
    event Mint(address indexed to, uint256 amount);
    event Burn(address indexed from, uint256 amount);
    event RoleMembershipUpdated(
        bytes32 indexed role,
        address indexed account,
        address indexed sender,
        bool hasRole
    );
    event SupplyCapUpdate(address indexed updater, uint256 newSupplyCap);
    event OwnershipTransferred(address indexed previousOwner, address indexed newOwner);
    event MintWindowSet(uint256 capWei, uint256 windowSecs);
    event MintWindowLoosenProposed(uint256 capWei, uint256 windowSecs, uint256 effectiveAt);
    event MintWindowLoosenCancelled();

    // --- Errors -----------------------------------------------------------

    error Unauthorized();
    error InvalidRecipient();
    error InvalidAmount();
    error InvalidSupplyCap();
    error InsufficientBalance(uint256 currentBalance, uint256 requested, address from);
    error InsufficientAllowance();
    error SupplyCapExceeded();
    error MintWindowCapExceeded();
    error InvalidWindow();
    error NotTightening();
    error NothingPending();
    error TimelockNotElapsed();

    // --- Construction -----------------------------------------------------

    constructor(uint256 initialSupplyCap, address owner_) {
        require(owner_ != address(0), "zero owner");
        owner = owner_;
        supplyCap = initialSupplyCap;
        emit OwnershipTransferred(address(0), owner_);
        emit SupplyCapUpdate(msg.sender, initialSupplyCap);
    }

    // --- ERC-20 read ------------------------------------------------------

    function balanceOf(address account) external view returns (uint256) {
        return _balances[account];
    }

    function allowance(address owner_, address spender) external view returns (uint256) {
        return _allowances[owner_][spender];
    }

    // --- ERC-20 write -----------------------------------------------------

    function transfer(address to, uint256 amount) external returns (bool) {
        _transfer(msg.sender, to, amount);
        return true;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        _allowances[msg.sender][spender] = amount;
        emit Approval(msg.sender, spender, amount);
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        _spendAllowance(from, msg.sender, amount);
        _transfer(from, to, amount);
        return true;
    }

    // --- TIP-20 memo variants --------------------------------------------

    function transferWithMemo(address to, uint256 amount, bytes32 memo) external returns (bool) {
        _transfer(msg.sender, to, amount);
        emit TransferWithMemo(msg.sender, to, amount, memo);
        return true;
    }

    function transferFromWithMemo(address from, address to, uint256 amount, bytes32 memo)
        external
        returns (bool)
    {
        _spendAllowance(from, msg.sender, amount);
        _transfer(from, to, amount);
        emit TransferWithMemo(from, to, amount, memo);
        return true;
    }

    function mintWithMemo(address to, uint256 amount, bytes32 memo) external {
        _mint(to, amount);
        emit TransferWithMemo(address(0), to, amount, memo);
    }

    function burnWithMemo(uint256 amount, bytes32 memo) external {
        _burn(msg.sender, amount);
        emit TransferWithMemo(msg.sender, address(0), amount, memo);
    }

    // --- Mint / burn ------------------------------------------------------

    function mint(address to, uint256 amount) external {
        _mint(to, amount);
    }

    function burn(uint256 amount) external {
        _burn(msg.sender, amount);
    }

    // --- Role admin -------------------------------------------------------

    function grantRole(bytes32 role, address account) external {
        if (msg.sender != owner) revert Unauthorized();
        if (!_roles[role][account]) {
            _roles[role][account] = true;
            emit RoleMembershipUpdated(role, account, msg.sender, true);
        }
    }

    function revokeRole(bytes32 role, address account) external {
        if (msg.sender != owner) revert Unauthorized();
        if (_roles[role][account]) {
            _roles[role][account] = false;
            emit RoleMembershipUpdated(role, account, msg.sender, false);
        }
    }

    function hasRole(bytes32 role, address account) external view returns (bool) {
        return _roles[role][account];
    }

    function transferOwnership(address newOwner) external {
        if (msg.sender != owner) revert Unauthorized();
        require(newOwner != address(0), "zero owner");
        emit OwnershipTransferred(owner, newOwner);
        owner = newOwner;
    }

    function setSupplyCap(uint256 newCap) external {
        if (msg.sender != owner) revert Unauthorized();
        if (newCap < totalSupply) revert InvalidSupplyCap();
        supplyCap = newCap;
        emit SupplyCapUpdate(msg.sender, newCap);
    }

    // --- Global mint rate-limit admin ------------------------------------

    /// TIGHTEN the rolling mint cap — immediate. A change qualifies as a
    /// tightening iff the new cap is finite AND no larger than the current
    /// effective cap (current `0` = uncapped = +∞, so any finite cap tightens)
    /// AND the window is no shorter. Anything else (raise / uncap / shorten)
    /// must go through the time-locked loosen path. Does NOT reset the running
    /// window — already-minted wei still counts against the new cap.
    function tightenMintWindow(uint256 capWei, uint256 windowSecs) external {
        if (msg.sender != owner) revert Unauthorized();
        if (!_isTightening(capWei, windowSecs)) revert NotTightening();
        _applyWindow(capWei, windowSecs);
    }

    /// Propose a LOOSENING (raise cap / uncap / shorten window). Takes effect
    /// only after `CAP_LOOSEN_TIMELOCK`, via `applyLoosenMintWindow` — the
    /// delay window in which a same-block owner-key-compromise drain is caught.
    function proposeLoosenMintWindow(uint256 capWei, uint256 windowSecs) external {
        if (msg.sender != owner) revert Unauthorized();
        if (capWei != 0 && windowSecs == 0) revert InvalidWindow();
        pendingWindowCapWei = capWei;
        pendingWindowSecs = windowSecs;
        pendingWindowEffectiveAt = block.timestamp + CAP_LOOSEN_TIMELOCK;
        emit MintWindowLoosenProposed(capWei, windowSecs, pendingWindowEffectiveAt);
    }

    /// Apply a previously-proposed loosen once the timelock has elapsed.
    /// Callable by anyone (the timelock, not the caller, is the gate).
    function applyLoosenMintWindow() external {
        if (pendingWindowEffectiveAt == 0) revert NothingPending();
        if (block.timestamp < pendingWindowEffectiveAt) revert TimelockNotElapsed();
        _applyWindow(pendingWindowCapWei, pendingWindowSecs);
        pendingWindowEffectiveAt = 0;
    }

    /// Cancel a pending loosen (owner) — the emergency brake if the proposal
    /// itself was the attack.
    function cancelLoosenMintWindow() external {
        if (msg.sender != owner) revert Unauthorized();
        pendingWindowEffectiveAt = 0;
        emit MintWindowLoosenCancelled();
    }

    function _isTightening(uint256 newCap, uint256 newSecs) internal view returns (bool) {
        if (newCap == 0) return false; // uncapping is always a loosening
        bool capOk = mintWindowCapWei == 0 || newCap <= mintWindowCapWei;
        bool secsOk = newSecs >= mintWindowSecs;
        return capOk && secsOk;
    }

    function _applyWindow(uint256 capWei, uint256 windowSecs) internal {
        if (capWei != 0 && windowSecs == 0) revert InvalidWindow();
        mintWindowCapWei = capWei;
        mintWindowSecs = windowSecs;
        emit MintWindowSet(capWei, windowSecs);
    }

    // --- TIP-20 metadata stubs -------------------------------------------

    function paused() external pure returns (bool) {
        return false;
    }

    // --- Internal ---------------------------------------------------------

    function _transfer(address from, address to, uint256 amount) internal {
        if (to == address(0)) revert InvalidRecipient();
        uint256 fromBal = _balances[from];
        if (fromBal < amount) revert InsufficientBalance(fromBal, amount, from);
        unchecked {
            _balances[from] = fromBal - amount;
        }
        _balances[to] += amount;
        emit Transfer(from, to, amount);
    }

    function _spendAllowance(address from, address spender, uint256 amount) internal {
        uint256 allowed = _allowances[from][spender];
        if (allowed != type(uint256).max) {
            if (allowed < amount) revert InsufficientAllowance();
            unchecked {
                _allowances[from][spender] = allowed - amount;
            }
        }
    }

    function _mint(address to, uint256 amount) internal {
        if (!_roles[ISSUER_ROLE][msg.sender]) revert Unauthorized();
        if (to == address(0)) revert InvalidRecipient();
        if (amount == 0) revert InvalidAmount();
        // Global rolling-window ceiling (C1): bounds EVERY mint path, not just
        // the one we remembered to route. `0` cap = disabled.
        if (mintWindowCapWei != 0) {
            if (block.timestamp >= mintWindowStart + mintWindowSecs) {
                mintWindowStart = block.timestamp;
                mintedInWindow = 0;
            }
            uint256 windowTotal = mintedInWindow + amount;
            if (windowTotal > mintWindowCapWei) revert MintWindowCapExceeded();
            mintedInWindow = windowTotal;
        }
        uint256 newSupply = totalSupply + amount;
        if (newSupply > supplyCap) revert SupplyCapExceeded();
        totalSupply = newSupply;
        _balances[to] += amount;
        emit Transfer(address(0), to, amount);
        emit Mint(to, amount);
    }

    function _burn(address from, uint256 amount) internal {
        uint256 bal = _balances[from];
        if (bal < amount) revert InsufficientBalance(bal, amount, from);
        unchecked {
            _balances[from] = bal - amount;
            totalSupply -= amount;
        }
        emit Transfer(from, address(0), amount);
        emit Burn(from, amount);
    }
}
