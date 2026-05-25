//! JSON-RPC client for `LocalharnessRegistry` — read AND write.
//!
//! Hand-rolled instead of pulling alloy: the apex chrome only needs a
//! handful of methods (`eth_call`, `eth_chainId`, `eth_gasPrice`,
//! `eth_getTransactionCount`, `eth_estimateGas`,
//! `eth_sendRawTransaction`, `eth_getTransactionReceipt`) and we
//! already have `reqwest` in the bundle. Avoiding alloy also sidesteps
//! the `serde::__private` compat snag we hit during the M6 spike.
//!
//! When `REGISTRY_ADDRESS` is the zero address the contract isn't
//! deployed yet — every query returns `Status::Unknown` so the UI can
//! degrade gracefully ("(registry pending deploy)") instead of
//! erroring.

use k256::ecdsa::SigningKey;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

use crate::wallet;

/// Tempo Moderato testnet RPC. Per the tempo-x402 reference.
pub const RPC_URL: &str = "https://rpc.moderato.tempo.xyz";

/// `LocalharnessRegistry` Diamond on Tempo Moderato testnet
/// (chain id 42431). **Fresh deployment 2026-05-25** —
/// `DeployDiamond.s.sol` + `AddErc721Fresh.s.sol` + `AddTbaFacet.s.sol`.
/// Replaces the previous diamond at `0xed7a2d…c656d` (test registrations
/// abandoned; old NFTs orphan in their owners' wallets).
///
/// The diamond proxy holds the storage; the actual `register /
/// ownerOfName / idOfName / …` selectors dispatch to per-facet
/// addresses. ABI-compatible with the previous diamond — bundle code
/// reads `nextId() / ownerOfName(string) / …` unchanged.
///
/// Owner (deployer / admin): 0x313b1659F5037080aA0C113D386C5954F348EF1e
/// Predecessor (diamond v1): 0xed7a2d170ab2d41721c9bd7368adbff6df0c656d
pub const REGISTRY_ADDRESS: &str = "0x6f2858b4b10bf8d4ea372a446e69bea8fbce2930";

/// Tempo Moderato chain id — used in EIP-155 v computation.
pub const CHAIN_ID: u64 = 42431;

/// `BootstrapFaucet.sol` — DORMANT. Deployed at
/// `0xA439c7C31fa8DeD94d90D3fD3958438A4876dc0f` but unusable on
/// Tempo Moderato because the chain refuses EOA↔contract native
/// value transfers ("value transfer not allowed"). Kept as a
/// historical breadcrumb; all distribution flows through
/// [`LOCALHARNESS_TOKEN_ADDRESS`] now.
pub const BOOTSTRAP_FAUCET_ADDRESS: &str = "0x0000000000000000000000000000000000000000";

/// `LocalharnessToken.sol` — the $localharness ERC-20 with a
/// once-per-address `faucet(recipient)` that mints `faucetAmount`
/// fresh tokens. Replaces BootstrapFaucet — works on Tempo Moderato
/// because every move is a contract call, not a native transfer.
///
/// Deployed 2026-05-24 from an ephemeral key; ownership transferred
/// to the admin EOA (`0x81E9c327…`) immediately after deploy.
///
/// name: "localharness", symbol: "localharness", decimals: 18,
/// faucetAmount: 1000 LH.
pub const LOCALHARNESS_TOKEN_ADDRESS: &str = "0xcC8A300658dC8d0648D984A5066Af3F8E75e0936";

/// What we can learn about a name without touching the wallet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Registry isn't deployed (or address still set to zero).
    Unknown,
    /// `idOfName` returned 0 — free to register.
    Available,
    /// `idOfName` returned a non-zero agentId.
    Taken { agent_id: u64 },
}

/// One entry in the "your agents" list rendered on apex. Read from
/// the diamond via `list_owned_tokens(owner)`.
#[derive(Debug, Clone)]
pub struct OwnedToken {
    pub token_id: u64,
    pub name: String,
    pub tba: Option<String>,
}

