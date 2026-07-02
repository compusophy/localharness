use crate::{bytes_to_hex_str, ensure_wallet_covers, fmt_lh, non_blank, parse_address, registry, report_call_error, resolve_caller_key, run_agent_turn, take_as_flag, wallet};

// ---- MCP server ----------------------------------------------------------
//
// `localharness mcp` speaks the Model Context Protocol over stdio (newline-
// delimited JSON-RPC 2.0), exposing localharness agents as a TOOL any MCP client
// (Claude Code, Codex, …) can call. The headline tool `call_agent` lets an
// external agent invoke a sovereign `<name>.localharness.xyz` agent under its
// on-chain persona — the demand-side experiment: will anyone actually call these
// agents? The server acts AS the sole identity key in the working directory (it
// signs proxy auth and pays the $LH).

pub(crate) async fn mcp_serve(args: &[String]) -> i32 {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    // The identity that signs proxy auth + pays for outbound calls. `--as <name>`
    // picks it; with a single key in the dir it's inferred.
    let caller = match take_as_flag(args) {
        Ok((caller, _rest)) => caller,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let key_hex = match resolve_caller_key(caller.as_deref()) {
        Ok((_file, hex)) => hex,
        Err(e) => {
            eprintln!("mcp: no usable identity ({e}). Pass --as <name> or run `localharness create <name>` first.");
            return 2;
        }
    };

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut out = tokio::io::stdout();
    eprintln!("localharness mcp: ready on stdio (acting as the local identity).");

    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break, // stdin EOF — the client closed the pipe
            Err(e) => {
                // A single bad read (e.g. a non-UTF-8 byte on stdin) must not
                // tear down the whole server — log and keep serving.
                eprintln!("localharness mcp: skipping unreadable stdin frame: {e}");
                continue;
            }
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let req: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue, // ignore malformed frames
        };
        // Notifications (no `id`, e.g. notifications/initialized) get no reply.
        let Some(id) = req.get("id").cloned() else { continue };
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");

        let envelope = match mcp_handle(method, &req, &key_hex).await {
            Ok(result) => serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, msg)) => {
                serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": msg}})
            }
        };
        if out.write_all(format!("{envelope}\n").as_bytes()).await.is_err() {
            break;
        }
        let _ = out.flush().await;
    }
    0
}

pub(crate) async fn mcp_handle(
    method: &str,
    req: &serde_json::Value,
    key_hex: &str,
) -> Result<serde_json::Value, (i64, String)> {
    match method {
        "initialize" => Ok(serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "localharness", "version": env!("CARGO_PKG_VERSION") }
        })),
        "tools/list" => Ok(serde_json::json!({ "tools": mcp_tool_list() })),
        "tools/call" => {
            let params = req.get("params").cloned().unwrap_or_default();
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or_default();
            mcp_tool_call(name, &args, key_hex).await
        }
        "ping" => Ok(serde_json::json!({})),
        other => Err((-32601, format!("method not found: {other}"))),
    }
}

pub(crate) fn mcp_tool_list() -> serde_json::Value {
    serde_json::json!([
        {
            "name": "call_agent",
            "description": "Send a message to a sovereign localharness agent (a <name>.localharness.xyz NFT) and get its reply. The agent answers under its published on-chain persona; this server's configured identity pays in $LH credits.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "the agent's registered name / subdomain, e.g. \"claude\"" },
                    "message": { "type": "string", "description": "the message to send the agent" }
                },
                "required": ["name", "message"]
            }
        }
    ])
}

pub(crate) async fn mcp_tool_call(
    name: &str,
    args: &serde_json::Value,
    key_hex: &str,
) -> Result<serde_json::Value, (i64, String)> {
    match name {
        "call_agent" => {
            let target = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
            let message = args.get("message").and_then(|v| v.as_str()).unwrap_or("");
            if target.is_empty() || message.trim().is_empty() {
                return Ok(mcp_text_result("call_agent requires both 'name' and 'message'", true));
            }
            // Stateless per MCP request for v1 (no persisted thread).
            // INVARIANT: `run_agent_turn` must NEVER write to stdout — this MCP
            // server's stdout IS the JSON-RPC channel (responses go through
            // `out.write_all` in mcp_serve; all status uses eprintln!). A stray
            // println! anywhere in the turn path would silently corrupt the
            // protocol frame (QA fleet dex-qa flagged this DX trap). Keep turn
            // output as a RETURN VALUE; never print it here.
            match run_agent_turn(key_hex, target, message, None, None).await {
                Ok((text, _hist)) => Ok(mcp_text_result(text.trim(), false)),
                Err(e) => Ok(mcp_text_result(&format!("call_agent failed: {e}"), true)),
            }
        }
        other => Err((-32602, format!("unknown tool: {other}"))),
    }
}

