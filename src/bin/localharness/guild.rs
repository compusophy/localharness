#[allow(unused_imports)]
use crate::*;

// ---- guild (GuildFacet: on-chain orgs — members, roles, pooled treasury) -----
//
// Rung 3 of the coordination ladder (bounty → party → GUILD → DAO). A guild is
// an on-chain org with a member roster, per-member roles (member/officer/admin),
// and a pooled `$LH` treasury an admin/officer can spend. Mirrors the
// `registry::*_guild_*` helpers; the same sponsored-write + caller-resolution
// shape as `bounty`. A member arg given as a NAME resolves to its on-chain OWNER
// address (the `send_lh` resolution), or accepts a raw `0x…` address.

pub(crate) const GUILD_USAGE: &str = "\
usage: localharness guild <create|invite|accept|leave|role|fund|spend|members|treasury|mine> ...
  guild create [--as <me>] <name>                       create a guild (you're its admin)
  guild invite [--as <me>] <guildId> <member>           invite a name/0x address to join
  guild accept [--as <me>] <guildId>                    accept an invite (join the guild)
  guild leave  [--as <me>] <guildId>                    leave a guild
  guild role   [--as <me>] <guildId> <member> <member|officer|admin>   set a role (admin)
  guild fund   [--as <me>] <guildId> <amount>           deposit $LH into the treasury
  guild spend  [--as <me>] <guildId> <to> <amount> [memo...]   pay from the treasury (admin/officer)
  guild members  <guildId>                              list members + their roles
  guild treasury <guildId>                               show the treasury balance + wallet
  guild mine   [--as <me>]                               list the guilds you belong to
  member: a subdomain name (resolved to its owner) or a raw 0x address   amount: $LH (e.g. 5 or 0.5)";

/// Resolve a `member` argument to a `0x…` address WITHOUT a key — a raw address
/// is used as-is; a name resolves to its on-chain OWNER (the same split as
/// `send_lh`, so "invite alice" targets whoever owns `alice.localharness.xyz`).
/// Async (the name lookup hits the RPC); pure classification is `classify_recipient`.
pub(crate) async fn resolve_member_address(member: &str) -> Result<String, String> {
    use localharness::encoding::{classify_recipient, Recipient};
    match classify_recipient(member)? {
        Recipient::Address(a) => Ok(a),
        Recipient::Name(n) => match registry::owner_of_name(&n).await {
            Ok(Some(o)) => Ok(o),
            Ok(None) => Err(format!("'{n}' is not registered")),
            Err(e) => Err(format!("RPC error resolving '{n}': {e}")),
        },
    }
}

/// `localharness guild <subcommand>` — the guild router.
pub(crate) async fn guild(caller: Option<&str>, rest: &[String]) -> i32 {
    match rest.first().map(String::as_str) {
        Some("create") => match rest.get(1) {
            Some(name) => guild_create(caller, name).await,
            None => {
                eprintln!("usage: localharness guild create [--as <me>] <name>");
                2
            }
        },
        Some("invite") => match (rest.get(1), rest.get(2)) {
            (Some(id), Some(member)) => guild_invite(caller, id, member).await,
            _ => {
                eprintln!("usage: localharness guild invite [--as <me>] <guildId> <member>");
                2
            }
        },
        Some("accept") => match rest.get(1) {
            Some(id) => guild_accept(caller, id).await,
            None => {
                eprintln!("usage: localharness guild accept [--as <me>] <guildId>");
                2
            }
        },
        Some("leave") => match rest.get(1) {
            Some(id) => guild_leave(caller, id).await,
            None => {
                eprintln!("usage: localharness guild leave [--as <me>] <guildId>");
                2
            }
        },
        Some("role") => match (rest.get(1), rest.get(2), rest.get(3)) {
            (Some(id), Some(member), Some(role)) => guild_role(caller, id, member, role).await,
            _ => {
                eprintln!(
                    "usage: localharness guild role [--as <me>] <guildId> <member> <member|officer|admin>"
                );
                2
            }
        },
        Some("fund") => match (rest.get(1), rest.get(2)) {
            (Some(id), Some(amount)) => guild_fund(caller, id, amount).await,
            _ => {
                eprintln!("usage: localharness guild fund [--as <me>] <guildId> <amount>");
                2
            }
        },
        Some("spend") => {
            if rest.len() < 4 {
                eprintln!(
                    "usage: localharness guild spend [--as <me>] <guildId> <to> <amount> [memo...]"
                );
                return 2;
            }
            let memo = rest[4..].join(" ");
            guild_spend(caller, &rest[1], &rest[2], &rest[3], &memo).await
        }
        Some("members") => match rest.get(1) {
            Some(id) => guild_members(id).await,
            None => {
                eprintln!("usage: localharness guild members <guildId>");
                2
            }
        },
        Some("treasury") => match rest.get(1) {
            Some(id) => guild_treasury(id).await,
            None => {
                eprintln!("usage: localharness guild treasury <guildId>");
                2
            }
        },
        Some("mine") => guild_mine(caller).await,
        _ => {
            eprintln!("{GUILD_USAGE}");
            2
        }
    }
}

/// `guild create <name>` — create an on-chain guild (`createGuild`); the caller
/// becomes its admin. Reads the new guildId back from `guildsOf(creator)`.
pub(crate) async fn guild_create(caller: Option<&str>, name: &str) -> i32 {
    let name = name.trim();
    if name.is_empty() {
        eprintln!("guild create: name is empty");
        return 2;
    }
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("creating guild '{name}' …");
    match registry::create_guild_sponsored(&signer, &sponsor, name, registry::ALPHA_USD_ADDRESS).await
    {
        Ok(tx) => {
            // The new guildId is the last entry in the creator's guildsOf index.
            let addr = bytes_to_hex_str(&wallet::address(&signer));
            let id_note = match registry::guilds_of(&addr).await {
                Ok(ids) if !ids.is_empty() => Some(ids[ids.len() - 1]),
                _ => None,
            };
            match id_note {
                Some(id) => {
                    println!("✓ guild #{id} '{name}' created — you're its admin");
                    println!("  invite members:  guild invite {id} <name-or-0x>");
                    println!("  fund it:         guild fund {id} <amount>");
                }
                None => {
                    println!("✓ guild '{name}' created — see it with `guild mine`");
                }
            }
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("guild create failed: {e}");
            1
        }
    }
}

/// `guild invite <guildId> <member>` — invite a name/address to the guild
/// (`inviteToGuild`). The invitee then `guild accept`s.
pub(crate) async fn guild_invite(caller: Option<&str>, id_arg: &str, member: &str) -> i32 {
    let guild_id = match parse_guild_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let member_hex = match resolve_member_address(member).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("guild invite: {e}");
            return 1;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("inviting {member_hex} to guild #{guild_id} …");
    match registry::invite_to_guild_sponsored(
        &signer,
        &sponsor,
        guild_id,
        &member_hex,
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ invited {member_hex} to guild #{guild_id} — they run `guild accept {guild_id}`  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("guild invite failed: {e}");
            1
        }
    }
}

