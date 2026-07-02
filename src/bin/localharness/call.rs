use crate::{bytes_to_hex_str, default_persona, ensure_diamond_allowance, fmt_duration, fmt_lh, load_sponsor, non_blank, parse_address, registry, resolve_caller_key, resolve_caller_label, sponsor_key, take_as_flag, wallet, CALL_COST_WEI, CALL_METER_TOPUP_WEI};

/// Prompt another agent and print its reply — HEADLESS, via the credit proxy.
///
/// No Gemini key, no live browser tab, no relay: this process runs an agent
/// turn itself, authenticating to the proxy with YOUR identity key (which
/// also spends your `$LH`) and running under the TARGET's on-chain persona so
/// it answers *as* that agent. The `?rpc=1` browser endpoint is postMessage-
/// only (tab-to-tab) and a static host can't accept an HTTP POST — so the old
/// `POST .../?rpc=1` path here always 405'd; the proxy is the real bridge.
///
///   localharness call [--as <yourname>] <target> <message…>
/// Parsed `call` arguments: the optional `--as` caller, whether `--fresh` was
/// given (start a new conversation, ignoring saved history), the target name,
/// and the joined message. Pure (no I/O) so it is unit-testable; `Err` carries
/// the usage line to print. Leading `--as`/`--fresh` flags may appear in any
/// order before the target.
pub(crate) struct ParsedCall {
    caller: Option<String>,
    fresh: bool,
    model: Option<String>,
    pay: Option<String>,
    /// `--verify <keys>`: comma-separated REQUIRED top-level JSON keys the reply
    /// must contain (an escrow gate that only matters together with `--pay`).
    /// `None` = no verification; settle on any non-empty reply as before.
    verify: Option<Vec<String>>,
    target: String,
    message: String,
}

pub(crate) const CALL_USAGE: &str =
    "usage: localharness call [--as <yourname>] [--fresh] [--model <id>] [--pay <amount>] [--verify <keys>] <target> <message>";

/// Split a `--verify` flag value into the list of required top-level JSON keys.
/// Comma-separated, each trimmed of surrounding whitespace; blank entries are
/// dropped (so `a,,b` → `["a","b"]` and a trailing comma is harmless). Pure.
pub(crate) fn parse_verify_keys(spec: &str) -> Vec<String> {
    spec.split(',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_string)
        .collect()
}

pub(crate) fn parse_call_args(rest: &[String]) -> Result<ParsedCall, String> {
    // `--as` is pulled from ANY position (via take_as_flag — consistent with
    // schedule/invite/send), so `call <target> "msg" --as me` works, not just
    // the leading form. --model/--fresh/--pay/--verify stay leading flags before
    // the target.
    let (caller, rest) = take_as_flag(rest)?;
    let mut fresh = false;
    let mut model = None;
    let mut pay = None;
    let mut verify = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--model" => match rest.get(i + 1) {
                Some(m) => {
                    model = Some(m.clone());
                    i += 2;
                }
                None => return Err(CALL_USAGE.to_string()),
            },
            "--pay" => match rest.get(i + 1) {
                Some(p) => {
                    pay = Some(p.clone());
                    i += 2;
                }
                None => return Err(CALL_USAGE.to_string()),
            },
            "--verify" => match rest.get(i + 1) {
                Some(v) => {
                    verify = Some(parse_verify_keys(v));
                    i += 2;
                }
                None => return Err(CALL_USAGE.to_string()),
            },
            "--fresh" => {
                fresh = true;
                i += 1;
            }
            _ => break,
        }
    }
    match rest[i..].split_first() {
        Some((t, msg)) if !msg.is_empty() => {
            // Guard the silent-swallow footgun: --model/--fresh/--pay/--verify are
            // LEADING flags (before the target) so they aren't parsed out of the
            // message text. If one lands AFTER the target it would be swallowed into
            // the message — silently DROPPING A PAYMENT (`--pay`), a model choice, etc.
            // (`--as` is position-independent and already pulled out above.) Error
            // clearly instead of paying nothing.
            if let Some(f) = msg
                .iter()
                .find(|w| matches!(w.as_str(), "--pay" | "--model" | "--verify" | "--fresh"))
            {
                return Err(format!(
                    "`{f}` must come BEFORE the target (e.g. `call {f} … {t} \"message\"`) — \
                     after the target it is swallowed into the message and ignored.\n{CALL_USAGE}"
                ));
            }
            Ok(ParsedCall {
                caller,
                fresh,
                model,
                pay,
                verify,
                target: t.clone(),
                message: msg.join(" "),
            })
        }
        _ => Err(CALL_USAGE.to_string()),
    }
}

/// Check that `reply` is a JSON OBJECT containing every key in `required`.
/// The escrow gate behind `--verify`: `Ok(())` → the reply satisfies the
/// schema; `Err(reason)` → a human-readable failure ("reply not JSON",
/// "missing key 'X'") to surface BEFORE withholding payment. `serde_json` is
/// already a crate dependency, so this is a real structural JSON parse — not a
/// substring heuristic — but it is a key-PRESENCE check, NOT full JSON Schema
/// (value types/shapes are not validated). Pure.
pub(crate) fn verify_reply(reply: &str, required: &[String]) -> Result<(), String> {
    // Agents routinely wrap JSON in markdown fences (```json … ```) or surround it
    // with prose, so extract the object (first '{' .. last '}') before parsing —
    // otherwise a perfectly valid reply would falsely WITHHOLD payment (the
    // false-negative the zero-trust gate must avoid).
    let trimmed = reply.trim();
    let json = match (trimmed.find('{'), trimmed.rfind('}')) {
        (Some(a), Some(b)) if b >= a => &trimmed[a..=b],
        _ => trimmed,
    };
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|_| "reply not JSON".to_string())?;
    let obj = value
        .as_object()
        .ok_or_else(|| "reply not a JSON object".to_string())?;
    for key in required {
        if !obj.contains_key(key) {
            return Err(format!("missing key '{key}'"));
        }
    }
    Ok(())
}

/// The directory holding persisted `call` conversations.
pub(crate) fn history_dir() -> std::path::PathBuf {
    std::path::Path::new(".localharness").join("history")
}