/// All NFTs (= registered names) currently owned by `owner_hex`.
/// Iterates `1..nextId` and filters by `ownerOf(i) == owner_hex`.
/// Fine for testnet where the total token count is small; if the
/// registry ever grows past a few hundred we'd swap to log-based
/// indexing or a multicall batch. Returns entries newest-first.
pub async fn list_owned_tokens(owner_hex: &str) -> Result<Vec<OwnedToken>, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(Vec::new());
    }
    let total = next_id().await?;
    let owner_lower = owner_hex.to_lowercase();
    let mut out: Vec<OwnedToken> = Vec::new();
    // nextId is one-past the highest issued id (we start at 1, so the
    // valid range is 1..nextId-1 inclusive — equivalent to 1..nextId).
    for id in 1..total {
        let owner = match owner_of_id(id).await {
            Ok(Some(addr)) => addr,
            _ => continue,
        };
        if owner.to_lowercase() != owner_lower {
            continue;
        }
        let name = name_of_id(id).await.unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let tba = tba_of_name(&name).await.ok().flatten();
        out.push(OwnedToken {
            token_id: id,
            name,
            tba,
        });
    }
    // Reverse so newer registrations land at the top.
    out.reverse();
    Ok(out)
}

async fn next_id() -> Result<u64, String> {
    let calldata = format!("0x{}", bytes_to_hex(&selector("nextId()")));
    let result_hex = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    decode_u256_as_u64(&result_hex)
}

async fn owner_of_id(id: u64) -> Result<Option<String>, String> {
    let mut data = Vec::with_capacity(4 + 32);
    data.extend_from_slice(&selector("ownerOfId(uint256)"));
    data.extend_from_slice(&u256_be(id as u128));
    let calldata = format!("0x{}", bytes_to_hex(&data));
    let result_hex = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    let trimmed = result_hex.trim().trim_start_matches("0x");
    if trimmed.len() < 64 {
        return Ok(None);
    }
    let addr_hex = &trimmed[trimmed.len() - 40..];
    if addr_hex.chars().all(|c| c == '0') {
        return Ok(None);
    }
    Ok(Some(format!("0x{}", addr_hex.to_lowercase())))
}

async fn name_of_id(id: u64) -> Result<String, String> {
    let mut data = Vec::with_capacity(4 + 32);
    data.extend_from_slice(&selector("nameOfId(uint256)"));
    data.extend_from_slice(&u256_be(id as u128));
    let calldata = format!("0x{}", bytes_to_hex(&data));
    let result_hex = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    // ABI-encoded string: offset (32 bytes, value 0x20) + length (32 bytes) + bytes
    let raw = hex_to_bytes(&result_hex)?;
    if raw.len() < 64 {
        return Err(format!("nameOfId: short response {} bytes", raw.len()));
    }
    let len = u64::from_be_bytes(raw[56..64].try_into().map_err(|e: std::array::TryFromSliceError| e.to_string())?) as usize;
    if raw.len() < 64 + len {
        return Err(format!("nameOfId: truncated body (need {} bytes, have {})", 64 + len, raw.len()));
    }
    String::from_utf8(raw[64..64 + len].to_vec()).map_err(|e| e.to_string())
}

/// `eth_call tokenBoundAccountByName(name)` and return the ERC-6551
/// account address. None when the name is unregistered. The address
/// is deterministic — it exists counterfactually even if the account
/// hasn't been deployed yet.
pub async fn tba_of_name(name: &str) -> Result<Option<String>, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(None);
    }
    let calldata = encode_string_call("tokenBoundAccountByName(string)", name);
    let result_hex = match eth_call(REGISTRY_ADDRESS, &calldata).await {
        Ok(h) => h,
        Err(err) => {
            // The contract reverts with "TBA: name unregistered" when
            // the name has no token — surface that as None rather than
            // an error so the UI can degrade cleanly.
            if err.contains("name unregistered") || err.contains("nonexistent token") {
                return Ok(None);
            }
            return Err(err);
        }
    };
    let trimmed = result_hex.trim().trim_start_matches("0x");
    if trimmed.len() < 64 {
        return Err(format!("tokenBoundAccountByName: short response {trimmed}"));
    }
    let addr_hex = &trimmed[trimmed.len() - 40..];
    if addr_hex.chars().all(|c| c == '0') {
        return Ok(None);
    }
    Ok(Some(format!("0x{}", addr_hex.to_lowercase())))
}

/// `eth_call ownerOfName(name)` and return the address as a
/// `0x`-prefixed lowercase hex string. `None` if the name has no
/// on-chain owner (returns the zero address).
pub async fn owner_of_name(name: &str) -> Result<Option<String>, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(None);
    }
    let calldata = encode_owner_of_name(name);
    let result_hex = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    // Address is the last 20 bytes of a 32-byte uint256 return.
    let trimmed = result_hex.trim().trim_start_matches("0x");
    if trimmed.len() < 64 {
        return Err(format!("ownerOfName: short response {trimmed}"));
    }
    let addr_hex = &trimmed[trimmed.len() - 40..];
    if addr_hex.chars().all(|c| c == '0') {
        return Ok(None);
    }
    Ok(Some(format!("0x{}", addr_hex.to_lowercase())))
}