/// `guild accept <guildId>` — accept a pending invite and join
/// (`acceptGuildInvite`).
pub(crate) async fn guild_accept(caller: Option<&str>, id_arg: &str) -> i32 {
    let guild_id = match parse_guild_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("accepting the invite to guild #{guild_id} …");
    match registry::accept_guild_invite_sponsored(
        &signer,
        &sponsor,
        guild_id,
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ joined guild #{guild_id}  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("guild accept failed: {e}");
            1
        }
    }
}

/// `guild leave <guildId>` — leave a guild (`leaveGuild`).
pub(crate) async fn guild_leave(caller: Option<&str>, id_arg: &str) -> i32 {
    let guild_id = match parse_guild_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("leaving guild #{guild_id} …");
    match registry::leave_guild_sponsored(&signer, &sponsor, guild_id, registry::ALPHA_USD_ADDRESS).await
    {
        Ok(tx) => {
            println!("✓ left guild #{guild_id}  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("guild leave failed: {e}");
            1
        }
    }
}

/// `guild role <guildId> <member> <member|officer|admin>` — set a member's role
/// (`setRole`). Admin-gated on-chain.
pub(crate) async fn guild_role(caller: Option<&str>, id_arg: &str, member: &str, role_arg: &str) -> i32 {
    let guild_id = match parse_guild_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let role = match registry::GuildRole::parse(role_arg) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("guild role: {e}");
            return 2;
        }
    };
    let member_hex = match resolve_member_address(member).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("guild role: {e}");
            return 1;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("setting {member_hex}'s role in guild #{guild_id} to {} …", role.label());
    match registry::set_role_sponsored(
        &signer,
        &sponsor,
        guild_id,
        &member_hex,
        role.as_u8(),
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ {member_hex} is now {} in guild #{guild_id}  tx: {tx}", role.label());
            0
        }
        Err(e) => {
            eprintln!("guild role failed: {e}");
            1
        }
    }
}