/// The serialization-backend tag a `model` routes to. Conversation history is
/// serialized in a BACKEND-SPECIFIC wire shape (a Gemini thread loaded into the
/// Anthropic backend dies with `missing field 'content'` and vice-versa), so
/// the persisted thread is keyed by this tag — the two backends never share a
/// file. Mirrors the `claude*` → Anthropic routing in `run_agent_turn`.
pub(crate) fn model_backend_tag(model: Option<&str>) -> &'static str {
    if model.map(|m| m.starts_with("claude")).unwrap_or(false) {
        "anthropic"
    } else {
        "gemini"
    }
}

/// Where a `call` conversation between `caller_label` and `target` on a given
/// `backend` is persisted, so repeated calls continue the same thread. Keyed by
/// backend too so a Gemini thread and an Anthropic thread to the same target
/// never collide (their on-disk formats are incompatible). Pure path builder.
pub(crate) fn history_path(caller_label: &str, target: &str, backend: &str) -> std::path::PathBuf {
    history_dir().join(format!("{caller_label}__{target}.{backend}.bin"))
}

/// Extract the target from a history filename `<caller>__<target>.<backend>.bin`
/// (or the legacy `<caller>__<target>.bin`) for the given caller label. `None`
/// when it doesn't belong to that caller. Pure. A trailing `.gemini`/`.anthropic`
/// backend tag is stripped so `threads`/`forget` show the bare target.
pub(crate) fn thread_file_target(caller_label: &str, file_name: &str) -> Option<String> {
    let stem = file_name
        .strip_prefix(&format!("{caller_label}__"))?
        .strip_suffix(".bin")
        .filter(|t| !t.is_empty())?;
    // Drop a known backend tag if present (newer files); legacy files have none.
    let target = stem
        .strip_suffix(".gemini")
        .or_else(|| stem.strip_suffix(".anthropic"))
        .unwrap_or(stem);
    if target.is_empty() {
        return None;
    }
    Some(target.to_string())
}

/// Map a failed `call` error to an actionable hint, if recognisable. Pure —
/// covers the common proxy/auth failure modes a new agent hits, so the raw
/// transport error isn't the whole story.
pub(crate) fn hint_for_call_error(err: &str) -> Option<&'static str> {
    let e = err.to_ascii_lowercase();
    // Below-the-agent's-price 402s are NOT a credits problem — hint the
    // price mechanism, not funding (checked before the generic 402 arm).
    if e.contains("below") && e.contains("price") {
        return Some(
            "your --pay is under the agent's advertised price — use `--pay auto` \
             (the default) to pay exactly its price, or `whoami <name>` to see it.",
        );
    }
    if e.contains("402")
        || e.contains("payment")
        || e.contains("no session")
        || e.contains("insufficient")
        || e.contains("credit")
    {
        return Some(
            "the credit proxy has no $LH for your identity. `call` meters \
             ~1 $LH per request, so a fresh identity must be funded first — \
             run `localharness redeem <code>`, or have another agent `send` you $LH.",
        );
    }
    if e.contains("401")
        || e.contains("403")
        || e.contains("unauthorized")
        || e.contains("forbidden")
        || e.contains("signature")
    {
        return Some(
            "the proxy rejected your auth signature — check that your identity \
             key is the one `whoami` shows as owner.",
        );
    }
    if e.contains("429") || e.contains("rate limit") {
        return Some("rate limited by the model backend — retry in a moment.");
    }
    None
}

/// Print an error line plus its actionable hint, if any.
pub(crate) fn report_call_error(prefix: &str, err: &str) {
    eprintln!("{prefix}: {err}");
    if let Some(hint) = hint_for_call_error(err) {
        eprintln!("  hint: {hint}");
    }
}

pub(crate) async fn call(rest: &[String]) -> i32 {
    let ParsedCall {
        caller,
        fresh,
        model,
        pay,
        verify,
        target,
        message,
    } = match parse_call_args(rest) {
        Ok(p) => p,
        Err(usage) => {
            eprintln!("{usage}");
            return 2;
        }
    };

    // An empty / whitespace-only message used to run (and BILL) a full metered
    // turn — reject it BEFORE any identity/meter/RPC work.
    if let Err(e) = non_blank(&message, "call: message") {
        eprintln!("{e}");
        return 1;
    }

    // `--pay`: validate the amount BEFORE running (and paying for) the turn.
    // `--pay auto` resolves the target's effective price (advertised
    // on-chain, else the platform default) — the same number the hosted
    // ask_agent gate would enforce.
    let pay_wei = match pay.as_deref() {
        None => None,
        Some("auto") => match registry::id_of_name(&target).await {
            Ok(id) if id != 0 => match registry::x402_ask_price_of(id).await {
                Ok(wei) => {
                    println!("--pay auto: '{target}' asks {}/call (paid to the agent; the model run also meters ~1 LH)", fmt_lh(wei));
                    Some(wei)
                }
                Err(e) => {
                    eprintln!("--pay auto: price lookup failed: {e}");
                    return 1;
                }
            },
            Ok(_) => {
                eprintln!("--pay auto: '{target}' is not a registered agent");
                return 2;
            }
            Err(e) => {
                eprintln!("--pay auto: RPC error: {e}");
                return 1;
            }
        },
        Some(p) => match localharness::encoding::parse_token_amount(p) {
            Some(v) if v > 0 => Some(v),
            _ => {
                eprintln!("--pay must be a positive $LH amount (e.g. 0.001) or 'auto', got '{p}'");
                return 2;
            }
        },
    };

    // Resolve the caller's identity key — it signs proxy auth + pays $LH.
    let (_key_file, key_hex) = match resolve_caller_key(caller.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    // Conversations persist per (caller, target, backend) so repeated calls
    // continue the same thread; `--fresh` starts over. Label by the bare
    // key-file stem (`resolve_caller_label` — basename minus KEY_SUFFIX), so a
    // cwd key and a config-home key for the same name share one history thread.
    // Keying on the backend too keeps a Gemini thread and an Anthropic thread
    // to the same target in SEPARATE files — their on-disk history formats are
    // incompatible (a Gemini thread loaded into the Anthropic backend dies with
    // `missing field 'content'`).
    let caller_label = match resolve_caller_label(caller.as_deref()) {
        Ok(l) => l,
        Err(e) => {
            // Unreachable in practice: resolve_caller_key above already resolved
            // the same identity. Mirror its exit code if the fs raced us.
            eprintln!("{e}");
            return 2;
        }
    };
    let backend = model_backend_tag(model.as_deref());
    let hist_file = history_path(&caller_label, &target, backend);
    let prior_history = if fresh {
        let _ = std::fs::remove_file(&hist_file);
        None
    } else {
        // A read failure (missing/corrupt file) is non-fatal: start fresh.
        std::fs::read(&hist_file).ok()
    };
    match run_agent_turn(&key_hex, &target, &message, prior_history, model.as_deref()).await {
        Ok((text, new_history)) => {
            // An empty reply is not a billable success — don't settle for a
            // blank answer (QA fleet: "charged, got zero response"). Mirrors the
            // proxy ask_agent + metered-path "no content, no charge" rule.
            if text.trim().is_empty() {
                eprintln!("call: the agent returned no text — no payment settled");
                return 1;
            }
            println!("{}", text.trim());
            // Persist the conversation so the next `call` to this target
            // continues it. Best-effort: a save failure must not flip the code.
            if let Some(bytes) = new_history {
                if let Some(dir) = hist_file.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&hist_file, bytes);
            }
            // `--verify <keys>`: a native escrow gate — withhold the payment
            // unless the reply is a JSON object carrying every required key.
            // Only meaningful WITH `--pay` (no payment ⇒ nothing to withhold);
            // the reply is still shown above either way. A failure prints why
            // and DOES NOT settle (the $LH stays with the caller).
            if let (Some(required), Some(value_wei)) = (verify.as_deref(), pay_wei) {
                if let Err(reason) = verify_reply(&text, required) {
                    eprintln!(
                        "--verify: {reason} — payment NOT sent ({} withheld)",
                        fmt_lh(value_wei)
                    );
                    return 1;
                }
            }
            // `--pay`: settle the $LH to the target AFTER a successful (and, with
            // `--verify`, schema-valid) reply — a failed call costs the caller
            // nothing.
            match pay_wei {
                Some(value_wei) => settle_call_payment(&key_hex, &target, value_wei).await,
                None => 0,
            }
        }
        Err(e) => {
            report_call_error("call failed", &e);
            1
        }
    }
}

