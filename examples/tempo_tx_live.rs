//! Live-test the Tempo Tx 0x76 encoder against Tempo Moderato.
//!
//! Three escalating scenarios. Each step verifies one layer of the
//! wire format before moving on:
//!
//! 1. **Self-paid native** — `fee_token = None`. Deployer wallet
//!    pays its own fees in native. Verifies the basic 0x76 envelope
//!    + sender signature.
//! 2. **Self-paid, explicit fee_token** — a USD-currency TIP-20
//!    DISCOVERED live via `currency()` (T7 rejects anything else, and
//!    fixture addresses go stale). Verifies the fee_token slot.
//! 3. **Sponsored** — fresh sender with zero balance; deployer signs as
//!    fee_payer; fees paid in the discovered fee_token from the
//!    deployer's balance. Verifies the dual-sign flow.
//!
//! If NO candidate reports `currency() == "USD"`, steps 2/3 are skipped
//! with an honest message (step 1 still proves the envelope).
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
// TIP-20 fee_token CANDIDATES, probed live via `currency()` at startup: T7
// requires a fee_token to be TIP-20 with `currency() == "USD"`, and the old
// hardcoded Moderato AlphaUSD slot (0x20c0…0001) now returns an EMPTY currency
// under T7 (FeeTokenNotUsdError) — so steps 2/3 discover a valid token instead
// of hard-failing on a stale fixture. First hit wins; none → honest skip.
// Candidates: the active chain's configured fee_token (chain.rs) is tried
// first, then these knowns (Moderato AlphaUSD, mainnet USDC.e).
const FEE_TOKEN_CANDIDATES: &[&str] = &[
    "0x20c0000000000000000000000000000000000001", // AlphaUSD (Moderato)
    "0x20c000000000000000000000b9537d11c60e8b50", // USDC.e (mainnet)
];

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

    // Steps 2/3 need a USD-currency TIP-20 fee_token. Discover one live —
    // the fixture list goes stale (T7 emptied the old AlphaUSD slot's
    // `currency()`), and a stale candidate must not hard-fail the example.
    let fee_token = match discover_usd_fee_token().await {
        Some(token) => token,
        None => {
            println!(
                "\nno USD-currency TIP-20 fee_token found among the candidates on this chain \
                 (T7 rejects non-USD fee tokens with FeeTokenNotUsdError) — \
                 skipping steps 2/3 (fee_token + sponsored flows). Step 1 verified the envelope."
            );
            return Ok(());
        }
    };
    println!("fee_token (currency()==\"USD\"): {fee_token}");

    // Step 2 — self-paid with an explicit fee_token slot. The deployer pays
    // its own fees in the discovered USD stablecoin.
    println!("\n=== step 2: self-paid, explicit fee_token ===");
    step_self_paid_lh(&signer, &address_hex, nonce, gas_price, &fee_token).await?;
    println!("[step 2 ok]");

    // Step 3 — sponsored: a fresh keypair (zero balance) is the
    // sender; the deployer signs as fee_payer; fees paid in the discovered
    // USD fee_token from the deployer's stash. The call: faucet our LH token
    // to the fresh sender so they end up with $LH afterward.
    println!("\n=== step 3: sponsored (fresh sender + deployer fee_payer) ===");
    step_sponsored_lh(&signer, &fee_token).await?;
    println!("[step 3 ok]");
    Ok(())
}

/// Probe the fee_token candidates (the active chain's configured one first,
/// then the known stablecoin slots) via `currency()` and return the first
/// that reports "USD" — the T7 fee_token eligibility rule. `None` when no
/// candidate qualifies (each miss is printed honestly, never a hard fail).
async fn discover_usd_fee_token() -> Option<String> {
    let mut candidates = vec![registry::ALPHA_USD_ADDRESS().to_string()];
    for c in FEE_TOKEN_CANDIDATES {
        if !candidates.iter().any(|k| k.eq_ignore_ascii_case(c)) {
            candidates.push((*c).to_string());
        }
    }
    for candidate in candidates {
        match token_currency(&candidate).await {
            Ok(cur) if cur == "USD" => return Some(candidate),
            Ok(cur) => println!("candidate {candidate}: currency()={cur:?} (not \"USD\") — skip"),
            Err(e) => println!("candidate {candidate}: currency() call failed ({e}) — skip"),
        }
    }
    None
}

/// `currency()` on a TIP-20 token via `eth_call`, ABI-decoded as a string.
async fn token_currency(token: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut hasher = Keccak256::new();
    hasher.update(b"currency()");
    let digest = hasher.finalize();
    let calldata = format!("0x{}", hex(&digest[..4]));
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_call",
        "params": [{"to": token, "data": calldata}, "latest"],
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
    let result = resp.get("result").and_then(|v| v.as_str()).ok_or("no result field")?;
    let bytes = hex_decode(result.trim_start_matches("0x"))?;
    // ABI `string` return: word 0 = offset, word 1 = length, then the bytes.
    if bytes.len() < 64 {
        return Err(format!("return too short for a string ({} bytes)", bytes.len()).into());
    }
    let len = usize::try_from(u64::from_be_bytes(bytes[56..64].try_into()?))?;
    if bytes.len() < 64 + len {
        return Err("string length exceeds returndata".into());
    }
    Ok(String::from_utf8_lossy(&bytes[64..64 + len]).into_owned())
}

async fn step_sponsored_lh(
    fee_payer_signer: &SigningKey,
    fee_token: &str,
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
        // policy checks add ~70k gas, and the T7 faucet mint alone
        // estimates ~545k; live sponsored gasUsed was 789,322
        // (400k OOG'd, 2026-07-05). Budget with real headroom.
        .gas_limit(1_000_000)
        .nonce(nonce)
        .fee_token(parse_address(fee_token)?)
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
    fee_token: &str,
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
        // The discovered USD-currency TIP-20 (probed via `currency()` at
        // startup). $LH is not eligible — its currency() is "credits", and
        // the chain rejects non-USD fee tokens (FeeTokenNotUsdError).
        .fee_token(parse_address(fee_token)?)
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
