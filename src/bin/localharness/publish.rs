use crate::{bytes_to_hex_str, fmt_lh, key_write_path, load_name_signer, load_signer, load_sponsor, name_is_valid, parse_address, registry, resolve_key_read_path, secure_key_file, tempo_tx, wallet};

/// Parsed `create` arguments: the name, an optional persona, and whether
/// `--publish` was given (publish the scaffolded `app.rl` in the same flow so a
/// live URL exists immediately — on-chain feedback #75).
pub(crate) struct ParsedCreate {
    pub name: String,
    pub persona: Option<String>,
    pub publish: bool,
}

/// Parse `create <name> [--persona <text|file>] [--publish]`. Pure/testable.
/// One-shot actor creation: a name plus, optionally, its on-chain system prompt,
/// and optionally a one-command publish of the scaffolded face. `--publish` is a
/// bare flag (no value) and may appear before or after `--persona`; the persona
/// text stops collecting at `--publish` so `--persona a b --publish` works.
pub(crate) fn parse_create_args(rest: &[String]) -> Result<ParsedCreate, String> {
    const USAGE: &str = "usage: localharness create <name> [--persona <text|file>] [--publish]";
    let name = rest.first().ok_or(USAGE)?.clone();
    let mut persona: Option<String> = None;
    let mut publish = false;
    let mut i = 1;
    while i < rest.len() {
        match rest[i].as_str() {
            "--publish" => {
                publish = true;
                i += 1;
            }
            "--persona" => {
                // Collect the persona text up to the next recognised flag.
                let mut words: Vec<String> = Vec::new();
                i += 1;
                while i < rest.len() && rest[i] != "--publish" && rest[i] != "--persona" {
                    words.push(rest[i].clone());
                    i += 1;
                }
                if words.is_empty() {
                    return Err(USAGE.to_string());
                }
                persona = Some(words.join(" "));
            }
            other => return Err(format!("unexpected argument '{other}' ({USAGE})")),
        }
    }
    Ok(ParsedCreate { name, persona, publish })
}

/// The starter cartridge `create` scaffolds as `./app.rl` so a fresh agent can
/// `publish` immediately instead of hand-writing boilerplate (on-chain
/// feedback #14). Pinned by `starter_cartridge_compiles_with_entry` so
/// compiler drift can never ship a scaffold that `publish` itself refuses.
const STARTER_CARTRIDGE: &str = r#"// app.rl — your agent's public face (a rustlite cartridge).
//
// `localharness publish <name> app.rl` compiles this and publishes it
// on-chain as what every visitor sees at <name>.localharness.xyz —
// served 24/7, no tab needed. Edit, publish again to update.
//
// The display is a 512x512 framebuffer by default; export `fn dims() -> i32`
// (= (width<<16)|height, each 16..1024) for a custom size. Draw via host::display:
//   clear(rgb)  fill_rect(x, y, w, h, rgb)  set_pixel(x, y, rgb)
//   draw_line(x0, y0, x1, y1, rgb)  fill_triangle(x0, y0, x1, y1, x2, y2, rgb)
//   draw_char(x, y, code, rgb, scale)  draw_number(x, y, value, rgb, scale)
//   present()
// Input:   host::display::pointer_x() / pointer_y() / pointer_down()
// State:   host::display::state_get(slot) / state_set(slot, value)
// Full reference: https://localharness.xyz/llms.txt
//
// Export `frame(t)` (animated; t ticks up every frame) or `render()` (one-shot).

fn frame(t: i32) {
    host::display::clear(0);

    // A scanline sweeping the field — replace with your app.
    let y: i32 = t % 144;
    host::display::fill_rect(0, y, 256, 2, 16777215);

    // Frame counter, bottom-right.
    host::display::draw_number(206, 130, t, 8421504, 1);

    host::display::present();
}
"#;

/// Claim `<name>.localharness.xyz` — sponsored register, on-chain verify,
/// key persisted. With `persona`, also publishes the on-chain system prompt
/// so the name is a configured AGENT in one command (the actor-model
/// primitive: spawn an actor *with* its behavior).
///
/// IDEMPOTENT on the key: an existing local key for `name` is REUSED (the
/// name registers to its address) instead of being overwritten by a fresh
/// wallet — so a key whose name was never registered (or was released, e.g.
/// across the chain reset) can be re-claimed by just running `create` again,
/// and `create` on a name you already own is a clean no-op success.
pub(crate) async fn create(name: &str, persona: Option<&str>) -> i32 {
    // Box the call: `create → create_publish → publish_scaffolded_face → publish`
    // can re-enter `create` (publish claims a missing name), so the future is
    // self-referential and needs indirection.
    Box::pin(create_publish(name, persona, false)).await
}

