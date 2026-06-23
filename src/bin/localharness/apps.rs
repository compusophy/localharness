//! `apps` — discover PUBLISHED apps in the OFF-CHAIN app store.

use crate::registry;

/// `localharness apps` — list every name that has a published cartridge in the
/// off-chain app store (the proxy's FREE `/api/apps` catalog). Read-only, no
/// auth, no `$LH` — the app-discovery on-ramp (the sibling of `discover`, which
/// finds agents). Prints each app's name + its live URL.
pub(crate) async fn list_apps() -> i32 {
    let url = format!("{}api/apps", registry::CREDIT_PROXY_URL);
    let text = match reqwest::Client::new().get(&url).send().await {
        Ok(r) => match r.text().await {
            Ok(t) => t,
            Err(e) => {
                eprintln!("apps: reading response: {e}");
                return 1;
            }
        },
        Err(e) => {
            eprintln!("apps: {e}");
            return 1;
        }
    };
    let names: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if names.is_empty() {
        println!("no apps published yet — be the first: `localharness publish <name> app.rl`");
        return 0;
    }
    println!("{} published app(s) in the store:", names.len());
    for n in &names {
        println!("  {n}  →  https://{n}.localharness.xyz/");
    }
    0
}
