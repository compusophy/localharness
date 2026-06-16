use k256::ecdsa::SigningKey;

use super::*;

// --- GuildFacet (on-chain orgs: members, roles, pooled $LH treasury) ------
//
// Rung 3 of `design/agent-coordination.md` (bounty → party → GUILD → DAO). A
// guild is an on-chain org with a member roster, per-member roles, and a pooled
// `$LH` treasury an admin/officer can spend. The sibling builds the facet
// against the SAME ABI declared here; reuses InviteFacet's approve→pull escrow
// (`fundGuild`) + X402/transfer payout (`spendTreasury`) + TbaFacet's wallet
// (`guildAddress`). EXACT ABI:
//   createGuild(string name) -> uint256 guildId
//   inviteToGuild(uint256 guildId, address member)
//   acceptGuildInvite(uint256 guildId)
//   leaveGuild(uint256 guildId)
//   setRole(uint256 guildId, address member, uint8 role)  (0 None/1 Member/2 Officer/3 Admin)
//   fundGuild(uint256 guildId, uint256 amount)  (transferFrom caller->diamond; APPROVE first)
//   spendTreasury(uint256 guildId, address to, uint256 amount, bytes memo)
//   reads: membersOf(uint256)->address[] | roleOf(uint256,address)->uint8
//          isGuildMember(uint256,address)->bool | treasuryBalanceOf(uint256)->uint256
//          guildAddress(uint256)->address | guildName(uint256)->string
//          guildsOf(address)->uint256[] | guildCount()->uint256
//
// NOTE ON SELECTOR COLLISIONS: the diamond can't share a selector across facets.
// If the sibling renamed a colliding selector with a `guild` prefix on the live
// diamond (e.g. `setRole`→`setGuildRole`, `membersOf`→`guildMembersOf`,
// `isGuildMember`→`isGuildMemberOf`), a READ here will revert/return empty —
// flag a rename as the likely cause when a guild read fails unexpectedly. The
// selectors below match the spec'd ABI verbatim; bump them together with the
// facet if a rename lands.

/// A member's role within a guild. The on-chain `uint8` enum
/// (`0 None / 1 Member / 2 Officer / 3 Admin`) decoded into a typed value, so
/// the CLI/browser never juggle raw bytes. `None` = not a member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuildRole {
    /// Not a member (the zero role).
    None,
    /// A plain member.
    Member,
    /// An officer (can spend the treasury; below admin).
    Officer,
    /// An admin (full control: roles + treasury).
    Admin,
}

impl GuildRole {
    /// Decode the on-chain `uint8` role byte. Unknown values clamp to `None`.
    pub fn from_u8(v: u8) -> GuildRole {
        match v {
            1 => GuildRole::Member,
            2 => GuildRole::Officer,
            3 => GuildRole::Admin,
            _ => GuildRole::None,
        }
    }

    /// The on-chain `uint8` value for this role (the inverse of
    /// [`GuildRole::from_u8`]).
    pub fn as_u8(self) -> u8 {
        match self {
            GuildRole::None => 0,
            GuildRole::Member => 1,
            GuildRole::Officer => 2,
            GuildRole::Admin => 3,
        }
    }

    /// Lowercase human label (`none`/`member`/`officer`/`admin`).
    pub fn label(self) -> &'static str {
        match self {
            GuildRole::None => "none",
            GuildRole::Member => "member",
            GuildRole::Officer => "officer",
            GuildRole::Admin => "admin",
        }
    }

    /// Parse a user-typed role word (`member`/`officer`/`admin`,
    /// case-insensitive). `none` is rejected — it's a removal, not a settable
    /// rank (use `leaveGuild`/role-0 paths deliberately, not a typo). Pure.
    pub fn parse(raw: &str) -> Result<GuildRole, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "member" => Ok(GuildRole::Member),
            "officer" => Ok(GuildRole::Officer),
            "admin" => Ok(GuildRole::Admin),
            other => Err(format!(
                "invalid role '{other}' — expected member, officer, or admin"
            )),
        }
    }
}

