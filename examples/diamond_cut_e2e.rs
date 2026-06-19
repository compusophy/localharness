//! SolidityLite Installment 0 — the CAPSTONE: cut a real facet INTO an
//! agent-owned child diamond, then call it through the diamond. Completes the
//! E2E loop: genesis -> deploy facet -> diamondCut -> loupe-verify -> call.
//!
//! Run:
//!   CHILD_DIAMOND=0x.. COUNTER_FACET=0x.. \
//!   EVM_PRIVATE_KEY=0x<child owner> SPONSOR_PRIVATE_KEY=0x<sponsor> \
//!     cargo run --example installment0_cut --features wallet

use k256::ecdsa::SigningKey;
use localharness::registry::{self, encode_diamond_cut, FacetCut};
use localharness::tempo_tx::TempoCall;
use localharness::wallet;

const ALPHA_USD: &str = "0x20c0000000000000000000000000000000000001";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let owner = key_from_env("EVM_PRIVATE_KEY")?;
    let sponsor = key_from_env("SPONSOR_PRIVATE_KEY")?;
    let owner_addr = wallet::address(&owner);
    let child = std::env::var("CHILD_DIAMOND")?;
    let counter = std::env::var("COUNTER_FACET")?;
    let child_b = parse_addr(&child)?;
    let counter_b = parse_addr(&counter)?;
    println!("child diamond: {child}\ncounter facet: {counter}\nowner: 0x{}", hex(&owner_addr));

    // 1. CUT — add the CounterFacet's 4 selectors into the child diamond.
    let cut = FacetCut {
        facet: counter_b,
        action: 0, // Add
        selectors: vec![
            [0xd0, 0x9d, 0xe0, 0x8a], // increment()
            [0x03, 0xdf, 0x17, 0x9c], // incrementBy(uint256)
            [0xf8, 0x97, 0x7e, 0x96], // countOf(address)
            [0x34, 0xea, 0xfb, 0x11], // totalCount()
        ],
    };
    let calldata = encode_diamond_cut(&[cut], &[0u8; 20], &[]);
    println!("\ndiamondCut: adding 4 CounterFacet selectors ({} calldata bytes) ...", calldata.len());
    let cut_tx = registry::submit_tempo_sponsored(
        &owner,
        &sponsor,
        vec![TempoCall { to: child_b, value_wei: 0, input: calldata }],
        ALPHA_USD,
        10_000_000,
    )
    .await?;
    println!("  cut tx: {cut_tx}");

    // 2. loupe-verify — facetAddress(increment) on the child == CounterFacet.
    let fa = format!("0xcdffacc6d09de08a{}", "0".repeat(56));
    let fa_ret = eth_call(&child, &fa).await?;
    let cut_ok = fa_ret.to_lowercase().ends_with(&counter.trim_start_matches("0x").to_lowercase());
    println!("  facetAddress(increment) -> {fa_ret}  (== CounterFacet: {cut_ok})");

    // 3. CALL — increment() through the child diamond's fallback -> CounterFacet.
    println!("\ncalling increment() through the child diamond ...");
    let inc_tx = registry::submit_tempo_sponsored(
        &owner,
        &sponsor,
        vec![TempoCall { to: child_b, value_wei: 0, input: vec![0xd0, 0x9d, 0xe0, 0x8a] }],
        ALPHA_USD,
        3_000_000,
    )
    .await?;
    println!("  increment tx: {inc_tx}");

    // 4. read countOf(owner) — should be >= 1.
    let co = format!("0xf8977e96000000000000000000000000{}", hex(&owner_addr));
    let co_ret = eth_call(&child, &co).await?;
    let tail = co_ret.trim_start_matches("0x");
    let count = u128::from_str_radix(&tail[tail.len().saturating_sub(32)..], 16).unwrap_or(0);
    println!("  countOf(owner) -> {co_ret}  (= {count})");

    if cut_ok && count >= 1 {
        println!(
            "\n✅ VERDICT: Installment 0 E2E COMPLETE. An agent-owned child diamond had a facet \
             CUT into it and the new selector executes through the diamond (countOf={count}). \
             The full deploy -> diamondCut -> call loop works end to end on Moderato."
        );
    } else {
        println!("\n⚠️  partial — cut_ok={cut_ok} count={count}; inspect above.");
    }
    Ok(())
}

async fn eth_call(to: &str, data: &str) -> Result<String, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"eth_call","params":[{"to":to,"data":data},"latest"]});
    let resp: serde_json::Value = client.post(registry::RPC_URL()).json(&body).send().await?.json().await?;
    if let Some(e) = resp.get("error") {
        return Err(format!("{e}").into());
    }
    Ok(resp.get("result").and_then(|v| v.as_str()).unwrap_or("").to_string())
}

fn key_from_env(name: &str) -> Result<SigningKey, Box<dyn std::error::Error>> {
    let h = std::env::var(name).map_err(|_| format!("set {name}=0x..."))?;
    SigningKey::from_slice(&hex_decode(h.trim().trim_start_matches("0x").trim_start_matches("0X"))?).map_err(Into::into)
}
fn parse_addr(h: &str) -> Result<[u8; 20], Box<dyn std::error::Error>> {
    let b = hex_decode(h.trim().trim_start_matches("0x"))?;
    if b.len() != 20 {
        return Err("not a 20-byte address".into());
    }
    let mut o = [0u8; 20];
    o.copy_from_slice(&b);
    Ok(o)
}
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
fn hex_decode(s: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if s.len() % 2 != 0 {
        return Err("odd-length hex".into());
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(Into::into)).collect()
}
