use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

use super::*;

// --- JSON-RPC plumbing --------------------------------------------------

#[derive(Serialize)]
pub(crate) struct RpcRequest<'a> {
    pub(crate) jsonrpc: &'a str,
    pub(crate) id: u32,
    pub(crate) method: &'a str,
    pub(crate) params: serde_json::Value,
}

#[derive(Deserialize)]
pub(crate) struct RpcResponse {
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) error: Option<RpcError>,
}

#[derive(Deserialize)]
pub(crate) struct RpcError {
    #[allow(dead_code)]
    code: i64,
    pub(crate) message: String,
}

/// Transport-level deadline for a single JSON-RPC READ (`rpc_value` /
/// `eth_call_batch`). Generous — real reads are sub-2s, but a big `eth_call`
/// under load shouldn't trip it. Its job is to bound the pathological case: a
/// TCP-connected-but-silent RPC node ("black hole"). On `wasm32`, `reqwest`
/// wraps the browser `fetch` API, which has NO default timeout AND
/// `reqwest::Client::timeout` is a documented no-op — so without this guard
/// such a node yields a future that never resolves and hangs EVERY consumer
/// (CLI, off-bundle, every browser paint site), not just the few UI paths that
/// wrap calls in `app::net::with_timeout`.
pub(crate) const RPC_TIMEOUT_MS: u32 = 20_000;

/// Build the shared read client. On native, `reqwest`'s own `timeout` works, so
/// set it directly (covers connect + the whole request/body). On wasm it's a
/// no-op (see [`RPC_TIMEOUT_MS`]) — the caller races against [`sleep_ms`].
pub(crate) fn read_client() -> reqwest::Client {
    #[cfg(not(target_arch = "wasm32"))]
    {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(RPC_TIMEOUT_MS as u64))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    }
    #[cfg(target_arch = "wasm32")]
    {
        reqwest::Client::new()
    }
}

/// Race a read future against an [`RPC_TIMEOUT_MS`] timer and return its output,
/// or a timeout `Err`. This is the portable backstop for the wasm no-op-timeout
/// case (`reqwest::Client::timeout` does nothing on `fetch`): it mirrors
/// `app::net::with_timeout`, racing the work against the cfg-gated [`sleep_ms`]
/// (tokio on native / `setTimeout` Promise on wasm) via
/// `futures_util::future::select`. The losing future is dropped (browser
/// `fetch` cancels on drop). Runs on BOTH targets — on native it's belt-and-
/// suspenders alongside the client builder timeout; on wasm it IS the timeout.
pub(crate) async fn timeout_send<F, T>(label: &str, fut: F) -> Result<T, String>
where
    F: std::future::Future<Output = T>,
{
    use futures_util::future::{select, Either};
    let work = std::pin::pin!(fut);
    let timer = std::pin::pin!(sleep_ms(RPC_TIMEOUT_MS));
    match select(work, timer).await {
        Either::Left((out, _)) => Ok(out),
        Either::Right(((), _)) => Err(format!(
            "{label}: RPC request timed out after {}s",
            RPC_TIMEOUT_MS / 1000
        )),
    }
}

/// Raw JSON-RPC call returning the `result` field verbatim. Methods like
/// `eth_getLogs` return arrays, so the result type must stay a `Value`
/// rather than being forced into a `String` (which silently broke log
/// decoding — the in-app feedback list).
pub(crate) async fn rpc_value(method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let body = RpcRequest {
        jsonrpc: "2.0",
        id: 1,
        method,
        params,
    };
    let client = read_client();
    // Race send + body-read against the deadline as ONE future so a node that
    // connects then stalls mid-body can't hang either step (the wasm case).
    let parsed: RpcResponse = timeout_send(method, async {
        let resp = client
            .post(RPC_URL)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("{method} send: {e}"))?;
        resp.json::<RpcResponse>()
            .await
            .map_err(|e| format!("{method} decode: {e}"))
    })
    .await??;
    if let Some(err) = parsed.error {
        return Err(format!("{method}: {}", err.message));
    }
    parsed
        .result
        .ok_or_else(|| format!("{method} returned no result"))
}

