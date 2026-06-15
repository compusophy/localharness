// Shared-volume agent tools (krafto #1.4): a cross-subdomain, owner-scoped
// encrypted KV over SessionRoomFacet (#22), so an owner's sibling subdomains share
// state with no external DB despite per-origin OPFS walls. The room is keyed to
// the owner (`room_id_created_by` + deterministic `K_room`), so every sibling
// converges on the same log with no key exchange; visitors get an owner-only
// error. Browser port of the proven `localharness room` CLI flow over the
// sponsored-tx path. Additive, not value-moving → no confirm-gate.

use crate::encoding::parse_address;
use crate::tools::ClosureTool;

/// createRoom gas (cast-estimated ≈1.31M + AA overhead → 2M); mirrors
/// `registry::create_room_sponsored`.
const CREATE_ROOM_GAS: u128 = 2_000_000;

/// Owner-gated context: `(identity_secret, writer_addr, owner_hex, room_id)`,
/// lazily creating the owner's room. `identity_secret` (32-byte k256 scalar)
/// derives `K_room`; `writer_addr` is the on-chain op writer. Errors (no panic)
/// for no local identity or a non-owner visitor.
async fn owner_room_context() -> Result<([u8; 32], [u8; 20], String, u64), crate::error::Error> {
    // The LOCAL credit signer = the owner's master wallet on the owner's device
    // (same key the apex iframe signs the sponsored tx with, so the on-chain
    // writer matches this address and `open_op` authenticates the blob).
    let (signer, addr) = crate::app::chat::credit_signer().await.ok_or_else(|| {
        crate::error::Error::other(
            "no local identity — the shared volume is owner-only; claim/own this subdomain first",
        )
    })?;
    let identity_secret: [u8; 32] = signer.to_bytes().into();
    let writer_addr = addr;

    // Owner gate: the shared volume is scoped to the OWNER. Compare the local
    // identity address to this subdomain's on-chain owner. A visitor's local
    // device key won't match → a clear owner-only message (no panic).
    let (_name, owner) = crate::app::tenant::current_tenant_owner()
        .await
        .map_err(crate::error::Error::other)?;
    let local_hex = crate::encoding::bytes_to_hex_str(&writer_addr);
    if !local_hex.eq_ignore_ascii_case(&owner) {
        return Err(crate::error::Error::other(
            "the shared volume is owner-only: this device's identity is not this \
             subdomain's on-chain owner, so it cannot read or write the owner's shared state",
        ));
    }

    // Resolve (or lazily create) the owner's shared-volume room. Keyed by the
    // OWNER address, so EVERY sibling subdomain of this owner converges on the
    // same room id — the cross-origin shared volume.
    let room_id = ensure_room(&owner).await?;
    Ok((identity_secret, writer_addr, owner, room_id))
}

/// The owner's shared-volume room id: the most recent room they created, or a
/// freshly created one. Create routes through the SAME sponsored Tempo path as
/// create_subdomain (`run_sponsored_tempo_call` — apex iframe signs the sender,
/// embedded sponsor pays gas), then reads the id back from the `RoomCreated`
/// logs filtered by the creator (race-free w.r.t. other accounts).
async fn ensure_room(owner_hex: &str) -> Result<u64, crate::error::Error> {
    if let Some(id) = crate::app::registry::room_id_created_by(owner_hex)
        .await
        .map_err(crate::error::Error::other)?
    {
        return Ok(id);
    }
    // None yet → create one. The owner is the creator + first member.
    let diamond = parse_address(crate::app::registry::REGISTRY_ADDRESS)
        .map_err(crate::error::Error::other)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond,
        value_wei: 0,
        input: crate::app::registry::encode_create_room(),
    };
    crate::app::events::run_sponsored_tempo_call(
        owner_hex,
        vec![call],
        CREATE_ROOM_GAS,
        "create shared-volume room",
    )
    .await
    .map_err(|e| crate::error::Error::other(format!("create shared volume failed: {e}")))?;

    // Read the new id back from the creator-filtered RoomCreated logs.
    crate::app::registry::room_id_created_by(owner_hex)
        .await
        .map_err(crate::error::Error::other)?
        .ok_or_else(|| {
            crate::error::Error::other(
                "shared volume created but its id is not yet visible on-chain — retry shortly",
            )
        })
}

