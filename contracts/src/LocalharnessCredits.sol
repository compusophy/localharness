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

    // --- Errors -----------------------------------------------------------

    error Unauthorized();
    error InvalidRecipient();
    error InvalidAmount();
    error InvalidSupplyCap();
    error InsufficientBalance(uint256 currentBalance, uint256 requested, address from);
    error InsufficientAllowance();
    error SupplyCapExceeded();

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