fn encode_owner_of_name(name: &str) -> String {
    encode_string_call("ownerOfName(string)", name)
}

/// Generic `fn(string)` calldata encoder. ABI: selector + 0x20 offset
/// + length + UTF-8 bytes padded to a 32-byte multiple.
fn encode_string_call(signature: &str, value: &str) -> String {
    let sel = selector(signature);
    let bytes = value.as_bytes();
    let len = bytes.len();
    let padded_len = len.div_ceil(32) * 32;

    let mut buf = Vec::with_capacity(4 + 32 + 32 + padded_len);
    buf.extend_from_slice(&sel);
    buf.extend_from_slice(&u256_be(0x20));
    buf.extend_from_slice(&u256_be(len as u128));
    buf.extend_from_slice(bytes);
    buf.resize(4 + 32 + 32 + padded_len, 0);

    let mut out = String::with_capacity(2 + buf.len() * 2);
    out.push_str("0x");
    for b in &buf {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// `eth_call idOfName(name)` and classify the result. Single round trip.
pub async fn check_name(name: &str) -> Result<Status, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(Status::Unknown);
    }

    let calldata = encode_id_of_name(name);
    let result_hex = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    let id = decode_u256_as_u64(&result_hex)?;
    Ok(if id == 0 {
        Status::Available
    } else {
        Status::Taken { agent_id: id }
    })
}

/// Register `name` on the contract under the given signer's address.
/// Returns the transaction hash once it's been included in a block.
/// The wallet needs testnet TMP for gas — the apex page is expected
/// to faucet-fund it on first claim attempt.
pub async fn claim_name(signer: &SigningKey, name: &str) -> Result<String, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Err("registry not deployed".into());
    }
    let from = wallet::address(signer);
    let from_hex = address_to_hex(&from);

    // Pull live tx parameters in parallel-ish (they're cheap reads).
    let nonce = eth_get_transaction_count(&from_hex).await?;
    let gas_price = eth_gas_price().await?;
    let calldata_hex = encode_register(name);
    let gas_limit = eth_estimate_gas(&from_hex, REGISTRY_ADDRESS, &calldata_hex).await?;

    // EIP-155 legacy tx: keccak the unsigned RLP, sign, RLP the
    // signed envelope. v = chain_id*2 + 35 + recoveryId.
    let calldata_bytes = hex_to_bytes(&calldata_hex)?;
    let unsigned = rlp_legacy_unsigned(
        nonce,
        gas_price,
        gas_limit,
        REGISTRY_ADDRESS,
        0,
        &calldata_bytes,
        CHAIN_ID,
    )?;
    let mut hasher = Keccak256::new();
    hasher.update(&unsigned);
    let mut prehash = [0u8; 32];
    prehash.copy_from_slice(&hasher.finalize());

    let sig = wallet::sign_hash(signer, &prehash);
    let r = &sig[..32];
    let s = &sig[32..64];
    // sig[64] is 27 + recoveryId in our wallet's output; lift it back
    // to a 0/1 recovery id for EIP-155 v derivation.
    let rec_id = (sig[64] - 27) as u64;
    let v = CHAIN_ID * 2 + 35 + rec_id;

    let signed = rlp_legacy_signed(
        nonce,
        gas_price,
        gas_limit,
        REGISTRY_ADDRESS,
        0,
        &calldata_bytes,
        v,
        r,
        s,
    )?;
    let raw_hex = format!("0x{}", bytes_to_hex(&signed));

    let tx_hash = eth_send_raw_transaction(&raw_hex).await?;
    // Wait for the receipt — claim should be confirmed before the
    // UI navigates the user away.
    wait_for_receipt(&tx_hash).await?;
    Ok(tx_hash)
}

/// Best-effort: hit the Tempo `tempo_fundAddress` faucet for the
/// supplied address. The faucet returns the funding tx hashes on
/// success. Bundled here because the apex claim flow uses it
/// pre-emptively before a brand-new wallet tries to send its first tx.
pub async fn request_faucet_funds(address_hex: &str) -> Result<(), String> {
    let body = RpcRequest {
        jsonrpc: "2.0",
        id: 1,
        method: "tempo_fundAddress",
        params: serde_json::json!([address_hex]),
    };
    let client = reqwest::Client::new();
    let resp = client
        .post(RPC_URL)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("faucet send: {e}"))?;
    let parsed: RpcResponse = resp
        .json()
        .await
        .map_err(|e| format!("faucet decode: {e}"))?;
    if let Some(err) = parsed.error {
        return Err(format!("faucet: {}", err.message));
    }
    Ok(())
}

