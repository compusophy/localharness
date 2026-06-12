//! LIVE regression proof for the `setPushSub` gas fix (the bug that blocked
//! phone push registration). Publishes a realistic ~365-byte Web Push
//! subscription on-chain via the SPONSORED path — exactly what the browser
//! does when a viewer grants notification permission — then reads it back.
//!
//! History: caps of 600k, 965k and 2.66M all silently out-of-gassed because
//! Tempo charges ~8.5k gas/BYTE for storage writes; the fix is
//! `1_500_000 + len * 9_000` (src/registry/push.rs).
//!
//! Run: `cargo run --example push_sub_live --features wallet`
//! Needs `~/.localharness/keys/claude.localharness.key` (or set
//! `PUSH_TEST_KEY` to a raw private-key hex). Writes to Moderato TESTNET.

use localharness::registry;

const ALPHA_USD: &str = "0x20c0000000000000000000000000000000000001";

// Same shape/length as a real Chrome FCM subscription (~365 bytes).
const SUB_JSON: &str = concat!(
    r#"{"endpoint":"https://fcm.googleapis.com/fcm/send/dummy-live-proof-"#,
    r#"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","#,
    r#""expirationTime":null,"keys":{"p256dh":"BNcRdreALRFXTkOOUHK1EtK2wtaz"#,
    r#"5Ry4YfYCA_0QTpQtUbVlUls0VJXg7A8u-Ts1XbjhazAkj7I99e8QcYP7DkM_dummy00","#,
    r#""auth":"tBHItJI5svbpez7KI4CCXg_dummy0"}}"#
);

#[tokio::main]
async fn main() -> Result<(), String> {
    let key_hex = match std::env::var("PUSH_TEST_KEY") {
        Ok(k) => k,
        Err(_) => {
            let home = dirs_path().join("keys").join("claude.localharness.key");
            std::fs::read_to_string(&home)
                .map_err(|e| format!("no PUSH_TEST_KEY and no {}: {e}", home.display()))?
        }
    };
    let sender = localharness::wallet::from_private_key_hex(key_hex.trim())
        .map_err(|e| format!("bad key: {e}"))?;
    // The dedicated low-budget testnet sponsor (same key the wasm bundle embeds).
    let sponsor = localharness::wallet::from_private_key_hex(
        "0x046a830b5203d1d2c0a205a1432746e4381d0874711b2de7f575a973644b9d43",
    )
    .map_err(|e| format!("bad sponsor key: {e}"))?;

    let addr = localharness::encoding::bytes_to_hex(&localharness::wallet::address(&sender));
    println!("device address  {addr}");
    println!("payload bytes   {}", SUB_JSON.len());

    let tx = registry::set_push_sub_sponsored(&sender, &sponsor, SUB_JSON.as_bytes(), ALPHA_USD)
        .await?;
    println!("setPushSub tx   {tx}");

    let back = registry::addr_push_sub_of(&addr).await?;
    match back {
        Some(s) if s == SUB_JSON => {
            println!("read-back       MATCH ({} bytes) — gas fix PROVEN live", s.len());
            Ok(())
        }
        Some(s) => Err(format!("read-back MISMATCH: got {} bytes", s.len())),
        None => Err("read-back EMPTY — the write reverted (out-of-gas again?)".into()),
    }
}

fn dirs_path() -> std::path::PathBuf {
    std::env::var("LOCALHARNESS_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("USERPROFILE")
                .or_else(|_| std::env::var("HOME"))
                .unwrap_or_default();
            std::path::PathBuf::from(home).join(".localharness")
        })
}
