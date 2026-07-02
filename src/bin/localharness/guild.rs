use crate::{bytes_to_hex_str, ensure_wallet_covers, fmt_lh, load_signer, parse_guild_id, registry, take_tba_flag, tba_execute_diamond_call, wallet};

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
  guild accept [--as <me>] [--tba <subguild>] <guildId> accept an invite (join the guild);
                                                        --tba: a sub-guild's TBA joins a parent guild
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
        Some("accept") => {
            // Optional `--tba <subguild-name>`: a SUB-guild's TBA accepts the
            // invite to a PARENT guild (nested divisions) — the TBA executes
            // `acceptGuildInvite`, not the caller's own EOA.
            let (tba, positional) = match take_tba_flag(&rest[1..]) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("{e}");
                    return 2;
                }
            };
            match positional.first() {
                Some(id) => guild_accept(caller, id, tba.as_deref()).await,
                None => {
                    eprintln!("usage: localharness guild accept [--as <me>] [--tba <subguild>] <guildId>");
                    2
                }
            }
        }
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
    let signer = match load_signer(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("creating guild '{name}' …");
    match registry::create_guild_sponsored(&signer, name).await
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
    let signer = match load_signer(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("inviting {member_hex} to guild #{guild_id} …");
    match registry::invite_to_guild_sponsored(&signer, guild_id, &member_hex)
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

/// `guild accept [--tba <subguild>] <guildId>` — accept a pending invite and
/// join (`acceptGuildInvite`). With `--tba <subguild-name>` the SUB-guild's TBA
/// executes the accept (NESTED divisions: a guild's wallet joins a PARENT
/// guild), routed through the sponsored tba-execute path; without it the
/// caller's own EOA joins directly.
pub(crate) async fn guild_accept(caller: Option<&str>, id_arg: &str, tba: Option<&str>) -> i32 {
    let guild_id = match parse_guild_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    // NESTED path: a sub-guild's TBA accepts the invite to the parent guild.
    if let Some(subguild) = tba {
        return tba_execute_diamond_call(
            caller,
            subguild,
            registry::encode_accept_guild_invite_calldata(guild_id),
            &format!("'{subguild}' joining guild #{guild_id}"),
        )
        .await;
    }
    let signer = match load_signer(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("accepting the invite to guild #{guild_id} …");
    match registry::accept_guild_invite_sponsored(&signer, guild_id)
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
    let signer = match load_signer(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("leaving guild #{guild_id} …");
    match registry::leave_guild_sponsored(&signer, guild_id).await
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
    let signer = match load_signer(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("setting {member_hex}'s role in guild #{guild_id} to {} …", role.label());
    match registry::set_role_sponsored(&signer, guild_id, &member_hex, role.as_u8())
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
    let signer = match load_signer(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    // The contribution pulls from the WALLET pot — auto-bridge any shortfall
    // out of the chat meter first (on-chain feedback #63).
    let from_hex = bytes_to_hex_str(&wallet::address(&signer));
    if let Err(code) = ensure_wallet_covers(&signer, &from_hex, amount_wei).await {
        return code;
    }
    println!("funding guild #{guild_id} with {} …", fmt_lh(amount_wei));
    match registry::fund_guild_sponsored(&signer, guild_id, amount_wei)
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

/// `tithe …` router — three flavors of agent→guild treasury funding:
///   • `tithe <guildId> <amount>`        MANUAL one-shot: the TBA contributes
///                                       `<amount>` $LH of its earnings now.
///   • `tithe auto <guildId> <bps>`      OPT-IN auto-tithe: the TBA approves the
///                                       diamond + `setTithe(guildId, bps)` so a
///                                       PERMISSIONLESS `collectTithe` can later
///                                       pull `bps/10000` of its balance.
///   • `tithe collect <agent>`           PERMISSIONLESS trigger: pull `<agent>`'s
///                                       consented tithe into its chosen guild.
/// `auto`/`collect` are dispatched by keyword; anything else is the manual path
/// (first positional = guildId), preserving the shipped `tithe <id> <amt>` ABI.
pub(crate) async fn tithe(caller: Option<&str>, rest: &[String]) -> i32 {
    match rest.first().map(String::as_str) {
        Some("auto") => match (rest.get(1), rest.get(2)) {
            (Some(id), Some(bps)) => tithe_auto(caller, id, bps).await,
            _ => {
                eprintln!("usage: localharness tithe auto --as <agent> <guildId> <bps>");
                2
            }
        },
        Some("collect") => match rest.get(1) {
            // `collect <agent>` defaults to the caller's own agent when --as set
            // and no explicit agent positional is given.
            Some(agent) => tithe_collect(caller, agent).await,
            None => match caller {
                Some(c) => tithe_collect(caller, c).await,
                None => {
                    eprintln!("usage: localharness tithe collect [--as <me>] <agent>");
                    2
                }
            },
        },
        // Manual one-shot: `tithe <guildId> <amount>`.
        _ => match (rest.first(), rest.get(1)) {
            (Some(id), Some(amount)) => tithe_manual(caller, id, amount).await,
            _ => {
                eprintln!("usage: localharness tithe --as <agent> <guildId> <amount>");
                eprintln!("       localharness tithe auto    --as <agent> <guildId> <bps>");
                eprintln!("       localharness tithe collect  [--as <me>] <agent>");
                2
            }
        },
    }
}

/// `tithe <guildId> <amount>` — an agent's token-bound account contributes
/// <amount> $LH of its OWN earnings to a guild's treasury (the revenue→treasury
/// leg that makes a guild self-funding). The agent's TBA executes a batched
/// approve(diamond)+fundGuild in ONE sponsored tx (auto-deploys the TBA if
/// needed). Driven by the agent's own key (--as).
pub(crate) async fn tithe_manual(caller: Option<&str>, id_arg: &str, amount: &str) -> i32 {
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
            eprintln!("tithe: invalid amount '{amount}' (expected a positive number of $LH)");
            return 2;
        }
    };
    let Some(agent) = caller else {
        eprintln!("tithe: --as <agent> is required (the agent whose TBA tithes its earnings)");
        return 2;
    };
    let signer = match load_signer(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let tba_addr = match registry::tba_of_name(agent).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            eprintln!("tithe: '{agent}' is not registered (no token-bound account)");
            return 1;
        }
        Err(e) => {
            eprintln!("tithe: RPC error resolving '{agent}': {e}");
            return 1;
        }
    };
    let token_id = registry::id_of_name(agent).await.unwrap_or(0);
    if token_id == 0 {
        eprintln!("tithe: '{agent}' has no token id");
        return 1;
    }
    // The TBA tithes its OWN balance — refuse if it can't cover it.
    let bal = registry::token_balance_of(&tba_addr).await.unwrap_or(0);
    if bal < amount_wei {
        eprintln!(
            "tithe: {agent}'s TBA holds {} but you asked to tithe {}",
            fmt_lh(bal),
            fmt_lh(amount_wei)
        );
        return 1;
    }
    let approve = match registry::approve_credits_call(amount_wei) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("tithe: {e}");
            return 1;
        }
    };
    let fund = match registry::fund_guild_call(guild_id, amount_wei) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("tithe: {e}");
            return 1;
        }
    };
    let targets = vec![(approve.to, approve.input), (fund.to, fund.input)];
    println!(
        "{agent}'s TBA {tba_addr} tithing {} to guild #{guild_id}'s treasury …",
        fmt_lh(amount_wei)
    );
    match registry::tba_execute_batch_sponsored(&signer, token_id, &tba_addr, &targets, 2_000_000)
    .await
    {
        Ok(tx) => {
            println!(
                "✓ {agent} tithed {} to guild #{guild_id}'s treasury  tx: {tx}",
                fmt_lh(amount_wei)
            );
            0
        }
        Err(e) => {
            eprintln!("tithe failed: {e}");
            1
        }
    }
}

