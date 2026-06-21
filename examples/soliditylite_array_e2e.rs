//! SolidityLite — prove DYNAMIC ARRAYS (`uint256[]` storage) from source work
//! on-chain. Compiles a `Stack` facet in-crate, deploys it, calls `push(v)` (which
//! stores at `keccak256(slot) + length` and bumps the length), reads `size()`
//! (`xs.length`) and `get(i)` (`xs[i]` — the keccak-derived element slot), and
//! `set(i, v)` (an element overwrite). Asserts the array length grows, elements
//! persist at the canonical Solidity layout, and an indexed overwrite sticks.
//!
//! Run:
//!   EVM_PRIVATE_KEY=0x<caller> SPONSOR_PRIVATE_KEY=0x<sponsor> \
//!     cargo run --example soliditylite_array_e2e --features wallet
//!
//! NOTE: This is the on-chain companion to the native bytecode proof in
//! `soliditylite::codegen::tests` (array_* tests) — those decode the emitted
//! KECCAK256/ADD/SSTORE shapes; this executes them on the real Tempo EVM.

use k256::ecdsa::SigningKey;
use localharness::registry;
use localharness::soliditylite::compile;
use localharness::tempo_tx::{TempoCall, TempoTxBuilder};
use localharness::wallet;

const ALPHA_USD: &str = "0x20c0000000000000000000000000000000000001";
const SRC: &str = "facet Stack { uint256 total; uint256[] xs; function push(uint256 v) external { xs.push(v); total = total + 1; } function set(uint256 i, uint256 v) external { xs[i] = v; } function get(uint256 i) external view returns (uint256) { return xs[i]; } function size() external view returns (uint256) { return xs.length; } }";
// Canonical ABI selectors (keccak256("<sig>")[..4]) for the Stack facet.
const PUSH_SEL: [u8; 4] = [0x95, 0x9a, 0xc4, 0x84]; // push(uint256)
const SET_SEL: [u8; 4] = [0x1a, 0xb0, 0x6e, 0xe5]; // set(uint256,uint256)
const GET_SEL: [u8; 4] = [0x95, 0x07, 0xd3, 0x9a]; // get(uint256)
const SIZE_SEL: [u8; 4] = [0x94, 0x9d, 0x22, 0x5d]; // size()

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sender = key_from_env("EVM_PRIVATE_KEY")?;
    let sponsor = key_from_env("SPONSOR_PRIVATE_KEY")?;
    let me = wallet::address(&sender);

    let art = compile(SRC).map_err(|e| format!("compile error: {e:?}"))?;
    println!("compiled Stack: init {} bytes / runtime {} bytes", art.init_code.len(), art.runtime.len());
    // The hardcoded selector constants must match the compiler's (declaration
    // order: push, set, get, size) — a guard against a stale constant.
    assert_eq!(art.selectors, vec![PUSH_SEL, SET_SEL, GET_SEL, SIZE_SEL], "selector constants drifted from the compiled facet");

    // Deploy via sponsored CREATE.
    let nonce = registry::next_nonce(&format!("0x{}", hex(&me))).await?;
    let gas_price = registry::current_gas_price().await?;
    let tx = TempoTxBuilder::new(registry::CHAIN_ID())
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
    let stack = rcpt.get("contractAddress").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let dstatus = rcpt.get("status").and_then(|v| v.as_str()).unwrap_or("");
    println!("deploy status={dstatus} stack={stack}");
    if dstatus != "0x1" || stack.is_empty() {
        println!("\n❌ Stack did not deploy.");
        return Ok(());
    }

    let size0 = view(&stack, SIZE_SEL, &[]).await?;
    println!("size() before = {size0}");

    push(&sender, &sponsor, &stack, 11).await?;
    push(&sender, &sponsor, &stack, 22).await?;
    let size2 = view(&stack, SIZE_SEL, &[]).await?;
    let elem0 = view(&stack, GET_SEL, &[word(0)]).await?;
    let elem1 = view(&stack, GET_SEL, &[word(1)]).await?;
    println!("after push(11), push(22): size={size2} xs[0]={elem0} xs[1]={elem1}");

    // Overwrite element 0 via set(0, 99).
    call(&sender, &sponsor, &stack, SET_SEL, &[word(0), word(99)]).await?;
    let elem0b = view(&stack, GET_SEL, &[word(0)]).await?;
    println!("after set(0, 99): xs[0]={elem0b}");

    if size0 == 0 && size2 == 2 && elem0 == 11 && elem1 == 22 && elem0b == 99 {
        println!("\n✅ VERDICT: SolidityLite dynamic arrays (uint256[] storage) work on the real Tempo EVM — push() appends at keccak256(slot)+length and grows the length (0 -> 2), xs[i] reads the keccak-derived element slot (11, 22), and xs[i] = v overwrites in place (xs[0] -> 99). Dynamic STORAGE compiles from SOURCE.");
    } else {
        println!("\n⚠️  size0={size0} size2={size2} elem0={elem0} elem1={elem1} elem0b={elem0b} (expected 0,2,11,22,99).");
    }
    Ok(())
}

/// `push(v)` — a sponsored mutating call.
async fn push(sender: &SigningKey, sponsor: &SigningKey, stack: &str, v: u8) -> Result<(), Box<dyn std::error::Error>> {
    call(sender, sponsor, stack, PUSH_SEL, &[word(v)]).await
}

/// A sponsored mutating call: `<selector>(<args...>)` with 32-byte-word args.
async fn call(sender: &SigningKey, sponsor: &SigningKey, stack: &str, sel: [u8; 4], args: &[[u8; 32]]) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = sel.to_vec();
    for a in args { input.extend_from_slice(a); }
    let h = registry::submit_tempo_sponsored(
        sender, sponsor,
        vec![TempoCall { to: parse_addr(stack)?, value_wei: 0, input }],
        ALPHA_USD, 4_000_000,
    ).await?;
    println!("  {sel:02x?} tx: {h}");
    Ok(())
}

/// An `eth_call` view: `<selector>(<args...>)` → the low 16 bytes of the returned word.
async fn view(stack: &str, sel: [u8; 4], args: &[[u8; 32]]) -> Result<u128, Box<dyn std::error::Error>> {
    let mut input = sel.to_vec();
    for a in args { input.extend_from_slice(a); }
    let data = format!("0x{}", hex(&input));
    let r = rpc_str("eth_call", serde_json::json!([{"to": stack, "data": data}, "latest"])).await?;
    let t = r.trim_start_matches("0x");
    Ok(u128::from_str_radix(&t[t.len().saturating_sub(32)..], 16).unwrap_or(u128::MAX))
}

/// A `u8` as a big-endian 32-byte ABI word.
fn word(v: u8) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[31] = v;
    w
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
    Ok(client.post(registry::RPC_URL()).json(&body).send().await?.json().await?)
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