/// Settle a caller-signed x402 payment of `value_wei` `$LH` to `target`'s
/// token-bound account — the `call --pay` tail: the demand-side "paid
/// agent-to-agent service" primitive (the headless sibling of the browser's
/// x402 flow, and of `mcp-call` where the PROXY settles). Sign the
/// `PaymentAuthorization` with the caller key, ensure the diamond's `$LH`
/// allowance, submit the sponsored `settle`. Returns the process exit code.
async fn settle_call_payment(key_hex: &str, target: &str, value_wei: u128) -> i32 {
    let signer = match wallet::from_private_key_hex(key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("--pay: bad identity key: {e}");
            return 1;
        }
    };
    let from = wallet::address(&signer);
    let from_hex = bytes_to_hex_str(&from);

    // The payee is the target's on-chain TBA (same rule as mcp-call / the
    // browser: payment goes to the agent's registered account, nowhere else).
    let to_hex = match registry::tba_of_name(target).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            eprintln!("--pay: '{target}' has no token-bound account to receive payment");
            return 1;
        }
        Err(e) => {
            eprintln!("--pay: RPC error resolving {target}: {e}");
            return 1;
        }
    };
    let to = match parse_address(&to_hex) {
        Ok(b) => b,
        Err(_) => {
            eprintln!("internal: bad TBA address for {target}: {to_hex}");
            return 1;
        }
    };

    if let Err(code) = ensure_diamond_allowance(&signer, &from_hex, value_wei).await {
        return code;
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let valid_before = now + 3600;
    let nonce = registry::random_x402_nonce();
    let signature = match registry::sign_x402(
        &signer,
        &from,
        &to,
        value_wei,
        0,
        valid_before,
        &nonce,
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("--pay: could not sign x402 authorization: {e}");
            return 1;
        }
    };
    let sponsor = match load_sponsor() {
        Ok(s) => s,
        Err(code) => return code,
    };
    match registry::settle_x402_sponsored(
        &signer,
        &sponsor,
        &from,
        &to,
        value_wei,
        0,
        valid_before,
        &nonce,
        &signature,
        registry::ALPHA_USD_ADDRESS(),
    )
    .await
    {
        Ok(tx) => {
            // `fmt_lh` already carries the " LH" suffix — print the RESOLVED
            // amount, never the raw `--pay auto` flag string.
            println!("paid {} to {target}'s account {to_hex} (tx {tx})", fmt_lh(value_wei));
            0
        }
        Err(e) => {
            report_call_error("--pay settlement failed", &e);
            1
        }
    }
}

/// Run ONE headless conversational turn as `target` — embodying its on-chain
/// persona, paid for by the identity behind `key_hex` (proxy auth + ~1 $LH
/// debited from its per-request meter, which this funds lazily — NOT an hourly
/// session). Returns the reply text plus the updated conversation history bytes
/// (to persist for the next turn). Shared by the CLI `call` command and the
/// `mcp` server's `call_agent` tool, so both reach an agent identically.
/// Best-effort lazy meter funding shared by `call` / `run_agent_turn` /
/// `probe`: when the caller's per-request meter is below one call's cost,
/// deposit [`CALL_METER_TOPUP_WEI`] from their own wallet (sponsored gas).
/// One retry on the known-transient Tempo RPC flake, and a WARN (not
/// silence) on final failure — a silently-skipped deposit surfaces minutes
/// later as an unexplained 402 (seen live: colony judges quietly dropping
/// out of the panel). Still best-effort: an unfunded wallet stays unfunded.
pub(crate) async fn ensure_meter_funded(caller: &k256::ecdsa::SigningKey) {
    let Ok(key) = sponsor_key() else {
        return;
    };
    let Ok(sponsor) = wallet::from_private_key_hex(&key) else {
        return;
    };
    let addr = bytes_to_hex_str(&wallet::address(caller));
    if registry::credit_balance_of(&addr).await.unwrap_or(0) >= CALL_COST_WEI {
        return;
    }
    let deposit = || {
        registry::deposit_credits_sponsored(
            caller,
            &sponsor,
            CALL_METER_TOPUP_WEI,
            registry::ALPHA_USD_ADDRESS(),
        )
    };
    match deposit().await {
        Ok(_) => {}
        Err(e) if crate::colony::is_transient_rpc_error(&e) => {
            if let Err(e2) = deposit().await {
                eprintln!("warning: meter top-up failed twice ({e2}) — the call may 402");
            }
        }
        Err(e) => {
            eprintln!("warning: meter top-up failed ({e}) — the call may 402");
        }
    }
}

