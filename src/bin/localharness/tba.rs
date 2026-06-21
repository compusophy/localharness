use crate::{bytes_to_hex_str, decode_hex_arg, fmt_lh, load_signer, load_signer_and_sponsor, registry, resolve_own_token_id, take_data_flag, take_tba_flag, wallet};

// ---- tba (token-bound account: make YOUR agent's wallet EXECUTE a call) ------
//
// Every identity NFT has a deterministic token-bound account (ERC-6551
// `MultiSignerAccount`) — a smart wallet the NFT holder controls. This command
// lets an agent ACT through it from a shell, with no browser tab: deploy it,
// see its `$LH`, and make it EXECUTE an arbitrary call (a `$LH` transfer, or
// any `to` + `--data <hex>`).
// Authorization is enforced on-chain by the account (only the NFT holder or an
// enrolled signer can `execute`); the embedded sponsor pays the fee. Unblocks a
// guild's TBA voting in a parent DAO, an agent's TBA paying/calling, etc.
// Built on `registry::tba_execute_call_sponsored` / `tba_send_lh_sponsored` /
// `create_token_bound_account_sponsored`.

pub(crate) const TBA_USAGE: &str = "\
usage: localharness tba <show|deploy|exec> ...
  tba show   [--as <me>] [<name>]            your (or <name>'s) TBA address, $LH, deployed?
  tba deploy [--as <me>] [<name>]            deploy the TBA on-chain (createTokenBoundAccount)
  tba exec   [--as <me>] [--tba <name-or-0xaddr>] <to> <amount> [--data <hex>]
                                             make a TBA execute a call:
                                               no --data → send <amount> $LH to <to>
                                               --data <hex> → call <to> with <hex>, <amount> as value
                                               --tba → act through an owned TBA other than
                                                       your main (a guild's wallet, etc.); default
                                                       is your main TBA. The chain gates execute to
                                                       the TBA owner, so it must be one you control.
  <to> is a name (→ its on-chain owner) or a 0x… address.
  <amount> is in $LH (the transfer amount, or the ETH value forwarded with --data).";

pub(crate) async fn tba(caller: Option<&str>, rest: &[String]) -> i32 {
    match rest.first().map(String::as_str) {
        Some("show") => tba_show(caller, rest.get(1).map(String::as_str)).await,
        Some("deploy") => tba_deploy(caller, rest.get(1).map(String::as_str)).await,
        Some("exec") => tba_exec(caller, &rest[1..]).await,
        _ => {
            eprintln!("{TBA_USAGE}");
            2
        }
    }
}

/// Resolve the tokenId to operate on: an explicit `<name>` if given (it must be
/// registered), else the caller's OWN identity (`resolve_own_token_id` — MAIN,
/// or sole holding). Returns `(token_id, label)` where `label` is for display.
pub(crate) async fn tba_target_token(
    caller: Option<&str>,
    name: Option<&str>,
    signer: &k256::ecdsa::SigningKey,
) -> Result<(u64, String), String> {
    if let Some(n) = name {
        match registry::id_of_name(n).await {
            Ok(0) => Err(format!("tba: '{n}' is not registered")),
            Ok(id) => Ok((id, n.to_string())),
            Err(e) => Err(format!("tba: RPC error resolving '{n}': {e}")),
        }
    } else {
        let id = resolve_own_token_id(caller, signer).await?;
        let label = registry::name_of_id(id).await.unwrap_or_else(|_| format!("token #{id}"));
        Ok((id, label))
    }
}

/// `tba show [--as <me>] [<name>]` — print the token-bound account address, its
/// `$LH` balance, and whether it's deployed on-chain. Read-only, no `$LH` spent.
/// With an explicit `<name>` it's a PURE read (no local identity key needed —
/// you can inspect any agent's wallet); without one it resolves YOUR identity,
/// which requires a local key.
pub(crate) async fn tba_show(caller: Option<&str>, name: Option<&str>) -> i32 {
    let (token_id, label) = if let Some(n) = name {
        // Explicit name → pure on-chain read, no key required.
        match registry::id_of_name(n).await {
            Ok(0) => {
                eprintln!("tba show: '{n}' is not registered");
                return 1;
            }
            Ok(id) => (id, n.to_string()),
            Err(e) => {
                eprintln!("tba show: RPC error resolving '{n}': {e}");
                return 1;
            }
        }
    } else {
        // No name → resolve the caller's OWN identity (needs a local key).
        let signer = match load_signer(caller) {
            Ok(s) => s,
            Err(code) => return code,
        };
        match tba_target_token(caller, None, &signer).await {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{e}");
                return 1;
            }
        }
    };
    let tba_addr = match registry::tba_of_token_id(token_id).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            eprintln!("tba show: no token-bound account for '{label}' (token #{token_id})");
            return 1;
        }
        Err(e) => {
            eprintln!("tba show: RPC error: {e}");
            return 1;
        }
    };
    let balance = registry::token_balance_of(&tba_addr).await.unwrap_or(0);
    let deployed = registry::is_contract_deployed(&tba_addr).await.unwrap_or(false);
    println!("{label}  (token #{token_id})");
    println!("  wallet (TBA):  {tba_addr}");
    println!("  balance:       {}", fmt_lh(balance));
    println!(
        "  deployed:      {}",
        if deployed { "yes" } else { "no — run `tba deploy` before it can execute" }
    );
    0
}