/// [`create`] with an optional one-command publish (`create --publish`): after a
/// successful claim, compile + publish the scaffolded `app.rl` as the agent's
/// public face in the SAME flow so a live URL exists immediately (on-chain
/// feedback #75). `--publish` is NOT the default — bare `create` stays cheap (a
/// name-only mint); the publish is an extra opt-in sponsored tx.
pub(crate) async fn create_publish(name: &str, persona: Option<&str>, do_publish: bool) -> i32 {
    if !name_is_valid(name) {
        eprintln!("invalid name '{name}' — use 1-63 chars of a-z, 0-9, hyphen");
        return 2;
    }
    // Reuse an existing local key (cwd first, then config home) — never
    // silently overwrite one; the key IS the identity, and a stale-but-keyed
    // name (registered pre-reset, then reset away) must re-register to the
    // SAME address its owner already holds.
    let (agent, reused_key, key_file) = match resolve_key_read_path(name) {
        Some(path) => match std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| wallet::from_private_key_hex(s.trim()).ok())
        {
            Some(signer) => {
                let agent = wallet::from_signing_key(signer);
                println!("reusing the existing local key for '{name}' ({path})");
                (agent, true, path)
            }
            None => {
                eprintln!(
                    "a key file for '{name}' exists at {path} but doesn't parse — refusing to \
                     overwrite it; move it aside and re-run"
                );
                return 1;
            }
        },
        // NEW keys go to the config home (the safe location, out of any
        // project repo); falls back to the cwd if no home dir is resolvable.
        None => (wallet::generate(), false, key_write_path(name)),
    };
    let addr = agent.address_hex();

    let gitignored = if reused_key {
        false
    } else {
        // Persist BEFORE the on-chain write so the key is never lost even if
        // registration fails — the key IS the controllable identity.
        if let Err(e) = std::fs::write(&key_file, format!("{}\n", agent.private_key_hex)) {
            eprintln!(
                "could not persist key to {key_file}: {e} — aborting before any on-chain write"
            );
            return 1;
        }
        // Lock perms (0600, unix) + keep a cwd-fallback key out of git.
        secure_key_file(&key_file)
    };

    match registry::owner_of_name(name).await {
        Ok(Some(o)) if o.eq_ignore_ascii_case(&addr) => {
            // Idempotent success: the name is already registered to THIS key.
            println!("'{name}' is already registered to your key ({addr}) — nothing to do");
            if let Some(p) = persona {
                println!("  publishing persona …");
                let code = set_persona(name, p).await;
                if code != 0 {
                    return code;
                }
            }
            if do_publish {
                return publish_scaffolded_face(name).await;
            }
            return 0;
        }
        Ok(Some(o)) => {
            eprintln!("'{name}' is already taken (owner {o}) — pick another name");
            if !reused_key {
                let _ = std::fs::remove_file(&key_file);
            }
            return 2;
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    }

    // PAID CLAIMS (sybil gate): a non-zero registrationCost is pulled from the
    // claimer's $LH wallet inside register(). Pre-check so a fresh unfunded
    // key gets an actionable message instead of a raw chain revert.
    if let Ok(cost) = registry::registration_cost().await {
        if cost > 0 {
            let balance = registry::token_balance_of(&addr).await.unwrap_or(0);
            if balance < cost {
                eprintln!(
                    "claiming a name costs {} — this identity ({addr}) holds {}.",
                    fmt_lh(cost),
                    fmt_lh(balance)
                );
                eprintln!(
                    "fund it first: `localharness buy 2` (pay $2 by card → ~1.6 $LH after Stripe's \
                     cut; $1 nets only ~0.67), accept an invite (localharness invite accept <code>), \
                     redeem a code (localharness redeem <code>), or have another identity \
                     `localharness send {addr} <amount>` — then re-run create."
                );
                return 2;
            }
            println!("claiming costs {} (pulled on-chain from your wallet)", fmt_lh(cost));
        }
    }

    println!("claiming {name}.localharness.xyz for {addr} …");
    let tx = match registry::claim_and_maybe_set_main_sponsored(&agent.signer, name)
    .await
    {
        Ok(tx) => tx,
        Err(e) => {
            eprintln!("registration failed: {e}");
            return 1;
        }
    };

    match registry::owner_of_name(name).await {
        Ok(Some(owner)) if owner.eq_ignore_ascii_case(&addr) => {
            // GAS is sponsored (no native token needed), but the name FEE
            // (registrationCost, ~1 LH) IS pulled from the wallet — verified live
            // (a fresh create went 5.00 -> 4.00 LH). Don't claim "you paid nothing":
            // that contradicted the "claiming costs …" line above and was the seed of
            // the recurring "told free, charged 1 LH" onboarding confusion.
            println!("✓ you are live at https://{name}.localharness.xyz/  (mint gas sponsored — you needed no native token; the name fee came from your $LH)");
            println!("  tx:  {tx}");
            println!("  key: {key_file}  (keep this — it is your identity)");
            if gitignored {
                println!("       (added *.localharness.key to .gitignore so the key isn't committed)");
            }
            // One-shot actor: publish the persona right after the claim so the
            // name ships with its behavior, no separate edit step.
            if let Some(p) = persona {
                println!("  publishing persona …");
                let code = set_persona(name, p).await;
                if code != 0 {
                    return code;
                }
            }
            // Scaffold a starter cartridge so `publish` works immediately
            // (feedback #14: create → publish required hand-written
            // boilerplate). Never overwrites — an existing app.rl is the
            // user's working copy. Best-effort: a write failure only loses
            // the convenience, not the claim.
            let scaffolded = !std::path::Path::new("app.rl").exists()
                && std::fs::write("app.rl", STARTER_CARTRIDGE).is_ok();
            if scaffolded {
                println!(
                    "  wrote starter app.rl — edit it, then: localharness publish {name} app.rl"
                );
            }
            // --publish: compile + publish the scaffolded (or pre-existing)
            // app.rl now, so a live URL exists immediately (feedback #75).
            if do_publish {
                println!("  --publish: publishing the starter app.rl as your face …");
                let code = publish_scaffolded_face(name).await;
                if code != 0 {
                    return code;
                }
            }
            // Honest funding hint: read the REAL wallet balance instead of the old
            // "(you start with 0)" claim, which lied to funded agents (fleet-found;
            // an agent holding 2.00 $LH was told it had 0). 1 $LH/round is the
            // platform METER (inference), distinct from the x402 ask price.
            match registry::token_balance_of(&addr).await {
                Ok(wei) if wei > 0 => println!(
                    "  calls are metered at 1 $LH per model round — your wallet holds {}",
                    fmt_lh(wei)
                ),
                _ => println!("  calls are metered at 1 $LH per model round and your wallet is empty — fund via `localharness redeem <code>` or `localharness invite accept <code>`"),
            }
            println!("  tip: `localharness mcp` exposes a call_agent tool to your IDE (Claude Code, …)");
            println!("  next: read https://localharness.xyz/llms.txt for the full API");
            0
        }
        other => {
            eprintln!("registration didn't verify on-chain: {other:?}");
            1
        }
    }
}

/// Set `<name>`'s on-chain public face choice: `directory`, `app`, or `html`.
/// What visitors see. Owner-gated `setMetadata`, sponsored. (`publish` already
/// sets `app`; this is how you switch back to a directory landing, etc.)
pub(crate) async fn set_face(name: &str, choice: &str) -> i32 {
    if !matches!(choice, "directory" | "app" | "html") {
        eprintln!("face must be one of: directory, app, html (got '{choice}')");
        return 2;
    }
    let signer = match load_name_signer(name) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let addr = bytes_to_hex_str(&wallet::address(&signer));
    let id = match registry::id_of_name(name).await {
        Ok(i) if i != 0 => i,
        Ok(_) => {
            eprintln!("{name} is not registered");
            return 1;
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    };
    match registry::owner_of_name(name).await {
        Ok(Some(o)) if o.eq_ignore_ascii_case(&addr) => {}
        Ok(Some(o)) => {
            eprintln!("{name} is owned by {o}, not your key ({addr})");
            return 1;
        }
        _ => {
            eprintln!("{name} is not registered");
            return 1;
        }
    }
    let diamond = match parse_address(registry::REGISTRY_ADDRESS()) {
        Ok(a) => a,
        Err(_) => {
            eprintln!("internal: bad registry address constant");
            return 1;
        }
    };
    let calls = vec![tempo_tx::TempoCall {
        to: diamond,
        value_wei: 0,
        input: registry::encode_set_public_face(id, choice),
    }];
    let sponsor = match load_sponsor() {
        Ok(s) => s,
        Err(code) => return code,
    };
    match registry::submit_tempo_sponsored(
        &signer,
        &sponsor,
        calls,
        registry::ALPHA_USD_ADDRESS(),
        1_200_000,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ {name}.localharness.xyz public face → {choice}");
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("set-face failed: {e}");
            1
        }
    }
}

