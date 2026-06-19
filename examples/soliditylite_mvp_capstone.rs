//! SolidityLite Installment 1 — THE CAPSTONE. The literal end-to-end MVP:
//! an agent (1) WRITES a facet in source, (2) COMPILES it in-crate, (3) DEPLOYS
//! it, (4) GENESISes a fresh diamond it OWNS, (5) CUTS the compiled facet into
//! that diamond, and (6) CALLS it through the diamond — verifying state + the
//! event log. Every step is individually proven (ticks 1-9); this assembles
//! them with the SolidityLite-COMPILED CounterFacet.
//!
//! Run:
//!   EVM_PRIVATE_KEY=0x<owner> SPONSOR_PRIVATE_KEY=0x<sponsor> \
//!     cargo run --example soliditylite_mvp_capstone --features wallet

use k256::ecdsa::SigningKey;
use localharness::registry::{self, encode_diamond_constructor_args, encode_diamond_cut, FacetCut};
use localharness::soliditylite::compile;
use localharness::tempo_tx::{TempoCall, TempoTxBuilder};
use localharness::wallet;

const ALPHA_USD: &str = "0x20c0000000000000000000000000000000000001";
// Prod stateless facets to seed a child diamond (reused as code via delegatecall).
const CUT_FACET: &str = "0xC311D0d06847eF2ba5BdBA9b64F6BAb2f89D9C89";
const LOUPE_FACET: &str = "0x28577026cDEeAb9b9E723666e16c530b94c9EED3";
const OWN_FACET: &str = "0x9D157FaAEB76956986aAc1b96afCE9Efe0D1CEc4";
const TOPIC0: &str = "0xcd5ad702c30bb253c9e421ea7f3e00faee62ce859708bfdaf949788e5ba0fdb5";
const SRC: &str = "facet CounterFacet { mapping(address => uint256) count; uint256 total; event Incremented(address indexed who, uint256 newCount, uint256 newTotal); function increment() external { count[msg.sender] = count[msg.sender] + 1; total = total + 1; emit Incremented(msg.sender, count[msg.sender], total); } function incrementBy(uint256 n) external { require(n > 0, \"zero\"); require(n <= 100, \"too big\"); count[msg.sender] = count[msg.sender] + n; total = total + n; emit Incremented(msg.sender, count[msg.sender], total); } function countOf(address who) external view returns (uint256) { return count[who]; } function totalCount() external view returns (uint256) { return total; } }";
// CounterFacet selectors.
const INCREMENT: [u8; 4] = [0xd0, 0x9d, 0xe0, 0x8a];
const COUNTER_SELS: [[u8; 4]; 4] = [
    [0xd0, 0x9d, 0xe0, 0x8a], // increment()
    [0x03, 0xdf, 0x17, 0x9c], // incrementBy(uint256)
    [0xf8, 0x97, 0x7e, 0x96], // countOf(address)
    [0x34, 0xea, 0xfb, 0x11], // totalCount()
];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let owner = key_from_env("EVM_PRIVATE_KEY")?;
    let sponsor = key_from_env("SPONSOR_PRIVATE_KEY")?;
    let me = wallet::address(&owner);
    let me_hex = hex(&me);
    println!("agent (owner): 0x{me_hex}");

    // (1)+(2) WRITE + COMPILE the facet from source.
    let art = compile(SRC).map_err(|e| format!("compile: {e:?}"))?;
    println!("1-2. compiled CounterFacet from source: {} byte runtime", art.runtime.len());

    // (3) DEPLOY the compiled facet.
    let facet = create(&owner, &sponsor, &me, art.init_code, 3_000_000).await?;
    println!("3.  deployed facet at {facet}");

    // (4) GENESIS a fresh child diamond the agent OWNS.
    let mut genesis = std::fs::read_to_string("contracts/out/Diamond.sol/Diamond.json")
        .ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|j| j["bytecode"]["object"].as_str().map(|s| s.to_string()))
        .ok_or("read Diamond.json (run forge build)")?;
    let mut init = hex_decode(genesis.split_off(0).trim_start_matches("0x"))?;
    let gcuts = vec![
        FacetCut { facet: parse_addr(CUT_FACET)?, action: 0, selectors: vec![[0x1f, 0x93, 0x1c, 0x1c]] },
        FacetCut { facet: parse_addr(LOUPE_FACET)?, action: 0, selectors: vec![[0x7a,0x0e,0xd6,0x27],[0xad,0xfc,0xa1,0x5e],[0x52,0xef,0x6b,0x2c],[0xcd,0xff,0xac,0xc6],[0x01,0xff,0xc9,0xa7]] },
        FacetCut { facet: parse_addr(OWN_FACET)?, action: 0, selectors: vec![[0xf2,0xfd,0xe3,0x8b],[0x8d,0xa5,0xcb,0x5b]] },
    ];
    init.extend_from_slice(&encode_diamond_constructor_args(&me, &gcuts));
    let child = create(&owner, &sponsor, &me, init, 25_000_000).await?;
    println!("4.  genesis child diamond (owned by the agent) at {child}");

    // (5) CUT the compiled facet into the agent's diamond.
    let cut = FacetCut { facet: parse_addr(&facet)?, action: 0, selectors: COUNTER_SELS.to_vec() };
    let calldata = encode_diamond_cut(&[cut], &[0u8; 20], &[]);
    let ctx = registry::submit_tempo_sponsored(&owner, &sponsor,
        vec![TempoCall { to: parse_addr(&child)?, value_wei: 0, input: calldata }], ALPHA_USD, 12_000_000).await?;
    println!("5.  cut CounterFacet into the diamond (tx {ctx})");
    let fa = rpc_str("eth_call", serde_json::json!([{"to": child, "data": format!("0xcdffacc6d09de08a{}", "0".repeat(56))}, "latest"])).await?;
    let cut_ok = fa.to_lowercase().ends_with(&facet.trim_start_matches("0x").to_lowercase());
    println!("    loupe facetAddress(increment) == compiled facet: {cut_ok}");

    // (6) CALL increment() THROUGH the diamond.
    let itx = registry::submit_tempo_sponsored(&owner, &sponsor,
        vec![TempoCall { to: parse_addr(&child)?, value_wei: 0, input: INCREMENT.to_vec() }], ALPHA_USD, 8_000_000).await?;
    let ircpt = poll_receipt(&itx).await?;
    let cnt = read_u128(&child, &format!("0xf8977e96{me_hex:0>64}")).await?;
    let tot = read_u128(&child, "0x34eafb11").await?;
    println!("6.  increment() through the diamond → countOf={cnt}, totalCount={tot}");

    let logs = ircpt.get("logs").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let log_ok = logs.first().map(|l| {
        let topics = l.get("topics").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let t0 = topics.first().and_then(|v| v.as_str()).unwrap_or("");
        let t1 = topics.get(1).and_then(|v| v.as_str()).unwrap_or("");
        let d = l.get("data").and_then(|v| v.as_str()).unwrap_or("").trim_start_matches("0x").to_string();
        t0.eq_ignore_ascii_case(TOPIC0) && t1.to_lowercase().ends_with(&me_hex)
            && d.len() == 128 && u128::from_str_radix(&d[32..64], 16).unwrap_or(0) == 1
    }).unwrap_or(false);
    println!("    Incremented event (topic0 + indexed caller + count==1): {log_ok}");

    if cut_ok && cnt == 1 && tot == 1 && log_ok {
        println!("\n✅ CAPSTONE: SolidityLite MVP demonstrated end to end on Moderato.");
        println!("   An agent WROTE a CounterFacet in source → COMPILED it in-crate → DEPLOYED it → GENESISed a diamond it OWNS → CUT the facet in → CALLED it through the diamond. State advanced (countOf/totalCount==1) and the Incremented event fired correctly. Self-modifying-platform keystone: PROVEN.");
    } else {
        println!("\n⚠️  cut_ok={cut_ok} cnt={cnt} tot={tot} log_ok={log_ok}");
    }
    Ok(())
}