/// JSON-RPC call whose result is a string (hex quantity, tx hash, etc.).
pub(crate) async fn rpc(method: &str, params: serde_json::Value) -> Result<String, String> {
    let value = rpc_value(method, params).await?;
    value
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("{method}: expected string result"))
}

pub(crate) async fn eth_call(to: &str, data_hex: &str) -> Result<String, String> {
    rpc(
        "eth_call",
        serde_json::json!([{ "to": to, "data": data_hex }, "latest"]),
    )
    .await
}

/// `true` if `address` has deployed bytecode (i.e. is a contract, not a
/// counterfactual / EOA). A token-bound account is deterministic — it
/// exists as an address even before `createTokenBoundAccount` deploys it,
/// so this distinguishes a live TBA from a not-yet-deployed one. Reads
/// `eth_getCode`; an empty result (`0x` / `0x0`) means undeployed.
pub async fn is_contract_deployed(address: &str) -> Result<bool, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(false);
    }
    // Validate the address shape so a malformed string surfaces a clear
    // error rather than an opaque RPC fault.
    let _ = parse_eth_address(address)?;
    let code = rpc(
        "eth_getCode",
        serde_json::json!([address, "latest"]),
    )
    .await?;
    let trimmed = code.trim().trim_start_matches("0x");
    Ok(!trimmed.is_empty() && !trimmed.chars().all(|c| c == '0'))
}

/// Build calldata for a `fn(uint256)` selector with a single id argument.
pub(crate) fn call_uint(sig: &str, id: u64) -> String {
    let mut data = Vec::with_capacity(4 + 32);
    data.extend_from_slice(&selector(sig));
    data.extend_from_slice(&u256_be(id as u128));
    format!("0x{}", bytes_to_hex(&data))
}

/// Decode an ABI `address` return (right-aligned in 32 bytes). `None` for the
/// zero address or a short result.
pub(crate) fn decode_address(result_hex: &str) -> Option<String> {
    let trimmed = result_hex.trim().trim_start_matches("0x");
    if trimmed.len() < 64 {
        return None;
    }
    let addr_hex = &trimmed[trimmed.len() - 40..];
    if addr_hex.chars().all(|c| c == '0') {
        return None;
    }
    Some(format!("0x{}", addr_hex.to_lowercase()))
}

/// Decode an ABI `string` return (offset + length + bytes). `None` on a
/// short/truncated/invalid body.
pub(crate) fn decode_string(result_hex: &str) -> Option<String> {
    let raw = hex_to_bytes(result_hex).ok()?;
    if raw.len() < 64 {
        return None;
    }
    let len = u64::from_be_bytes(raw[56..64].try_into().ok()?) as usize;
    // `len` is attacker-controlled — slice via checked add so a huge length
    // returns None instead of overflowing.
    let end = len.checked_add(64)?;
    let body = raw.get(64..end)?;
    String::from_utf8(body.to_vec()).ok()
}

/// Send many `eth_call`s as ONE JSON-RPC batch (a single POST). Returns each
/// call's `result` hex in input order; a per-call RPC error maps to `Err` for
/// just that entry. Collapses an N-token scan from N round-trips into one.
pub(crate) async fn eth_call_batch(calls: &[(&str, String)]) -> Result<Vec<Result<String, String>>, String> {
    if calls.is_empty() {
        return Ok(Vec::new());
    }
    let batch: Vec<serde_json::Value> = calls
        .iter()
        .enumerate()
        .map(|(i, (to, data))| {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": i,
                "method": "eth_call",
                "params": [{ "to": to, "data": data }, "latest"],
            })
        })
        .collect();
    let client = read_client();
    // Same deadline as the single-call path — race send + body-read together.
    let parsed: Vec<serde_json::Value> = timeout_send("eth_call batch", async {
        let resp = client
            .post(RPC_URL)
            .json(&serde_json::Value::Array(batch))
            .send()
            .await
            .map_err(|e| format!("eth_call batch send: {e}"))?;
        resp.json::<Vec<serde_json::Value>>()
            .await
            .map_err(|e| format!("eth_call batch decode: {e}"))
    })
    .await??;
    // Batch responses may arrive out of order — index by the `id` we set.
    let mut out: Vec<Result<String, String>> = (0..calls.len())
        .map(|_| Err("missing batch response".to_string()))
        .collect();
    for item in parsed {
        let Some(idx) = item.get("id").and_then(|v| v.as_u64()).map(|i| i as usize) else {
            continue;
        };
        if idx >= out.len() {
            continue;
        }
        if let Some(err) = item.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("rpc error");
            out[idx] = Err(msg.to_string());
        } else if let Some(result) = item.get("result").and_then(|r| r.as_str()) {
            out[idx] = Ok(result.to_string());
        }
    }
    Ok(out)
}