/// `tithe auto <guildId> <bps>` — OPT IN to the auto-tithe: the agent's TBA
/// `approve`s the diamond to spend its `$LH` AND `setTithe(guildId, bps)` in ONE
/// sponsored batch. Afterwards a PERMISSIONLESS `tithe collect <agent>` (a
/// scheduler / guild officer / anyone) can pull `bps/10000` of the agent's
/// CURRENT balance into the guild it chose — bounded by the approved allowance,
/// and unable to redirect funds because collect reads only the agent's own
/// stored config. `bps` is basis points (1..=10000; 10000 = 100%). The approve
/// allowance is the agent's hard ceiling on cumulative tithing — we approve a
/// generous standing amount (the agent's full current balance) so repeated
/// collects don't each need a fresh approve; the agent revokes via `revokeTithe`
/// (or by re-approving 0) to stop.
pub(crate) async fn tithe_auto(caller: Option<&str>, id_arg: &str, bps_arg: &str) -> i32 {
    let guild_id = match parse_guild_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let bps: u64 = match bps_arg.trim().parse() {
        Ok(b) if (1..=registry::TITHE_MAX_BPS).contains(&b) => b,
        _ => {
            eprintln!(
                "tithe auto: invalid bps '{bps_arg}' (expected 1..={} basis points; 10000 = 100%)",
                registry::TITHE_MAX_BPS
            );
            return 2;
        }
    };
    let Some(agent) = caller else {
        eprintln!("tithe auto: --as <agent> is required (the agent whose TBA opts in)");
        return 2;
    };
    let signer = match load_signer(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let tba_addr = match registry::tba_of_name(agent).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            eprintln!("tithe auto: '{agent}' is not registered (no token-bound account)");
            return 1;
        }
        Err(e) => {
            eprintln!("tithe auto: RPC error resolving '{agent}': {e}");
            return 1;
        }
    };
    let token_id = registry::id_of_name(agent).await.unwrap_or(0);
    if token_id == 0 {
        eprintln!("tithe auto: '{agent}' has no token id");
        return 1;
    }
    // Standing allowance = the TBA's full current balance, so repeated collects
    // draw against one approve. The on-chain pull is the authoritative gate;
    // this just sets the ceiling. (Zero balance still opts in — the rate is set
    // and collection sizes against whatever the TBA later holds, up to this.)
    let allowance = registry::token_balance_of(&tba_addr).await.unwrap_or(0);
    let approve = match registry::approve_credits_call(allowance) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("tithe auto: {e}");
            return 1;
        }
    };
    let set = match registry::set_tithe_call(guild_id, bps) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("tithe auto: {e}");
            return 1;
        }
    };
    let targets = vec![(approve.to, approve.input), (set.to, set.input)];
    let pct = bps as f64 / 100.0;
    println!(
        "{agent}'s TBA {tba_addr} opting in to tithe {pct}% of its balance to guild #{guild_id} …"
    );
    match registry::tba_execute_batch_sponsored(&signer, token_id, &tba_addr, &targets, 2_000_000)
    .await
    {
        Ok(tx) => {
            println!("✓ {agent} now tithes {pct}% to guild #{guild_id} on each collect  tx: {tx}");
            println!("  collect it:  tithe collect {agent}");
            println!("  stop:        (re-run with a new rate, or revoke the approve)");
            0
        }
        Err(e) => {
            eprintln!("tithe auto failed: {e}");
            1
        }
    }
}