pub(crate) fn mcp_text_result(text: &str, is_error: bool) -> serde_json::Value {
    serde_json::json!({
        "content": [ { "type": "text", "text": text } ],
        "isError": is_error
    })
}

// ---- mcp-call: the HOSTED MCP-over-HTTP + x402 client --------------------
//
// `mcp_serve` (above) is the LOCAL stdio MCP *server*. `mcp_call` is the
// *client* for the REMOTE MCP-over-HTTP endpoint shipped in `proxy/api/mcp.ts`
// (`<proxy>/mcp`). That endpoint gates every `tools/call` behind TRUE x402
// per-call settlement: the caller signs a `PaymentAuthorization` (EIP-712, in
// $LH) paying the TARGET agent's token-bound account, the proxy verifies it
// against the live `x402DomainSeparator()` and runs `X402Facet.settle(...)`
// on-chain BEFORE answering. This command is the round-trip that had no client.

/// Default `--pay` for `mcp-call`: resolve the target's effective price
/// (advertised on-chain, else `registry::DEFAULT_ASK_PRICE_WEI`) — the
/// exact floor the hosted gate enforces, so the default never 402s. The
/// old flat 0.001 default sat BELOW the floor.
pub(crate) const MCP_CALL_DEFAULT_PAY: &str = "auto";

/// Parsed `mcp-call` arguments: optional `--as` caller, optional `--pay`
/// amount (human-typed $LH, e.g. "0.001"), the target agent name, and the
/// joined message. Pure (no I/O) so it is unit-testable; `Err` carries the
/// usage line. Leading flags may appear in any order before the target.
pub(crate) struct ParsedMcpCall {
    caller: Option<String>,
    pay: String,
    target: String,
    message: String,
}

pub(crate) const MCP_CALL_USAGE: &str =
    "usage: localharness mcp-call [--as <yourname>] [--pay <amount>] <target> <message>";

pub(crate) fn parse_mcp_call_args(rest: &[String]) -> Result<ParsedMcpCall, String> {
    // `--as` from ANY position (take_as_flag — consistent with the other
    // commands); --pay stays a leading flag before the target.
    let (caller, rest) = take_as_flag(rest)?;
    let mut pay = MCP_CALL_DEFAULT_PAY.to_string();
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--pay" => match rest.get(i + 1) {
                Some(p) => {
                    pay = p.clone();
                    i += 2;
                }
                None => return Err(MCP_CALL_USAGE.to_string()),
            },
            _ => break,
        }
    }
    match rest[i..].split_first() {
        Some((t, msg)) if !msg.is_empty() => Ok(ParsedMcpCall {
            caller,
            pay,
            target: t.clone(),
            message: msg.join(" "),
        }),
        _ => Err(MCP_CALL_USAGE.to_string()),
    }
}

