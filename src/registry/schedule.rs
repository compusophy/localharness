use k256::ecdsa::SigningKey;

use super::*;

// --- OFF-CHAIN scheduling (GitHub store via the proxy) ----------------------
//
// Scheduled jobs live OFF-CHAIN: the browser/CLI POST `/api/schedule` and the
// proxy keeps the job as a GitHub-store record (no escrow, no gas, no sponsor —
// the same off-chain model as the app store / telemetry). A `reminder` is a
// future web-push (zero $LH); an `agent` job runs the target each fire, billed
// per run from the OWNER's meter (same cost as an interactive message). The
// cron worker fires both stores. Auth = the personal-sign proxy token; the
// signer is the job OWNER (billing + push). The on-chain ScheduleFacet client
// (escrowed jobs, keeper drain) was PURGED 2026-07-30 — pre-1.0.0 reset, no
// jobs to drain; the facet itself dies at the reset genesis.

/// Create an off-chain scheduled job via `POST /api/schedule`. `kind` =
/// `"reminder"` | `"agent"`; `target` is the subdomain to run (agent only —
/// ignored/empty for a reminder). Returns the new job id. `now_secs` is the
/// caller's UNIX time, passed in so this is cross-target (native CLI =
/// `SystemTime`, browser = `js_sys::Date::now()`).
pub async fn create_offchain_job(
    signer: &SigningKey,
    now_secs: u64,
    kind: &str,
    target: &str,
    task: &str,
    interval_secs: u64,
    runs: u32,
) -> Result<String, String> {
    let token = proxy_auth_token(signer, now_secs, "schedule");
    let url = format!("{CREDIT_PROXY_URL}api/schedule");
    let mut body = serde_json::json!({
        "action": "create",
        "kind": kind,
        "task": task,
        "intervalSecs": interval_secs,
        "runs": runs,
    });
    if !target.is_empty() {
        body["target"] = serde_json::Value::String(target.to_string());
    }
    let resp = http_post_json_authed_returning(&url, &token, &body).await?;
    resp.get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("schedule: unexpected response {resp}"))
}

/// Cancel an off-chain scheduled job by id (owner-gated server-side: the proxy
/// only deletes a job whose stored owner matches the token's signer). `Ok(())`
/// on success; an unknown id / not-yours surfaces the proxy's error.
pub async fn cancel_offchain_job(signer: &SigningKey, now_secs: u64, id: &str) -> Result<(), String> {
    let token = proxy_auth_token(signer, now_secs, "schedule");
    let url = format!("{CREDIT_PROXY_URL}api/schedule");
    let body = serde_json::json!({ "action": "cancel", "id": id });
    http_post_json_authed_returning(&url, &token, &body).await.map(|_| ())
}

/// List the caller's off-chain scheduled jobs (the `jobs` array). Read-only.
pub async fn list_offchain_jobs(
    signer: &SigningKey,
    now_secs: u64,
) -> Result<Vec<serde_json::Value>, String> {
    let token = proxy_auth_token(signer, now_secs, "schedule");
    let url = format!("{CREDIT_PROXY_URL}api/schedule");
    let body = serde_json::json!({ "action": "list" });
    let resp = http_post_json_authed_returning(&url, &token, &body).await?;
    Ok(resp
        .get("jobs")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default())
}
