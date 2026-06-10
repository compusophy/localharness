#[allow(unused_imports)]
use crate::*;

/// The on-chain facts `whoami` resolves for a name.
pub(crate) struct WhoamiInfo {
    name: String,
    owner: Option<String>,
    token_id: u64,
    tba: Option<String>,
    has_persona: bool,
    public_face: Option<String>,
}

/// Render a `WhoamiInfo` as the terminal report. Pure (no I/O) so the layout
/// is unit-testable. Unregistered names get a one-liner.
pub(crate) fn format_whoami(info: &WhoamiInfo) -> String {
    let Some(owner) = &info.owner else {
        return format!("{} is unregistered", info.name);
    };
    let wallet = match &info.tba {
        Some(a) => format!("{a}  (token-bound account)"),
        None => "—".to_string(),
    };
    let persona = if info.has_persona { "published" } else { "none" };
    let face = info
        .public_face
        .clone()
        .unwrap_or_else(|| "unset (directory)".to_string());
    format!(
        "{name}.localharness.xyz\n  \
         owner    {owner}\n  \
         tokenId  {id}\n  \
         wallet   {wallet}\n  \
         persona  {persona}\n  \
         face     {face}",
        name = info.name,
        id = info.token_id,
    )
}

/// Render a `WhoamiInfo` as a JSON object (`whoami --json`). Stable field
/// names so agents can script against the CLI. Pure.
pub(crate) fn format_whoami_json(info: &WhoamiInfo) -> String {
    let v = serde_json::json!({
        "name": info.name,
        "registered": info.owner.is_some(),
        "owner": info.owner,
        "tokenId": info.token_id,
        "wallet": info.tba,
        "persona": info.has_persona,
        "face": info.public_face,
    });
    serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string())
}

/// Resolve the on-chain profile of `<name>`. All read-only RPC — no `$LH`.
/// A failed sub-read (TBA / persona / face) degrades to absent rather than
/// failing the whole lookup; only an owner-read error is fatal.
pub(crate) async fn resolve_whoami(name: &str) -> Result<WhoamiInfo, String> {
    let owner = registry::owner_of_name(name).await?;
    if owner.is_none() {
        return Ok(WhoamiInfo {
            name: name.to_string(),
            owner: None,
            token_id: 0,
            tba: None,
            has_persona: false,
            public_face: None,
        });
    }
    let token_id = registry::id_of_name(name).await.unwrap_or(0);
    let tba = registry::tba_of_name(name).await.ok().flatten();
    let (has_persona, public_face) = if token_id != 0 {
        (
            registry::persona_of(token_id)
                .await
                .ok()
                .flatten()
                .is_some(),
            registry::public_face_of(token_id).await.ok().flatten(),
        )
    } else {
        (false, None)
    };
    Ok(WhoamiInfo {
        name: name.to_string(),
        owner,
        token_id,
        tba,
        has_persona,
        public_face,
    })
}

/// Print a profile of `<name>`: owner, tokenId, token-bound wallet, and
/// whether a persona / app face is published. `--json` for machine output.
pub(crate) async fn whoami(name: &str, json: bool) -> i32 {
    match resolve_whoami(name).await {
        Ok(info) => {
            println!(
                "{}",
                if json {
                    format_whoami_json(&info)
                } else {
                    format_whoami(&info)
                }
            );
            0
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            1
        }
    }
}

// ---- status (the unified read-only economy dashboard) --------------------
//
// `status [--as <me>] [<name>]` aggregates an agent's WHOLE on-chain state in
// ONE command — what previously meant running `whoami` + `credits` + `bounty
// mine` + `guild mine` + `reputation show` + `jobs` separately. Every read is
// READ-ONLY (no tx, no gas). With a `<name>` it's a PURE on-chain read of any
// agent; with no name it resolves the caller's OWN identity (needs a local key,
// like `bounty mine` / `guild mine` / `jobs`). It reuses the EXACT registry
// reads those commands use — `id_of_name`/`owner_of_name`/`main_of`/`name_of_id`
// + `tba_of_token_id` (identity), `token_balance_of` (balances), `reputation_of`
// (reputation), `guilds_of`/`guild_name`/`role_of_guild` (guilds),
// `bounties_of`/`get_bounty`/`task_of_bounty` (bounties), `jobs_of`/`get_job`/
// `task_of` (jobs) — so it adds NO new on-chain surface. Every list is bounded
// (`STATUS_LIST_CAP`) so a prolific agent doesn't flood the terminal, and every
// section degrades gracefully to "none" / "—" rather than failing the whole view.

