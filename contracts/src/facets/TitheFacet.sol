// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibTitheStorage} from "../libraries/LibTitheStorage.sol";
import {LibGuildStorage} from "../libraries/LibGuildStorage.sol";
import {LibCreditsStorage} from "../libraries/LibCreditsStorage.sol";

/// The slice of the `$LH` token (`LocalharnessCredits`, TIP-20) this facet
/// reaches for: the same `transferFrom`-pull GuildFacet.fundGuild uses, plus
/// the `balanceOf` / `allowance` reads needed to SIZE a tithe (bps of the
/// account's current balance, capped by what it has approved). Declared
/// minimally so the dependency is legible and a test mock needs three
/// selectors.
interface IERC20Tithe {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function balanceOf(address account) external view returns (uint256);
    function allowance(address owner, address spender) external view returns (uint256);
}

/// @title TitheFacet
/// @notice OPT-IN, PERMISSIONLESS-PULL auto-tithe — the revenue→treasury
///         automation that makes a guild self-funding from its members'
///         earnings WITHOUT a tab and WITHOUT a per-contribution signature.
///
///         THE FLOW (three calls, two of them self-only):
///           1. `setTithe(guildId, bps)` — the agent's token-bound account
///              CONSENTS to tithing `bps/10000` of its `$LH` balance to
///              `guildId`. KEYED ON `msg.sender`, so an account only ever
///              configures ITSELF. (The account ALSO one-time `approve`s the
///              diamond to spend its `$LH` — batched with this in one
///              sponsored Tempo tx by the client; the approve is the standing
///              allowance the pull draws against, and is the account's hard
///              upper bound on total tithed.)
///           2. `collectTithe(account)` — PERMISSIONLESS to call (a scheduler,
///              a guild officer, anyone). Reads ONLY `account`'s OWN stored
///              `(guildId, bps)`, computes `amount = bps * balanceOf(account)
///              / 10000` capped by the account's remaining allowance, pulls it
///              `transferFrom(account → diamond)`, and credits
///              `LibGuildStorage.guildBalance[guildId]` — the EXACT same effect
///              as `fundGuild`. The caller chooses WHEN; the account already
///              chose WHO and HOW MUCH, so a hostile trigger can move funds
///              only where the account itself consented.
///           3. `revokeTithe()` — the account clears its OWN config (keyed on
///              `msg.sender`); subsequent `collectTithe` calls revert.
///
///         WHY PERMISSIONLESS TRIGGERING IS SAFE (the threat model):
///           • collectTithe can NOT redirect funds — `guildId`/`bps` come from
///             the ACCOUNT's stored config, never from the caller. A griefer
///             triggering it merely does the account's chosen tithe early.
///           • collectTithe can NOT over-pull — `amount` is bounded by both
///             the live balance (bps ≤ 100%) AND the remaining allowance
///             (`min(bpsAmount, allowance)`); the account's `approve` is the
///             ceiling on cumulative tithing.
///           • collectTithe can NOT drain via reentrancy — CEI: the guild
///             ledger is credited BEFORE the external `transferFrom`, the same
///             ordering as `fundGuild`/BountyFacet, so a hostile token can't
///             double-credit.
///           • the guild must EXIST — credits land in a real guild's treasury
///             (the `fundGuild` invariant), never an unknown id.
///
///         NO NEW VALUE PATH: a tithe IS a `fundGuild`, just self-consented
///         and permissionlessly triggerable. The `$LH` lives in the diamond;
///         `LibGuildStorage.guildBalance` is the per-guild ledger; the guild's
///         Admin `spendTreasury`s it exactly as a manual fund.
///
///         CUTTING IT (diamond owner; mirror script/AddGuildFacet): deploy +
///         diamondCut Add the 4 selectors in script/AddTitheFacet.s.sol. No
///         post-cut config — the credits token is read from the shared
///         CreditsFacet storage slot, the guild ledger from the shared
///         GuildFacet slot (GuildFacet + CreditsFacet must already be cut,
///         which they are on the live diamond).
contract TitheFacet {
    // --- Events ---------------------------------------------------------

    /// An account set/updated its tithe consent.
    event TitheSet(address indexed account, uint256 indexed guildId, uint256 bps);
    /// An account cleared its tithe consent.
    event TitheRevoked(address indexed account);
    /// A tithe was collected: `amount` `$LH` pulled from `account` into
    /// `guildId`'s treasury (mirrors GuildFacet's `GuildFunded` accounting).
    event TitheCollected(address indexed account, uint256 indexed guildId, uint256 amount);

    // --- Errors ---------------------------------------------------------

    error NotConfigured();    // credits token unset / no tithe config for the account
    error UnknownGuild();     // the account's configured guild does not exist
    error InvalidBps();       // setTithe with bps == 0 or bps > MAX_BPS
    error NothingToCollect(); // computed tithe amount is 0 (zero balance / zero allowance)

    // --- Config (self-only: keyed on msg.sender) ------------------------

    /// Opt IN to tithing `bps/10000` of THIS account's `$LH` balance to
    /// `guildId`. Self-only — keyed on `msg.sender`, so an agent's TBA only
    /// ever configures itself (this is why permissionless `collectTithe` is
    /// safe). `bps` must be 1..=MAX_BPS (10000 = 100%); 0 is rejected — use
    /// `revokeTithe` to opt out. The guild must exist (no tithing into a void).
    /// Calling again OVERWRITES the prior config (change rate or target guild).
    ///
    /// NOTE: this only records consent. The account must separately `approve`
    /// the diamond on the `$LH` token for the standing allowance the pull
    /// draws against — the client batches `approve` + `setTithe` into one
    /// sponsored tx. The allowance is the account's hard ceiling on cumulative
    /// tithing; collection never pulls beyond it.
    function setTithe(uint256 guildId, uint256 bps) external {
        if (bps == 0 || bps > LibTitheStorage.MAX_BPS) revert InvalidBps();
        if (!LibGuildStorage.load().guilds[guildId].exists) revert UnknownGuild();

        LibTitheStorage.Config storage c = LibTitheStorage.load().configOf[msg.sender];
        c.guildId = guildId;
        c.bps = bps;
        emit TitheSet(msg.sender, guildId, bps);
    }

    /// Opt OUT — clear THIS account's tithe consent. Self-only (keyed on
    /// `msg.sender`). After this, `collectTithe(msg.sender)` reverts
    /// `NotConfigured` until a new `setTithe`. Does NOT touch the `$LH`
    /// allowance (the account revokes that on the token itself if desired).
    function revokeTithe() external {
        delete LibTitheStorage.load().configOf[msg.sender];
        emit TitheRevoked(msg.sender);
    }

    // --- Collect (PERMISSIONLESS; consent enforced from `account`'s config) -

    /// Collect `account`'s consented tithe — PERMISSIONLESS to call. Reads
    /// ONLY `account`'s OWN stored `(guildId, bps)` (never the caller's), so a
    /// scheduler/officer/anyone can TRIGGER it but cannot redirect or inflate
    /// it. Pulls `min(bps * balanceOf(account) / 10000, remaining allowance)`
    /// `$LH` from `account` into `guildId`'s treasury, crediting
    /// `LibGuildStorage.guildBalance` exactly as `fundGuild` does.
    ///
    /// MONEY-SAFETY (all enforced here):
    ///   • config must exist (`bps != 0`) — else `NotConfigured`.
    ///   • the guild must still exist — else `UnknownGuild` (treasury is real).
    ///   • amount is bps-of-balance (bps ≤ 100% by `setTithe`'s cap, so it can
    ///     never exceed the balance) AND capped by the account's remaining
    ///     allowance to the diamond (the account's hard ceiling).
    ///   • a zero computed amount reverts `NothingToCollect` (no event spam,
    ///     no zero-value `transferFrom` ambiguity).
    ///   • CEI: the guild ledger is credited BEFORE the external `transferFrom`
    ///     — a hostile re-entrant token can't double-credit (same ordering as
    ///     `fundGuild`).
    function collectTithe(address account) external returns (uint256 amount) {
        LibTitheStorage.Config memory c = LibTitheStorage.load().configOf[account];
        if (c.bps == 0) revert NotConfigured();

        LibGuildStorage.Storage storage gs = LibGuildStorage.load();
        if (!gs.guilds[c.guildId].exists) revert UnknownGuild();

        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();

        // Size the tithe: bps of the CURRENT balance (bps ≤ 10000 ⇒ ≤ balance),
        // then clamp to the remaining allowance the account granted the diamond.
        uint256 balance = IERC20Tithe(token).balanceOf(account);
        amount = (balance * c.bps) / LibTitheStorage.MAX_BPS;
        uint256 allowed = IERC20Tithe(token).allowance(account, address(this));
        if (allowed < amount) amount = allowed;
        if (amount == 0) revert NothingToCollect();

        // CEI: credit the guild ledger BEFORE the external pull (exactly the
        // GuildFacet.fundGuild ordering). A revert in transferFrom unwinds the
        // credit with it; a re-entrant token sees the already-credited ledger
        // but a second collect re-reads the reduced balance/allowance.
        gs.guildBalance[c.guildId] += amount;
        require(
            IERC20Tithe(token).transferFrom(account, address(this), amount),
            "tithe: transfer failed"
        );

        emit TitheCollected(account, c.guildId, amount);
    }

    // --- View -----------------------------------------------------------

    /// Read `account`'s tithe consent: `(guildId, bps)`. `bps == 0` means no
    /// config (never set / revoked). Pure read of the account's OWN record.
    function titheOf(address account) external view returns (uint256 guildId, uint256 bps) {
        LibTitheStorage.Config memory c = LibTitheStorage.load().configOf[account];
        return (c.guildId, c.bps);
    }
}