/// The `X-PAYMENT` request header name the proxy reads for an x402 per-call
/// authorization (also accepts `x-x402-authorization`; case-insensitive).
const X402_PAYMENT_HEADER: &str = "X-PAYMENT";

/// The provider + model id the proxy `/prices` table is keyed by: a `claude-*`
/// id routes to `anthropic`, anything else to `gemini` (the only providers the
/// CLI `call` uses). Returns `(provider, model_id)` — model `""` for Gemini,
/// whose table row is the single `*`. Pure.
fn provider_and_model(model: Option<&str>) -> (&'static str, &str) {
    match model {
        Some(m) if m.starts_with("claude") => ("anthropic", m),
        _ => ("gemini", ""),
    }
}

/// Resolve a model's price (in `$LH` wei) from the proxy `/prices` `prices[]`
/// array: an exact `(provider, model)` row, else the provider's `*` fallback
/// row. `None` when neither is present. Pure (testable). Gemini always matches
/// its `*` row (it passes `model == "*"`-equivalent `""`, so only the fallback
/// arm fires).
fn price_wei_for_model(prices: &serde_json::Value, provider: &str, model: &str) -> Option<u128> {
    let rows = prices.as_array()?;
    let lookup = |want_model: &str| -> Option<u128> {
        rows.iter().find_map(|r| {
            if r.get("provider")?.as_str()? == provider && r.get("model")?.as_str()? == want_model {
                r.get("price_wei")?.as_str()?.parse::<u128>().ok()
            } else {
                None
            }
        })
    };
    lookup(model).or_else(|| lookup("*"))
}

