// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibDiamond} from "../libraries/LibDiamond.sol";
import {LibRegistryStorage} from "../libraries/LibRegistryStorage.sol";
import {LibCreditsStorage} from "../libraries/LibCreditsStorage.sol";
import {LibRegistrationCostStorage} from "../libraries/LibRegistrationCostStorage.sol";

interface IERC20Min {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
}

/// @title LocalharnessRegistryFacet
/// @notice Subdomain registry — port of the flat `LocalharnessRegistry`
///         contract to a Diamond facet. Surface:
///         `register / setMetadata / isTaken / ownerOfName`
///         plus the storage-getter views `ownerOfId / idOfName /
///         nameOfId / idOf / nextId / metadata` and the cost-gate
///         knobs `setRegistrationCost / registrationCost`.
///
///         Storage lives in `LibRegistryStorage` at a dedicated slot
///         (`keccak256("localharness.registry.storage.v1")`) so
///         future facets — ERC-721 conformance, ERC-8004 reputation,
///         ERC-6551 helpers, MPP payments — can be added without
///         collision. Cost-gate state lives in
///         `LibRegistrationCostStorage` at its own slot for the same
///         reason.
contract LocalharnessRegistryFacet {
    event Registered(uint256 indexed agentId, address indexed owner, string name);
    event MetadataSet(uint256 indexed agentId, bytes32 indexed key, bytes value);
    event RegistrationCostUpdated(uint256 oldCostWei, uint256 newCostWei);

    /// `name` is not a valid DNS label (1-63 bytes of lowercase a-z / 0-9 /
    /// hyphen, no leading or trailing hyphen). Mirrors the CLI's
    /// `name_is_valid` so a direct contract call can't mint an unreachable
    /// "ghost" subdomain (uppercase / underscore / emoji / oversized break
    /// DNS routing).
    error InvalidName(string name);
    // ERC-721 Transfer event — emitted on register (mint, from = 0) and
    // on the proper ERC-721 transferFrom (lives in ERC721Facet).
    event Transfer(address indexed from, address indexed to, uint256 indexed tokenId);

    // --- public mutators -------------------------------------------------

    /// Register `name` to `msg.sender`. Mints an ERC-721 token whose
    /// tokenId == agentId. One name -> one tokenId; an address can hold
    /// many tokens (multi-agent ownership is the intended path for
    /// wallets-per-agent via ERC-6551).
    ///
    /// When the registration cost is non-zero AND the credits token is
    /// configured, charges `costWei` from `msg.sender` to the diamond's
    /// own balance via `transferFrom`. Caller must approve the diamond
    /// for at least `costWei` ahead of time (typically batched into the
    /// same sponsored Tempo tx as the register call).
    function register(string calldata name) external returns (uint256 agentId) {
        LibRegistryStorage.Storage storage s = LibRegistryStorage.load();
        // Defense-in-depth: reject malformed names BEFORE any mint / state
        // write so a direct contract call can't bypass the CLI guard and
        // mint a DNS-unreachable ghost. Validate first — an invalid name is
        // an invalid name regardless of whether it happens to be taken.
        if (!_isValidName(name)) revert InvalidName(name);
        require(s.idOfName[name] == 0, "name taken");
        // Token IDs MUST start at 1. `idOfName[name] == 0` is the "name is
        // free" sentinel, so a token with id 0 would make its own name read
        // as unclaimed — anyone could re-register it and overwrite
        // ownerOfId[0], stealing the name/NFT. DiamondInit seeds nextId=1,
        // but lazy-init here too so a facet cut / redeploy that forgets the
        // initializer can never mint token 0.
        if (s.nextId == 0) s.nextId = 1;
        agentId = s.nextId++;
        s.ownerOfId[agentId] = msg.sender;
        s.idOfName[name] = agentId;
        s.nameOfId[agentId] = name;
        s.idOf[msg.sender] = agentId; // "most recent" for back-compat reads
        s.balanceOf[msg.sender] += 1;
        emit Registered(agentId, msg.sender, name);
        emit Transfer(address(0), msg.sender, agentId);

        _chargeRegistrationCost();
    }

    function _chargeRegistrationCost() internal {
        uint256 costWei = LibRegistrationCostStorage.load().costWei;
        if (costWei == 0) return;
        address creditsToken = LibCreditsStorage.load().creditsToken;
        if (creditsToken == address(0)) return;
        // External call last (CEI). transferFrom reverts on insufficient
        // balance or insufficient allowance — the whole register tx then
        // reverts atomically, so the user never gets the name without
        // paying.
        require(
            IERC20Min(creditsToken).transferFrom(msg.sender, address(this), costWei),
            "registration: transfer failed"
        );
    }

    /// Set the per-registration cost. Owner-only. Zero disables the
    /// cost gate entirely (registration is free). Emitted event lets
    /// off-chain indexers track price changes.
    function setRegistrationCost(uint256 newCostWei) external {
        LibDiamond.enforceIsContractOwner();
        LibRegistrationCostStorage.Storage storage s = LibRegistrationCostStorage.load();
        emit RegistrationCostUpdated(s.costWei, newCostWei);
        s.costWei = newCostWei;
    }

    function registrationCost() external view returns (uint256) {
        return LibRegistrationCostStorage.load().costWei;
    }

    // --- Treasury (LH accumulated from `register` fees) ----------------

    event TreasuryWithdrawn(address indexed to, uint256 amount);

    /// LH balance the diamond holds. Reads the credits token's
    /// `balanceOf(address(this))` directly so the value stays accurate
    /// without needing a separate accumulator field.
    function treasuryBalance() external view returns (uint256) {
        address creditsToken = LibCreditsStorage.load().creditsToken;
        if (creditsToken == address(0)) return 0;
        // Inline balanceOf call — avoids importing a full IERC20 just
        // for this single selector.
        (bool ok, bytes memory ret) =
            creditsToken.staticcall(abi.encodeWithSignature("balanceOf(address)", address(this)));
        if (!ok || ret.length < 32) return 0;
        return abi.decode(ret, (uint256));
    }

    /// Owner-only treasury withdrawal. The diamond IS the holder, so a
    /// plain `transfer(to, amount)` against the credits token sends
    /// from `_balances[diamond]` directly — no allowance ceremony.
    /// Used to recycle accumulated registration fees: owner can
    /// redistribute, refund users, or burn (transfer to a sink).
    function withdrawTreasury(address to, uint256 amount) external {
        LibDiamond.enforceIsContractOwner();
        require(to != address(0), "treasury: zero recipient");
        address creditsToken = LibCreditsStorage.load().creditsToken;
        require(creditsToken != address(0), "treasury: token unset");
        (bool ok, bytes memory ret) =
            creditsToken.call(abi.encodeWithSignature("transfer(address,uint256)", to, amount));
        require(ok && (ret.length == 0 || abi.decode(ret, (bool))), "treasury: transfer failed");
        emit TreasuryWithdrawn(to, amount);
    }

    function setMetadata(uint256 agentId, bytes32 key, bytes calldata value) external {
        LibRegistryStorage.Storage storage s = LibRegistryStorage.load();
        require(s.ownerOfId[agentId] == msg.sender, "not owner");
        s.metadata[agentId][key] = value;
        emit MetadataSet(agentId, key, value);
    }

    // --- views -----------------------------------------------------------

    function isTaken(string calldata name) external view returns (bool) {
        return LibRegistryStorage.load().idOfName[name] != 0;
    }

    function ownerOfName(string calldata name) external view returns (address) {
        LibRegistryStorage.Storage storage s = LibRegistryStorage.load();
        uint256 id = s.idOfName[name];
        return id == 0 ? address(0) : s.ownerOfId[id];
    }

    function ownerOfId(uint256 agentId) external view returns (address) {
        return LibRegistryStorage.load().ownerOfId[agentId];
    }

    function idOfName(string calldata name) external view returns (uint256) {
        return LibRegistryStorage.load().idOfName[name];
    }

    function nameOfId(uint256 agentId) external view returns (string memory) {
        return LibRegistryStorage.load().nameOfId[agentId];
    }

    function idOf(address owner) external view returns (uint256) {
        return LibRegistryStorage.load().idOf[owner];
    }

    function nextId() external view returns (uint256) {
        return LibRegistryStorage.load().nextId;
    }

    function metadata(uint256 agentId, bytes32 key) external view returns (bytes memory) {
        return LibRegistryStorage.load().metadata[agentId][key];
    }

    // --- internals -------------------------------------------------------

    /// A valid DNS label, EXACTLY matching the CLI's `name_is_valid`
    /// (src/bin/localharness.rs): 1-63 bytes, every byte lowercase `a-z`
    /// (0x61-0x7a) / digit `0-9` (0x30-0x39) / hyphen `-` (0x2d), and the
    /// first + last byte are NOT a hyphen (RFC 1035 — `-foo` / `foo-` are
    /// dead-on-arrival subdomains). A multi-byte UTF-8 char (emoji) fails
    /// the per-byte range check; uppercase fails the `a-z` range.
    function _isValidName(string memory name) internal pure returns (bool) {
        bytes memory b = bytes(name);
        if (b.length < 1 || b.length > 63) return false;
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