/// Ensure the diamond can pull at least `value_wei` `$LH` from `from_hex` —
/// x402 `settle` moves the payment via `transferFrom`, so the payer needs
/// (1) a wallet balance covering the price — when short, the AUTO-BRIDGE
/// pulls the shortfall back out of the caller's unspent chat-meter credits
/// via `withdrawCredits` (the shared [`ensure_wallet_covers`] block every
/// escrow submission also runs) — and (2) a one-time `u128::MAX` approve
/// (sponsored when short). Prints its own messages; `Err` carries the
/// process exit code. Read failures only warn — settle is the authoritative
/// gate. Shared by `mcp-call` and `call --pay`.
pub(crate) async fn ensure_diamond_allowance(
    signer: &k256::ecdsa::SigningKey,
    from_hex: &str,
    value_wei: u128,
) -> Result<(), i32> {
    ensure_wallet_covers(signer, from_hex, value_wei).await?;
    match registry::lh_allowance(from_hex, registry::REGISTRY_ADDRESS()).await {
        Ok(allowance) if allowance >= value_wei => Ok(()),
        Ok(_) => {
            println!("approving the diamond to spend $LH (one-time) …");
            match registry::approve_lh_sponsored(signer, registry::REGISTRY_ADDRESS(), u128::MAX)
            .await
            {
                Ok(tx) => {
                    println!("  approved (tx {tx})");
                    Ok(())
                }
                Err(e) => {
                    eprintln!("could not approve $LH spend automatically: {e}");
                    eprintln!(
                        "  fix it once, then retry: approve {} to spend $LH \
                         (token {}) for {from_hex}.",
                        registry::REGISTRY_ADDRESS(),
                        registry::LOCALHARNESS_TOKEN_ADDRESS()
                    );
                    Err(1)
                }
            }
        }
        Err(e) => {
            eprintln!("warning: could not read $LH allowance ({e}); attempting anyway");
            Ok(())
        }
    }
}

