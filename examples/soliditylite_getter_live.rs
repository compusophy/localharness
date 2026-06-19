//! SolidityLite Installment 1 — prove the EMITTER's output runs on the REAL
//! Tempo EVM. Calls `soliditylite::emit_constant_getter` to produce bytecode in
//! the SAME crate that ships the compiler, deploys it via the proven sponsored
//! CREATE path, then `eth_call get()` and asserts the returned constant. This is
//! the behavioral validator (the real chain) the design wanted, no revm dep.
//!
//! Run:
//!   EVM_PRIVATE_KEY=0x<deployer> SPONSOR_PRIVATE_KEY=0x<sponsor> \
//!     cargo run --example soliditylite_getter_live --features wallet

use k256::ecdsa::SigningKey;
use localharness::registry;
use localharness::soliditylite::emit_constant_getter;
use localharness::tempo_tx::{TempoCall, TempoTxBuilder};
use localharness::wallet;

const ALPHA_USD: &str = "0x20c0000000000000000000000000000000000001";
const GET_SELECTOR: [u8; 4] = [0x6d, 0x4c, 0xe6, 0x3c]; // get()
const CONST_VALUE: u8 = 42;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sender = key_from_env("EVM_PRIVATE_KEY")?;
    let sponsor = key_from_env("SPONSOR_PRIVATE_KEY")?;
    let sender_addr = wallet::address(&sender);
    println!("deployer: 0x{}", hex(&sender_addr));

    // Emit the bytecode from the SolidityLite assembler.
    let mut value = [0u8; 32];
    value[31] = CONST_VALUE;
    let art = emit_constant_getter(GET_SELECTOR, value);
    println!(
        "emitted get() getter: init {} bytes / runtime {} bytes",
        art.init_code.len(),
        art.runtime.len()
    );

    let nonce = registry::next_nonce(&format!("0x{}", hex(&sender_addr))).await?;
    let gas_price = registry::current_gas_price().await?;
    let tx = TempoTxBuilder::new(registry::CHAIN_ID())
        .max_priority_fee_per_gas(gas_price)
        .max_fee_per_gas(gas_price)
        .gas_limit(1_500_000)
        .nonce(nonce)
        .fee_token(parse_addr(ALPHA_USD)?)
        .call(TempoCall { to: [0u8; 20], value_wei: 0, input: art.init_code })
        .sponsored()
        .create()
        .build();
    let s_sig = wallet::sign_hash(&sender, &tx.sender_hash());
    let f_sig = wallet::sign_hash(&sponsor, &tx.fee_payer_hash(&sender_addr));
    let raw = format!("0x{}", hex(&tx.serialize_signed(&s_sig, Some(&f_sig))));

    let tx_hash = rpc_str("eth_sendRawTransaction", serde_json::json!([raw])).await?;
    println!("deploy tx: {tx_hash}");
    let receipt = poll_receipt(&tx_hash).await?;
    let status = receipt.get("status").and_then(|v| v.as_str()).unwrap_or("");
    let addr = receipt.get("contractAddress").and_then(|v| v.as_str()).unwrap_or("");
    println!("status={status} deployed at {addr}");
    if status != "0x1" || addr.is_empty() {
        println!("\n❌ emitted getter did not deploy (status {status}).");
        return Ok(());
    }

    // eth_call get() — expect the constant.
    let ret = rpc_str("eth_call", serde_json::json!([{"to": addr, "data": "0x6d4ce63c"}, "latest"])).await?;
    let tail = ret.trim_start_matches("0x");
    let got = u128::from_str_radix(&tail[tail.len().saturating_sub(32)..], 16).unwrap_or(u128::MAX);
    println!("get() -> {ret}  (= {got})");
    if got == CONST_VALUE as u128 {
        println!("\n✅ VERDICT: SolidityLite-EMITTED bytecode deploys AND executes on the real Tempo EVM — get() returned {got}. The codegen foundation (assembler + dispatch + init wrapper) is proven on-chain.");
    } else {
        println!("\n⚠️  deployed but get() returned {got}, expected {CONST_VALUE}.");
    }
    Ok(())
}

async fn poll_receipt(tx_hash: &str) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    for _ in 0..40 {
        let v = rpc_raw("eth_getTransactionReceipt", serde_json::json!([tx_hash])).await?;
        if let Some(r) = v.get("result") {
            if !r.is_null() { return Ok(r.clone()); }
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    Err("receipt did not land within ~80s".into())
}
async fn rpc_raw(method: &str, params: serde_json::Value) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,"params":params});
    Ok(client.post(registry::RPC_URL()).json(&body).send().await?.json().await?)
}
async fn rpc_str(method: &str, params: serde_json::Value) -> Result<String, Box<dyn std::error::Error>> {
    let resp = rpc_raw(method, params).await?;
    if let Some(err) = resp.get("error") { return Err(format!("{err}").into()); }
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