/// How many rows each list section (guilds / bounties / jobs) prints before a
/// `… +N more` note. Keeps the dashboard a glanceable single screen.
pub(crate) const STATUS_LIST_CAP: usize = 10;

/// Render the trailing "… +N more (use `<hint>`)" note for a list section that
/// exceeded [`STATUS_LIST_CAP`], or empty when it didn't. Pure + testable.
pub(crate) fn status_more_note(total: usize, hint: &str) -> String {
    if total > STATUS_LIST_CAP {
        format!("    … +{} more (see `{hint}`)\n", total - STATUS_LIST_CAP)
    } else {
        String::new()
    }
}

/// `status [--as <me>] [<name>]` — the unified read-only economy dashboard.
/// Resolves the target identity (explicit `<name>` = any agent, pure read; else
/// the caller's own key), then prints Identity / Balances / Reputation / Guilds /
/// Bounties / Scheduled jobs. Read-only — no `$LH`, no tx, no gas.
pub(crate) async fn status(caller: Option<&str>, name: Option<&str>) -> i32 {
    // 1. Resolve the target: a name is a pure read of any agent; no name
    //    resolves the caller's own identity from their local key.
    let (label, owner_eoa, token_id) = match name {
        Some(n) => {
            let owner = match registry::owner_of_name(n).await {
                Ok(Some(o)) => o,
                Ok(None) => {
                    eprintln!("status: '{n}' is not registered");
                    return 1;
                }
                Err(e) => {
                    eprintln!("status: RPC error resolving '{n}': {e}");
                    return 1;
                }
            };
            let id = registry::id_of_name(n).await.unwrap_or(0);
            (n.to_string(), owner, id)
        }
        None => {
            let signer = match load_signer(caller) {
                Ok(s) => s,
                Err(code) => return code,
            };
            let addr = addr_to_hex(wallet::address(&signer));
            // Prefer the caller's MAIN identity for the tokenId-keyed sections;
            // fall back to the key-file stem as the display label.
            let main_id = registry::main_of(&addr).await.unwrap_or(0);
            let label = if main_id != 0 {
                registry::name_of_id(main_id)
                    .await
                    .ok()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| resolve_caller_label(caller).unwrap_or_else(|_| addr.clone()))
            } else {
                resolve_caller_label(caller).unwrap_or_else(|_| addr.clone())
            };
            (label, addr, main_id)
        }
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    println!("== {label} ==  (read-only economy dashboard)");

    // 2. Identity ----------------------------------------------------------
    println!("\nidentity");
    println!("  name      {label}");
    if token_id != 0 {
        println!("  tokenId   #{token_id}");
    } else {
        println!("  tokenId   — (no registered identity)");
    }
    // MAIN identity, shown only when it differs from this token (so a sub-name
    // surfaces its primary). owner_eoa here is the holder address.
    let main_id = registry::main_of(&owner_eoa).await.unwrap_or(0);
    if main_id != 0 && main_id != token_id {
        let main_name = registry::name_of_id(main_id)
            .await
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("token #{main_id}"));
        println!("  main      {main_name} (#{main_id})");
    }
    println!("  owner     {owner_eoa}");
    let tba = if token_id != 0 {
        registry::tba_of_token_id(token_id).await.ok().flatten()
    } else {
        None
    };
    match &tba {
        Some(a) => println!("  wallet    {a}  (token-bound account)"),
        None => println!("  wallet    —"),
    }

    // 3. Balances ($LH of the owner EOA + the TBA) -------------------------
    println!("\nbalances ($LH)");
    let eoa_bal = registry::token_balance_of(&owner_eoa).await.unwrap_or(0);
    println!("  owner EOA {}", fmt_lh(eoa_bal));
    match &tba {
        Some(a) => {
            let tba_bal = registry::token_balance_of(a).await.unwrap_or(0);
            println!("  TBA       {}", fmt_lh(tba_bal));
        }
        None => println!("  TBA       —"),
    }

    // 4. Reputation (attestation count + average ★) ------------------------
    println!("\nreputation");
    if token_id == 0 {
        println!("  none yet");
    } else {
        match registry::reputation_of(token_id).await {
            Ok((0, _)) => println!("  none yet"),
            Ok((count, sum)) => {
                // Average to 2 dp without floats (same math as `reputation show`).
                let avg_x100 = (sum * 100 + count / 2) / count;
                println!(
                    "  {count} attestation(s)   avg {}.{:02} / 5",
                    avg_x100 / 100,
                    avg_x100 % 100
                );
            }
            Err(e) => println!("  (could not read: {e})"),
        }
    }

    // 5. Guilds (guildsOf → id + name + the agent's role) ------------------
    println!("\nguilds");
    match registry::guilds_of(&owner_eoa).await {
        Ok(ids) if ids.is_empty() => println!("  none"),
        Ok(ids) => {
            let total = ids.len();
            for id in ids.into_iter().take(STATUS_LIST_CAP) {
                let gname = registry::guild_name(id).await.unwrap_or_default();
                let role = registry::role_of_guild(id, &owner_eoa)
                    .await
                    .map(|r| r.label().to_string())
                    .unwrap_or_else(|_| "?".to_string());
                let name_part = if gname.is_empty() {
                    String::new()
                } else {
                    format!(" '{gname}'")
                };
                println!("  #{id}{name_part}  [you: {role}]");
            }
            print!("{}", status_more_note(total, "guild mine"));
        }
        Err(e) => println!("  (could not read: {e})"),
    }

    // 6. Bounties (bountiesOf → posted: id + status) -----------------------
    println!("\nbounties posted");
    match registry::bounties_of(&owner_eoa).await {
        Ok(ids) if ids.is_empty() => println!("  none"),
        Ok(ids) => {
            let total = ids.len();
            for id in ids.into_iter().take(STATUS_LIST_CAP) {
                match registry::get_bounty(id).await {
                    Ok(b) => {
                        let task = registry::task_of_bounty(id).await.unwrap_or_default();
                        let snippet: String =
                            task.replace('\n', " ").chars().take(50).collect();
                        println!(
                            "  #{id}  reward {}  [{}]  {snippet}",
                            fmt_lh(b.reward_wei),
                            b.status_label()
                        );
                    }
                    Err(e) => println!("  #{id}  (could not read: {e})"),
                }
            }
            print!("{}", status_more_note(total, "bounty mine"));
        }
        Err(e) => println!("  (could not read: {e})"),
    }

    // 7. Scheduled jobs (jobsOf → active id + budget + interval) -----------
    println!("\nscheduled jobs");
    match registry::jobs_of(&owner_eoa).await {
        Ok(ids) if ids.is_empty() => println!("  none"),
        Ok(ids) => {
            let total = ids.len();
            for id in ids.into_iter().take(STATUS_LIST_CAP) {
                match registry::get_job(id).await {
                    Ok(job) => {
                        let target = registry::name_of_id(job.target_id)
                            .await
                            .ok()
                            .filter(|n| !n.is_empty())
                            .unwrap_or_else(|| format!("token#{}", job.target_id));
                        let next = if job.next_run == 0 {
                            "—".to_string()
                        } else if job.next_run <= now {
                            "due now".to_string()
                        } else {
                            format!("in {}", fmt_interval(job.next_run - now))
                        };
                        println!(
                            "  #{id}  -> {target}  every {}  next {next}  budget {}  [{}]",
                            fmt_interval(job.interval),
                            fmt_lh(job.budget_wei),
                            job.status_label()
                        );
                    }
                    Err(e) => println!("  #{id}  (could not read: {e})"),
                }
            }
            print!("{}", status_more_note(total, "jobs"));
        }
        Err(e) => println!("  (could not read: {e})"),
    }

    0
}