/// Read + decrypt all of `room_id`'s ops under `k_room`, mirroring the CLI's
/// `read_ops`: blobs that don't open (tamper, or a foreign writer in a future
/// multi-identity room) are skipped rather than erroring.
async fn read_ops(
    room_id: u64,
    k_room: &[u8; 32],
) -> Result<Vec<crate::kv_reduce::KvOp>, crate::error::Error> {
    let raw = crate::app::registry::ops_of(room_id, 0)
        .await
        .map_err(crate::error::Error::other)?;
    let mut out = Vec::with_capacity(raw.len());
    for (writer_hex, _ts, blob) in raw {
        let Ok(writer_bytes) = crate::encoding::hex_to_bytes(&writer_hex) else {
            continue;
        };
        let Ok(writer): Result<[u8; 20], _> = writer_bytes.as_slice().try_into() else {
            continue;
        };
        if let Some(op) = crate::kv_room::open_op(&blob, k_room, &writer, room_id) {
            out.push(op);
        }
    }
    Ok(out)
}

/// `shared_state_set(key, value)` — append an encrypted set-op to the owner's
/// shared volume (the room is lazily created on first use). Mirrors the CLI's
/// `room set`: derive `K_room`, pick `next_lamport`, seal the op, append it via a
/// sponsored Tempo tx.
pub(crate) fn shared_state_set_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "key": {
                "type": "string",
                "description": "The key to write in the shared volume, e.g. \
                    \"task_status\" or \"worker_1/progress\"."
            },
            "value": {
                "type": "string",
                "description": "The value to store under `key` (UTF-8 text)."
            }
        },
        "required": ["key", "value"]
    });
    ClosureTool::new(
        "shared_state_set",
        "Write a key/value into your SHARED VOLUME — encrypted on-chain state that \
         ALL of your sibling subdomains (your other agents) read and write, with no \
         external database. Use this so a coordinator and its workers sync memory \
         across the cross-origin OPFS walls (each subdomain's local files are \
         isolated; this shared volume is not). Owner-only: only the owner's agents \
         can read or write it. Last-writer-wins per key. Returns { key, room_id, \
         tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("").trim();
            if key.is_empty() {
                return Err(crate::error::Error::other("key cannot be empty"));
            }
            let value = args.get("value").and_then(|v| v.as_str()).unwrap_or("");

            let (identity_secret, writer_addr, owner, room_id) = owner_room_context().await?;
            let k_room = crate::kv_room::derive_room_key(&identity_secret, room_id);

            // Read the current log to pick the next lamport (max seen + 1) —
            // exactly the CLI's discipline so writes from sibling subdomains
            // interleave deterministically (kv_reduce LWW).
            let existing = read_ops(room_id, &k_room).await?;
            let lamport = crate::kv_reduce::next_lamport(&existing);

            let op = crate::kv_reduce::KvOp {
                key: key.to_string(),
                value: Some(value.as_bytes().to_vec()),
                lamport,
                writer: writer_addr,
                ts: (js_sys::Date::now() / 1000.0) as u64,
            };
            // Seal with the owner's identity key — its address MUST be the
            // on-chain msg.sender that appends the blob (it is: the apex iframe
            // signs the sponsored append as the owner's master wallet).
            let (signer, _addr) = crate::app::chat::credit_signer().await.ok_or_else(|| {
                crate::error::Error::other("identity vanished mid-call")
            })?;
            let blob = crate::kv_room::seal_op(&op, &k_room, &signer, room_id)
                .ok_or_else(|| crate::error::Error::other("failed to seal shared-state op"))?;

            // Append via the SAME sponsored Tempo path as create_subdomain.
            // Length-scaled gas, matching registry::append_op_sponsored.
            let diamond = parse_address(crate::app::registry::REGISTRY_ADDRESS)
                .map_err(crate::error::Error::other)?;
            let call = crate::tempo_tx::TempoCall {
                to: diamond,
                value_wei: 0,
                input: crate::app::registry::encode_append_op(room_id, &blob),
            };
            let gas = 2_000_000u128 + (blob.len() as u128) * 9_000;
            let tx_hash = crate::app::events::run_sponsored_tempo_call(
                &owner,
                vec![call],
                gas,
                "shared_state_set",
            )
            .await
            .map_err(|e| crate::error::Error::other(format!("shared_state_set failed: {e}")))?;

            Ok(serde_json::json!({
                "key": key,
                "room_id": room_id,
                "tx_hash": tx_hash,
            }))
        },
    )
}

