//! SolidityLite Installment 1 — prove MAPPINGS + PARAMS + msg.sender from
//! source work on-chain. Compiles a `Ledger` facet in-crate, deploys it, calls
//! `add(amt)` (a uint256 param) which writes `bal[msg.sender] += amt`, and reads
//! `balanceOf(who)` (a mapping read keyed by an address param). Asserts the
//! per-caller mapping balance advances across calls.
//!
//! Run:
//!   EVM_PRIVATE_KEY=0x<caller> SPONSOR_PRIVATE_KEY=0x<sponsor> \
//!     cargo run --example soliditylite_mapping_e2e --features wallet

use k256::ecdsa::SigningKey;
use localharness::registry;
use localharness::soliditylite::compile;
use localharness::tempo_tx::{TempoCall, TempoTxBuilder};
use localharness::wallet;

const ALPHA_USD: &str = "0x20c0000000000000000000000000000000000001";
const SRC: &str = "facet Ledger { mapping(address => uint256) bal; function add(uint256 amt) external { bal[msg.sender] = bal[msg.sender] + amt; } function balanceOf(address who) external view returns (uint256) { return bal[who]; } }";
const ADD_SEL: [u8; 4] = [0x10, 0x03, 0xe2, 0xd2]; // add(uint256)

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sender = key_from_env("EVM_PRIVATE_KEY")?;
    let sponsor = key_from_env("SPONSOR_PRIVATE_KEY")?;
    let me = wallet::address(&sender);

    let art = compile(SRC).map_err(|e| format!("compile error: {e:?}"))?;
    println!("compiled Ledger: init {} bytes / runtime {} bytes", art.init_code.len(), art.runtime.len());

    // Deploy via sponsored CREATE.
    let nonce = registry::next_nonce(&format!("0x{}", hex(&me))).await?;
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
    let f = wallet::sign_hash(&sponsor, &tx.fee_payer_hash(&me));
    let raw = format!("0x{}", hex(&tx.serialize_signed(&s, Some(&f))));
    let dtx = rpc_str("eth_sendRawTransaction", serde_json::json!([raw])).await?;
    let rcpt = poll_receipt(&dtx).await?;
    let ledger = rcpt.get("contractAddress").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let dstatus = rcpt.get("status").and_then(|v| v.as_str()).unwrap_or("");
    println!("deploy status={dstatus} ledger={ledger}");
    if dstatus != "0x1" || ledger.is_empty() {
        println!("\n❌ Ledger did not deploy.");
        return Ok(());
    }

    let me_hex = hex(&me);
    let before = balance_of(&ledger, &me_hex).await?;
    println!("balanceOf(me) before = {before}");

    add(&sender, &sponsor, &ledger, 5).await?;
    let after5 = balance_of(&ledger, &me_hex).await?;
    println!("after add(5)  = {after5}");
    add(&sender, &sponsor, &ledger, 3).await?;
    let after8 = balance_of(&ledger, &me_hex).await?;
    println!("after add(3)  = {after8}");

    if before == 0 && after5 == 5 && after8 == 8 {
        println!("\n✅ VERDICT: SolidityLite mappings + uint256/address params + msg.sender work on the real Tempo EVM — bal[msg.sender] += amt persists per-caller (0 -> 5 -> 8), balanceOf(who) reads the keccak-derived mapping slot. The CounterFacet-grade primitives compile from SOURCE.");
    } else {
        println!("\n⚠️  before={before} after5={after5} after8={after8} (expected 0,5,8).");
    }
    Ok(())
}

async fn add(sender: &SigningKey, sponsor: &SigningKey, ledger: &str, amt: u8) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = ADD_SEL.to_vec();
    let mut w = [0u8; 32];
    w[31] = amt;
    input.extend_from_slice(&w);
    let h = registry::submit_tempo_sponsored(
        sender, sponsor,
        vec![TempoCall { to: parse_addr(ledger)?, value_wei: 0, input }],
        ALPHA_USD, 4_000_000,
    ).await?;
    println!("  add({amt}) tx: {h}");
    Ok(())
}

async fn balance_of(ledger: &str, who_hex: &str) -> Result<u128, Box<dyn std::error::Error>> {
    let data = format!("0x70a08231{who_hex:0>64}"); // balanceOf(address)
    let r = rpc_str("eth_call", serde_json::json!([{"to": ledger, "data": data}, "latest"])).await?;
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
