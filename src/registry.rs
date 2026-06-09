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
pub const REGISTRY_ADDRESS: &str = "0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c";

/// Tempo Moderato chain id — used in EIP-155 v computation.
pub const CHAIN_ID: u64 = 42431;

/// `BootstrapFaucet.sol` — DORMANT. Deployed at
/// `0xA439c7C31fa8DeD94d90D3fD3958438A4876dc0f` but unusable on
/// Tempo Moderato because the chain refuses EOA↔contract native
/// value transfers ("value transfer not allowed"). Kept as a
/// historical breadcrumb; all distribution flows through
/// [`LOCALHARNESS_TOKEN_ADDRESS`] now.
pub const BOOTSTRAP_FAUCET_ADDRESS: &str = "0x0000000000000000000000000000000000000000";

/// `LocalharnessCredits` — TIP-20-shaped credit token (currency =
/// "credits", explicitly NOT USD so it's NOT fee-token-eligible).
/// Replaces the standalone `LocalharnessToken.sol` at
/// `0xcC8A300658…` (orphaned — old balances do not migrate; testnet
/// reset).
///
/// Deployed 2026-05-26 alongside `CreditsFacet` on the diamond. The
/// diamond holds ISSUER_ROLE on this token, so the only path to
/// fresh supply is through the facet's `claimDaily()`. Owner can
/// tune the per-day allowance via `setDailyAllowance` on the diamond.
///
/// name: "localharness credits", symbol: "LH", decimals: 18.
pub const LOCALHARNESS_TOKEN_ADDRESS: &str = "0x90B84c7234Aae89BadA7f69160B9901B9bc37B17";

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
/// Enumerate every subdomain `owner_hex` holds, newest-first.
///
/// There is no owner→tokens index on-chain yet (only `ownerOfId` id→owner +
/// a `balanceOf` count), so the whole registry is still scanned — but in a
/// SINGLE JSON-RPC batch POST instead of `nextId` sequential round-trips (the
/// old loop did ~one RPC per token, serialized, which was ~5s once a few dozen
/// names existed). One batch for `ownerOfId(1..nextId)`, then two more for the
/// `nameOfId` + `tokenBoundAccount` of just the matches. For the O(holdings)
/// fix (one call, your tokens only) see the `tokensOfOwner` facet draft.
pub async fn list_owned_tokens(owner_hex: &str) -> Result<Vec<OwnedToken>, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(Vec::new());
    }
    let total = next_id().await?;
    if total <= 1 {
        return Ok(Vec::new());
    }
    let owner_lower = owner_hex.to_lowercase();

    // ONE batched POST: ownerOfId(1..total). nextId is one-past the highest id.
    let owner_calls: Vec<(&str, String)> = (1..total)
        .map(|id| (REGISTRY_ADDRESS, call_uint("ownerOfId(uint256)", id)))
        .collect();
    let owners = eth_call_batch(&owner_calls).await?;
    let my_ids: Vec<u64> = owners
        .iter()
        .enumerate()
        .filter_map(|(i, res)| {
            let addr = decode_address(res.as_ref().ok()?)?;
            (addr == owner_lower).then_some((i as u64) + 1)
        })
        .collect();
    if my_ids.is_empty() {
        return Ok(Vec::new());
    }

    // Two more batched POSTs: name + TBA of just the owned ids.
    let name_calls: Vec<(&str, String)> = my_ids
        .iter()
        .map(|&id| (REGISTRY_ADDRESS, call_uint("nameOfId(uint256)", id)))
        .collect();
    let tba_calls: Vec<(&str, String)> = my_ids
        .iter()
        .map(|&id| (REGISTRY_ADDRESS, call_uint("tokenBoundAccount(uint256)", id)))
        .collect();
    let names = eth_call_batch(&name_calls).await?;
    let tbas = eth_call_batch(&tba_calls).await?;

    let mut out: Vec<OwnedToken> = Vec::with_capacity(my_ids.len());
    for (k, &id) in my_ids.iter().enumerate() {
        let name = names
            .get(k)
            .and_then(|r| r.as_ref().ok())
            .and_then(|h| decode_string(h))
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let tba = tbas
            .get(k)
            .and_then(|r| r.as_ref().ok())
            .and_then(|h| decode_address(h));
        out.push(OwnedToken {
            token_id: id,
            name,
            tba,
        });
    }
    // Newest registrations at the top.
    out.reverse();
    Ok(out)
}

async fn next_id() -> Result<u64, String> {
    let calldata = format!("0x{}", bytes_to_hex(&selector("nextId()")));
    let result_hex = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    decode_u256_as_u64(&result_hex)
}

/// Total registered subdomains. Token ids start at 1, so the count is
/// `nextId - 1`. Used by the admin Usage tab.
pub async fn subdomain_count() -> Result<u64, String> {
    Ok(next_id().await?.saturating_sub(1))
}

pub async fn name_of_id(id: u64) -> Result<String, String> {
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
    // `len` is attacker-controlled — `64 + len` could overflow, so add checked.
    let end = len
        .checked_add(64)
        .filter(|&end| end <= raw.len())
        .ok_or_else(|| format!("nameOfId: truncated body (len {}, have {})", len, raw.len()))?;
    String::from_utf8(raw[64..end].to_vec()).map_err(|e| e.to_string())
}

/// `eth_call tokenBoundAccount(tokenId)` and return the ERC-6551
/// account address. None when the token isn't registered. The address
/// is deterministic — counterfactual even before deployment.
pub async fn tba_of_token_id(token_id: u64) -> Result<Option<String>, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(None);
    }
    let mut data = Vec::with_capacity(4 + 32);
    data.extend_from_slice(&selector("tokenBoundAccount(uint256)"));
    data.extend_from_slice(&u256_be(token_id as u128));
    let calldata = format!("0x{}", bytes_to_hex(&data));
    let result_hex = match eth_call(REGISTRY_ADDRESS, &calldata).await {
        Ok(h) => h,
        Err(err) => {
            if err.contains("nonexistent token") || err.contains("registry unset") {
                return Ok(None);
            }
            return Err(err);
        }
    };
    let trimmed = result_hex.trim().trim_start_matches("0x");
    if trimmed.len() < 64 {
        return Err(format!("tokenBoundAccount: short response {trimmed}"));
    }
    let addr_hex = &trimmed[trimmed.len() - 40..];
    if addr_hex.chars().all(|c| c == '0') {
        return Ok(None);
    }
    Ok(Some(format!("0x{}", addr_hex.to_lowercase())))
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

/// `eth_call idOfName(name)` → the token id (0 if unregistered).
pub async fn id_of_name(name: &str) -> Result<u64, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(0);
    }
    let calldata = encode_id_of_name(name);
    let result_hex = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    decode_u256_as_u64(&result_hex)
}

/// List the most recently registered agents (newest id first), up to
/// `limit`. Each entry is `(token_id, name)`. Used by the public
/// directory (`?explore=1`). One `nameOfId` read per agent — fine at
/// launch scale; revisit with an event index if the registry grows large.
pub async fn list_recent_agents(limit: u64) -> Result<Vec<(u64, String)>, String> {
    let next = next_id().await?;
    if next <= 1 {
        return Ok(Vec::new());
    }
    let max_id = next - 1;
    let start = max_id.saturating_sub(limit.saturating_sub(1)).max(1);
    let mut out = Vec::new();
    let mut id = max_id;
    loop {
        if let Ok(name) = name_of_id(id).await {
            if !name.is_empty() {
                out.push((id, name));
            }
        }
        if id <= start {
            break;
        }
        id -= 1;
    }
    Ok(out)
}

/// Pure: filter + rank `(name, persona)` pairs by a query (case-insensitive
/// substring against the name OR the persona). Name matches rank above
/// persona-only matches; input order (recency) is preserved within each tier.
/// Empty query returns all. The matching core of [`discover_agents`], split out
/// for testing.
pub fn rank_agent_matches(agents: &[(String, String)], query: &str) -> Vec<(String, String)> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return agents.to_vec();
    }
    let mut name_hits = Vec::new();
    let mut persona_hits = Vec::new();
    for (name, persona) in agents {
        if name.to_lowercase().contains(&q) {
            name_hits.push((name.clone(), persona.clone()));
        } else if persona.to_lowercase().contains(&q) {
            persona_hits.push((name.clone(), persona.clone()));
        }
    }
    name_hits.extend(persona_hits);
    name_hits
}

/// Discover agents by capability/keyword — the "Agent Yellow Pages". Scans the
/// most recent `scan` registered agents, fetches each one's on-chain persona,
/// and returns `(name, persona)` matches for `query`, ranked by relevance (name
/// hit first, then persona hit; newest-first within a tier). Read-only; an agent
/// (or `localharness discover`) uses it to FIND a peer, then `call`/`mcp-call` it.
pub async fn discover_agents(query: &str, scan: u64) -> Result<Vec<(String, String)>, String> {
    let agents = list_recent_agents(scan).await?;
    if agents.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<u64> = agents.iter().map(|(id, _)| *id).collect();
    let personas = personas_of(&ids).await;
    let pairs: Vec<(String, String)> = agents
        .into_iter()
        .zip(personas)
        .map(|((_, name), persona)| (name, persona.unwrap_or_default()))
        .collect();
    Ok(rank_agent_matches(&pairs, query))
}

// --- Published app cartridge (cross-visitor) -------------------------
//
// A subdomain's app is the compiled wasm cartridge stored on-chain under
// a fixed metadata key, so ANY visitor (not just the owner's device)
// boots into it. We store the wasm, not the source — it's smaller (less
// gas) and the visitor runs it without recompiling. The owner publishes
// via a sponsored `setMetadata` call (see `events::publish_app`).

/// Storage key for the published app wasm: `keccak256("localharness.app.wasm")`.
fn app_metadata_key() -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let digest = Keccak256::digest(b"localharness.app.wasm");
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Read a subdomain's published app wasm from on-chain metadata, if any.
pub async fn app_wasm_of(token_id: u64) -> Result<Option<Vec<u8>>, String> {
    let key = app_metadata_key();
    let mut data = Vec::with_capacity(4 + 64);
    data.extend_from_slice(&selector("metadata(uint256,bytes32)"));
    data.extend_from_slice(&u256_be(token_id as u128));
    data.extend_from_slice(&key);
    let calldata = format!("0x{}", bytes_to_hex(&data));
    let result_hex = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    let bytes = hex_to_bytes(&result_hex)?;
    // ABI-encoded `bytes`: [offset(32)][length(32)][payload...].
    if bytes.len() < 64 {
        return Ok(None);
    }
    let mut len_buf = [0u8; 8];
    len_buf.copy_from_slice(&bytes[56..64]);
    let len = u64::from_be_bytes(len_buf) as usize;
    if len == 0 {
        return Ok(None);
    }
    // `len` is attacker-controlled (up to u64::MAX) — `64 + len` could overflow
    // (panic in debug / wrap in release), so add it checked before slicing.
    let payload = len
        .checked_add(64)
        .and_then(|end| bytes.get(64..end))
        .ok_or_else(|| "app wasm truncated".to_string())?;
    Ok(Some(payload.to_vec()))
}

/// Encode `setMetadata(tokenId, appKey, wasm)` calldata for a sponsored
/// publish tx.
pub fn encode_set_app_wasm(token_id: u64, wasm: &[u8]) -> Vec<u8> {
    let key = app_metadata_key();
    let len = wasm.len();
    let padded = len.div_ceil(32) * 32;
    let mut buf = Vec::with_capacity(4 + 96 + 32 + padded);
    buf.extend_from_slice(&selector("setMetadata(uint256,bytes32,bytes)"));
    buf.extend_from_slice(&u256_be(token_id as u128)); // agentId
    buf.extend_from_slice(&key); // bytes32 key (static, inline)
    buf.extend_from_slice(&u256_be(0x60)); // offset to the bytes arg
    buf.extend_from_slice(&u256_be(len as u128)); // bytes length
    buf.extend_from_slice(wasm);
    buf.resize(4 + 96 + 32 + padded, 0); // zero-pad payload to 32
    buf
}

/// Storage key for the seed-encrypted Gemini API key:
/// `keccak256("localharness.gemini_key.enc")`.
fn gemini_key_metadata_key() -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let digest = Keccak256::digest(b"localharness.gemini_key.enc");
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Read a subdomain's on-chain seed-encrypted Gemini key ciphertext, if
/// any. Same ABI-`bytes` decode as `app_wasm_of`.
pub async fn gemini_key_of(token_id: u64) -> Result<Option<Vec<u8>>, String> {
    let mut data = Vec::with_capacity(4 + 64);
    data.extend_from_slice(&selector("metadata(uint256,bytes32)"));
    data.extend_from_slice(&u256_be(token_id as u128));
    data.extend_from_slice(&gemini_key_metadata_key());
    let calldata = format!("0x{}", bytes_to_hex(&data));
    let result_hex = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    let bytes = hex_to_bytes(&result_hex)?;
    if bytes.len() < 64 {
        return Ok(None);
    }
    let mut len_buf = [0u8; 8];
    len_buf.copy_from_slice(&bytes[56..64]);
    let len = u64::from_be_bytes(len_buf) as usize;
    if len == 0 {
        return Ok(None);
    }
    let payload = len
        .checked_add(64)
        .and_then(|end| bytes.get(64..end))
        .ok_or_else(|| "gemini key ciphertext truncated".to_string())?;
    Ok(Some(payload.to_vec()))
}

/// Encode `setMetadata(tokenId, geminiKeyKey, ciphertext)` calldata for a
/// sponsored on-chain key-sync tx.
pub fn encode_set_gemini_key(token_id: u64, ciphertext: &[u8]) -> Vec<u8> {
    let key = gemini_key_metadata_key();
    let len = ciphertext.len();
    let padded = len.div_ceil(32) * 32;
    let mut buf = Vec::with_capacity(4 + 96 + 32 + padded);
    buf.extend_from_slice(&selector("setMetadata(uint256,bytes32,bytes)"));
    buf.extend_from_slice(&u256_be(token_id as u128));
    buf.extend_from_slice(&key);
    buf.extend_from_slice(&u256_be(0x60));
    buf.extend_from_slice(&u256_be(len as u128));
    buf.extend_from_slice(ciphertext);
    buf.resize(4 + 96 + 32 + padded, 0);
    buf
}

// --- Public-face selection (on-chain, visitor-readable) --------------
//
// A subdomain's public face (what visitors see) is one of: a directory
// landing (default), a cartridge app, or an HTML page. The CHOICE lives
// on-chain under `keccak256("localharness.public_face")` so every visitor
// honours it — not just the owner's device. HTML content lives under
// `keccak256("localharness.public.html")` (cartridge wasm reuses the
// existing `localharness.app.wasm` slot). All written via the same
// owner-gated `setMetadata` as the published wasm.

fn keccak_key(label: &[u8]) -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let digest = Keccak256::digest(label);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Read raw `bytes` metadata stored under `key` for `token_id`. `None`
/// when the slot is empty. Shared ABI-`bytes` decode (offset+len+payload).
async fn metadata_bytes_of(token_id: u64, key: [u8; 32]) -> Result<Option<Vec<u8>>, String> {
    let mut data = Vec::with_capacity(4 + 64);
    data.extend_from_slice(&selector("metadata(uint256,bytes32)"));
    data.extend_from_slice(&u256_be(token_id as u128));
    data.extend_from_slice(&key);
    let calldata = format!("0x{}", bytes_to_hex(&data));
    let result_hex = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    let bytes = hex_to_bytes(&result_hex)?;
    if bytes.len() < 64 {
        return Ok(None);
    }
    let mut len_buf = [0u8; 8];
    len_buf.copy_from_slice(&bytes[56..64]);
    let len = u64::from_be_bytes(len_buf) as usize;
    if len == 0 {
        return Ok(None);
    }
    let payload = len
        .checked_add(64)
        .and_then(|end| bytes.get(64..end))
        .ok_or_else(|| "metadata truncated".to_string())?;
    Ok(Some(payload.to_vec()))
}

/// Encode `setMetadata(tokenId, key, payload)` calldata for a sponsored tx.
fn encode_set_metadata_bytes(token_id: u64, key: [u8; 32], payload: &[u8]) -> Vec<u8> {
    let len = payload.len();
    let padded = len.div_ceil(32) * 32;
    let mut buf = Vec::with_capacity(4 + 96 + 32 + padded);
    buf.extend_from_slice(&selector("setMetadata(uint256,bytes32,bytes)"));
    buf.extend_from_slice(&u256_be(token_id as u128));
    buf.extend_from_slice(&key);
    buf.extend_from_slice(&u256_be(0x60));
    buf.extend_from_slice(&u256_be(len as u128));
    buf.extend_from_slice(payload);
    buf.resize(4 + 96 + 32 + padded, 0);
    buf
}

const PUBLIC_FACE_LABEL: &[u8] = b"localharness.public_face";
const PUBLIC_HTML_LABEL: &[u8] = b"localharness.public.html";

/// The subdomain's chosen public face: `"directory"`, `"app"`, `"html"`,
/// or `None` if never set (legacy/default — callers infer from whether a
/// cartridge is published).
pub async fn public_face_of(token_id: u64) -> Result<Option<String>, String> {
    match metadata_bytes_of(token_id, keccak_key(PUBLIC_FACE_LABEL)).await? {
        Some(b) => Ok(String::from_utf8(b)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())),
        None => Ok(None),
    }
}

/// Encode `setMetadata` for the public-face choice (a short string).
pub fn encode_set_public_face(token_id: u64, choice: &str) -> Vec<u8> {
    encode_set_metadata_bytes(token_id, keccak_key(PUBLIC_FACE_LABEL), choice.as_bytes())
}

/// Read a subdomain's published public-face HTML, if any.
pub async fn public_html_of(token_id: u64) -> Result<Option<Vec<u8>>, String> {
    metadata_bytes_of(token_id, keccak_key(PUBLIC_HTML_LABEL)).await
}

/// Encode `setMetadata` for the published public-face HTML.
pub fn encode_set_public_html(token_id: u64, html: &[u8]) -> Vec<u8> {
    encode_set_metadata_bytes(token_id, keccak_key(PUBLIC_HTML_LABEL), html)
}

const PERSONA_LABEL: &[u8] = b"localharness.persona";

/// Read a subdomain's published persona — the system instructions a
/// headless caller runs the agent under so it answers *as* that agent.
/// `None` when unset (caller falls back to a generic system prompt).
pub async fn persona_of(token_id: u64) -> Result<Option<String>, String> {
    match metadata_bytes_of(token_id, keccak_key(PERSONA_LABEL)).await? {
        Some(b) => Ok(String::from_utf8(b)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())),
        None => Ok(None),
    }
}

/// Encode `setMetadata` for a subdomain's persona (its public system
/// prompt). Owner-gated, same path as the published app/html.
pub fn encode_set_persona(token_id: u64, persona: &str) -> Vec<u8> {
    encode_set_metadata_bytes(token_id, keccak_key(PERSONA_LABEL), persona.as_bytes())
}

/// Read the personas for MANY tokens in ONE JSON-RPC batch POST (vs N
/// serial `persona_of` round-trips). Returns one entry per input id, in
/// input order: `Some(persona)` when set, `None` when unset / empty / the
/// per-call RPC failed (graceful degradation — a single bad slot never
/// fails the whole batch). Backs the public-landing agent portfolio cards.
pub async fn personas_of(token_ids: &[u64]) -> Vec<Option<String>> {
    if token_ids.is_empty() || REGISTRY_ADDRESS == zero_address() {
        return token_ids.iter().map(|_| None).collect();
    }
    let key = keccak_key(PERSONA_LABEL);
    let calls: Vec<(&str, String)> = token_ids
        .iter()
        .map(|&id| (REGISTRY_ADDRESS, call_metadata(id, key)))
        .collect();
    match eth_call_batch(&calls).await {
        Ok(results) => results
            .iter()
            .map(|r| {
                r.as_ref()
                    .ok()
                    .and_then(|hex| decode_metadata_bytes(hex))
                    .and_then(|b| String::from_utf8(b).ok())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
            .collect(),
        // Whole-batch failure (network) → degrade every card to no-preview.
        Err(_) => token_ids.iter().map(|_| None).collect(),
    }
}

/// `metadata(tokenId, key)` calldata (hex) for batching.
fn call_metadata(token_id: u64, key: [u8; 32]) -> String {
    let mut data = Vec::with_capacity(4 + 64);
    data.extend_from_slice(&selector("metadata(uint256,bytes32)"));
    data.extend_from_slice(&u256_be(token_id as u128));
    data.extend_from_slice(&key);
    format!("0x{}", bytes_to_hex(&data))
}

/// Decode an ABI `bytes` return (offset + length + payload). `None` when
/// short / empty / truncated. Shared by the batched metadata reads.
fn decode_metadata_bytes(result_hex: &str) -> Option<Vec<u8>> {
    let bytes = hex_to_bytes(result_hex).ok()?;
    if bytes.len() < 64 {
        return None;
    }
    let mut len_buf = [0u8; 8];
    len_buf.copy_from_slice(&bytes[56..64]);
    let len = u64::from_be_bytes(len_buf) as usize;
    if len == 0 {
        return None;
    }
    len.checked_add(64)
        .and_then(|end| bytes.get(64..end))
        .map(|s| s.to_vec())
}

const CAPABILITY_LABEL: &[u8] = b"localharness.capability";

/// Roadmap Phase 0c — the capability-descriptor seam (economy foundation).
///
/// A capability descriptor (price, payee, the service an agent offers) is
/// served OFF-CHAIN (it can be large / change often), but a settle path must
/// not trust the served bytes blindly — a payee swap would drain the payer.
/// So on-chain we store ONLY `keccak256(payload)` (a 32-byte commitment), and
/// [`verify_descriptor`] recomputes the hash of the served bytes and checks it.
/// Purely additive, network-free to encode; forecloses nothing.
pub fn encode_set_capability(token_id: u64, payload: &[u8]) -> Vec<u8> {
    let commitment = keccak_key(payload); // keccak256 of the served descriptor
    encode_set_metadata_bytes(token_id, keccak_key(CAPABILITY_LABEL), &commitment)
}

/// Read the on-chain capability commitment (the stored `keccak256(payload)`),
/// or `None` if unset. Exactly 32 bytes when present.
pub async fn capability_descriptor_of(token_id: u64) -> Result<Option<[u8; 32]>, String> {
    match metadata_bytes_of(token_id, keccak_key(CAPABILITY_LABEL)).await? {
        Some(b) if b.len() == 32 => {
            let mut out = [0u8; 32];
            out.copy_from_slice(&b);
            Ok(Some(out))
        }
        Some(_) => Err("capability commitment is not 32 bytes".to_string()),
        None => Ok(None),
    }
}

/// Verify that `served_payload` matches `token_id`'s on-chain capability
/// commitment. `Ok(true)` iff a commitment is set AND `keccak256(served_payload)`
/// equals it. `Ok(false)` on mismatch OR when no commitment is set (fail
/// closed — never trust a served descriptor an owner hasn't committed to).
pub async fn verify_descriptor(token_id: u64, served_payload: &[u8]) -> Result<bool, String> {
    match capability_descriptor_of(token_id).await? {
        Some(commitment) => Ok(keccak_key(served_payload) == commitment),
        None => Ok(false),
    }
}

/// The localharness credit-proxy origin (a drop-in Gemini base URL). Shared
/// by the browser app and the native CLI so a headless `call` reaches Gemini
/// with the platform key, gated on the caller's `$LH` session — no Gemini
/// key, no live tab, no relay. Mirror of `app::chat::CREDIT_PROXY_URL`.
pub const CREDIT_PROXY_URL: &str = "https://proxy-tau-ten-15.vercel.app/";

/// Mint a credit-proxy auth token `address:timestamp:signature` for `signer`,
/// where the signature is an Ethereum personal-sign over
/// `localharness-proxy:<addr>:<ts>`. The proxy recovers the address and gates
/// on an active session / credit balance. `now_secs` is the UNIX timestamp.
pub fn proxy_auth_token(signer: &SigningKey, now_secs: u64) -> String {
    let addr = format!("0x{}", bytes_to_hex(&crate::wallet::address(signer)));
    let msg = format!("localharness-proxy:{addr}:{now_secs}");
    let sig = crate::wallet::personal_sign(signer, msg.as_bytes());
    format!("{addr}:{now_secs}:0x{}", bytes_to_hex(&sig))
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

/// `balanceOf(holder)` on an arbitrary ERC-20/TIP-20 token. Used by the
/// sponsor balance monitor to read the sponsor's fee-token (AlphaUSD)
/// balance and warn when it runs low.
pub async fn erc20_balance_of(token_hex: &str, holder_hex: &str) -> Result<u128, String> {
    let holder_bytes = hex_to_bytes(holder_hex)?;
    if holder_bytes.len() != 20 {
        return Err(format!("holder must be 20 bytes, got {}", holder_bytes.len()));
    }
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(&holder_bytes);
    let mut calldata = Vec::with_capacity(36);
    calldata.extend_from_slice(&selector("balanceOf(address)"));
    calldata.extend_from_slice(&padded);
    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(token_hex, &calldata_hex).await?;
    decode_u256_as_u128(&result)
}

// `token_faucet_self` removed in 2026-05-26 token migration — the
// new credit token has no `faucet(address)` method. Use
// `claim_daily_sponsored` against the diamond instead.

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

// --- Tempo tx submission ---------------------------------------------

/// Native TIP-20 stablecoins on Tempo Moderato. These ARE eligible as
/// `fee_token` on a Tempo Transaction; our $LH is not (TIP-20-compliance
/// check fails). Pick one as the default fee_token for user-facing txs.
pub const ALPHA_USD_ADDRESS: &str = "0x20c0000000000000000000000000000000000001";

/// Sign and submit a SELF-PAID Tempo tx. Sender pays fees in
/// `fee_token` (`None` = native). Returns the tx hash once mined.
pub async fn submit_tempo_self_paid(
    sender: &SigningKey,
    calls: Vec<crate::tempo_tx::TempoCall>,
    fee_token: Option<&str>,
    gas_limit: u128,
) -> Result<String, String> {
    use crate::tempo_tx::{sign_self_paid, TempoTxBuilder};
    let sender_addr = wallet::address(sender);
    let sender_hex = address_to_hex(&sender_addr);
    let nonce = eth_get_transaction_count(&sender_hex).await?;
    let gas_price = eth_gas_price().await?;
    let mut builder = TempoTxBuilder::new(CHAIN_ID)
        .max_priority_fee_per_gas(gas_price)
        .max_fee_per_gas(gas_price)
        .gas_limit(gas_limit)
        .nonce(nonce)
        .calls(calls);
    if let Some(token) = fee_token {
        builder = builder.fee_token(parse_eth_address(token)?);
    }
    let tx = builder.build();
    let raw = sign_self_paid(tx, sender);
    let raw_hex = format!("0x{}", bytes_to_hex(&raw));
    let tx_hash = eth_send_raw_transaction(&raw_hex).await?;
    wait_for_receipt(&tx_hash).await?;
    Ok(tx_hash)
}

/// Sign and submit a SPONSORED Tempo tx. `sender` signs the intent
/// (and needs no balance); `fee_payer` signs as the gas payer (needs
/// `fee_token` balance). The chain debits `fee_payer`'s `fee_token`
/// balance for the cost; `sender` pays nothing.
pub async fn submit_tempo_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    calls: Vec<crate::tempo_tx::TempoCall>,
    fee_token: &str,
    gas_limit: u128,
) -> Result<String, String> {
    use crate::tempo_tx::{sign_sponsored, TempoTxBuilder};
    let sender_addr = wallet::address(sender);
    let sender_hex = address_to_hex(&sender_addr);
    let nonce = eth_get_transaction_count(&sender_hex).await?;
    let gas_price = eth_gas_price().await?;
    let tx = TempoTxBuilder::new(CHAIN_ID)
        .max_priority_fee_per_gas(gas_price)
        .max_fee_per_gas(gas_price)
        .gas_limit(gas_limit)
        .nonce(nonce)
        .calls(calls)
        .fee_token(parse_eth_address(fee_token)?)
        .sponsored()
        .build();
    let raw = sign_sponsored(tx, sender, fee_payer);
    let raw_hex = format!("0x{}", bytes_to_hex(&raw));
    let tx_hash = eth_send_raw_transaction(&raw_hex).await?;
    wait_for_receipt(&tx_hash).await?;
    Ok(tx_hash)
}

fn parse_eth_address(hex_str: &str) -> Result<[u8; 20], String> {
    let bytes = hex_to_bytes(hex_str)?;
    if bytes.len() != 20 {
        return Err(format!("address must be 20 bytes, got {}", bytes.len()));
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes);
    Ok(out)
}

// --- MAIN identity helpers -------------------------------------------

/// `eth_call mainOf(holder)` — returns the tokenId the holder has
/// registered as their MAIN, or 0 if none. Used by the bundle to
/// decide whether to auto-register on first claim and to badge the
/// MAIN entry in the apex agents list.
pub async fn main_of(holder_hex: &str) -> Result<u64, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(0);
    }
    let selector = selector("mainOf(address)");
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
    let result = eth_call(REGISTRY_ADDRESS, &calldata_hex).await?;
    decode_u256_as_u64(&result)
}

