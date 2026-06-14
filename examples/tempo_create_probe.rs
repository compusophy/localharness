//! Probe: does a SPONSORED Tempo 0x76 CONTRACT-CREATION tx deploy on Moderato?
//!
//! This is the gating experiment for the SolidityLite facet-deploy path
//! (`design/soliditylite.md` §10): `TempoCall` had no CREATE path. The encoder
//! now grows a `TempoTxBuilder::create()` flag that RLP-encodes the call's `to`
//! as empty (0x80) so `input` runs as init-code. This probe verifies the chain
//! actually deploys from such a tx and that the created address is recoverable
//! from the receipt — the single assumption the whole deploy stage rests on.
//!
//! Run:
//!   EVM_PRIVATE_KEY=0x<root> SPONSOR_PRIVATE_KEY=0x<sponsor> \
//!     cargo run --example tempo_create_probe --features wallet

use k256::ecdsa::SigningKey;
use localharness::registry;
use localharness::tempo_tx::{TempoCall, TempoTxBuilder};
use localharness::wallet;

// AlphaUSD — the TIP-20 fee_token the sponsor pays in (same as tempo_tx_live).
const ALPHA_USD: &str = "0x20c0000000000000000000000000000000000001";

// Minimal init-code that deploys a 1-byte runtime (0x00 = STOP):
//   PUSH1 0x01  PUSH1 0x0c  PUSH1 0x00  CODECOPY  PUSH1 0x01  PUSH1 0x00  RETURN | 0x00
// CODECOPY(dest=0, offset=12, len=1) copies the trailing 0x00 into memory; RETURN
// returns it as the runtime. extcodesize of the deployed contract = 1.
const INIT_CODE: [u8; 13] = [
    0x60, 0x01, 0x60, 0x0c, 0x60, 0x00, 0x39, 0x60, 0x01, 0x60, 0x00, 0xf3, 0x00,
];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sender = key_from_env("EVM_PRIVATE_KEY")?; // the deployer (signs intent)
    let sponsor = key_from_env("SPONSOR_PRIVATE_KEY")?; // pays fees as fee_payer
    let sender_addr = wallet::address(&sender);
    let sender_hex = hex_addr(&sender_addr);
    println!("deployer (sender):  {sender_hex}");
    println!("fee_payer (sponsor): {}", hex_addr(&wallet::address(&sponsor)));

    let nonce = registry::next_nonce(&sender_hex).await?;
    let gas_price = registry::current_gas_price().await?;
    println!("nonce={nonce} gas_price={gas_price}");

    let tx = TempoTxBuilder::new(registry::CHAIN_ID)
        .max_priority_fee_per_gas(gas_price)
        .max_fee_per_gas(gas_price)
        .gas_limit(600_000)
        .nonce(nonce)
        .fee_token(parse_addr(ALPHA_USD)?)
        .call(TempoCall { to: [0u8; 20], value_wei: 0, input: INIT_CODE.to_vec() })
        .sponsored()
        .create()
        .build();

    let sender_sig = wallet::sign_hash(&sender, &tx.sender_hash());
    let fp_sig = wallet::sign_hash(&sponsor, &tx.fee_payer_hash(&sender_addr));
    let raw = tx.serialize_signed(&sender_sig, Some(&fp_sig));
    let raw_hex = format!("0x{}", hex(&raw));
    println!("raw create tx: {} bytes", raw.len());

    let tx_hash = match rpc_str("eth_sendRawTransaction", serde_json::json!([raw_hex])).await {
        Ok(h) => h,
        Err(e) => {
            println!("\n❌ VERDICT: sponsored 0x76 CREATE REJECTED at submit — {e}");
            return Ok(());
        }
    };
    println!("tx_hash: {tx_hash}");

    let receipt = poll_receipt(&tx_hash).await?;
    let status = receipt.get("status").and_then(|v| v.as_str()).unwrap_or("");
    let created = receipt
        .get("contractAddress")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    println!("receipt status={status} contractAddress={created}");

    if status != "0x1" {
        println!("\n❌ VERDICT: CREATE tx mined but status != success ({status}).");
        return Ok(());
    }
    if created.is_empty() {
        println!("\n⚠️  VERDICT: mined OK but receipt has NO contractAddress — address recovery needs another path.");
        return Ok(());
    }
    let code = rpc_str("eth_getCode", serde_json::json!([created, "latest"])).await?;
    let code_len = code.trim_start_matches("0x").len() / 2;
    println!("eth_getCode({created}) = {code} ({code_len} byte runtime)");
    if code_len > 0 {
        println!("\n✅ VERDICT: sponsored 0x76 CONTRACT CREATION WORKS on Moderato.");
        println!("   Deployed a {code_len}-byte runtime at {created}. The SolidityLite deploy stage is unblocked.");
    } else {
        println!("\n⚠️  VERDICT: tx ok + address present but code is EMPTY — init-code returned nothing.");
    }
    Ok(())
}

async fn poll_receipt(tx_hash: &str) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    for _ in 0..40 {
        let v = rpc_raw("eth_getTransactionReceipt", serde_json::json!([tx_hash])).await?;
        if let Some(r) = v.get("result") {
            if !r.is_null() {
                return Ok(r.clone());
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    Err("receipt did not land within ~80s".into())
}

async fn rpc_raw(
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,"params":params});
    let resp: serde_json::Value =
        client.post(registry::RPC_URL).json(&body).send().await?.json().await?;
    Ok(resp)
}

async fn rpc_str(
    method: &str,
    params: serde_json::Value,
) -> Result<String, Box<dyn std::error::Error>> {
    let resp = rpc_raw(method, params).await?;
    if let Some(err) = resp.get("error") {
        return Err(format!("{err}").into());
    }
    Ok(resp.get("result").and_then(|v| v.as_str()).unwrap_or("").to_string())
}

fn key_from_env(name: &str) -> Result<SigningKey, Box<dyn std::error::Error>> {
    let h = std::env::var(name).map_err(|_| format!("set {name}=0x..."))?;
    let s = h.trim().trim_start_matches("0x").trim_start_matches("0X");
    SigningKey::from_slice(&hex_decode(s)?).map_err(Into::into)
}

fn parse_addr(h: &str) -> Result<[u8; 20], Box<dyn std::error::Error>> {
    let b = hex_decode(h.trim().trim_start_matches("0x"))?;
    if b.len() != 20 {
        return Err("not a 20-byte address".into());
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&b);
    Ok(out)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn hex_addr(a: &[u8; 20]) -> String {
    format!("0x{}", hex(a))
}
fn hex_decode(s: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if s.len() % 2 != 0 {
        return Err("odd-length hex".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(Into::into))
        .collect()
}