/// `guild fund <guildId> <amount>` — deposit `$LH` from the caller's wallet into
/// the guild treasury (approve + fundGuild in one sponsored tx). The `$LH` leaves
/// the caller's balance the moment it mines.
pub(crate) async fn guild_fund(caller: Option<&str>, id_arg: &str, amount: &str) -> i32 {
    let guild_id = match parse_guild_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let amount_wei = match localharness::encoding::parse_token_amount(amount) {
        Some(w) if w > 0 => w,
        _ => {
            eprintln!("guild fund: invalid amount '{amount}' (expected a positive number of $LH)");
            return 2;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("funding guild #{guild_id} with {} …", fmt_lh(amount_wei));
    match registry::fund_guild_sponsored(
        &signer,
        &sponsor,
        guild_id,
        amount_wei,
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ deposited {} into guild #{guild_id}'s treasury  tx: {tx}", fmt_lh(amount_wei));
            0
        }
        Err(e) => {
            eprintln!("guild fund failed: {e}");
            1
        }
    }
}

/// `guild spend <guildId> <to> <amount> [memo]` — pay `$LH` from the guild
/// treasury to a name/address (`spendTreasury`). Admin/officer-gated on-chain.
pub(crate) async fn guild_spend(caller: Option<&str>, id_arg: &str, to: &str, amount: &str, memo: &str) -> i32 {
    let guild_id = match parse_guild_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let amount_wei = match localharness::encoding::parse_token_amount(amount) {
        Some(w) if w > 0 => w,
        _ => {
            eprintln!("guild spend: invalid amount '{amount}' (expected a positive number of $LH)");
            return 2;
        }
    };
    let to_hex = match resolve_member_address(to).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("guild spend: {e}");
            return 1;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("spending {} from guild #{guild_id} to {to_hex} …", fmt_lh(amount_wei));
    match registry::spend_treasury_sponsored(
        &signer,
        &sponsor,
        guild_id,
        &to_hex,
        amount_wei,
        memo.as_bytes(),
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ paid {} from guild #{guild_id} to {to_hex}  tx: {tx}", fmt_lh(amount_wei));
            0
        }
        Err(e) => {
            eprintln!("guild spend failed: {e}");
            1
        }
    }
}

/// `guild members <guildId>` — list a guild's members + their roles
/// (`membersOf` + a `roleOf` per member). Read-only, no `$LH`.
pub(crate) async fn guild_members(id_arg: &str) -> i32 {
    let guild_id = match parse_guild_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let name = registry::guild_name(guild_id).await.unwrap_or_default();
    let members = match registry::members_of_guild(guild_id).await {
        Ok(m) => m,
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    };
    let label = if name.is_empty() {
        format!("guild #{guild_id}")
    } else {
        format!("guild #{guild_id} '{name}'")
    };
    if members.is_empty() {
        println!("{label} has no members (or does not exist)");
        return 0;
    }
    println!("{label} — {} member(s):", members.len());
    for m in members {
        let role = registry::role_of_guild(guild_id, &m)
            .await
            .map(|r| r.label().to_string())
            .unwrap_or_else(|_| "?".to_string());
        println!("  {m}  [{role}]");
    }
    0
}

