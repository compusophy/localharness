// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @title LocalharnessRegistry
/// @notice Minimal subdomain registry for *.localharness.xyz tenants.
///         Surface mirrors ERC-8122 (`register`, `ownerOf`, `setMetadata`)
///         with one addition: a `name -> agentId` reverse index so the
///         apex chrome can answer "is this name taken?" in a single
///         `eth_call`.
///
///         Names are validated on-chain (a-z, 0-9, dash; 3-32 chars; no
///         leading/trailing dash) so the wasm bundle doesn't have to
///         keep its sanitiser and the contract's in sync.
///
///         Numeric `agentId` starts at 1 — id 0 is the "not found"
///         sentinel for `idOfName`. Free registration on testnet; no
///         expiry; one-name-per-address to deter casual squatting
///         (loosen via `transfer` if needed).
///
/// @dev   Intentionally NOT a full ERC-721 / ERC-6909 implementation
///        yet. M9+ may upgrade to ERC-721 so ERC-6551 token-bound
///        accounts derive deterministically; that migration only adds
///        functions, never removes them.
contract LocalharnessRegistry {
    // --- storage ---------------------------------------------------------

    mapping(uint256 => address) public ownerOfId;
    mapping(string  => uint256) public idOfName;     // 0 == unregistered
    mapping(uint256 => string)  public nameOfId;
    mapping(address => uint256) public idOf;          // reverse: one per addr
    mapping(uint256 => mapping(bytes32 => bytes)) public metadata;
    uint256 public nextId = 1;                        // skip 0

    // --- events ----------------------------------------------------------

    event Registered(uint256 indexed agentId, address indexed owner, string name);
    event Transferred(uint256 indexed agentId, address indexed from, address indexed to);
    event MetadataSet(uint256 indexed agentId, bytes32 indexed key, bytes value);

    // --- public mutators -------------------------------------------------

    /// Register `name` to `msg.sender`. Reverts if the name is taken
    /// or if the sender already owns a name.
    function register(string calldata name) external returns (uint256 agentId) {
        require(idOfName[name] == 0, "name taken");
        require(idOf[msg.sender] == 0, "sender already owns one");
        require(_isValidName(name), "invalid name");
        agentId = nextId++;
        ownerOfId[agentId] = msg.sender;
        idOfName[name] = agentId;
        nameOfId[agentId] = name;
        idOf[msg.sender] = agentId;
        emit Registered(agentId, msg.sender, name);
    }

    /// Hand a name off to another address. Resets the recipient's
    /// "one per address" slot — they must not already own one.
    function transfer(uint256 agentId, address to) external {
        require(ownerOfId[agentId] == msg.sender, "not owner");
        require(to != address(0), "burn via release");
        require(idOf[to] == 0, "recipient already owns one");
        delete idOf[msg.sender];
        ownerOfId[agentId] = to;
        idOf[to] = agentId;
        emit Transferred(agentId, msg.sender, to);
    }

    /// Owner-only metadata write. Standard keys we expect to see used:
    ///   `bytes32("description")` → free-form description bytes
    ///   `bytes32("avatar")`      → ipfs:// or https:// URL
    ///   `bytes32("agent_uri")`   → off-chain registration file URL
    /// Reading is via the public `metadata` mapping.
    function setMetadata(uint256 agentId, bytes32 key, bytes calldata value) external {
        require(ownerOfId[agentId] == msg.sender, "not owner");
        metadata[agentId][key] = value;
        emit MetadataSet(agentId, key, value);
    }

    // --- views (convenience; storage mappings are also public) -----------

    function isTaken(string calldata name) external view returns (bool) {
        return idOfName[name] != 0;
    }

    function ownerOfName(string calldata name) external view returns (address) {
        uint256 id = idOfName[name];
        return id == 0 ? address(0) : ownerOfId[id];
    }

    // --- internals -------------------------------------------------------

    function _isValidName(string memory name) internal pure returns (bool) {
        bytes memory b = bytes(name);
        if (b.length < 3 || b.length > 32) return false;
        // No leading/trailing dash.
        if (b[0] == 0x2d || b[b.length - 1] == 0x2d) return false;
        for (uint256 i = 0; i < b.length; i++) {
            bytes1 c = b[i];
            bool ok =
                (c >= 0x30 && c <= 0x39) || // 0-9
                (c >= 0x61 && c <= 0x7a) || // a-z
                (c == 0x2d);                // -
            if (!ok) return false;
        }
        return true;
    }
}