// --- ABI encoding -------------------------------------------------------

/// Function selector = first 4 bytes of keccak256("<sig>").
fn selector(signature: &str) -> [u8; 4] {
    let mut h = Keccak256::new();
    h.update(signature.as_bytes());
    let digest = h.finalize();
    let mut out = [0u8; 4];
    out.copy_from_slice(&digest[..4]);
    out
}

/// Encode `idOfName(string)` calldata. ABI layout:
///   [0..4]     selector
///   [4..36]    offset to string head (always 0x20 for one dynamic arg)
///   [36..68]   string length (uint256, big-endian)
///   [68..]     string bytes, right-padded to 32-byte multiple
fn encode_id_of_name(name: &str) -> String {
    let sel = selector("idOfName(string)");
    let bytes = name.as_bytes();
    let len = bytes.len();
    let padded_len = len.div_ceil(32) * 32;

    let mut buf = Vec::with_capacity(4 + 32 + 32 + padded_len);
    buf.extend_from_slice(&sel);
    buf.extend_from_slice(&u256_be(0x20));
    buf.extend_from_slice(&u256_be(len as u128));
    buf.extend_from_slice(bytes);
    buf.resize(4 + 32 + 32 + padded_len, 0);

    let mut out = String::with_capacity(2 + buf.len() * 2);
    out.push_str("0x");
    for b in &buf {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Encode `register(string)` calldata. Same shape as `idOfName`.
fn encode_register(name: &str) -> String {
    let sel = selector("register(string)");
    let bytes = name.as_bytes();
    let len = bytes.len();
    let padded_len = len.div_ceil(32) * 32;

    let mut buf = Vec::with_capacity(4 + 32 + 32 + padded_len);
    buf.extend_from_slice(&sel);
    buf.extend_from_slice(&u256_be(0x20));
    buf.extend_from_slice(&u256_be(len as u128));
    buf.extend_from_slice(bytes);
    buf.resize(4 + 32 + 32 + padded_len, 0);

    let mut out = String::with_capacity(2 + buf.len() * 2);
    out.push_str("0x");
    for b in &buf {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn u256_be(value: u128) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[16..].copy_from_slice(&value.to_be_bytes());
    out
}

fn decode_u256_as_u64(hex: &str) -> Result<u64, String> {
    let stripped = hex.trim().trim_start_matches("0x");
    if stripped.is_empty() {
        return Ok(0);
    }
    if stripped.len() > 64 {
        return Err(format!("u256 hex too long: {}", stripped.len()));
    }
    // High bytes must be zero for u64.
    let high_end = stripped.len().saturating_sub(16);
    if stripped[..high_end].chars().any(|c| c != '0') {
        return Err("u256 exceeds u64 range".into());
    }
    let tail = &stripped[high_end..];
    u64::from_str_radix(tail, 16).map_err(|e| e.to_string())
}

fn zero_address() -> &'static str {
    "0x0000000000000000000000000000000000000000"
}

fn address_to_hex(addr: &[u8; 20]) -> String {
    let mut s = String::with_capacity(42);
    s.push_str("0x");
    for b in addr {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn hex_to_bytes(hex: &str) -> Result<Vec<u8>, String> {
    let trimmed = hex.trim().trim_start_matches("0x").trim_start_matches("0X");
    if trimmed.len() % 2 != 0 {
        return Err("hex odd length".into());
    }
    let mut out = Vec::with_capacity(trimmed.len() / 2);
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = nibble_value(bytes[i])?;
        let lo = nibble_value(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn nibble_value(b: u8) -> Result<u8, String> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(format!("non-hex byte {b}")),
    }
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn parse_hex_quantity(hex: &str) -> Result<u128, String> {
    let trimmed = hex.trim().trim_start_matches("0x");
    if trimmed.is_empty() {
        return Ok(0);
    }
    u128::from_str_radix(trimmed, 16).map_err(|e| e.to_string())
}

// --- public helpers for cross-module tx flows -------------------------
//
// The browser app's chat flow (subdomain origin) and iframe signer
// (apex origin) both need to compose native-ETH transfers — visitor
// pays the agent's TBA for a turn. These wrap the registry's RLP +
// JSON-RPC primitives so callers don't reimplement EIP-155 envelope
// encoding. Available on every target; gated only by `wallet`.

/// Pending nonce for `address_hex`. Use this as the next tx nonce so a
/// burst of payments doesn't collide with the previous tx still being
/// mined.
pub async fn next_nonce(address_hex: &str) -> Result<u128, String> {
    eth_get_transaction_count(address_hex).await
}

/// Current `eth_gasPrice` reported by the node, in wei.
pub async fn current_gas_price() -> Result<u128, String> {
    eth_gas_price().await
}

/// Submit a signed raw tx hex and block until the receipt is mined.
/// Returns the tx hash. Errors if the receipt status is `0x0` (revert)
/// or if no receipt lands within the polling window.
pub async fn submit_and_wait_receipt(raw_hex: &str) -> Result<String, String> {
    let tx_hash = eth_send_raw_transaction(raw_hex).await?;
    wait_for_receipt(&tx_hash).await?;
    Ok(tx_hash)
}

/// EIP-155 unsigned RLP for a native ETH transfer (zero calldata).
/// Hash this with keccak256 to get the prehash a signer commits to.
pub fn rlp_native_transfer_unsigned(
    to_hex: &str,
    value_wei: u128,
    nonce: u128,
    gas_price: u128,
    gas_limit: u128,
) -> Result<Vec<u8>, String> {
    rlp_legacy_unsigned(nonce, gas_price, gas_limit, to_hex, value_wei, &[], CHAIN_ID)
}

/// Assemble a `0x`-prefixed signed raw tx hex from a native-ETH
/// transfer's parameters plus a 65-byte signature (r||s||v, where v
/// is 27 or 28 — the format `wallet::sign_hash` produces). Lifts v
/// into the EIP-155 form (`chain_id * 2 + 35 + recovery_id`).
pub fn rlp_native_transfer_signed(
    to_hex: &str,
    value_wei: u128,
    nonce: u128,
    gas_price: u128,
    gas_limit: u128,
    sig_65: &[u8; 65],
) -> Result<String, String> {
    let rec_id = (sig_65[64] - 27) as u64;
    let v = CHAIN_ID * 2 + 35 + rec_id;
    let signed = rlp_legacy_signed(
        nonce,
        gas_price,
        gas_limit,
        to_hex,
        value_wei,
        &[],
        v,
        &sig_65[..32],
        &sig_65[32..64],
    )?;
    Ok(format!("0x{}", bytes_to_hex(&signed)))
}

/// Gas limit for a vanilla native-ETH transfer with no calldata.
/// The protocol-mandated 21_000 (EIP-2028 doesn't apply here — no data).
pub const NATIVE_TRANSFER_GAS_LIMIT: u128 = 21_000;

/// Native-ETH balance of `address_hex` in wei.
pub async fn balance_of(address_hex: &str) -> Result<u128, String> {
    let hex = rpc(
        "eth_getBalance",
        serde_json::json!([address_hex, "latest"]),
    )
    .await?;
    parse_hex_quantity(&hex)
}

/// Poll `eth_getBalance` until it reports at least `min_wei`, with
/// 1-second cadence. Returns the observed balance on success, errors
/// if no observation reached `min_wei` within `max_attempts` seconds.
/// Used by the identity-creation flow to confirm the faucet drip
/// actually landed before letting the user try a real tx.
pub async fn wait_for_min_balance(
    address_hex: &str,
    min_wei: u128,
    max_attempts: u32,
) -> Result<u128, String> {
    for _ in 0..max_attempts {
        let bal = balance_of(address_hex).await?;
        if bal >= min_wei {
            return Ok(bal);
        }
        sleep_ms(1000).await;
    }
    Err(format!(
        "balance for {address_hex} did not reach {min_wei} wei within {max_attempts}s"
    ))
}

// --- $localharness ERC-20 helpers ------------------------------------

/// `balanceOf(holder)` on [`LOCALHARNESS_TOKEN_ADDRESS`]. Returns the
/// holder's $localharness balance in 18-decimal token wei. Useful for
/// confirming the faucet/transfer flows actually landed funds.
pub async fn token_balance_of(holder_hex: &str) -> Result<u128, String> {
    if LOCALHARNESS_TOKEN_ADDRESS == zero_address() {
        return Err("localharness token not deployed".into());
    }
    let selector = selector("balanceOf(address)");
    let holder_bytes = hex_to_bytes(holder_hex)?;
    if holder_bytes.len() != 20 {
        return Err(format!("holder must be 20 bytes, got {}", holder_bytes.len()));
    }
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(&holder_bytes);
    let mut calldata = Vec::with_capacity(36);
    calldata.extend_from_slice(&selector);
    calldata.extend_from_slice(&padded);

    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(LOCALHARNESS_TOKEN_ADDRESS, &calldata_hex).await?;
    decode_u256_as_u128(&result)
}

/// Sign + submit `LocalharnessToken.faucet(signer.address)`. Mints
/// `faucetAmount` fresh tokens to the signer (one claim per address
/// ever). Caller pays gas. Used by the identity-creation flow so a
/// fresh wallet ends up with a starter $localharness balance.
pub async fn token_faucet_self(signer: &SigningKey) -> Result<String, String> {
    let from_bytes = wallet::address(signer);
    let calldata = encode_address_call("faucet(address)", &from_bytes);
    sign_and_submit_call(signer, LOCALHARNESS_TOKEN_ADDRESS, 0, &calldata).await
}

/// Sign + submit `LocalharnessToken.transfer(to, amount)`. The
/// payment loop's substitute for `rlp_native_transfer` —
/// `transfer` is an ERC-20 contract call, which Tempo allows.
pub async fn token_transfer(
    signer: &SigningKey,
    to_hex: &str,
    amount_token_wei: u128,
) -> Result<String, String> {
    let to_bytes = hex_to_bytes(to_hex)?;
    if to_bytes.len() != 20 {
        return Err(format!("to must be 20 bytes, got {}", to_bytes.len()));
    }
    let selector = selector("transfer(address,uint256)");
    let mut to_padded = [0u8; 32];
    to_padded[12..].copy_from_slice(&to_bytes);
    let amount_bytes = u256_be(amount_token_wei);
    let mut calldata = Vec::with_capacity(4 + 32 + 32);
    calldata.extend_from_slice(&selector);
    calldata.extend_from_slice(&to_padded);
    calldata.extend_from_slice(&amount_bytes);
    sign_and_submit_call(signer, LOCALHARNESS_TOKEN_ADDRESS, 0, &calldata).await
}

/// Build calldata for `f(address)`: selector || padded-address.
fn encode_address_call(signature: &str, addr: &[u8; 20]) -> Vec<u8> {
    let selector = selector(signature);
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(addr);
    let mut out = Vec::with_capacity(36);
    out.extend_from_slice(&selector);
    out.extend_from_slice(&padded);
    out
}

/// Build, sign, submit, wait-for-receipt for a contract call.
/// `to_hex` is the contract, `value_wei` is the native value sent
/// with the call (usually 0 for ERC-20 ops on Tempo), `calldata` is
/// the encoded selector + args. Errors propagate from any leg.
async fn sign_and_submit_call(
    signer: &SigningKey,
    to_hex: &str,
    value_wei: u128,
    calldata: &[u8],
) -> Result<String, String> {
    if to_hex == zero_address() {
        return Err("target contract address is zero".into());
    }
    let from_bytes = wallet::address(signer);
    let from_hex = address_to_hex(&from_bytes);

    let nonce = eth_get_transaction_count(&from_hex).await?;
    let gas_price = eth_gas_price().await?;
    let calldata_hex = format!("0x{}", bytes_to_hex(calldata));
    let gas_limit = eth_estimate_gas(&from_hex, to_hex, &calldata_hex).await?;

    let unsigned = rlp_legacy_unsigned(
        nonce, gas_price, gas_limit, to_hex, value_wei, calldata, CHAIN_ID,
    )?;
    let mut hasher = Keccak256::new();
    hasher.update(&unsigned);
    let mut prehash = [0u8; 32];
    prehash.copy_from_slice(&hasher.finalize());

    let sig = wallet::sign_hash(signer, &prehash);
    let rec_id = (sig[64] - 27) as u64;
    let v = CHAIN_ID * 2 + 35 + rec_id;
    let signed = rlp_legacy_signed(
        nonce, gas_price, gas_limit, to_hex, value_wei, calldata,
        v, &sig[..32], &sig[32..64],
    )?;
    let raw_hex = format!("0x{}", bytes_to_hex(&signed));

    let tx_hash = eth_send_raw_transaction(&raw_hex).await?;
    wait_for_receipt(&tx_hash).await?;
    Ok(tx_hash)
}

fn decode_u256_as_u128(hex: &str) -> Result<u128, String> {
    let trimmed = hex.trim_start_matches("0x");
    if trimmed.is_empty() {
        return Ok(0);
    }
    // Strip leading zeros so we fit in u128 (last 32 hex chars).
    let tail = if trimmed.len() <= 32 {
        trimmed
    } else {
        &trimmed[trimmed.len() - 32..]
    };
    u128::from_str_radix(tail, 16).map_err(|e| e.to_string())
}

// --- legacy / EIP-155 transaction RLP --------------------------------

/// EIP-155 unsigned RLP for any legacy tx — contract call OR native
/// transfer. Pass empty `data` for native, populated `data` for a
/// contract call. Hash with keccak256 to get the prehash a signer
/// commits to. The native-transfer-specific wrapper
/// [`rlp_native_transfer_unsigned`] is built on top of this.
pub fn rlp_call_unsigned(
    to_hex: &str,
    value_wei: u128,
    data: &[u8],
    nonce: u128,
    gas_price: u128,
    gas_limit: u128,
) -> Result<Vec<u8>, String> {
    rlp_legacy_unsigned(nonce, gas_price, gas_limit, to_hex, value_wei, data, CHAIN_ID)
}

/// Assemble a `0x`-prefixed signed raw tx hex for any legacy-style
/// tx (contract call or native). General-purpose counterpart to
/// [`rlp_call_unsigned`].
pub fn rlp_call_signed(
    to_hex: &str,
    value_wei: u128,
    data: &[u8],
    nonce: u128,
    gas_price: u128,
    gas_limit: u128,
    sig_65: &[u8; 65],
) -> Result<String, String> {
    let rec_id = (sig_65[64] - 27) as u64;
    let v = CHAIN_ID * 2 + 35 + rec_id;
    let signed = rlp_legacy_signed(
        nonce, gas_price, gas_limit, to_hex, value_wei, data,
        v, &sig_65[..32], &sig_65[32..64],
    )?;
    Ok(format!("0x{}", bytes_to_hex(&signed)))
}

#[allow(clippy::too_many_arguments)]
fn rlp_legacy_unsigned(
    nonce: u128,
    gas_price: u128,
    gas_limit: u128,
    to_hex: &str,
    value: u128,
    data: &[u8],
    chain_id: u64,
) -> Result<Vec<u8>, String> {
    let to_bytes = hex_to_bytes(to_hex)?;
    // EIP-155: rlp([nonce, gasPrice, gasLimit, to, value, data, chainId, 0, 0])
    let items = vec![
        wallet::rlp_uint(nonce),
        wallet::rlp_uint(gas_price),
        wallet::rlp_uint(gas_limit),
        wallet::rlp_bytes(&to_bytes),
        wallet::rlp_uint(value),
        wallet::rlp_bytes(data),
        wallet::rlp_uint(chain_id as u128),
        wallet::rlp_uint(0),
        wallet::rlp_uint(0),
    ];
    Ok(wallet::rlp_list(&items))
}

#[allow(clippy::too_many_arguments)]
fn rlp_legacy_signed(
    nonce: u128,
    gas_price: u128,
    gas_limit: u128,
    to_hex: &str,
    value: u128,
    data: &[u8],
    v: u64,
    r: &[u8],
    s: &[u8],
) -> Result<Vec<u8>, String> {
    let to_bytes = hex_to_bytes(to_hex)?;
    // r and s are 32 bytes each; RLP wants minimal-leading-zero
    // representations. Strip leading zeros (but not all if the value is 0).
    let r_min = strip_leading_zeros(r);
    let s_min = strip_leading_zeros(s);
    let items = vec![
        wallet::rlp_uint(nonce),
        wallet::rlp_uint(gas_price),
        wallet::rlp_uint(gas_limit),
        wallet::rlp_bytes(&to_bytes),
        wallet::rlp_uint(value),
        wallet::rlp_bytes(data),
        wallet::rlp_uint(v as u128),
        wallet::rlp_bytes(r_min),
        wallet::rlp_bytes(s_min),
    ];
    Ok(wallet::rlp_list(&items))
}

fn strip_leading_zeros(bytes: &[u8]) -> &[u8] {
    let first_nz = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len() - 1);
    &bytes[first_nz..]
}

// --- JSON-RPC plumbing --------------------------------------------------

#[derive(Serialize)]
struct RpcRequest<'a> {
    jsonrpc: &'a str,
    id: u32,
    method: &'a str,
    params: serde_json::Value,
}

#[derive(Deserialize)]
struct RpcResponse {
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    error: Option<RpcError>,
}

#[derive(Deserialize)]
struct RpcError {
    #[allow(dead_code)]
    code: i64,
    message: String,
}

async fn rpc(method: &str, params: serde_json::Value) -> Result<String, String> {
    let body = RpcRequest {
        jsonrpc: "2.0",
        id: 1,
        method,
        params,
    };
    let client = reqwest::Client::new();
    let resp = client
        .post(RPC_URL)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("{method} send: {e}"))?;
    let parsed: RpcResponse = resp
        .json()
        .await
        .map_err(|e| format!("{method} decode: {e}"))?;
    if let Some(err) = parsed.error {
        return Err(format!("{method}: {}", err.message));
    }
    parsed
        .result
        .ok_or_else(|| format!("{method} returned no result"))
}

async fn eth_call(to: &str, data_hex: &str) -> Result<String, String> {
    rpc(
        "eth_call",
        serde_json::json!([{ "to": to, "data": data_hex }, "latest"]),
    )
    .await
}

async fn eth_get_transaction_count(addr: &str) -> Result<u128, String> {
    let hex = rpc(
        "eth_getTransactionCount",
        serde_json::json!([addr, "pending"]),
    )
    .await?;
    parse_hex_quantity(&hex)
}

async fn eth_gas_price() -> Result<u128, String> {
    let hex = rpc("eth_gasPrice", serde_json::json!([])).await?;
    parse_hex_quantity(&hex)
}

async fn eth_estimate_gas(from: &str, to: &str, data_hex: &str) -> Result<u128, String> {
    let hex = rpc(
        "eth_estimateGas",
        serde_json::json!([{ "from": from, "to": to, "data": data_hex }]),
    )
    .await?;
    // Add a 25% buffer so we don't get caught by gas-estimation jitter.
    let estimate = parse_hex_quantity(&hex)?;
    Ok(estimate + estimate / 4)
}

async fn eth_send_raw_transaction(raw_hex: &str) -> Result<String, String> {
    rpc("eth_sendRawTransaction", serde_json::json!([raw_hex])).await
}

/// Poll `eth_getTransactionReceipt` until the receipt resolves. Errors
/// after ~30 seconds — Tempo Moderato blocks are ~1s so 30 attempts
/// is more than enough headroom.
async fn wait_for_receipt(tx_hash: &str) -> Result<(), String> {
    for _ in 0..30 {
        let body = RpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "eth_getTransactionReceipt",
            params: serde_json::json!([tx_hash]),
        };
        let client = reqwest::Client::new();
        let resp = client
            .post(RPC_URL)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("receipt poll: {e}"))?;
        // Receipt comes back as an object or null — bypass the
        // RpcResponse string-only deserializer.
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("receipt parse: {e}"))?;
        if let Some(receipt) = json.get("result").filter(|v| !v.is_null()) {
            let status = receipt
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            if status == "0x1" {
                return Ok(());
            } else if status == "0x0" {
                return Err(format!("tx reverted: {tx_hash}"));
            }
        }
        // Wait ~1s before next poll. spawn_local + a 1s timer would
        // be cleaner; gloo_timers is an option if this becomes a
        // bottleneck. For now: a busy yield via JS microtask.
        sleep_ms(1000).await;
    }
    Err(format!("receipt timeout for {tx_hash}"))
}

