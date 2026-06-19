//! SolidityLite Installment 1 — prove the CounterFacet LOGIC (mappings +
//! arithmetic + require/comparison) compiles from source and works on-chain.
//! Compiles the CounterFacet-core (the design's CounterFacet minus the event),
//! deploys it, and exercises: incrementBy(5) -> countOf==5; incrementBy(101)
//! REVERTS (require(n<=100)); increment() -> countOf==6, totalCount==6.
//!
//! Run:
//!   EVM_PRIVATE_KEY=0x<caller> SPONSOR_PRIVATE_KEY=0x<sponsor> \
//!     cargo run --example soliditylite_counter_e2e --features wallet

use k256::ecdsa::SigningKey;
use localharness::registry;
use localharness::soliditylite::compile;
use localharness::tempo_tx::{TempoCall, TempoTxBuilder};
use localharness::wallet;

const ALPHA_USD: &str = "0x20c0000000000000000000000000000000000001";
const SRC: &str = "facet Counter { mapping(address => uint256) count; uint256 total; function increment() external { count[msg.sender] = count[msg.sender] + 1; total = total + 1; } function incrementBy(uint256 n) external { require(n > 0, \"zero\"); require(n <= 100, \"too big\"); count[msg.sender] = count[msg.sender] + n; total = total + n; } function countOf(address who) external view returns (uint256) { return count[who]; } function totalCount() external view returns (uint256) { return total; } }";
const INCREMENT: [u8; 4] = [0xd0, 0x9d, 0xe0, 0x8a]; // increment()
const INCREMENT_BY: [u8; 4] = [0x03, 0xdf, 0x17, 0x9c]; // incrementBy(uint256)

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sender = key_from_env("EVM_PRIVATE_KEY")?;
    let sponsor = key_from_env("SPONSOR_PRIVATE_KEY")?;
    let me = wallet::address(&sender);
    let me_hex = hex(&me);

    let art = compile(SRC).map_err(|e| format!("compile error: {e:?}"))?;
    println!("compiled Counter-core: init {} bytes / runtime {} bytes", art.init_code.len(), art.runtime.len());

    // Deploy.
    let nonce = registry::next_nonce(&format!("0x{me_hex}")).await?;
    let gas_price = registry::current_gas_price().await?;
    let tx = TempoTxBuilder::new(registry::CHAIN_ID())
        .max_priority_fee_per_gas(gas_price).max_fee_per_gas(gas_price)
        .gas_limit(2_500_000).nonce(nonce).fee_token(parse_addr(ALPHA_USD)?)
        .call(TempoCall { to: [0u8; 20], value_wei: 0, input: art.init_code })
        .sponsored().create().build();
    let s = wallet::sign_hash(&sender, &tx.sender_hash());
    let f = wallet::sign_hash(&sponsor, &tx.fee_payer_hash(&me));
    let raw = format!("0x{}", hex(&tx.serialize_signed(&s, Some(&f))));
    let rcpt = poll_receipt(&rpc_str("eth_sendRawTransaction", serde_json::json!([raw])).await?).await?;
    let c = rcpt.get("contractAddress").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if rcpt.get("status").and_then(|v| v.as_str()).unwrap_or("") != "0x1" || c.is_empty() {
        println!("\n❌ Counter did not deploy."); return Ok(());
    }
    println!("deployed Counter at {c}");

    // incrementBy(5) -> countOf == 5
    call_by(&sender, &sponsor, &c, 5).await?;
    let v5 = count_of(&c, &me_hex).await?;
    println!("after incrementBy(5): countOf={v5}");

    // incrementBy(101) MUST revert (require(n<=100)).
    let reverted = call_by(&sender, &sponsor, &c, 101).await.is_err();
    let v_after_bad = count_of(&c, &me_hex).await?;
    println!("incrementBy(101): reverted={reverted}, countOf still {v_after_bad}");

    // increment() -> countOf == 6
    let h = registry::submit_tempo_sponsored(&sender, &sponsor,
        vec![TempoCall { to: parse_addr(&c)?, value_wei: 0, input: INCREMENT.to_vec() }], ALPHA_USD, 6_000_000).await?;
    println!("increment() tx: {h}");
    let v6 = count_of(&c, &me_hex).await?;
    let tot = read_u128(&c, "0x34eafb11").await?; // totalCount()
    println!("after increment(): countOf={v6}, totalCount={tot}");

    if v5 == 5 && reverted && v_after_bad == 5 && v6 == 6 && tot == 6 {
        println!("\n✅ VERDICT: the CounterFacet LOGIC compiles from SOURCE and runs on Moderato — incrementBy(5)->5, incrementBy(101) REVERTS (require(n<=100) enforced, state unchanged at 5), increment()->6, totalCount==6. mappings + arithmetic + require/comparison all work from source. Only events remain for the literal full CounterFacet.");
    } else {
        println!("\n⚠️  v5={v5} reverted={reverted} after_bad={v_after_bad} v6={v6} total={tot} (expected 5,true,5,6,6).");
    }
    Ok(())
}

async fn call_by(sender: &SigningKey, sponsor: &SigningKey, c: &str, n: u8) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = INCREMENT_BY.to_vec();
    let mut w = [0u8; 32]; w[31] = n;
    input.extend_from_slice(&w);
    registry::submit_tempo_sponsored(sender, sponsor,
        vec![TempoCall { to: parse_addr(c)?, value_wei: 0, input }], ALPHA_USD, 6_000_000).await?;
    Ok(())
}
async fn count_of(c: &str, who_hex: &str) -> Result<u128, Box<dyn std::error::Error>> {
    read_u128(c, &format!("0xf8977e96{who_hex:0>64}")).await
}
async fn read_u128(c: &str, data: &str) -> Result<u128, Box<dyn std::error::Error>> {
    let r = rpc_str("eth_call", serde_json::json!([{"to": c, "data": data}, "latest"])).await?;
    let t = r.trim_start_matches("0x");
    Ok(u128::from_str_radix(&t[t.len().saturating_sub(32)..], 16).unwrap_or(u128::MAX))
}
async fn poll_receipt(tx_hash: &str) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    for _ in 0..40 {
        let v = rpc_raw("eth_getTransactionReceipt", serde_json::json!([tx_hash])).await?;
        if let Some(r) = v.get("result") { if !r.is_null() { return Ok(r.clone()); } }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    Err("receipt did not land".into())
}
async fn rpc_raw(method: &str, params: serde_json::Value) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    Ok(client.post(registry::RPC_URL()).json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})).send().await?.json().await?)
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
    let mut o = [0u8; 20]; o.copy_from_slice(&b); Ok(o)
}
fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }
fn hex_decode(s: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if s.len() % 2 != 0 { return Err("odd-length hex".into()); }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(Into::into)).collect()
}