/// Parse `list`'s optional `--as <name>` / `--json` flags (order-independent).
/// `list` takes no positional args — anything else is an error.
pub(crate) fn parse_list_flags(args: &[String]) -> Result<(Option<String>, bool), String> {
    let (mut caller, mut json, mut i) = (None, false, 0);
    while i < args.len() {
        match args[i].as_str() {
            "--as" => {
                caller = Some(
                    args.get(i + 1)
                        .ok_or("usage: localharness list [--as <me>] [--json]")?
                        .clone(),
                );
                i += 2;
            }
            "--json" => {
                json = true;
                i += 1;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok((caller, json))
}

/// Render the caller's owned subdomains. Pure (no I/O) so it's unit-testable.
pub(crate) fn format_owned(addr: &str, tokens: &[registry::OwnedToken], json: bool) -> String {
    if json {
        let arr: Vec<serde_json::Value> = tokens
            .iter()
            .map(|t| {
                serde_json::json!({ "name": t.name, "tokenId": t.token_id, "wallet": t.tba })
            })
            .collect();
        return serde_json::to_string_pretty(&serde_json::json!({
            "owner": addr,
            "count": tokens.len(),
            "subdomains": arr,
        }))
        .unwrap_or_else(|_| "{}".to_string());
    }
    if tokens.is_empty() {
        return format!("no subdomains owned by {addr}\n");
    }
    let mut out = format!("{} subdomain(s) owned by {addr}:\n", tokens.len());
    for t in tokens {
        let wallet = t.tba.as_deref().unwrap_or("—");
        out.push_str(&format!("  {}  (tokenId {})  {wallet}\n", t.name, t.token_id));
    }
    out
}

/// List the subdomains the caller's identity owns (read-only — no `$LH`).
/// Mirrors the browser `list_subdomains` tool.
pub(crate) async fn list_mine(caller_name: Option<&str>, json: bool) -> i32 {
    let signer = match load_signer(caller_name) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let addr = format!("0x{}", to_hex(&wallet::address(&signer)));
    match registry::list_owned_tokens(&addr).await {
        Ok(tokens) => {
            print!("{}", format_owned(&addr, &tokens, json));
            0
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            1
        }
    }
}

/// A parsed `qa/v1` autonomous-fleet feedback envelope. `version`/`body` are
/// consumed by the triage agent (roadmap Phase 4); `source` tags the listing.
#[allow(dead_code)]
pub(crate) struct QaEnvelope {
    source: String,
    version: String,
    body: String,
}

/// Parse a `qa/v1 source=<s> v<ver>: <body>` envelope. `None` unless it is a
/// well-formed qa/v1 envelope — the triage path must NOT consume a body (e.g.
/// a repro string) from a malformed or non-fleet entry, since the feedback log
/// is permissionless and an attacker can plant crafted text (a critique gate).
pub(crate) fn parse_qa_envelope(text: &str) -> Option<QaEnvelope> {
    let (header, body) = text.strip_prefix("qa/v1 ")?.split_once(": ")?;
    let source = header.split_whitespace().find_map(|t| t.strip_prefix("source="))?;
    let version = header.split_whitespace().find_map(|t| {
        t.strip_prefix('v')
            .filter(|v| v.starts_with(|c: char| c.is_ascii_digit()))
    })?;
    if source.is_empty() || body.trim().is_empty() {
        return None;
    }
    Some(QaEnvelope {
        source: source.to_string(),
        version: version.to_string(),
        body: body.to_string(),
    })
}

/// Render the on-chain feedback log (newest first). Pure for testing. Entries
/// the autonomous fleet authored (valid `qa/v1` envelopes) are tagged so the
/// maintainer can tell agent-filed bugs from human ones at a glance.
pub(crate) fn format_feedback(entries: &[registry::FeedbackEntry]) -> String {
    if entries.is_empty() {
        return "no on-chain feedback yet\n".to_string();
    }
    let mut out = format!("{} on-chain feedback entr(ies), newest first:\n", entries.len());
    for e in entries {
        let tag = match parse_qa_envelope(&e.text) {
            Some(env) => format!(" [fleet:{}]", env.source),
            None => String::new(),
        };
        out.push_str(&format!(
            "  [{}] {}{}\n    {}\n",
            e.timestamp,
            e.sender,
            tag,
            e.text.replace('\n', " ")
        ));
    }
    out
}

/// Collapse feedback bodies into a deduplicated, recurrence-ranked work-list:
/// the same bug filed across many probe runs becomes ONE item, ranked by how
/// often it recurred (most-reported first). Dedup BEFORE ranking, else the
/// log's natural repetition drowns the signal. Ties break by first-seen order
/// for stable output. The triage agent's deterministic core (roadmap Phase 4).
pub(crate) fn triage_findings(bodies: &[String]) -> Vec<(String, usize)> {
    use std::collections::HashMap;
    // key -> (representative text, count, first-seen index)
    let mut counts: HashMap<String, (String, usize, usize)> = HashMap::new();
    for (i, body) in bodies.iter().enumerate() {
        let key = body.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        if key.is_empty() {
            continue;
        }
        let e = counts.entry(key).or_insert_with(|| (body.trim().to_string(), 0, i));
        e.1 += 1;
    }
    let mut v: Vec<(String, usize, usize)> = counts.into_values().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)));
    v.into_iter().map(|(rep, count, _)| (rep, count)).collect()
}