/// Sign + submit `MainIdentityFacet.registerMain(tokenId)`. Caller pays
/// gas. Idempotent on-chain if the caller already has this tokenId as
/// their MAIN; switches MAIN if they declare a different owned tokenId.
pub async fn register_main(signer: &SigningKey, token_id: u64) -> Result<String, String> {
    sign_and_submit_call(signer, REGISTRY_ADDRESS, 0, &encode_register_main(token_id)).await
}

/// Sponsored counterpart to [`register_main`]. `sender` (the holder
/// authorizing the MAIN change) signs the intent and needs zero balance;
/// `fee_payer` pays the gas in `fee_token` (typically AlphaUSD). Use this
/// from bundle paths where the user shouldn't need to hold native gas
/// to update their MAIN.
///
/// When `main_cost()` is non-zero on-chain, prepends a
/// `credits.approve(diamond, cost)` call so `registerMain`'s internal
/// `transferFrom` has the allowance it needs. User pays the cost in
/// LH from their balance; the credits land at the diamond's treasury.
pub async fn register_main_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    token_id: u64,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let token_addr = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS)?;

    let cost = main_cost().await.unwrap_or(0);

    let main_call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_register_main(token_id),
    };

    let calls = if cost > 0 {
        let approve_call = crate::tempo_tx::TempoCall {
            to: token_addr,
            value_wei: 0,
            input: encode_approve(&diamond_addr, cost),
        };
        vec![approve_call, main_call]
    } else {
        vec![main_call]
    };

    // registerMain inner: storage write + event (~50k). +approve
    // (~50k) + transferFrom (~30k) when cost > 0. + ~275k Tempo
    // sponsorship. 700k gives headroom either way.
    submit_tempo_sponsored(sender, fee_payer, calls, fee_token, 700_000).await
}

fn encode_register_main(token_id: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(4 + 32);
    data.extend_from_slice(&selector("registerMain(uint256)"));
    data.extend_from_slice(&u256_be(token_id as u128));
    data
}

// --- MultiSignerAccount (TBA add/remove device signer) ---------------

/// `eth_call isAuthorizedSigner(signer)` on a TBA. Returns true if
/// `signer` is recognized by the TBA's MultiSignerAccount impl —
/// either as the NFT holder (implicit) or as a previously-added device.
pub async fn is_authorized_signer(tba_address: &str, signer_hex: &str) -> Result<bool, String> {
    let mut data = Vec::with_capacity(4 + 32);
    data.extend_from_slice(&selector("isAuthorizedSigner(address)"));
    let signer_bytes = hex_to_bytes(signer_hex)?;
    if signer_bytes.len() != 20 {
        return Err(format!("signer must be 20 bytes, got {}", signer_bytes.len()));
    }
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(&signer_bytes);
    data.extend_from_slice(&padded);
    let calldata = format!("0x{}", bytes_to_hex(&data));
    let result_hex = eth_call(tba_address, &calldata).await?;
    let trimmed = result_hex.trim().trim_start_matches("0x");
    Ok(trimmed.chars().last().map(|c| c == '1').unwrap_or(false))
}

/// Read `token()` on an ERC-6551 account → its owning tokenId (the 3rd
/// returned word: chainId, tokenContract, tokenId). Lets us route owner
/// actions through a TBA when we only know the TBA address.
pub async fn tba_token_id_of(tba_hex: &str) -> Result<u64, String> {
    let calldata = format!("0x{}", bytes_to_hex(&selector("token()")));
    let result = eth_call(tba_hex, &calldata).await?;
    let bytes = hex_to_bytes(&result)?;
    if bytes.len() < 96 {
        return Err("token(): short response".into());
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[88..96]); // low 8 bytes of the tokenId word
    Ok(u64::from_be_bytes(buf))
}

/// Execute a batch of calls AS the TBA (the asset owner), signed by a
/// local key authorized on that TBA — the consolidation owner-action
/// path. Batches `createTokenBoundAccount(token_id)` (idempotent) + one
/// `TBA.execute(target, 0, data)` per entry. Sponsored.
pub async fn tba_execute_batch_sponsored(
    signer: &SigningKey,
    fee_payer: &SigningKey,
    token_id: u64,
    tba_hex: &str,
    targets: &[([u8; 20], Vec<u8>)],
    fee_token: &str,
    gas_limit: u128,
) -> Result<String, String> {
    let diamond = parse_eth_address(REGISTRY_ADDRESS)?;
    let tba = parse_eth_address(tba_hex)?;
    let mut calls = Vec::with_capacity(targets.len() + 1);
    calls.push(crate::tempo_tx::TempoCall {
        to: diamond,
        value_wei: 0,
        input: encode_create_tba(token_id),
    });
    for (target, data) in targets {
        calls.push(crate::tempo_tx::TempoCall {
            to: tba,
            value_wei: 0,
            input: encode_tba_execute(target, 0, data),
        });
    }
    submit_tempo_sponsored(signer, fee_payer, calls, fee_token, gas_limit).await
}

/// Sponsored TBA add-signer. The TBA must exist on-chain (have
/// bytecode) before `addSigner` will work — counterfactual addresses
/// have no code. We always batch `createTokenBoundAccount(tokenId)`
/// before the `addSigner` call; `createTokenBoundAccount` is
/// idempotent, so this is safe whether the TBA is already deployed
/// or not.
///
/// `sender` must be the NFT holder (or an already-authorized signer)
/// of the MAIN; `fee_payer` is the bundle sponsor.
pub async fn add_signer_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    token_id: u64,
    tba_address: &str,
    new_signer_hex: &str,
    fee_token: &str,
) -> Result<String, String> {
    let new_signer = parse_eth_address(new_signer_hex)?;
    let tba_addr = parse_eth_address(tba_address)?;
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;

    let create_call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_create_tba(token_id),
    };
    let add_call = crate::tempo_tx::TempoCall {
        to: tba_addr,
        value_wei: 0,
        input: encode_add_signer(&new_signer),
    };
    // Also record the device in the on-chain enumerable index
    // (DeviceRegistryFacet) so the UI reads the linked set in ONE call —
    // no log scraping. Authority (addSigner) + index (linkDevice) written
    // together in this one sponsored tx.
    let link_call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_link_device(token_id, &new_signer),
    };
    // createTokenBoundAccount first-deploys the MultiSignerAccount via
    // CREATE2 — live-measured at ~742k gas (full contract bytecode +
    // storage), NOT the ~250k an earlier note assumed; near-zero on
    // idempotent reruns. addSigner is a single SSTORE + event (~50k).
    // Plus ~275k Tempo sponsorship. First-time pairing therefore needs
    // ~1.07M, which overflowed the old 1M limit and reverted out-of-gas
    // (the TBA never deployed). 2M gives comfortable headroom; the
    // sponsor is billed on gas USED, not the limit, so the ceiling is
    // free on the cheap idempotent path.
    submit_tempo_sponsored(
        sender,
        fee_payer,
        vec![create_call, add_call, link_call],
        fee_token,
        2_200_000,
    )
    .await
}

/// Encode `linkDevice(uint256,address)` for DeviceRegistryFacet.
fn encode_link_device(main_id: u64, device: &[u8; 20]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 64);
    out.extend_from_slice(&selector("linkDevice(uint256,address)"));
    out.extend_from_slice(&u256_be(main_id as u128));
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(device);
    out.extend_from_slice(&padded);
    out
}

/// Read `devicesOf(mainId)` — the identity's linked devices, from the
/// on-chain enumerable index in ONE call (no log scraping). Returns
/// lowercase `0x…` addresses.
pub async fn devices_of(main_id: u64) -> Result<Vec<String>, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(Vec::new());
    }
    let mut calldata = selector("devicesOf(uint256)").to_vec();
    calldata.extend_from_slice(&u256_be(main_id as u128));
    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(REGISTRY_ADDRESS, &calldata_hex).await?;
    let bytes = hex_to_bytes(&result)?;
    // ABI dynamic address[]: [offset(32)][len(32)][addr0(32)]...
    if bytes.len() < 64 {
        return Ok(Vec::new());
    }
    let mut len_buf = [0u8; 8];
    len_buf.copy_from_slice(&bytes[56..64]); // low 8 bytes of the length word
    let len = u64::from_be_bytes(len_buf) as usize;
    // Don't pre-allocate `len` (attacker-controlled, up to u64::MAX → OOM); the
    // index math below is checked so a hostile length just stops the decode.
    let mut out = Vec::new();
    for i in 0..len {
        let start = match i.checked_mul(32).and_then(|o| o.checked_add(64)) {
            Some(s) => s,
            None => break,
        };
        let Some(word) = start
            .checked_add(32)
            .and_then(|end| bytes.get(start + 12..end))
        else {
            break;
        };
        out.push(format!("0x{}", bytes_to_hex(word)));
    }
    Ok(out)
}

/// Single-read link check — `isDeviceLinked(mainId, addr)` on the index.
/// THE source of truth a device reads on load (no polling, no scraping).
pub async fn is_device_linked(main_id: u64, addr_hex: &str) -> Result<bool, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(false);
    }
    let addr = parse_eth_address(addr_hex)?;
    let mut calldata = selector("isDeviceLinked(uint256,address)").to_vec();
    calldata.extend_from_slice(&u256_be(main_id as u128));
    calldata.extend_from_slice(&addr_word(&addr));
    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(REGISTRY_ADDRESS, &calldata_hex).await?;
    decode_u256_as_u64(&result).map(|v| v != 0)
}

fn encode_unlink_device(main_id: u64, device: &[u8; 20]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 64);
    out.extend_from_slice(&selector("unlinkDevice(uint256,address)"));
    out.extend_from_slice(&u256_be(main_id as u128));
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(device);
    out.extend_from_slice(&padded);
    out
}

fn encode_erc721_transfer_from(from: &[u8; 20], to: &[u8; 20], token_id: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 96);
    out.extend_from_slice(&selector("transferFrom(address,address,uint256)"));
    out.extend_from_slice(&addr_word(from));
    out.extend_from_slice(&addr_word(to));
    out.extend_from_slice(&u256_be(token_id as u128));
    out
}

/// CONSOLIDATION: transfer every `token_id` (subdomains owned by `owner`)
/// into the MAIN's TBA, so one account owns them all and every linked
/// device controls them. `owner` signs (it currently holds the NFTs);
/// sponsored. One-way by design — move back later via TBA.execute.
pub async fn consolidate_into_main_sponsored(
    owner: &SigningKey,
    fee_payer: &SigningKey,
    main_tba_hex: &str,
    token_ids: &[u64],
    fee_token: &str,
) -> Result<String, String> {
    if token_ids.is_empty() {
        return Err("no subdomains to consolidate".into());
    }
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let to = parse_eth_address(main_tba_hex)?;
    let from = wallet::address(owner);
    let calls: Vec<_> = token_ids
        .iter()
        .map(|&tid| crate::tempo_tx::TempoCall {
            to: diamond_addr,
            value_wei: 0,
            input: encode_erc721_transfer_from(&from, &to, tid),
        })
        .collect();
    // ~60k per ERC-721 transfer + ~275k sponsorship.
    let gas = 300_000 + token_ids.len() as u128 * 90_000;
    submit_tempo_sponsored(owner, fee_payer, calls, fee_token, gas).await
}

fn encode_release_name(token_id: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 32);
    out.extend_from_slice(&selector("releaseName(uint256)"));
    out.extend_from_slice(&u256_be(token_id as u128));
    out
}

/// Public `releaseName(tokenId)` calldata — for the iframe-signed agent
/// path (the owner signs the sender hash via the apex signer).
pub fn release_name_calldata(token_id: u64) -> Vec<u8> {
    encode_release_name(token_id)
}

/// Public `register(string)` calldata as raw bytes — for the iframe-signed
/// agent batch path (`batch_create_subdomains`), where many register calls
/// are packed into ONE sponsored Tempo tx. Same ABI as the single claim.
/// NOTE: this is a bare `register` with no `approve` — correct only while
/// `registrationCost()` is 0 (FREE, current testnet config). A non-zero
/// cost would require an approve/transferFrom pair per name (handled by the
/// single-create path), which the batch deliberately does not do.
pub fn register_calldata(name: &str) -> Vec<u8> {
    // `encode_register` returns 0x-hex; strip it back to bytes. Infallible
    // for our own well-formed output, so a decode error degrades to empty
    // calldata (the tx reverts harmlessly rather than panicking in wasm).
    hex_to_bytes(&encode_register(name)).unwrap_or_default()
}

/// Release (recycle) a subdomain — burn the NFT + free the name — via a
/// sponsored tx. `sender` must own the token. DESTRUCTIVE: the UI/tool
/// MUST require typed confirmation before calling this. Refuses the MAIN
/// on-chain.
pub async fn release_name_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    token_id: u64,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_release_name(token_id),
    };
    // 1M, not a flat 400k: a name burn runs ~375-425k all-in (cold-slot clears
    // + ~275k sponsorship), so 400k OOG-reverted while the UI reported success.
    // Over-budget is free — the sponsor pays gas USED, not the limit.
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, 1_000_000).await
}

/// Batch-release (burn) several names in ONE sponsored tx. `sender` must
/// own every `token_id`; the on-chain ReleaseFacet refuses a caller's MAIN
/// per-id (so a MAIN slipped into the list reverts the WHOLE batch — filter
/// it out before calling). DESTRUCTIVE: the UI/tool MUST require a single
/// typed master confirmation before calling this. Mirrors
/// `consolidate_into_main_sponsored`'s multi-call construction, but burns
/// instead of transfers. (Browser callers use the iframe-signed path in
/// `app::events::run_bulk_release`; this is the off-bundle/native twin of
/// `release_name_sponsored`.)
pub async fn release_names_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    token_ids: &[u64],
    fee_token: &str,
) -> Result<String, String> {
    if token_ids.is_empty() {
        return Err("no subdomains to release".into());
    }
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let calls: Vec<_> = token_ids
        .iter()
        .map(|&tid| crate::tempo_tx::TempoCall {
            to: diamond_addr,
            value_wei: 0,
            input: encode_release_name(tid),
        })
        .collect();
    // Each burn ~100-150k inner; +275k sponsorship once for the whole batch.
    // 1M base mirrors the single-release headroom (release_name_sponsored),
    // then ~250k/extra burn. Over-budget is free (sponsor billed on gas USED).
    let gas = 1_000_000 + (token_ids.len() as u128).saturating_sub(1) * 250_000;
    submit_tempo_sponsored(sender, fee_payer, calls, fee_token, gas).await
}

/// Sponsored TBA remove-signer + index unlink (the unlink half of the
/// device lifecycle). `sender` must be an authorized signer of the MAIN.
pub async fn remove_signer_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    token_id: u64,
    tba_address: &str,
    signer_hex: &str,
    fee_token: &str,
) -> Result<String, String> {
    let signer_addr = parse_eth_address(signer_hex)?;
    let tba_addr = parse_eth_address(tba_address)?;
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let remove_call = crate::tempo_tx::TempoCall {
        to: tba_addr,
        value_wei: 0,
        input: encode_remove_signer(&signer_addr),
    };
    // Also drop it from the on-chain index so the UI stops showing it.
    let unlink_call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_unlink_device(token_id, &signer_addr),
    };
    submit_tempo_sponsored(sender, fee_payer, vec![remove_call, unlink_call], fee_token, 600_000)
        .await
}

// --- Device pairing (PairingFacet on the diamond) --------------------

/// keccak256 of a one-time pairing code, used as the rendezvous key.
/// The desktop shows the raw code; both sides hash it to the same
/// `bytes32` topic so the phone never has to transmit a 0x address.
pub fn pairing_code_hash(code: &str) -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let mut out = [0u8; 32];
    out.copy_from_slice(&Keccak256::digest(code.trim().as_bytes()));
    out
}

/// Phone side. Announce that `device` wants to pair, keyed by
/// `code_hash`. Submitted as a SPONSORED Tempo tx: the fresh device key
/// signs the sender intent (proving control of the device address that
/// gets enrolled) and the bundle `fee_payer` pays — the phone holds zero
/// of anything. The device address is recoverable from the on-chain
/// `PairingAnnounced(codeHash, device, …)` log by the desktop.
pub async fn announce_pairing_sponsored(
    device: &SigningKey,
    fee_payer: &SigningKey,
    code_hash: &[u8; 32],
    pubkey: &[u8],
    fee_token: &str,
) -> Result<String, String> {
    // ABI: announcePairing(bytes32 codeHash, bytes pubkey).
    // head = [codeHash, offset=0x40]; tail at 0x40 = [len][data(padded)].
    let padded = pubkey.len().div_ceil(32) * 32;
    let mut input = Vec::with_capacity(4 + 32 + 32 + 32 + padded);
    input.extend_from_slice(&selector("announcePairing(bytes32,bytes)"));
    input.extend_from_slice(code_hash);
    input.extend_from_slice(&u256_be(0x40));
    input.extend_from_slice(&u256_be(pubkey.len() as u128));
    input.extend_from_slice(pubkey);
    input.resize(4 + 32 + 32 + 32 + padded, 0);
    let call = crate::tempo_tx::TempoCall {
        to: parse_eth_address(REGISTRY_ADDRESS)?,
        value_wei: 0,
        input,
    };
    // announcePairing inner is one event emit (~30k). Plus ~275k Tempo
    // sponsorship overhead.
    submit_tempo_sponsored(device, fee_payer, vec![call], fee_token, 450_000).await
}

/// Desktop side. Poll for a `PairingAnnounced` log matching `code_hash`
/// and return the announcing device's `(address, compressed_pubkey)`, or
/// `None` if no device has announced yet. The pubkey lets the desktop
/// ECIES-wrap the Gemini key directly to the device. Scans the recent
/// ~99k-block window (Tempo's `eth_getLogs` cap).
pub async fn find_pairing_device(
    code_hash: &[u8; 32],
) -> Result<Option<(String, Vec<u8>)>, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(None);
    }
    use sha3::{Digest, Keccak256};
    let topic0 = format!(
        "0x{}",
        bytes_to_hex(&Keccak256::digest(
            b"PairingAnnounced(bytes32,address,bytes,uint256)"
        ))
    );
    let code_topic = format!("0x{}", bytes_to_hex(code_hash));

    let latest_hex = rpc("eth_blockNumber", serde_json::json!([])).await?;
    let latest = parse_hex_quantity(&latest_hex)? as u64;
    let from = latest.saturating_sub(99_000);
    let from_hex = format!("0x{from:x}");

    // Filter by topic0 (event sig) AND topic1 (indexed codeHash) so only
    // an announcement for THIS code comes back.
    let logs = eth_get_logs(
        REGISTRY_ADDRESS,
        vec![serde_json::json!(topic0), serde_json::json!(code_topic)],
        &from_hex,
    )
    .await?;

    for log in &logs {
        let topics = log.get("topics").and_then(|t| t.as_array());
        let device = topics
            .and_then(|t| t.get(2))
            .and_then(|t| t.as_str())
            .map(|s| s.trim_start_matches("0x"))
            .filter(|s| s.len() >= 64)
            .map(|s| format!("0x{}", &s[24..]).to_lowercase());
        let Some(device) = device else { continue };

        // data = [offset(0x40)][timestamp][pubkey_len][pubkey…].
        let data_hex = log.get("data").and_then(|d| d.as_str()).unwrap_or("0x");
        let data = hex_to_bytes(data_hex).unwrap_or_default();
        let pubkey = if data.len() >= 96 {
            let mut len_buf = [0u8; 8];
            len_buf.copy_from_slice(&data[88..96]);
            let len = u64::from_be_bytes(len_buf) as usize;
            len.checked_add(96)
                .and_then(|end| data.get(96..end))
                .map(|s| s.to_vec())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        return Ok(Some((device, pubkey)));
    }
    Ok(None)
}

/// Per-device metadata slot for a Gemini key ECIES-wrapped to one device:
/// `keccak256("localharness.gemini_key.dev." || device_address)`. Each
/// linked device gets its own slot under the MAIN tokenId, so the desktop
/// can wrap the key to each device independently.
fn gemini_key_dev_metadata_key(device_addr: &[u8; 20]) -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let mut hasher = Keccak256::new();
    hasher.update(b"localharness.gemini_key.dev.");
    hasher.update(device_addr);
    let mut out = [0u8; 32];
    out.copy_from_slice(&hasher.finalize());
    out
}

/// Read the ECIES-wrapped Gemini key blob a desktop posted for one
/// device, if any. The device decrypts it with its own signing key.
pub async fn wrapped_device_key_of(
    token_id: u64,
    device_addr_hex: &str,
) -> Result<Option<Vec<u8>>, String> {
    let device_addr = parse_eth_address(device_addr_hex)?;
    let mut data = Vec::with_capacity(4 + 64);
    data.extend_from_slice(&selector("metadata(uint256,bytes32)"));
    data.extend_from_slice(&u256_be(token_id as u128));
    data.extend_from_slice(&gemini_key_dev_metadata_key(&device_addr));
    let calldata = format!("0x{}", bytes_to_hex(&data));
    let result_hex = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    let bytes = hex_to_bytes(&result_hex)?;
    if bytes.len() < 64 {
        return Ok(None);
    }
    let mut len_buf = [0u8; 8];
    len_buf.copy_from_slice(&bytes[56..64]);
    let len = u64::from_be_bytes(len_buf) as usize;
    if len == 0 {
        return Ok(None);
    }
    let payload = len
        .checked_add(64)
        .and_then(|end| bytes.get(64..end))
        .ok_or_else(|| "wrapped key truncated".to_string())?;
    Ok(Some(payload.to_vec()))
}

/// Desktop side. Post an ECIES-wrapped Gemini key blob for one device
/// under its per-device MAIN metadata slot. `sender` is the MAIN's NFT
/// holder (the apex master wallet — only the owner can setMetadata);
/// `fee_payer` is the bundle sponsor. The phone reads it back via
/// [`wrapped_device_key_of`] and decrypts with its device key.
pub async fn set_device_wrapped_key_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    token_id: u64,
    device_addr_hex: &str,
    blob: &[u8],
    fee_token: &str,
) -> Result<String, String> {
    let device_addr = parse_eth_address(device_addr_hex)?;
    let key = gemini_key_dev_metadata_key(&device_addr);
    let len = blob.len();
    let padded = len.div_ceil(32) * 32;
    let mut input = Vec::with_capacity(4 + 96 + 32 + padded);
    input.extend_from_slice(&selector("setMetadata(uint256,bytes32,bytes)"));
    input.extend_from_slice(&u256_be(token_id as u128));
    input.extend_from_slice(&key);
    input.extend_from_slice(&u256_be(0x60));
    input.extend_from_slice(&u256_be(len as u128));
    input.extend_from_slice(blob);
    input.resize(4 + 96 + 32 + padded, 0);
    let call = crate::tempo_tx::TempoCall {
        to: parse_eth_address(REGISTRY_ADDRESS)?,
        value_wei: 0,
        input,
    };
    let words = (padded / 32 + 4) as u128;
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, 1_200_000 + words * 40_000).await
}

// --- Registration cost (LocalharnessRegistryFacet on the diamond) ---

/// `eth_call mainCost()` — the LH amount the diamond's `registerMain`
/// pulls from the caller via transferFrom on every MAIN change. Zero
/// means the gate is off.
pub async fn main_cost() -> Result<u128, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(0);
    }
    let calldata = format!("0x{}", bytes_to_hex(&selector("mainCost()")));
    let result = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    decode_u256_as_u128(&result)
}

/// `eth_call treasuryBalance()` — total LH the diamond holds. Reads
/// the credits token's `balanceOf(diamond)`. Useful for surfacing
/// "X LH collected from registrations" in admin UIs.
pub async fn treasury_balance() -> Result<u128, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(0);
    }
    let calldata = format!("0x{}", bytes_to_hex(&selector("treasuryBalance()")));
    let result = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    decode_u256_as_u128(&result)
}

/// `eth_call registrationCost()` — the LH amount (in token wei, 18
/// decimals) the diamond's `register(name)` will pull from the sender
/// via transferFrom. Zero means the cost gate is disabled.
pub async fn registration_cost() -> Result<u128, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(0);
    }
    let calldata = format!("0x{}", bytes_to_hex(&selector("registrationCost()")));
    let result = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    decode_u256_as_u128(&result)
}

/// Encode `approve(spender, amount)` calldata for an ERC-20 token.
fn encode_approve(spender: &[u8; 20], amount_wei: u128) -> Vec<u8> {
    let sel = selector("approve(address,uint256)");
    let mut spender_padded = [0u8; 32];
    spender_padded[12..].copy_from_slice(spender);
    let amount_padded = u256_be(amount_wei);
    let mut out = Vec::with_capacity(4 + 32 + 32);
    out.extend_from_slice(&sel);
    out.extend_from_slice(&spender_padded);
    out.extend_from_slice(&amount_padded);
    out
}

/// ERC-20 `transfer(to, amount)` calldata — same shape as `encode_approve`
/// with the `transfer` selector.
fn encode_transfer(to: &[u8; 20], amount_wei: u128) -> Vec<u8> {
    let sel = selector("transfer(address,uint256)");
    let mut to_padded = [0u8; 32];
    to_padded[12..].copy_from_slice(to);
    let amount_padded = u256_be(amount_wei);
    let mut out = Vec::with_capacity(4 + 32 + 32);
    out.extend_from_slice(&sel);
    out.extend_from_slice(&to_padded);
    out.extend_from_slice(&amount_padded);
    out
}

// --- Credits / daily allowance (CreditsFacet on the diamond) ---------

/// Sign + submit `CreditsFacet.claimDaily()` as a sponsored Tempo tx.
/// User holds zero of anything; sponsor pays AlphaUSD. The on-chain
/// `msg.sender` is the user (the diamond mints credits TO `msg.sender`),
/// so the sponsorship channel only covers the fee — never the issuance.
/// Reverts on-chain if the caller has already claimed this UTC day.
pub async fn claim_daily_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    fee_token: &str,
) -> Result<String, String> {
    let call = crate::tempo_tx::TempoCall {
        to: parse_eth_address(REGISTRY_ADDRESS)?,
        value_wei: 0,
        input: selector("claimDaily()").to_vec(),
    };
    // claimDaily inner: a single SSTORE + mint (token Transfer event +
    // memo event) — ~120k. Plus ~275k Tempo sponsorship overhead.
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, 600_000).await
}

/// `eth_call canClaim(account)` — true iff `account` is eligible to
/// call `claimDaily()` right now (token configured, allowance > 0,
/// not yet claimed this UTC day).
pub async fn can_claim_credits(account_hex: &str) -> Result<bool, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(false);
    }
    let mut data = Vec::with_capacity(4 + 32);
    data.extend_from_slice(&selector("canClaim(address)"));
    let account_bytes = hex_to_bytes(account_hex)?;
    if account_bytes.len() != 20 {
        return Err(format!("account must be 20 bytes, got {}", account_bytes.len()));
    }
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(&account_bytes);
    data.extend_from_slice(&padded);
    let calldata = format!("0x{}", bytes_to_hex(&data));
    let result_hex = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    let trimmed = result_hex.trim().trim_start_matches("0x");
    Ok(trimmed.chars().last().map(|c| c == '1').unwrap_or(false))
}

/// `eth_call dailyAllowance()` — the current per-claim amount in
/// 18-decimal token wei.
pub async fn daily_allowance() -> Result<u128, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(0);
    }
    let calldata = format!("0x{}", bytes_to_hex(&selector("dailyAllowance()")));
    let result = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    decode_u256_as_u128(&result)
}

/// `eth_call lastClaimDay(account)` — the UTC day number (block.timestamp / 86400)
/// of the account's most recent claimDaily(). Returns 0 if never claimed.
pub async fn last_claim_day(account_hex: &str) -> Result<u64, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(0);
    }
    let mut data = Vec::with_capacity(4 + 32);
    data.extend_from_slice(&selector("lastClaimDay(address)"));
    let account_bytes = hex_to_bytes(account_hex)?;
    if account_bytes.len() != 20 {
        return Err(format!("account must be 20 bytes, got {}", account_bytes.len()));
    }
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(&account_bytes);
    data.extend_from_slice(&padded);
    let calldata = format!("0x{}", bytes_to_hex(&data));
    let result_hex = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    let val = decode_u256_as_u128(&result_hex)?;
    Ok(val as u64)
}