/// Encode `createGuild(string name)` — one dynamic `string` arg (offset 0x20 +
/// length + padded UTF-8 bytes), the SAME layout as `register(string)`. Returns
/// raw calldata for a `TempoCall.input`.
pub(crate) fn encode_create_guild(name: &str) -> Vec<u8> {
    let bytes = name.as_bytes();
    let padded_len = bytes.len().div_ceil(32) * 32;
    let mut out = Vec::with_capacity(4 + 32 + 32 + padded_len);
    out.extend_from_slice(&selector("createGuild(string)"));
    out.extend_from_slice(&u256_be(0x20)); // offset to the string head
    out.extend_from_slice(&u256_be(bytes.len() as u128)); // length
    out.extend_from_slice(bytes);
    out.resize(out.len() + (padded_len - bytes.len()), 0); // right-pad
    out
}

/// Encode `inviteToGuild(uint256 guildId, address member)` — two static head
/// words (guildId, then the member address right-aligned in word 1).
pub(crate) fn encode_invite_to_guild(guild_id: u64, member: &[u8; 20]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 64);
    out.extend_from_slice(&selector("inviteToGuild(uint256,address)"));
    out.extend_from_slice(&u256_be(guild_id as u128));
    out.extend_from_slice(&addr_word(member));
    out
}

/// Encode `setRole(uint256 guildId, address member, uint8 role)` — three static
/// head words (guildId, member address, role byte right-aligned).
pub(crate) fn encode_set_role(guild_id: u64, member: &[u8; 20], role: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 96);
    out.extend_from_slice(&selector("setRole(uint256,address,uint8)"));
    out.extend_from_slice(&u256_be(guild_id as u128));
    out.extend_from_slice(&addr_word(member));
    out.extend_from_slice(&u256_be(role as u128));
    out
}

/// Encode `fundGuild(uint256 guildId, uint256 amount)` — two static head words.
/// Batched AFTER an `approve(diamond, amount)` (the facet `transferFrom`s the
/// caller→diamond inside its body, the InviteFacet escrow shape).
pub(crate) fn encode_fund_guild(guild_id: u64, amount_wei: u128) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 64);
    out.extend_from_slice(&selector("fundGuild(uint256,uint256)"));
    out.extend_from_slice(&u256_be(guild_id as u128));
    out.extend_from_slice(&u256_be(amount_wei));
    out
}

/// Encode `spendTreasury(uint256 guildId, address to, uint256 amount, bytes
/// memo)`. Head: guildId(0) / to(1) / amount(2) / OFFSET-to-memo(3) = 4 fixed
/// head words = `4 * 32` = 0x80; tail = `[length][padded memo bytes]`. The ONLY
/// dynamic arg is `memo`, and it is LAST, so the offset is the full head size.
pub(crate) fn encode_spend_treasury(guild_id: u64, to: &[u8; 20], amount_wei: u128, memo: &[u8]) -> Vec<u8> {
    let padded_len = memo.len().div_ceil(32) * 32;
    let mut out = Vec::with_capacity(4 + 4 * 32 + 32 + padded_len);
    out.extend_from_slice(&selector("spendTreasury(uint256,address,uint256,bytes)"));
    out.extend_from_slice(&u256_be(guild_id as u128)); // head 0: guildId
    out.extend_from_slice(&addr_word(to)); // head 1: to
    out.extend_from_slice(&u256_be(amount_wei)); // head 2: amount
    out.extend_from_slice(&u256_be(4 * 32)); // head 3: offset to memo tail
    out.extend_from_slice(&u256_be(memo.len() as u128)); // tail: length
    out.extend_from_slice(memo); // tail: memo bytes
    out.resize(out.len() + (padded_len - memo.len()), 0); // right-pad
    out
}