/// `localharness triage` — read the on-chain feedback log and print a
/// deduplicated, recurrence-ranked work-list. Read-only, no `$LH`. Prefers the
/// `qa/v1` body when an entry is a fleet envelope.
pub(crate) async fn triage() -> i32 {
    let entries = match registry::list_feedback().await {
        Ok(e) => e,
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    };
    let bodies: Vec<String> = entries
        .iter()
        .map(|e| parse_qa_envelope(&e.text).map(|env| env.body).unwrap_or_else(|| e.text.clone()))
        .collect();
    let ranked = triage_findings(&bodies);
    if ranked.is_empty() {
        println!("no feedback to triage");
        return 0;
    }
    println!("{} distinct item(s), most-recurring first:", ranked.len());
    for (i, (rep, count)) in ranked.iter().enumerate() {
        println!("  {}. (x{count}) {}", i + 1, rep.replace('\n', " "));
    }
    0
}

/// How many matched agents `discover` prints (and the cap on the extra
/// reputation reads — we only fetch reputation for the agents actually shown,
/// not the whole scanned set, so the per-result RPC cost stays bounded).
pub(crate) const DISCOVER_SHOW: usize = 20;

/// Format an agent's on-chain reputation as a compact inline tag for `discover`.
/// `rep` is `Some((count, sum))` from `reputation_of` (sum ≤ 5·count), or `None`
/// when the read FAILED (shown as `—`, so one bad RPC never sinks the row). The
/// average is `sum/count` to 2 dp WITHOUT floats — the same math as
/// `reputation show` — so the two surfaces always agree. Pure (no I/O).
pub(crate) fn format_reputation_inline(rep: Option<(u64, u64)>) -> String {
    match rep {
        None => "reputation: —".to_string(),
        Some((0, _)) => "reputation: none yet".to_string(),
        Some((count, sum)) => {
            // Average to 2 dp without floats: (sum*100)/count rounded — identical
            // to `reputation_show`'s `avg_x100`.
            let avg_x100 = (sum * 100 + count / 2) / count;
            let plural = if count == 1 { "attestation" } else { "attestations" };
            format!(
                "reputation {}.{:02} from {count} {plural}",
                avg_x100 / 100,
                avg_x100 % 100
            )
        }
    }
}