/// Sponsored Tempo tx that calls `tba.execute(target, value, data, 0)`
/// on a `MultiSignerAccount` TBA. The TBA must be deployed; we batch
/// `createTokenBoundAccount(token_id)` first so the call is safe on
/// counterfactual TBAs too (createTokenBoundAccount is idempotent).
///
/// `sender` must be one of the TBA's authorized signers: the NFT
/// holder of the owning token, or an EOA previously added via
/// `addSigner`. The TBA's `execute` revert "not authorised" otherwise.
// Discrete params are the TBA-execute tx fields (signers, token, target,
// value, calldata, fee token, gas); bundling them into a struct would
// just move the noise. Kept flat as a low-level wire helper.
#[allow(clippy::too_many_arguments)]
pub async fn tba_execute_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    token_id: u64,
    tba_address: &str,
    target_hex: &str,
    value_wei: u128,
    inner_data: Vec<u8>,
    fee_token: &str,
    gas_limit: u128,
) -> Result<String, String> {
    let tba_addr = parse_eth_address(tba_address)?;
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let target = parse_eth_address(target_hex)?;

    let create_call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_create_tba(token_id),
    };
    let execute_call = crate::tempo_tx::TempoCall {
        to: tba_addr,
        value_wei: 0,
        input: encode_tba_execute(&target, value_wei, &inner_data),
    };
    submit_tempo_sponsored(
        sender,
        fee_payer,
        vec![create_call, execute_call],
        fee_token,
        gas_limit,
    )
    .await
}

/// Convenience: send LH from `token_id`'s TBA to a recipient. Wraps
/// `tba_execute_sponsored` with credits.transfer calldata pre-built.
/// The TBA must hold enough LH to cover `amount_wei`.
pub async fn tba_transfer_lh_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    token_id: u64,
    tba_address: &str,
    recipient_hex: &str,
    amount_wei: u128,
    fee_token: &str,
) -> Result<String, String> {
    let recipient = parse_eth_address(recipient_hex)?;
    let mut transfer_data = Vec::with_capacity(4 + 32 + 32);
    transfer_data.extend_from_slice(&selector("transfer(address,uint256)"));
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(&recipient);
    transfer_data.extend_from_slice(&padded);
    transfer_data.extend_from_slice(&u256_be(amount_wei));

    tba_execute_sponsored(
        sender,
        fee_payer,
        token_id,
        tba_address,
        LOCALHARNESS_TOKEN_ADDRESS,
        0,
        transfer_data,
        fee_token,
        // create TBA — ~742k live-measured on a COLD first deploy
        // (CREATE2 of the full MultiSignerAccount), near-zero idempotent
        // thereafter — + execute (~30k) + inner ERC-20 transfer (~52k) +
        // Tempo sponsorship (~275k). A first transfer from an
        // undeployed TBA needs ~1.1M, so 800k would revert out-of-gas;
        // 2M covers the cold path and is free on the warm one (sponsor
        // billed on gas USED, not the limit).
        2_000_000,
    )
    .await
}

fn encode_tba_execute(target: &[u8; 20], value_wei: u128, data: &[u8]) -> Vec<u8> {
    // execute(address,uint256,bytes,uint8) — ABI:
    //   selector(4) | target(32) | value(32) | dataOffset(32, =0x80) |
    //   operation(32, =0) | dataLength(32) | dataPadded
    let sel = selector("execute(address,uint256,bytes,uint8)");
    let mut target_padded = [0u8; 32];
    target_padded[12..].copy_from_slice(target);
    let data_len = data.len();
    let padded_len = data_len.div_ceil(32) * 32;
    // Static head = target(32) + value(32) + offset(32) + operation(32) = 128
    let data_offset: u128 = 0x80;

    let mut out = Vec::with_capacity(4 + 128 + 32 + padded_len);
    out.extend_from_slice(&sel);
    out.extend_from_slice(&target_padded);
    out.extend_from_slice(&u256_be(value_wei));
    out.extend_from_slice(&u256_be(data_offset));
    out.extend_from_slice(&u256_be(0)); // operation = 0 (CALL)
    out.extend_from_slice(&u256_be(data_len as u128));
    out.extend_from_slice(data);
    out.resize(out.len() + (padded_len - data_len), 0);
    out
}

fn encode_create_tba(token_id: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(4 + 32);
    data.extend_from_slice(&selector("createTokenBoundAccount(uint256)"));
    data.extend_from_slice(&u256_be(token_id as u128));
    data
}

fn encode_add_signer(addr: &[u8; 20]) -> Vec<u8> {
    let sel = selector("addSigner(address)");
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(addr);
    let mut out = Vec::with_capacity(4 + 32);
    out.extend_from_slice(&sel);
    out.extend_from_slice(&padded);
    out
}

fn encode_remove_signer(addr: &[u8; 20]) -> Vec<u8> {
    let sel = selector("removeSigner(address)");
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(addr);
    let mut out = Vec::with_capacity(4 + 32);
    out.extend_from_slice(&sel);
    out.extend_from_slice(&padded);
    out
}

/// Convenience for the first-claim flow: register `name` on-chain, then
/// IF the caller has no MAIN registered yet, set the newly-minted token
/// as their MAIN in a second tx. Idempotent on the MAIN side — re-runs
/// after the user already has a MAIN are a no-op. Errors on the MAIN
/// leg are logged and swallowed (the name claim is what matters for
/// correctness; the MAIN flag is an enhancement).
pub async fn claim_and_maybe_set_main(
    signer: &SigningKey,
    name: &str,
) -> Result<String, String> {
    let tx_hash = claim_name(signer, name).await?;
    let addr_hex = address_to_hex(&wallet::address(signer));
    match main_of(&addr_hex).await {
        Ok(0) => {
            // No MAIN yet — find the freshly-minted token id and set it.
            if let Ok(Status::Taken { agent_id }) = check_name(name).await {
                if let Err(err) = register_main(signer, agent_id).await {
                    log_main_warning(&err);
                }
            }
        }
        Ok(_) => {} // already has a MAIN; leave it alone
        Err(err) => log_main_warning(&err),
    }
    Ok(tx_hash)
}

/// Same as `claim_and_maybe_set_main` but uses Tempo's sponsored-tx
/// flow: the `sender` signs the intent (and needs zero balance);
/// `fee_payer` signs to cover gas in `fee_token` (typically AlphaUSD).
/// This is what the bundle uses for first-claim onboarding — the user
/// who just visited the page can claim a subdomain without holding
/// any tokens.
///
/// If the diamond's `registrationCost()` is non-zero, this batches a
/// `LocalharnessCredits.approve(diamond, cost)` call BEFORE register
/// in the same Tempo tx — register then pulls the credits via
/// `transferFrom` inside its own body. User pays the cost in LH from
/// their balance; the credits accumulate at the diamond's address.
pub async fn claim_and_maybe_set_main_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    name: &str,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let token_addr = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS)?;

    let cost = registration_cost().await.unwrap_or(0);

    let register_input = hex_to_bytes(&encode_register(name))?;
    let register_call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: register_input,
    };

    let calls = if cost > 0 {
        let approve_call = crate::tempo_tx::TempoCall {
            to: token_addr,
            value_wei: 0,
            input: encode_approve(&diamond_addr, cost),
        };
        vec![approve_call, register_call]
    } else {
        vec![register_call]
    };

    let tx_hash = submit_tempo_sponsored(
        sender,
        fee_payer,
        calls,
        fee_token,
        // `eth_estimateGas` on `register(name)` against the live diamond
        // reports ~1.32M gas for the inner call (ERC-721 mint + storage
        // writes + counterfactual TBA address derivation). Sponsorship
        // (fee_payer recovery + AlphaUSD transfer) adds ~275k. The
        // approve+transferFrom pair adds ~80k. Budget 2.2M for
        // headroom; sponsor pays in AlphaUSD and only consumed gas is
        // debited, so over-budgeting is free.
        2_200_000,
    )
    .await?;

    // After register, fetch the new tokenId and set MAIN if none.
    let sender_addr = address_to_hex(&wallet::address(sender));
    if let Ok(0) = main_of(&sender_addr).await {
        if let Ok(Status::Taken { agent_id }) = check_name(name).await {
            if let Err(err) =
                register_main_sponsored(sender, fee_payer, agent_id, fee_token).await
            {
                log_main_warning(&err);
            }
        }
    }
    Ok(tx_hash)
}

// --- Redeem codes + credit sessions ----------------------------------
//
// These back the `$LH` credit-proxy bootstrap: `redeem` mints credits
// from a one-time code (RedeemFacet), `open_session` spends credits to
// open a time-bounded usage session the Vercel Edge proxy reads via
// `session_expiry_of` on every request (SessionFacet). See
// `[[project-credit-proxy-monetization]]`.

/// Encode `redeem(string)` calldata. Same dynamic-string ABI shape as
/// `encode_register`.
fn encode_redeem(code: &str) -> Vec<u8> {
    let sel = selector("redeem(string)");
    let bytes = code.as_bytes();
    let len = bytes.len();
    let padded_len = len.div_ceil(32) * 32;

    let mut buf = Vec::with_capacity(4 + 32 + 32 + padded_len);
    buf.extend_from_slice(&sel);
    buf.extend_from_slice(&u256_be(0x20));
    buf.extend_from_slice(&u256_be(len as u128));
    buf.extend_from_slice(bytes);
    buf.resize(4 + 32 + 32 + padded_len, 0);
    buf
}

/// Redeem a one-time code for `$LH`, via a sponsored Tempo tx so the
/// caller needs zero balance. The plaintext `code` is hashed on-chain
/// (`keccak256`) and matched against the owner-loaded set; the credits
/// are minted to `sender`.
pub async fn redeem_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    code: &str,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_redeem(code),
    };
    // redeem mints on the credits token (cold balanceOf + totalSupply
    // SSTOREs, AccessControl role checks, memo event) plus the claimed-flag
    // SSTORE — empirically ~1.07M inner, NOT the ~120k first assumed (a 600k
    // limit silently out-of-gassed every redeem). Plus ~275k sponsorship.
    // 2M gives headroom; sponsor is billed on gas used, not the limit.
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, 2_000_000).await
}

/// Read `sessionExpiryOf(address)` — unix-seconds expiry of the
/// account's current credit session (0 / past = none). The credit
/// proxy makes this same call on every request.
pub async fn session_expiry_of(account_hex: &str) -> Result<u64, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(0);
    }
    let account = parse_eth_address(account_hex)?;
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(&account);
    let mut calldata = Vec::with_capacity(4 + 32);
    calldata.extend_from_slice(&selector("sessionExpiryOf(address)"));
    calldata.extend_from_slice(&padded);
    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(REGISTRY_ADDRESS, &calldata_hex).await?;
    decode_u256_as_u64(&result)
}

/// Read `sessionPrice()` — `$LH` (wei) required to open one session.
pub async fn session_price() -> Result<u128, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(0);
    }
    let calldata = format!("0x{}", bytes_to_hex(&selector("sessionPrice()")));
    let result = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    decode_u256_as_u128(&result)
}

/// Open (or renew) the caller's credit session via a sponsored Tempo
/// tx. When `sessionPrice()` is non-zero, batches a
/// `LocalharnessCredits.approve(diamond, price)` call BEFORE
/// `openSession()` in the same tx — `openSession` then pulls the
/// credits via `transferFrom` inside its own body (same cost-gate
/// pattern as `register`).
pub async fn open_session_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let token_addr = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS)?;

    let price = session_price().await.unwrap_or(0);

    let open_call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: selector("openSession()").to_vec(),
    };

    let calls = if price > 0 {
        let approve_call = crate::tempo_tx::TempoCall {
            to: token_addr,
            value_wei: 0,
            input: encode_approve(&diamond_addr, price),
        };
        vec![approve_call, open_call]
    } else {
        vec![open_call]
    };

    // approve (~46k) + openSession (transferFrom + 1 SSTORE + event,
    // ~90k) + ~275k sponsorship. 600k headroom.
    submit_tempo_sponsored(sender, fee_payer, calls, fee_token, 600_000).await
}

fn encode_deposit_credits(amount_wei: u128) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 32);
    out.extend_from_slice(&selector("depositCredits(uint256)"));
    out.extend_from_slice(&u256_be(amount_wei));
    out
}

/// Read `creditOf(address)` — the user's prepaid per-request `$LH`
/// balance in the credit meter (the proxy reads this to gate a call).
pub async fn credit_balance_of(account_hex: &str) -> Result<u128, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(0);
    }
    let account = parse_eth_address(account_hex)?;
    let mut padded = [0u8; 32];
    padded[12..].copy_from_slice(&account);
    let mut calldata = Vec::with_capacity(4 + 32);
    calldata.extend_from_slice(&selector("creditOf(address)"));
    calldata.extend_from_slice(&padded);
    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(REGISTRY_ADDRESS, &calldata_hex).await?;
    decode_u256_as_u128(&result)
}

/// Prepay `$LH` into the per-request credit meter via a sponsored Tempo
/// tx — batches `approve(diamond, amount)` + `depositCredits(amount)`
/// (same cost-gate shape as `open_session_sponsored`).
pub async fn deposit_credits_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    amount_wei: u128,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let token_addr = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS)?;
    let approve_call = crate::tempo_tx::TempoCall {
        to: token_addr,
        value_wei: 0,
        input: encode_approve(&diamond_addr, amount_wei),
    };
    let deposit_call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_deposit_credits(amount_wei),
    };
    // approve + transferFrom (pull $LH into the diamond) + cold meter-
    // balance SSTORE + event. Like redeem, comfortably more than the old
    // 600k once cold SSTOREs are counted — 1.5M gives headroom.
    submit_tempo_sponsored(sender, fee_payer, vec![approve_call, deposit_call], fee_token, 1_500_000)
        .await
}

// --- x402 payment authorization (settled in $LH via X402Facet) -------
//
// EIP-712 "exact"-scheme settlement for agent-to-agent payments. The
// payer signs a `PaymentAuthorization` (gasless); the payee submits
// `settle`. Domain/typehash MUST match `contracts/src/facets/X402Facet.sol`
// — the `x402_domain_matches_live_facet` test pins it to the deployed
// diamond.

fn keccak32(data: &[u8]) -> [u8; 32] {
    let mut h = Keccak256::new();
    h.update(data);
    let d = h.finalize();
    let mut o = [0u8; 32];
    o.copy_from_slice(&d);
    o
}

fn addr_word(a: &[u8; 20]) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[12..].copy_from_slice(a);
    w
}

/// EIP-712 domain separator for the x402 facet (name "localharness-x402",
/// version "1", `CHAIN_ID`, diamond). Matches `x402DomainSeparator()`.
pub fn x402_domain_separator() -> Result<[u8; 32], String> {
    let diamond = parse_eth_address(REGISTRY_ADDRESS)?;
    let mut dom = Vec::with_capacity(160);
    dom.extend_from_slice(&keccak32(
        b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
    ));
    dom.extend_from_slice(&keccak32(b"localharness-x402"));
    dom.extend_from_slice(&keccak32(b"1"));
    dom.extend_from_slice(&u256_be(CHAIN_ID as u128));
    dom.extend_from_slice(&addr_word(&diamond));
    Ok(keccak32(&dom))
}

/// EIP-712 digest of an x402 `PaymentAuthorization` (what the payer signs).
pub fn x402_digest(
    from: &[u8; 20],
    to: &[u8; 20],
    value_wei: u128,
    valid_after: u64,
    valid_before: u64,
    nonce: &[u8; 32],
) -> Result<[u8; 32], String> {
    let mut st = Vec::with_capacity(224);
    st.extend_from_slice(&keccak32(
        b"PaymentAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)",
    ));
    st.extend_from_slice(&addr_word(from));
    st.extend_from_slice(&addr_word(to));
    st.extend_from_slice(&u256_be(value_wei));
    st.extend_from_slice(&u256_be(valid_after as u128));
    st.extend_from_slice(&u256_be(valid_before as u128));
    st.extend_from_slice(nonce);
    let struct_hash = keccak32(&st);

    let mut pre = Vec::with_capacity(66);
    pre.extend_from_slice(&[0x19, 0x01]);
    pre.extend_from_slice(&x402_domain_separator()?);
    pre.extend_from_slice(&struct_hash);
    Ok(keccak32(&pre))
}

/// Sign an x402 authorization with an EOA key — the 65-byte sig that
/// goes in the `X-PAYMENT` payload. (k256 emits low-s, which the facet
/// requires.) Agents paying from a contract TBA sign via EIP-1271 paths
/// instead; this is the EOA fast path.
pub fn sign_x402(
    signer: &SigningKey,
    from: &[u8; 20],
    to: &[u8; 20],
    value_wei: u128,
    valid_after: u64,
    valid_before: u64,
    nonce: &[u8; 32],
) -> Result<[u8; 65], String> {
    let digest = x402_digest(from, to, value_wei, valid_after, valid_before, nonce)?;
    Ok(crate::wallet::sign_hash(signer, &digest))
}

fn encode_settle(
    from: &[u8; 20],
    to: &[u8; 20],
    value_wei: u128,
    valid_after: u64,
    valid_before: u64,
    nonce: &[u8; 32],
    signature: &[u8; 65],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 32 * 9 + 96);
    out.extend_from_slice(&selector(
        "settle(address,address,uint256,uint256,uint256,bytes32,bytes)",
    ));
    out.extend_from_slice(&addr_word(from));
    out.extend_from_slice(&addr_word(to));
    out.extend_from_slice(&u256_be(value_wei));
    out.extend_from_slice(&u256_be(valid_after as u128));
    out.extend_from_slice(&u256_be(valid_before as u128));
    out.extend_from_slice(nonce);
    out.extend_from_slice(&u256_be(7 * 32)); // offset to the `bytes` arg
    out.extend_from_slice(&u256_be(signature.len() as u128)); // 65
    out.extend_from_slice(signature);
    out.resize(out.len() + 31, 0); // pad 65 -> 96 (32-byte multiple)
    out
}

/// Submit an x402 settlement (sponsored). `submitter` is the payee /
/// facilitator (signs the Tempo tx); fees paid by `fee_payer`. Moves
/// `value_wei` `$LH` from the signed authorization's payer to `to`.
/// The payer must have `approve`d the diamond for `$LH` once.
#[allow(clippy::too_many_arguments)]
pub async fn settle_x402_sponsored(
    submitter: &SigningKey,
    fee_payer: &SigningKey,
    from: &[u8; 20],
    to: &[u8; 20],
    value_wei: u128,
    valid_after: u64,
    valid_before: u64,
    nonce: &[u8; 32],
    signature: &[u8; 65],
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_settle(from, to, value_wei, valid_after, valid_before, nonce, signature),
    };
    submit_tempo_sponsored(submitter, fee_payer, vec![call], fee_token, 400_000).await
}

/// Read `authorizationState(from, nonce)` — true if that x402 nonce was
/// already settled (lets a payee detect replays before serving).
pub async fn x402_authorization_state(
    from_hex: &str,
    nonce: &[u8; 32],
) -> Result<bool, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(false);
    }
    let from = parse_eth_address(from_hex)?;
    let mut calldata = Vec::with_capacity(4 + 64);
    calldata.extend_from_slice(&selector("authorizationState(address,bytes32)"));
    calldata.extend_from_slice(&addr_word(&from));
    calldata.extend_from_slice(nonce);
    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(REGISTRY_ADDRESS, &calldata_hex).await?;
    Ok(decode_u256_as_u64(&result).map(|v| v != 0).unwrap_or(false))
}

/// A fresh random 32-byte x402 nonce (CSPRNG via `getrandom`). Each
/// `PaymentAuthorization` needs a unique nonce — the on-chain `settle`
/// records it one-shot, so a replayed nonce reverts.
pub fn random_x402_nonce() -> [u8; 32] {
    use rand_core::RngCore;
    let mut n = [0u8; 32];
    rand_core::OsRng.fill_bytes(&mut n);
    n
}

/// `eth_call allowance(owner, spender)` on [`LOCALHARNESS_TOKEN_ADDRESS`] —
/// how much `$LH` (18-decimal wei) `owner` has approved `spender` to pull
/// via `transferFrom`. The x402 `settle` pulls `$LH` from the payer through
/// the diamond's `transferFrom`, so the payer must have approved the diamond
/// (`REGISTRY_ADDRESS`) for at least the payment value; this lets the client
/// check before paying and approve if short.
pub async fn lh_allowance(owner_hex: &str, spender_hex: &str) -> Result<u128, String> {
    if LOCALHARNESS_TOKEN_ADDRESS == zero_address() {
        return Ok(0);
    }
    let owner = parse_eth_address(owner_hex)?;
    let spender = parse_eth_address(spender_hex)?;
    let mut calldata = Vec::with_capacity(4 + 64);
    calldata.extend_from_slice(&selector("allowance(address,address)"));
    calldata.extend_from_slice(&addr_word(&owner));
    calldata.extend_from_slice(&addr_word(&spender));
    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(LOCALHARNESS_TOKEN_ADDRESS, &calldata_hex).await?;
    decode_u256_as_u128(&result)
}

/// Approve `spender` to pull up to `amount_wei` `$LH` from `sender` via a
/// sponsored Tempo tx (sender holds zero gas; `fee_payer` pays AlphaUSD).
/// The x402 prerequisite: before paying an agent over `/mcp`, the payer
/// approves the diamond (`REGISTRY_ADDRESS`) so `settle`'s `transferFrom`
/// succeeds. Pass a large/`u128::MAX` amount to approve once and reuse.
pub async fn approve_lh_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    spender_hex: &str,
    amount_wei: u128,
    fee_token: &str,
) -> Result<String, String> {
    let token_addr = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS)?;
    let spender = parse_eth_address(spender_hex)?;
    let approve_call = crate::tempo_tx::TempoCall {
        to: token_addr,
        value_wei: 0,
        input: encode_approve(&spender, amount_wei),
    };
    // approve is a single SSTORE (cold the first time) + event. 300k is
    // ample headroom on top of the AA-settlement overhead.
    submit_tempo_sponsored(sender, fee_payer, vec![approve_call], fee_token, 300_000).await
}

/// Transfer `amount_wei` `$LH` from `sender` to `to_hex` as a sponsored Tempo tx
/// (sponsor pays AlphaUSD; sender holds zero native). The CLI/native twin of the
/// browser `send_lh` tool — "one agent sends another `$LH`", the same effect as a
/// redeem code (controlled funding now that the daily allowance is disabled).
pub async fn transfer_lh_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    to_hex: &str,
    amount_wei: u128,
    fee_token: &str,
) -> Result<String, String> {
    let token_addr = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS)?;
    let to = parse_eth_address(to_hex)?;
    let transfer_call = crate::tempo_tx::TempoCall {
        to: token_addr,
        value_wei: 0,
        input: encode_transfer(&to, amount_wei),
    };
    submit_tempo_sponsored(sender, fee_payer, vec![transfer_call], fee_token, 300_000).await
}

// --- Scheduling (ScheduleFacet on the diamond) -----------------------
//
// The durable, tab-independent job registry: a holder ESCROWS `$LH` to
// back a recurring job that runs a `<name>.localharness.xyz` agent on a
// fixed interval (the on-chain answer to "persistent without keeping the
// tab open"). Escrow is the same approve-then-call shape `depositCredits`
// uses — `scheduleJob` does `transferFrom(caller -> diamond, budgetWei)`
// inside its body, so the bundle batches `approve(diamond, budgetWei)` on
// `$LH` + `scheduleJob(...)` into ONE sponsored Tempo tx. See
// `contracts/src/facets/ScheduleFacet.sol`.

/// One scheduled job, decoded from `getJob(uint256)`. Field order/types
/// mirror `LibScheduleStorage.Job` exactly (the ABI tuple `getJob` returns).
/// `status` is the raw `Status` enum byte: 0 Active / 1 Paused / 2 Cancelled
/// / 3 Exhausted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledJob {
    /// Who scheduled it (refund recipient / billing identity), 0x-hex.
    pub owner: String,
    /// Seconds between runs (the cadence).
    pub interval: u64,
    /// Raw lifecycle byte: 0 Active, 1 Paused, 2 Cancelled, 3 Exhausted.
    pub status: u8,
    /// Unix seconds of the next due fire (the CAS key; 0 once terminal).
    pub next_run: u64,
    /// `$LH` (wei) still escrowed for this job — debited per run, refundable.
    pub budget_wei: u128,
    /// Remaining runs (the hard count cap); hitting 0 → Exhausted.
    pub runs_left: u32,
    /// tokenId of the agent run each tick (name resolved off-chain).
    pub target_id: u64,
}

impl ScheduledJob {
    /// Human-readable lifecycle label for the raw `status` byte.
    pub fn status_label(&self) -> &'static str {
        match self.status {
            0 => "active",
            1 => "paused",
            2 => "cancelled",
            3 => "exhausted",
            _ => "unknown",
        }
    }
}

/// Encode `scheduleJob(uint256 targetId, bytes task, uint64 interval,
/// uint128 budgetWei, uint32 maxRuns)` calldata. `task` is a DYNAMIC `bytes`
/// arg, so the head holds an OFFSET to a tail of `[length][padded data]`
/// (same dynamic-bytes layout `encode_settle`'s signature uses). The four
/// scalars are static head words (uint64/uint128/uint32 right-aligned, the
/// 5-word fixed head means the bytes offset is always `5 * 32`).
fn encode_schedule_job(
    target_id: u64,
    task: &[u8],
    interval_secs: u64,
    budget_wei: u128,
    max_runs: u32,
) -> Vec<u8> {
    let padded_len = task.len().div_ceil(32) * 32;
    let mut out = Vec::with_capacity(4 + 5 * 32 + 32 + padded_len);
    out.extend_from_slice(&selector("scheduleJob(uint256,bytes,uint64,uint128,uint32)"));
    // Head word 0: targetId (uint256).
    out.extend_from_slice(&u256_be(target_id as u128));
    // Head word 1: offset to the `bytes task` tail — 5 fixed head words.
    out.extend_from_slice(&u256_be(5 * 32));
    // Head words 2..5: interval / budgetWei / maxRuns (each right-aligned).
    out.extend_from_slice(&u256_be(interval_secs as u128));
    out.extend_from_slice(&u256_be(budget_wei));
    out.extend_from_slice(&u256_be(max_runs as u128));
    // Tail: length + the task bytes, right-padded to a 32-byte multiple.
    out.extend_from_slice(&u256_be(task.len() as u128));
    out.extend_from_slice(task);
    out.resize(out.len() + (padded_len - task.len()), 0);
    out
}

fn encode_cancel_job(job_id: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 32);
    out.extend_from_slice(&selector("cancelJob(uint256)"));
    out.extend_from_slice(&u256_be(job_id as u128));
    out
}

/// Schedule a recurring job via a sponsored Tempo tx. Batches
/// `approve(diamond, budgetWei)` on `$LH` + `scheduleJob(targetId, task,
/// interval, budgetWei, maxRuns)` in ONE tx — `scheduleJob` then escrows the
/// budget via `transferFrom` inside its own body (same cost-gate shape as
/// `deposit_credits_sponsored`). Returns the tx hash once mined; read the new
/// job id back from `jobs_of(owner)` (its last entry) or the `JobScheduled`
/// event.
#[allow(clippy::too_many_arguments)]
pub async fn schedule_job_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    target_id: u64,
    task: &[u8],
    interval_secs: u64,
    budget_wei: u128,
    max_runs: u32,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let token_addr = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS)?;
    let approve_call = crate::tempo_tx::TempoCall {
        to: token_addr,
        value_wei: 0,
        input: encode_approve(&diamond_addr, budget_wei),
    };
    let schedule_call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_schedule_job(target_id, task, interval_secs, budget_wei, max_runs),
    };
    // approve (~46k) + scheduleJob + ~275k sponsorship overhead. MEASURED via
    // `cast estimate`: scheduleJob alone is ~2.88M for a ~45-byte task (3 packed
    // cold job slots + the cold `task` bytes ~7.6k/BYTE + the two enumerable-index
    // pushes jobIds/jobsOfOwner + transferFrom + event). The old 1.5M base
    // OUT-OF-GASSED at ~1.9M (receipt status=false). 3.5M base + 9k/byte gives
    // comfortable headroom; the sponsor only pays gas USED, so over-budgeting is
    // free. (See the CLAUDE.md "cast estimate, never guess" gotcha.)
    let gas = 3_500_000 + (task.len() as u128) * 9_000;
    submit_tempo_sponsored(sender, fee_payer, vec![approve_call, schedule_call], fee_token, gas).await
}

/// Cancel a scheduled job via a sponsored Tempo tx — REFUNDS the job's full
/// remaining `budgetWei` to the owner (`cancelJob` is owner-gated on-chain).
pub async fn cancel_job_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    job_id: u64,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_cancel_job(job_id),
    };
    // status flip + budget zero (1 SSTORE) + the refund `transfer` + event.
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, 400_000).await
}

