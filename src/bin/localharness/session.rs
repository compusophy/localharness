#[allow(unused_imports)]
use crate::*;
use std::time::{SystemTime, UNIX_EPOCH};

// ---- session room: encrypted on-chain shared KV (SessionRoomFacet, #22) ------
//
// A SessionRoom is a member-gated, append-only log of ENCRYPTED key/value ops on
// the diamond. An agent persists state across turns/devices by appending sealed
// ops instead of re-sending full context; reads fold the log into a converged
// map (CRDT) off-chain. The chain stores only ciphertext.
//
// v1 is SINGLE-IDENTITY: the room key `K_room` is derived deterministically from
// the caller's identity secret + room id (`kv_room::derive_room_key`), so every
// device/session of the SAME identity reads/writes the same room with NO key
// exchange. Multi-identity rooms (ECIES-granting `K_room` to members enrolled via
// the facet's `roomAddMember`) are phase 2 — the chain + driver already support
// the membership; only the off-chain grant handshake is pending.

pub(crate) const SESSION_USAGE: &str = "\
usage: localharness room <create|set|get|list|clear> ...
  room create [--as <me>]                     create a room → prints the roomId
  room set [--as <me>] <roomId> <key> <value...>   write an encrypted key/value
  room get [--as <me>] <roomId> <key>         read one key's current value
  room list [--as <me>] <roomId>              read the whole converged map
  room clear [--as <me>] <roomId>             wipe the log (creator-only)";

pub(crate) async fn room(caller: Option<&str>, rest: &[String]) -> i32 {
    match rest.first().map(|s| s.as_str()) {
        Some("create") => session_create(caller).await,
        Some("set") => session_set(caller, &rest[1..]).await,
        Some("get") => session_get(caller, &rest[1..]).await,
        Some("list") => session_list(caller, &rest[1..]).await,
        Some("clear") => session_clear(caller, &rest[1..]).await,
        _ => {
            eprintln!("{SESSION_USAGE}");
            2
        }
    }
}

/// Caller's identity secret (32-byte k256 scalar) + 20-byte address. The secret
/// derives `K_room`; the address is the op writer (== on-chain `msg.sender`).
fn caller_secret_and_addr(
    signer: &k256::ecdsa::SigningKey,
) -> ([u8; 32], [u8; 20]) {
    let secret: [u8; 32] = signer.to_bytes().into();
    (secret, localharness::wallet::address(signer))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

async fn session_create(caller: Option<&str>) -> i32 {
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("creating session room …");
    match registry::create_room_sponsored(&signer, &sponsor, registry::ALPHA_USD_ADDRESS).await {
        Ok(_tx) => {
            let creator = format!("0x{}", localharness::encoding::bytes_to_hex(&localharness::wallet::address(&signer)));
            match registry::room_id_created_by(&creator).await {
                Ok(Some(id)) => {
                    println!("✓ room #{id} created");
                    println!("  set:  localharness room set {id} <key> <value>");
                    println!("  read: localharness room list {id}");
                    0
                }
                _ => {
                    println!("✓ room created (could not read back the id from logs — run `session list <id>` once you know it)");
                    0
                }
            }
        }
        Err(e) => {
            eprintln!("session create: {e}");
            1
        }
    }
}

async fn session_set(caller: Option<&str>, args: &[String]) -> i32 {
    if args.len() < 3 {
        eprintln!("{SESSION_USAGE}");
        return 2;
    }
    let room_id = match parse_id(&args[0], "room") {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let key = args[1].clone();
    let value = args[2..].join(" ");
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let (secret, addr) = caller_secret_and_addr(&signer);
    let k_room = localharness::kv_room::derive_room_key(&secret, room_id);

    // Read the current log to pick the next lamport (max seen + 1).
    let existing = read_ops(room_id, &k_room).await;
    let lamport = localharness::kv_reduce::next_lamport(&existing);

    let op = localharness::kv_reduce::KvOp {
        key: key.clone(),
        value: Some(value.into_bytes()),
        lamport,
        writer: addr,
        ts: now_secs(),
    };
    let Some(blob) = localharness::kv_room::seal_op(&op, &k_room, &signer, room_id) else {
        eprintln!("session set: failed to seal op");
        return 1;
    };
    match registry::append_op_sponsored(&signer, &sponsor, room_id, &blob, registry::ALPHA_USD_ADDRESS).await {
        Ok(_tx) => {
            println!("✓ set {key} in room #{room_id}");
            0
        }
        Err(e) => {
            eprintln!("session set: {e}");
            1
        }
    }
}

async fn session_get(caller: Option<&str>, args: &[String]) -> i32 {
    if args.len() < 2 {
        eprintln!("{SESSION_USAGE}");
        return 2;
    }
    let room_id = match parse_id(&args[0], "room") {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let key = &args[1];
    let signer = match load_signer(caller) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let (secret, _) = caller_secret_and_addr(&signer);
    let k_room = localharness::kv_room::derive_room_key(&secret, room_id);
    let ops = read_ops(room_id, &k_room).await;
    let map = localharness::kv_reduce::reduce(&ops, 0, 0);
    match map.get(key) {
        Some(v) => {
            println!("{}", String::from_utf8_lossy(v));
            0
        }
        None => {
            eprintln!("session get: no value for '{key}' in room #{room_id}");
            1
        }
    }
}

async fn session_list(caller: Option<&str>, args: &[String]) -> i32 {
    let Some(room_arg) = args.first() else {
        eprintln!("{SESSION_USAGE}");
        return 2;
    };
    let room_id = match parse_id(room_arg, "room") {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let signer = match load_signer(caller) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let (secret, _) = caller_secret_and_addr(&signer);
    let k_room = localharness::kv_room::derive_room_key(&secret, room_id);
    let ops = read_ops(room_id, &k_room).await;
    let map = localharness::kv_reduce::reduce(&ops, 0, 0);
    if map.is_empty() {
        println!("room #{room_id}: (empty)");
        return 0;
    }
    println!("room #{room_id}: {} key(s)", map.len());
    for (k, v) in &map {
        println!("  {k} = {}", String::from_utf8_lossy(v));
    }
    0
}

async fn session_clear(caller: Option<&str>, args: &[String]) -> i32 {
    let Some(room_arg) = args.first() else {
        eprintln!("{SESSION_USAGE}");
        return 2;
    };
    let room_id = match parse_id(room_arg, "room") {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    match registry::clear_room_sponsored(&signer, &sponsor, room_id, registry::ALPHA_USD_ADDRESS).await {
        Ok(_tx) => {
            println!("✓ room #{room_id} cleared");
            0
        }
        Err(e) => {
            eprintln!("session clear: {e}");
            1
        }
    }
}

/// Read + decrypt all of `room_id`'s ops with `k_room`. Blobs that don't open
/// (foreign writer in a future multi-identity room, tamper) are skipped.
async fn read_ops(room_id: u64, k_room: &[u8; 32]) -> Vec<localharness::kv_reduce::KvOp> {
    let raw = match registry::ops_of(room_id, 0).await {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::with_capacity(raw.len());
    for (writer_hex, _ts, blob) in raw {
        let Ok(writer_bytes) = localharness::encoding::hex_to_bytes(&writer_hex) else {
            continue;
        };
        let Ok(writer): Result<[u8; 20], _> = writer_bytes.as_slice().try_into() else {
            continue;
        };
        if let Some(op) = localharness::kv_room::open_op(&blob, k_room, &writer, room_id) {
            out.push(op);
        }
    }
    out
}