/// Proactively build an `X-PAYMENT` x402 header (name, JSON value) for a metered
/// call, or `None` to use the existing meter/session path. Takes the x402 path
/// ONLY when the proxy advertises a payee (`/prices`.x402.payTo) AND the caller's
/// wallet covers the chosen model's price. Signs an authorization to the payee
/// for exactly the price, ensuring the diamond allowance so `settle`'s
/// `transferFrom` works. Best-effort: every failure returns `None` (fall back).
async fn try_build_x402_payment(
    caller: &k256::ecdsa::SigningKey,
    model: Option<&str>,
) -> Option<(String, String)> {
    let base = registry::CREDIT_PROXY_URL.trim_end_matches('/');
    let prices: serde_json::Value = reqwest::Client::new()
        .get(format!("{base}/prices"))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    // x402 metering must be ON (payTo non-null), else use the meter path.
    let payee_hex = prices.get("x402")?.get("payTo")?.as_str()?.to_string();
    let payee = parse_address(&payee_hex).ok()?;

    let (provider, model_id) = provider_and_model(model);
    let cost = price_wei_for_model(prices.get("prices")?, provider, model_id)?;

    let from = wallet::address(caller);
    let from_hex = bytes_to_hex_str(&from);
    // Only pay per-call if the WALLET can cover the price (else fall back to the
    // meter, which auto-bridges from unspent credits).
    if registry::token_balance_of(&from_hex).await.unwrap_or(0) < cost {
        return None;
    }
    // settle pulls via the diamond's transferFrom → ensure the one-time approve.
    if ensure_diamond_allowance(caller, &from_hex, cost).await.is_err() {
        return None;
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let valid_before = now + 300;
    let nonce = registry::random_x402_nonce();
    let sig = registry::sign_x402(caller, &from, &payee, cost, 0, valid_before, &nonce).ok()?;
    let auth = registry::x402_authorization_json(
        &from_hex,
        &payee_hex,
        cost,
        0,
        valid_before,
        &nonce,
        &sig,
    );
    // eprintln (NOT println): `run_agent_turn` is shared with the MCP server,
    // whose stdout IS the JSON-RPC channel — a stray stdout line corrupts it.
    // Say truthfully where the money comes from: this x402 authorization is
    // settled via `X402Facet.settle` — a transferFrom pulling from the caller's
    // WALLET to the platform payee. The chat METER is NOT debited on this path
    // (dogfood: the old "to the platform meter" wording sent users checking an
    // unchanged meter while their wallet dropped).
    eprintln!(
        "x402: paying {} per call from your WALLET (x402 settle to the platform payee; the meter is untouched)",
        fmt_lh(cost)
    );
    Some((X402_PAYMENT_HEADER.to_string(), auth.to_string()))
}

pub(crate) async fn run_agent_turn(
    key_hex: &str,
    target: &str,
    message: &str,
    prior_history: Option<Vec<u8>>,
    model: Option<&str>,
) -> Result<(String, Option<Vec<u8>>), String> {
    let caller =
        wallet::from_private_key_hex(key_hex).map_err(|e| format!("bad identity key: {e}"))?;

    // Embody the target's PUBLISHED persona (falls back to a generic prompt).
    let target_id = match registry::id_of_name(target).await {
        Ok(id) if id != 0 => id,
        Ok(_) => return Err(format!("{target} is not a registered agent")),
        Err(e) => return Err(format!("RPC error: {e}")),
    };
    let mut system = match registry::persona_of(target_id).await {
        Ok(Some(p)) => p,
        Ok(None) => default_persona(target),
        Err(e) => return Err(format!("RPC error reading persona: {e}")),
    };
    // Ground the agent in the ACTUAL runtime chain. Unlike the browser session
    // (which injects the RUNTIME_SUMMARY digest), the headless `call`/`abtest`
    // system prompt was persona-only, so asked what chain it runs on an agent
    // would HALLUCINATE (fleet repro: it answered "Arbitrum"). Derived from the
    // active ChainConfig so it's correct on BOTH mainnet and testnet — never
    // hardcode a chain here (that mismatch is the recurring footgun).
    let chain = registry::chain::active();
    system = format!(
        "You run on the localharness platform — a self-sovereign agent platform — on \
         {} (EVM chain id {}). Its credit token is $LH and all payments and state \
         settle on this chain; never claim to run on any other blockchain.\n\n{system}",
        chain.name, chain.chain_id
    );
    // Fold in the target's self-recorded lessons so a headless call embodies
    // the same learned behavior as its in-tab sessions. Best-effort: an RPC
    // failure degrades to no lessons rather than failing the call.
    if let Ok(Some(lessons)) = registry::lessons_of(target_id).await {
        if let Some(section) = localharness::lessons::compose_section(&lessons) {
            system = format!("{system}\n\n{section}");
        }
    }
    // Fold in the target's self-defined skills the SAME way, so a headless call
    // can invoke the same named skills the agent uses in-tab. Best-effort.
    if let Ok(Some(skills)) = registry::skills_of(target_id).await {
        if let Some(section) = localharness::skills::compose_section(&skills) {
            system = format!("{system}\n\n{section}");
        }
    }
    // Inject the target's advertised x402 price so the agent never misreports
    // itself as free/sponsored to a PAYING caller (fleet repro: an mcp-call
    // settled 0.05 $LH while the agent answered "my price is 0"). Best-effort:
    // a read failure falls back to the platform default (the enforced floor).
    let price_wei = registry::x402_ask_price_of(target_id)
        .await
        .unwrap_or(registry::DEFAULT_ASK_PRICE_WEI);
    system = format!(
        "{system}\n\nYour advertised price is {} per paid call; callers may have paid it. Never claim to be free or sponsored.",
        fmt_lh(price_wei)
    );

    // PAY-PER-CALL: when the proxy advertises x402 metering (`/prices`.x402.payTo
    // non-null) AND the caller's wallet covers the chosen model's price, sign an
    // x402 authorization to the platform meter payee and carry it as `X-PAYMENT`
    // — the proxy serves + settles on-chain and does NOT touch the creditOf
    // meter, so we skip the lazy meter top-up. Best-effort: any failure (off,
    // unfunded, RPC) falls back UNCHANGED to the meter path below.
    //
    // INVARIANT (load-bearing): the X-PAYMENT carries a ONE-SHOT nonce, valid for
    // exactly ONE request. This turn fires exactly one upstream request (`caps`
    // below disables builtins + subagents, no compaction is set), so the header
    // (attached at the connection level) is never replayed. Do NOT enable
    // compaction / subagents / image on an x402-bearing turn — a second request
    // would replay the spent nonce and 402 mid-turn with no meter fallback. If
    // ever needed, scope X-PAYMENT per request (fresh nonce each) instead.
    let x402_header = try_build_x402_payment(&caller, model).await;
    if x402_header.is_none() {
        // Pay PER REQUEST via the meter: fund it so the proxy debits ~CALL_COST_WEI
        // per call. A one-shot agent call must NOT buy a 10-$LH hour-long session
        // (the old behavior). Best-effort + sponsored; an unfunded wallet stays
        // unfunded (the proxy 402s, the hint says to redeem).
        ensure_meter_funded(&caller).await;
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let token = registry::proxy_auth_token(&caller, now, "gemini");
    let base = url::Url::parse(registry::CREDIT_PROXY_URL)
        .map_err(|e| format!("internal: bad proxy url: {e}"))?;

    // A pure conversational turn: no local builtins (a remote prompt must not
    // read the CALLER's filesystem), no subagents.
    let caps = localharness::types::CapabilitiesConfig {
        enabled_tools: Some(Vec::new()),
        enable_subagents: false,
        ..Default::default()
    };
    // Route by model: a `claude-*` id uses the Anthropic backend; anything else
    // (or none) uses Gemini. BOTH reach the model the same way — through the
    // credit proxy with the same signed token — so a subsidized identity calls
    // either provider with no provider key of its own. Only the config build +
    // start call differ per backend; the start-with-history-fallback and the
    // chat/harvest/shutdown tail are shared (`start_with_history_fallback` /
    // `run_turn_and_persist`).
    if model.map(|m| m.starts_with("claude")).unwrap_or(false) {
        #[cfg(feature = "anthropic")]
        {
            let model = model.unwrap().to_string();
            // Build a config, optionally seeded with prior history. Cloned inputs
            // so a failed history-seeded start can be retried from scratch.
            let build = |history: Option<Vec<u8>>| {
                let mut cfg = localharness::AnthropicAgentConfig::new(token.clone())
                    .with_base_url(base.clone())
                    .with_model(model.clone())
                    .with_system_instructions(system.clone())
                    .with_capabilities(caps.clone());
                if let Some((name, value)) = x402_header.clone() {
                    cfg = cfg.with_extra_header(name, value);
                }
                if let Some(bytes) = history {
                    cfg = cfg.with_history_bytes(bytes);
                }
                cfg
            };
            let agent =
                start_with_history_fallback(target, prior_history, "anthropic session", |h| {
                    localharness::Agent::start_anthropic(build(h))
                })
                .await?;
            return run_turn_and_persist(agent, message).await;
        }
        #[cfg(not(feature = "anthropic"))]
        {
            return Err("Claude models require a build with `--features anthropic`".to_string());
        }
    }

    let build = |history: Option<Vec<u8>>| {
        let mut cfg = localharness::GeminiAgentConfig::new(token.clone())
            .with_base_url(base.clone())
            .with_system_instructions(system.clone())
            .with_capabilities(caps.clone());
        if let Some((name, value)) = x402_header.clone() {
            cfg = cfg.with_extra_header(name, value);
        }
        if let Some(bytes) = history {
            cfg = cfg.with_history_bytes(bytes);
        }
        cfg
    };
    let agent = start_with_history_fallback(target, prior_history, "agent session", |h| {
        localharness::Agent::start_gemini(build(h))
    })
    .await?;
    run_turn_and_persist(agent, message).await
}

/// Start an agent seeded with `prior_history`, falling back to a FRESH start
/// (with a warning) when the saved thread is incompatible/corrupt — rather than
/// failing the whole call. `label` names the backend in the could-not-start
/// error ("anthropic session" / "agent session"); `start` runs the backend's
/// actual start call on a given history. The shared start-half of both
/// `run_agent_turn` branches.
async fn start_with_history_fallback<F, Fut>(
    target: &str,
    prior_history: Option<Vec<u8>>,
    label: &str,
    start: F,
) -> Result<localharness::Agent, String>
where
    F: Fn(Option<Vec<u8>>) -> Fut,
    Fut: std::future::Future<Output = localharness::Result<localharness::Agent>>,
{
    match start(prior_history.clone()).await {
        Ok(a) => Ok(a),
        Err(_) if prior_history.is_some() => {
            // Incompatible/corrupt saved thread → warn + start fresh rather than
            // failing the whole call.
            eprintln!(
                "warning: could not load saved conversation with {target} \
                 (incompatible or corrupt) — starting a fresh thread"
            );
            start(None)
                .await
                .map_err(|e| format!("could not start {label}: {e}"))
        }
        Err(e) => Err(format!("could not start {label}: {e}")),
    }
}

/// Run ONE chat turn on a started agent, harvest the reply text + the updated
/// conversation-history bytes, and shut the agent down. The shared tail of both
/// `run_agent_turn` branches.
async fn run_turn_and_persist(
    agent: localharness::Agent,
    message: &str,
) -> Result<(String, Option<Vec<u8>>), String> {
    let reply = match agent.chat(message).await {
        Ok(resp) => resp.text().await.map_err(|e| format!("response error: {e}")),
        Err(e) => Err(e.to_string()),
    };
    let new_history = agent.history_bytes().ok().flatten();
    let _ = agent.shutdown().await;
    reply.map(|text| (text, new_history))
}

/// List the caller's saved conversation threads (`localharness threads`).
pub(crate) fn threads(caller_name: Option<&str>) -> i32 {
    let label = match resolve_caller_label(caller_name) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let now = std::time::SystemTime::now();
    // (target, age_secs_since_last_active). The conversation history is a
    // backend-specific serialized blob (not cheaply parseable to a text
    // preview), so we surface the file's mtime as a relative "last active" —
    // far more useful than the raw byte count (on-chain feedback #93/#95).
    let mut found: Vec<(String, Option<u64>)> = match std::fs::read_dir(history_dir()) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let target = thread_file_target(&label, &name)?;
                let age = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| now.duration_since(t).ok())
                    .map(|d| d.as_secs());
                Some((target, age))
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    found.sort();
    if found.is_empty() {
        println!("no saved conversations for {label}");
        return 0;
    }
    println!("conversations for {label}:");
    for (target, age) in found {
        match age {
            Some(secs) => println!("  {target}  (last active {} ago)", fmt_duration(secs)),
            None => println!("  {target}"),
        }
    }
    0
}

/// Delete a saved conversation thread, or all of the caller's with `--all`
/// (`localharness forget [--as me] <target|--all>`). Never touches identity
/// keys or on-chain state — only local history files.
pub(crate) fn forget(caller_name: Option<&str>, target: &str) -> i32 {
    let label = match resolve_caller_label(caller_name) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    if target == "--all" {
        let mut n = 0;
        if let Ok(rd) = std::fs::read_dir(history_dir()) {
            for e in rd.flatten() {
                let Ok(name) = e.file_name().into_string() else {
                    continue;
                };
                if thread_file_target(&label, &name).is_some()
                    && std::fs::remove_file(e.path()).is_ok()
                {
                    n += 1;
                }
            }
        }
        println!("forgot {n} conversation(s) for {label}");
        return 0;
    }
    // A target can have a thread per backend (plus a legacy untagged file);
    // forget them all so `forget <target>` clears the conversation regardless
    // of which model it ran under.
    let mut removed = false;
    for candidate in [
        history_path(&label, target, "gemini"),
        history_path(&label, target, "anthropic"),
        history_dir().join(format!("{label}__{target}.bin")), // legacy untagged
    ] {
        if std::fs::remove_file(candidate).is_ok() {
            removed = true;
        }
    }
    if removed {
        println!("forgot conversation with {target}");
    } else {
        println!("no saved conversation with {target}");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;

    #[test]
    fn parse_call_plain_target_and_message() {
        let p = parse_call_args(&args(&["alice", "how", "are", "you"])).unwrap();
        assert_eq!(p.caller, None);
        assert_eq!(p.target, "alice");
        assert_eq!(p.message, "how are you");
    }

    #[test]
    fn parse_call_single_word_message() {
        let p = parse_call_args(&args(&["alice", "hello"])).unwrap();
        assert_eq!(p.caller, None);
        assert_eq!(p.target, "alice");
        assert_eq!(p.message, "hello");
    }

    #[test]
    fn parse_call_with_as_flag() {
        let p = parse_call_args(&args(&["--as", "bob", "alice", "what's", "up"])).unwrap();
        assert_eq!(p.caller.as_deref(), Some("bob"));
        assert!(!p.fresh);
        assert_eq!(p.target, "alice");
        assert_eq!(p.message, "what's up");
    }

    #[test]
    fn parse_call_fresh_flag() {
        let p = parse_call_args(&args(&["--fresh", "alice", "hi"])).unwrap();
        assert!(p.fresh);
        assert_eq!(p.caller, None);
        assert_eq!(p.target, "alice");
        assert_eq!(p.message, "hi");
    }

    #[test]
    fn parse_call_flags_order_independent() {
        let a = parse_call_args(&args(&["--as", "bob", "--fresh", "alice", "hi"])).unwrap();
        let b = parse_call_args(&args(&["--fresh", "--as", "bob", "alice", "hi"])).unwrap();
        for p in [a, b] {
            assert_eq!(p.caller.as_deref(), Some("bob"));
            assert!(p.fresh);
            assert_eq!(p.target, "alice");
            assert_eq!(p.message, "hi");
        }
    }

    #[test]
    fn parse_call_rejects_a_leading_flag_placed_after_the_target() {
        // The silent-swallow footgun (found dogfooding --pay): a leading flag placed
        // AFTER the target would be joined into the message and ignored — silently
        // dropping a PAYMENT. Error clearly instead of paying nothing.
        for parts in [
            vec!["alice", "hi", "--pay", "auto"],
            vec!["--as", "bob", "alice", "hi", "--pay", "0.01"],
            vec!["alice", "hi", "--model", "claude-opus"],
        ] {
            match parse_call_args(&args(&parts)) {
                Err(e) => assert!(e.contains("must come BEFORE the target"), "got: {e}"),
                Ok(_) => panic!("expected an error for a leading flag placed after the target: {parts:?}"),
            }
        }
        // A LEGIT leading --pay still parses (the working path stays intact).
        let p = parse_call_args(&args(&["--as", "bob", "--pay", "auto", "alice", "hi"])).unwrap();
        assert_eq!(p.pay.as_deref(), Some("auto"));
        assert_eq!(p.message, "hi");
    }

    #[test]
    fn parse_call_accepts_model_flag_in_any_order() {
        // `--as`/`--model`/`--fresh` may appear in any order before the target.
        let perms = [
            vec!["--model", "claude-opus", "--as", "bob", "--fresh", "alice", "hi"],
            vec!["--fresh", "--model", "claude-opus", "--as", "bob", "alice", "hi"],
            vec!["--as", "bob", "--model", "claude-opus", "--fresh", "alice", "hi"],
        ];
        for parts in perms {
            let p = parse_call_args(&args(&parts)).unwrap();
            assert_eq!(p.caller.as_deref(), Some("bob"));
            assert_eq!(p.model.as_deref(), Some("claude-opus"));
            assert!(p.fresh);
            assert_eq!(p.target, "alice");
            assert_eq!(p.message, "hi");
        }
        // `--model` requires a value.
        assert!(parse_call_args(&args(&["--model"])).is_err());
    }

    #[test]
    fn parse_call_defaults_to_not_fresh() {
        let p = parse_call_args(&args(&["alice", "hi"])).unwrap();
        assert!(!p.fresh);
    }

    #[test]
    fn parse_call_pay_flag() {
        // No --pay → None (no settlement attempted).
        let p = parse_call_args(&args(&["alice", "hi"])).unwrap();
        assert_eq!(p.pay, None);
        // --pay before the target, in any order with the other flags.
        for parts in [
            vec!["--pay", "0.05", "alice", "hi"],
            vec!["--fresh", "--pay", "0.05", "alice", "hi"],
            vec!["--pay", "0.05", "--fresh", "alice", "hi"],
        ] {
            let p = parse_call_args(&args(&parts)).unwrap();
            assert_eq!(p.pay.as_deref(), Some("0.05"));
            assert_eq!(p.target, "alice");
            assert_eq!(p.message, "hi");
        }
        // `--pay` requires a value.
        assert!(parse_call_args(&args(&["--pay"])).is_err());
    }

    #[test]
    fn parse_call_verify_flag() {
        // No --verify → None (no escrow gate).
        let p = parse_call_args(&args(&["alice", "hi"])).unwrap();
        assert_eq!(p.verify, None);
        // --verify before the target, in any order with the other flags, and the
        // comma-separated keys are split + trimmed.
        for parts in [
            vec!["--verify", "answer,score", "alice", "hi"],
            vec!["--pay", "0.05", "--verify", "answer, score", "alice", "hi"],
            vec!["--verify", "answer ,score", "--pay", "auto", "alice", "hi"],
        ] {
            let p = parse_call_args(&args(&parts)).unwrap();
            assert_eq!(
                p.verify.as_deref(),
                Some(["answer".to_string(), "score".to_string()].as_slice())
            );
            assert_eq!(p.target, "alice");
            assert_eq!(p.message, "hi");
        }
        // `--verify` requires a value.
        assert!(parse_call_args(&args(&["--verify"])).is_err());
    }

    #[test]
    fn parse_verify_keys_splits_trims_and_drops_blanks() {
        assert_eq!(parse_verify_keys("a,b,c"), vec!["a", "b", "c"]);
        // Whitespace around each key is trimmed; blank entries (incl. a trailing
        // comma) are dropped.
        assert_eq!(parse_verify_keys(" a , b ,, c,"), vec!["a", "b", "c"]);
        // A single key, no commas.
        assert_eq!(parse_verify_keys("answer"), vec!["answer"]);
        // All-blank → empty (vacuously satisfied by any JSON object).
        assert!(parse_verify_keys(" , , ").is_empty());
    }

    #[test]
    fn verify_reply_accepts_object_with_all_keys() {
        let required = vec!["answer".to_string(), "score".to_string()];
        assert!(verify_reply(r#"{"answer":"yes","score":9}"#, &required).is_ok());
        // Surrounding whitespace is tolerated (trimmed before parse).
        assert!(verify_reply("  {\"answer\":1,\"score\":2}  \n", &required).is_ok());
        // Extra keys are fine — only the required ones must be present.
        assert!(verify_reply(r#"{"answer":1,"score":2,"extra":true}"#, &required).is_ok());
        // No required keys → any object passes.
        assert!(verify_reply(r#"{"x":1}"#, &[]).is_ok());
        // Markdown-fenced JSON (the common LLM output) must NOT falsely withhold.
        assert!(verify_reply("```json\n{\"answer\":1,\"score\":2}\n```", &required).is_ok());
        assert!(verify_reply("```\n{\"answer\":1,\"score\":2}\n```", &required).is_ok());
        // JSON surrounded by prose is extracted (first '{' .. last '}').
        assert!(verify_reply("Here you go: {\"answer\":1,\"score\":2}. Done!", &required).is_ok());
    }

    #[test]
    fn verify_reply_rejects_missing_key_non_object_and_non_json() {
        let required = vec!["answer".to_string(), "score".to_string()];
        // A present-but-incomplete object names the FIRST missing key.
        let err = verify_reply(r#"{"answer":"yes"}"#, &required).unwrap_err();
        assert!(err.contains("missing key 'score'"), "got: {err}");
        // Valid JSON but not an object (array / scalar) → not an object.
        assert!(verify_reply(r#"["answer","score"]"#, &required)
            .unwrap_err()
            .contains("not a JSON object"));
        assert!(verify_reply("42", &required)
            .unwrap_err()
            .contains("not a JSON object"));
        // Not JSON at all → not JSON (e.g. a prose reply that ignored the schema).
        assert!(verify_reply("the answer is yes", &required)
            .unwrap_err()
            .contains("not JSON"));
    }

    #[test]
    fn parse_call_message_preserves_internal_spacing_as_single_spaces() {
        // join(" ") normalises arg boundaries to single spaces — documents the
        // contract so a caller relying on exact whitespace isn't surprised.
        let p = parse_call_args(&args(&["alice", "a", "b", "c"])).unwrap();
        assert_eq!(p.message, "a b c");
    }

    #[test]
    fn parse_call_rejects_missing_message() {
        assert!(parse_call_args(&args(&["alice"])).is_err());
    }

    // ---- mcp-call (the hosted MCP-over-HTTP + x402 client) ----------------

    #[test]
    fn parse_call_rejects_empty() {
        assert!(parse_call_args(&args(&[])).is_err());
    }

    #[test]
    fn parse_call_rejects_as_without_name() {
        assert!(parse_call_args(&args(&["--as"])).is_err());
    }

    #[test]
    fn parse_call_rejects_as_name_without_target_or_message() {
        // `--as bob` alone: caller set, but no target/message → usage error.
        assert!(parse_call_args(&args(&["--as", "bob"])).is_err());
        // `--as bob alice` : target but no message → usage error.
        assert!(parse_call_args(&args(&["--as", "bob", "alice"])).is_err());
    }

    #[test]
    fn thread_file_target_parses_own_files_only() {
        // Backend-tagged files (current format): the tag is stripped.
        assert_eq!(
            thread_file_target("claude", "claude__alice.gemini.bin").as_deref(),
            Some("alice")
        );
        assert_eq!(
            thread_file_target("claude", "claude__alice.anthropic.bin").as_deref(),
            Some("alice")
        );
        // Legacy untagged files still parse (backward compatibility).
        assert_eq!(
            thread_file_target("claude", "claude__alice.bin").as_deref(),
            Some("alice")
        );
        // A target containing the separator stays intact (strip_prefix once).
        assert_eq!(
            thread_file_target("claude", "claude__a__b.gemini.bin").as_deref(),
            Some("a__b")
        );
        // Different caller → not ours.
        assert_eq!(thread_file_target("claude", "bob__alice.gemini.bin"), None);
        // Wrong extension, or empty target → rejected.
        assert_eq!(thread_file_target("claude", "claude__alice.txt"), None);
        assert_eq!(thread_file_target("claude", "claude__.bin"), None);
        assert_eq!(thread_file_target("claude", "claude__.gemini.bin"), None);
        assert_eq!(thread_file_target("claude", "unrelated.bin"), None);
    }

    #[test]
    fn thread_file_target_roundtrips_history_path() {
        // The parser must invert the filename half of history_path for both
        // backends.
        for backend in ["gemini", "anthropic"] {
            let p = history_path("claude", "alice", backend);
            let name = p.file_name().unwrap().to_str().unwrap();
            assert_eq!(thread_file_target("claude", name).as_deref(), Some("alice"));
        }
    }

    #[test]
    fn model_backend_tag_routes_claude_to_anthropic() {
        assert_eq!(model_backend_tag(Some("claude-opus-4")), "anthropic");
        assert_eq!(model_backend_tag(Some("claude")), "anthropic");
        assert_eq!(model_backend_tag(Some("gemini-3.5-flash")), "gemini");
        assert_eq!(model_backend_tag(None), "gemini");
    }

    #[test]
    fn history_path_keys_on_backend_so_formats_never_collide() {
        // The cross-backend bug: a Gemini thread and an Anthropic thread to the
        // same target must live in SEPARATE files (incompatible on-disk shapes).
        let g = history_path("claude", "alice", "gemini");
        let a = history_path("claude", "alice", "anthropic");
        assert_ne!(g, a, "backends must not share a history file");
        assert!(g.ends_with("claude__alice.gemini.bin"));
        assert!(a.ends_with("claude__alice.anthropic.bin"));
    }

    #[test]
    fn history_path_keys_on_caller_and_target() {
        let p = history_path("claude", "alice", "gemini");
        assert!(p.ends_with("claude__alice.gemini.bin"));
        // Distinct caller or target → distinct file (no cross-thread bleed).
        assert_ne!(
            history_path("claude", "alice", "gemini"),
            history_path("bob", "alice", "gemini")
        );
        assert_ne!(
            history_path("claude", "alice", "gemini"),
            history_path("claude", "bob", "gemini")
        );
        // Lives under a hidden dir so it doesn't clutter the working tree.
        assert!(p.starts_with(".localharness"));
    }

    #[test]
    fn hint_for_call_error_classifies_common_failures() {
        // Payment / session / credits → the $LH hint.
        for s in [
            "HTTP 402 Payment Required",
            "proxy: no session for 0xabc",
            "insufficient credit",
        ] {
            assert!(
                hint_for_call_error(s).unwrap().contains("$LH"),
                "expected $LH hint for {s:?}"
            );
        }
        // Auth → the signature hint.
        for s in ["401 Unauthorized", "bad signature", "403 Forbidden"] {
            assert!(
                hint_for_call_error(s).unwrap().contains("signature"),
                "expected auth hint for {s:?}"
            );
        }
        // Rate limit.
        assert!(hint_for_call_error("429 Too Many Requests")
            .unwrap()
            .contains("rate limited"));
    }

    #[test]
    fn provider_and_model_routes_claude_to_anthropic_else_gemini() {
        assert_eq!(provider_and_model(Some("claude-opus-4-8")), ("anthropic", "claude-opus-4-8"));
        // Gemini always queries the single `*` row → empty model id.
        assert_eq!(provider_and_model(Some("gemini-3.5-flash")), ("gemini", ""));
        assert_eq!(provider_and_model(None), ("gemini", ""));
    }

    #[test]
    fn price_wei_for_model_matches_exact_then_falls_back_to_star() {
        let prices = serde_json::json!([
            { "provider": "gemini", "model": "*", "price_wei": "10000000000000000" },
            { "provider": "anthropic", "model": "claude-opus-4-8", "price_wei": "200000000000000000" },
            { "provider": "anthropic", "model": "*", "price_wei": "50000000000000000" },
        ]);
        // Gemini → its `*` row.
        assert_eq!(price_wei_for_model(&prices, "gemini", ""), Some(10_000_000_000_000_000));
        // Exact anthropic model row.
        assert_eq!(
            price_wei_for_model(&prices, "anthropic", "claude-opus-4-8"),
            Some(200_000_000_000_000_000)
        );
        // Unlisted anthropic model → provider `*` fallback.
        assert_eq!(
            price_wei_for_model(&prices, "anthropic", "claude-future"),
            Some(50_000_000_000_000_000)
        );
        // A provider with no row at all → None.
        assert_eq!(price_wei_for_model(&prices, "openai", "gpt-5"), None);
        // Non-array prices → None (malformed payload, fall back to meter).
        assert_eq!(price_wei_for_model(&serde_json::json!({}), "gemini", ""), None);
    }

    #[test]
    fn hint_for_call_error_is_case_insensitive_and_silent_on_unknown() {
        assert!(hint_for_call_error("PAYMENT REQUIRED").is_some());
        // An unrecognised error gets no hint (caller still prints the raw text).
        assert_eq!(hint_for_call_error("connection reset by peer"), None);
        assert_eq!(hint_for_call_error("some unrelated parse error"), None);
    }
}