/// Read `jobsOf(address)` — every job id the owner has scheduled (Active +
/// terminal). The enumerable index backing the "my jobs" view.
pub async fn jobs_of(owner_hex: &str) -> Result<Vec<u64>, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(Vec::new());
    }
    let owner = parse_eth_address(owner_hex)?;
    let mut calldata = selector("jobsOf(address)").to_vec();
    calldata.extend_from_slice(&addr_word(&owner));
    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(REGISTRY_ADDRESS, &calldata_hex).await?;
    let bytes = hex_to_bytes(&result)?;
    // ABI dynamic uint256[]: [offset(32)][len(32)][id0(32)]...
    if bytes.len() < 64 {
        return Ok(Vec::new());
    }
    let mut len_buf = [0u8; 8];
    len_buf.copy_from_slice(&bytes[56..64]); // low 8 bytes of the length word
    let len = u64::from_be_bytes(len_buf) as usize;
    // Don't pre-allocate `len` (attacker-controlled → OOM); checked index math
    // below just stops the decode on a hostile length.
    let mut out = Vec::new();
    for i in 0..len {
        let start = match i.checked_mul(32).and_then(|o| o.checked_add(64)) {
            Some(s) => s,
            None => break,
        };
        // Each id is a uint256; we read the low 8 bytes (ids are monotonic
        // u64-scale counters, never near 2^64).
        let Some(word) = start.checked_add(32).and_then(|end| bytes.get(start + 24..end)) else {
            break;
        };
        let mut id_buf = [0u8; 8];
        id_buf.copy_from_slice(word);
        out.push(u64::from_be_bytes(id_buf));
    }
    Ok(out)
}

/// Read `getJob(uint256)` → the full [`ScheduledJob`] record. The returned
/// tuple is all-static (the `task` lives in its own mapping, read via
/// [`task_of`]), so it decodes as 7 consecutive ABI words in `Job` order:
/// owner, interval, status, nextRun, budgetWei, runsLeft, targetId.
pub async fn get_job(job_id: u64) -> Result<ScheduledJob, String> {
    let calldata = call_uint("getJob(uint256)", job_id);
    let result = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    let bytes = hex_to_bytes(&result)?;
    if bytes.len() < 7 * 32 {
        return Err(format!("getJob: short response {} bytes", bytes.len()));
    }
    let word = |i: usize| &bytes[i * 32..(i + 1) * 32];
    let owner = format!("0x{}", bytes_to_hex(&word(0)[12..32])); // address, low 20 bytes
    let u64_low = |w: &[u8]| {
        let mut b = [0u8; 8];
        b.copy_from_slice(&w[24..32]);
        u64::from_be_bytes(b)
    };
    let u128_low = |w: &[u8]| {
        let mut b = [0u8; 16];
        b.copy_from_slice(&w[16..32]);
        u128::from_be_bytes(b)
    };
    Ok(ScheduledJob {
        owner,
        interval: u64_low(word(1)),
        status: bytes[2 * 32 + 31], // Status enum in the low byte of word 2
        next_run: u64_low(word(3)),
        budget_wei: u128_low(word(4)),
        runs_left: u64_low(word(5)) as u32,
        target_id: u64_low(word(6)),
    })
}

/// Read `taskOf(uint256)` — the job's task prompt, decoded UTF-8. Stored as
/// on-chain `bytes` (same ABI shape as a `string` return: offset + length +
/// body); we interpret it as UTF-8 since the MVP task is an inline prompt.
pub async fn task_of(job_id: u64) -> Result<String, String> {
    let calldata = call_uint("taskOf(uint256)", job_id);
    let result = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    let raw = hex_to_bytes(&result)?;
    if raw.len() < 64 {
        return Err(format!("taskOf: short response {} bytes", raw.len()));
    }
    let len = u64::from_be_bytes(
        raw[56..64].try_into().map_err(|e: std::array::TryFromSliceError| e.to_string())?,
    ) as usize;
    // `len` is attacker-controlled — checked add so a huge length errors
    // instead of overflowing the slice.
    let end = len
        .checked_add(64)
        .filter(|&end| end <= raw.len())
        .ok_or_else(|| format!("taskOf: truncated body (len {}, have {})", len, raw.len()))?;
    String::from_utf8(raw[64..end].to_vec()).map_err(|e| e.to_string())
}

// --- InviteFacet (user-funded, refundable $LH invite codes) ----------
//
// A holder escrows their OWN $LH behind a bearer code; an accepter redeems it
// (paid out from escrow); the funder reclaims it after expiry if unclaimed.
// Mirrors `RedeemFacet`'s `keccak256(bytes(code))` hashing — only the hash is
// on-chain, the plaintext is the bearer secret distributed off-chain. See
// `design/invites.md`. EXACT ABI (matched to the parallel facet build):
//   createInvite(bytes32 codeHash, uint256 amount, uint64 ttlSeconds)
//   acceptInvite(string code)
//   reclaimInvite(bytes32 codeHash)
//   getInvite(bytes32) -> (address funder, uint128 amount, uint64 expiry, uint8 status)
//   escrowedOf(address) -> uint256

/// `keccak256(bytes(code))` — the on-chain invite key. IDENTICAL primitive to
/// `RedeemFacet.redeem`'s hash (`keccak_key(code.as_bytes())`), so a code
/// hashed here matches what `acceptInvite(string)` recomputes on-chain. NOT
/// trimmed: the facet hashes the exact string passed to `acceptInvite`, and
/// generated invite codes never carry whitespace — trimming here would diverge
/// from the chain for a code that legitimately contained leading/trailing space.
pub fn invite_code_hash(code: &str) -> [u8; 32] {
    keccak_key(code.as_bytes())
}

/// Encode `createInvite(bytes32 codeHash, uint256 amount, uint64 ttlSeconds)` —
/// three static head words (`amount`/`ttlSeconds` right-aligned in their words).
fn encode_create_invite(code_hash: &[u8; 32], amount_wei: u128, ttl_secs: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 3 * 32);
    out.extend_from_slice(&selector("createInvite(bytes32,uint256,uint64)"));
    out.extend_from_slice(code_hash);
    out.extend_from_slice(&u256_be(amount_wei));
    out.extend_from_slice(&u256_be(ttl_secs as u128));
    out
}

/// Encode `acceptInvite(string code)` — one dynamic-string arg (offset 0x20 +
/// length + right-padded bytes), the SAME ABI shape as `encode_redeem`.
fn encode_accept_invite(code: &str) -> Vec<u8> {
    let bytes = code.as_bytes();
    let len = bytes.len();
    let padded_len = len.div_ceil(32) * 32;
    let mut out = Vec::with_capacity(4 + 32 + 32 + padded_len);
    out.extend_from_slice(&selector("acceptInvite(string)"));
    out.extend_from_slice(&u256_be(0x20));
    out.extend_from_slice(&u256_be(len as u128));
    out.extend_from_slice(bytes);
    out.resize(4 + 32 + 32 + padded_len, 0);
    out
}

/// Encode `reclaimInvite(bytes32 codeHash)` — one static word.
fn encode_reclaim_invite(code_hash: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 32);
    out.extend_from_slice(&selector("reclaimInvite(bytes32)"));
    out.extend_from_slice(code_hash);
    out
}

/// Create a refundable invite via a sponsored Tempo tx. Batches
/// `approve(diamond, amount)` on `$LH` + `createInvite(codeHash, amount, ttl)`
/// in ONE tx — `createInvite` then escrows the `$LH` via `transferFrom(caller,
/// diamond, amount)` inside its own body (the identical approve→pull escrow
/// pattern as `schedule_job_sponsored` / `deposit_credits_sponsored`). The
/// `amount` leaves the funder's spendable balance the moment this mines; it is
/// paid to the accepter on `acceptInvite` or refunded to the funder on
/// `reclaimInvite` after expiry. Returns the tx hash once mined.
#[allow(clippy::too_many_arguments)]
pub async fn create_invite_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    code_hash: [u8; 32],
    amount_wei: u128,
    ttl_secs: u64,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let token_addr = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS)?;
    let approve_call = crate::tempo_tx::TempoCall {
        to: token_addr,
        value_wei: 0,
        input: encode_approve(&diamond_addr, amount_wei),
    };
    let create_call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_create_invite(&code_hash, amount_wei, ttl_secs),
    };
    // approve (~46k) + createInvite (transferFrom pull + the invite struct's TWO
    // cold SSTOREs + the `escrowedOf` SSTORE + event) + ~275k sponsorship. These
    // are cold writes (CLAUDE.md "cold SSTOREs dominate; never guess — cast
    // estimate"); budget generously at 2.5M. The sponsor is billed on gas USED,
    // not the limit, so over-budgeting is free.
    submit_tempo_sponsored(sender, fee_payer, vec![approve_call, create_call], fee_token, 2_500_000)
        .await
}

/// Accept an invite via a sponsored Tempo tx: `acceptInvite(code)` pays the
/// escrowed `$LH` out to the CALLER (`sender`). The plaintext `code` is hashed
/// on-chain (`keccak256(bytes(code))`) to find the invite; the facet flips its
/// status to `Accepted` before the payout (CEI), so a replay reverts. Returns
/// the tx hash once mined.
pub async fn accept_invite_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    code: &str,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_accept_invite(code),
    };
    // status flip (1 SSTORE) + the payout `transfer` + `escrowedOf` decrement +
    // event — cheaper than create. Mirror redeem's mint-path budget for
    // headroom (cold token-balance SSTOREs on the payout).
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, 2_000_000).await
}

/// Reclaim an expired, unclaimed invite via a sponsored Tempo tx:
/// `reclaimInvite(codeHash)` refunds the escrowed `$LH` to the FUNDER. The call
/// is permissionless (anyone can poke it; the `$LH` only ever goes to the
/// recorded funder), but the funder's own front-end / CLI normally triggers it.
/// Returns the tx hash once mined.
pub async fn reclaim_invite_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    code_hash: [u8; 32],
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_reclaim_invite(&code_hash),
    };
    // status flip + the refund `transfer` + `escrowedOf` decrement + event.
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, 600_000).await
}

/// Read `escrowedOf(address)` — total `$LH` (18-decimal wei) the funder
/// currently has locked across all their `Open` invites (the running sum the
/// facet maintains on create/accept/reclaim). The "pending invites" total.
pub async fn escrowed_of(account_hex: &str) -> Result<u128, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(0);
    }
    let account = parse_eth_address(account_hex)?;
    let mut calldata = selector("escrowedOf(address)").to_vec();
    calldata.extend_from_slice(&addr_word(&account));
    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(REGISTRY_ADDRESS, &calldata_hex).await?;
    decode_u256_as_u128(&result)
}

/// Read `getInvite(bytes32) -> (address funder, uint128 amount, uint64 expiry,
/// uint8 status)`. Status: 0=Open, 1=Accepted, 2=Reclaimed. An empty/unknown
/// invite returns the zero record (funder `0x0…0`, amount 0). All four fields
/// pack into 4 consecutive ABI words (each value right-aligned in its word).
pub async fn get_invite(code_hash: [u8; 32]) -> Result<(String, u128, u64, u8), String> {
    let mut calldata = selector("getInvite(bytes32)").to_vec();
    calldata.extend_from_slice(&code_hash);
    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(REGISTRY_ADDRESS, &calldata_hex).await?;
    let bytes = hex_to_bytes(&result)?;
    if bytes.len() < 4 * 32 {
        return Err(format!("getInvite: short response {} bytes", bytes.len()));
    }
    let word = |i: usize| &bytes[i * 32..(i + 1) * 32];
    let funder = format!("0x{}", bytes_to_hex(&word(0)[12..32])); // address, low 20 bytes
    let mut amt = [0u8; 16];
    amt.copy_from_slice(&word(1)[16..32]); // uint128, low 16 bytes
    let amount = u128::from_be_bytes(amt);
    let mut exp = [0u8; 8];
    exp.copy_from_slice(&word(2)[24..32]); // uint64, low 8 bytes
    let expiry = u64::from_be_bytes(exp);
    let status = word(3)[31]; // uint8 enum in the low byte
    Ok((funder, amount, expiry, status))
}

// --- BountyFacet (escrowed agent-economy task board) -----------------
//
// The DEMAND primitive: a poster ESCROWS `$LH` behind a task (`transferFrom`
// poster→diamond inside `postBounty`, so the bundle batches `approve(diamond,
// reward)` + `postBounty` in ONE sponsored tx — the identical escrow shape as
// `schedule_job_sponsored` / `create_invite_sponsored`). A claimant (identified
// by THEIR OWN tokenId) claims it, submits a result, and is paid the escrow on
// the poster's `acceptResult`. EXACT ABI (matched to the parallel facet build):
//   postBounty(bytes task, uint128 rewardWei, uint64 ttlSeconds) -> uint256 bountyId
//   claimBounty(uint256 bountyId, uint256 claimantTokenId)
//   submitResult(uint256 bountyId, bytes result)
//   acceptResult(uint256 bountyId) / cancelBounty(uint256) / reclaimExpired(uint256)
//   getBounty(uint256) -> (address poster, uint128 rewardWei, uint64 expiry,
//                          uint8 status, uint256 claimantTokenId)
//   taskOf(uint256)->bytes / resultOf(uint256)->bytes
//   openBounties(uint256 startAfter, uint256 limit) -> (uint256[], uint256)
//   bountiesOf(address) -> uint256[]
// status: 0 Open / 1 Claimed / 2 Submitted / 3 Paid / 4 Cancelled / 5 Reclaimed.

/// One bounty record, decoded from `getBounty(uint256)`. Field order/types
/// mirror the facet's returned tuple exactly: poster, rewardWei, expiry,
/// status, claimantTokenId. `status` is the raw enum byte (see the table above).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bounty {
    /// Who posted it (the escrow funder / accept authority), 0x-hex address.
    pub poster: String,
    /// `$LH` (wei) escrowed as the reward — paid to the claimant on accept.
    pub reward_wei: u128,
    /// Unix seconds the bounty expires (the `reclaimExpired` gate; 0 if unset).
    pub expiry: u64,
    /// Raw lifecycle byte: 0 Open, 1 Claimed, 2 Submitted, 3 Paid, 4 Cancelled,
    /// 5 Reclaimed.
    pub status: u8,
    /// tokenId of the agent that claimed it (0 while Open).
    pub claimant_token_id: u64,
}

impl Bounty {
    /// Human-readable lifecycle label for the raw `status` byte.
    pub fn status_label(&self) -> &'static str {
        match self.status {
            0 => "open",
            1 => "claimed",
            2 => "submitted",
            3 => "paid",
            4 => "cancelled",
            5 => "reclaimed",
            _ => "unknown",
        }
    }
}

/// Encode `postBounty(bytes task, uint128 rewardWei, uint64 ttlSeconds)`. `task`
/// is the FIRST (dynamic `bytes`) arg, so head word 0 holds the OFFSET to the
/// tail (3 fixed head words = `3 * 32`); words 1/2 are `rewardWei`/`ttlSeconds`
/// right-aligned; the tail is `[length][padded data]` (same dynamic-bytes layout
/// `encode_schedule_job`'s `task` uses, but the bytes arg is FIRST here).
fn encode_post_bounty(task: &[u8], reward_wei: u128, ttl_secs: u64) -> Vec<u8> {
    let padded_len = task.len().div_ceil(32) * 32;
    let mut out = Vec::with_capacity(4 + 3 * 32 + 32 + padded_len);
    out.extend_from_slice(&selector("postBounty(bytes,uint128,uint64)"));
    // Head word 0: offset to the `bytes task` tail — 3 fixed head words.
    out.extend_from_slice(&u256_be(3 * 32));
    // Head words 1..3: rewardWei / ttlSeconds (each right-aligned).
    out.extend_from_slice(&u256_be(reward_wei));
    out.extend_from_slice(&u256_be(ttl_secs as u128));
    // Tail: length + the task bytes, right-padded to a 32-byte multiple.
    out.extend_from_slice(&u256_be(task.len() as u128));
    out.extend_from_slice(task);
    out.resize(out.len() + (padded_len - task.len()), 0);
    out
}

/// Encode `claimBounty(uint256 bountyId, uint256 claimantTokenId)` — two static
/// head words (bountyId, then the CLAIMANT'S OWN tokenId).
fn encode_claim_bounty(bounty_id: u64, claimant_token_id: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 64);
    out.extend_from_slice(&selector("claimBounty(uint256,uint256)"));
    out.extend_from_slice(&u256_be(bounty_id as u128));
    out.extend_from_slice(&u256_be(claimant_token_id as u128));
    out
}

/// Encode `submitResult(uint256 bountyId, bytes result)` — a static `bountyId`
/// head word then a dynamic `bytes result` (offset = 2 head words = `2 * 32`,
/// then `[length][padded data]`).
fn encode_submit_result(bounty_id: u64, result: &[u8]) -> Vec<u8> {
    let padded_len = result.len().div_ceil(32) * 32;
    let mut out = Vec::with_capacity(4 + 2 * 32 + 32 + padded_len);
    out.extend_from_slice(&selector("submitResult(uint256,bytes)"));
    out.extend_from_slice(&u256_be(bounty_id as u128));
    out.extend_from_slice(&u256_be(2 * 32)); // offset to the bytes tail
    out.extend_from_slice(&u256_be(result.len() as u128));
    out.extend_from_slice(result);
    out.resize(out.len() + (padded_len - result.len()), 0);
    out
}

/// Post a bounty via a sponsored Tempo tx. Batches `approve(diamond, rewardWei)`
/// on `$LH` + `postBounty(task, rewardWei, ttlSeconds)` in ONE tx — `postBounty`
/// then escrows the reward via `transferFrom(poster, diamond, rewardWei)` inside
/// its own body (the identical approve→pull escrow pattern as
/// `schedule_job_sponsored`). The `rewardWei` leaves the poster's spendable
/// balance the moment this mines; it pays the claimant on `acceptResult` or is
/// refunded on `cancelBounty` / `reclaimExpired`. Returns the tx hash once mined;
/// read the new bounty id back from `bounties_of(poster)` (its last entry).
#[allow(clippy::too_many_arguments)]
pub async fn post_bounty_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    task: &[u8],
    reward_wei: u128,
    ttl_secs: u64,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let token_addr = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS)?;
    let approve_call = crate::tempo_tx::TempoCall {
        to: token_addr,
        value_wei: 0,
        input: encode_approve(&diamond_addr, reward_wei),
    };
    let post_call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_post_bounty(task, reward_wei, ttl_secs),
    };
    // approve (~46k) + postBounty (transferFrom pull + the bounty struct's cold
    // SSTOREs + the cold `task` bytes ~7.6k/BYTE + the bountiesOf enumerable push
    // + event) + ~275k sponsorship overhead. Cold writes dominate (CLAUDE.md
    // "cast estimate, never guess"); budget the same 3.5M base + 9k/byte the
    // scheduleJob escrow uses (also a struct + bytes + index push). The sponsor
    // is billed on gas USED, so over-budgeting is free.
    let gas = 3_500_000 + (task.len() as u128) * 9_000;
    submit_tempo_sponsored(sender, fee_payer, vec![approve_call, post_call], fee_token, gas).await
}

/// Claim an Open bounty via a sponsored Tempo tx. `claimant_token_id` is the
/// CLAIMANT'S OWN registered tokenId (the on-chain identity that earns the
/// reward) — resolve it from the caller's identity (see the CLI's claimant
/// resolution), NOT the bounty's poster. `claimBounty` flips the status to
/// Claimed and records the claimant.
pub async fn claim_bounty_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    bounty_id: u64,
    claimant_token_id: u64,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_claim_bounty(bounty_id, claimant_token_id),
    };
    // status flip + claimant SSTORE + event. 400k mirrors the cancelJob budget.
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, 400_000).await
}

/// Submit a result for a Claimed bounty via a sponsored Tempo tx. Stores the
/// `result` bytes on-chain (cold `bytes` write, ~7.6k/BYTE) and flips the status
/// to Submitted, awaiting the poster's `acceptResult`.
pub async fn submit_result_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    bounty_id: u64,
    result: &[u8],
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_submit_result(bounty_id, result),
    };
    // status flip + the cold `result` bytes SSTOREs (~7.6k/byte) + event. Scale
    // the same 1.2M base + 9k/byte the on-chain `bytes` writes use elsewhere.
    let gas = 1_200_000 + (result.len() as u128) * 9_000;
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, gas).await
}

/// Accept a Submitted bounty's result via a sponsored Tempo tx: the poster (only)
/// calls `acceptResult(bountyId)`, which pays the escrowed `$LH` out to the
/// claimant's TBA and flips the status to Paid (CEI). Returns the tx hash.
pub async fn accept_result_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    bounty_id: u64,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: call_uint_bytes("acceptResult(uint256)", bounty_id),
    };
    // status flip (1 SSTORE) + the payout `transfer` (cold token balances) +
    // event. Mirror the redeem/accept-invite payout budget for headroom.
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, 2_000_000).await
}

/// Cancel a bounty via a sponsored Tempo tx: the poster (only) calls
/// `cancelBounty(bountyId)`, which REFUNDS the full escrowed `$LH` to the poster
/// and flips the status to Cancelled (allowed before payout).
pub async fn cancel_bounty_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    bounty_id: u64,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: call_uint_bytes("cancelBounty(uint256)", bounty_id),
    };
    // status flip + the refund `transfer` + event.
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, 600_000).await
}

/// Reclaim an expired, unaccepted bounty via a sponsored Tempo tx:
/// `reclaimExpired(bountyId)` refunds the escrowed `$LH` to the poster once the
/// TTL has elapsed without an accepted result, flipping the status to Reclaimed.
pub async fn reclaim_expired_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    bounty_id: u64,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: call_uint_bytes("reclaimExpired(uint256)", bounty_id),
    };
    // status flip + the refund `transfer` + event.
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, 600_000).await
}

/// Read `openBounties(uint256 startAfter, uint256 limit)` → `(uint256[] ids,
/// uint256 nextCursor)`. The paginated open-board scan: pass `startAfter = 0` to
/// begin, then the returned cursor to page on (0 = no more). Returns only the
/// id list here (the cursor is the facet's internal pagination detail); call
/// repeatedly bumping `start_after` to the last id when walking the whole board.
/// The ABI return is a dynamic `uint256[]` (head = offset to it + the cursor
/// word) followed by the cursor; we decode the array (low 8 bytes of each id,
/// monotonic u64-scale counters).
pub async fn open_bounties(start_after: u64, limit: u64) -> Result<Vec<u64>, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(Vec::new());
    }
    let mut calldata = selector("openBounties(uint256,uint256)").to_vec();
    calldata.extend_from_slice(&u256_be(start_after as u128));
    calldata.extend_from_slice(&u256_be(limit as u128));
    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(REGISTRY_ADDRESS, &calldata_hex).await?;
    let bytes = hex_to_bytes(&result)?;
    decode_uint_array_with_cursor(&bytes)
}

/// Decode a `(uint256[] ids, uint256 cursor)` ABI return into the id `Vec`
/// (dropping the trailing cursor word). Head layout: word 0 = OFFSET to the
/// array, word 1 = the cursor. At `offset` sits `[length][id0][id1]…`. Pure +
/// testable; hostile-length-safe (no pre-alloc; checked index math stops the
/// decode on a bogus length instead of OOMing).
fn decode_uint_array_with_cursor(bytes: &[u8]) -> Result<Vec<u64>, String> {
    // Need at least the two head words (array offset + cursor).
    if bytes.len() < 64 {
        return Ok(Vec::new());
    }
    // Word 0: offset to the dynamic array (low 8 bytes — never near 2^64).
    let mut off_buf = [0u8; 8];
    off_buf.copy_from_slice(&bytes[24..32]);
    let arr_off = u64::from_be_bytes(off_buf) as usize;
    // The length word sits at the array offset.
    let len_start = match arr_off.checked_add(32) {
        Some(s) if s <= bytes.len() => arr_off,
        _ => return Ok(Vec::new()),
    };
    let mut len_buf = [0u8; 8];
    len_buf.copy_from_slice(&bytes[len_start + 24..len_start + 32]);
    let len = u64::from_be_bytes(len_buf) as usize;
    let body = len_start + 32; // first id word
    let mut out = Vec::new();
    for i in 0..len {
        let start = match i.checked_mul(32).and_then(|o| o.checked_add(body)) {
            Some(s) => s,
            None => break,
        };
        let Some(word) = start.checked_add(32).and_then(|end| bytes.get(start + 24..end)) else {
            break;
        };
        let mut id_buf = [0u8; 8];
        id_buf.copy_from_slice(word);
        out.push(u64::from_be_bytes(id_buf));
    }
    Ok(out)
}

/// Read `bountiesOf(address)` — every bounty id the address has POSTED (Open +
/// terminal). The enumerable index backing the "my bounties" view. Same ABI
/// shape as `jobsOf` (a bare dynamic `uint256[]`).
pub async fn bounties_of(account_hex: &str) -> Result<Vec<u64>, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(Vec::new());
    }
    let account = parse_eth_address(account_hex)?;
    let mut calldata = selector("bountiesOf(address)").to_vec();
    calldata.extend_from_slice(&addr_word(&account));
    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(REGISTRY_ADDRESS, &calldata_hex).await?;
    let bytes = hex_to_bytes(&result)?;
    // Bare dynamic uint256[]: [offset(32)][len(32)][id0(32)]… — same decode as
    // `jobs_of`.
    if bytes.len() < 64 {
        return Ok(Vec::new());
    }
    let mut len_buf = [0u8; 8];
    len_buf.copy_from_slice(&bytes[56..64]);
    let len = u64::from_be_bytes(len_buf) as usize;
    let mut out = Vec::new();
    for i in 0..len {
        let start = match i.checked_mul(32).and_then(|o| o.checked_add(64)) {
            Some(s) => s,
            None => break,
        };
        let Some(word) = start.checked_add(32).and_then(|end| bytes.get(start + 24..end)) else {
            break;
        };
        let mut id_buf = [0u8; 8];
        id_buf.copy_from_slice(word);
        out.push(u64::from_be_bytes(id_buf));
    }
    Ok(out)
}

/// Read `getBounty(uint256)` → the full [`Bounty`] record. The returned tuple is
/// all-static (the `task`/`result` live in their own mappings, read via
/// [`task_of_bounty`] / [`result_of_bounty`]), so it decodes as 5 consecutive
/// ABI words: poster, rewardWei, expiry, status, claimantTokenId. Returns the
/// poster as a 0x-hex address and each numeric in its native width.
pub async fn get_bounty(bounty_id: u64) -> Result<Bounty, String> {
    let calldata = call_uint("getBounty(uint256)", bounty_id);
    let result = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    let bytes = hex_to_bytes(&result)?;
    if bytes.len() < 5 * 32 {
        return Err(format!("getBounty: short response {} bytes", bytes.len()));
    }
    let word = |i: usize| &bytes[i * 32..(i + 1) * 32];
    let poster = format!("0x{}", bytes_to_hex(&word(0)[12..32])); // address, low 20 bytes
    let u64_low = |w: &[u8]| {
        let mut b = [0u8; 8];
        b.copy_from_slice(&w[24..32]);
        u64::from_be_bytes(b)
    };
    let mut amt = [0u8; 16];
    amt.copy_from_slice(&word(1)[16..32]); // uint128, low 16 bytes
    Ok(Bounty {
        poster,
        reward_wei: u128::from_be_bytes(amt),
        expiry: u64_low(word(2)),
        status: bytes[3 * 32 + 31], // uint8 enum in the low byte of word 3
        claimant_token_id: u64_low(word(4)),
    })
}

/// Read `taskOf(uint256)` — the bounty's task prompt, decoded UTF-8. Stored as
/// on-chain `bytes` (offset + length + body, same shape as a `string` return);
/// we interpret it as UTF-8 since the MVP task is an inline prompt.
pub async fn task_of_bounty(bounty_id: u64) -> Result<String, String> {
    decode_bytes_string_call("bountyTaskOf(uint256)", bounty_id, "bountyTaskOf").await
}

/// Read `resultOf(uint256)` — the submitted result bytes, decoded UTF-8. Empty
/// until the claimant `submitResult`s. Same `bytes` ABI shape as [`task_of_bounty`].
pub async fn result_of_bounty(bounty_id: u64) -> Result<String, String> {
    decode_bytes_string_call("resultOf(uint256)", bounty_id, "resultOf").await
}

/// Shared `fn(uint256) -> bytes` reader: eth_call the selector with `id`, decode
/// the returned dynamic `bytes` (offset + length + body) as UTF-8. The decode is
/// length-checked (attacker-controlled length can't overflow the slice).
async fn decode_bytes_string_call(sig: &str, id: u64, what: &str) -> Result<String, String> {
    let calldata = call_uint(sig, id);
    let result = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    let raw = hex_to_bytes(&result)?;
    if raw.len() < 64 {
        return Err(format!("{what}: short response {} bytes", raw.len()));
    }
    let len = u64::from_be_bytes(
        raw[56..64].try_into().map_err(|e: std::array::TryFromSliceError| e.to_string())?,
    ) as usize;
    let end = len
        .checked_add(64)
        .filter(|&end| end <= raw.len())
        .ok_or_else(|| format!("{what}: truncated body (len {}, have {})", len, raw.len()))?;
    String::from_utf8(raw[64..end].to_vec()).map_err(|e| e.to_string())
}