pub(crate) async fn eth_get_logs(
    address: &str,
    topics: Vec<serde_json::Value>,
    from_block: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let result = rpc_value(
        "eth_getLogs",
        serde_json::json!([{
            "address": address,
            "topics": topics,
            "fromBlock": from_block,
            "toBlock": "latest"
        }]),
    )
    .await?;
    match result {
        serde_json::Value::Array(logs) => Ok(logs),
        _ => Ok(Vec::new()),
    }
}


pub(crate) async fn eth_get_transaction_count(addr: &str) -> Result<u128, String> {
    let hex = rpc(
        "eth_getTransactionCount",
        serde_json::json!([addr, "pending"]),
    )
    .await?;
    parse_hex_quantity(&hex)
}

pub(crate) async fn eth_gas_price() -> Result<u128, String> {
    let hex = rpc("eth_gasPrice", serde_json::json!([])).await?;
    parse_hex_quantity(&hex)
}

pub(crate) async fn eth_estimate_gas(from: &str, to: &str, data_hex: &str) -> Result<u128, String> {
    let hex = rpc(
        "eth_estimateGas",
        serde_json::json!([{ "from": from, "to": to, "data": data_hex }]),
    )
    .await?;
    // Add a 25% buffer so we don't get caught by gas-estimation jitter.
    let estimate = parse_hex_quantity(&hex)?;
    Ok(estimate + estimate / 4)
}

pub(crate) async fn eth_send_raw_transaction(raw_hex: &str) -> Result<String, String> {
    match rpc("eth_sendRawTransaction", serde_json::json!([raw_hex])).await {
        Ok(hash) => Ok(hash),
        Err(err) => {
            // "already known" / "ALREADY_EXISTS" / "nonce too low" all
            // mean a previous submit of the same signed bytes (or
            // same-nonce sibling) is already in the mempool. Compute
            // the tx hash locally and let the caller's receipt poll
            // pick it up. Avoids spurious failures when the user
            // double-clicks `create` or retries after a UI hiccup.
            let lower = err.to_lowercase();
            let is_duplicate = lower.contains("already known")
                || lower.contains("already exists")
                || lower.contains("nonce too low");
            if is_duplicate {
                let bytes = hex_to_bytes(raw_hex)?;
                let mut hasher = Keccak256::new();
                hasher.update(&bytes);
                let digest = hasher.finalize();
                Ok(format!("0x{}", bytes_to_hex(&digest)))
            } else {
                Err(err)
            }
        }
    }
}