/// Public ABI calldata for `acceptGuildInvite(uint256 guildId)` — the inner
/// call a guild's TBA executes to join a PARENT guild (nested divisions). Thin
/// `pub` wrapper over the same `call_uint_bytes` the sponsored helper uses, so a
/// caller (the CLI `guild accept --tba`) can route it through
/// `tba_execute_call_sponsored` without re-rolling ABI.
pub fn encode_accept_guild_invite_calldata(guild_id: u64) -> Vec<u8> {
    call_uint_bytes("acceptGuildInvite(uint256)", guild_id)
}

/// Create a guild via a sponsored Tempo tx: `createGuild(name)` mints the org
/// (caller becomes its Admin), returning the tx hash once mined. Read the new
/// guildId back from `guilds_of(creator)` (its last entry, like `bounties_of`).
pub async fn create_guild_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    name: &str,
    fee_token: &str,
) -> Result<String, String> {
    // The guild struct's cold SSTOREs (id↔owner, name `bytes`, the creator's
    // Admin role + the `guildsOf` enumerable push) + ~275k sponsorship. Cold
    // writes dominate (CLAUDE.md "cast estimate, never guess"); budget the same
    // base + per-byte the on-chain name write costs. Sponsor billed on gas USED.
    // Measured live: `cast estimate createGuild` ≈ 2.87M (the full name mint —
    // ERC721 + name↔id + ownerOfId + MAIN — plus the guild struct). A 1.5M base
    // OOG'd the live tx (static call succeeded → pure gas). Budget 3.5M base like
    // scheduleJob (comfortably above 2.87M + sponsorship overhead). Sponsor billed
    // on gas USED, so the headroom is free.
    let gas = 3_500_000 + (name.len() as u128) * 9_000;
    sponsored_diamond_call(sender, fee_payer, encode_create_guild(name), fee_token, gas).await
}

/// Invite an address to a guild via a sponsored Tempo tx
/// (`inviteToGuild(guildId, member)`). The invitee then `acceptGuildInvite`s to
/// join. Admin/officer-gated on-chain.
pub async fn invite_to_guild_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    guild_id: u64,
    member_hex: &str,
    fee_token: &str,
) -> Result<String, String> {
    let member = parse_eth_address(member_hex)?;
    // A pending-invite SSTORE + event. 400k mirrors the bounty-claim budget.
    sponsored_diamond_call(
        sender,
        fee_payer,
        encode_invite_to_guild(guild_id, &member),
        fee_token,
        400_000,
    )
    .await
}

/// Accept a pending guild invite via a sponsored Tempo tx
/// (`acceptGuildInvite(guildId)`): the caller joins as a Member, added to the
/// roster + `guildsOf` index.
pub async fn accept_guild_invite_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    guild_id: u64,
    fee_token: &str,
) -> Result<String, String> {
    // Role SSTORE + the roster + `guildsOf` enumerable pushes + event — cold
    // index writes dominate. Measured: `cast estimate acceptGuildInvite` ≈ 1.33M
    // (a 1.0M limit OOG'd live, gasUsed pinned at the cap). Budget 2M (sponsor
    // billed on gas USED, so the headroom is free).
    sponsored_diamond_call(
        sender,
        fee_payer,
        call_uint_bytes("acceptGuildInvite(uint256)", guild_id),
        fee_token,
        2_000_000,
    )
    .await
}

/// Leave a guild via a sponsored Tempo tx (`leaveGuild(guildId)`): the caller's
/// role is cleared and they're removed from the roster + `guildsOf` index.
pub async fn leave_guild_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    guild_id: u64,
    fee_token: &str,
) -> Result<String, String> {
    // Role clear + the roster/index removals (swap-and-pop array writes, symmetric
    // to accept's pushes) + event. Budget 1.5M like accept (the 600k guess was the
    // same under-estimate class as the createGuild/accept OOGs).
    sponsored_diamond_call(
        sender,
        fee_payer,
        call_uint_bytes("leaveGuild(uint256)", guild_id),
        fee_token,
        1_500_000,
    )
    .await
}

