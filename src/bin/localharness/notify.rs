//! `localharness notify` — buzz YOUR OWN phone from a shell (feedback #69).
//!
//! The headless half of the notifications loop: the browser app's "enable
//! notifications" flow publishes a Web Push subscription on-chain under the
//! owner's MAIN tokenId (`keccak256("localharness.push_sub")`,
//! `src/app/notifications.rs`); this command signs the standard proxy auth
//! token and POSTs `{title, body}` to the proxy's `/api/notify` route, which
//! resolves the CALLER's own subscription (self-only — no cross-user
//! targeting) and delivers the push. Metered like a `call` (~0.01 `$LH`),
//! which is also the spam leash. The shell-side "notify me when done":
//!
//! ```sh
//! long_job && localharness notify "job done" "the overnight build is green"
//! ```

use crate::{load_signer, registry};


/// `localharness notify [--as <me>] [--to <agent>] <title> [body...]` —
/// Web-Push a note to the caller's OWN registered device, or with `--to` to
/// ANOTHER agent's notification inbox + enrolled phone (cross-agent; the
/// proxy stamps the push with the sender's chain-verified name).
///
/// CROSS-AGENT ENROLLMENT: if the `--to` target has no device enrolled for Web
/// Push, the proxy returns a clear `enrolled: false` 200 (the sender is not
/// charged and did nothing wrong) — we relay its `message` verbatim instead of
/// claiming the note was delivered.
pub(crate) async fn notify(caller: Option<&str>, rest: &[String]) -> i32 {
    const USAGE: &str = "usage: localharness notify [--as <me>] [--to <agent>] <title> [body...]";
    let (to, rest) = match crate::util::take_value_flag(rest, "--to", USAGE) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let Some(title) = rest.first().map(|s| s.trim()).filter(|s| !s.is_empty()) else {
        eprintln!("{USAGE}");
        return 2;
    };
    let body = rest[1..].join(" ").trim().to_string();

    let signer = match load_signer(caller) {
        Ok(s) => s,
        Err(code) => return code,
    };
    // Pay PER REQUEST: top the meter up from the wallet if needed, exactly
    // like `call` (best-effort + sponsored; an empty wallet just 402s below).
    crate::call::ensure_meter_funded(&signer).await;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let token = registry::proxy_auth_token(&signer, now, "notify");
    let endpoint = format!(
        "{}/api/notify",
        registry::CREDIT_PROXY_URL.trim_end_matches('/')
    );

    let mut payload = serde_json::json!({ "title": title, "body": body });
    if let Some(target) = to.as_deref() {
        payload["to"] = serde_json::Value::String(target.to_lowercase());
    }
    let resp = match reqwest::Client::new()
        .post(&endpoint)
        .header("content-type", "application/json")
        .header("x-goog-api-key", token)
        .json(&payload)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("notify failed: proxy unreachable ({e})");
            return 1;
        }
    };
    let status = resp.status();
    let json: serde_json::Value = resp.json().await.unwrap_or_default();

    if status.is_success() {
        match to.as_deref() {
            // CROSS-AGENT: the proxy returns 200 even when the target has no
            // device enrolled for Web Push (`enrolled: false`) — the note
            // cannot reach them, but it is not the sender's failure. Relay the
            // proxy's clear message verbatim instead of a misleading "sent".
            Some(target) => {
                let enrolled = json
                    .get("enrolled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                if enrolled {
                    println!("notification sent to {target}'s inbox/device.");
                } else {
                    let msg = json
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("the target has not enrolled any device for Web Push, so the note did not reach them");
                    println!("{msg}");
                }
            }
            None => println!("notification sent — check your device."),
        }
        return 0;
    }
    let msg = json
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown proxy error");
    eprintln!("notify failed ({}): {msg}", status.as_u16());
    if status.as_u16() == 404 && to.is_none() {
        // The actionable half: the push target is enrolled in the BROWSER app.
        eprintln!(
            "hint: open your subdomain in the app (admin → account → notifications → \
             [enable notifications]) on the device you want buzzed, then retry."
        );
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;

    #[tokio::test]
    async fn notify_requires_a_title() {
        // No args / a blank title is a usage error (exit 2), caught before any
        // key loading or network I/O.
        assert_eq!(notify(None, &[]).await, 2);
        assert_eq!(notify(None, &args(&["   "])).await, 2);
    }
}