/// Build calldata for a `fn(uint256)` selector with a single id argument,
/// returning the RAW bytes (the `Vec<u8>` twin of [`call_uint`], which returns a
/// 0x-hex string). Used for the bounty single-arg WRITE selectors that go into a
/// `TempoCall.input`.
fn call_uint_bytes(sig: &str, id: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(4 + 32);
    data.extend_from_slice(&selector(sig));
    data.extend_from_slice(&u256_be(id as u128));
    data
}

/// Discover OPEN bounties by keyword — the demand-side twin of [`discover_agents`].
/// Scans the open board (`open_bounties`, paging up to `scan` ids), reads each
/// one's task text + reward, and returns `(id, task, reward_wei)` matches for
/// `query`, ranked by query-vs-task relevance (the SAME `rank_agent_matches`
/// substring ranking, applied to the task text). An empty query returns all open
/// bounties (newest-first, as the board returns them). Read-only.
pub async fn discover_bounties(query: &str, scan: u64) -> Result<Vec<(u64, String, u128)>, String> {
    let ids = open_bounties(0, scan).await?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    // Fetch each open bounty's task text + reward. (The board is small at launch
    // scale; one read pair per id, like `discover_agents`' persona fetch.)
    let mut entries: Vec<(u64, String, u128)> = Vec::with_capacity(ids.len());
    // Reuse the agent-rank pipeline: build (key, task) pairs where the "name"
    // slot is the id (so ranking matches on the task text in the persona slot).
    let mut pairs: Vec<(String, String)> = Vec::with_capacity(ids.len());
    for id in ids {
        let task = task_of_bounty(id).await.unwrap_or_default();
        let reward = get_bounty(id).await.map(|b| b.reward_wei).unwrap_or(0);
        pairs.push((id.to_string(), task.clone()));
        entries.push((id, task, reward));
    }
    let ranked = rank_agent_matches(&pairs, query);
    // Map the ranked (id-string, task) pairs back to the (id, task, reward)
    // entries, preserving the rank order.
    let mut out: Vec<(u64, String, u128)> = Vec::with_capacity(ranked.len());
    for (id_str, _task) in ranked {
        if let Some(entry) = entries.iter().find(|(id, _, _)| id.to_string() == id_str) {
            out.push(entry.clone());
        }
    }
    Ok(out)
}

// --- GuildFacet (on-chain orgs: members, roles, pooled $LH treasury) ------
//
// Rung 3 of `design/agent-coordination.md` (bounty → party → GUILD → DAO). A
// guild is an on-chain org with a member roster, per-member roles, and a pooled
// `$LH` treasury an admin/officer can spend. The sibling builds the facet
// against the SAME ABI declared here; reuses InviteFacet's approve→pull escrow
// (`fundGuild`) + X402/transfer payout (`spendTreasury`) + TbaFacet's wallet
// (`guildAddress`). EXACT ABI:
//   createGuild(string name) -> uint256 guildId
//   inviteToGuild(uint256 guildId, address member)
//   acceptGuildInvite(uint256 guildId)
//   leaveGuild(uint256 guildId)
//   setRole(uint256 guildId, address member, uint8 role)  (0 None/1 Member/2 Officer/3 Admin)
//   fundGuild(uint256 guildId, uint256 amount)  (transferFrom caller->diamond; APPROVE first)
//   spendTreasury(uint256 guildId, address to, uint256 amount, bytes memo)
//   reads: membersOf(uint256)->address[] | roleOf(uint256,address)->uint8
//          isGuildMember(uint256,address)->bool | treasuryBalanceOf(uint256)->uint256
//          guildAddress(uint256)->address | guildName(uint256)->string
//          guildsOf(address)->uint256[] | guildCount()->uint256
//
// NOTE ON SELECTOR COLLISIONS: the diamond can't share a selector across facets.
// If the sibling renamed a colliding selector with a `guild` prefix on the live
// diamond (e.g. `setRole`→`setGuildRole`, `membersOf`→`guildMembersOf`,
// `isGuildMember`→`isGuildMemberOf`), a READ here will revert/return empty —
// flag a rename as the likely cause when a guild read fails unexpectedly. The
// selectors below match the spec'd ABI verbatim; bump them together with the
// facet if a rename lands.

/// A member's role within a guild. The on-chain `uint8` enum
/// (`0 None / 1 Member / 2 Officer / 3 Admin`) decoded into a typed value, so
/// the CLI/browser never juggle raw bytes. `None` = not a member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuildRole {
    /// Not a member (the zero role).
    None,
    /// A plain member.
    Member,
    /// An officer (can spend the treasury; below admin).
    Officer,
    /// An admin (full control: roles + treasury).
    Admin,
}

impl GuildRole {
    /// Decode the on-chain `uint8` role byte. Unknown values clamp to `None`.
    pub fn from_u8(v: u8) -> GuildRole {
        match v {
            1 => GuildRole::Member,
            2 => GuildRole::Officer,
            3 => GuildRole::Admin,
            _ => GuildRole::None,
        }
    }

    /// The on-chain `uint8` value for this role (the inverse of [`from_u8`]).
    pub fn as_u8(self) -> u8 {
        match self {
            GuildRole::None => 0,
            GuildRole::Member => 1,
            GuildRole::Officer => 2,
            GuildRole::Admin => 3,
        }
    }

    /// Lowercase human label (`none`/`member`/`officer`/`admin`).
    pub fn label(self) -> &'static str {
        match self {
            GuildRole::None => "none",
            GuildRole::Member => "member",
            GuildRole::Officer => "officer",
            GuildRole::Admin => "admin",
        }
    }

    /// Parse a user-typed role word (`member`/`officer`/`admin`,
    /// case-insensitive). `none` is rejected — it's a removal, not a settable
    /// rank (use `leaveGuild`/role-0 paths deliberately, not a typo). Pure.
    pub fn parse(raw: &str) -> Result<GuildRole, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "member" => Ok(GuildRole::Member),
            "officer" => Ok(GuildRole::Officer),
            "admin" => Ok(GuildRole::Admin),
            other => Err(format!(
                "invalid role '{other}' — expected member, officer, or admin"
            )),
        }
    }
}

/// Encode `createGuild(string name)` — one dynamic `string` arg (offset 0x20 +
/// length + padded UTF-8 bytes), the SAME layout as `register(string)`. Returns
/// raw calldata for a `TempoCall.input`.
fn encode_create_guild(name: &str) -> Vec<u8> {
    let bytes = name.as_bytes();
    let padded_len = bytes.len().div_ceil(32) * 32;
    let mut out = Vec::with_capacity(4 + 32 + 32 + padded_len);
    out.extend_from_slice(&selector("createGuild(string)"));
    out.extend_from_slice(&u256_be(0x20)); // offset to the string head
    out.extend_from_slice(&u256_be(bytes.len() as u128)); // length
    out.extend_from_slice(bytes);
    out.resize(out.len() + (padded_len - bytes.len()), 0); // right-pad
    out
}

/// Encode `inviteToGuild(uint256 guildId, address member)` — two static head
/// words (guildId, then the member address right-aligned in word 1).
fn encode_invite_to_guild(guild_id: u64, member: &[u8; 20]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 64);
    out.extend_from_slice(&selector("inviteToGuild(uint256,address)"));
    out.extend_from_slice(&u256_be(guild_id as u128));
    out.extend_from_slice(&addr_word(member));
    out
}

/// Encode `setRole(uint256 guildId, address member, uint8 role)` — three static
/// head words (guildId, member address, role byte right-aligned).
fn encode_set_role(guild_id: u64, member: &[u8; 20], role: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 96);
    out.extend_from_slice(&selector("setRole(uint256,address,uint8)"));
    out.extend_from_slice(&u256_be(guild_id as u128));
    out.extend_from_slice(&addr_word(member));
    out.extend_from_slice(&u256_be(role as u128));
    out
}

/// Encode `fundGuild(uint256 guildId, uint256 amount)` — two static head words.
/// Batched AFTER an `approve(diamond, amount)` (the facet `transferFrom`s the
/// caller→diamond inside its body, the InviteFacet escrow shape).
fn encode_fund_guild(guild_id: u64, amount_wei: u128) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 64);
    out.extend_from_slice(&selector("fundGuild(uint256,uint256)"));
    out.extend_from_slice(&u256_be(guild_id as u128));
    out.extend_from_slice(&u256_be(amount_wei));
    out
}

/// Encode `spendTreasury(uint256 guildId, address to, uint256 amount, bytes
/// memo)`. Head: guildId(0) / to(1) / amount(2) / OFFSET-to-memo(3) = 4 fixed
/// head words = `4 * 32` = 0x80; tail = `[length][padded memo bytes]`. The ONLY
/// dynamic arg is `memo`, and it is LAST, so the offset is the full head size.
fn encode_spend_treasury(guild_id: u64, to: &[u8; 20], amount_wei: u128, memo: &[u8]) -> Vec<u8> {
    let padded_len = memo.len().div_ceil(32) * 32;
    let mut out = Vec::with_capacity(4 + 4 * 32 + 32 + padded_len);
    out.extend_from_slice(&selector("spendTreasury(uint256,address,uint256,bytes)"));
    out.extend_from_slice(&u256_be(guild_id as u128)); // head 0: guildId
    out.extend_from_slice(&addr_word(to)); // head 1: to
    out.extend_from_slice(&u256_be(amount_wei)); // head 2: amount
    out.extend_from_slice(&u256_be(4 * 32)); // head 3: offset to memo tail
    out.extend_from_slice(&u256_be(memo.len() as u128)); // tail: length
    out.extend_from_slice(memo); // tail: memo bytes
    out.resize(out.len() + (padded_len - memo.len()), 0); // right-pad
    out
}

/// Create a guild via a sponsored Tempo tx: `createGuild(name)` mints the org
/// (caller becomes its Admin), returning the tx hash once mined. Read the new
/// guildId back from `guilds_of(creator)` (its last entry, like `bounties_of`).
pub async fn create_guild_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    name: &str,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_create_guild(name),
    };
    // The guild struct's cold SSTOREs (id↔owner, name `bytes`, the creator's
    // Admin role + the `guildsOf` enumerable push) + ~275k sponsorship. Cold
    // writes dominate (CLAUDE.md "cast estimate, never guess"); budget the same
    // base + per-byte the on-chain name write costs. Sponsor billed on gas USED.
    // Measured live: `cast estimate createGuild` ≈ 2.87M (the full name mint —
    // ERC721 + name↔id + ownerOfId + MAIN — plus the guild struct). A 1.5M base
    // OOG'd the live tx (static call succeeded → pure gas). Budget 3.5M base like
    // scheduleJob (comfortably above 2.87M + sponsorship overhead). Sponsor billed
    // on gas USED, so the headroom is free.
    let gas = 3_500_000 + (name.len() as u128) * 9_000;
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, gas).await
}

/// Invite an address to a guild via a sponsored Tempo tx
/// (`inviteToGuild(guildId, member)`). The invitee then `acceptGuildInvite`s to
/// join. Admin/officer-gated on-chain.
pub async fn invite_to_guild_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    guild_id: u64,
    member_hex: &str,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let member = parse_eth_address(member_hex)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_invite_to_guild(guild_id, &member),
    };
    // A pending-invite SSTORE + event. 400k mirrors the bounty-claim budget.
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, 400_000).await
}

/// Accept a pending guild invite via a sponsored Tempo tx
/// (`acceptGuildInvite(guildId)`): the caller joins as a Member, added to the
/// roster + `guildsOf` index.
pub async fn accept_guild_invite_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    guild_id: u64,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: call_uint_bytes("acceptGuildInvite(uint256)", guild_id),
    };
    // Role SSTORE + the roster + `guildsOf` enumerable pushes + event — cold
    // index writes dominate. Measured: `cast estimate acceptGuildInvite` ≈ 1.33M
    // (a 1.0M limit OOG'd live, gasUsed pinned at the cap). Budget 2M (sponsor
    // billed on gas USED, so the headroom is free).
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, 2_000_000).await
}

/// Leave a guild via a sponsored Tempo tx (`leaveGuild(guildId)`): the caller's
/// role is cleared and they're removed from the roster + `guildsOf` index.
pub async fn leave_guild_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    guild_id: u64,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: call_uint_bytes("leaveGuild(uint256)", guild_id),
    };
    // Role clear + the roster/index removals (swap-and-pop array writes, symmetric
    // to accept's pushes) + event. Budget 1.5M like accept (the 600k guess was the
    // same under-estimate class as the createGuild/accept OOGs).
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, 1_500_000).await
}

/// Set a member's role via a sponsored Tempo tx (`setRole(guildId, member,
/// role)`). `role` is the raw `uint8` (0 None / 1 Member / 2 Officer / 3 Admin)
/// — pass [`GuildRole::as_u8`]. Admin-gated on-chain.
pub async fn set_role_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    guild_id: u64,
    member_hex: &str,
    role: u8,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let member = parse_eth_address(member_hex)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_set_role(guild_id, &member, role),
    };
    // A single role SSTORE + event.
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, 400_000).await
}

/// Fund a guild's treasury via a sponsored Tempo tx. Batches `approve(diamond,
/// amount)` on `$LH` + `fundGuild(guildId, amount)` in ONE tx — `fundGuild`
/// then escrows via `transferFrom(caller, diamond, amount)` inside its body (the
/// identical approve→pull pattern as `post_bounty_sponsored` /
/// `create_invite_sponsored`). The `$LH` leaves the caller's spendable balance
/// the moment this mines and lands in the guild's pooled treasury.
pub async fn fund_guild_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    guild_id: u64,
    amount_wei: u128,
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let token_addr = parse_eth_address(LOCALHARNESS_TOKEN_ADDRESS)?;
    let approve_call = crate::tempo_tx::TempoCall {
        to: token_addr,
        value_wei: 0,
        input: encode_approve(&diamond_addr, amount_wei),
    };
    let fund_call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_fund_guild(guild_id, amount_wei),
    };
    // approve (~46k) + fundGuild (transferFrom pull + the treasury-balance
    // SSTORE + event) + ~275k sponsorship. Mirror the invite escrow budget.
    submit_tempo_sponsored(sender, fee_payer, vec![approve_call, fund_call], fee_token, 2_000_000)
        .await
}

/// Spend from a guild's treasury via a sponsored Tempo tx
/// (`spendTreasury(guildId, to, amount, memo)`): pays `amount` `$LH` from the
/// pooled treasury to `to`, with an optional `memo` recorded on-chain.
/// Admin/officer-gated on-chain. The `memo` `bytes` write scales the gas.
pub async fn spend_treasury_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    guild_id: u64,
    to_hex: &str,
    amount_wei: u128,
    memo: &[u8],
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let to = parse_eth_address(to_hex)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_spend_treasury(guild_id, &to, amount_wei, memo),
    };
    // treasury-balance debit + the payout `transfer` (cold token balances) + the
    // cold `memo` bytes (~9k/byte) + event. Base mirrors the redeem/payout budget.
    let gas = 2_000_000 + (memo.len() as u128) * 9_000;
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, gas).await
}

/// Read `membersOf(guildId)` → the guild's member roster as lowercase `0x…`
/// addresses. Bare dynamic `address[]` ABI return (`[offset][len][addr0]…`), the
/// SAME decode as `devices_of`. Hostile-length-safe (no pre-alloc; checked
/// index math stops the decode on a bogus length).
pub async fn members_of_guild(guild_id: u64) -> Result<Vec<String>, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(Vec::new());
    }
    let mut calldata = selector("guildMembersOf(uint256)").to_vec();
    calldata.extend_from_slice(&u256_be(guild_id as u128));
    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(REGISTRY_ADDRESS, &calldata_hex).await?;
    let bytes = hex_to_bytes(&result)?;
    if bytes.len() < 64 {
        return Ok(Vec::new());
    }
    let mut len_buf = [0u8; 8];
    len_buf.copy_from_slice(&bytes[56..64]); // low 8 bytes of the length word
    let len = u64::from_be_bytes(len_buf) as usize;
    let mut out = Vec::new();
    for i in 0..len {
        let start = match i.checked_mul(32).and_then(|o| o.checked_add(64)) {
            Some(s) => s,
            None => break,
        };
        let Some(word) = start.checked_add(32).and_then(|end| bytes.get(start + 12..end)) else {
            break;
        };
        out.push(format!("0x{}", bytes_to_hex(word)));
    }
    Ok(out)
}

/// Read `roleOf(guildId, member)` → the member's [`GuildRole`] (decoded from the
/// `uint8` enum; `None` for a non-member).
pub async fn role_of_guild(guild_id: u64, addr_hex: &str) -> Result<GuildRole, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(GuildRole::None);
    }
    let addr = parse_eth_address(addr_hex)?;
    let mut calldata = selector("roleOf(uint256,address)").to_vec();
    calldata.extend_from_slice(&u256_be(guild_id as u128));
    calldata.extend_from_slice(&addr_word(&addr));
    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(REGISTRY_ADDRESS, &calldata_hex).await?;
    // uint8 enum right-aligned in a 32-byte word — read the low byte via u64.
    let v = decode_u256_as_u64(&result)?;
    Ok(GuildRole::from_u8(v as u8))
}

/// Read `isGuildMember(guildId, member)` → whether the address is on the roster.
/// The single-read membership check (no roster walk).
pub async fn is_guild_member(guild_id: u64, addr_hex: &str) -> Result<bool, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(false);
    }
    let addr = parse_eth_address(addr_hex)?;
    let mut calldata = selector("isGuildMember(uint256,address)").to_vec();
    calldata.extend_from_slice(&u256_be(guild_id as u128));
    calldata.extend_from_slice(&addr_word(&addr));
    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(REGISTRY_ADDRESS, &calldata_hex).await?;
    decode_u256_as_u64(&result).map(|v| v != 0)
}

/// Read `treasuryBalanceOf(guildId)` → the guild's pooled `$LH` (18-decimal wei).
pub async fn treasury_balance_of(guild_id: u64) -> Result<u128, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(0);
    }
    let calldata = call_uint("treasuryBalanceOf(uint256)", guild_id);
    let result = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    decode_u256_as_u128(&result)
}

/// Read `guildAddress(guildId)` → the guild's on-chain wallet (its TBA), as a
/// lowercase `0x…` address. Returns the zero address as a literal string when
/// unset rather than `None` — guild treasury reads want the raw address either
/// way (the CLI prints it verbatim).
pub async fn guild_address(guild_id: u64) -> Result<String, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(zero_address().to_string());
    }
    let calldata = call_uint("guildAddress(uint256)", guild_id);
    let result = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    Ok(decode_address(&result).unwrap_or_else(|| zero_address().to_string()))
}

/// Read `guildName(guildId)` → the guild's display name (decoded `string`).
/// Empty for an unknown guild.
pub async fn guild_name(guild_id: u64) -> Result<String, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(String::new());
    }
    let calldata = call_uint("guildName(uint256)", guild_id);
    let result = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    Ok(decode_string(&result).unwrap_or_default())
}

/// Read `guildsOf(address)` → every guildId the address is a member of. Bare
/// dynamic `uint256[]` ABI return (`[offset][len][id0]…`), the SAME decode as
/// `bounties_of`/`jobs_of`. Hostile-length-safe.
pub async fn guilds_of(addr_hex: &str) -> Result<Vec<u64>, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(Vec::new());
    }
    let account = parse_eth_address(addr_hex)?;
    let mut calldata = selector("guildsOf(address)").to_vec();
    calldata.extend_from_slice(&addr_word(&account));
    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(REGISTRY_ADDRESS, &calldata_hex).await?;
    let bytes = hex_to_bytes(&result)?;
    if bytes.len() < 64 {
        return Ok(Vec::new());
    }
    let mut len_buf = [0u8; 8];
    len_buf.copy_from_slice(&bytes[56..64]);
    let len = u64::from_be_bytes(len_buf) as usize;
    let mut out = Vec::new();
    for i in 0..len {
        let start = match i.checked_mul(32).and_then(|o| o.checked_add(64)) {
            Some(s) => s,
            None => break,
        };
        let Some(word) = start.checked_add(32).and_then(|end| bytes.get(start + 24..end)) else {
            break;
        };
        let mut id_buf = [0u8; 8];
        id_buf.copy_from_slice(word);
        out.push(u64::from_be_bytes(id_buf));
    }
    Ok(out)
}

/// Read `guildCount()` → the total number of guilds created (the next-id - 1
/// counter; informational).
pub async fn guild_count() -> Result<u64, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(0);
    }
    let mut calldata = selector("guildCount()").to_vec();
    let _ = &mut calldata; // no args
    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(REGISTRY_ADDRESS, &calldata_hex).await?;
    decode_u256_as_u64(&result)
}

#[cfg(test)]
mod guild_tests {
    use super::*;

    /// `createGuild(string)` — dynamic-string layout (offset 0x20 + length +
    /// padded bytes), the exact `register(string)` shape. A wrong offset would
    /// mis-decode the name on-chain.
    #[test]
    fn create_guild_calldata_layout() {
        let cd = encode_create_guild("builders");
        assert_eq!(&cd[0..4], &selector("createGuild(string)"));
        // head: offset word = 0x20.
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 0x20);
        // length word = 8 ("builders").
        assert_eq!(u64::from_be_bytes(cd[36 + 24..36 + 32].try_into().unwrap()), 8);
        // body = the UTF-8 name, right-padded to 32.
        assert_eq!(&cd[68..68 + 8], b"builders");
        assert_eq!(cd.len(), 4 + 32 + 32 + 32); // selector + offset + len + 1 padded word
    }

    /// `createGuild` with a >32-byte name pads the body to a 64-byte multiple
    /// (two tail words) — guards the `div_ceil` padding.
    #[test]
    fn create_guild_pads_long_name() {
        let name = "a-very-long-guild-name-over-32-bytes!!"; // 38 bytes
        let cd = encode_create_guild(name);
        assert_eq!(u64::from_be_bytes(cd[36 + 24..36 + 32].try_into().unwrap()), name.len() as u64);
        // 38 bytes -> padded to 64; total = sel + 2 head words + 2 tail words.
        assert_eq!(cd.len(), 4 + 32 + 32 + 64);
        assert_eq!(&cd[68..68 + name.len()], name.as_bytes());
    }

    /// `inviteToGuild(uint256,address)` — two static words. The address must be
    /// right-aligned (low 20 bytes of word 1); a left/right padding slip would
    /// invite the wrong account, so test an all-high-bit address.
    #[test]
    fn invite_to_guild_calldata_layout() {
        let member = [0xFFu8; 20];
        let cd = encode_invite_to_guild(0x2A, &member);
        assert_eq!(&cd[0..4], &selector("inviteToGuild(uint256,address)"));
        assert_eq!(cd.len(), 4 + 64);
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 0x2A); // guildId
        // word 1: address in the LOW 20 bytes; top 12 zero.
        assert!(cd[36..36 + 12].iter().all(|&b| b == 0));
        assert_eq!(&cd[36 + 12..36 + 32], &member);
    }

    /// `setRole(uint256,address,uint8)` — three static words; role byte
    /// right-aligned in word 2. Pins both the selector and the role placement.
    #[test]
    fn set_role_calldata_layout() {
        let member = [0xABu8; 20];
        let cd = encode_set_role(7, &member, GuildRole::Officer.as_u8());
        assert_eq!(&cd[0..4], &selector("setRole(uint256,address,uint8)"));
        assert_eq!(cd.len(), 4 + 96);
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 7); // guildId
        assert_eq!(&cd[36 + 12..36 + 32], &member); // member in word 1
        // role = 2 (Officer) in the low byte of word 2.
        assert!(cd[68..68 + 31].iter().all(|&b| b == 0));
        assert_eq!(cd[68 + 31], 2);
    }

    /// `fundGuild(uint256,uint256)` — two static words (guildId, amount).
    #[test]
    fn fund_guild_calldata_layout() {
        let amount = 1_500_000_000_000_000_000u128; // 1.5 $LH
        let cd = encode_fund_guild(9, amount);
        assert_eq!(&cd[0..4], &selector("fundGuild(uint256,uint256)"));
        assert_eq!(cd.len(), 4 + 64);
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 9); // guildId
        // amount in the low 16 bytes of word 1.
        assert_eq!(u128::from_be_bytes(cd[36 + 16..36 + 32].try_into().unwrap()), amount);
    }

    /// `spendTreasury(uint256,address,uint256,bytes)` — the only multi-arg
    /// DYNAMIC layout. memo is LAST, so the offset = 4 head words (0x80). Pins
    /// every head slot + the tail length/body so a shifted offset (the classic
    /// dynamic-encoding bug) is caught.
    #[test]
    fn spend_treasury_calldata_layout() {
        let to = [0xCDu8; 20];
        let amount = 2_000_000_000_000_000_000u128; // 2 $LH
        let memo = b"q3 grant"; // 8 bytes -> one padded tail word
        let cd = encode_spend_treasury(0x10, &to, amount, memo);
        assert_eq!(&cd[0..4], &selector("spendTreasury(uint256,address,uint256,bytes)"));
        // head 0: guildId.
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 0x10);
        // head 1: to (right-aligned).
        assert_eq!(&cd[36 + 12..36 + 32], &to);
        // head 2: amount.
        assert_eq!(u128::from_be_bytes(cd[68 + 16..68 + 32].try_into().unwrap()), amount);
        // head 3: offset to memo = 4 head words = 0x80.
        assert_eq!(u64::from_be_bytes(cd[100 + 24..100 + 32].try_into().unwrap()), 0x80);
        // tail length word at byte 4 + 0x80 = 132.
        assert_eq!(u64::from_be_bytes(cd[132 + 24..132 + 32].try_into().unwrap()), memo.len() as u64);
        // tail body = the memo, right-padded to 32.
        assert_eq!(&cd[164..164 + memo.len()], memo);
        assert_eq!(cd.len(), 4 + 4 * 32 + 32 + 32); // sel + 4 head + len + 1 padded word
    }

    /// `spendTreasury` with an EMPTY memo: offset still 0x80, length 0, no body.
    #[test]
    fn spend_treasury_empty_memo() {
        let to = [0x01u8; 20];
        let cd = encode_spend_treasury(1, &to, 1, b"");
        assert_eq!(u64::from_be_bytes(cd[100 + 24..100 + 32].try_into().unwrap()), 0x80); // offset
        assert_eq!(u64::from_be_bytes(cd[132 + 24..132 + 32].try_into().unwrap()), 0); // length 0
        assert_eq!(cd.len(), 4 + 4 * 32 + 32); // sel + head + length word, no body
    }

    /// Single-arg `uint256` write selectors (accept/leave) route through
    /// `call_uint_bytes` — pin selector + the id word.
    #[test]
    fn accept_and_leave_calldata_layout() {
        let accept = call_uint_bytes("acceptGuildInvite(uint256)", 5);
        assert_eq!(&accept[0..4], &selector("acceptGuildInvite(uint256)"));
        assert_eq!(accept.len(), 36);
        assert_eq!(u64::from_be_bytes(accept[28..36].try_into().unwrap()), 5);
        let leave = call_uint_bytes("leaveGuild(uint256)", 5);
        assert_eq!(&leave[0..4], &selector("leaveGuild(uint256)"));
        assert_eq!(leave.len(), 36);
        assert_eq!(u64::from_be_bytes(leave[28..36].try_into().unwrap()), 5);
    }

    /// Role enum round-trips and clamps unknown bytes to `None`.
    #[test]
    fn guild_role_from_to_u8_and_parse() {
        assert_eq!(GuildRole::from_u8(0), GuildRole::None);
        assert_eq!(GuildRole::from_u8(1), GuildRole::Member);
        assert_eq!(GuildRole::from_u8(2), GuildRole::Officer);
        assert_eq!(GuildRole::from_u8(3), GuildRole::Admin);
        assert_eq!(GuildRole::from_u8(99), GuildRole::None); // unknown clamps
        for r in [GuildRole::None, GuildRole::Member, GuildRole::Officer, GuildRole::Admin] {
            assert_eq!(GuildRole::from_u8(r.as_u8()), r);
        }
        assert_eq!(GuildRole::parse("member").unwrap(), GuildRole::Member);
        assert_eq!(GuildRole::parse("  OFFICER ").unwrap(), GuildRole::Officer);
        assert_eq!(GuildRole::parse("Admin").unwrap(), GuildRole::Admin);
        assert!(GuildRole::parse("none").is_err()); // not a settable rank
        assert!(GuildRole::parse("boss").is_err());
    }
}

#[cfg(test)]
mod x402_tests {
    use super::*;

    #[test]
    fn x402_domain_matches_live_facet() {
        // Pinned to the deployed X402Facet's `x402DomainSeparator()` on the
        // diamond — guards the Rust EIP-712 encoding against the contract.
        let expected =
            "54530933a67f96286ac528dbff39d00c0ea49f4c6bd0f034343a0c78927f0b7a";
        let got = x402_domain_separator().unwrap();
        assert_eq!(bytes_to_hex(&got), expected);
    }