/// Map a 4-byte custom-error / `Error(string)` selector (the first 4 bytes of
/// revert data) to a friendly, actionable message. PURE + network-free — the
/// core of revert decoding, unit-tested in isolation. Covers the facets a CLI
/// user actually hits on a sponsored write: ScheduleFacet
/// (`schedule`/`unschedule`), InviteFacet (`invite create/accept/reclaim`), and
/// the cost/escrow facets the writes pull `$LH` through. Selectors are computed
/// from the EXACT `error Name(...)` signatures in the facet sources, so a
/// rename there must be mirrored here (the unit test pins the bytes).
///
/// `None` for an unrecognised selector so the caller can fall back to the
/// generic hint — never a misleading guess.
///
/// Each entry also carries its stable `LH2xxx` code from the central
/// [`crate::error_codes`] registry; [`decode_known_revert`] returns the bare
/// message (back-compat), while [`decode_known_revert_coded`] prefixes the
/// `LH2xxx:` label so a surfaced revert reads "LH2003: SpendExceedsBudget — …".
/// `KNOWN` (signature, LH-code, friendly message). The signature is keccak'd to
/// its 4-byte selector exactly as Solidity does (same `selector()` the encoders
/// use), so this list is the source of truth, not hand-copied hex.
pub(crate) const KNOWN_REVERTS: &[(&str, u16, &str)] = {
    use crate::error_codes as c;
    &[
        // --- ScheduleFacet (schedule / unschedule / pause / resume / topup) ---
        ("NotDue()", c::TX_NOT_DUE, "this job isn't due yet — the scheduler only fires on the interval. Check `localharness jobs` for its next run."),
        ("StaleNextRun()", c::TX_STALE_NEXT_RUN, "this run was already fired by the scheduler — nothing to do (the on-chain clock already advanced)."),
        ("SpendExceedsBudget()", c::TX_SPEND_EXCEEDS_BUDGET, "the run would spend more $LH than the job's remaining budget — top it up or it will be marked exhausted."),
        ("NotScheduler()", c::TX_NOT_SCHEDULER, "only the scheduler worker can record a run — this isn't a user action."),
        ("NotJobOwner()", c::TX_NOT_JOB_OWNER, "you don't own this job — only its scheduler can cancel/pause/top it up. Check `localharness jobs` under the right `--as` identity."),
        ("UnknownJob()", c::TX_UNKNOWN_JOB, "no job with that id — list yours with `localharness jobs` (the id is the `#N`)."),
        ("JobNotActive()", c::TX_JOB_NOT_ACTIVE, "the job is already cancelled or exhausted — there's nothing to cancel. See `localharness jobs`."),
        ("JobNotPaused()", c::TX_JOB_NOT_PAUSED, "the job isn't paused, so it can't be resumed."),
        ("UnregisteredTarget()", c::TX_UNREGISTERED_TARGET, "the target isn't a registered agent — run `localharness whoami <target>` to confirm it exists first."),
        ("ZeroInterval()", c::TX_ZERO_INTERVAL, "the interval is below the 60s minimum the facet allows — use `--every 60s` or more."),
        ("ZeroRuns()", c::TX_ZERO_RUNS, "max-runs must be at least 1 — drop `--runs 0`."),
        // --- InviteFacet (invite create / accept / reclaim) ---
        ("CodeTaken()", c::TX_CODE_TAKEN, "that invite code already exists on-chain — generate a fresh one (`invite create` makes a new code each time)."),
        ("BadTtl()", c::TX_BAD_TTL, "the invite TTL is outside the allowed 1h..90d window — use e.g. `--ttl 7d`."),
        ("EscrowCapExceeded()", c::TX_ESCROW_CAP_EXCEEDED, "this would push your locked invite escrow past the per-funder cap — reclaim an expired invite (`invite reclaim <code>`) or use a smaller amount."),
        ("UnknownInvite()", c::TX_UNKNOWN_INVITE, "no invite matches that code — double-check you copied the full code, including the `inv-` prefix."),
        ("NotOpen()", c::TX_NOT_OPEN, "this invite was already accepted or reclaimed — it's spent."),
        ("Expired()", c::TX_EXPIRED, "this invite has expired — it can no longer be accepted, only reclaimed by its funder (`invite reclaim <code>`)."),
        ("NotYetExpired()", c::TX_NOT_YET_EXPIRED, "this invite hasn't expired yet — reclaim only works AFTER the TTL elapses. Until then it can still be accepted."),
        // --- Shared (both facets + the cost/escrow path) ---
        ("ZeroBudget()", c::TX_ZERO_BUDGET, "the budget/amount must be greater than 0."),
        ("ZeroAmount()", c::TX_ZERO_AMOUNT, "the amount must be greater than 0."),
        ("NotConfigured()", c::TX_NOT_CONFIGURED, "the on-chain credits token isn't configured — this is a platform-side misconfiguration, not your input. Report it via `localharness feedback`."),
        // --- Generic ERC-20 transferFrom failure (escrow pull) ---
        // The facets `require(transferFrom(...))` with these reason strings; if
        // the require trips it surfaces as Error(string). The selector branch
        // below decodes the actual string, but map the bare selector too.
        ("Error(string)", c::TX_REASON_STRING, "the on-chain call reverted with a reason string (decoded above when available)."),
    ]
};