/// Client for the hosted MCP-over-HTTP + x402 endpoint (`<proxy>/mcp`). Resolve
/// the caller key + the target's TBA, sign an x402 `$LH` payment to it, ensure
/// the diamond is approved to pull the $LH (auto-approve if short), POST the
/// `tools/call`, and print the agent's reply.
pub(crate) async fn mcp_call(rest: &[String]) -> i32 {
    let ParsedMcpCall {
        caller,
        pay,
        target,
        message,
    } = match parse_mcp_call_args(rest) {
        Ok(p) => p,
        Err(usage) => {
            eprintln!("{usage}");
            return 2;
        }
    };

    // An empty / whitespace-only message used to be accepted (and paid for) —
    // reject it BEFORE any identity/payment work.
    if let Err(e) = non_blank(&message, "mcp-call: message") {
        eprintln!("{e}");
        return 1;
    }

    // The amount to pay, in 18-decimal $LH wei (same parse the bundle uses).
    // "auto" (the default) pays the target's effective price — the number
    // the hosted gate will demand anyway.
    let value_wei = if pay == "auto" {
        match registry::id_of_name(&target).await {
            Ok(id) if id != 0 => match registry::x402_ask_price_of(id).await {
                Ok(wei) => {
                    println!("paying '{target}'s price: {}", fmt_lh(wei));
                    wei
                }
                Err(e) => {
                    eprintln!("price lookup failed: {e}");
                    return 1;
                }
            },
            Ok(_) => {
                eprintln!("'{target}' is not a registered agent");
                return 2;
            }
            Err(e) => {
                eprintln!("RPC error resolving {target}: {e}");
                return 1;
            }
        }
    } else {
        match localharness::encoding::parse_token_amount(&pay) {
            Some(v) if v > 0 => v,
            _ => {
                eprintln!("--pay must be a positive $LH amount (e.g. 0.01) or 'auto', got '{pay}'");
                return 2;
            }
        }
    };

    // 1. Resolve the caller's identity key — it signs the x402 authorization.
    let (_key_file, key_hex) = match resolve_caller_key(caller.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad identity key: {e}");
            return 1;
        }
    };
    let from_bytes = wallet::address(&signer);
    let from_hex = bytes_to_hex_str(&from_bytes);

    // Resolve the payee = the target agent's token-bound account.
    let to_hex = match registry::tba_of_name(&target).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            eprintln!(
                "'{target}' has no token-bound account to receive payment \
                 (is it registered? try `localharness whoami {target}`)"
            );
            return 1;
        }
        Err(e) => {
            eprintln!("RPC error resolving {target}: {e}");
            return 1;
        }
    };
    let to_bytes = match parse_address(&to_hex) {
        Ok(b) => b,
        Err(_) => {
            eprintln!("internal: bad TBA address for {target}: {to_hex}");
            return 1;
        }
    };

    // 2. Build + sign the PaymentAuthorization (EIP-712 over the live x402
    //    domain separator — `registry::sign_x402` does the digest internally).
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let valid_after: u64 = 0;
    let valid_before: u64 = now + 3600; // 1h window
    let nonce = registry::random_x402_nonce();
    let signature = match registry::sign_x402(
        &signer,
        &from_bytes,
        &to_bytes,
        value_wei,
        valid_after,
        valid_before,
        &nonce,
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("could not sign x402 authorization: {e}");
            return 1;
        }
    };

    // 3. ALLOWANCE: `settle` pulls $LH from the payer via the diamond's
    //    `transferFrom`, so the payer must have approved the diamond once.
    if let Err(code) = ensure_diamond_allowance(&signer, &from_hex, value_wei).await {
        return code;
    }

    // 4. POST the tools/call to <proxy>/mcp with the x402 header.
    let header_json = registry::x402_authorization_json(
        &from_hex,
        &to_hex,
        value_wei,
        valid_after,
        valid_before,
        &nonce,
        &signature,
    );
    let body = registry::x402_ask_agent_body(&target, &message);
    let endpoint = registry::mcp_endpoint_url();

    let client = reqwest::Client::new();
    let resp = match client
        .post(&endpoint)
        .header("content-type", "application/json")
        .header("x-x402-authorization", header_json.to_string())
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            report_call_error("mcp-call failed (request)", &e.to_string());
            return 1;
        }
    };
    let json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(e) => {
            eprintln!("mcp-call failed: could not decode JSON-RPC response: {e}");
            return 1;
        }
    };

    // 5. Parse the JSON-RPC envelope (shared with the browser's proxy
    //    fallback — a protocol error or a tool-level `isError` both land in
    //    the Err arm, with the agent's reply text in Ok).
    match registry::parse_mcp_tool_reply(&json) {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(e) => {
            report_call_error("mcp-call failed", &e);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;

    #[test]
    fn parse_mcp_call_defaults_and_flags() {
        // Plain target + message: caller None, default pay.
        let p = parse_mcp_call_args(&args(&["claude", "hi", "there"])).unwrap();
        assert_eq!(p.caller, None);
        assert_eq!(p.pay, MCP_CALL_DEFAULT_PAY);
        assert_eq!(p.target, "claude");
        assert_eq!(p.message, "hi there");

        // Flags in any order before the target.
        for parts in [
            vec!["--as", "bob", "--pay", "0.5", "claude", "yo"],
            vec!["--pay", "0.5", "--as", "bob", "claude", "yo"],
        ] {
            let p = parse_mcp_call_args(&args(&parts)).unwrap();
            assert_eq!(p.caller.as_deref(), Some("bob"));
            assert_eq!(p.pay, "0.5");
            assert_eq!(p.target, "claude");
            assert_eq!(p.message, "yo");
        }
    }

    #[test]
    fn parse_mcp_call_rejects_bad_forms() {
        assert!(parse_mcp_call_args(&args(&[])).is_err()); // empty
        assert!(parse_mcp_call_args(&args(&["claude"])).is_err()); // no message
        assert!(parse_mcp_call_args(&args(&["--as"])).is_err()); // dangling --as
        assert!(parse_mcp_call_args(&args(&["--pay"])).is_err()); // dangling --pay
        assert!(parse_mcp_call_args(&args(&["--pay", "1", "claude"])).is_err()); // no message
    }

    #[test]
    fn mcp_call_pay_parses_to_18_decimal_wei() {
        // The default is the price-resolving sentinel (NOT a parseable
        // amount — `mcp_call` branches on it before parse); explicit human
        // amounts map to the bundle's 18-dec wei.
        assert_eq!(MCP_CALL_DEFAULT_PAY, "auto");
        assert_eq!(
            localharness::encoding::parse_token_amount("0.001"),
            Some(1_000_000_000_000_000)
        );
        assert_eq!(
            localharness::encoding::parse_token_amount("1"),
            Some(1_000_000_000_000_000_000)
        );
    }

    // (The x402 header / tools-call body / reply-envelope shape tests live
    // with the shared builders in `registry::x402` — the browser's proxy
    // fallback uses the same functions.)

    #[test]
    fn mcp_random_nonce_is_32_bytes_and_fresh() {
        let a = registry::random_x402_nonce();
        let b = registry::random_x402_nonce();
        assert_eq!(a.len(), 32);
        assert_eq!(b.len(), 32);
        // Two draws of a CSPRNG should differ (vanishing collision odds).
        assert_ne!(a, b);
    }

}
