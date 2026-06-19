//! SolidityLite Installment 0 — child-diamond GENESIS, live on Moderato.
//!
//! Deploys a fresh EIP-2535 `Diamond` OWNED BY the deployer (an agent's own
//! key), seeded with the production Cut/Loupe/Ownership facets (reused as
//! stateless code, delegatecalled in the child's own storage). This is the
//! per-agent SANDBOX diamond the SolidityLite safety model is built on: an
//! agent can cut its OWN diamond freely, never the canonical one.
//!
//! Genesis init-code = `Diamond` creation bytecode (forge) ‖
//! `encode_diamond_constructor_args(owner, [cut,loupe,own])`, deployed via the
//! sponsored Tempo 0x76 CREATE path (TempoTxBuilder::create()).
//!
//! Run:
//!   EVM_PRIVATE_KEY=0x<owner> SPONSOR_PRIVATE_KEY=0x<sponsor> \
//!     cargo run --example child_diamond_genesis --features wallet

use k256::ecdsa::SigningKey;
use localharness::registry::{self, encode_diamond_constructor_args, FacetCut};
use localharness::tempo_tx::{TempoCall, TempoTxBuilder};
use localharness::wallet;

const ALPHA_USD: &str = "0x20c0000000000000000000000000000000000001";

// Production stateless facets to seed the child with (queried from the canonical
// diamond's loupe; reused as code, run in the CHILD's storage via delegatecall).
const CUT_FACET: &str = "0xC311D0d06847eF2ba5BdBA9b64F6BAb2f89D9C89";
const LOUPE_FACET: &str = "0x28577026cDEeAb9b9E723666e16c530b94c9EED3";
const OWN_FACET: &str = "0x9D157FaAEB76956986aAc1b96afCE9Efe0D1CEc4";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sender = key_from_env("EVM_PRIVATE_KEY")?; // becomes the child diamond's owner
    let sponsor = key_from_env("SPONSOR_PRIVATE_KEY")?;
    let owner = wallet::address(&sender);
    println!("owner (sender):     0x{}", hex(&owner));
    println!("fee_payer (sponsor): 0x{}", hex(&wallet::address(&sponsor)));

    // Diamond creation bytecode from forge output.
    let artifact = std::fs::read_to_string("contracts/out/Diamond.sol/Diamond.json")
        .map_err(|e| format!("read Diamond.json (run `forge build` in contracts/): {e}"))?;
    let json: serde_json::Value = serde_json::from_str(&artifact)?;
    let bytecode_hex = json["bytecode"]["object"].as_str().ok_or("no bytecode.object")?;
    let mut init_code = hex_decode(bytecode_hex.trim_start_matches("0x"))?;
    println!("Diamond creation bytecode: {} bytes", init_code.len());

    // Genesis cut: the three core facets with their exact selector sets.
    let cuts = vec![
        FacetCut { facet: parse_addr(CUT_FACET)?, action: 0, selectors: vec![[0x1f, 0x93, 0x1c, 0x1c]] },
        FacetCut {
            facet: parse_addr(LOUPE_FACET)?,
            action: 0,
            selectors: vec![
                [0x7a, 0x0e, 0xd6, 0x27], // facets()
                [0xad, 0xfc, 0xa1, 0x5e], // facetFunctionSelectors(address)
                [0x52, 0xef, 0x6b, 0x2c], // facetAddresses()
                [0xcd, 0xff, 0xac, 0xc6], // facetAddress(bytes4)
                [0x01, 0xff, 0xc9, 0xa7], // supportsInterface(bytes4)
            ],
        },
        FacetCut {
            facet: parse_addr(OWN_FACET)?,
            action: 0,
            selectors: vec![[0xf2, 0xfd, 0xe3, 0x8b], [0x8d, 0xa5, 0xcb, 0x5b]], // transferOwnership, owner
        },
    ];
    let ctor_args = encode_diamond_constructor_args(&owner, &cuts);
    init_code.extend_from_slice(&ctor_args);
    println!("constructor args: {} bytes -> total init-code {} bytes", ctor_args.len(), init_code.len());

    let nonce = registry::next_nonce(&format!("0x{}", hex(&owner))).await?;
    let gas_price = registry::current_gas_price().await?;
    println!("nonce={nonce} gas_price={gas_price}");

    let tx = TempoTxBuilder::new(registry::CHAIN_ID())
        .max_priority_fee_per_gas(gas_price)
        .max_fee_per_gas(gas_price)
        .gas_limit(25_000_000)
        .nonce(nonce)
        .fee_token(parse_addr(ALPHA_USD)?)
        .call(TempoCall { to: [0u8; 20], value_wei: 0, input: init_code })
        .sponsored()
        .create()
        .build();

    let sender_sig = wallet::sign_hash(&sender, &tx.sender_hash());
    let fp_sig = wallet::sign_hash(&sponsor, &tx.fee_payer_hash(&owner));
    let raw = format!("0x{}", hex(&tx.serialize_signed(&sender_sig, Some(&fp_sig))));

    let tx_hash = match rpc_str("eth_sendRawTransaction", serde_json::json!([raw])).await {
        Ok(h) => h,
        Err(e) => { println!("\n❌ genesis REJECTED at submit — {e}"); return Ok(()); }
    };
    println!("tx_hash: {tx_hash}");
    let receipt = poll_receipt(&tx_hash).await?;
    let status = receipt.get("status").and_then(|v| v.as_str()).unwrap_or("");
    let child = receipt.get("contractAddress").and_then(|v| v.as_str()).unwrap_or("");
    println!("receipt status={status} child diamond={child}");
    if status != "0x1" || child.is_empty() {
        println!("\n❌ genesis did not produce a diamond (status {status}).");
        return Ok(());
    }

    // Verify the child is a working diamond: owner() + facetAddress(diamondCut).
    let owner_ret = rpc_str("eth_call", serde_json::json!([{"to": child, "data": "0x8da5cb5b"}, "latest"])).await?;
    // facetAddress(bytes4) = 0xcdffacc6; the bytes4 arg (0x1f931c1c) is left-aligned in its word.
    let fa_data = format!("0xcdffacc61f931c1c{}", "0".repeat(56));
    let cut_facet_ret = rpc_str("eth_call", serde_json::json!([{"to": child, "data": fa_data}, "latest"])).await?;
    let owner_lc = owner_ret.to_lowercase();
    let owner_ok = owner_lc.ends_with(&hex(&owner));
    let cut_ok = cut_facet_ret.to_lowercase().ends_with(&CUT_FACET.trim_start_matches("0x").to_lowercase());
    println!("owner() -> {owner_ret}  (matches deployer: {owner_ok})");
    println!("facetAddress(diamondCut) -> {cut_facet_ret}  (== prod CutFacet: {cut_ok})");
    if owner_ok && cut_ok {
        println!("\n✅ VERDICT: a child diamond OWNED BY the agent is LIVE at {child}.");
        println!("   owner() returns the deployer + diamondCut routes to the prod CutFacet — it is cuttable + loupe-verifiable. SolidityLite Installment 0 genesis works.");
    } else {
        println!("\n⚠️  deployed at {child} but a verification check failed — inspect above.");
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
    let mut out = [0u8; 20];
    out.copy_from_slice(&b);
    Ok(out)
}

fn hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() }
fn hex_decode(s: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if s.len() % 2 != 0 { return Err("odd-length hex".into()); }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(Into::into)).collect()
}
