//! Live-test the Tempo Tx 0x76 encoder against Tempo Moderato.
//!
//! Three escalating scenarios. Each step verifies one layer of the
//! wire format before moving on:
//!
//! 1. **Self-paid native** — `fee_token = None`. Deployer wallet
//!    pays its own fees in native. Verifies the basic 0x76 envelope
//!    + sender signature.
//! 2. **Self-paid $LH** — `fee_token = LOCALHARNESS_TOKEN_ADDRESS()`.
//!    Deployer pays its own fees in $LH. Verifies the fee_token slot.
//! 3. **Sponsored $LH** — fresh sender with zero balance; deployer
//!    signs as fee_payer; fees paid in $LH from the deployer's
//!    balance. Verifies the dual-sign flow.
//!
//! Run with:
//!   EVM_PRIVATE_KEY=0x...  cargo run --example tempo_tx_live --features wallet
//!
//! Each step prints the raw tx bytes and the RPC response so you can
//! see exactly what the chain rejected (if anything). Failed steps
//! halt the chain — fix the encoder and re-run.

use k256::ecdsa::SigningKey;
use localharness::registry;
use localharness::tempo_tx::{TempoCall, TempoTxBuilder};
use localharness::wallet;
use sha3::{Digest, Keccak256};

const TOKEN_ADDRESS: &str = "0xcC8A300658dC8d0648D984A5066Af3F8E75e0936";
// Tempo's native TIP-20 stablecoins (Moderato testnet). Auto-funded by
// `tempo_fundAddress`. Used as fee_token candidates — $LH is NOT TIP-20
// and the chain rejects it with FeeTokenNotTip20Error.
const ALPHA_USD: &str = "0x20c0000000000000000000000000000000000001";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pk_hex = std::env::var("EVM_PRIVATE_KEY")
        .map_err(|_| "set EVM_PRIVATE_KEY=0x... in the environment")?;
    let signer = parse_signing_key(&pk_hex)?;
    let address = wallet::address(&signer);
    let address_hex = hex_address(&address);

    println!("deployer: {address_hex}");
    let nonce = registry::next_nonce(&address_hex).await?;
    let gas_price = registry::current_gas_price().await?;
    println!("nonce={nonce} gas_price={gas_price}");

    // Step 1 — self-paid native, simplest possible tempo tx.
    println!("\n=== step 1: self-paid native ===");
    let nonce = step_self_paid_native(&signer, &address_hex, nonce, gas_price).await?;
    println!("[step 1 ok] next nonce: {nonce}");

    // Step 2 — self-paid $LH. The deployer holds $LH (faucet'd) so
    // this should work if Tempo accepts the token as fee_token.
    println!("\n=== step 2: self-paid $LH ===");
    step_self_paid_lh(&signer, &address_hex, nonce, gas_price).await?;
    println!("[step 2 ok]");

    // Step 3 — sponsored: a fresh keypair (zero balance) is the
    // sender; the deployer signs as fee_payer; fees paid in AlphaUSD
    // from the deployer's stash. The call: faucet our LH token to
    // the fresh sender so they end up with $LH afterward.
    println!("\n=== step 3: sponsored (fresh sender + deployer fee_payer) ===");
    step_sponsored_lh(&signer).await?;
    println!("[step 3 ok]");
    Ok(())
}

async fn step_sponsored_lh(
    fee_payer_signer: &SigningKey,
) -> Result<(), Box<dyn std::error::Error>> {
    // Generate a brand-new sender with zero of everything. Verifies
    // the user truly doesn't need any balance — fee_payer's
    // sponsorship covers it.
    let mut rng_bytes = [0u8; 32];
    use rand_core::RngCore;
    rand_core::OsRng.fill_bytes(&mut rng_bytes);
    let sender_signer = SigningKey::from_slice(&rng_bytes)?;
    let sender_addr = wallet::address(&sender_signer);
    let sender_hex = hex_address(&sender_addr);
    println!("fresh sender: {sender_hex}");

    // Fresh sender's nonce is 0 (never used).
    let nonce = 0u128;
    let gas_price = registry::current_gas_price().await?;

    // Call: faucet LH to the fresh sender. After this tx, sender has
    // $LH but never held any native or any stablecoin themselves.
    let calldata = encode_address_call("faucet(address)", &sender_addr);
    let call = TempoCall {
        to: parse_address(TOKEN_ADDRESS)?,
        value_wei: 0,
        input: calldata,
    };

    let tx = TempoTxBuilder::new(registry::CHAIN_ID())
        .max_priority_fee_per_gas(gas_price)
        .max_fee_per_gas(gas_price)
        // Sponsored txs cost more — fee_payer signature recovery +
        // policy checks add ~70k gas. Budget for it.
        .gas_limit(400_000)
        .nonce(nonce)
        .fee_token(parse_address(ALPHA_USD)?)
        .call(call)
        .sponsored()
        .build();

    // Sender signs the sender hash. They commit to the intent (call to
    // faucet) without committing to which token covers fees — the
    // fee_payer picks that.
    let sender_hash = tx.sender_hash();
    println!("sender_hash: 0x{}", hex(&sender_hash));
    let sender_sig = wallet::sign_hash(&sender_signer, &sender_hash);

    // Fee payer signs the fee_payer hash, which includes the sender's
    // address + the fee_token. The chain ensures the fee_payer
    // explicitly authorized this exact (sender, intent, token) triple.
    let fp_hash = tx.fee_payer_hash(&sender_addr);
    println!("fee_payer_hash: 0x{}", hex(&fp_hash));
    let fp_sig = wallet::sign_hash(fee_payer_signer, &fp_hash);

    let serialized = tx.serialize_signed(&sender_sig, Some(&fp_sig));
    let raw_hex = format!("0x{}", hex(&serialized));
    println!(
        "raw tx ({} bytes): {}...",
        serialized.len(),
        &raw_hex[..raw_hex.len().min(120)]
    );

    let tx_hash = submit(&raw_hex).await?;
    println!("tx_hash: {tx_hash}");
    Ok(())
}

