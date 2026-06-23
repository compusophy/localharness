use k256::ecdsa::SigningKey;

use super::*;

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
    let total = next_id().await?;
    if total <= 1 {
        return Ok(Vec::new());
    }
    let owner_lower = owner_hex.to_lowercase();

    // ONE batched POST: ownerOfId(1..total). nextId is one-past the highest id.
    let owner_calls: Vec<(&str, String)> = (1..total)
        .map(|id| (REGISTRY_ADDRESS(), call_uint("ownerOfId(uint256)", id)))
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
        .map(|&id| (REGISTRY_ADDRESS(), call_uint("nameOfId(uint256)", id)))
        .collect();
    let tba_calls: Vec<(&str, String)> = my_ids
        .iter()
        .map(|&id| (REGISTRY_ADDRESS(), call_uint("tokenBoundAccount(uint256)", id)))
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

pub(crate) async fn next_id() -> Result<u64, String> {
    let result_hex = read_view(selector("nextId()"), &[]).await?;
    decode_u256_as_u64(&result_hex)
}

/// Total registered subdomains. Token ids start at 1, so the count is
/// `nextId - 1`. Used by the admin Usage tab.
pub async fn subdomain_count() -> Result<u64, String> {
    Ok(next_id().await?.saturating_sub(1))
}

pub async fn name_of_id(id: u64) -> Result<String, String> {
    // ABI string return (offset + length + bytes) — same length-checked,
    // Err-on-short decode as the bounty `bytes`-as-UTF-8 reads.
    decode_bytes_string_call("nameOfId(uint256)", id, "nameOfId").await
}

/// `eth_call tokenBoundAccount(tokenId)` and return the ERC-6551
/// account address. None when the token isn't registered. The address
/// is deterministic — counterfactual even before deployment.
pub async fn tba_of_token_id(token_id: u64) -> Result<Option<String>, String> {
    let result_hex = match read_view(
        selector("tokenBoundAccount(uint256)"),
        &[u256_be(token_id as u128)],
    )
    .await
    {
        Ok(h) => h,
        Err(err) => {
            if err.contains("nonexistent token") || err.contains("registry unset") {
                return Ok(None);
            }
            return Err(err);
        }
    };
    Ok(decode_address(&result_hex))
}