    #[test]
    fn x402_sign_recovers_payer() {
        let w = crate::wallet::generate();
        let from = w.address;
        let to = [0x11u8; 20];
        let nonce = [0x22u8; 32];
        let sig = sign_x402(&w.signer, &from, &to, 1_000, 0, 9_999_999_999, &nonce).unwrap();
        let digest = x402_digest(&from, &to, 1_000, 0, 9_999_999_999, &nonce).unwrap();
        // EIP-712 digest is signed directly (no personal-sign prefix).
        let recovered = crate::wallet::recover_address(&sig, &digest).unwrap();
        assert_eq!(recovered, from);
    }

    // --- Calldata-encoder layout guards (network-free). A wrong ABI offset
    // here would send $LH / NFTs to the wrong place, so pin the layout. ---

    #[test]
    fn erc721_transfer_from_calldata_layout() {
        let from = [0xAAu8; 20];
        let to = [0xBBu8; 20];
        let cd = encode_erc721_transfer_from(&from, &to, 0x1234);
        // Canonical ERC-721/20 transferFrom(address,address,uint256) selector.
        assert_eq!(&cd[0..4], &[0x23, 0xb8, 0x72, 0xdd]);
        assert_eq!(cd.len(), 4 + 96);
        assert_eq!(&cd[4 + 12..4 + 32], &from); // from in word 0
        assert_eq!(&cd[4 + 44..4 + 64], &to); // to in word 1
        assert_eq!(u64::from_be_bytes(cd[4 + 88..4 + 96].try_into().unwrap()), 0x1234);
    }

    #[test]
    fn release_name_calldata_layout() {
        let cd = encode_release_name(7);
        assert_eq!(&cd[0..4], &selector("releaseName(uint256)"));
        assert_eq!(cd.len(), 36);
        assert_eq!(u64::from_be_bytes(cd[28..36].try_into().unwrap()), 7);
    }

    #[test]
    fn link_unlink_device_calldata_layout() {
        let dev = [0xCDu8; 20];
        let link = encode_link_device(3, &dev);
        assert_eq!(&link[0..4], &selector("linkDevice(uint256,address)"));
        assert_eq!(link.len(), 68);
        assert_eq!(u64::from_be_bytes(link[28..36].try_into().unwrap()), 3); // mainId
        assert_eq!(&link[36 + 12..36 + 32], &dev); // device in word 2
        let unlink = encode_unlink_device(3, &dev);
        assert_eq!(&unlink[0..4], &selector("unlinkDevice(uint256,address)"));
        assert_eq!(unlink.len(), 68);
        assert_eq!(&unlink[36 + 12..36 + 32], &dev);
    }

    #[test]
    fn deposit_credits_calldata_layout() {
        let cd = encode_deposit_credits(1_000_000_000_000_000_000);
        assert_eq!(&cd[0..4], &selector("depositCredits(uint256)"));
        assert_eq!(cd.len(), 36);
    }

    /// ERC-20 `transfer(address,uint256)` — the `send_lh` payload. A wrong
    /// selector or mis-padded address word sends `$LH` to the wrong account.
    /// Tests an address with the HIGH bit of every byte set, so a left/right
    /// padding mistake (top 12 bytes vs low 20) would be caught.
    #[test]
    fn transfer_calldata_layout() {
        let to = [0xFFu8; 20];
        let amount = 1_500_000_000_000_000_000u128; // 1.5 $LH
        let cd = encode_transfer(&to, amount);
        // keccak256("transfer(address,uint256)")[0..4] = 0xa9059cbb.
        assert_eq!(&cd[0..4], &[0xa9, 0x05, 0x9c, 0xbb]);
        assert_eq!(cd.len(), 4 + 64);
        // Address right-aligned in word 0: top 12 bytes ZERO, low 20 = `to`.
        assert_eq!(&cd[4..4 + 12], &[0u8; 12]);
        assert_eq!(&cd[4 + 12..4 + 32], &to);
        // Amount as a full uint256 in word 1 (16 high bytes zero, low 16 = u128).
        assert_eq!(&cd[4 + 32..4 + 48], &[0u8; 16]);
        assert_eq!(
            u128::from_be_bytes(cd[4 + 48..4 + 64].try_into().unwrap()),
            amount
        );
    }

    /// ERC-20 `approve(address,uint256)` with `u128::MAX` (the one-time
    /// "approve forever" the mcp-call path uses). The amount must land as
    /// 2^128-1 in the LOW 16 bytes of word 1, NOT wrap or shift.
    #[test]
    fn approve_calldata_layout_max_amount() {
        let spender = [0xABu8; 20];
        let cd = encode_approve(&spender, u128::MAX);
        // keccak256("approve(address,uint256)")[0..4] = 0x095ea7b3.
        assert_eq!(&cd[0..4], &[0x09, 0x5e, 0xa7, 0xb3]);
        assert_eq!(cd.len(), 4 + 64);
        assert_eq!(&cd[4 + 12..4 + 32], &spender);
        // High 16 bytes of the amount word are zero; low 16 are all 0xFF.
        assert_eq!(&cd[4 + 32..4 + 48], &[0u8; 16]);
        assert_eq!(&cd[4 + 48..4 + 64], &[0xFFu8; 16]);
    }

    /// x402 `settle(...)` calldata: the dynamic-`bytes` signature must be
    /// pointed at by the right offset (7 words) and length-prefixed with 65,
    /// then zero-padded to a 32-byte multiple. A wrong offset/length makes the
    /// facet read a bogus signature → reject (or worse, accept the wrong one).
    #[test]
    fn settle_calldata_layout() {
        let from = [0x11u8; 20];
        let to = [0x22u8; 20];
        let nonce = [0x33u8; 32];
        let sig = [0x44u8; 65];
        let value = 7_000u128;
        let cd = encode_settle(&from, &to, value, 1, 2, &nonce, &sig);
        assert_eq!(
            &cd[0..4],
            &selector("settle(address,address,uint256,uint256,uint256,bytes32,bytes)")
        );
        // 6 static words + offset word + length word + 65 sig + 31 pad = 96 tail.
        assert_eq!(cd.len(), 4 + 6 * 32 + 32 + 32 + 96);
        assert_eq!(&cd[4 + 12..4 + 32], &from); // word 0
        assert_eq!(&cd[4 + 44..4 + 64], &to); // word 1
        assert_eq!(u128::from_be_bytes(cd[4 + 80..4 + 96].try_into().unwrap()), value); // word 2
        assert_eq!(&cd[4 + 5 * 32..4 + 6 * 32], &nonce); // word 5
        // Word 6 = offset to the bytes arg = 7*32 = 224.
        assert_eq!(u64::from_be_bytes(cd[4 + 6 * 32 + 24..4 + 7 * 32].try_into().unwrap()), 7 * 32);
        // Word 7 = bytes length = 65.
        assert_eq!(u64::from_be_bytes(cd[4 + 7 * 32 + 24..4 + 8 * 32].try_into().unwrap()), 65);
        // The 65 signature bytes follow, then zero padding to a 32-multiple.
        assert_eq!(&cd[4 + 8 * 32..4 + 8 * 32 + 65], &sig);
        assert_eq!(&cd[4 + 8 * 32 + 65..], &[0u8; 31]);
    }

    /// `scheduleJob(uint256,bytes,uint64,uint128,uint32)` calldata: the
    /// dynamic `bytes task` is the 2nd arg, so head word 1 must hold the
    /// offset (5 fixed head words = 160) and the tail must be length-prefixed
    /// then zero-padded to a 32-byte multiple. The four scalars are static head
    /// words AFTER the offset. A wrong offset/length would make the facet read a
    /// bogus task (or revert), and a mis-placed scalar would escrow the wrong
    /// budget / set the wrong interval — so pin every word.
    #[test]
    fn schedule_job_calldata_layout() {
        let task = b"ping the oracle"; // 15 bytes -> pads to 32
        let cd = encode_schedule_job(0x42, task, 300, 1_500_000_000_000_000_000u128, 100);
        assert_eq!(&cd[0..4], &selector("scheduleJob(uint256,bytes,uint64,uint128,uint32)"));
        // 5 static head words + length word + 32 bytes of padded task tail.
        assert_eq!(cd.len(), 4 + 5 * 32 + 32 + 32);
        // Word 0: targetId.
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 0x42);
        // Word 1: offset to the bytes tail = 5*32 = 160.
        assert_eq!(u64::from_be_bytes(cd[4 + 32 + 24..4 + 2 * 32].try_into().unwrap()), 5 * 32);
        // Word 2: interval (uint64, right-aligned).
        assert_eq!(u64::from_be_bytes(cd[4 + 2 * 32 + 24..4 + 3 * 32].try_into().unwrap()), 300);
        // Word 3: budgetWei (uint128 in the low 16 bytes).
        assert_eq!(
            u128::from_be_bytes(cd[4 + 3 * 32 + 16..4 + 4 * 32].try_into().unwrap()),
            1_500_000_000_000_000_000u128
        );
        // Word 4: maxRuns (uint32, right-aligned).
        assert_eq!(u64::from_be_bytes(cd[4 + 4 * 32 + 24..4 + 5 * 32].try_into().unwrap()), 100);
        // Tail word 5: bytes length = 15.
        assert_eq!(
            u64::from_be_bytes(cd[4 + 5 * 32 + 24..4 + 6 * 32].try_into().unwrap()),
            task.len() as u64
        );
        // The task bytes follow, then zero padding to the 32-byte boundary.
        assert_eq!(&cd[4 + 6 * 32..4 + 6 * 32 + task.len()], task);
        assert_eq!(&cd[4 + 6 * 32 + task.len()..], &[0u8; 32 - 15]);
    }

    /// A task that is an EXACT 32-byte multiple needs NO trailing padding —
    /// guard the `div_ceil` boundary (a 32-byte task must not gain a phantom
    /// 32-byte zero word).
    #[test]
    fn schedule_job_task_exact_multiple_no_extra_pad() {
        let task = [0xABu8; 32];
        let cd = encode_schedule_job(1, &task, 60, 1, 1);
        // 5 head + length + exactly 32 bytes of task, no extra pad word.
        assert_eq!(cd.len(), 4 + 5 * 32 + 32 + 32);
        assert_eq!(&cd[4 + 6 * 32..], &task);
    }

    #[test]
    fn cancel_job_calldata_layout() {
        let cd = encode_cancel_job(9);
        assert_eq!(&cd[0..4], &selector("cancelJob(uint256)"));
        assert_eq!(cd.len(), 36);
        assert_eq!(u64::from_be_bytes(cd[28..36].try_into().unwrap()), 9);
    }

    #[test]
    fn scheduled_job_status_label_maps_enum() {
        let mut j = ScheduledJob {
            owner: "0x00".into(),
            interval: 60,
            status: 0,
            next_run: 0,
            budget_wei: 0,
            runs_left: 0,
            target_id: 0,
        };
        assert_eq!(j.status_label(), "active");
        j.status = 1;
        assert_eq!(j.status_label(), "paused");
        j.status = 2;
        assert_eq!(j.status_label(), "cancelled");
        j.status = 3;
        assert_eq!(j.status_label(), "exhausted");
        j.status = 9;
        assert_eq!(j.status_label(), "unknown");
    }

    /// `rank_agent_matches` hostile inputs: case-insensitivity, name-tier vs
    /// persona-tier ordering, substring (not word) matching, empty registry,
    /// all-whitespace query (returns all), and duplicate handling.
    #[test]
    fn rank_agent_matches_hostile_inputs() {
        // Empty registry → empty, regardless of query.
        assert!(rank_agent_matches(&[], "anything").is_empty());
        assert!(rank_agent_matches(&[], "").is_empty());

        let agents = vec![
            ("auditor".to_string(), "reviews code".to_string()),
            ("bob".to_string(), "I AUDIT contracts".to_string()),
            ("carol".to_string(), "unrelated".to_string()),
        ];
        // Substring (not whole-word) match: "audit" hits the name "auditor"
        // (name tier) AND the persona "I AUDIT" (persona tier, case-insensitive).
        let hits = rank_agent_matches(&agents, "audit");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0, "auditor"); // name tier first
        assert_eq!(hits[1].0, "bob"); // persona tier second
        // Whitespace-padded query is trimmed, then matched.
        assert_eq!(rank_agent_matches(&agents, "  AUDIT  ").len(), 2);
        // All-whitespace query returns the whole list, order preserved.
        let all = rank_agent_matches(&agents, "\t \n");
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].0, "auditor");
        // A name match is NOT also double-counted into the persona tier (else-if).
        let dual = vec![("audit".to_string(), "audit audit".to_string())];
        assert_eq!(rank_agent_matches(&dual, "audit").len(), 1);
    }

    // --- BountyFacet calldata layouts (network-free). A wrong offset/length on
    // the dynamic `bytes` args would escrow against a bogus task or pay the
    // wrong claimant, so pin every word. ---

    /// `postBounty(bytes task, uint128 rewardWei, uint64 ttlSeconds)`: the
    /// dynamic `bytes task` is the FIRST arg, so head word 0 is the offset
    /// (3 fixed head words = 96) and the tail is length-prefixed + zero-padded.
    #[test]
    fn post_bounty_calldata_layout() {
        let task = b"audit my solidity contract"; // 26 bytes -> pads to 32
        let reward = 5_000_000_000_000_000_000u128; // 5 $LH
        let cd = encode_post_bounty(task, reward, 86_400);
        assert_eq!(&cd[0..4], &selector("postBounty(bytes,uint128,uint64)"));
        // 3 static head words + length word + 32 bytes padded task tail.
        assert_eq!(cd.len(), 4 + 3 * 32 + 32 + 32);
        // Word 0: offset to the bytes tail = 3*32 = 96.
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 3 * 32);
        // Word 1: rewardWei (uint128 in the low 16 bytes).
        assert_eq!(
            u128::from_be_bytes(cd[4 + 32 + 16..4 + 2 * 32].try_into().unwrap()),
            reward
        );
        // Word 2: ttlSeconds (uint64, right-aligned).
        assert_eq!(u64::from_be_bytes(cd[4 + 2 * 32 + 24..4 + 3 * 32].try_into().unwrap()), 86_400);
        // Tail word 3: bytes length = 26.
        assert_eq!(
            u64::from_be_bytes(cd[4 + 3 * 32 + 24..4 + 4 * 32].try_into().unwrap()),
            task.len() as u64
        );
        // The task bytes follow, then zero padding to the 32-byte boundary.
        assert_eq!(&cd[4 + 4 * 32..4 + 4 * 32 + task.len()], task);
        assert_eq!(&cd[4 + 4 * 32 + task.len()..], &[0u8; 32 - 26]);
    }

    /// A task that is an EXACT 32-byte multiple gets NO trailing pad word —
    /// guard the `div_ceil` boundary.
    #[test]
    fn post_bounty_task_exact_multiple_no_extra_pad() {
        let task = [0xCDu8; 64];
        let cd = encode_post_bounty(&task, 1, 60);
        // 3 head + length + exactly 64 bytes of task, no extra pad word.
        assert_eq!(cd.len(), 4 + 3 * 32 + 32 + 64);
        assert_eq!(&cd[4 + 4 * 32..], &task);
    }

    /// `claimBounty(uint256 bountyId, uint256 claimantTokenId)`: two static
    /// words — the bountyId then the CLAIMANT'S OWN tokenId. A swapped pair
    /// would claim the wrong bounty or credit the wrong identity.
    #[test]
    fn claim_bounty_calldata_layout() {
        let cd = encode_claim_bounty(7, 42);
        assert_eq!(&cd[0..4], &selector("claimBounty(uint256,uint256)"));
        assert_eq!(cd.len(), 4 + 64);
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 7); // bountyId
        assert_eq!(u64::from_be_bytes(cd[4 + 32 + 24..4 + 2 * 32].try_into().unwrap()), 42); // claimantTokenId
    }

    /// `submitResult(uint256 bountyId, bytes result)`: a static `bountyId` head
    /// word then a dynamic `bytes` (offset = 2 head words = 64).
    #[test]
    fn submit_result_calldata_layout() {
        let result = b"done: see ipfs://Qm..."; // 22 bytes -> pads to 32
        let cd = encode_submit_result(3, result);
        assert_eq!(&cd[0..4], &selector("submitResult(uint256,bytes)"));
        assert_eq!(cd.len(), 4 + 2 * 32 + 32 + 32);
        // Word 0: bountyId.
        assert_eq!(u64::from_be_bytes(cd[4 + 24..4 + 32].try_into().unwrap()), 3);
        // Word 1: offset to the bytes tail = 2*32 = 64.
        assert_eq!(u64::from_be_bytes(cd[4 + 32 + 24..4 + 2 * 32].try_into().unwrap()), 2 * 32);
        // Word 2: bytes length = 22.
        assert_eq!(
            u64::from_be_bytes(cd[4 + 2 * 32 + 24..4 + 3 * 32].try_into().unwrap()),
            result.len() as u64
        );
        assert_eq!(&cd[4 + 3 * 32..4 + 3 * 32 + result.len()], result);
        assert_eq!(&cd[4 + 3 * 32 + result.len()..], &[0u8; 32 - 22]);
    }

    /// The single-arg bounty WRITE selectors (`acceptResult`/`cancelBounty`/
    /// `reclaimExpired`) are `fn(uint256)` — one selector + one id word.
    #[test]
    fn single_arg_bounty_calldata_layouts() {
        for sig in [
            "acceptResult(uint256)",
            "cancelBounty(uint256)",
            "reclaimExpired(uint256)",
        ] {
            let cd = call_uint_bytes(sig, 11);
            assert_eq!(&cd[0..4], &selector(sig));
            assert_eq!(cd.len(), 36);
            assert_eq!(u64::from_be_bytes(cd[28..36].try_into().unwrap()), 11);
        }
    }

    /// `decode_uint_array_with_cursor` decodes a `(uint256[], uint256)` ABI
    /// return (the `openBounties` shape): the array offset in word 0, the cursor
    /// in word 1, then `[len][id…]` at the offset. Build a canonical encoding
    /// and round-trip it; the cursor word is dropped.
    #[test]
    fn open_bounties_cursor_decode() {
        let mut bytes = Vec::new();
        // Word 0: offset to the array = 64 (the array sits after the two head
        // words). Word 1: cursor (ignored by the decoder).
        bytes.extend_from_slice(&u256_be(64));
        bytes.extend_from_slice(&u256_be(99)); // cursor
        // Array body at offset 64: length = 3, then ids 5, 8, 13.
        bytes.extend_from_slice(&u256_be(3));
        bytes.extend_from_slice(&u256_be(5));
        bytes.extend_from_slice(&u256_be(8));
        bytes.extend_from_slice(&u256_be(13));
        let ids = decode_uint_array_with_cursor(&bytes).unwrap();
        assert_eq!(ids, vec![5, 8, 13]);
    }

    /// Empty / short / hostile returns must not panic. A too-short response
    /// yields an empty list; a bogus (huge) length stops the decode at the
    /// buffer edge rather than over-reading.
    #[test]
    fn open_bounties_cursor_decode_hostile() {
        assert!(decode_uint_array_with_cursor(&[]).unwrap().is_empty());
        assert!(decode_uint_array_with_cursor(&[0u8; 32]).unwrap().is_empty());
        // Offset 64, but claim length 1000 with only one id word present →
        // decode stops at the buffer edge (no panic, partial read).
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&u256_be(64));
        bytes.extend_from_slice(&u256_be(0));
        bytes.extend_from_slice(&u256_be(1000)); // lying length
        bytes.extend_from_slice(&u256_be(7)); // only one real id
        let ids = decode_uint_array_with_cursor(&bytes).unwrap();
        assert_eq!(ids, vec![7]); // stopped cleanly after the available word
    }

    /// `Bounty::status_label` maps every documented enum byte (and unknowns).
    #[test]
    fn bounty_status_label_maps_enum() {
        let mut b = Bounty {
            poster: "0x00".into(),
            reward_wei: 0,
            expiry: 0,
            status: 0,
            claimant_token_id: 0,
        };
        for (s, label) in [
            (0u8, "open"),
            (1, "claimed"),
            (2, "submitted"),
            (3, "paid"),
            (4, "cancelled"),
            (5, "reclaimed"),
            (9, "unknown"),
        ] {
            b.status = s;
            assert_eq!(b.status_label(), label);
        }
    }

    /// `discover_bounties`' ranking reuses `rank_agent_matches` over the task
    /// text (the id occupies the "name" slot, the task the "persona" slot). A
    /// query hits a bounty whose TASK contains it; an empty query keeps all.
    /// (Pure-ranking exercise of the same pipeline `discover_bounties` runs.)
    #[test]
    fn bounty_rank_over_task_text() {
        let pairs = vec![
            ("1".to_string(), "audit a solidity contract".to_string()),
            ("2".to_string(), "write a poem".to_string()),
            ("3".to_string(), "SOLIDITY gas review".to_string()),
        ];
        let hits = rank_agent_matches(&pairs, "solidity");
        // Two tasks mention solidity (case-insensitive); both rank in the
        // persona tier (ids never contain the query), input order preserved.
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0, "1");
        assert_eq!(hits[1].0, "3");
        // Empty query keeps the whole board.
        assert_eq!(rank_agent_matches(&pairs, "").len(), 3);
    }
}

#[cfg(target_arch = "wasm32")]
fn log_main_warning(err: &str) {
    use wasm_bindgen::JsValue;
    web_sys::console::warn_1(&JsValue::from_str(&format!("auto-set MAIN: {err}")));
}
#[cfg(not(target_arch = "wasm32"))]
fn log_main_warning(_err: &str) {
    // Native path doesn't have a console; silent — callers can check
    // mainOf themselves after the fact if they need to verify.
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
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<RpcError>,
}

#[derive(Deserialize)]
struct RpcError {
    #[allow(dead_code)]
    code: i64,
    message: String,
}

/// Transport-level deadline for a single JSON-RPC READ (`rpc_value` /
/// `eth_call_batch`). Generous — real reads are sub-2s, but a big `eth_call`
/// under load shouldn't trip it. Its job is to bound the pathological case: a
/// TCP-connected-but-silent RPC node ("black hole"). On `wasm32`, `reqwest`
/// wraps the browser `fetch` API, which has NO default timeout AND
/// `reqwest::Client::timeout` is a documented no-op — so without this guard
/// such a node yields a future that never resolves and hangs EVERY consumer
/// (CLI, off-bundle, every browser paint site), not just the few UI paths that
/// wrap calls in `app::net::with_timeout`.
const RPC_TIMEOUT_MS: u32 = 20_000;

/// Build the shared read client. On native, `reqwest`'s own `timeout` works, so
/// set it directly (covers connect + the whole request/body). On wasm it's a
/// no-op (see [`RPC_TIMEOUT_MS`]) — the caller races against [`sleep_ms`].
fn read_client() -> reqwest::Client {
    #[cfg(not(target_arch = "wasm32"))]
    {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(RPC_TIMEOUT_MS as u64))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    }
    #[cfg(target_arch = "wasm32")]
    {
        reqwest::Client::new()
    }
}

/// Race a read future against an [`RPC_TIMEOUT_MS`] timer and return its output,
/// or a timeout `Err`. This is the portable backstop for the wasm no-op-timeout
/// case (`reqwest::Client::timeout` does nothing on `fetch`): it mirrors
/// `app::net::with_timeout`, racing the work against the cfg-gated [`sleep_ms`]
/// (tokio on native / `setTimeout` Promise on wasm) via
/// `futures_util::future::select`. The losing future is dropped (browser
/// `fetch` cancels on drop). Runs on BOTH targets — on native it's belt-and-
/// suspenders alongside the client builder timeout; on wasm it IS the timeout.
async fn timeout_send<F, T>(label: &str, fut: F) -> Result<T, String>
where
    F: std::future::Future<Output = T>,
{
    use futures_util::future::{select, Either};
    let work = std::pin::pin!(fut);
    let timer = std::pin::pin!(sleep_ms(RPC_TIMEOUT_MS));
    match select(work, timer).await {
        Either::Left((out, _)) => Ok(out),
        Either::Right(((), _)) => Err(format!(
            "{label}: RPC request timed out after {}s",
            RPC_TIMEOUT_MS / 1000
        )),
    }
}

/// Raw JSON-RPC call returning the `result` field verbatim. Methods like
/// `eth_getLogs` return arrays, so the result type must stay a `Value`
/// rather than being forced into a `String` (which silently broke log
/// decoding — the in-app feedback list).
async fn rpc_value(method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let body = RpcRequest {
        jsonrpc: "2.0",
        id: 1,
        method,
        params,
    };
    let client = read_client();
    // Race send + body-read against the deadline as ONE future so a node that
    // connects then stalls mid-body can't hang either step (the wasm case).
    let parsed: RpcResponse = timeout_send(method, async {
        let resp = client
            .post(RPC_URL)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("{method} send: {e}"))?;
        resp.json::<RpcResponse>()
            .await
            .map_err(|e| format!("{method} decode: {e}"))
    })
    .await??;
    if let Some(err) = parsed.error {
        return Err(format!("{method}: {}", err.message));
    }
    parsed
        .result
        .ok_or_else(|| format!("{method} returned no result"))
}

/// JSON-RPC call whose result is a string (hex quantity, tx hash, etc.).
async fn rpc(method: &str, params: serde_json::Value) -> Result<String, String> {
    let value = rpc_value(method, params).await?;
    value
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("{method}: expected string result"))
}

async fn eth_call(to: &str, data_hex: &str) -> Result<String, String> {
    rpc(
        "eth_call",
        serde_json::json!([{ "to": to, "data": data_hex }, "latest"]),
    )
    .await
}

/// Build calldata for a `fn(uint256)` selector with a single id argument.
fn call_uint(sig: &str, id: u64) -> String {
    let mut data = Vec::with_capacity(4 + 32);
    data.extend_from_slice(&selector(sig));
    data.extend_from_slice(&u256_be(id as u128));
    format!("0x{}", bytes_to_hex(&data))
}

/// Decode an ABI `address` return (right-aligned in 32 bytes). `None` for the
/// zero address or a short result.
fn decode_address(result_hex: &str) -> Option<String> {
    let trimmed = result_hex.trim().trim_start_matches("0x");
    if trimmed.len() < 64 {
        return None;
    }
    let addr_hex = &trimmed[trimmed.len() - 40..];
    if addr_hex.chars().all(|c| c == '0') {
        return None;
    }
    Some(format!("0x{}", addr_hex.to_lowercase()))
}

/// Decode an ABI `string` return (offset + length + bytes). `None` on a
/// short/truncated/invalid body.
fn decode_string(result_hex: &str) -> Option<String> {
    let raw = hex_to_bytes(result_hex).ok()?;
    if raw.len() < 64 {
        return None;
    }
    let len = u64::from_be_bytes(raw[56..64].try_into().ok()?) as usize;
    // `len` is attacker-controlled — slice via checked add so a huge length
    // returns None instead of overflowing.
    let end = len.checked_add(64)?;
    let body = raw.get(64..end)?;
    String::from_utf8(body.to_vec()).ok()
}

/// Send many `eth_call`s as ONE JSON-RPC batch (a single POST). Returns each
/// call's `result` hex in input order; a per-call RPC error maps to `Err` for
/// just that entry. Collapses an N-token scan from N round-trips into one.
async fn eth_call_batch(calls: &[(&str, String)]) -> Result<Vec<Result<String, String>>, String> {
    if calls.is_empty() {
        return Ok(Vec::new());
    }
    let batch: Vec<serde_json::Value> = calls
        .iter()
        .enumerate()
        .map(|(i, (to, data))| {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": i,
                "method": "eth_call",
                "params": [{ "to": to, "data": data }, "latest"],
            })
        })
        .collect();
    let client = read_client();
    // Same deadline as the single-call path — race send + body-read together.
    let parsed: Vec<serde_json::Value> = timeout_send("eth_call batch", async {
        let resp = client
            .post(RPC_URL)
            .json(&serde_json::Value::Array(batch))
            .send()
            .await
            .map_err(|e| format!("eth_call batch send: {e}"))?;
        resp.json::<Vec<serde_json::Value>>()
            .await
            .map_err(|e| format!("eth_call batch decode: {e}"))
    })
    .await??;
    // Batch responses may arrive out of order — index by the `id` we set.
    let mut out: Vec<Result<String, String>> = (0..calls.len())
        .map(|_| Err("missing batch response".to_string()))
        .collect();
    for item in parsed {
        let Some(idx) = item.get("id").and_then(|v| v.as_u64()).map(|i| i as usize) else {
            continue;
        };
        if idx >= out.len() {
            continue;
        }
        if let Some(err) = item.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("rpc error");
            out[idx] = Err(msg.to_string());
        } else if let Some(result) = item.get("result").and_then(|r| r.as_str()) {
            out[idx] = Ok(result.to_string());
        }
    }
    Ok(out)
}