/// `tithe collect <agent>` — PERMISSIONLESS: trigger `<agent>`'s consented
/// auto-tithe (`collectTithe(<agent>'s TBA)`). Pulls `bps/10000` of the agent's
/// CURRENT `$LH` balance (capped by the allowance it approved) into the guild
/// the agent chose, crediting the treasury exactly like `fundGuild`. SAFE for
/// anyone to run — the on-chain facet reads only the agent's OWN stored config,
/// so the trigger can't redirect or inflate the tithe. Signed by the caller
/// (`--as`); the agent need not be online.
pub(crate) async fn tithe_collect(caller: Option<&str>, agent: &str) -> i32 {
    let signer = match load_signer(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let tba_addr = match registry::tba_of_name(agent).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            eprintln!("tithe collect: '{agent}' is not registered (no token-bound account)");
            return 1;
        }
        Err(e) => {
            eprintln!("tithe collect: RPC error resolving '{agent}': {e}");
            return 1;
        }
    };
    // Surface the consented config so the operator sees what will move.
    match registry::tithe_of(&tba_addr).await {
        Ok((_, 0)) => {
            eprintln!("tithe collect: '{agent}' has not opted in (run `tithe auto --as {agent} <guildId> <bps>`)");
            return 1;
        }
        Ok((guild_id, bps)) => {
            let pct = bps as f64 / 100.0;
            println!("collecting {agent}'s tithe ({pct}% to guild #{guild_id}) …");
        }
        Err(e) => {
            eprintln!("tithe collect: RPC error reading config: {e}");
            return 1;
        }
    }
    match registry::collect_tithe_sponsored(&signer, &tba_addr)
    .await
    {
        Ok(tx) => {
            println!("✓ collected {agent}'s tithe into its guild treasury  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("tithe collect failed: {e}");
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
    let signer = match load_signer(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("spending {} from guild #{guild_id} to {to_hex} …", fmt_lh(amount_wei));
    match registry::spend_treasury_sponsored(&signer, guild_id, &to_hex, amount_wei, memo.as_bytes())
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