async fn create(owner: &SigningKey, sponsor: &SigningKey, me: &[u8; 20], init: Vec<u8>, gas: u128) -> Result<String, Box<dyn std::error::Error>> {
    let nonce = registry::next_nonce(&format!("0x{}", hex(me))).await?;
    let gp = registry::current_gas_price().await?;
    let tx = TempoTxBuilder::new(registry::CHAIN_ID())
        .max_priority_fee_per_gas(gp).max_fee_per_gas(gp).gas_limit(gas).nonce(nonce)
        .fee_token(parse_addr(ALPHA_USD)?)
        .call(TempoCall { to: [0u8; 20], value_wei: 0, input: init })
        .sponsored().create().build();
    let s = wallet::sign_hash(owner, &tx.sender_hash());
    let f = wallet::sign_hash(sponsor, &tx.fee_payer_hash(me));
    let raw = format!("0x{}", hex(&tx.serialize_signed(&s, Some(&f))));
    let rcpt = poll_receipt(&rpc_str("eth_sendRawTransaction", serde_json::json!([raw])).await?).await?;
    if rcpt.get("status").and_then(|v| v.as_str()).unwrap_or("") != "0x1" {
        return Err("CREATE reverted".into());
    }
    Ok(rcpt.get("contractAddress").and_then(|v| v.as_str()).unwrap_or("").to_string())
}
async fn read_u128(c: &str, data: &str) -> Result<u128, Box<dyn std::error::Error>> {
    let r = rpc_str("eth_call", serde_json::json!([{"to": c, "data": data}, "latest"])).await?;
    let t = r.trim_start_matches("0x");
    Ok(u128::from_str_radix(&t[t.len().saturating_sub(32)..], 16).unwrap_or(u128::MAX))
}
async fn poll_receipt(tx_hash: &str) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    for _ in 0..45 {
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