/// `shared_state_get(key)` — read the converged value for `key`, or "(unset)".
/// Mirrors the CLI's `room get`: ops → open → reduce → lookup.
pub(crate) fn shared_state_get_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "key": {
                "type": "string",
                "description": "The key to read from the shared volume."
            }
        },
        "required": ["key"]
    });
    ClosureTool::new(
        "shared_state_get",
        "Read one key from your SHARED VOLUME (the encrypted on-chain state shared \
         across all of your sibling subdomains/agents). Read-only, costs nothing. \
         Use this to pick up memory another of your agents wrote with \
         shared_state_set. Owner-only. Returns { key, value, found } — value is \
         \"(unset)\" when the key has never been written (or was deleted).",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("").trim();
            if key.is_empty() {
                return Err(crate::error::Error::other("key cannot be empty"));
            }
            let (identity_secret, _writer, _owner, room_id) = owner_room_context().await?;
            let k_room = crate::kv_room::derive_room_key(&identity_secret, room_id);
            let ops = read_ops(room_id, &k_room).await?;
            // ttl=0 disables expiry (durable shared state), now is irrelevant.
            let map = crate::kv_reduce::reduce(&ops, 0, 0);
            match map.get(key) {
                Some(v) => Ok(serde_json::json!({
                    "key": key,
                    "value": String::from_utf8_lossy(v),
                    "found": true,
                    "room_id": room_id,
                })),
                None => Ok(serde_json::json!({
                    "key": key,
                    "value": "(unset)",
                    "found": false,
                    "room_id": room_id,
                })),
            }
        },
    )
}

/// `shared_state_list()` — return the whole converged key→value map. Mirrors the
/// CLI's `room list`: ops → open → reduce → the full map.
pub(crate) fn shared_state_list_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    ClosureTool::new(
        "shared_state_list",
        "List your entire SHARED VOLUME — every key/value the owner's agents have \
         written to the cross-subdomain encrypted on-chain state. Read-only, costs \
         nothing. Use this to survey what shared memory exists before coordinating. \
         Owner-only. Returns { room_id, count, state: { key: value, ... } }.",
        serde_json::json!({ "type": "object", "properties": {} }),
        |_args: serde_json::Value, _ctx| async move {
            let (identity_secret, _writer, _owner, room_id) = owner_room_context().await?;
            let k_room = crate::kv_room::derive_room_key(&identity_secret, room_id);
            let ops = read_ops(room_id, &k_room).await?;
            let map = crate::kv_reduce::reduce(&ops, 0, 0);
            let state: serde_json::Map<String, serde_json::Value> = map
                .into_iter()
                .map(|(k, v)| (k, serde_json::json!(String::from_utf8_lossy(&v))))
                .collect();
            Ok(serde_json::json!({
                "room_id": room_id,
                "count": state.len(),
                "state": state,
            }))
        },
    )
}