// The bare (uncoded) accessor — the back-compat surface returning just the
// friendly message. Production now surfaces via `decode_known_revert_coded`, so
// on non-test wasm builds this is only kept for the native unit tests; silence
// the dead-code lint there rather than drop a documented helper.
#[cfg_attr(all(target_arch = "wasm32", not(test)), allow(dead_code))]
pub(crate) fn decode_known_revert(selector_bytes: [u8; 4]) -> Option<&'static str> {
    for (sig, _code, msg) in KNOWN_REVERTS {
        if selector(sig) == selector_bytes {
            return Some(msg);
        }
    }
    None
}

/// Like [`decode_known_revert`] but prefixes the stable `LH2xxx:` code +
/// the facet error name, e.g. "LH2003: SpendExceedsBudget — the run would spend
/// more $LH …". `None` for an unrecognised selector. This is what surfaces to
/// the user so a revert is coded + named instead of a bare 4-byte selector.
pub(crate) fn decode_known_revert_coded(selector_bytes: [u8; 4]) -> Option<String> {
    for (sig, code, msg) in KNOWN_REVERTS {
        if selector(sig) == selector_bytes {
            // "Name" from "Name()"; the registry label from the code.
            let name = sig.split('(').next().unwrap_or(sig);
            return Some(format!("{}: {name} — {msg}", crate::error_codes::fmt_label(*code)));
        }
    }
    None
}

/// Turn raw revert return-data into a human message. Recognises:
///   - the standard `Error(string)` envelope (`0x08c379a0` + ABI string) — the
///     `require("...")` reason, e.g. "schedule: escrow failed" / "ERC20:
///     transfer amount exceeds balance" (an under-funded escrow);
///   - a known custom-error selector via `decode_known_revert`.
/// Returns `None` for empty/unrecognised data so the caller keeps the bare hash
/// plus a generic hint. PURE — unit-tested.
pub(crate) fn decode_revert_data(data: &[u8]) -> Option<String> {
    if data.len() < 4 {
        return None;
    }
    let sel: [u8; 4] = [data[0], data[1], data[2], data[3]];
    // Standard Error(string): 0x08c379a0 || abi.encode(string). Decode the
    // string and pass it through verbatim — `require` reasons are already
    // human-readable (and often the most actionable: an escrow-pull failure
    // means "you don't have enough $LH / haven't approved the diamond").
    if sel == [0x08, 0xc3, 0x79, 0xa0] {
        let label = crate::error_codes::fmt_label(crate::error_codes::TX_REASON_STRING);
        let hex = format!("0x{}", bytes_to_hex(&data[4..]));
        if let Some(reason) = decode_string(&hex) {
            let reason = reason.trim();
            if !reason.is_empty() {
                let lower = reason.to_ascii_lowercase();
                // An ERC-20 balance/allowance failure on an escrow pull is the
                // single most common cause — say what to DO about it.
                if lower.contains("balance") || lower.contains("allowance") || lower.contains("escrow") {
                    return Some(format!(
                        "{label}: {reason} — you likely don't have enough $LH for the escrow. \
                         Fund it (`localharness redeem <code>` or have another agent \
                         `send` you $LH), then retry."
                    ));
                }
                return Some(format!("{label}: {reason}"));
            }
        }
    }
    // Panic(uint256) (0x4e487b71) → arithmetic/assert; rare here, generic.
    if sel == [0x4e, 0x48, 0x7b, 0x71] {
        return Some(format!(
            "{}: the contract hit an internal assertion (Panic) — this is a platform bug, \
             not your input; please `localharness feedback` it.",
            crate::error_codes::fmt_label(crate::error_codes::TX_PANIC)
        ));
    }
    decode_known_revert_coded(sel)
}