async fn eth_get_logs(
    address: &str,
    topics: Vec<serde_json::Value>,
    from_block: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let result = rpc_value(
        "eth_getLogs",
        serde_json::json!([{
            "address": address,
            "topics": topics,
            "fromBlock": from_block,
            "toBlock": "latest"
        }]),
    )
    .await?;
    match result {
        serde_json::Value::Array(logs) => Ok(logs),
        _ => Ok(Vec::new()),
    }
}

/// Get the list of authorized signers for a TBA by reading
/// SignerAdded / SignerRemoved events and computing the current set.
pub async fn tba_signers(tba_hex: &str) -> Result<Vec<String>, String> {
    use sha3::{Digest, Keccak256};

    let added_topic = format!("0x{}", bytes_to_hex(
        &Keccak256::digest(b"SignerAdded(address,address)")
    ));
    let removed_topic = format!("0x{}", bytes_to_hex(
        &Keccak256::digest(b"SignerRemoved(address,address)")
    ));

    // DEPRECATED: log-scraping signers is wrong (and Tempo caps
    // eth_getLogs at 100k blocks anyway). Use `devices_of` — the on-chain
    // enumerable index in DeviceRegistryFacet — read in a single call.
    let added_logs = eth_get_logs(
        tba_hex,
        vec![serde_json::json!(added_topic)],
        "0x0",
    ).await.unwrap_or_default();

    let removed_logs = eth_get_logs(
        tba_hex,
        vec![serde_json::json!(removed_topic)],
        "0x0",
    ).await.unwrap_or_default();

    let mut signers = std::collections::HashSet::new();

    for log in &added_logs {
        if let Some(topics) = log.get("topics").and_then(|t| t.as_array()) {
            // topic[1] = indexed signer address (32 bytes, address in last 20)
            if let Some(topic) = topics.get(1).and_then(|t| t.as_str()) {
                let addr = format!("0x{}", &topic.trim_start_matches("0x")[24..]);
                signers.insert(addr.to_lowercase());
            }
        }
    }

    for log in &removed_logs {
        if let Some(topics) = log.get("topics").and_then(|t| t.as_array()) {
            if let Some(topic) = topics.get(1).and_then(|t| t.as_str()) {
                let addr = format!("0x{}", &topic.trim_start_matches("0x")[24..]);
                signers.remove(&addr.to_lowercase());
            }
        }
    }

    Ok(signers.into_iter().collect())
}

/// One harvested `FeedbackSubmitted` event from the registry diamond.
#[derive(Debug, Clone)]
pub struct FeedbackEntry {
    /// Submitter address (`0x…`, lowercase).
    pub sender: String,
    /// Unix seconds the contract stamped at submission.
    pub timestamp: u64,
    /// The feedback text.
    pub text: String,
}

/// ABI-encode `submitFeedback(string)`: selector + offset(0x20) + length +
/// the UTF-8 bytes padded to a 32-byte boundary.
pub fn encode_submit_feedback(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let padded = len.div_ceil(32) * 32;
    let mut buf = Vec::with_capacity(4 + 64 + padded);
    buf.extend_from_slice(&selector("submitFeedback(string)"));
    buf.extend_from_slice(&u256_be(0x20));
    buf.extend_from_slice(&u256_be(len as u128));
    buf.extend_from_slice(bytes);
    buf.resize(4 + 64 + padded, 0);
    buf
}

/// Submit on-chain feedback via `FeedbackFacet.submitFeedback`, sponsored.
/// Gas is LENGTH-SCALED: the facet stores the full string in cold SSTOREs
/// (~1.3M for a short note up to ~17M near the 2048-byte cap), so a flat cap
/// silently out-of-gasses long notes (see CLAUDE.md feedback-gas gotcha).
pub async fn submit_feedback_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    text: &str,
    fee_token: &str,
) -> Result<String, String> {
    let diamond = parse_eth_address(REGISTRY_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond,
        value_wei: 0,
        input: encode_submit_feedback(text),
    };
    let gas = 1_500_000u128 + (text.len() as u128) * 9_000;
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, gas).await
}

/// Read recent `FeedbackSubmitted(address indexed sender, uint256
/// timestamp, string text)` events from the diamond, newest first.
///
/// Tempo caps `eth_getLogs` to a 100k-block window, so we scan the most
/// recent ~99k blocks (same bound as `scripts/harvest-feedback.sh`).
/// The non-indexed `(timestamp, text)` payload is ABI-decoded from the
/// log `data`; `sender` comes from the indexed topic.
pub async fn list_feedback() -> Result<Vec<FeedbackEntry>, String> {
    use sha3::{Digest, Keccak256};
    let topic0 = format!(
        "0x{}",
        bytes_to_hex(&Keccak256::digest(b"FeedbackSubmitted(address,uint256,string)"))
    );

    let latest_hex = rpc("eth_blockNumber", serde_json::json!([])).await?;
    let latest = parse_hex_quantity(&latest_hex)? as u64;
    let from = latest.saturating_sub(99_000);
    let from_hex = format!("0x{from:x}");

    let logs = eth_get_logs(REGISTRY_ADDRESS, vec![serde_json::json!(topic0)], &from_hex).await?;

    let mut out = Vec::new();
    for log in &logs {
        let sender = log
            .get("topics")
            .and_then(|t| t.as_array())
            .and_then(|t| t.get(1))
            .and_then(|t| t.as_str())
            .map(|t| format!("0x{}", &t.trim_start_matches("0x")[24..]).to_lowercase())
            .unwrap_or_default();
        let Some(data_hex) = log.get("data").and_then(|d| d.as_str()) else {
            continue;
        };
        let Ok(bytes) = hex_to_bytes(data_hex) else { continue };
        if let Some(entry) = decode_feedback_data(&bytes, sender) {
            out.push(entry);
        }
    }
    out.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(out)
}

/// Decode a `(uint256 timestamp, string text)` ABI payload. Layout:
/// word0 = timestamp, word1 = offset (0x40), word2 = string length,
/// then the UTF-8 bytes.
fn decode_feedback_data(bytes: &[u8], sender: String) -> Option<FeedbackEntry> {
    if bytes.len() < 96 {
        return None;
    }
    let mut ts = [0u8; 8];
    ts.copy_from_slice(&bytes[24..32]); // low 8 bytes of the uint256
    let timestamp = u64::from_be_bytes(ts);

    let mut len_buf = [0u8; 8];
    len_buf.copy_from_slice(&bytes[88..96]); // low 8 bytes of the length word
    let len = u64::from_be_bytes(len_buf) as usize;

    // `len` is attacker-controlled — `96 + len` could overflow, so add checked.
    let end = len.checked_add(96)?;
    let text_bytes = bytes.get(96..end)?;
    let text = String::from_utf8_lossy(text_bytes).into_owned();
    Some(FeedbackEntry { sender, timestamp, text })
}

// ─── Agent-teams P2P signaling (SignalingFacet) ────────────────────────────
// The on-chain seam for the WebRTC collaboration layer: a peer announces an
// EPHEMERAL signaling key under a TOPIC, discovers others via `peersOf`, then
// exchanges SDP offers/answers through `postSignal`/`inboxOf` (blobs sealed to
// the recipient's ephemeral pubkey). Topics:
//   - own devices: keccak256("localharness.devices" || owner_addr)
//   - agent team:  keccak256("localharness.team"   || team_id)
// `Presence` and `Signal` share the ABI shape `(address, uint64, bytes)`, so one
// decoder serves both reads.

/// Signaling topic for an owner's OWN devices.
pub fn devices_topic(owner_addr: &str) -> [u8; 32] {
    let mut pre = b"localharness.devices".to_vec();
    if let Ok(a) = parse_eth_address(owner_addr) {
        pre.extend_from_slice(&a);
    }
    keccak_key(&pre)
}

/// Signaling topic for an agent team.
pub fn team_topic(team_id: u64) -> [u8; 32] {
    let mut pre = b"localharness.team".to_vec();
    pre.extend_from_slice(&u256_be(team_id as u128));
    keccak_key(&pre)
}

/// 32-byte ABI word for an address (left-padded).
fn address_word(addr: &[u8; 20]) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[12..32].copy_from_slice(addr);
    w
}

/// ABI-encode a trailing dynamic `bytes` (length word + padded data) onto `d`.
fn push_abi_bytes(d: &mut Vec<u8>, bytes: &[u8]) {
    d.extend_from_slice(&u256_be(bytes.len() as u128));
    d.extend_from_slice(bytes);
    let pad = (32 - (bytes.len() % 32)) % 32;
    d.extend(std::iter::repeat(0u8).take(pad));
}

/// The 32-byte digest the OWNER signs to authorize an `announce`:
/// `keccak256(topic || ephemeral || pubkey)` — `abi.encodePacked(bytes32,
/// address, bytes)` on-chain. MUST match `SignalingFacet.announce`'s digest
/// byte-for-byte (topic[32] ‖ ephemeral_addr[20] ‖ raw pubkey).
pub fn announce_digest(topic: &[u8; 32], ephemeral: &[u8; 20], pubkey: &[u8]) -> [u8; 32] {
    let mut pre = Vec::with_capacity(32 + 20 + pubkey.len());
    pre.extend_from_slice(topic);
    pre.extend_from_slice(ephemeral);
    pre.extend_from_slice(pubkey);
    keccak32(&pre)
}

fn encode_announce(
    topic: &[u8; 32],
    owner: &[u8; 20],
    ephemeral: &[u8; 20],
    pubkey: &[u8],
    sig: &[u8; 65],
) -> Vec<u8> {
    // announce(bytes32 topic, address owner, address ephemeral, bytes pubkey,
    //          bytes sig). Head: topic, owner, ephemeral, off(pubkey), off(sig)
    // = 5 words. Two trailing dynamic `bytes` (pubkey then sig).
    let mut d = selector("announce(bytes32,address,address,bytes,bytes)").to_vec();
    d.extend_from_slice(topic);
    d.extend_from_slice(&address_word(owner));
    d.extend_from_slice(&address_word(ephemeral));
    // 5 head words = 0xa0 bytes before the first dynamic payload.
    d.extend_from_slice(&u256_be(0xa0)); // offset to `pubkey`
    // pubkey tail = len word + padded data; sig follows it.
    let pubkey_tail = 32 + ((pubkey.len() + 31) / 32) * 32;
    d.extend_from_slice(&u256_be((0xa0 + pubkey_tail) as u128)); // offset to `sig`
    push_abi_bytes(&mut d, pubkey);
    push_abi_bytes(&mut d, sig);
    d
}

/// Announce `ephemeral` + `pubkey` under `topic` (sponsored; tx caller =
/// `sender`/master). `owner` is the seed-holder whose key authorizes the
/// announcement; the digest `keccak256(topic||ephemeral||pubkey)` is signed
/// with `owner_key` (the same seed key — `sender` and `owner_key` are the same
/// wallet on the owner's device). The facet recovers the sig vs `owner` and
/// requires `topic == devices_topic(owner)`, so an attacker without the seed
/// can't populate the roster.
#[allow(clippy::too_many_arguments)]
pub async fn announce_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    owner_key: &SigningKey,
    owner: &[u8; 20],
    topic: &[u8; 32],
    ephemeral: &[u8; 20],
    pubkey: &[u8],
    fee_token: &str,
) -> Result<String, String> {
    let digest = announce_digest(topic, ephemeral, pubkey);
    let sig = crate::wallet::sign_hash(owner_key, &digest); // low-s r‖s‖v, v∈{27,28}
    let call = crate::tempo_tx::TempoCall {
        to: parse_eth_address(REGISTRY_ADDRESS)?,
        value_wei: 0,
        input: encode_announce(topic, owner, ephemeral, pubkey, &sig),
    };
    let gas = 1_200_000u128 + (pubkey.len() as u128) * 9_000;
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, gas).await
}

fn encode_post_signal(to: &[u8; 20], blob: &[u8]) -> Vec<u8> {
    let mut d = selector("postSignal(address,bytes)").to_vec();
    d.extend_from_slice(&address_word(to));
    d.extend_from_slice(&u256_be(0x40)); // offset to `blob` (2 head words in)
    push_abi_bytes(&mut d, blob);
    d
}

/// Post a signaling blob (an SDP offer/answer/ICE bundle, sealed to `to`) into
/// `to`'s inbox (sponsored).
pub async fn post_signal_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    to: &[u8; 20],
    blob: &[u8],
    fee_token: &str,
) -> Result<String, String> {
    let call = crate::tempo_tx::TempoCall {
        to: parse_eth_address(REGISTRY_ADDRESS)?,
        value_wei: 0,
        input: encode_post_signal(to, blob),
    };
    let gas = 1_200_000u128 + (blob.len() as u128) * 9_000;
    submit_tempo_sponsored(sender, fee_payer, vec![call], fee_token, gas).await
}

/// One discovered/received entry. `peersOf` → (ephemeral, ts, pubkey);
/// `inboxOf` → (from, ts, blob).
pub type AddrTsBytes = (String, u64, Vec<u8>);

/// Decode an ABI `(address, uint64, bytes)[]` return — the shared shape of
/// `Presence[]` (peersOf) and `Signal[]` (inboxOf). Bounds-checked: a malformed
/// word stops decoding rather than panicking.
fn decode_addr_ts_bytes_array(result_hex: &str) -> Vec<AddrTsBytes> {
    let raw = match hex_to_bytes(result_hex) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    // `read_usize` reads the low 8 bytes of a 32-byte word — so any offset or
    // length is an attacker-controlled value up to u64::MAX. Every derived index
    // below uses checked arithmetic so a hostile word stops the decode (returns
    // what was parsed so far) instead of overflowing (panic in debug / wraparound
    // garbage in release) or slicing out of bounds.
    let read_usize = |off: usize| -> Option<usize> {
        let end = off.checked_add(32)?;
        let w = raw.get(off..end)?;
        Some(u64::from_be_bytes(w[24..32].try_into().ok()?) as usize)
    };
    let mut out = Vec::new();
    let arr_off = match read_usize(0) {
        Some(o) => o,
        None => return out,
    };
    let len = match read_usize(arr_off) {
        Some(l) => l,
        None => return out,
    };
    let heads = match arr_off.checked_add(32) {
        Some(h) => h, // element offsets are relative to here
        None => return out,
    };
    for i in 0..len {
        // head slot for element i = heads + i*32
        let head_slot = match i.checked_mul(32).and_then(|o| heads.checked_add(o)) {
            Some(s) => s,
            None => break,
        };
        let elem = match read_usize(head_slot) {
            Some(rel) => match heads.checked_add(rel) {
                Some(e) => e,
                None => break,
            },
            None => break,
        };
        let addr = match elem
            .checked_add(12)
            .zip(elem.checked_add(32))
            .and_then(|(a, b)| raw.get(a..b))
        {
            Some(a) => format!("0x{}", bytes_to_hex(a)),
            None => break,
        };
        let ts = match elem
            .checked_add(56)
            .zip(elem.checked_add(64))
            .and_then(|(a, b)| raw.get(a..b))
        {
            Some(t) => u64::from_be_bytes(t.try_into().unwrap_or_default()),
            None => break,
        };
        let boff = match elem.checked_add(64).and_then(read_usize) {
            // bytes offset is relative to the element
            Some(rel) => match elem.checked_add(rel) {
                Some(b) => b,
                None => break,
            },
            None => break,
        };
        let blen = match read_usize(boff) {
            Some(l) => l,
            None => break,
        };
        let bytes = boff
            .checked_add(32)
            .and_then(|start| start.checked_add(blen).map(|end| (start, end)))
            .and_then(|(start, end)| raw.get(start..end))
            .map(|s| s.to_vec())
            .unwrap_or_default();
        out.push((addr, ts, bytes));
    }
    out
}

/// The ephemeral peers announced under `topic` (peersOf). Callers filter stale
/// entries by the `ts` field.
pub async fn peers_of(topic: &[u8; 32]) -> Result<Vec<AddrTsBytes>, String> {
    let mut data = selector("peersOf(bytes32)").to_vec();
    data.extend_from_slice(topic);
    let res = eth_call(REGISTRY_ADDRESS, &format!("0x{}", bytes_to_hex(&data))).await?;
    Ok(decode_addr_ts_bytes_array(&res))
}

/// `peer`'s signaling inbox from `from_index` onward (inboxOf). The caller
/// tracks its own cursor.
pub async fn inbox_of(peer: &[u8; 20], from_index: u64) -> Result<Vec<AddrTsBytes>, String> {
    let mut data = selector("inboxOf(address,uint256)").to_vec();
    data.extend_from_slice(&address_word(peer));
    data.extend_from_slice(&u256_be(from_index as u128));
    let res = eth_call(REGISTRY_ADDRESS, &format!("0x{}", bytes_to_hex(&data))).await?;
    Ok(decode_addr_ts_bytes_array(&res))
}

/// `peer`'s inbox length (a cheap cursor poll).
pub async fn inbox_length(peer: &[u8; 20]) -> Result<u64, String> {
    let mut data = selector("inboxLength(address)").to_vec();
    data.extend_from_slice(&address_word(peer));
    let res = eth_call(REGISTRY_ADDRESS, &format!("0x{}", bytes_to_hex(&data))).await?;
    let raw = hex_to_bytes(&res)?;
    if raw.len() < 32 {
        return Ok(0);
    }
    Ok(u64::from_be_bytes(raw[24..32].try_into().map_err(|_| "bad len")?))
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
    match rpc("eth_sendRawTransaction", serde_json::json!([raw_hex])).await {
        Ok(hash) => Ok(hash),
        Err(err) => {
            // "already known" / "ALREADY_EXISTS" / "nonce too low" all
            // mean a previous submit of the same signed bytes (or
            // same-nonce sibling) is already in the mempool. Compute
            // the tx hash locally and let the caller's receipt poll
            // pick it up. Avoids spurious failures when the user
            // double-clicks `create` or retries after a UI hiccup.
            let lower = err.to_lowercase();
            let is_duplicate = lower.contains("already known")
                || lower.contains("already exists")
                || lower.contains("nonce too low");
            if is_duplicate {
                let bytes = hex_to_bytes(raw_hex)?;
                let mut hasher = Keccak256::new();
                hasher.update(&bytes);
                let digest = hasher.finalize();
                Ok(format!("0x{}", bytes_to_hex(&digest)))
            } else {
                Err(err)
            }
        }
    }
}

/// Map a 4-byte custom-error / `Error(string)` selector (the first 4 bytes of
/// revert data) to a friendly, actionable message. PURE + network-free — the
/// core of revert decoding, unit-tested in isolation. Covers the facets a CLI
/// user actually hits on a sponsored write: ScheduleFacet
/// (`schedule`/`unschedule`), InviteFacet (`invite create/accept/reclaim`), and
/// the cost/escrow facets the writes pull `$LH` through. Selectors are computed
/// from the EXACT `error Name(...)` signatures in the facet sources, so a
/// rename there must be mirrored here (the unit test pins the bytes).
///
/// `None` for an unrecognised selector so the caller can fall back to the
/// generic hint — never a misleading guess.
fn decode_known_revert(selector_bytes: [u8; 4]) -> Option<&'static str> {
    // (error signature, friendly message). The signature is keccak'd to its
    // 4-byte selector exactly as Solidity does (same `selector()` the encoders
    // use), so this list is the source of truth, not hand-copied hex.
    const KNOWN: &[(&str, &str)] = &[
        // --- ScheduleFacet (schedule / unschedule / pause / resume / topup) ---
        ("NotDue()", "this job isn't due yet — the scheduler only fires on the interval. Check `localharness jobs` for its next run."),
        ("StaleNextRun()", "this run was already fired by the scheduler — nothing to do (the on-chain clock already advanced)."),
        ("SpendExceedsBudget()", "the run would spend more $LH than the job's remaining budget — top it up or it will be marked exhausted."),
        ("NotScheduler()", "only the scheduler worker can record a run — this isn't a user action."),
        ("NotJobOwner()", "you don't own this job — only its scheduler can cancel/pause/top it up. Check `localharness jobs` under the right `--as` identity."),
        ("UnknownJob()", "no job with that id — list yours with `localharness jobs` (the id is the `#N`)."),
        ("JobNotActive()", "the job is already cancelled or exhausted — there's nothing to cancel. See `localharness jobs`."),
        ("JobNotPaused()", "the job isn't paused, so it can't be resumed."),
        ("UnregisteredTarget()", "the target isn't a registered agent — run `localharness whoami <target>` to confirm it exists first."),
        ("ZeroInterval()", "the interval is below the 60s minimum the facet allows — use `--every 60s` or more."),
        ("ZeroRuns()", "max-runs must be at least 1 — drop `--runs 0`."),
        // --- InviteFacet (invite create / accept / reclaim) ---
        ("CodeTaken()", "that invite code already exists on-chain — generate a fresh one (`invite create` makes a new code each time)."),
        ("BadTtl()", "the invite TTL is outside the allowed 1h..90d window — use e.g. `--ttl 7d`."),
        ("EscrowCapExceeded()", "this would push your locked invite escrow past the per-funder cap — reclaim an expired invite (`invite reclaim <code>`) or use a smaller amount."),
        ("UnknownInvite()", "no invite matches that code — double-check you copied the full code, including the `inv-` prefix."),
        ("NotOpen()", "this invite was already accepted or reclaimed — it's spent."),
        ("Expired()", "this invite has expired — it can no longer be accepted, only reclaimed by its funder (`invite reclaim <code>`)."),
        ("NotYetExpired()", "this invite hasn't expired yet — reclaim only works AFTER the TTL elapses. Until then it can still be accepted."),
        // --- Shared (both facets + the cost/escrow path) ---
        ("ZeroBudget()", "the budget/amount must be greater than 0."),
        ("ZeroAmount()", "the amount must be greater than 0."),
        ("NotConfigured()", "the on-chain credits token isn't configured — this is a platform-side misconfiguration, not your input. Report it via `localharness feedback`."),
        // --- Generic ERC-20 transferFrom failure (escrow pull) ---
        // The facets `require(transferFrom(...))` with these reason strings; if
        // the require trips it surfaces as Error(string). The selector branch
        // below decodes the actual string, but map the bare selector too.
        ("Error(string)", "the on-chain call reverted with a reason string (decoded above when available)."),
    ];
    for (sig, msg) in KNOWN {
        if selector(sig) == selector_bytes {
            return Some(msg);
        }
    }
    None
}

/// Turn raw revert return-data into a human message. Recognises:
///   - the standard `Error(string)` envelope (`0x08c379a0` + ABI string) — the
///     `require("...")` reason, e.g. "schedule: escrow failed" / "ERC20:
///     transfer amount exceeds balance" (an under-funded escrow);
///   - a known custom-error selector via `decode_known_revert`.
/// Returns `None` for empty/unrecognised data so the caller keeps the bare hash
/// plus a generic hint. PURE — unit-tested.
fn decode_revert_data(data: &[u8]) -> Option<String> {
    if data.len() < 4 {
        return None;
    }
    let sel: [u8; 4] = [data[0], data[1], data[2], data[3]];
    // Standard Error(string): 0x08c379a0 || abi.encode(string). Decode the
    // string and pass it through verbatim — `require` reasons are already
    // human-readable (and often the most actionable: an escrow-pull failure
    // means "you don't have enough $LH / haven't approved the diamond").
    if sel == [0x08, 0xc3, 0x79, 0xa0] {
        let hex = format!("0x{}", bytes_to_hex(&data[4..]));
        if let Some(reason) = decode_string(&hex) {
            let reason = reason.trim();
            if !reason.is_empty() {
                let lower = reason.to_ascii_lowercase();
                // An ERC-20 balance/allowance failure on an escrow pull is the
                // single most common cause — say what to DO about it.
                if lower.contains("balance") || lower.contains("allowance") || lower.contains("escrow") {
                    return Some(format!(
                        "{reason} — you likely don't have enough $LH for the escrow. \
                         Fund it (`localharness redeem <code>` or have another agent \
                         `send` you $LH), then retry."
                    ));
                }
                return Some(reason.to_string());
            }
        }
    }
    // Panic(uint256) (0x4e487b71) → arithmetic/assert; rare here, generic.
    if sel == [0x4e, 0x48, 0x7b, 0x71] {
        return Some("the contract hit an internal assertion (Panic) — this is a platform bug, not your input; please `localharness feedback` it.".to_string());
    }
    decode_known_revert(sel).map(|m| m.to_string())
}

/// Best-effort fetch + decode of WHY a sponsored tx reverted: re-run the same
/// top-level call via `eth_call` at the block it failed in, capture the revert
/// return-data from the node's error `data` field, and decode it
/// (`decode_revert_data`). The replay is read-only (no new tx, no gas, no
/// `$LH`). Returns `None` on any failure to obtain a reason — the caller still
/// has the hash + a generic hint. Never errors out the original flow.
async fn fetch_revert_reason(tx_hash: &str) -> Option<String> {
    // 1. Pull the original tx so we can replay its call shape.
    let tx = rpc_value("eth_getTransactionByHash", serde_json::json!([tx_hash]))
        .await
        .ok()?;
    let tx = tx.as_object()?;
    let to = tx.get("to")?.as_str()?;
    let from = tx.get("from")?.as_str()?;
    let input = tx.get("input").and_then(|v| v.as_str()).unwrap_or("0x");
    // Replay AT the failing block (state just before is closest to reproduce).
    let block = tx.get("blockNumber").and_then(|v| v.as_str()).unwrap_or("latest");

    // 2. eth_call the same call — a reverting call returns the revert data in
    //    the JSON-RPC error's `data` field. Capture it explicitly (the shared
    //    `rpc_value` discards `error.data`, so do a raw call here).
    let body = RpcRequest {
        jsonrpc: "2.0",
        id: 1,
        method: "eth_call",
        params: serde_json::json!([{ "from": from, "to": to, "data": input }, block]),
    };
    let client = reqwest::Client::new();
    let resp = client.post(RPC_URL).json(&body).send().await.ok()?;
    let json: serde_json::Value = resp.json().await.ok()?;

    // The revert payload can live in error.data (string or {message,data}).
    let err = json.get("error")?;
    let data_hex = err
        .get("data")
        .and_then(|d| {
            d.as_str()
                .map(|s| s.to_string())
                .or_else(|| d.get("data").and_then(|x| x.as_str()).map(|s| s.to_string()))
        })
        .filter(|s| s.len() > 2 && s.starts_with("0x"));
    if let Some(hex) = data_hex {
        if let Ok(bytes) = hex_to_bytes(&hex) {
            if let Some(reason) = decode_revert_data(&bytes) {
                return Some(reason);
            }
        }
    }
    // No structured data — some nodes embed the reason in error.message.
    err.get("message").and_then(|m| m.as_str()).and_then(|m| {
        let m = m.trim();
        // Only surface a message that actually says something beyond "reverted".
        if m.is_empty() || m.eq_ignore_ascii_case("execution reverted") {
            None
        } else {
            Some(m.to_string())
        }
    })
}

/// The catch-all line appended to a bare revert so the user is never left
/// staring at only a hash. Lists the common, user-fixable causes.
const GENERIC_REVERT_HINT: &str = "the transaction reverted on-chain. Common causes: \
    not enough $LH for the escrow/cost (fund with `localharness redeem <code>`), \
    you don't own the name/job you're acting on, a duplicate/expired/already-spent \
    invite, or a not-yet-due job. Run `localharness whoami <name>` / `jobs` to check state.";

/// Poll `eth_getTransactionReceipt` until the receipt resolves. Errors
/// after ~30 seconds — Tempo Moderato blocks are ~1s so 30 attempts
/// is more than enough headroom. On a `0x0` (revert) status, best-effort
/// fetch + decode the revert REASON (so the user sees WHY, not just a hash)
/// and always append the generic hint.
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
                // Decode WHY it reverted (best-effort; read-only replay).
                let reason = fetch_revert_reason(tx_hash).await;
                return Err(match reason {
                    Some(r) => format!("tx reverted: {r}\n  {GENERIC_REVERT_HINT}\n  tx: {tx_hash}"),
                    None => format!("tx reverted — {GENERIC_REVERT_HINT}\n  tx: {tx_hash}"),
                });
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
pub async fn sleep_ms(ms: u32) {
    tokio::time::sleep(std::time::Duration::from_millis(ms as u64)).await;
}