/// Set a member's role via a sponsored Tempo tx (`setRole(guildId, member,
/// role)`). `role` is the raw `uint8` (0 None / 1 Member / 2 Officer / 3 Admin)
/// — pass [`GuildRole::as_u8`]. Admin-gated on-chain.
pub async fn set_role_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    guild_id: u64,
    member_hex: &str,
    role: u8,
    fee_token: &str,
) -> Result<String, String> {
    let member = parse_eth_address(member_hex)?;
    // A single role SSTORE + event.
    sponsored_diamond_call(
        sender,
        fee_payer,
        encode_set_role(guild_id, &member, role),
        fee_token,
        400_000,
    )
    .await
}

/// Fund a guild's treasury via a sponsored Tempo tx. Batches `approve(diamond,
/// amount)` on `$LH` + `fundGuild(guildId, amount)` in ONE tx — `fundGuild`
/// then escrows via `transferFrom(caller, diamond, amount)` inside its body (the
/// identical approve→pull pattern as `post_bounty_sponsored` /
/// `create_invite_sponsored`). The `$LH` leaves the caller's spendable balance
/// the moment this mines and lands in the guild's pooled treasury.
pub async fn fund_guild_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    guild_id: u64,
    amount_wei: u128,
    fee_token: &str,
) -> Result<String, String> {
    fund_guild_sponsored_bridged(sender, fee_payer, guild_id, amount_wei, fee_token, 0).await
}

/// [`fund_guild_sponsored`] with the meter auto-bridge: `bridge_wei > 0`
/// prepends `withdrawCredits(bridge_wei)` in the SAME atomic tx so unspent
/// chat-meter credits can back the contribution (see
/// `sponsored_escrow_diamond_call_bridged`).
pub async fn fund_guild_sponsored_bridged(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    guild_id: u64,
    amount_wei: u128,
    fee_token: &str,
    bridge_wei: u128,
) -> Result<String, String> {
    // approve (~46k) + fundGuild (transferFrom pull + the treasury-balance
    // SSTORE + event) + ~275k sponsorship. Mirror the invite escrow budget.
    sponsored_escrow_diamond_call_bridged(
        sender,
        fee_payer,
        amount_wei,
        encode_fund_guild(guild_id, amount_wei),
        fee_token,
        2_000_000,
        bridge_wei,
    )
    .await
}

/// Spend from a guild's treasury via a sponsored Tempo tx
/// (`spendTreasury(guildId, to, amount, memo)`): pays `amount` `$LH` from the
/// pooled treasury to `to`, with an optional `memo` recorded on-chain.
/// Admin/officer-gated on-chain. The `memo` `bytes` write scales the gas.
pub async fn spend_treasury_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    guild_id: u64,
    to_hex: &str,
    amount_wei: u128,
    memo: &[u8],
    fee_token: &str,
) -> Result<String, String> {
    let to = parse_eth_address(to_hex)?;
    // treasury-balance debit + the payout `transfer` (cold token balances) + the
    // cold `memo` bytes (~9k/byte) + event. Base mirrors the redeem/payout budget.
    let gas = 2_000_000 + (memo.len() as u128) * 9_000;
    sponsored_diamond_call(
        sender,
        fee_payer,
        encode_spend_treasury(guild_id, &to, amount_wei, memo),
        fee_token,
        gas,
    )
    .await
}

/// Read `membersOf(guildId)` → the guild's member roster as lowercase `0x…`
/// addresses. Bare dynamic `address[]` ABI return (`[offset][len][addr0]…`), the
/// SAME decode as `devices_of`. Hostile-length-safe (no pre-alloc; checked
/// index math stops the decode on a bogus length).
pub async fn members_of_guild(guild_id: u64) -> Result<Vec<String>, String> {
    let result = read_view(
        selector("guildMembersOf(uint256)"),
        &[u256_be(guild_id as u128)],
    )
    .await?;
    let bytes = hex_to_bytes(&result)?;
    Ok(decode_address_array(&bytes))
}

