// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the OPT-IN auto-tithe — the revenue→treasury
///      automation that lets an agent's earnings flow into a guild it has
///      consented to support, WITHOUT a tab and WITHOUT a per-contribution
///      signature. Diamond storage pattern — a fresh slot, no collision with
///      the registry / TBA / guild / bounty / invite / schedule / credits
///      storage already cut into the diamond. Add new fields ONLY at the end
///      of the struct (and only at the end of `Config`) — diamond storage
///      layout is positional and immutable (the "append-only rule").
///
///      THE CONSENT MODEL (why permissionless collection is SAFE).
///      The config is KEYED ON `account` (the agent's token-bound account)
///      and ONLY that account can write it — `TitheFacet.setTithe` /
///      `revokeTithe` key on `msg.sender`, so an agent only ever configures
///      ITSELF. `collectTithe(account)` is permissionless to CALL (a
///      scheduler, a guild officer, anyone), but it reads ONLY that account's
///      OWN stored `(guildId, bps)` — it cannot redirect funds to a guild the
///      account never chose, nor tithe a higher rate than the account set.
///      The caller picks WHEN; the account already picked WHO and HOW MUCH.
///
///      THE TITHE = a PULL of `bps/10000` of the account's CURRENT `$LH`
///      balance, capped by the account's remaining allowance to the diamond.
///      The pulled `$LH` is credited into `LibGuildStorage.guildBalance`
///      exactly as `fundGuild` does (the diamond physically holds it; the
///      guild ledger tracks each guild's share — the same safe BountyFacet
///      escrow). No new value path: a tithe IS a fund, just self-consented
///      and permissionlessly triggerable.
library LibTitheStorage {
    bytes32 constant POSITION = keccak256("localharness.tithe.storage.v1");

    /// The maximum tithe rate, in basis points (100% = 10000). A config with
    /// `bps == 0` is "not configured" (the sentinel); a `setTithe` of 0 is
    /// rejected (use `revokeTithe`). Capping at 100% is a money-safety bound:
    /// the on-chain math `bps * balance / 10000` can never compute MORE than
    /// the whole balance, so a bad `bps` can't overflow into an absurd pull.
    uint256 internal constant MAX_BPS = 10000;

    /// One account's opt-in tithe consent. `bps == 0` ⇔ no config (the
    /// `revokeTithe` / never-set state). Keyed by the consenting account in
    /// `configOf`, so the record can only ever describe the account itself.
    struct Config {
        uint256 guildId; // the guild the account consents to tithe to
        uint256 bps;     // the rate in basis points (1..=MAX_BPS); 0 = unset
    }

    struct Storage {
        /// account (the agent's TBA) -> its OWN tithe consent. The default
        /// (zero `Config`, `bps == 0`) is "not configured". Only the account
        /// writes its own entry (`setTithe`/`revokeTithe` key on msg.sender).
        mapping(address => Config) configOf;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