/// `localharness discover <query>` — the Agent Yellow Pages: search the on-chain
/// registry for agents whose name or persona matches `<query>`, so you can find
/// a peer by capability and then `call` / `mcp-call` it. Read-only, no `$LH`.
///
/// Each printed result also shows that agent's on-chain reputation inline (avg
/// rating + attestation count, or "none yet") so capability AND track-record are
/// weighed together — no separate `reputation show` per agent. Ordering is
/// UNCHANGED: results stay in `discover_agents`' query-match order (reputation is
/// informational — a fresh, capable agent legitimately has 0 attestations, so
/// ranking by it would bury new agents). We only fetch reputation for the agents
/// actually printed (≤ `DISCOVER_SHOW`), and a failed read shows `—` rather than
/// failing the whole command.
pub(crate) async fn discover(query: &str) -> i32 {
    const SCAN: u64 = 100;
    match registry::discover_agents(query, SCAN).await {
        Ok(matches) if matches.is_empty() => {
            println!("no agents match \"{query}\" (scanned the {SCAN} most recent)");
            0
        }
        Ok(matches) => {
            println!("{} agent(s) matching \"{query}\":", matches.len());
            println!("(ordered by task match; reputation shown for context, not ranked on)");
            for (name, persona) in matches.iter().take(DISCOVER_SHOW) {
                let snippet: String = persona.replace('\n', " ").chars().take(100).collect();
                let snippet = if snippet.trim().is_empty() {
                    "(no persona)".to_string()
                } else {
                    snippet
                };
                // Reputation is informational only — resolve this printed agent's
                // tokenId (`discover_agents` discards it) and read its on-chain
                // attestation count + rating sum. Any failure (unregistered name /
                // RPC error) degrades to "—" so one bad read never sinks the row
                // or the command; the ranking above is never touched.
                let rep = match registry::id_of_name(name).await {
                    Ok(id) if id != 0 => registry::reputation_of(id).await.ok(),
                    _ => None,
                };
                let rep_tag = format_reputation_inline(rep);
                println!("  {name}.localharness.xyz — {snippet}  [{rep_tag}]");
            }
            println!("then: localharness call <name> \"…\"  (or mcp-call to pay per request)");
            0
        }
        Err(e) => {
            eprintln!("discover: RPC error: {e}");
            1
        }
    }
}

