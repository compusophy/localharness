//! SolidityLite Installment 1 — prove a COMPILED stateful facet's SSTORE
//! persists on-chain. Compiles a `Tally` facet FROM SOURCE in-crate, deploys it
//! via the proven sponsored CREATE path, calls `bump()` twice, then `eth_call
//! get()` and asserts the state advanced to 2. The behavioral proof that the
//! compiler's storage writes work on the real Tempo EVM.
//!
//! Run:
//!   EVM_PRIVATE_KEY=0x<deployer> SPONSOR_PRIVATE_KEY=0x<sponsor> \
//!     cargo run --example soliditylite_facet_e2e --features wallet

use k256::ecdsa::SigningKey;
use localharness::registry;
use localharness::soliditylite::compile;
use localharness::tempo_tx::{TempoCall, TempoTxBuilder};
use localharness::wallet;

const ALPHA_USD: &str = "0x20c0000000000000000000000000000000000001";
const SRC: &str = "facet Tally { uint256 n; function bump() external { n = n + 1; } function get() external view returns (uint256) { return n; } }";
const BUMP: [u8; 4] = [0x68, 0x11, 0x0b, 0x2f]; // bump()
const GET_DATA: &str = "0x6d4ce63c"; // get()

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sender = key_from_env("EVM_PRIVATE_KEY")?;
    let sponsor = key_from_env("SPONSOR_PRIVATE_KEY")?;
    let sender_addr = wallet::address(&sender);

    // 1. Compile the stateful facet FROM SOURCE.
    let art = compile(SRC).map_err(|e| format!("compile error: {e:?}"))?;
    println!("compiled Tally: init {} bytes / runtime {} bytes", art.init_code.len(), art.runtime.len());

    // 2. Deploy via sponsored CREATE.
    let nonce = registry::next_nonce(&format!("0x{}", hex(&sender_addr))).await?;
    let gas_price = registry::current_gas_price().await?;
    let tx = TempoTxBuilder::new(registry::CHAIN_ID)
        .max_priority_fee_per_gas(gas_price)
        .max_fee_per_gas(gas_price)
        .gas_limit(2_000_000)
        .nonce(nonce)
        .fee_token(parse_addr(ALPHA_USD)?)
        .call(TempoCall { to: [0u8; 20], value_wei: 0, input: art.init_code })
        .sponsored()
        .create()
        .build();
    let s = wallet::sign_hash(&sender, &tx.sender_hash());
    let f = wallet::sign_hash(&sponsor, &tx.fee_payer_hash(&sender_addr));
    let raw = format!("0x{}", hex(&tx.serialize_signed(&s, Some(&f))));
    let dtx = rpc_str("eth_sendRawTransaction", serde_json::json!([raw])).await?;
    let rcpt = poll_receipt(&dtx).await?;
    let facet = rcpt.get("contractAddress").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let dstatus = rcpt.get("status").and_then(|v| v.as_str()).unwrap_or("");
    println!("deploy status={dstatus} facet={facet}");
    if dstatus != "0x1" || facet.is_empty() {
        println!("\n❌ compiled facet did not deploy.");
        return Ok(());
    }

    // 3. get() before: expect 0.
    let before = read_u128(&facet).await?;
    println!("get() before = {before}");

    // 4. bump() twice (direct sponsored calls into the facet's own storage).
    for i in 1..=2 {
        let h = registry::submit_tempo_sponsored(
            &sender, &sponsor,
            vec![TempoCall { to: parse_addr(&facet)?, value_wei: 0, input: BUMP.to_vec() }],
            ALPHA_USD, 4_000_000,
        ).await?;
        println!("bump() #{i} tx: {h}");
    }

    // 5. get() after: expect 2.
    let after = read_u128(&facet).await?;
    println!("get() after = {after}");
    if before == 0 && after == 2 {
        println!("\n✅ VERDICT: a SolidityLite-COMPILED stateful facet works on the real Tempo EVM — bump() twice -> get()==2. The compiler's SSTORE/SLOAD/ADD from SOURCE persist state on-chain.");
    } else {
        println!("\n⚠️  before={before} after={after} (expected 0 then 2).");
    }
    Ok(())
}

async fn read_u128(addr: &str) -> Result<u128, Box<dyn std::error::Error>> {
    let r = rpc_str("eth_call", serde_json::json!([{"to": addr, "data": GET_DATA}, "latest"])).await?;
    let t = r.trim_start_matches("0x");
    Ok(u128::from_str_radix(&t[t.len().saturating_sub(32)..], 16).unwrap_or(u128::MAX))
}

async fn poll_receipt(tx_hash: &str) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    for _ in 0..40 {
        let v = rpc_raw("eth_getTransactionReceipt", serde_json::json!([tx_hash])).await?;
        if let Some(r) = v.get("result") {
            if !r.is_null() { return Ok(r.clone()); }
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    Err("receipt did not land".into())
}
async fn rpc_raw(method: &str, params: serde_json::Value) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,"params":params});
    Ok(client.post(registry::RPC_URL).json(&body).send().await?.json().await?)
}
async fn rpc_str(method: &str, params: serde_json::Value) -> Result<String, Box<dyn std::error::Error>> {
    let resp = rpc_raw(method, params).await?;
    if let Some(e) = resp.get("error") { return Err(format!("{e}").into()); }
    Ok(resp.get("result").and_then(|v| v.as_str()).unwrap_or("").to_string())
}
fn key_from_env(name: &str) -> Result<SigningKey, Box<dyn std::error::Error>> {
    let h = std::env::var(name).map_err(|_| format!("set {name}=0x..."))?;
    SigningKey::from_slice(&hex_decode(h.trim().trim_start_matches("0x").trim_start_matches("0X"))?).map_err(Into::into)
}
fn parse_addr(h: &str) -> Result<[u8; 20], Box<dyn std::error::Error>> {
    let b = hex_decode(h.trim().trim_start_matches("0x"))?;
    if b.len() != 20 { return Err("not a 20-byte address".into()); }
    let mut o = [0u8; 20];
    o.copy_from_slice(&b);
    Ok(o)
}
fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }
fn hex_decode(s: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if s.len() % 2 != 0 { return Err("odd-length hex".into()); }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(Into::into)).collect()
}