/// Best-effort fetch + decode of WHY a sponsored tx reverted: re-run the same
/// top-level call via `eth_call` at the block it failed in, capture the revert
/// return-data from the node's error `data` field, and decode it
/// (`decode_revert_data`). The replay is read-only (no new tx, no gas, no
/// `$LH`). Returns `None` on any failure to obtain a reason — the caller still
/// has the hash + a generic hint. Never errors out the original flow.
pub(crate) async fn fetch_revert_reason(tx_hash: &str) -> Option<String> {
    // 1. Pull the original tx so we can replay its call shape.
    let tx = rpc_value("eth_getTransactionByHash", serde_json::json!([tx_hash]))
        .await
        .ok()?;
    let tx = tx.as_object()?;
    let to = tx.get("to")?.as_str()?;
    let from = tx.get("from")?.as_str()?;
    let input = tx.get("input").and_then(|v| v.as_str()).unwrap_or("0x");
    // Replay AT the failing block (state just before is closest to reproduce).
    let block = tx.get("blockNumber").and_then(|v| v.as_str()).unwrap_or("latest");

    // 2. eth_call the same call — a reverting call returns the revert data in
    //    the JSON-RPC error's `data` field. Capture it explicitly (the shared
    //    `rpc_value` discards `error.data`, so do a raw call here).
    let body = RpcRequest {
        jsonrpc: "2.0",
        id: 1,
        method: "eth_call",
        params: serde_json::json!([{ "from": from, "to": to, "data": input }, block]),
    };
    let client = reqwest::Client::new();
    let resp = client.post(RPC_URL).json(&body).send().await.ok()?;
    let json: serde_json::Value = resp.json().await.ok()?;

    // The revert payload can live in error.data (string or {message,data}).
    let err = json.get("error")?;
    let data_hex = err
        .get("data")
        .and_then(|d| {
            d.as_str()
                .map(|s| s.to_string())
                .or_else(|| d.get("data").and_then(|x| x.as_str()).map(|s| s.to_string()))
        })
        .filter(|s| s.len() > 2 && s.starts_with("0x"));
    if let Some(hex) = data_hex {
        if let Ok(bytes) = hex_to_bytes(&hex) {
            if let Some(reason) = decode_revert_data(&bytes) {
                return Some(reason);
            }
        }
    }
    // No structured data — some nodes embed the reason in error.message.
    err.get("message").and_then(|m| m.as_str()).and_then(|m| {
        let m = m.trim();
        // Only surface a message that actually says something beyond "reverted".
        if m.is_empty() || m.eq_ignore_ascii_case("execution reverted") {
            None
        } else {
            Some(m.to_string())
        }
    })
}

/// The catch-all line appended to a bare revert so the user is never left
/// staring at only a hash. Lists the common, user-fixable causes.
pub(crate) const GENERIC_REVERT_HINT: &str = "the transaction reverted on-chain. Common causes: \
    not enough $LH for the escrow/cost (fund with `localharness redeem <code>`), \
    you don't own the name/job you're acting on, a duplicate/expired/already-spent \
    invite, or a not-yet-due job. Run `localharness whoami <name>` / `jobs` to check state.";

/// Poll `eth_getTransactionReceipt` until the receipt resolves. Errors
/// after ~30 seconds — Tempo Moderato blocks are ~1s so 30 attempts
/// is more than enough headroom. On a `0x0` (revert) status, best-effort
/// fetch + decode the revert REASON (so the user sees WHY, not just a hash)
/// and always append the generic hint.
pub(crate) async fn wait_for_receipt(tx_hash: &str) -> Result<(), String> {
    for _ in 0..30 {
        let body = RpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "eth_getTransactionReceipt",
            params: serde_json::json!([tx_hash]),
        };
        let client = reqwest::Client::new();
        let resp = client
            .post(RPC_URL)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("receipt poll: {e}"))?;
        // Receipt comes back as an object or null — bypass the
        // RpcResponse string-only deserializer.
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("receipt parse: {e}"))?;
        if let Some(receipt) = json.get("result").filter(|v| !v.is_null()) {
            let status = receipt
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            if status == "0x1" {
                return Ok(());
            } else if status == "0x0" {
                // Decode WHY it reverted (best-effort; read-only replay).
                let reason = fetch_revert_reason(tx_hash).await;
                return Err(match reason {
                    Some(r) => format!("tx reverted: {r}\n  {GENERIC_REVERT_HINT}\n  tx: {tx_hash}"),
                    None => format!("tx reverted — {GENERIC_REVERT_HINT}\n  tx: {tx_hash}"),
                });
            }
        }
        // Wait ~1s before next poll. spawn_local + a 1s timer would
        // be cleaner; gloo_timers is an option if this becomes a
        // bottleneck. For now: a busy yield via JS microtask.
        sleep_ms(1000).await;
    }
    Err(format!("receipt timeout for {tx_hash}"))
}