/// Cross-target sleep — `tokio::time::sleep` on native, a Promise
/// around `setTimeout` on wasm. Used by `claim_name` to poll the
/// transaction receipt every second.
#[cfg(not(target_arch = "wasm32"))]
async fn sleep_ms(ms: u32) {
    tokio::time::sleep(std::time::Duration::from_millis(ms as u64)).await;
}

#[cfg(target_arch = "wasm32")]
async fn sleep_ms(ms: u32) {
    use wasm_bindgen_futures::JsFuture;
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                &resolve,
                ms as i32,
            );
        }
    });
    let _ = JsFuture::from(promise).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_matches_known_value() {
        // keccak256("idOfName(string)") = 0x127c388a...
        // Verified independently: `cast sig "idOfName(string)"`.
        let sel = selector("idOfName(string)");
        let hex: String = sel.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "127c388a");
    }

    #[test]
    fn encode_short_name_layout() {
        let cd = encode_id_of_name("abc");
        // selector + 0x20 offset + 0x03 length + "abc" + padding
        assert!(cd.starts_with("0x127c388a"));
        // Total length: "0x" + (4 + 32 + 32 + 32) bytes * 2 chars/byte
        assert_eq!(cd.len(), 2 + (4 + 32 + 32 + 32) * 2);
    }

    #[test]
    fn decode_zero_means_available() {
        // 32-byte zero word
        let z = format!("0x{}", "0".repeat(64));
        assert_eq!(decode_u256_as_u64(&z).unwrap(), 0);
    }

    #[test]
    fn decode_normal_id() {
        // agentId = 7
        let mut s = "0".repeat(63);
        s.push('7');
        let hex = format!("0x{s}");
        assert_eq!(decode_u256_as_u64(&hex).unwrap(), 7);
    }

    #[test]
    fn decode_oversize_errors() {
        // Bit set in the upper 192 bits — can't fit in u64.
        let mut s = String::from("1");
        s.push_str(&"0".repeat(63));
        let hex = format!("0x{s}");
        assert!(decode_u256_as_u64(&hex).is_err());
    }
}