/// Map a filesystem IO error to a clean, OS-agnostic message. `verb` is the
/// attempted action ("read"/"write"). Addresses on-chain QA feedback: raw
/// `std::fs` errors leaked "(os error 2)" to users instead of a readable
/// "file not found".
pub(crate) fn clean_io_error(verb: &str, path: &str, e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => format!("file not found: {path}"),
        std::io::ErrorKind::PermissionDenied => format!("permission denied: {path}"),
        _ => format!("cannot {verb} {path}: {e}"),
    }
}

/// Read a file, mapping common IO errors to clean, OS-agnostic messages.
pub(crate) fn read_file_clean(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| clean_io_error("read", path, &e))
}

/// True when `arg` is UNAMBIGUOUSLY meant as a file path (whitespace-free +
/// a known text/source extension) rather than literal persona text. A bare
/// separator no longer qualifies — it misread inline personas like
/// "monochrome/brutalist" as paths (fleet-found); an EXISTING file always
/// wins regardless (see [`resolve_persona_arg`]).
pub(crate) fn looks_like_path(arg: &str) -> bool {
    !arg.chars().any(char::is_whitespace)
        && [".txt", ".md", ".rl", ".json", ".toml", ".prompt"]
            .iter()
            .any(|ext| arg.to_ascii_lowercase().ends_with(ext))
}

/// Resolve the `persona` arg to its text: a readable file's contents, or the
/// arg used verbatim. Returns a clean error (never a raw OS error) when the arg
/// is path-shaped but unreadable. A non-path-shaped string is always literal
/// text — so a one-line persona never trips the filesystem.
pub(crate) fn resolve_persona_arg(text_or_path: &str) -> Result<String, String> {
    match std::fs::read_to_string(text_or_path) {
        Ok(s) => Ok(s),
        // Path-shaped + unreadable → the user meant a file; surface it cleanly.
        Err(e) if looks_like_path(text_or_path) => Err(clean_io_error("read", text_or_path, &e)),
        // Otherwise the arg IS the persona text.
        Err(_) => Ok(text_or_path.to_string()),
    }
}

/// True if the compiled cartridge exports a `frame` or `render` function — the
/// entry point the display loader calls. A cartridge without one compiles fine
/// but renders nothing as a public face. Parses the wasm export section (id 7);
/// conservative — returns false if the bytes don't parse cleanly.
pub(crate) fn cartridge_has_entry(wasm: &[u8]) -> bool {
    fn leb(b: &[u8], i: &mut usize) -> Option<u64> {
        let (mut result, mut shift) = (0u64, 0u32);
        loop {
            let byte = *b.get(*i)?;
            *i += 1;
            result |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Some(result);
            }
            shift += 7;
            if shift >= 64 {
                return None;
            }
        }
    }
    if wasm.len() < 8 || &wasm[0..4] != b"\0asm" {
        return false;
    }
    let mut i = 8; // skip magic + version
    while i < wasm.len() {
        let id = wasm[i];
        i += 1;
        let Some(size) = leb(wasm, &mut i) else {
            return false;
        };
        let section_end = i + size as usize;
        if section_end > wasm.len() {
            return false;
        }
        if id == 7 {
            let mut j = i;
            let Some(count) = leb(wasm, &mut j) else {
                return false;
            };
            for _ in 0..count {
                let Some(name_len) = leb(wasm, &mut j) else {
                    return false;
                };
                let Some(name) = wasm.get(j..j + name_len as usize) else {
                    return false;
                };
                j += name_len as usize;
                if name == b"frame" || name == b"render" {
                    return true;
                }
                j += 1; // export kind
                if leb(wasm, &mut j).is_none() {
                    return false;
                }
            }
        }
        i = section_end;
    }
    false
}

/// Compile rustlite on a generously-sized worker thread. The compiler is deeply
/// recursive (parser / typecheck / codegen), and a non-trivial cartridge
/// overflows the ~1MB Windows MAIN-thread stack that `#[tokio::main]` runs on
/// (RUST_MIN_STACK only sizes spawned threads, not main). A 64 MB std::thread
/// has the headroom; `CompileError` is Send, so the result crosses back.
pub(crate) fn compile_big_stack(src: &str) -> Result<Vec<u8>, localharness::rustlite::CompileError> {
    let owned = src.to_string();
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || localharness::rustlite::compile(&owned))
        .expect("spawn rustlite compile thread")
        .join()
        .expect("rustlite compile thread panicked")
}