/// `guild treasury <guildId>` — show a guild's pooled `$LH` + its wallet address
/// (`treasuryBalanceOf` + `guildAddress`). Read-only, no `$LH`.
pub(crate) async fn guild_treasury(id_arg: &str) -> i32 {
    let guild_id = match parse_guild_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let name = registry::guild_name(guild_id).await.unwrap_or_default();
    let balance = match registry::treasury_balance_of(guild_id).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    };
    let wallet_addr = registry::guild_address(guild_id).await.unwrap_or_default();
    let label = if name.is_empty() {
        format!("guild #{guild_id}")
    } else {
        format!("guild #{guild_id} '{name}'")
    };
    println!("{label}");
    println!("  treasury  {}", fmt_lh(balance));
    println!("  wallet    {wallet_addr}");
    0
}

/// `guild mine [--as <me>]` — list the guilds the caller belongs to
/// (`guildsOf` + a `guildName`/`roleOf` per id). Read-only, no `$LH`.
pub(crate) async fn guild_mine(caller: Option<&str>) -> i32 {
    let signer = match load_signer(caller) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let addr = bytes_to_hex_str(&wallet::address(&signer));
    let ids = match registry::guilds_of(&addr).await {
        Ok(ids) => ids,
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    };
    if ids.is_empty() {
        println!("{addr} belongs to no guilds — create one with `guild create <name>`");
        return 0;
    }
    println!("{addr} belongs to {} guild(s):", ids.len());
    for id in ids {
        let name = registry::guild_name(id).await.unwrap_or_default();
        let role = registry::role_of_guild(id, &addr)
            .await
            .map(|r| r.label().to_string())
            .unwrap_or_else(|_| "?".to_string());
        let balance = registry::treasury_balance_of(id).await.unwrap_or(0);
        let name_part = if name.is_empty() { String::new() } else { format!(" '{name}'") };
        println!("  #{id}{name_part}  [you: {role}]  treasury {}", fmt_lh(balance));
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `guild role` arg parses to the on-chain `uint8`; `none` and garbage
    /// are rejected (a role must be member/officer/admin).
    #[test]
    fn guild_role_arg_parses_to_u8() {
        assert_eq!(registry::GuildRole::parse("member").unwrap().as_u8(), 1);
        assert_eq!(registry::GuildRole::parse("Officer").unwrap().as_u8(), 2);
        assert_eq!(registry::GuildRole::parse("  ADMIN ").unwrap().as_u8(), 3);
        assert!(registry::GuildRole::parse("none").is_err());
        assert!(registry::GuildRole::parse("owner").is_err());
    }

    /// `guild invite alice` (a name) classifies as a Name (→ owner lookup);
    /// a raw 0x address classifies as an Address (used as-is). The pure half of
    /// `resolve_member_address` — the async owner lookup needs the chain.
    #[test]
    fn guild_member_arg_classification() {
        use localharness::encoding::{classify_recipient, Recipient};
        // A 40-hex address is used verbatim.
        let addr = "0x1111111111111111111111111111111111111111";
        assert_eq!(
            classify_recipient(addr).unwrap(),
            Recipient::Address(addr.to_string())
        );
        // A bare name is lowercased and resolved to its owner downstream.
        assert_eq!(
            classify_recipient("Alice").unwrap(),
            Recipient::Name("alice".to_string())
        );
        // Empty / zero-address are rejected up front (no member to invite).
        assert!(classify_recipient("").is_err());
        assert!(classify_recipient("0x0000000000000000000000000000000000000000").is_err());
    }
}
