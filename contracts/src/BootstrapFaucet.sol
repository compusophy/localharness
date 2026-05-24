// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @title BootstrapFaucet — admin-pre-funded distribution for new
///        localharness wallets on Tempo Moderato testnet.
///
/// The bundle calls `fund(recipient)` when a user creates a new
/// identity, so the recipient ends up with enough test ETH to
/// register a name + pay for a few turns without hammering the
/// public `tempo_fundAddress` faucet.
///
/// Anyone can call `fund(recipient)` — the contract doesn't care who
/// the caller is, only that the recipient hasn't claimed before.
/// Caller pays gas. Recipient receives `dripWei`. One claim per
/// recipient ever.
///
/// Admin (deployer, transferable) controls the drip amount, can
/// withdraw the balance, and tops the contract up by sending native
/// ETH directly. No upgradeability — if anything needs to change
/// structurally, deploy a new contract and rewire the bundle's
/// `BOOTSTRAP_FAUCET_ADDRESS` constant.
contract BootstrapFaucet {
    address public owner;
    uint256 public dripWei;
    mapping(address => bool) public claimed;

    event Funded(address indexed recipient, uint256 amount);
    event DripAmountUpdated(uint256 newAmount);
    event OwnerTransferred(address indexed previousOwner, address indexed newOwner);

    /// Deploy with an initial drip amount in wei. Send native ETH at
    /// deploy time to pre-fund the contract.
    constructor(uint256 initialDripWei) payable {
        owner = msg.sender;
        dripWei = initialDripWei;
        emit OwnerTransferred(address(0), msg.sender);
        emit DripAmountUpdated(initialDripWei);
    }

    /// Send `dripWei` to `recipient`. Idempotent per recipient.
    /// Reverts if the recipient already claimed or the contract is
    /// drained. Reverts on transfer failure (e.g. recipient is a
    /// contract that rejects native value).
    function fund(address recipient) external {
        require(recipient != address(0), "zero recipient");
        require(!claimed[recipient], "already claimed");
        require(address(this).balance >= dripWei, "drained");
        claimed[recipient] = true;
        (bool ok, ) = recipient.call{value: dripWei}("");
        require(ok, "transfer failed");
        emit Funded(recipient, dripWei);
    }

    /// Update the per-claim drip amount. Owner-only.
    function setDripAmount(uint256 amount) external onlyOwner {
        dripWei = amount;
        emit DripAmountUpdated(amount);
    }

    /// Transfer ownership to a new admin. Owner-only.
    function transferOwnership(address newOwner) external onlyOwner {
        require(newOwner != address(0), "zero address");
        address previous = owner;
        owner = newOwner;
        emit OwnerTransferred(previous, newOwner);
    }

    /// Withdraw the full contract balance to the owner. Owner-only.
    function withdraw() external onlyOwner {
        (bool ok, ) = payable(owner).call{value: address(this).balance}("");
        require(ok, "withdraw failed");
    }

    /// Accept top-ups via plain ETH transfers from anyone — the admin
    /// uses this to refill the contract from their EOA. No event so
    /// it stays minimal; check `address(this).balance` directly.
    receive() external payable {}

    modifier onlyOwner() {
        require(msg.sender == owner, "not owner");
        _;
    }
}