/// `eth_call tokenBoundAccountByName(name)` and return the ERC-6551
/// account address. None when the name is unregistered. The address
/// is deterministic — it exists counterfactually even if the account
/// hasn't been deployed yet.
pub async fn tba_of_name(name: &str) -> Result<Option<String>, String> {
    let calldata = encode_string_call("tokenBoundAccountByName(string)", name);
    let result_hex = match eth_call(REGISTRY_ADDRESS(), &calldata).await {
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
    Ok(decode_address(&result_hex))
}

/// `eth_call ownerOfName(name)` and return the address as a
/// `0x`-prefixed lowercase hex string. `None` if the name has no
/// on-chain owner (returns the zero address).
pub async fn owner_of_name(name: &str) -> Result<Option<String>, String> {
    let calldata = encode_owner_of_name(name);
    let result_hex = eth_call(REGISTRY_ADDRESS(), &calldata).await?;
    // Address is the last 20 bytes of a 32-byte uint256 return.
    Ok(decode_address(&result_hex))
}

pub(crate) fn encode_owner_of_name(name: &str) -> String {
    encode_string_call("ownerOfName(string)", name)
}

/// Generic `fn(string)` calldata encoder — the UTF-8-string flavor of the
/// shared [`encode_dynamic_call_hex`].
pub(crate) fn encode_string_call(signature: &str, value: &str) -> String {
    encode_dynamic_call_hex(signature, value.as_bytes())
}

/// `eth_call idOfName(name)` and classify the result. Single round trip.
pub async fn check_name(name: &str) -> Result<Status, String> {
    let calldata = encode_id_of_name(name);
    let result_hex = eth_call(REGISTRY_ADDRESS(), &calldata).await?;
    let id = decode_u256_as_u64(&result_hex)?;
    Ok(if id == 0 {
        Status::Available
    } else {
        Status::Taken { agent_id: id }
    })
}

/// `eth_call idOfName(name)` → the token id (0 if unregistered).
pub async fn id_of_name(name: &str) -> Result<u64, String> {
    let calldata = encode_id_of_name(name);
    let result_hex = eth_call(REGISTRY_ADDRESS(), &calldata).await?;
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
    // ids newest-first.
    let ids: Vec<u64> = (start..=max_id).rev().collect();
    // ONE batched JSON-RPC request instead of up to `limit` SERIAL round-trips.
    // The old loop `await`ed `name_of_id` per id; at ~300ms/RTT on the public
    // RPC a 60-name directory took ~20s to paint. `nameOfId` returns the same
    // ABI-string layout `decode_metadata_bytes` already handles, so this mirrors
    // `personas_of` exactly — index-aligned results, dead/empty slots dropped.
    let sel = selector("nameOfId(uint256)");
    let calls: Vec<(&str, String)> = ids
        .iter()
        .map(|&id| (REGISTRY_ADDRESS(), encode_call_hex(sel, &[u256_be(id as u128)])))
        .collect();
    let results = eth_call_batch(&calls).await?;
    let out = ids
        .iter()
        .zip(results.iter())
        .filter_map(|(&id, r)| {
            r.as_ref()
                .ok()
                .and_then(|hex| decode_metadata_bytes(hex))
                .and_then(|b| String::from_utf8(b).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .map(|name| (id, name))
        })
        .collect();
    Ok(out)
}

/// Pure: filter + rank `(name, persona)` pairs by a query, case-insensitive.
/// Multi-keyword: the query is whitespace-split into tokens and an agent
/// matches if the WHOLE phrase or ANY token appears in its name or persona —
/// so one `discover` for "game tool puzzle" replaces a sequential scan per
/// keyword. Ranking: name hits above persona-only hits; within a tier, a
/// whole-phrase hit outranks token hits, then more matched tokens rank
/// higher, then input order (recency) breaks ties. Empty query returns all.
/// Mirrors the proxy's `rankAgentMatches` (api/mcp.ts) EXACTLY — keep the two
/// in lockstep. The matching core of [`discover_agents`] (and
/// `discover_bounties`' task ranking), split out for testing.
pub fn rank_agent_matches(agents: &[(String, String)], query: &str) -> Vec<(String, String)> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return agents.to_vec();
    }
    let tokens: Vec<&str> = q.split_whitespace().collect();
    let overlap = |text_lower: &str| tokens.iter().filter(|t| text_lower.contains(**t)).count();

    // (score, input order, pair) per tier; phrase hit = +100 over token hits.
    let mut name_hits: Vec<(usize, usize, (String, String))> = Vec::new();
    let mut persona_hits: Vec<(usize, usize, (String, String))> = Vec::new();
    for (order, (name, persona)) in agents.iter().enumerate() {
        let name_lower = name.to_lowercase();
        let persona_lower = persona.to_lowercase();
        let name_overlap = overlap(&name_lower);
        if name_lower.contains(&q) || name_overlap > 0 {
            let score = if name_lower.contains(&q) { 100 } else { 0 } + name_overlap;
            name_hits.push((score, order, (name.clone(), persona.clone())));
        } else {
            let persona_overlap = overlap(&persona_lower);
            if persona_lower.contains(&q) || persona_overlap > 0 {
                let score = if persona_lower.contains(&q) { 100 } else { 0 } + persona_overlap;
                persona_hits.push((score, order, (name.clone(), persona.clone())));
            }
        }
    }
    // Stable per tier: higher score first, input order (recency) as tiebreak.
    name_hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    persona_hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    name_hits
        .into_iter()
        .chain(persona_hits)
        .map(|(_, _, pair)| pair)
        .collect()
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

// --- Published app cartridge (OFF-CHAIN app store) -------------------
//
// A subdomain's app is a compiled wasm cartridge. It lives OFF-CHAIN now
// (GitHub, fetched by NAME via the proxy's `/api/app` serve route) — the
// blockchain keeps only OWNERSHIP/provenance (the name NFT + signature
// proof that authorizes a publish). On-chain `setMetadata` publishing cost
// ~$0.32–$2.80/cart and drained the gas sponsor; off-chain is free and
// uncapped, mirroring the feedback/telemetry model. The owner publishes via
// the CLI `publish` (→ `POST /api/publish`) or the studio; a visitor's device
// fetches the bytes from the store and runs them. See `proxy/api/{publish,app}.ts`
// and the off-chain-apps pivot.

/// Max compiled-cartridge bytes the app store accepts — the host::compose
/// per-child wasm budget (256 KB). Off-chain has no gas cap; this keeps any
/// published cartridge composable. Mirrors `proxy/api/publish.ts`'s server-side
/// cap; shared by the CLI + browser publish paths so there's one number.
pub const APP_STORE_MAX_WASM_BYTES: usize = 256 * 1024;

/// Base path of the off-chain app store's serve route. A published cartridge is
/// `GET {CREDIT_PROXY_URL}api/app?name=<name>` → raw `application/wasm` bytes.
fn app_store_url(name: &str) -> String {
    format!("{CREDIT_PROXY_URL}api/app?name={name}")
}

/// Publish a compiled cartridge (+ its source) to the OFF-CHAIN app store via the
/// proxy's `POST /api/publish`, authed by a personal-sign `token` (mint it with
/// [`proxy_auth_token`]). The proxy gates on the token's signer OWNING `name`
/// on-chain, then commits the bytes to GitHub. Cross-target (native CLI + the
/// browser studio / agent tools). No gas, no sponsor — the chain keeps only
/// ownership. `Ok(())` on success.
pub async fn publish_app_to_store(
    name: &str,
    token: &str,
    wasm: &[u8],
    source: &str,
) -> Result<(), String> {
    let url = format!("{CREDIT_PROXY_URL}api/publish");
    let body = serde_json::json!({
        "name": name,
        "wasm_hex": format!("0x{}", bytes_to_hex(wasm)),
        "source": source,
    });
    http_post_json_authed(&url, token, &body).await
}

/// Publish an HTML page as `name`'s public face to the OFF-CHAIN app store — the
/// HTML-face sibling of [`publish_app_to_store`] (`POST /api/publish` with an
/// `html` body, same personal-sign auth + on-chain ownership gate). The browser
/// reads it back via [`html_from_store`]. No gas.
pub async fn publish_html_to_store(name: &str, token: &str, html: &str) -> Result<(), String> {
    let url = format!("{CREDIT_PROXY_URL}api/publish");
    let body = serde_json::json!({ "name": name, "html": html });
    http_post_json_authed(&url, token, &body).await
}

/// Fetch a subdomain's published HTML page from the OFF-CHAIN app store by name
/// (`GET /api/app?name=<name>&kind=html`). `Ok(None)` = no page published.
pub async fn html_from_store(name: &str) -> Result<Option<Vec<u8>>, String> {
    let n = name.trim();
    if n.is_empty() {
        return Ok(None);
    }
    http_get_bytes(&format!("{CREDIT_PROXY_URL}api/app?name={n}&kind=html")).await
}

/// Fetch a subdomain's published cartridge from the OFF-CHAIN app store by name.
/// `Ok(None)` = no app published (a 404 — the visitor falls back to the
/// directory/html face). Works on native AND wasm (the browser load path).
pub async fn app_wasm_from_store(name: &str) -> Result<Option<Vec<u8>>, String> {
    let n = name.trim();
    if n.is_empty() {
        return Ok(None);
    }
    http_get_bytes(&app_store_url(n)).await
}

/// Read a subdomain's published app wasm. Resolves `token_id`→name then fetches
/// the off-chain store (kept for callers that only hold the id, e.g. the
/// host::compose tool); name-holding callers should use [`app_wasm_from_store`]
/// directly to skip the id→name round-trip.
pub async fn app_wasm_of(token_id: u64) -> Result<Option<Vec<u8>>, String> {
    let name = name_of_id(token_id).await?;
    app_wasm_from_store(&name).await
}

/// Storage key for the legacy on-chain app wasm: `keccak256("localharness.app.wasm")`.
/// Retained for back-compat reads of pre-off-chain publishes; new publishes go to
/// the app store (no on-chain bytes).
pub(crate) fn app_metadata_key() -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let digest = Keccak256::digest(b"localharness.app.wasm");
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Encode `setMetadata(tokenId, appKey, wasm)` calldata. LEGACY — the on-chain
/// publish path; kept for tooling/tests. Live publishing is off-chain now.
pub fn encode_set_app_wasm(token_id: u64, wasm: &[u8]) -> Vec<u8> {
    encode_set_metadata_bytes(token_id, app_metadata_key(), wasm)
}

/// Storage key for the seed-encrypted Gemini API key:
/// `keccak256("localharness.gemini_key.enc")`.
pub(crate) fn gemini_key_metadata_key() -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let digest = Keccak256::digest(b"localharness.gemini_key.enc");
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Read a subdomain's on-chain seed-encrypted Gemini key ciphertext, if
/// any. Same ABI-`bytes` decode as `app_wasm_of`.
pub async fn gemini_key_of(token_id: u64) -> Result<Option<Vec<u8>>, String> {
    metadata_bytes_of(token_id, gemini_key_metadata_key()).await
}

/// Encode `setMetadata(tokenId, geminiKeyKey, ciphertext)` calldata for a
/// sponsored on-chain key-sync tx.
pub fn encode_set_gemini_key(token_id: u64, ciphertext: &[u8]) -> Vec<u8> {
    encode_set_metadata_bytes(token_id, gemini_key_metadata_key(), ciphertext)
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

pub(crate) fn keccak_key(label: &[u8]) -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let digest = Keccak256::digest(label);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Read raw `bytes` metadata stored under `key` for `token_id`. `None`
/// when the slot is empty. Shared ABI-`bytes` decode (offset+len+payload).
pub(crate) async fn metadata_bytes_of(token_id: u64, key: [u8; 32]) -> Result<Option<Vec<u8>>, String> {
    let result_hex = read_view(
        selector("metadata(uint256,bytes32)"),
        &[u256_be(token_id as u128), key],
    )
    .await?;
    Ok(decode_abi_bytes(&result_hex))
}

/// Encode `setMetadata(tokenId, key, payload)` calldata for a sponsored tx.
pub(crate) fn encode_set_metadata_bytes(token_id: u64, key: [u8; 32], payload: &[u8]) -> Vec<u8> {
    encode_set_metadata(token_id, key, payload)
}

pub(crate) const PUBLIC_FACE_LABEL: &[u8] = b"localharness.public_face";
pub(crate) const PUBLIC_HTML_LABEL: &[u8] = b"localharness.public.html";

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

/// Read a subdomain's published public-face HTML. OFF-CHAIN now (the app store,
/// fetched by name) — resolves `token_id`→name then reads the store. Kept on the
/// `token_id` signature for its callers (resolve_public_face).
pub async fn public_html_of(token_id: u64) -> Result<Option<Vec<u8>>, String> {
    let name = name_of_id(token_id).await?;
    html_from_store(&name).await
}

/// Encode `setMetadata` for the published public-face HTML. LEGACY — the on-chain
/// HTML publish; retained for the TBA-owner on-chain fallback. Live HTML publishing
/// is off-chain ([`publish_html_to_store`]).
pub fn encode_set_public_html(token_id: u64, html: &[u8]) -> Vec<u8> {
    encode_set_metadata_bytes(token_id, keccak_key(PUBLIC_HTML_LABEL), html)
}

pub(crate) const PUSH_SUB_LABEL: &[u8] = b"localharness.push_sub";

/// Read a token's published Web Push subscription JSON
/// (`{endpoint, keys: {p256dh, auth}}`), if any. Written by the browser
/// app's "enable notifications" flow; consumed by the proxy's scheduler
/// worker to notify the owner when a scheduled job completes (tab closed).
pub async fn push_sub_of(token_id: u64) -> Result<Option<String>, String> {
    match metadata_bytes_of(token_id, keccak_key(PUSH_SUB_LABEL)).await? {
        Some(b) => Ok(String::from_utf8(b)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())),
        None => Ok(None),
    }
}

/// Encode `setMetadata` for a Web Push subscription JSON.
///
/// KNOWN TRADEOFF (v1): the payload is PLAINTEXT on-chain — a push endpoint
/// is a bearer capability URL (push payloads stay E2E-encrypted to the
/// browser via p256dh/auth, but anyone reading chain state can spam the
/// endpoint). Follow-up: ECIES-seal to a proxy-held key.
pub fn encode_set_push_sub(token_id: u64, sub_json: &[u8]) -> Vec<u8> {
    encode_set_metadata_bytes(token_id, keccak_key(PUSH_SUB_LABEL), sub_json)
}

pub(crate) const PERSONA_LABEL: &[u8] = b"localharness.persona";

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

pub(crate) const LESSONS_LABEL: &[u8] = b"localharness.lessons";

/// Read a subdomain's self-recorded lessons blob (plain text, one lesson per
/// line; see `crate::lessons`) — folded into the agent's system prompt on
/// every surface. `None` when unset.
pub async fn lessons_of(token_id: u64) -> Result<Option<String>, String> {
    match metadata_bytes_of(token_id, keccak_key(LESSONS_LABEL)).await? {
        Some(b) => Ok(String::from_utf8(b)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())),
        None => Ok(None),
    }
}