/// Parse the wasm import section (id 2) and return every `host::<module>::<func>`
/// the cartridge binds — its exact platform-call surface (its "tool schemas" in
/// cartridge-author terms) — sorted + deduped. Imports are emitted as
/// `host_<module>` (codegen), so the `host_` prefix is stripped. Conservative:
/// malformed / truncated bytes yield an empty Vec, never a panic.
pub(crate) fn cartridge_host_calls(wasm: &[u8]) -> Vec<String> {
    fn leb(b: &[u8], i: &mut usize) -> Option<u64> {
        let (mut result, mut shift) = (0u64, 0u32);
        loop {
            let byte = *b.get(*i)?;
            *i += 1;
            result |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Some(result);
            }
            shift += 7;
            if shift >= 64 {
                return None;
            }
        }
    }
    // Skip a wasm `limits` (flag byte, min, optional max) — table/mem imports.
    fn skip_limits(b: &[u8], i: &mut usize) -> Option<()> {
        let flag = leb(b, i)?;
        leb(b, i)?; // min
        if flag & 0x01 != 0 {
            leb(b, i)?; // max
        }
        Some(())
    }
    fn parse(wasm: &[u8]) -> Option<Vec<String>> {
        if wasm.len() < 8 || &wasm[0..4] != b"\0asm" {
            return None;
        }
        let mut calls: Vec<String> = Vec::new();
        let mut i = 8; // skip magic + version
        while i < wasm.len() {
            let id = wasm[i];
            i += 1;
            let size = leb(wasm, &mut i)?;
            let section_end = i.checked_add(size as usize)?;
            if section_end > wasm.len() {
                return None;
            }
            if id == 2 {
                let mut j = i;
                let count = leb(wasm, &mut j)?;
                for _ in 0..count {
                    let mod_len = leb(wasm, &mut j)? as usize;
                    let module = wasm.get(j..)?.get(..mod_len)?;
                    j += mod_len;
                    let field_len = leb(wasm, &mut j)? as usize;
                    let field = wasm.get(j..)?.get(..field_len)?;
                    j += field_len;
                    let kind = *wasm.get(j)?;
                    j += 1;
                    match kind {
                        0x00 => {
                            leb(wasm, &mut j)?; // func: type index
                            if let Some(m) = module.strip_prefix(b"host_") {
                                if let (Ok(m), Ok(f)) =
                                    (std::str::from_utf8(m), std::str::from_utf8(field))
                                {
                                    calls.push(format!("host::{m}::{f}"));
                                }
                            }
                        }
                        0x01 => {
                            j += 1; // table: elem type
                            skip_limits(wasm, &mut j)?;
                        }
                        0x02 => skip_limits(wasm, &mut j)?, // mem
                        0x03 => j += 2,                     // global: valtype + mut
                        _ => return None,
                    }
                }
            }
            i = section_end;
        }
        calls.sort();
        calls.dedup();
        Some(calls)
    }
    parse(wasm).unwrap_or_default()
}

/// Parse `compile <source.rl> [out.wasm] [--out <file>] [--host-calls|--schemas]`.
/// A bare 2nd positional is the out path (back-compat); `--out` is the flag form.
/// `--host-calls` (telemetry #52 alias `--schemas`) dumps the cartridge's
/// host-call surface. Pure/testable. Returns `(source, out, host_calls)`.
pub(crate) fn parse_compile_args(rest: &[String]) -> Result<(String, Option<String>, bool), String> {
    const USAGE: &str =
        "usage: localharness compile <source.rl> [out.wasm] [--out <file>] [--host-calls]";
    let (mut source, mut out, mut host_calls) = (None, None, false);
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--host-calls" | "--schemas" => host_calls = true,
            "--out" => {
                i += 1;
                // A following flag is a missing value, not the out path.
                out = Some(rest.get(i).filter(|s| !s.starts_with("--")).ok_or(USAGE)?.clone());
            }
            s if s.starts_with("--") => return Err(USAGE.to_string()),
            s if source.is_none() => source = Some(s.to_string()),
            s if out.is_none() => out = Some(s.to_string()),
            _ => return Err(USAGE.to_string()),
        }
        i += 1;
    }
    Ok((source.ok_or(USAGE)?, out, host_calls))
}