fn encode_address_call(signature: &str, addr: &[u8; 20]) -> Vec<u8> {
    let mut hasher = Keccak256::new();
    hasher.update(signature.as_bytes());
    let digest = hasher.finalize();
    let mut selector = [0u8; 4];
    selector.copy_from_slice(&digest[..4]);
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(addr);
    let mut out = Vec::with_capacity(36);
    out.extend_from_slice(&selector);
    out.extend_from_slice(&padded);
    out
}

async fn step_self_paid_native(
    signer: &SigningKey,
    address_hex: &str,
    nonce: u128,
    gas_price: u128,
) -> Result<u128, Box<dyn std::error::Error>> {
    // Simplest call: read-only `balanceOf(self)` to the LH token.
    // Read-only because we want a CALL that's safe to attempt but
    // verifies the chain accepted the 0x76 envelope.
    let calldata = encode_balance_of(&parse_address(address_hex)?);
    let call = TempoCall {
        to: parse_address(TOKEN_ADDRESS)?,
        value_wei: 0,
        input: calldata,
    };

    let tx = TempoTxBuilder::new(registry::CHAIN_ID())
        .max_priority_fee_per_gas(gas_price)
        .max_fee_per_gas(gas_price)
        .gas_limit(100_000)
        .nonce(nonce)
        .call(call)
        .build();

    let sender_hash = tx.sender_hash();
    println!("sender_hash: 0x{}", hex(&sender_hash));
    let sig = wallet::sign_hash(signer, &sender_hash);
    let serialized = tx.serialize_signed(&sig, None);
    let raw_hex = format!("0x{}", hex(&serialized));
    println!("raw tx ({} bytes): {}", serialized.len(), &raw_hex[..raw_hex.len().min(120)]);

    let tx_hash = submit(&raw_hex).await?;
    println!("tx_hash: {tx_hash}");
    Ok(nonce + 1)
}

async fn step_self_paid_lh(
    signer: &SigningKey,
    address_hex: &str,
    nonce: u128,
    gas_price: u128,
) -> Result<(), Box<dyn std::error::Error>> {
    let calldata = encode_balance_of(&parse_address(address_hex)?);
    let call = TempoCall {
        to: parse_address(TOKEN_ADDRESS)?,
        value_wei: 0,
        input: calldata,
    };

    let tx = TempoTxBuilder::new(registry::CHAIN_ID())
        .max_priority_fee_per_gas(gas_price)
        .max_fee_per_gas(gas_price)
        .gas_limit(100_000)
        .nonce(nonce)
        // AlphaUSD is a native TIP-20 stablecoin Tempo accepts as
        // fee_token. $LH is not TIP-20 — chain returns
        // FeeTokenNotTip20Error if used.
        .fee_token(parse_address(ALPHA_USD)?)
        .call(call)
        .build();

    let sender_hash = tx.sender_hash();
    println!("sender_hash: 0x{}", hex(&sender_hash));
    let sig = wallet::sign_hash(signer, &sender_hash);
    let serialized = tx.serialize_signed(&sig, None);
    let raw_hex = format!("0x{}", hex(&serialized));
    println!("raw tx ({} bytes): {}", serialized.len(), &raw_hex[..raw_hex.len().min(120)]);

    let tx_hash = submit(&raw_hex).await?;
    println!("tx_hash: {tx_hash}");
    Ok(())
}

async fn submit(raw_hex: &str) -> Result<String, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_sendRawTransaction",
        "params": [raw_hex],
    });
    let resp: serde_json::Value = client
        .post(registry::RPC_URL())
        .json(&body)
        .send()
        .await?
        .json()
        .await?;
    if let Some(err) = resp.get("error") {
        return Err(format!("rpc error: {err}").into());
    }
    let hash = resp
        .get("result")
        .and_then(|v| v.as_str())
        .ok_or("no result field")?
        .to_string();
    Ok(hash)
}

fn parse_signing_key(hex_str: &str) -> Result<SigningKey, Box<dyn std::error::Error>> {
    let stripped = hex_str.trim().trim_start_matches("0x").trim_start_matches("0X");
    let bytes = hex_decode(stripped)?;
    SigningKey::from_slice(&bytes).map_err(Into::into)
}

fn parse_address(hex_str: &str) -> Result<[u8; 20], Box<dyn std::error::Error>> {
    let stripped = hex_str.trim().trim_start_matches("0x").trim_start_matches("0X");
    let bytes = hex_decode(stripped)?;
    if bytes.len() != 20 {
        return Err(format!("not a 20-byte address: {}", bytes.len()).into());
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn encode_balance_of(addr: &[u8; 20]) -> Vec<u8> {
    let mut hasher = Keccak256::new();
    hasher.update(b"balanceOf(address)");
    let digest = hasher.finalize();
    let mut selector = [0u8; 4];
    selector.copy_from_slice(&digest[..4]);
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(addr);
    let mut out = Vec::with_capacity(36);
    out.extend_from_slice(&selector);
    out.extend_from_slice(&padded);
    out
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn hex_address(addr: &[u8; 20]) -> String {
    format!("0x{}", hex(addr))
}

fn hex_decode(s: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if s.len() % 2 != 0 {
        return Err("odd-length hex".into());
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = nibble(bytes[i])?;
        let lo = nibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn nibble(b: u8) -> Result<u8, Box<dyn std::error::Error>> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(format!("non-hex byte {b}").into()),
    }
}
