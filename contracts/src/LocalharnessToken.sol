// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @title LocalharnessToken — ERC-20 ($localharness) with a public
///        once-per-address self-faucet. Replaces the native-ETH
///        BootstrapFaucet at 0xA439… which is dormant after Tempo
///        Moderato turned out to refuse EOA↔contract native transfers.
///
/// The faucet mints fresh tokens to the recipient out of thin air —
/// no pre-funding required. `claimed[recipient]` prevents double-dips.
/// Owner controls the per-claim amount + can mint to arbitrary
/// recipients + can transfer ownership.
///
/// All transfers are ERC-20 `transfer` / `transferFrom` calls, which
/// Tempo allows (it only blocks native value transfers). That makes
/// this the working substrate for the visitor-pays-agent loop:
/// `transfer(agent_tba, price)` instead of `rlp_native_transfer`.
///
/// Hand-rolled rather than pulling OZ — the surface we need is tiny
/// and the contract should be auditable in one read.
contract LocalharnessToken {
    // --- ERC-20 metadata ----------------------------------------------

    string public constant name = "localharness";
    string public constant symbol = "localharness";
    uint8 public constant decimals = 18;

    // --- ERC-20 state -------------------------------------------------

    uint256 public totalSupply;
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    event Transfer(address indexed from, address indexed to, uint256 amount);
    event Approval(address indexed owner, address indexed spender, uint256 amount);

    // --- Admin --------------------------------------------------------

    address public owner;
    /// Tokens dispensed per `faucet(recipient)` call. Owner-tunable.
    uint256 public faucetAmount = 1_000 * 10 ** 18; // 1000 LH tokens
    mapping(address => bool) public faucetClaimed;

    event FaucetClaimed(address indexed recipient, uint256 amount);
    event FaucetAmountUpdated(uint256 newAmount);
    event OwnerTransferred(address indexed previousOwner, address indexed newOwner);

    constructor() {
        owner = msg.sender;
        emit OwnerTransferred(address(0), msg.sender);
    }

    // --- ERC-20 ops ---------------------------------------------------

    function transfer(address to, uint256 amount) external returns (bool) {
        _transfer(msg.sender, to, amount);
        return true;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        emit Approval(msg.sender, spender, amount);
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        uint256 a = allowance[from][msg.sender];
        require(a >= amount, "allowance");
        if (a != type(uint256).max) {
            allowance[from][msg.sender] = a - amount;
        }
        _transfer(from, to, amount);
        return true;
    }

    function _transfer(address from, address to, uint256 amount) internal {
        require(to != address(0), "zero to");
        uint256 bal = balanceOf[from];
        require(bal >= amount, "balance");
        unchecked {
            balanceOf[from] = bal - amount;
            balanceOf[to] += amount;
        }
        emit Transfer(from, to, amount);
    }

    // --- Faucet -------------------------------------------------------

    /// Mint `faucetAmount` tokens to `recipient`. Anyone can call on
    /// behalf of any address; one claim per recipient ever. Caller
    /// pays gas, recipient gets the tokens. No pre-funding needed.
    function faucet(address recipient) external {
        require(recipient != address(0), "zero recipient");
        require(!faucetClaimed[recipient], "already claimed");
        faucetClaimed[recipient] = true;
        uint256 amount = faucetAmount;
        totalSupply += amount;
        unchecked {
            balanceOf[recipient] += amount;
        }
        emit Transfer(address(0), recipient, amount);
        emit FaucetClaimed(recipient, amount);
    }

    // --- Admin ops ----------------------------------------------------

    /// Mint arbitrary tokens to any recipient. Owner-only — the
    /// permanent escape hatch for distributing extra allocations
    /// (giveaways, partnerships, top-ups) that don't fit the
    /// per-recipient faucet model.
    function mint(address to, uint256 amount) external onlyOwner {
        require(to != address(0), "zero to");
        totalSupply += amount;
        unchecked {
            balanceOf[to] += amount;
        }
        emit Transfer(address(0), to, amount);
    }

    function setFaucetAmount(uint256 amount) external onlyOwner {
        faucetAmount = amount;
        emit FaucetAmountUpdated(amount);
    }

    function transferOwnership(address newOwner) external onlyOwner {
        require(newOwner != address(0), "zero address");
        address previous = owner;
        owner = newOwner;
        emit OwnerTransferred(previous, newOwner);
    }

    modifier onlyOwner() {
        require(msg.sender == owner, "not owner");
        _;
    }
}