/// Read `roleOf(guildId, member)` → the member's [`GuildRole`] (decoded from the
/// `uint8` enum; `None` for a non-member).
pub async fn role_of_guild(guild_id: u64, addr_hex: &str) -> Result<GuildRole, String> {
    let addr = parse_eth_address(addr_hex)?;
    let result = read_view(
        selector("roleOf(uint256,address)"),
        &[u256_be(guild_id as u128), addr_word(&addr)],
    )
    .await?;
    // uint8 enum right-aligned in a 32-byte word — read the low byte via u64.
    let v = decode_u256_as_u64(&result)?;
    Ok(GuildRole::from_u8(v as u8))
}

/// Read `isGuildMember(guildId, member)` → whether the address is on the roster.
/// The single-read membership check (no roster walk).
pub async fn is_guild_member(guild_id: u64, addr_hex: &str) -> Result<bool, String> {
    let addr = parse_eth_address(addr_hex)?;
    let result = read_view(
        selector("isGuildMember(uint256,address)"),
        &[u256_be(guild_id as u128), addr_word(&addr)],
    )
    .await?;
    decode_u256_as_u64(&result).map(|v| v != 0)
}

/// Read `treasuryBalanceOf(guildId)` → the guild's pooled `$LH` (18-decimal wei).
pub async fn treasury_balance_of(guild_id: u64) -> Result<u128, String> {
    let result = read_view(
        selector("treasuryBalanceOf(uint256)"),
        &[u256_be(guild_id as u128)],
    )
    .await?;
    decode_u256_as_u128(&result)
}

/// Read `guildAddress(guildId)` → the guild's on-chain wallet (its TBA), as a
/// lowercase `0x…` address. Returns the zero address as a literal string when
/// unset rather than `None` — guild treasury reads want the raw address either
/// way (the CLI prints it verbatim).
pub async fn guild_address(guild_id: u64) -> Result<String, String> {
    let result = read_view(
        selector("guildAddress(uint256)"),
        &[u256_be(guild_id as u128)],
    )
    .await?;
    Ok(decode_address(&result).unwrap_or_else(|| zero_address().to_string()))
}

/// Read `guildName(guildId)` → the guild's display name (decoded `string`).
/// Empty for an unknown guild.
pub async fn guild_name(guild_id: u64) -> Result<String, String> {
    let result = read_view(selector("guildName(uint256)"), &[u256_be(guild_id as u128)]).await?;
    Ok(decode_string(&result).unwrap_or_default())
}

/// Read `guildsOf(address)` → every guildId the address is a member of. Bare
/// dynamic `uint256[]` ABI return (`[offset][len][id0]…`), the SAME decode as
/// `bounties_of`/`jobs_of`. Hostile-length-safe.
pub async fn guilds_of(addr_hex: &str) -> Result<Vec<u64>, String> {
    let account = parse_eth_address(addr_hex)?;
    let result = read_view(selector("guildsOf(address)"), &[addr_word(&account)]).await?;
    let bytes = hex_to_bytes(&result)?;
    Ok(decode_u64_array(&bytes))
}

/// Read `guildCount()` → the total number of guilds created (the next-id - 1
/// counter; informational).
pub async fn guild_count() -> Result<u64, String> {
    let result = read_view(selector("guildCount()"), &[]).await?;
    decode_u256_as_u64(&result)
}


#[cfg(test)]
mod guild_tests {
    use super::*;

