//! Pure Web Push enrollment-verification core (native-testable; telemetry #40).
//!
//! Enrollment used to be fire-and-forget: the browser POSTed its subscription
//! to the proxy push store and trusted the 200. A sub that silently never
//! landed (or was later evicted) meant every closed-tab push died while the
//! user believed they were enrolled. [`verify_enrolled`] checks the store's
//! GET response actually contains THIS device's subscription; [`bell_status`]
//! is the one place the bell panel's enrolled/not-enrolled line comes from.

/// Parse the proxy push-store GET body (`{"subs":[{endpoint,keys,dev?},…]}`)
/// and return `Some(total_devices)` iff a sub matching this device's
/// `endpoint` (or non-empty stable `dev` id) is present; `None` = the
/// enrollment did NOT land.
pub fn verify_enrolled(store_json: &str, endpoint: &str, dev: &str) -> Option<usize> {
    let v: serde_json::Value = serde_json::from_str(store_json).ok()?;
    let subs = v.get("subs")?.as_array()?;
    let landed = subs.iter().any(|s| {
        s.get("endpoint").and_then(|e| e.as_str()) == Some(endpoint)
            || (!dev.is_empty() && s.get("dev").and_then(|d| d.as_str()) == Some(dev))
    });
    landed.then_some(subs.len())
}

/// The bell panel's push-state line for the INSTANT paint on open (the async
/// enroll result overwrites it). `permission` = the Notification permission
/// ("granted"/"denied"/"default"); `enrolled_hint` = this device verified an
/// enrollment before (cached flag).
pub fn bell_status(permission: &str, enrolled_hint: bool) -> &'static str {
    match (permission, enrolled_hint) {
        ("denied", _) => {
            "push: blocked — allow notifications for this site in browser settings"
        }
        ("granted", true) => "push: enrolled — this device gets alerts with the tab closed",
        ("granted", false) => "push: enrolling this device…",
        _ => "push: awaiting permission…",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(subs: &[(&str, Option<&str>)]) -> String {
        let list: Vec<serde_json::Value> = subs
            .iter()
            .map(|(ep, dev)| {
                let mut o = serde_json::json!({"endpoint": ep, "keys": {"p256dh": "k", "auth": "a"}});
                if let Some(d) = dev {
                    o["dev"] = serde_json::json!(d);
                }
                o
            })
            .collect();
        serde_json::json!({ "subs": list }).to_string()
    }

    #[test]
    fn verify_matches_endpoint() {
        let s = store(&[("https://push/x", None), ("https://push/y", None)]);
        assert_eq!(verify_enrolled(&s, "https://push/y", ""), Some(2));
    }

    #[test]
    fn verify_matches_dev_when_endpoint_rotated() {
        // PWA reinstall rotates the endpoint but the stable dev id survives —
        // the store upserted by dev, so the dev match still proves enrollment.
        let s = store(&[("https://push/new", Some("dev-1"))]);
        assert_eq!(verify_enrolled(&s, "https://push/old", "dev-1"), Some(1));
    }

    #[test]
    fn verify_rejects_missing_sub_and_garbage() {
        let s = store(&[("https://push/other", Some("dev-2"))]);
        assert_eq!(verify_enrolled(&s, "https://push/mine", "dev-1"), None);
        assert_eq!(verify_enrolled("not json", "e", "d"), None);
        assert_eq!(verify_enrolled("{}", "e", "d"), None);
        // empty dev must not match a sub that also has no dev field
        assert_eq!(verify_enrolled(&store(&[("https://push/o", None)]), "https://push/m", ""), None);
    }

    #[test]
    fn bell_status_covers_states() {
        assert!(bell_status("denied", true).contains("blocked"));
        assert!(bell_status("granted", true).contains("enrolled"));
        assert!(bell_status("granted", false).contains("enrolling"));
        assert!(bell_status("default", false).contains("awaiting"));
    }
}