/// Encode `setMetadata` for a subdomain's lessons blob. Owner-gated, same
/// path as the published persona.
pub fn encode_set_lessons(token_id: u64, lessons: &str) -> Vec<u8> {
    encode_set_metadata_bytes(token_id, keccak_key(LESSONS_LABEL), lessons.as_bytes())
}

pub(crate) const SKILLS_LABEL: &[u8] = b"localharness.skills";

/// Read a subdomain's self-defined skills blob (a JSON array of
/// `{name, instructions}`; see `crate::skills`) — folded into the agent's
/// system prompt on every surface so it can invoke a skill by name. `None`
/// when unset.
pub async fn skills_of(token_id: u64) -> Result<Option<String>, String> {
    match metadata_bytes_of(token_id, keccak_key(SKILLS_LABEL)).await? {
        Some(b) => Ok(String::from_utf8(b)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())),
        None => Ok(None),
    }
}

/// Encode `setMetadata` for a subdomain's skills blob. Owner-gated, same path
/// as the published lessons.
pub fn encode_set_skills(token_id: u64, skills: &str) -> Vec<u8> {
    encode_set_metadata_bytes(token_id, keccak_key(SKILLS_LABEL), skills.as_bytes())
}

/// Read the personas for MANY tokens in ONE JSON-RPC batch POST (vs N
/// serial `persona_of` round-trips). Returns one entry per input id, in
/// input order: `Some(persona)` when set, `None` when unset / empty / the
/// per-call RPC failed (graceful degradation — a single bad slot never
/// fails the whole batch). Backs the public-landing agent portfolio cards.
pub async fn personas_of(token_ids: &[u64]) -> Vec<Option<String>> {
    if token_ids.is_empty() {
        return Vec::new();
    }
    let key = keccak_key(PERSONA_LABEL);
    let calls: Vec<(&str, String)> = token_ids
        .iter()
        .map(|&id| (REGISTRY_ADDRESS(), call_metadata(id, key)))
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
pub(crate) fn call_metadata(token_id: u64, key: [u8; 32]) -> String {
    encode_call_hex(
        selector("metadata(uint256,bytes32)"),
        &[u256_be(token_id as u128), key],
    )
}

/// Decode an ABI `bytes` return (offset + length + payload). `None` when
/// short / empty / truncated. Thin alias over the shared [`decode_abi_bytes`]
/// (kept for the batched-metadata call sites that name it).
pub(crate) fn decode_metadata_bytes(result_hex: &str) -> Option<Vec<u8>> {
    decode_abi_bytes(result_hex)
}

pub(crate) const CAPABILITY_LABEL: &[u8] = b"localharness.capability";

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

// `claim_name` (the legacy SELF-PAID EIP-155 register) and
// `request_faucet_funds` (the Tempo native-gas faucet it depended on) were
// removed as dead code — every live claim path is the SPONSORED Tempo flow
// (`claim_and_maybe_set_main_sponsored`), where the user holds zero of
// anything.

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Multi-keyword queries (on-chain feedback #33/34): ONE discover call
    /// covers several keywords instead of a sequential scan per keyword.
    /// Semantics mirror the proxy's `rankAgentMatches` (api/mcp.ts): an agent
    /// matches if the whole phrase OR any token hits; more matched tokens
    /// rank higher; a whole-phrase hit outranks token hits; name tier still
    /// beats persona tier; recency breaks ties.
    #[test]
    fn rank_agent_matches_multi_keyword() {
        let agents = vec![
            ("chess-game".to_string(), "plays chess".to_string()),
            ("toolsmith".to_string(), "builds developer tools".to_string()),
            (
                "arcade".to_string(),
                "a game tool for retro arcade fun".to_string(),
            ),
            ("dave".to_string(), "writes haikus".to_string()),
        ];

        // "game tool" — every agent matching EITHER token comes back in one
        // call; both name-token hits rank above the persona hit.
        let hits = rank_agent_matches(&agents, "game tool");
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].0, "chess-game"); // name token, earlier (recency)
        assert_eq!(hits[1].0, "toolsmith"); // name token
        assert_eq!(hits[2].0, "arcade"); // persona-only (both tokens, phrase)
        // dave matches nothing → excluded.

        // Within the persona tier, a whole-phrase hit + more tokens outranks
        // a single-token hit.
        let personas = vec![
            ("a".to_string(), "tool things".to_string()),
            ("b".to_string(), "a game tool combo".to_string()),
        ];
        let hits = rank_agent_matches(&personas, "game tool");
        assert_eq!(hits[0].0, "b"); // phrase (100) + 2 tokens
        assert_eq!(hits[1].0, "a"); // 1 token

        // Single-token queries keep the old behavior exactly: tiers by
        // name-vs-persona, recency within a tier.
        let hits = rank_agent_matches(&agents, "game");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0, "chess-game");
        assert_eq!(hits[1].0, "arcade");
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
    fn encode_set_lessons_abi_layout() {
        let cd = encode_set_lessons(7, "hi");
        assert_eq!(&cd[0..4], &selector("setMetadata(uint256,bytes32,bytes)"));
        assert_eq!(&cd[4..36], &u256_be(7));
        assert_eq!(&cd[36..68], &keccak_key(LESSONS_LABEL));
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
    fn lessons_key_distinct_from_other_metadata_keys() {
        // The lessons slot must never collide with persona/app/html/face —
        // and the TS worker inlines its literal hash, so pin it here too.
        let lessons = keccak_key(LESSONS_LABEL);
        assert_ne!(lessons, keccak_key(PERSONA_LABEL));
        assert_ne!(lessons, keccak_key(PUBLIC_FACE_LABEL));
        assert_ne!(lessons, keccak_key(PUBLIC_HTML_LABEL));
        assert_ne!(lessons, app_metadata_key());
        // Must equal the literal inlined in proxy/api/scheduler.ts (LESSONS_KEY).
        let hex: String = lessons.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "08564cae936ec460c48a23578c7df5665bad18fe42f3c5dbde517ad67a9d9c89"
        );
    }

    #[test]
    fn encode_set_skills_abi_layout() {
        let cd = encode_set_skills(7, "hi");
        assert_eq!(&cd[0..4], &selector("setMetadata(uint256,bytes32,bytes)"));
        assert_eq!(&cd[4..36], &u256_be(7));
        assert_eq!(&cd[36..68], &keccak_key(SKILLS_LABEL));
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
    fn skills_key_distinct_from_other_metadata_keys() {
        // The skills slot must never collide with persona/lessons/app/html/face.
        let skills = keccak_key(SKILLS_LABEL);
        assert_ne!(skills, keccak_key(LESSONS_LABEL));
        assert_ne!(skills, keccak_key(PERSONA_LABEL));
        assert_ne!(skills, keccak_key(PUBLIC_FACE_LABEL));
        assert_ne!(skills, keccak_key(PUBLIC_HTML_LABEL));
        assert_ne!(skills, app_metadata_key());
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
}