/// Read the on-chain feedback log (`localharness feedback`, no text). With
/// `--json`, emit a machine-readable array instead of the human view — for
/// tooling like the feedback→GitHub-issues bridge.
pub(crate) async fn feedback_read(json: bool) -> i32 {
    match registry::list_feedback().await {
        Ok(entries) => {
            if json {
                print!("{}", feedback_json(&entries));
            } else {
                print!("{}", format_feedback(&entries));
            }
            0
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            1
        }
    }
}

/// Render the feedback log as a JSON array (`feedback --json`), newest first,
/// matching the human view. Each item: `{ timestamp, sender, text }`, plus
/// `{ fleet_source, body }` when the entry is a `qa/v1` fleet envelope.
/// `(timestamp, sender)` is a stable dedup key for tooling — `list_feedback`
/// is a windowed log scan, so there's no stable on-chain append index to emit.
pub(crate) fn feedback_json(entries: &[registry::FeedbackEntry]) -> String {
    let items: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| {
            let mut o = serde_json::json!({
                "timestamp": e.timestamp,
                "sender": e.sender,
                "text": e.text,
            });
            if let Some(env) = parse_qa_envelope(&e.text) {
                o["fleet_source"] = serde_json::json!(env.source);
                o["body"] = serde_json::json!(env.body);
            }
            o
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::Value::Array(items))
        .unwrap_or_else(|_| "[]".to_string())
        + "\n"
}