#[cfg(target_arch = "wasm32")]
pub async fn sleep_ms(ms: u32) {
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
    fn rank_agent_matches_filters_and_ranks() {
        let agents = vec![
            ("alice".to_string(), "A friendly chatbot".to_string()),
            ("solidity-bob".to_string(), "general assistant".to_string()),
            (
                "carol".to_string(),
                "An expert SOLIDITY auditor + security reviewer".to_string(),
            ),
            ("dave".to_string(), "writes haikus".to_string()),
        ];
        // "solidity" hits a NAME (bob) and a PERSONA (carol, case-insensitive);
        // the name hit ranks first.
        let hits = rank_agent_matches(&agents, "solidity");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0, "solidity-bob");
        assert_eq!(hits[1].0, "carol");
        // no match → empty
        assert!(rank_agent_matches(&agents, "nonexistent").is_empty());
        // empty / whitespace query returns all, order preserved
        assert_eq!(rank_agent_matches(&agents, "").len(), 4);
        assert_eq!(rank_agent_matches(&agents, "   ").len(), 4);
    }

    #[test]
    fn decode_presence_signal_array() {
        // Hand-crafted ABI `(address, uint64, bytes)[]` with one element:
        // (0x11..11, ts=5, bytes=[0xAA, 0xBB]) — the Presence/Signal shape that
        // peersOf/inboxOf return. Verifies the nested-offset decode.
        let hex = String::from("0x")
            + "0000000000000000000000000000000000000000000000000000000000000020" // array offset
            + "0000000000000000000000000000000000000000000000000000000000000001" // len = 1
            + "0000000000000000000000000000000000000000000000000000000000000020" // head[0] offset
            + "0000000000000000000000001111111111111111111111111111111111111111" // address
            + "0000000000000000000000000000000000000000000000000000000000000005" // ts = 5
            + "0000000000000000000000000000000000000000000000000000000000000060" // bytes offset
            + "0000000000000000000000000000000000000000000000000000000000000002" // bytes len = 2
            + "aabb000000000000000000000000000000000000000000000000000000000000"; // bytes data
        let out = decode_addr_ts_bytes_array(&hex);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "0x1111111111111111111111111111111111111111");
        assert_eq!(out[0].1, 5);
        assert_eq!(out[0].2, vec![0xAA, 0xBB]);
        // An empty array decodes to nothing (no panic).
        let empty = String::from("0x")
            + "0000000000000000000000000000000000000000000000000000000000000020"
            + "0000000000000000000000000000000000000000000000000000000000000000";
        assert!(decode_addr_ts_bytes_array(&empty).is_empty());
    }

    #[test]
    fn selector_matches_known_value() {
        // keccak256("idOfName(string)") = 0x127c388a...
        // Verified independently: `cast sig "idOfName(string)"`.
        let sel = selector("idOfName(string)");
        let hex: String = sel.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "127c388a");
    }

    #[test]
    fn decode_known_revert_maps_facet_errors() {
        // The decoder must recognise the exact custom-error selectors the
        // ScheduleFacet + InviteFacet emit — keyed off their source signatures.
        // A few representative ones, each with an actionable message.
        let cases = [
            ("NotDue()", "due"),
            ("UnknownJob()", "no job"),
            ("NotJobOwner()", "don't own"),
            ("UnregisteredTarget()", "registered agent"),
            ("CodeTaken()", "already exists"),
            ("NotYetExpired()", "hasn't expired"),
            ("Expired()", "expired"),
            ("NotOpen()", "already accepted or reclaimed"),
            ("BadTtl()", "1h..90d"),
            ("EscrowCapExceeded()", "escrow"),
            ("UnknownInvite()", "no invite"),
        ];
        for (sig, needle) in cases {
            let sel = selector(sig);
            let msg = decode_known_revert(sel)
                .unwrap_or_else(|| panic!("no mapping for {sig}"));
            assert!(
                msg.to_lowercase().contains(needle),
                "message for {sig} ({msg:?}) should mention {needle:?}"
            );
        }
        // An unknown selector → None (caller falls back to the generic hint).
        assert_eq!(decode_known_revert([0xde, 0xad, 0xbe, 0xef]), None);
    }

    #[test]
    fn decode_known_revert_selector_bytes_are_keccak_of_signature() {
        // Pin the wire bytes so a facet rename (which changes the on-chain
        // selector) trips this test, forcing the map to be updated in lockstep.
        // keccak256("NotDue()")[..4] — verifiable with `cast sig "NotDue()"`.
        let not_due = selector("NotDue()");
        let hex: String = not_due.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "47a2375f");
        assert!(decode_known_revert(not_due).is_some());
    }

    #[test]
    fn decode_revert_data_decodes_error_string_envelope() {
        // Standard Error(string) = 0x08c379a0 || abi.encode("...").
        // Hand-encode `require(false, "schedule: escrow failed")`.
        let reason = b"schedule: escrow failed";
        let mut data = vec![0x08, 0xc3, 0x79, 0xa0];
        data.extend_from_slice(&u256_be(0x20)); // offset to string
        data.extend_from_slice(&u256_be(reason.len() as u128)); // length
        let mut padded = reason.to_vec();
        padded.resize(padded.len().div_ceil(32) * 32, 0);
        data.extend_from_slice(&padded);

        let out = decode_revert_data(&data).expect("decodes Error(string)");
        assert!(out.contains("schedule: escrow failed"), "got {out:?}");

        // An ERC-20 balance failure on an escrow pull → actionable funding hint.
        let bal = b"ERC20: transfer amount exceeds balance";
        let mut d2 = vec![0x08, 0xc3, 0x79, 0xa0];
        d2.extend_from_slice(&u256_be(0x20));
        d2.extend_from_slice(&u256_be(bal.len() as u128));
        let mut p2 = bal.to_vec();
        p2.resize(p2.len().div_ceil(32) * 32, 0);
        d2.extend_from_slice(&p2);
        let out2 = decode_revert_data(&d2).expect("decodes balance error");
        assert!(out2.to_lowercase().contains("$lh"), "should suggest funding: {out2:?}");
    }

    #[test]
    fn decode_revert_data_maps_custom_error_and_handles_empty() {
        // A bare custom-error selector (no args) → its friendly message.
        let sel = selector("NotYetExpired()");
        let out = decode_revert_data(&sel).expect("maps custom error");
        assert!(out.to_lowercase().contains("hasn't expired"), "got {out:?}");
        // Empty / too-short data → None (caller keeps the hash + generic hint).
        assert_eq!(decode_revert_data(&[]), None);
        assert_eq!(decode_revert_data(&[0x01, 0x02]), None);
        // Unknown selector → None.
        assert_eq!(decode_revert_data(&[0xde, 0xad, 0xbe, 0xef]), None);
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
    fn invite_code_hash_matches_keccak256_of_code_bytes() {
        // The on-chain `acceptInvite(string)` recomputes keccak256(bytes(code));
        // our `invite_code_hash` MUST produce the same 32 bytes. Known vector
        // for the empty string: keccak256("") = c5d2460186f7...
        let h_empty = invite_code_hash("");
        let hex_empty: String = h_empty.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex_empty,
            "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        );
        // It IS exactly keccak_key(code.as_bytes()) — the same primitive
        // RedeemFacet's code hashing uses (so an invite code and a redeem code
        // hash identically), and matches `cast keccak "<code>"`.
        let code = "inv-100-A8kZqM2pQr";
        assert_eq!(invite_code_hash(code), keccak_key(code.as_bytes()));
        // Distinct codes hash distinctly; the hash is deterministic.
        assert_ne!(invite_code_hash("inv-10-aaaa"), invite_code_hash("inv-10-aaab"));
        assert_eq!(invite_code_hash(code), invite_code_hash(code));
    }

    #[test]
    fn encode_create_invite_layout() {
        let code_hash = invite_code_hash("inv-100-deadbeef01");
        let amount: u128 = 100 * 1_000_000_000_000_000_000; // 100 $LH in wei
        let ttl: u64 = 7 * 24 * 3600; // 7d
        let cd = encode_create_invite(&code_hash, amount, ttl);
        // selector(4) + 3 static head words = 100 bytes, no dynamic tail.
        assert_eq!(cd.len(), 4 + 3 * 32);
        // Selector pins the EXACT ABI signature the facet exposes.
        assert_eq!(&cd[..4], &selector("createInvite(bytes32,uint256,uint64)"));
        // Word 0 is the raw codeHash (bytes32 is NOT right-aligned — it occupies
        // the whole word as-is).
        assert_eq!(&cd[4..36], &code_hash[..]);
        // Word 1 = amount, right-aligned (low 16 bytes carry the u128).
        assert_eq!(&cd[36..68], &u256_be(amount)[..]);
        // Word 2 = ttlSeconds, right-aligned in its word.
        assert_eq!(&cd[68..100], &u256_be(ttl as u128)[..]);
    }

    #[test]
    fn encode_accept_invite_dynamic_string_layout() {
        let code = "inv-1000-Qm2pZ8kXaa"; // 19 bytes -> 1 padded word
        let cd = encode_accept_invite(code);
        assert_eq!(&cd[..4], &selector("acceptInvite(string)"));
        // Head word 0 = offset 0x20 to the string tail.
        assert_eq!(&cd[4..36], &u256_be(0x20)[..]);
        // Head word 1 = the string byte length.
        assert_eq!(&cd[36..68], &u256_be(code.len() as u128)[..]);
        // Tail = the bytes, then zero-padded to a 32-byte multiple.
        assert_eq!(&cd[68..68 + code.len()], code.as_bytes());
        let padded = code.len().div_ceil(32) * 32;
        assert_eq!(cd.len(), 4 + 32 + 32 + padded);
        // The padding bytes are zero.
        assert!(cd[68 + code.len()..].iter().all(|&b| b == 0));
        // Same dynamic-string ABI shape as `encode_redeem` (offset/len/body),
        // so the facet decodes it identically to RedeemFacet's `redeem(string)`.
        assert_eq!(&cd[4..36], &encode_redeem(code)[4..36]);
        assert_eq!(&cd[36..], &encode_redeem(code)[36..]);
    }

    #[test]
    fn encode_reclaim_invite_layout() {
        let code_hash = invite_code_hash("inv-10-cafef00d11");
        let cd = encode_reclaim_invite(&code_hash);
        assert_eq!(cd.len(), 4 + 32);
        assert_eq!(&cd[..4], &selector("reclaimInvite(bytes32)"));
        assert_eq!(&cd[4..36], &code_hash[..]);
    }

    #[test]
    fn devices_topic_preimage_is_label_then_raw_address() {
        // MUST equal keccak256("localharness.devices" || owner_20bytes), the
        // SAME preimage SignalingFacet recomputes on-chain as
        // keccak256(abi.encodePacked("localharness.devices", owner)). Any drift
        // here silently breaks the owner-gated announce (topic != devicesTopic).
        let owner = "0x1111111111111111111111111111111111111111";
        let topic = devices_topic(owner);
        let mut pre = b"localharness.devices".to_vec();
        pre.extend_from_slice(&parse_eth_address(owner).unwrap());
        assert_eq!(topic, keccak_key(&pre));
        // The label is 20 ASCII bytes; the address is appended raw (20 bytes),
        // so the preimage is exactly 40 bytes.
        assert_eq!(pre.len(), 40);
    }

    #[test]
    fn announce_digest_is_packed_topic_ephemeral_pubkey() {
        // The owner signs keccak256(topic || ephemeral || pubkey) — matching
        // SignalingFacet's keccak256(abi.encodePacked(bytes32, address, bytes)).
        // topic[32] ‖ ephemeral_addr[20] ‖ raw pubkey, NO padding.
        let topic = [0xABu8; 32];
        let eph = [0x22u8; 20];
        let pubkey = vec![0x02u8; 33];
        let mut pre = Vec::new();
        pre.extend_from_slice(&topic);
        pre.extend_from_slice(&eph);
        pre.extend_from_slice(&pubkey);
        assert_eq!(pre.len(), 32 + 20 + 33);
        assert_eq!(announce_digest(&topic, &eph, &pubkey), keccak32(&pre));
    }

    #[test]
    fn announce_digest_signature_recovers_to_owner() {
        // Full round-trip: the sig the driver attaches MUST recover to the
        // OWNER over the announce digest — exactly what the facet's `_recover`
        // checks (`recover(...) == owner`). Proves the driver's signing path
        // and the facet's recovery path agree.
        let w = crate::wallet::generate();
        let owner = crate::wallet::address(&w.signer); // [u8;20]
        let topic = [0x11u8; 32];
        let eph = [0x99u8; 20];
        let pubkey = vec![0x03u8; 33];
        let digest = announce_digest(&topic, &eph, &pubkey);
        let sig = crate::wallet::sign_hash(&w.signer, &digest); // low-s r‖s‖v
        let recovered = crate::wallet::recover_address(&sig, &digest)
            .expect("sig recovers");
        assert_eq!(recovered, owner, "announce sig recovers to the owner");
    }

    #[test]
    fn encode_announce_5arg_layout() {
        // New owner-signed signature; pin the selector + the two-trailing-bytes
        // ABI layout so it matches announce(bytes32,address,address,bytes,bytes).
        let topic = [0x11u8; 32];
        let owner = [0x22u8; 20];
        let eph = [0x33u8; 20];
        let pubkey = vec![0x02u8; 33]; // compressed-pubkey-shaped (1 padded word past len)
        let sig = [0x44u8; 65];
        let cd = encode_announce(&topic, &owner, &eph, &pubkey, &sig);

        assert_eq!(
            &cd[..4],
            &selector("announce(bytes32,address,address,bytes,bytes)")
        );
        // Head: topic, owner, ephemeral, off(pubkey)=0xa0, off(sig)=0xa0+96=0x100.
        assert_eq!(&cd[4..36], &topic[..]);
        assert_eq!(&cd[36..68], &address_word(&owner)[..]);
        assert_eq!(&cd[68..100], &address_word(&eph)[..]);
        assert_eq!(&cd[100..132], &u256_be(0xa0)[..]); // pubkey offset
        assert_eq!(&cd[132..164], &u256_be(0x100)[..]); // sig offset (0xa0 + 0x60)
        // pubkey tail at 4 + 0xa0 = 0xa4: len word (33) then 64 padded bytes.
        let pk_off = 4 + 0xa0;
        assert_eq!(&cd[pk_off..pk_off + 32], &u256_be(33)[..]);
        assert_eq!(&cd[pk_off + 32..pk_off + 32 + 33], &pubkey[..]);
        // sig tail at 4 + 0x100 = 0x104: len word (65) then 96 padded bytes.
        let sig_off = 4 + 0x100;
        assert_eq!(&cd[sig_off..sig_off + 32], &u256_be(65)[..]);
        assert_eq!(&cd[sig_off + 32..sig_off + 32 + 65], &sig[..]);
        // Total: 4 sel + 5 head words + (32+64) pubkey + (32+96) sig.
        assert_eq!(cd.len(), 4 + 5 * 32 + (32 + 64) + (32 + 96));
    }

    #[test]
    fn proxy_auth_token_format_and_recovers_signer() {
        let w = crate::wallet::generate();
        let token = proxy_auth_token(&w.signer, 1_700_000_000);
        let parts: Vec<&str> = token.split(':').collect();
        assert_eq!(parts.len(), 3, "token is address:timestamp:signature");

        let addr = format!("0x{}", bytes_to_hex(&crate::wallet::address(&w.signer)));
        assert_eq!(parts[0], addr, "first field is the 0x address");
        assert_eq!(parts[1], "1700000000", "second field is the unix timestamp");
        assert!(parts[2].starts_with("0x"));
        assert_eq!(parts[2].len(), 2 + 130, "signature is 65 bytes");

        // The signature must recover the signer over the exact message the
        // proxy reconstructs: "localharness-proxy:<addr>:<ts>".
        let msg = format!("localharness-proxy:{}:{}", parts[0], parts[1]);
        let digest = crate::wallet::personal_sign_digest(msg.as_bytes());
        let sig: [u8; 65] = hex_to_bytes(parts[2]).unwrap().try_into().unwrap();
        let recovered = crate::wallet::recover_address(&sig, &digest).unwrap();
        assert_eq!(format!("0x{}", bytes_to_hex(&recovered)), addr);
    }

    #[test]
    fn encode_submit_feedback_abi_layout() {
        let cd = encode_submit_feedback("hi");
        assert_eq!(&cd[0..4], &selector("submitFeedback(string)"));
        assert_eq!(&cd[4..36], &u256_be(0x20), "string offset");
        assert_eq!(&cd[36..68], &u256_be(2), "string length");
        assert_eq!(&cd[68..70], b"hi");
        assert_eq!(cd.len(), 4 + 64 + 32, "selector + offset + len + padded payload");
        // A 32-byte string takes exactly one more word (no over-pad).
        assert_eq!(encode_submit_feedback(&"x".repeat(32)).len(), 4 + 64 + 32);
        assert_eq!(encode_submit_feedback(&"x".repeat(33)).len(), 4 + 64 + 64);
    }

    #[test]
    fn encode_set_persona_abi_layout() {
        let cd = encode_set_persona(7, "hi");
        assert_eq!(&cd[0..4], &selector("setMetadata(uint256,bytes32,bytes)"));
        assert_eq!(&cd[4..36], &u256_be(7));
        assert_eq!(&cd[36..68], &keccak_key(PERSONA_LABEL));
        assert_eq!(&cd[68..100], &u256_be(0x60), "bytes offset");
        assert_eq!(&cd[100..132], &u256_be(2), "payload length");
        assert_eq!(&cd[132..134], b"hi");
        assert_eq!(
            cd.len(),
            4 + 96 + 32 + 32,
            "selector + 3 words + len + padded payload"
        );
    }

    #[test]
    fn encode_set_capability_commits_to_hash_not_payload() {
        let payload = b"price=10;payee=0xabc;service=qa";
        let cd = encode_set_capability(7, payload);
        // setMetadata(tokenId, key, bytes) where bytes = keccak256(payload) (32).
        assert_eq!(&cd[0..4], &selector("setMetadata(uint256,bytes32,bytes)"));
        assert_eq!(&cd[4..36], &u256_be(7));
        assert_eq!(&cd[36..68], &keccak_key(CAPABILITY_LABEL));
        assert_eq!(&cd[68..100], &u256_be(0x60), "bytes offset");
        assert_eq!(&cd[100..132], &u256_be(32), "commitment is 32 bytes");
        // The stored payload IS the hash — the raw descriptor never goes on-chain.
        assert_eq!(&cd[132..164], &keccak_key(payload));
        assert_ne!(&cd[132..164], &payload[..32.min(payload.len())]);
        assert_eq!(cd.len(), 4 + 96 + 32 + 32);
    }

    #[test]
    fn capability_key_distinct_from_other_metadata_keys() {
        let cap = keccak_key(CAPABILITY_LABEL);
        assert_ne!(cap, keccak_key(PERSONA_LABEL));
        assert_ne!(cap, keccak_key(PUBLIC_FACE_LABEL));
        assert_ne!(cap, keccak_key(PUBLIC_HTML_LABEL));
        assert_ne!(cap, app_metadata_key());
    }

    #[test]
    fn persona_key_distinct_from_other_metadata_keys() {
        // A copy-paste label collision would make persona overwrite the
        // app/html/public_face slots — assert the keys are all distinct.
        let persona = keccak_key(PERSONA_LABEL);
        assert_ne!(persona, keccak_key(PUBLIC_FACE_LABEL));
        assert_ne!(persona, keccak_key(PUBLIC_HTML_LABEL));
        assert_ne!(persona, app_metadata_key());
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

    // ─── ABI dynamic-decode edge cases (untrusted RPC hex must never panic) ──
    //
    // The decoders below read offsets/lengths out of attacker-controlled words
    // (the low 8 bytes → up to u64::MAX) and then slice the response. These tests
    // feed deliberately empty / truncated / malformed-offset / huge-length hex
    // and assert the decoder returns empty/None/Err WITHOUT panicking. The test
    // profile has overflow-checks ON, so an unchecked `64 + len` / `arr_off + 32`
    // would panic here — that's exactly the regression these pin down.

    // 64 hex chars per ABI word.
    const Z: &str = "0000000000000000000000000000000000000000000000000000000000000000";
    fn word_usize(v: u64) -> String {
        format!("{:064x}", v)
    }
    /// A 32-byte word whose LOW 8 bytes are u64::MAX (the largest value the
    /// low-8-bytes offset/length readers will extract → forces overflow if any
    /// add is unchecked).
    fn word_u64_max() -> String {
        format!("{:048x}{:016x}", 0u64, u64::MAX)
    }

    #[test]
    fn addr_ts_bytes_array_empty_and_short_inputs() {
        // Totally empty RPC result ("0x").
        assert!(decode_addr_ts_bytes_array("0x").is_empty());
        // Not even one word.
        assert!(decode_addr_ts_bytes_array("0x00").is_empty());
        // Odd-length / non-hex never panics (hex_to_bytes errors → empty).
        assert!(decode_addr_ts_bytes_array("0xabc").is_empty());
        assert!(decode_addr_ts_bytes_array("0xzz").is_empty());
        assert!(decode_addr_ts_bytes_array("nonsense").is_empty());
        // Array offset points past the buffer → empty, no panic.
        let off_oob = format!("0x{}", word_usize(0x40)); // offset 64, only 32 bytes present
        assert!(decode_addr_ts_bytes_array(&off_oob).is_empty());
    }

    #[test]
    fn addr_ts_bytes_array_hostile_offsets_dont_overflow() {
        // Array offset = u64::MAX. `arr_off + 32` must NOT overflow.
        let huge_off = format!("0x{}", word_u64_max());
        assert!(decode_addr_ts_bytes_array(&huge_off).is_empty());

        // Valid array offset (0x20) + length = u64::MAX. The per-element head
        // read must stop at the buffer end, not loop u64::MAX times or overflow
        // `heads + i*32`.
        let huge_len = format!("0x{}{}", word_usize(0x20), word_u64_max());
        assert!(decode_addr_ts_bytes_array(&huge_len).is_empty());

        // One element whose head-offset word is u64::MAX → `heads + rel` overflow.
        let bad_head = String::from("0x")
            + &word_usize(0x20) // array offset
            + &word_usize(1) // len = 1
            + &word_u64_max(); // head[0] = u64::MAX (relative element offset)
        assert!(decode_addr_ts_bytes_array(&bad_head).is_empty());

        // One element whose inner bytes-offset is u64::MAX → `elem + rel` overflow.
        let bad_bytes_off = String::from("0x")
            + &word_usize(0x20) // array offset
            + &word_usize(1) // len = 1
            + &word_usize(0x20) // head[0] → element starts right after heads
            + &word_usize(0x1111) // address word
            + &word_usize(7) // ts
            + &word_u64_max(); // bytes offset = u64::MAX
        assert!(decode_addr_ts_bytes_array(&bad_bytes_off).is_empty());
    }

    #[test]
    fn addr_ts_bytes_array_multi_element_decodes() {
        // Two elements: (0x11..,1,[0xAA]) and (0x22..,2,[0xBB,0xCC]).
        // Each element is a `(address,uint64,bytes)` tuple, encoded as 5 words:
        // [addr][ts][bytes-rel-offset(0x60)][bytes-len][bytes-data].
        let elem0 = String::from("")
            + "0000000000000000000000001111111111111111111111111111111111111111" // addr
            + &word_usize(1) // ts
            + &word_usize(0x60) // bytes offset (relative to element)
            + &word_usize(1) // bytes len
            + "aa00000000000000000000000000000000000000000000000000000000000000"; // data
        let elem1 = String::from("")
            + "0000000000000000000000002222222222222222222222222222222222222222"
            + &word_usize(2)
            + &word_usize(0x60)
            + &word_usize(2)
            + "bbcc000000000000000000000000000000000000000000000000000000000000";
        // elem0 is 5 words = 0xA0 bytes. heads = arr_off(0x20)+0x20 = 0x40.
        // head[0] rel = 0x40 (2 head words), head[1] rel = 0x40 + 0xA0 = 0xE0.
        let hex = String::from("0x")
            + &word_usize(0x20) // array offset
            + &word_usize(2) // len = 2
            + &word_usize(0x40) // head[0]
            + &word_usize(0xE0) // head[1]
            + &elem0
            + &elem1;
        let out = decode_addr_ts_bytes_array(&hex);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, "0x1111111111111111111111111111111111111111");
        assert_eq!(out[0].1, 1);
        assert_eq!(out[0].2, vec![0xAA]);
        assert_eq!(out[1].0, "0x2222222222222222222222222222222222222222");
        assert_eq!(out[1].1, 2);
        assert_eq!(out[1].2, vec![0xBB, 0xCC]);
    }

    #[test]
    fn metadata_bytes_edge_cases() {
        // Empty result → None.
        assert_eq!(decode_metadata_bytes("0x"), None);
        // Shorter than two words → None (not a panic).
        assert_eq!(decode_metadata_bytes(&format!("0x{Z}")), None);
        // Zero length → None.
        let zero_len = format!("0x{}{}", word_usize(0x20), Z);
        assert_eq!(decode_metadata_bytes(&zero_len), None);
        // Huge length (u64::MAX) → None, no overflow on `64 + len`.
        let huge = format!("0x{}{}", word_usize(0x20), word_u64_max());
        assert_eq!(decode_metadata_bytes(&huge), None);
        // Length present but payload truncated → None.
        let trunc = format!("0x{}{}", word_usize(0x20), word_usize(10)); // claims 10 bytes, has 0
        assert_eq!(decode_metadata_bytes(&trunc), None);
        // Well-formed 3-byte payload → Some.
        let ok = format!(
            "0x{}{}{}",
            word_usize(0x20),
            word_usize(3),
            "aabbcc0000000000000000000000000000000000000000000000000000000000"
        );
        assert_eq!(decode_metadata_bytes(&ok), Some(vec![0xAA, 0xBB, 0xCC]));
    }

    #[test]
    fn decode_string_edge_cases() {
        // Empty / short → None.
        assert_eq!(decode_string("0x"), None);
        assert_eq!(decode_string(&format!("0x{Z}")), None);
        // Huge length → None, no `64 + len` overflow.
        let huge = format!("0x{}{}", word_usize(0x20), word_u64_max());
        assert_eq!(decode_string(&huge), None);
        // Truncated body → None.
        let trunc = format!("0x{}{}", word_usize(0x20), word_usize(64));
        assert_eq!(decode_string(&trunc), None);
        // Valid "hi".
        let ok = format!(
            "0x{}{}{}",
            word_usize(0x20),
            word_usize(2),
            "6869000000000000000000000000000000000000000000000000000000000000"
        );
        assert_eq!(decode_string(&ok).as_deref(), Some("hi"));
    }

    #[test]
    fn decode_feedback_data_edge_cases() {
        // < 96 bytes → None. (FeedbackEntry has no PartialEq → use is_none.)
        assert!(decode_feedback_data(&[], "s".into()).is_none());
        assert!(decode_feedback_data(&[0u8; 95], "s".into()).is_none());
        // Huge length word (low 8 bytes = u64::MAX) → None, no `96 + len` overflow.
        let mut buf = vec![0u8; 96];
        buf[88..96].copy_from_slice(&u64::MAX.to_be_bytes());
        assert!(decode_feedback_data(&buf, "s".into()).is_none());
        // Well-formed: ts=9, text="ab".
        let body = String::from("")
            + &word_usize(9) // timestamp
            + &word_usize(0x40) // offset
            + &word_usize(2) // text len
            + "6162000000000000000000000000000000000000000000000000000000000000";
        let bytes = hex_to_bytes(&body).unwrap();
        let entry = decode_feedback_data(&bytes, "sender".into()).unwrap();
        assert_eq!(entry.timestamp, 9);
        assert_eq!(entry.text, "ab");
        assert_eq!(entry.sender, "sender");
    }

    #[test]
    fn hex_to_bytes_rejects_malformed_without_panic() {
        assert!(hex_to_bytes("0xabc").is_err()); // odd length
        assert!(hex_to_bytes("0xzz").is_err()); // non-hex
        assert!(hex_to_bytes("0x").unwrap().is_empty()); // empty is ok
        assert_eq!(hex_to_bytes("0xAaBb").unwrap(), vec![0xAA, 0xBB]); // case-insensitive
        assert_eq!(hex_to_bytes("deadbeef").unwrap(), vec![0xDE, 0xAD, 0xBE, 0xEF]); // no prefix
    }

    #[test]
    fn decode_address_edge_cases() {
        // Short / empty → None.
        assert_eq!(decode_address("0x"), None);
        assert_eq!(decode_address("0x00"), None);
        // All-zero word → None (zero address means "unset").
        assert_eq!(decode_address(&format!("0x{Z}")), None);
        // A real address in the low 20 bytes.
        let w = format!("0x{}", "0".repeat(24) + "1111111111111111111111111111111111111111");
        assert_eq!(
            decode_address(&w).as_deref(),
            Some("0x1111111111111111111111111111111111111111")
        );
    }

    #[test]
    fn decode_u256_as_u128_truncation_and_empty() {
        // Empty → 0.
        assert_eq!(decode_u256_as_u128("0x").unwrap(), 0);
        // Normal small value.
        assert_eq!(decode_u256_as_u128(&format!("0x{}", word_usize(42))).unwrap(), 42);
        // Exactly u128::MAX in the low 16 bytes.
        let max = format!("0x{}{}", "0".repeat(32), "f".repeat(32));
        assert_eq!(decode_u256_as_u128(&max).unwrap(), u128::MAX);
    }
}