/// `tba deploy [--as <me>] [<name>]` — deploy the token-bound account on-chain
/// via `createTokenBoundAccount` (idempotent; a no-op if already deployed).
/// Needed before the TBA can `execute` / hold signers. Sponsored gas.
pub(crate) async fn tba_deploy(caller: Option<&str>, name: Option<&str>) -> i32 {
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let (token_id, label) = match tba_target_token(caller, name, &signer).await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let tba_addr = match registry::tba_of_token_id(token_id).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            eprintln!("tba deploy: no token-bound account for '{label}' (token #{token_id})");
            return 1;
        }
        Err(e) => {
            eprintln!("tba deploy: RPC error: {e}");
            return 1;
        }
    };
    if registry::is_contract_deployed(&tba_addr).await.unwrap_or(false) {
        println!("{label}'s TBA {tba_addr} is already deployed — nothing to do.");
        return 0;
    }
    println!("deploying {label}'s TBA {tba_addr} …");
    match registry::create_token_bound_account_sponsored(
        &signer,
        &sponsor,
        token_id,
        registry::ALPHA_USD_ADDRESS(),
    )
    .await
    {
        Ok(tx) => {
            println!("✓ deployed  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("tba deploy failed: {e}");
            1
        }
    }
}

/// `tba exec [--as <me>] [--tba <name-or-0xaddr>] <to> <amount> [--data <hex>]` —
/// make a token-bound account EXECUTE a call. With no `--data` this is a plain
/// `$LH` transfer of `<amount>` to `<to>` (`execute($LH, 0, transfer(to,
/// amount))`); with `--data <hex>` it calls `<to>` with that calldata and
/// forwards `<amount>` as the call value (`execute(to, amount, data)`). `<to>`
/// is a name (resolved to its on-chain owner address) or a raw `0x…` address.
/// The acting TBA defaults to the CALLER'S OWN main; `--tba` overrides it with
/// any TBA the caller controls — a name (→ `tokenBoundAccountByName`) or a raw
/// `0x…` address — so a GUILD's wallet can act (e.g. join + vote in a parent
/// guild's DAO). The MultiSignerAccount gates `execute` to the TBA owner
/// on-chain (`_isAuthorized`); a client-side owner check warns early for a name
/// target. The TBA is deployed first if needed (when its token id is known).
pub(crate) async fn tba_exec(caller: Option<&str>, rest: &[String]) -> i32 {
    // Pull an optional `--tba <name-or-0xaddr>` (override the acting TBA) and an
    // optional `--data <hex>` from anywhere in the args.
    let (tba_flag, after_tba) = match take_tba_flag(rest) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let (data_hex, positional) = match take_data_flag(&after_tba) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    if positional.len() != 2 {
        eprintln!("{TBA_USAGE}");
        return 2;
    }
    let to_arg = &positional[0];
    let amount_arg = &positional[1];

    // Resolve `<to>`: a 0x address, or a name → its on-chain OWNER.
    use localharness::encoding::{classify_recipient, Recipient};
    let to_hex = match classify_recipient(to_arg) {
        Ok(Recipient::Address(a)) => a,
        Ok(Recipient::Name(n)) => match registry::owner_of_name(&n).await {
            Ok(Some(o)) => o,
            Ok(None) => {
                eprintln!("tba exec: '{n}' is not registered");
                return 1;
            }
            Err(e) => {
                eprintln!("tba exec: RPC error resolving '{n}': {e}");
                return 1;
            }
        },
        Err(e) => {
            eprintln!("tba exec: {e}");
            return 2;
        }
    };

    // `<amount>` is the $LH transfer amount (no --data) or the ETH call value.
    let amount_wei = match localharness::encoding::parse_token_amount(amount_arg) {
        Some(w) => w,
        None => {
            eprintln!("tba exec: invalid amount '{amount_arg}' (expected a number of $LH)");
            return 2;
        }
    };
    // The transfer path needs a positive amount; the --data path may forward 0.
    if data_hex.is_none() && amount_wei == 0 {
        eprintln!("tba exec: amount must be greater than 0 for a $LH transfer");
        return 2;
    }

    // Decode `--data <hex>` (0x-optional) when present.
    let data = match &data_hex {
        Some(h) => match decode_hex_arg(h) {
            Ok(b) => Some(b),
            Err(e) => {
                eprintln!("tba exec: {e}");
                return 2;
            }
        },
        None => None,
    };

    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let caller_addr = bytes_to_hex_str(&wallet::address(&signer));

    // Resolve the ACTING TBA. Default (no --tba) = the caller's OWN main TBA, as
    // before. With --tba it's an arbitrary owned TBA: a name → its
    // `tokenBoundAccountByName`, or a raw 0x address. The MultiSignerAccount gates
    // `execute` to the TBA owner on-chain (`_isAuthorized`), so signing as the
    // caller only works for a TBA the caller controls — the client check below is
    // a clean early warning, the chain is the real gate. `exec_token_id` is the id
    // backing the TBA when known (a name target / the caller's own), used to
    // auto-deploy a counterfactual TBA; `None` for a raw-address target (no
    // reverse index → we can't deploy it, only warn).
    let (tba_addr, exec_token_id, tba_label) = match &tba_flag {
        // --tba <name-or-0xaddr>: an explicit, possibly-foreign-but-owned TBA.
        Some(target) => {
            use localharness::encoding::{classify_recipient, Recipient};
            match classify_recipient(target) {
                Ok(Recipient::Address(a)) => {
                    // Raw TBA address — no on-chain reverse index to its token, so
                    // we can't resolve the controlling owner or auto-deploy. The
                    // on-chain `_isAuthorized` is the real gate.
                    (a.clone(), None, a)
                }
                Ok(Recipient::Name(n)) => {
                    let addr = match registry::tba_of_name(&n).await {
                        Ok(Some(a)) => a,
                        Ok(None) => {
                            eprintln!("tba exec: '{n}' is not registered (no token-bound account)");
                            return 1;
                        }
                        Err(e) => {
                            eprintln!("tba exec: RPC error resolving '{n}': {e}");
                            return 1;
                        }
                    };
                    // Client-side owner check: warn (don't block) when the name's
                    // controlling NFT owner isn't the caller. The chain still gates.
                    match registry::owner_of_name(&n).await {
                        Ok(Some(o)) if o.eq_ignore_ascii_case(&caller_addr) => {}
                        Ok(Some(o)) => {
                            eprintln!(
                                "warning: '{n}' is controlled by {o}, not you ({caller_addr}) — \
                                 the TBA's on-chain _isAuthorized will reject this unless you're \
                                 an enrolled signer."
                            );
                        }
                        _ => {}
                    }
                    // The token id backs the auto-deploy of a counterfactual TBA.
                    let id = registry::id_of_name(&n).await.unwrap_or(0);
                    (addr, if id != 0 { Some(id) } else { None }, n)
                }
                Err(e) => {
                    eprintln!("tba exec: --tba {e}");
                    return 2;
                }
            }
        }
        // No --tba: the caller's OWN identity (the original, unchanged behaviour).
        None => {
            let token_id = match resolve_own_token_id(caller, &signer).await {
                Ok(id) => id,
                Err(e) => {
                    eprintln!("{e}");
                    return 1;
                }
            };
            match registry::tba_of_token_id(token_id).await {
                Ok(Some(a)) => (a, Some(token_id), "your".to_string()),
                Ok(None) => {
                    eprintln!("tba exec: no token-bound account for your token #{token_id}");
                    return 1;
                }
                Err(e) => {
                    eprintln!("tba exec: RPC error: {e}");
                    return 1;
                }
            }
        }
    };

    // The TBA must be deployed before it can execute. Deploy first if we know its
    // token id (caller's own, or a name target). A raw-address target can't be
    // deployed (no token id) — surface a clean error instead of an opaque revert.
    if !registry::is_contract_deployed(&tba_addr).await.unwrap_or(false) {
        match exec_token_id {
            Some(token_id) => {
                println!("{tba_label} TBA {tba_addr} isn't deployed yet — deploying first …");
                if let Err(e) = registry::create_token_bound_account_sponsored(
                    &signer,
                    &sponsor,
                    token_id,
                    registry::ALPHA_USD_ADDRESS(),
                )
                .await
                {
                    eprintln!("tba exec: TBA deploy failed: {e}");
                    return 1;
                }
            }
            None => {
                eprintln!(
                    "tba exec: TBA {tba_addr} isn't deployed and was given as a raw address \
                     (no token id to deploy it) — pass `--tba <name>` so it can be deployed, \
                     or deploy it first with `tba deploy`."
                );
                return 1;
            }
        }
    }

    let result = match &data {
        // Arbitrary call: execute(to, amount_as_value, data).
        Some(bytes) => {
            println!(
                "{tba_label} TBA {tba_addr} → execute({to_hex}, value {}, {} bytes of calldata) …",
                fmt_lh(amount_wei),
                bytes.len()
            );
            registry::tba_execute_call_sponsored(
                &signer,
                &sponsor,
                &tba_addr,
                &to_hex,
                amount_wei,
                bytes,
                registry::ALPHA_USD_ADDRESS(),
            )
            .await
        }
        // Plain $LH transfer: execute($LH, 0, transfer(to, amount)).
        None => {
            println!("{tba_label} TBA {tba_addr} → send {} $LH to {to_hex} …", fmt_lh(amount_wei));
            registry::tba_send_lh_sponsored(
                &signer,
                &sponsor,
                &tba_addr,
                &to_hex,
                amount_wei,
                registry::ALPHA_USD_ADDRESS(),
            )
            .await
        }
    };
    match result {
        Ok(tx) => {
            println!("✓ executed  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("tba exec failed: {e}");
            1
        }
    }
}

/// Drive a `--tba <subguild-name>`'s token-bound account to EXECUTE a diamond
/// call with `calldata` — the shared spine of the phase-2 nested-division
/// wrappers (`guild accept --tba`, `vote cast --tba`). Resolves
/// `<subguild-name>` → its TBA (`tba_of_name`) + backing tokenId (for an
/// auto-deploy), warns (doesn't block — the chain is the real gate) when the
/// caller doesn't own the name, deploys the counterfactual TBA if needed, then
/// routes ONE sponsored `execute(diamond, 0, calldata)` through the SAME
/// `tba_execute_call_sponsored` path `tba exec` uses (the NFT holder = the local
/// key signs; the sponsor pays gas). The diamond (`REGISTRY_ADDRESS()`) is always
/// the inner `to`; value is always 0 (these calls move no native token).
/// `action` labels progress lines. Returns a process exit code.
pub(crate) async fn tba_execute_diamond_call(
    caller: Option<&str>,
    subguild: &str,
    calldata: Vec<u8>,
    action: &str,
) -> i32 {
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let caller_addr = bytes_to_hex_str(&wallet::address(&signer));

    // Resolve the acting (sub)guild's TBA + its backing tokenId.
    let tba_addr = match registry::tba_of_name(subguild).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            eprintln!("{action}: '{subguild}' is not registered (no token-bound account)");
            return 1;
        }
        Err(e) => {
            eprintln!("{action}: RPC error resolving '{subguild}': {e}");
            return 1;
        }
    };
    // Client-side owner check: warn (don't block) when the name's controlling
    // NFT owner isn't the caller. The TBA's on-chain `_isAuthorized` still gates.
    if let Ok(Some(o)) = registry::owner_of_name(subguild).await {
        if !o.eq_ignore_ascii_case(&caller_addr) {
            eprintln!(
                "warning: '{subguild}' is controlled by {o}, not you ({caller_addr}) — \
                 its TBA will reject this unless you're an enrolled signer."
            );
        }
    }
    let token_id = registry::id_of_name(subguild).await.unwrap_or(0);

    // The TBA must be deployed before it can execute; deploy first if needed.
    if !registry::is_contract_deployed(&tba_addr).await.unwrap_or(false) {
        if token_id == 0 {
            eprintln!("{action}: '{subguild}' has no token id to deploy its TBA — run `tba deploy {subguild}`");
            return 1;
        }
        println!("{subguild}'s TBA {tba_addr} isn't deployed yet — deploying first …");
        if let Err(e) = registry::create_token_bound_account_sponsored(
            &signer,
            &sponsor,
            token_id,
            registry::ALPHA_USD_ADDRESS(),
        )
        .await
        {
            eprintln!("{action}: TBA deploy failed: {e}");
            return 1;
        }
    }

    println!("{subguild}'s TBA {tba_addr} → {action} …");
    match registry::tba_execute_call_sponsored(
        &signer,
        &sponsor,
        &tba_addr,
        registry::REGISTRY_ADDRESS(), // inner `to` = the diamond
        0,                          // no native value
        &calldata,
        registry::ALPHA_USD_ADDRESS(),
    )
    .await
    {
        Ok(tx) => {
            println!("✓ {action}  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("{action} failed: {e}");
            1
        }
    }
}