/// Submit on-chain feedback as the caller's identity (sponsored). This is the
/// agent-to-platform leg of the feedback loop: a test agent reports bugs / UX
/// friction / errors here, and `feedback` (no text) reads them back.
pub(crate) async fn feedback_submit(caller_name: Option<&str>, text: &str) -> i32 {
    let text = text.trim();
    if text.is_empty() {
        eprintln!("feedback text is empty");
        return 2;
    }
    if text.len() > 2048 {
        eprintln!("feedback too long: {} bytes (max 2048)", text.len());
        return 1;
    }
    let (signer, sponsor) = match load_signer_and_sponsor(caller_name) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("submitting {}-byte feedback on-chain …", text.len());
    match registry::submit_feedback_sponsored(&signer, &sponsor, text, registry::ALPHA_USD_ADDRESS)
        .await
    {
        Ok(tx) => {
            println!("✓ feedback submitted\n  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("feedback failed: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_reputation_inline_formats_like_reputation_show() {
        // No attestations → "none yet" (matches the zero branch of `reputation show`).
        assert_eq!(format_reputation_inline(Some((0, 0))), "reputation: none yet");
        // A failed read → "—", so one bad RPC degrades a row without sinking it.
        assert_eq!(format_reputation_inline(None), "reputation: —");
        // 21/5 = 4.20 → 2 dp, no floats; plural for >1 attestation.
        assert_eq!(
            format_reputation_inline(Some((5, 21))),
            "reputation 4.20 from 5 attestations"
        );
        // A single attestation is grammatically singular.
        assert_eq!(
            format_reputation_inline(Some((1, 4))),
            "reputation 4.00 from 1 attestation"
        );
        // Rounding half-up to 2 dp matches `reputation show`'s avg_x100: 10/3 = 3.333… → 3.33.
        assert_eq!(
            format_reputation_inline(Some((3, 10))),
            "reputation 3.33 from 3 attestations"
        );
    }

    #[test]
    fn format_whoami_unregistered_is_one_line() {
        let info = WhoamiInfo {
            name: "ghost".into(),
            owner: None,
            token_id: 0,
            tba: None,
            has_persona: false,
            public_face: None,
        };
        assert_eq!(format_whoami(&info), "ghost is unregistered");
    }

    #[test]
    fn format_whoami_full_profile() {
        let info = WhoamiInfo {
            name: "claude".into(),
            owner: Some("0xabc".into()),
            token_id: 8,
            tba: Some("0xdef".into()),
            has_persona: true,
            public_face: Some("app".into()),
        };
        let out = format_whoami(&info);
        assert!(out.starts_with("claude.localharness.xyz\n"));
        assert!(out.contains("owner    0xabc"));
        assert!(out.contains("tokenId  8"));
        assert!(out.contains("wallet   0xdef  (token-bound account)"));
        assert!(out.contains("persona  published"));
        assert!(out.contains("face     app"));
    }

    #[test]
    fn format_whoami_absent_persona_and_face() {
        let info = WhoamiInfo {
            name: "bare".into(),
            owner: Some("0x1".into()),
            token_id: 3,
            tba: None,
            has_persona: false,
            public_face: None,
        };
        let out = format_whoami(&info);
        assert!(out.contains("persona  none"));
        assert!(out.contains("face     unset (directory)"));
        assert!(out.contains("wallet   —"));
    }

    #[test]
    fn format_whoami_json_registered_roundtrips() {
        let info = WhoamiInfo {
            name: "claude".into(),
            owner: Some("0xabc".into()),
            token_id: 8,
            tba: Some("0xdef".into()),
            has_persona: true,
            public_face: Some("app".into()),
        };
        let v: serde_json::Value = serde_json::from_str(&format_whoami_json(&info)).unwrap();
        assert_eq!(v["name"], "claude");
        assert_eq!(v["registered"], true);
        assert_eq!(v["owner"], "0xabc");
        assert_eq!(v["tokenId"], 8);
        assert_eq!(v["wallet"], "0xdef");
        assert_eq!(v["persona"], true);
        assert_eq!(v["face"], "app");
    }

    #[test]
    fn format_whoami_json_unregistered_nulls() {
        let info = WhoamiInfo {
            name: "ghost".into(),
            owner: None,
            token_id: 0,
            tba: None,
            has_persona: false,
            public_face: None,
        };
        let v: serde_json::Value = serde_json::from_str(&format_whoami_json(&info)).unwrap();
        assert_eq!(v["registered"], false);
        assert!(v["owner"].is_null());
        assert!(v["wallet"].is_null());
        assert!(v["face"].is_null());
        assert_eq!(v["persona"], false);
    }

    #[test]
    fn status_more_note_caps_and_hints() {
        // At or below the cap → no note.
        assert_eq!(status_more_note(0, "jobs"), "");
        assert_eq!(status_more_note(STATUS_LIST_CAP, "jobs"), "");
        // Over the cap → the overflow count + the drill-down hint.
        let n = status_more_note(STATUS_LIST_CAP + 3, "bounty mine");
        assert!(n.contains("+3 more"));
        assert!(n.contains("bounty mine"));
        assert!(n.ends_with('\n'));
    }

    #[test]
    fn parse_list_flags_handles_as_and_json_any_order() {
        assert_eq!(parse_list_flags(&args(&[])).unwrap(), (None, false));
        assert_eq!(parse_list_flags(&args(&["--json"])).unwrap(), (None, true));
        let (c, j) = parse_list_flags(&args(&["--as", "bob", "--json"])).unwrap();
        assert_eq!((c.as_deref(), j), (Some("bob"), true));
        let (c, j) = parse_list_flags(&args(&["--json", "--as", "bob"])).unwrap();
        assert_eq!((c.as_deref(), j), (Some("bob"), true));
        assert!(parse_list_flags(&args(&["--as"])).is_err()); // dangling --as
        assert!(parse_list_flags(&args(&["alice"])).is_err()); // no positionals
    }

    #[test]
    fn format_owned_text_and_json() {
        let toks = vec![
            registry::OwnedToken { token_id: 8, name: "claude".into(), tba: Some("0xabc".into()) },
            registry::OwnedToken { token_id: 3, name: "alice".into(), tba: None },
        ];
        let text = format_owned("0xowner", &toks, false);
        assert!(text.contains("2 subdomain"));
        assert!(text.contains("claude  (tokenId 8)  0xabc"));
        assert!(text.contains("alice  (tokenId 3)  —"));

        let v: serde_json::Value =
            serde_json::from_str(&format_owned("0xowner", &toks, true)).unwrap();
        assert_eq!(v["count"], 2);
        assert_eq!(v["owner"], "0xowner");
        assert_eq!(v["subdomains"][0]["name"], "claude");
        assert_eq!(v["subdomains"][0]["tokenId"], 8);
        assert!(v["subdomains"][1]["wallet"].is_null());
    }

    #[test]
    fn triage_dedups_and_ranks_by_recurrence() {
        let bodies = vec![
            "Compile leaks OS error".to_string(),
            "compile leaks os error".to_string(),       // same modulo case
            "  Compile   leaks OS error ".to_string(),  // same modulo whitespace
            "whoami is slow".to_string(),
        ];
        let ranked = triage_findings(&bodies);
        assert_eq!(ranked.len(), 2, "two distinct issues after dedup");
        assert_eq!(ranked[0].1, 3, "the recurring one ranks first with count 3");
        assert!(ranked[0].0.to_lowercase().contains("compile leaks"));
        assert_eq!(ranked[1].1, 1);
    }

    #[test]
    fn triage_skips_empty_bodies() {
        let bodies = vec!["".to_string(), "   ".to_string(), "real bug".to_string()];
        let ranked = triage_findings(&bodies);
        assert_eq!(ranked, vec![("real bug".to_string(), 1)]);
    }

    #[test]
    fn parse_qa_envelope_accepts_valid_rejects_others() {
        let env =
            parse_qa_envelope("qa/v1 source=qa-probe v0.20.0: compile leaked os error").unwrap();
        assert_eq!(env.source, "qa-probe");
        assert_eq!(env.version, "0.20.0");
        assert!(env.body.contains("compile leaked"));
        // Not a fleet envelope → rejected (triage won't consume its body).
        assert!(parse_qa_envelope("just some human feedback").is_none());
        assert!(parse_qa_envelope("qa/v1 source=x v1.0.0:   ").is_none()); // empty body
        assert!(parse_qa_envelope("qa/v1 no source or colon").is_none());
        assert!(parse_qa_envelope("qa/v1 source=x vNOTVERSION: body").is_none());
    }

    #[test]
    fn feedback_json_emits_fields_and_fleet_envelope() {
        let entries = vec![
            registry::FeedbackEntry {
                sender: "0xabc".into(),
                timestamp: 100,
                text: "[BUG] something broke".into(),
            },
            registry::FeedbackEntry {
                sender: "0xdef".into(),
                timestamp: 200,
                text: "qa/v1 source=qa-probe v0.20.0: a real bug".into(),
            },
        ];
        let v: serde_json::Value = serde_json::from_str(&feedback_json(&entries)).unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        // plain entry: dedup-key fields + raw text, no fleet fields
        assert_eq!(arr[0]["sender"], "0xabc");
        assert_eq!(arr[0]["timestamp"], 100);
        assert_eq!(arr[0]["text"], "[BUG] something broke");
        assert!(arr[0].get("fleet_source").is_none());
        // qa/v1 envelope: gets fleet_source + decoded body
        assert_eq!(arr[1]["fleet_source"], "qa-probe");
        assert!(arr[1]["body"].as_str().unwrap().contains("a real bug"));
        // empty log → valid empty array
        let empty: serde_json::Value = serde_json::from_str(&feedback_json(&[])).unwrap();
        assert_eq!(empty.as_array().unwrap().len(), 0);
    }

    #[test]
    fn format_feedback_tags_fleet_envelopes_only() {
        let entries = vec![
            registry::FeedbackEntry {
                sender: "0x1".into(),
                timestamp: 1,
                text: "qa/v1 source=qa-probe v0.20.0: a real bug".into(),
            },
            registry::FeedbackEntry {
                sender: "0x2".into(),
                timestamp: 2,
                text: "a human note".into(),
            },
        ];
        let out = format_feedback(&entries);
        assert!(out.contains("[fleet:qa-probe]"));
        assert!(
            out.lines().any(|l| l.contains("0x2") && !l.contains("[fleet")),
            "human feedback must not be tagged as fleet"
        );
    }

    #[test]
    fn format_feedback_empty_and_entries() {
        assert!(format_feedback(&[]).contains("no on-chain feedback"));
        let entries = vec![
            registry::FeedbackEntry {
                sender: "0xabc".into(),
                timestamp: 1700000000,
                text: "create flow worked\nbut whoami was slow".into(),
            },
        ];
        let out = format_feedback(&entries);
        assert!(out.contains("1 on-chain feedback"));
        assert!(out.contains("0xabc"));
        // Newlines collapsed so one entry stays one block.
        assert!(out.contains("create flow worked but whoami was slow"));
    }

    #[test]
    fn format_owned_empty() {
        assert!(format_owned("0xo", &[], false).contains("no subdomains"));
        let v: serde_json::Value = serde_json::from_str(&format_owned("0xo", &[], true)).unwrap();
        assert_eq!(v["count"], 0);
        assert!(v["subdomains"].as_array().unwrap().is_empty());
    }
}