/// Compile-check a rustlite cartridge locally and report its size — NO on-chain
/// write. Lets an author iterate before spending a sponsored publish. With
/// `out_path`, also writes the compiled `.wasm`. With `host_calls`, dumps the
/// `host::<module>::<func>` platform-call surface the cartridge binds.
pub(crate) fn compile_check(source_path: &str, out_path: Option<&str>, host_calls: bool) -> i32 {
    let src = match read_file_clean(source_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    match compile_big_stack(&src) {
        Ok(wasm) => {
            println!("✓ compiled {source_path} → {} bytes of wasm", wasm.len());
            if let Some(out) = out_path {
                if let Err(e) = std::fs::write(out, &wasm) {
                    eprintln!("  {}", clean_io_error("write", out, &e));
                    return 1;
                }
                println!("  wrote {out}");
            }
            // Dump BEFORE the entry/cap gates: an entry-less or over-cap cartridge
            // is exactly the broken case an author wants to introspect (telemetry #52).
            if host_calls {
                let calls = cartridge_host_calls(&wasm);
                if calls.is_empty() {
                    println!("  host-calls: none (binds no host:: platform calls)");
                } else {
                    println!("  host-calls ({}):", calls.len());
                    for c in &calls {
                        println!("    {c}");
                    }
                }
            }
            if !cartridge_has_entry(&wasm) {
                eprintln!(
                    "  ✗ no `frame` or `render` export — the loader has no entry to \
                     call, so this would render nothing as a face"
                );
                return 1;
            }
            if wasm.len() > APPSTORE_PUBLISH_CAP {
                eprintln!(
                    "  ✗ {} bytes exceeds the {APPSTORE_PUBLISH_CAP}-byte app-store publish cap",
                    wasm.len()
                );
                return 1;
            }
            println!(
                "  fits the {APPSTORE_PUBLISH_CAP}-byte publish cap ({} bytes to spare)",
                APPSTORE_PUBLISH_CAP - wasm.len()
            );
            // No native wasm host exists — the host_* imports are browser closures.
            // Point authors at the real exec surface instead of faking a headless run.
            println!(
                "  run it live via the in-browser run_cartridge tool, or open \
                 https://<name>.localharness.xyz after publish"
            );
            0
        }
        Err(e) => {
            // Full rendering: LH code + message + line/col + caret snippet.
            eprintln!("compile failed: {}", e.render(&src));
            1
        }
    }
}

/// Publish the local `app.rl` (the one `create` scaffolds) as `<name>`'s public
/// face — the body of `create --publish`. Ensures `app.rl` exists (writing the
/// starter if not), then delegates to the normal [`publish`] path so a fresh
/// agent has a live URL the moment `create` returns (on-chain feedback #75).
pub(crate) async fn publish_scaffolded_face(name: &str) -> i32 {
    if !std::path::Path::new("app.rl").exists() {
        if let Err(e) = std::fs::write("app.rl", STARTER_CARTRIDGE) {
            eprintln!("  could not write starter app.rl to publish: {e}");
            return 1;
        }
    }
    publish(name, "app.rl").await
}

/// Compile a rustlite cartridge and publish it as `<name>`'s public face —
/// served to every visitor 24/7 with NO browser tab running. The app (cartridge)
/// face publishes OFF-CHAIN to the app store (`publish_app_offchain`, free, no
/// gas); the HTML face stays on-chain (`setMetadata`). Ownership stays on-chain.
pub(crate) async fn publish(name: &str, source_path: &str) -> i32 {
    // One command: if we don't hold this name's key yet (in cwd OR the config
    // home), claim the subdomain first (sponsored), then publish — no separate
    // `create` step (test-user fleet feedback, nova-qa). `create` refuses names
    // already taken by someone else and cleans up its key on failure, so
    // delegating is safe.
    if resolve_key_read_path(name).is_none() {
        eprintln!("no local key for '{name}' — claiming the subdomain first…");
        let code = create(name, None).await;
        if code != 0 {
            return code;
        }
    }
    let key_file = match resolve_key_read_path(name) {
        Some(p) => p,
        None => {
            eprintln!("could not find {name}'s key after claim");
            return 1;
        }
    };
    let key_hex = match std::fs::read_to_string(&key_file) {
        Ok(s) => s.trim().to_string(),
        Err(e) => {
            eprintln!("could not read {key_file} after claim: {e}");
            return 1;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let addr = bytes_to_hex_str(&wallet::address(&signer));

    // Only the owner can set metadata — fail early with a clear message.
    match registry::owner_of_name(name).await {
        Ok(Some(o)) if o.eq_ignore_ascii_case(&addr) => {}
        Ok(Some(o)) => {
            eprintln!("{name} is owned by {o}, not your key ({addr})");
            return 1;
        }
        Ok(None) => {
            eprintln!("{name} is not registered — run `localharness create {name}` first");
            return 1;
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    }

    let src = match read_file_clean(source_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };

    // Route by extension: .html/.htm publishes the raw bytes as the HTML face
    // ON-CHAIN (rasterized to every visitor's framebuffer); anything else
    // compiles as a rustlite cartridge and publishes OFF-CHAIN to the app store
    // (free, no gas — the blockchain keeps only the name's ownership, which we
    // already verified above). HTML stays on-chain for now (smaller, rarer).
    if publishes_as_html(source_path) {
        let html = src.as_bytes();
        if html.is_empty() {
            eprintln!("{source_path} is empty — nothing to publish");
            return 1;
        }
        // Off-chain HTML publish — no gas. The owner check at the top of publish()
        // already proved this key owns the name (= what the proxy re-checks).
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let token = registry::proxy_auth_token(&signer, now, "publish");
        println!(
            "publishing {} bytes as the html face of {name}.localharness.xyz (off-chain, no gas) …",
            html.len()
        );
        return match registry::publish_html_to_store(name, &token, &src).await {
            Ok(()) => {
                // Visitors only see stored html when the on-chain face CHOICE
                // says "html" — the unset arm infers cartridge-else-directory
                // (fleet-found: publish printed success while visitors kept
                // seeing the directory). Set the choice in the same command.
                let face = match registry::id_of_name(name).await {
                    Ok(id) if id != 0 => registry::public_face_of(id).await.ok().flatten(),
                    _ => None,
                };
                if face.as_deref() != Some("html") {
                    let code = set_face(name, "html").await;
                    if code != 0 {
                        eprintln!("html is in the store, but visitors won't see it until `localharness face {name} html` succeeds");
                        return code;
                    }
                }
                println!("✓ published — https://{name}.localharness.xyz/ now serves your html");
                println!("  to every visitor, 24/7, with no browser tab running.");
                println!("  content: app store (GitHub); ownership + face choice stay on-chain.");
                0
            }
            Err(e) => {
                eprintln!("publish failed: {e}");
                1
            }
        };
    }

    // App (cartridge) face — OFF-CHAIN publish to the app store.
    let wasm = match compile_big_stack(&src) {
        Ok(w) => w,
        Err(e) => {
            // Full rendering: LH code + line/col + caret snippet.
            eprintln!("compile failed: {}", e.render(&src));
            return 1;
        }
    };
    // A cartridge with no entry point compiles but renders nothing — refuse to
    // publish a dead face (the visitor would see a blank canvas forever).
    if !cartridge_has_entry(&wasm) {
        eprintln!(
            "compiled cartridge has no `frame`/`render` export — it would render \
             nothing as a face; aborting before publish"
        );
        return 1;
    }
    if wasm.len() > APPSTORE_PUBLISH_CAP {
        eprintln!(
            "compiled app is {} bytes; max {APPSTORE_PUBLISH_CAP} for the app store",
            wasm.len()
        );
        return 1;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let token = registry::proxy_auth_token(&signer, now, "publish");
    publish_app_offchain(name, &token, &wasm, &src).await
}

/// Max bytes of a compiled cartridge the app store accepts (host::compose
/// per-child wasm budget). Single source of truth in the registry so the CLI,
/// browser, and proxy all agree.
const APPSTORE_PUBLISH_CAP: usize = registry::APP_STORE_MAX_WASM_BYTES;

/// Publish a compiled cartridge (+ its source) to the OFF-CHAIN app store via
/// `registry::publish_app_to_store` (`POST /api/publish`, personal-sign authed,
/// gated server-side on the caller owning `name` on-chain). No gas, no sponsor —
/// the blockchain keeps only ownership. A visitor's browser fetches it back via
/// `registry::app_wasm_from_store`. Returns a process exit code.
async fn publish_app_offchain(name: &str, token: &str, wasm: &[u8], source: &str) -> i32 {
    println!(
        "publishing {} bytes as the app face of {name}.localharness.xyz (off-chain, no gas) …",
        wasm.len()
    );
    match registry::publish_app_to_store(name, token, wasm, source).await {
        Ok(()) => {
            println!("✓ published — https://{name}.localharness.xyz/ now serves your app");
            println!("  to every visitor, 24/7, with no browser tab running.");
            println!("  content: app store (GitHub); ownership stays on-chain — no gas spent.");
            0
        }
        Err(e) => {
            eprintln!("publish failed: {e}");
            1
        }
    }
}

/// `publish` routes by extension: `.html`/`.htm` (case-insensitive) publishes
/// the raw bytes as the HTML public face; everything else compiles as a
/// rustlite cartridge. Pure/testable.
pub(crate) fn publishes_as_html(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".html") || lower.ends_with(".htm")
}

/// System prompt for a target that hasn't published a persona on-chain.
pub(crate) fn default_persona(name: &str) -> String {
    format!(
        "You are {name}, an autonomous agent on localharness reachable at \
         {name}.localharness.xyz. Another agent is contacting you over the \
         network. Answer concisely and in character as {name}. Match your \
         response length to the question — answer simple or short questions \
         briefly and directly, expanding into detail only when the task genuinely \
         needs it. NEVER use emojis in your responses — plain text only. You have not \
         published a custom persona, so act as a helpful general-purpose agent."
    )
}

/// Publish `<name>`'s persona — the public system prompt a headless `call`
/// runs the agent under so it answers *as* that agent. Owner-gated
/// `setMetadata`, sponsored. `text_or_path` is used verbatim, unless it names
/// a readable file (then the file's contents are the persona).
pub(crate) async fn set_persona(name: &str, text_or_path: &str) -> i32 {
    let signer = match load_name_signer(name) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let addr = bytes_to_hex_str(&wallet::address(&signer));

    match registry::owner_of_name(name).await {
        Ok(Some(o)) if o.eq_ignore_ascii_case(&addr) => {}
        Ok(Some(o)) => {
            eprintln!("{name} is owned by {o}, not your key ({addr})");
            return 1;
        }
        Ok(None) => {
            eprintln!("{name} is not registered — run `localharness create {name}` first");
            return 1;
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    }

    // A readable path is loaded as a file; otherwise the arg IS the persona.
    // A path-shaped-but-unreadable arg gets a CLEAN error, not a raw OS error.
    let persona = match resolve_persona_arg(text_or_path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let persona = persona.trim();
    if persona.is_empty() {
        eprintln!("persona is empty");
        return 2;
    }
    if persona.len() > 4096 {
        eprintln!("persona is {} bytes; max 4096", persona.len());
        return 1;
    }

    let id = match registry::id_of_name(name).await {
        Ok(i) if i != 0 => i,
        _ => {
            eprintln!("no tokenId for {name}");
            return 1;
        }
    };
    let diamond = match parse_address(registry::REGISTRY_ADDRESS()) {
        Ok(a) => a,
        Err(_) => {
            eprintln!("internal: bad registry address constant");
            return 1;
        }
    };
    let calls = vec![tempo_tx::TempoCall {
        to: diamond,
        value_wei: 0,
        input: registry::encode_set_persona(id, persona),
    }];
    let gas = registry::set_metadata_gas(persona.len());

    let sponsor = match load_sponsor() {
        Ok(s) => s,
        Err(code) => return code,
    };
    println!("publishing {}-byte persona for {name}.localharness.xyz …", persona.len());
    match registry::submit_tempo_sponsored(
        &signer,
        &sponsor,
        calls,
        registry::ALPHA_USD_ADDRESS(),
        gas,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ persona set — `localharness call {name} \"…\"` now answers as {name}");
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("persona publish failed: {e}");
            1
        }
    }
}

/// `price <name> <amount|clear>` — advertise <name>'s per-call `$LH` price
/// on-chain (the floor the hosted `ask_agent` gate enforces; callers'
/// `--pay auto` resolves it). `clear`/`0` empties the slot, reverting to
/// the platform default. Headless twin of the admin panel's X402 PRICE.
pub(crate) async fn set_price(name: &str, amount: &str) -> i32 {
    let signer = match load_name_signer(name) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let addr = bytes_to_hex_str(&wallet::address(&signer));
    match registry::owner_of_name(name).await {
        Ok(Some(o)) if o.eq_ignore_ascii_case(&addr) => {}
        Ok(Some(o)) => {
            eprintln!("{name} is owned by {o}, not your key ({addr})");
            return 1;
        }
        Ok(None) => {
            eprintln!("{name} is not registered — run `localharness create {name}` first");
            return 1;
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    }
    let wei = if amount == "clear" {
        0
    } else {
        match localharness::encoding::parse_token_amount(amount) {
            Some(v) => v,
            None => {
                eprintln!("'{amount}' is not a $LH amount (e.g. 0.1) or 'clear'");
                return 2;
            }
        }
    };
    let id = match registry::id_of_name(name).await {
        Ok(i) if i != 0 => i,
        _ => {
            eprintln!("no tokenId for {name}");
            return 1;
        }
    };
    let diamond = match parse_address(registry::REGISTRY_ADDRESS()) {
        Ok(a) => a,
        Err(_) => {
            eprintln!("internal: bad registry address constant");
            return 1;
        }
    };
    let calls = vec![tempo_tx::TempoCall {
        to: diamond,
        value_wei: 0,
        input: registry::encode_set_x402_price(id, wei),
    }];
    let sponsor = match load_sponsor() {
        Ok(s) => s,
        Err(code) => return code,
    };
    let label = if wei == 0 {
        format!("clearing {name}'s advertised price (callers pay the default) …")
    } else {
        format!("advertising {} per call for {name}.localharness.xyz …", fmt_lh(wei))
    };
    println!("{label}");
    match registry::submit_tempo_sponsored(
        &signer,
        &sponsor,
        calls,
        registry::ALPHA_USD_ADDRESS(),
        1_200_000,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ price set — `localharness whoami {name}` shows it; callers `--pay auto` it");
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("price publish failed: {e}");
            1
        }
    }
}

/// Whether `release`'s typed `--confirm` matches the name EXACTLY. Pure +
/// testable — the gate of the destructive-action convention (typed, never
/// auto-filled).
pub(crate) fn release_confirmed(name: &str, confirm: Option<&str>) -> bool {
    confirm == Some(name)
}

/// The refusal printed when the typed confirmation doesn't match. It
/// DELIBERATELY names neither the correct `--confirm` value nor a ready-to-
/// paste command — the old message echoed the exact working command line,
/// a copy-paste bypass of the typed-confirmation friction.
pub(crate) const RELEASE_REFUSAL: &str = "\
release refused: this burns the name permanently. --confirm must exactly \
match the name being released — type it out deliberately.";

/// `release <name> --confirm <name>` — burn an owned subdomain NFT and free
/// the name (ReleaseFacet). DESTRUCTIVE: per the house convention the typed
/// confirmation is required and never auto-filled — `--confirm` must repeat
/// the exact name. Refuses the caller's MAIN client-side (the facet refuses
/// it on-chain too). The browser twin is the `release_subdomain` chat tool.
pub(crate) async fn release(caller: Option<&str>, name: &str, confirm: Option<&str>) -> i32 {
    if !release_confirmed(name, confirm) {
        eprintln!("{RELEASE_REFUSAL}");
        return 2;
    }
    let signer = match load_signer(caller) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let addr = bytes_to_hex_str(&wallet::address(&signer));
    let token_id = match registry::id_of_name(name).await {
        Ok(id) if id != 0 => id,
        Ok(_) => {
            eprintln!("'{name}' is not registered — nothing to release");
            return 2;
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    };
    match registry::owner_of_name(name).await {
        Ok(Some(o)) if o.eq_ignore_ascii_case(&addr) => {}
        Ok(Some(o)) => {
            eprintln!("'{name}' is owned by {o}, not your identity {addr}");
            return 2;
        }
        Ok(None) | Err(_) => {
            eprintln!("could not resolve '{name}'s owner");
            return 1;
        }
    }
    if registry::main_of(&addr).await.unwrap_or(0) == token_id {
        eprintln!("'{name}' is your MAIN identity — it cannot be released");
        return 2;
    }
    println!("releasing {name}.localharness.xyz (token #{token_id}) …");
    match registry::release_name_sponsored(&signer, token_id)
        .await
    {
        Ok(tx) => {
            println!("✓ released — '{name}' is free to register again");
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("release failed: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;

    #[test]
    fn release_confirmation_must_match_exactly() {
        assert!(release_confirmed("alice", Some("alice")));
        // Anything but the exact name refuses: missing, case-shifted,
        // whitespace-padded, or a different name.
        assert!(!release_confirmed("alice", None));
        assert!(!release_confirmed("alice", Some("Alice")));
        assert!(!release_confirmed("alice", Some(" alice")));
        assert!(!release_confirmed("alice", Some("bob")));
        assert!(!release_confirmed("alice", Some("")));
    }

    #[test]
    fn release_refusal_offers_no_copy_paste_bypass() {
        // The old refusal echoed `localharness release <name> --confirm <name>`
        // — the exact working command — defeating the typed-confirmation
        // friction. The refusal must explain the rule WITHOUT handing back a
        // pastable command or the correct value.
        assert!(!RELEASE_REFUSAL.contains("localharness release"));
        assert!(!RELEASE_REFUSAL.contains("re-run"), "must not coach a paste-and-retry");
        assert!(RELEASE_REFUSAL.contains("--confirm"));
        assert!(RELEASE_REFUSAL.contains("exactly"));
        // And it is name-agnostic by construction (a const), so it can never
        // leak the correct confirmation value.
    }

    #[test]
    fn parse_create_args_name_only_and_with_persona() {
        let c = parse_create_args(&args(&["alice"])).unwrap();
        assert_eq!(c.name, "alice");
        assert_eq!(c.persona, None);
        assert!(!c.publish);

        let c = parse_create_args(&args(&["alice", "--persona", "you", "are", "alice"]))
            .unwrap();
        assert_eq!(c.name, "alice");
        assert_eq!(c.persona.as_deref(), Some("you are alice"));
        assert!(!c.publish);
    }

    #[test]
    fn parse_create_args_publish_flag_any_position() {
        // Bare --publish.
        let c = parse_create_args(&args(&["alice", "--publish"])).unwrap();
        assert!(c.publish);
        assert_eq!(c.persona, None);
        // --publish AFTER a multi-word persona stops the persona collection.
        let c = parse_create_args(&args(&["alice", "--persona", "you are alice", "--publish"]))
            .unwrap();
        assert_eq!(c.persona.as_deref(), Some("you are alice"));
        assert!(c.publish);
        // --publish BEFORE --persona.
        let c = parse_create_args(&args(&["alice", "--publish", "--persona", "terse"])).unwrap();
        assert_eq!(c.persona.as_deref(), Some("terse"));
        assert!(c.publish);
    }

    #[test]
    fn parse_create_args_rejects_bad_forms() {
        assert!(parse_create_args(&args(&[])).is_err()); // no name
        assert!(parse_create_args(&args(&["alice", "--persona"])).is_err()); // empty persona
        assert!(parse_create_args(&args(&["alice", "--persona", "--publish"])).is_err()); // empty persona before flag
        assert!(parse_create_args(&args(&["alice", "bob"])).is_err()); // stray positional
    }

    /// `publish` routes .html/.htm (any case) to the HTML face and everything
    /// else through the rustlite compiler — a mis-route either feeds HTML to
    /// the compiler (confusing error) or publishes source text as a "page".
    #[test]
    fn publish_routes_by_extension() {
        assert!(publishes_as_html("index.html"));
        assert!(publishes_as_html("path/to/Page.HTML"));
        assert!(publishes_as_html("a.htm"));
        assert!(!publishes_as_html("app.rl"));
        assert!(!publishes_as_html("html")); // no extension — not a match
        assert!(!publishes_as_html("page.html.rl")); // suffix wins
    }

    /// The scaffold `create` writes must always compile AND carry an entry
    /// point — otherwise `publish` itself refuses it (no-entry guard) and the
    /// one-command onboarding the scaffold exists for is broken.
    #[test]
    fn starter_cartridge_compiles_with_entry() {
        let wasm = localharness::rustlite::compile(STARTER_CARTRIDGE)
            .expect("starter cartridge must compile");
        assert!(
            cartridge_has_entry(&wasm),
            "starter cartridge must export frame/render"
        );
        assert!(
            wasm.len() <= APPSTORE_PUBLISH_CAP,
            "starter cartridge must fit the publish cap"
        );
    }

    #[test]
    fn rustlite_compiles_a_minimal_cartridge() {
        // Uses only primitives proven in the live claude-app.rl face.
        let src = "fn frame(t: i32) {\n  \
                   let w: i32 = host::display::width();\n  \
                   host::display::clear(0);\n  \
                   host::display::fill_rect(0, 0, w, 8, 16777215);\n  \
                   host::display::present();\n}";
        let wasm = localharness::rustlite::compile(src).expect("minimal cartridge compiles");
        assert_eq!(&wasm[0..4], b"\0asm", "valid wasm magic header");
        assert!(wasm.len() <= APPSTORE_PUBLISH_CAP);
    }

    #[test]
    fn rustlite_rejects_garbage() {
        assert!(localharness::rustlite::compile("this is not rustlite").is_err());
    }

    #[test]
    fn cartridge_entry_detection() {
        // A real frame() cartridge exports the entry the loader calls.
        let with =
            localharness::rustlite::compile("fn frame(t: i32) { host::display::present(); }")
                .unwrap();
        assert!(cartridge_has_entry(&with), "frame() must be detected");

        // Compiles, but only a helper — no entry → would render nothing.
        let without = localharness::rustlite::compile("fn helper(n: i32) -> i32 { n + 1 }").unwrap();
        assert!(!cartridge_has_entry(&without), "no entry must be rejected");

        // The shipped bitmask cartridge has an entry.
        let bitmask = localharness::rustlite::compile(include_str!("../../../examples/cartridges/bitmask.rl")).unwrap();
        assert!(cartridge_has_entry(&bitmask));

        // Malformed / truncated bytes never panic and report no entry.
        assert!(!cartridge_has_entry(b""));
        assert!(!cartridge_has_entry(b"\0asm")); // header only
        assert!(!cartridge_has_entry(b"\0asm\x01\0\0\0\x07\xff")); // bogus section size
    }

    #[test]
    fn cartridge_host_calls_lists_bound_platform_calls() {
        // The dump is the cartridge's OWN host-call surface, sorted + deduped.
        let wasm = localharness::rustlite::compile(
            "fn frame(t: i32) { host::display::clear(0); host::display::present(); }",
        )
        .unwrap();
        assert_eq!(
            cartridge_host_calls(&wasm),
            vec![
                "host::display::clear".to_string(),
                "host::display::present".to_string()
            ]
        );
    }

    #[test]
    fn cartridge_host_calls_spans_modules_and_strips_host_prefix() {
        // A second module (net) proves the `host_`-prefix strip is per-import.
        let wasm = localharness::rustlite::compile(
            "fn frame(t: i32) { host::display::present(); let h: i32 = host::net::open(0); }",
        )
        .unwrap();
        let calls = cartridge_host_calls(&wasm);
        assert!(calls.contains(&"host::net::open".to_string()), "got: {calls:?}");
        assert!(calls.contains(&"host::display::present".to_string()), "got: {calls:?}");
    }

    #[test]
    fn cartridge_host_calls_dedups_repeated_binds() {
        // The same call in two branches yields a single import → one entry.
        let wasm = localharness::rustlite::compile(
            "fn frame(t: i32) { if t > 0 { host::display::present(); } else { host::display::present(); } }",
        )
        .unwrap();
        assert_eq!(cartridge_host_calls(&wasm), vec!["host::display::present".to_string()]);
    }

    #[test]
    fn cartridge_host_calls_robust_to_garbage() {
        // Malformed / truncated bytes never panic and report no calls.
        assert!(cartridge_host_calls(b"").is_empty());
        assert!(cartridge_host_calls(b"\0asm").is_empty()); // header only
        assert!(cartridge_host_calls(b"\0asm\x01\0\0\0\x02\xff").is_empty()); // bogus section size
    }

    #[test]
    fn parse_compile_args_flags_and_aliases() {
        // `--host-calls` and its `--schemas` alias resolve to the same bool.
        let hc = parse_compile_args(&args_of(&["app.rl", "--host-calls"])).unwrap();
        let sc = parse_compile_args(&args_of(&["app.rl", "--schemas"])).unwrap();
        assert_eq!(hc, ("app.rl".to_string(), None, true));
        assert_eq!(sc, ("app.rl".to_string(), None, true));
        // `--out <path>` resolves the out path; a bare 2nd positional still works.
        assert_eq!(
            parse_compile_args(&args_of(&["app.rl", "--out", "o.wasm"])).unwrap(),
            ("app.rl".to_string(), Some("o.wasm".to_string()), false)
        );
        assert_eq!(
            parse_compile_args(&args_of(&["app.rl", "o.wasm"])).unwrap(),
            ("app.rl".to_string(), Some("o.wasm".to_string()), false)
        );
        // Missing source or a dangling `--out` is an error, not a panic.
        assert!(parse_compile_args(&args_of(&["--host-calls"])).is_err());
        assert!(parse_compile_args(&args_of(&["app.rl", "--out"])).is_err());
        // `--out` must not swallow a following flag as its value.
        assert!(parse_compile_args(&args_of(&["app.rl", "--out", "--host-calls"])).is_err());
    }

    fn args_of(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn read_file_clean_maps_not_found_without_leaking_os_error() {
        // Closes on-chain QA finding #1: "os error 2" must not reach the user.
        let err = read_file_clean("definitely-nonexistent-file-xyz123.rl").unwrap_err();
        assert!(err.contains("file not found"), "got: {err}");
        assert!(err.contains("definitely-nonexistent-file-xyz123.rl"), "got: {err}");
        assert!(!err.contains("os error"), "must not leak raw OS error: {err}");
    }

    #[test]
    fn looks_like_path_distinguishes_files_from_prose() {
        // Path-shaped: whitespace-free with a known source/text extension.
        assert!(looks_like_path("persona.txt"));
        assert!(looks_like_path("prompts/agent.md"));
        assert!(looks_like_path("C:\\agents\\bob.prompt"));
        assert!(looks_like_path("./x.rl"));
        // Plain prose persona text is NOT a path — even with a separator
        // (fleet-found: "monochrome/brutalist" died as 'file not found').
        assert!(!looks_like_path("You are bob, a helpful agent"));
        assert!(!looks_like_path("bob"));
        assert!(!looks_like_path("monochrome/brutalist"));
        assert!(!looks_like_path("either/or thinker.md")); // whitespace = prose
    }

    #[test]
    fn resolve_persona_arg_literal_text_passthrough() {
        // Non-path-shaped, unreadable strings are the persona text verbatim —
        // they must NOT touch the filesystem error path.
        let p = resolve_persona_arg("You are bob, answer tersely").unwrap();
        assert_eq!(p, "You are bob, answer tersely");
        assert_eq!(resolve_persona_arg("monochrome/brutalist").unwrap(), "monochrome/brutalist");
    }

    #[test]
    fn resolve_persona_arg_existing_file_wins() {
        // An arg naming a file that EXISTS is read as the persona, even
        // without a known extension — existence decides.
        let path = std::env::temp_dir().join("lh_persona_probe_xyz123");
        std::fs::write(&path, "from the file").unwrap();
        assert_eq!(resolve_persona_arg(path.to_str().unwrap()).unwrap(), "from the file");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn resolve_persona_arg_missing_file_is_clean_error() {
        // A path-shaped arg that doesn't exist → clean error, no raw OS error,
        // and NOT silently used as literal text.
        let err = resolve_persona_arg("definitely-nonexistent-xyz123.txt").unwrap_err();
        assert!(err.contains("file not found"), "got: {err}");
        assert!(!err.contains("os error"), "must not leak raw OS error: {err}");
    }
}