/// Cross-target sleep — `tokio::time::sleep` on native, a Promise
/// around `setTimeout` on wasm. Used by `claim_name` to poll the
/// transaction receipt every second.
#[cfg(not(target_arch = "wasm32"))]
pub async fn sleep_ms(ms: u32) {
    tokio::time::sleep(std::time::Duration::from_millis(ms as u64)).await;
}

#[cfg(target_arch = "wasm32")]
pub async fn sleep_ms(ms: u32) {
    use wasm_bindgen_futures::JsFuture;
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                &resolve,
                ms as i32,
            );
        }
    });
    let _ = JsFuture::from(promise).await;
}


#[cfg(target_arch = "wasm32")]
pub(crate) fn log_main_warning(err: &str) {
    use wasm_bindgen::JsValue;
    web_sys::console::warn_1(&JsValue::from_str(&format!("auto-set MAIN: {err}")));
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn log_main_warning(_err: &str) {
    // Native path doesn't have a console; silent — callers can check
    // mainOf themselves after the fact if they need to verify.
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_known_revert_maps_facet_errors() {
        // The decoder must recognise the exact custom-error selectors the
        // ScheduleFacet + InviteFacet emit — keyed off their source signatures.
        // A few representative ones, each with an actionable message.
        let cases = [
            ("NotDue()", "due"),
            ("UnknownJob()", "no job"),
            ("NotJobOwner()", "don't own"),
            ("UnregisteredTarget()", "registered agent"),
            ("CodeTaken()", "already exists"),
            ("NotYetExpired()", "hasn't expired"),
            ("Expired()", "expired"),
            ("NotOpen()", "already accepted or reclaimed"),
            ("BadTtl()", "1h..90d"),
            ("EscrowCapExceeded()", "escrow"),
            ("UnknownInvite()", "no invite"),
        ];
        for (sig, needle) in cases {
            let sel = selector(sig);
            let msg = decode_known_revert(sel)
                .unwrap_or_else(|| panic!("no mapping for {sig}"));
            assert!(
                msg.to_lowercase().contains(needle),
                "message for {sig} ({msg:?}) should mention {needle:?}"
            );
        }
        // An unknown selector → None (caller falls back to the generic hint).
        assert_eq!(decode_known_revert([0xde, 0xad, 0xbe, 0xef]), None);
    }

    #[test]
    fn decode_known_revert_selector_bytes_are_keccak_of_signature() {
        // Pin the wire bytes so a facet rename (which changes the on-chain
        // selector) trips this test, forcing the map to be updated in lockstep.
        // keccak256("NotDue()")[..4] — verifiable with `cast sig "NotDue()"`.
        let not_due = selector("NotDue()");
        let hex: String = not_due.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "47a2375f");
        assert!(decode_known_revert(not_due).is_some());
    }

    #[test]
    fn decode_revert_data_decodes_error_string_envelope() {
        // Standard Error(string) = 0x08c379a0 || abi.encode("...").
        // Hand-encode `require(false, "schedule: escrow failed")`.
        let reason = b"schedule: escrow failed";
        let mut data = vec![0x08, 0xc3, 0x79, 0xa0];
        data.extend_from_slice(&u256_be(0x20)); // offset to string
        data.extend_from_slice(&u256_be(reason.len() as u128)); // length
        let mut padded = reason.to_vec();
        padded.resize(padded.len().div_ceil(32) * 32, 0);
        data.extend_from_slice(&padded);

        let out = decode_revert_data(&data).expect("decodes Error(string)");
        assert!(out.contains("schedule: escrow failed"), "got {out:?}");

        // An ERC-20 balance failure on an escrow pull → actionable funding hint.
        let bal = b"ERC20: transfer amount exceeds balance";
        let mut d2 = vec![0x08, 0xc3, 0x79, 0xa0];
        d2.extend_from_slice(&u256_be(0x20));
        d2.extend_from_slice(&u256_be(bal.len() as u128));
        let mut p2 = bal.to_vec();
        p2.resize(p2.len().div_ceil(32) * 32, 0);
        d2.extend_from_slice(&p2);
        let out2 = decode_revert_data(&d2).expect("decodes balance error");
        assert!(out2.to_lowercase().contains("$lh"), "should suggest funding: {out2:?}");
    }

    #[test]
    fn decode_known_revert_coded_prefixes_lh2xxx_and_name() {
        // The coded decoder must carry the stable LH2xxx code + the facet error
        // name so a revert surfaces "LH2003: SpendExceedsBudget — …".
        let cases = [
            ("SpendExceedsBudget()", "LH2003", "SpendExceedsBudget"),
            ("NotScheduler()", "LH2004", "NotScheduler"),
            ("NotDue()", "LH2001", "NotDue"),
            ("CodeTaken()", "LH2012", "CodeTaken"),
            ("Expired()", "LH2017", "Expired"),
        ];
        for (sig, code, name) in cases {
            let out = decode_known_revert_coded(selector(sig))
                .unwrap_or_else(|| panic!("no coded mapping for {sig}"));
            assert!(out.starts_with(code), "expected {code} prefix, got {out:?}");
            assert!(out.contains(name), "expected name {name} in {out:?}");
        }
        // Unknown selector → None (caller keeps the bare hash + generic hint).
        assert_eq!(decode_known_revert_coded([0xde, 0xad, 0xbe, 0xef]), None);
    }

    #[test]
    fn decode_revert_data_maps_custom_error_and_handles_empty() {
        // A bare custom-error selector (no args) → its friendly message,
        // now prefixed with its stable LH2xxx code.
        let sel = selector("NotYetExpired()");
        let out = decode_revert_data(&sel).expect("maps custom error");
        assert!(out.to_lowercase().contains("hasn't expired"), "got {out:?}");
        assert!(out.starts_with("LH2018"), "expected the LH2018 code prefix, got {out:?}");
        // Empty / too-short data → None (caller keeps the hash + generic hint).
        assert_eq!(decode_revert_data(&[]), None);
        assert_eq!(decode_revert_data(&[0x01, 0x02]), None);
        // Unknown selector → None.
        assert_eq!(decode_revert_data(&[0xde, 0xad, 0xbe, 0xef]), None);
    }

    #[test]
    fn decode_string_edge_cases() {
        // Empty / short → None.
        assert_eq!(decode_string("0x"), None);
        assert_eq!(decode_string(&format!("0x{Z}")), None);
        // Huge length → None, no `64 + len` overflow.
        let huge = format!("0x{}{}", word_usize(0x20), word_u64_max());
        assert_eq!(decode_string(&huge), None);
        // Truncated body → None.
        let trunc = format!("0x{}{}", word_usize(0x20), word_usize(64));
        assert_eq!(decode_string(&trunc), None);
        // Valid "hi".
        let ok = format!(
            "0x{}{}{}",
            word_usize(0x20),
            word_usize(2),
            "6869000000000000000000000000000000000000000000000000000000000000"
        );
        assert_eq!(decode_string(&ok).as_deref(), Some("hi"));
    }

    #[test]
    fn decode_address_edge_cases() {
        // Short / empty → None.
        assert_eq!(decode_address("0x"), None);
        assert_eq!(decode_address("0x00"), None);
        // All-zero word → None (zero address means "unset").
        assert_eq!(decode_address(&format!("0x{Z}")), None);
        // A real address in the low 20 bytes.
        let w = format!("0x{}", "0".repeat(24) + "1111111111111111111111111111111111111111");
        assert_eq!(
            decode_address(&w).as_deref(),
            Some("0x1111111111111111111111111111111111111111")
        );
    }
}