    /// `createGuild(string)` — dynamic-string layout (offset 0x20 + length +
    /// padded bytes), the exact `register(string)` shape. A wrong offset would
    /// mis-decode the name on-chain.
    #[test]
    fn create_guild_calldata_layout() {
        let cd = encode_create_guild("builders");
        assert_eq!(&cd[0..4], &selector("createGuild(string)"));
        // head: offset word = 0x20.
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 0x20);
        // length word = 8 ("builders").
        assert_eq!(u64::from_be_bytes(cd[36 + 24..36 + 32].try_into().unwrap()), 8);
        // body = the UTF-8 name, right-padded to 32.
        assert_eq!(&cd[68..68 + 8], b"builders");
        assert_eq!(cd.len(), 4 + 32 + 32 + 32); // selector + offset + len + 1 padded word
    }

    /// `createGuild` with a >32-byte name pads the body to a 64-byte multiple
    /// (two tail words) — guards the `div_ceil` padding.
    #[test]
    fn create_guild_pads_long_name() {
        let name = "a-very-long-guild-name-over-32-bytes!!"; // 38 bytes
        let cd = encode_create_guild(name);
        assert_eq!(u64::from_be_bytes(cd[36 + 24..36 + 32].try_into().unwrap()), name.len() as u64);
        // 38 bytes -> padded to 64; total = sel + 2 head words + 2 tail words.
        assert_eq!(cd.len(), 4 + 32 + 32 + 64);
        assert_eq!(&cd[68..68 + name.len()], name.as_bytes());
    }

    /// `inviteToGuild(uint256,address)` — two static words. The address must be
    /// right-aligned (low 20 bytes of word 1); a left/right padding slip would
    /// invite the wrong account, so test an all-high-bit address.
    #[test]
    fn invite_to_guild_calldata_layout() {
        let member = [0xFFu8; 20];
        let cd = encode_invite_to_guild(0x2A, &member);
        assert_eq!(&cd[0..4], &selector("inviteToGuild(uint256,address)"));
        assert_eq!(cd.len(), 4 + 64);
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 0x2A); // guildId
        // word 1: address in the LOW 20 bytes; top 12 zero.
        assert!(cd[36..36 + 12].iter().all(|&b| b == 0));
        assert_eq!(&cd[36 + 12..36 + 32], &member);
    }

    /// `setRole(uint256,address,uint8)` — three static words; role byte
    /// right-aligned in word 2. Pins both the selector and the role placement.
    #[test]
    fn set_role_calldata_layout() {
        let member = [0xABu8; 20];
        let cd = encode_set_role(7, &member, GuildRole::Officer.as_u8());
        assert_eq!(&cd[0..4], &selector("setRole(uint256,address,uint8)"));
        assert_eq!(cd.len(), 4 + 96);
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 7); // guildId
        assert_eq!(&cd[36 + 12..36 + 32], &member); // member in word 1
        // role = 2 (Officer) in the low byte of word 2.
        assert!(cd[68..68 + 31].iter().all(|&b| b == 0));
        assert_eq!(cd[68 + 31], 2);
    }

    /// `fundGuild(uint256,uint256)` — two static words (guildId, amount).
    #[test]
    fn fund_guild_calldata_layout() {
        let amount = 1_500_000_000_000_000_000u128; // 1.5 $LH
        let cd = encode_fund_guild(9, amount);
        assert_eq!(&cd[0..4], &selector("fundGuild(uint256,uint256)"));
        assert_eq!(cd.len(), 4 + 64);
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 9); // guildId
        // amount in the low 16 bytes of word 1.
        assert_eq!(u128::from_be_bytes(cd[36 + 16..36 + 32].try_into().unwrap()), amount);
    }

    /// `spendTreasury(uint256,address,uint256,bytes)` — the only multi-arg
    /// DYNAMIC layout. memo is LAST, so the offset = 4 head words (0x80). Pins
    /// every head slot + the tail length/body so a shifted offset (the classic
    /// dynamic-encoding bug) is caught.
    #[test]
    fn spend_treasury_calldata_layout() {
        let to = [0xCDu8; 20];
        let amount = 2_000_000_000_000_000_000u128; // 2 $LH
        let memo = b"q3 grant"; // 8 bytes -> one padded tail word
        let cd = encode_spend_treasury(0x10, &to, amount, memo);
        assert_eq!(&cd[0..4], &selector("spendTreasury(uint256,address,uint256,bytes)"));
        // head 0: guildId.
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 0x10);
        // head 1: to (right-aligned).
        assert_eq!(&cd[36 + 12..36 + 32], &to);
        // head 2: amount.
        assert_eq!(u128::from_be_bytes(cd[68 + 16..68 + 32].try_into().unwrap()), amount);
        // head 3: offset to memo = 4 head words = 0x80.
        assert_eq!(u64::from_be_bytes(cd[100 + 24..100 + 32].try_into().unwrap()), 0x80);
        // tail length word at byte 4 + 0x80 = 132.
        assert_eq!(u64::from_be_bytes(cd[132 + 24..132 + 32].try_into().unwrap()), memo.len() as u64);
        // tail body = the memo, right-padded to 32.
        assert_eq!(&cd[164..164 + memo.len()], memo);
        assert_eq!(cd.len(), 4 + 4 * 32 + 32 + 32); // sel + 4 head + len + 1 padded word
    }

    /// `spendTreasury` with an EMPTY memo: offset still 0x80, length 0, no body.
    #[test]
    fn spend_treasury_empty_memo() {
        let to = [0x01u8; 20];
        let cd = encode_spend_treasury(1, &to, 1, b"");
        assert_eq!(u64::from_be_bytes(cd[100 + 24..100 + 32].try_into().unwrap()), 0x80); // offset
        assert_eq!(u64::from_be_bytes(cd[132 + 24..132 + 32].try_into().unwrap()), 0); // length 0
        assert_eq!(cd.len(), 4 + 4 * 32 + 32); // sel + head + length word, no body
    }

    /// Single-arg `uint256` write selectors (accept/leave) route through
    /// `call_uint_bytes` — pin selector + the id word.
    #[test]
    fn accept_and_leave_calldata_layout() {
        let accept = call_uint_bytes("acceptGuildInvite(uint256)", 5);
        assert_eq!(&accept[0..4], &selector("acceptGuildInvite(uint256)"));
        assert_eq!(accept.len(), 36);
        assert_eq!(u64::from_be_bytes(accept[28..36].try_into().unwrap()), 5);
        let leave = call_uint_bytes("leaveGuild(uint256)", 5);
        assert_eq!(&leave[0..4], &selector("leaveGuild(uint256)"));
        assert_eq!(leave.len(), 36);
        assert_eq!(u64::from_be_bytes(leave[28..36].try_into().unwrap()), 5);
    }

    /// Role enum round-trips and clamps unknown bytes to `None`.
    #[test]
    fn guild_role_from_to_u8_and_parse() {
        assert_eq!(GuildRole::from_u8(0), GuildRole::None);
        assert_eq!(GuildRole::from_u8(1), GuildRole::Member);
        assert_eq!(GuildRole::from_u8(2), GuildRole::Officer);
        assert_eq!(GuildRole::from_u8(3), GuildRole::Admin);
        assert_eq!(GuildRole::from_u8(99), GuildRole::None); // unknown clamps
        for r in [GuildRole::None, GuildRole::Member, GuildRole::Officer, GuildRole::Admin] {
            assert_eq!(GuildRole::from_u8(r.as_u8()), r);
        }
        assert_eq!(GuildRole::parse("member").unwrap(), GuildRole::Member);
        assert_eq!(GuildRole::parse("  OFFICER ").unwrap(), GuildRole::Officer);
        assert_eq!(GuildRole::parse("Admin").unwrap(), GuildRole::Admin);
        assert!(GuildRole::parse("none").is_err()); // not a settable rank
        assert!(GuildRole::parse("boss").is_err());
    }
}
