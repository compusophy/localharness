#[allow(unused_imports)]
use crate::*;

/// Parse `create <name> [--persona <text|file>]`. Pure/testable. One-shot
/// actor creation: a name plus, optionally, its on-chain system prompt.
pub(crate) fn parse_create_args(rest: &[String]) -> Result<(String, Option<String>), String> {
    const USAGE: &str = "usage: localharness create <name> [--persona <text|file>]";
    let name = rest.first().ok_or(USAGE)?.clone();
    let persona = match rest.get(1).map(String::as_str) {
        None => None,
        Some("--persona") => Some(
            rest.get(2..)
                .filter(|s| !s.is_empty())
                .map(|s| s.join(" "))
                .ok_or(USAGE)?,
        ),
        Some(other) => return Err(format!("unexpected argument '{other}' ({USAGE})")),
    };
    Ok((name, persona))
}

/// Claim `<name>.localharness.xyz` — fresh identity, sponsored register,
/// on-chain verify, key persisted. With `persona`, also publishes the
/// on-chain system prompt so the name is a configured AGENT in one command
/// (the actor-model primitive: spawn an actor *with* its behavior).
pub(crate) async fn create(name: &str, persona: Option<&str>) -> i32 {
    if !name_is_valid(name) {
        eprintln!("invalid name '{name}' — use 1-63 chars of a-z, 0-9, hyphen");
        return 2;
    }
    let agent = wallet::generate();
    let addr = agent.address_hex();
    // NEW keys go to the config home (the safe location, out of any project
    // repo); falls back to the cwd if no home dir is resolvable. Existing cwd
    // keys keep working — `resolve_key_read_path` reads cwd first.
    let key_file = key_write_path(name);

    // Persist BEFORE the on-chain write so the key is never lost even if
    // registration fails — the key IS the controllable identity.
    if let Err(e) = std::fs::write(&key_file, format!("{}\n", agent.private_key_hex)) {
        eprintln!("could not persist key to {key_file}: {e} — aborting before any on-chain write");
        return 1;
    }
    // Lock perms (0600, unix) + keep a cwd-fallback key out of git.
    let gitignored = secure_key_file(&key_file);

    match registry::owner_of_name(name).await {
        Ok(Some(o)) => {
            eprintln!("'{name}' is already taken (owner {o}) — pick another name");
            let _ = std::fs::remove_file(&key_file);
            return 2;
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    }

    let sponsor = match load_sponsor() {
        Ok(s) => s,
        Err(code) => return code,
    };

    println!("claiming {name}.localharness.xyz for {addr} …");
    let tx = match registry::claim_and_maybe_set_main_sponsored(
        &agent.signer,
        &sponsor,
        name,
        registry::ALPHA_USD_ADDRESS,
    )
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
            println!("✓ you are live at https://{name}.localharness.xyz/");
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
    let addr = format!("0x{}", to_hex(&wallet::address(&signer)));
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
    let diamond = match parse_addr20(registry::REGISTRY_ADDRESS) {
        Some(a) => a,
        None => {
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
        registry::ALPHA_USD_ADDRESS,
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

/// The on-chain `setMetadata` publish cap for a compiled cartridge (bytes).
pub(crate) const PUBLISH_CAP: usize = 16_384;

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

/// True when `arg` looks like it was MEANT as a file path (a path separator or a
/// known text/source extension) rather than literal persona text. Used so the
/// `persona` command can give a clean "file not found" error when the user
/// clearly intended a file, instead of silently using the path string as the
/// persona OR leaking a raw "(os error 2)".
pub(crate) fn looks_like_path(arg: &str) -> bool {
    arg.contains('/')
        || arg.contains('\\')
        || [".txt", ".md", ".rl", ".json", ".toml", ".prompt"]
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

/// Compile-check a rustlite cartridge locally and report its size — NO on-chain
/// write. Lets an author iterate before spending a sponsored publish. With
/// `out_path`, also writes the compiled `.wasm` (handy for local validation).
pub(crate) fn compile_check(source_path: &str, out_path: Option<&str>) -> i32 {
    let src = match read_file_clean(source_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    match localharness::rustlite::compile(&src) {
        Ok(wasm) => {
            println!("✓ compiled {source_path} → {} bytes of wasm", wasm.len());
            if let Some(out) = out_path {
                if let Err(e) = std::fs::write(out, &wasm) {
                    eprintln!("  {}", clean_io_error("write", out, &e));
                    return 1;
                }
                println!("  wrote {out}");
            }
            if !cartridge_has_entry(&wasm) {
                eprintln!(
                    "  ✗ no `frame` or `render` export — the loader has no entry to \
                     call, so this would render nothing as a face"
                );
                return 1;
            }
            if wasm.len() > PUBLISH_CAP {
                eprintln!(
                    "  ✗ {} bytes exceeds the {PUBLISH_CAP}-byte on-chain publish cap",
                    wasm.len()
                );
                return 1;
            }
            println!(
                "  fits the {PUBLISH_CAP}-byte publish cap ({} bytes to spare)",
                PUBLISH_CAP - wasm.len()
            );
            0
        }
        Err(e) => {
            eprintln!("compile failed: {e}");
            1
        }
    }
}

/// Compile a rustlite cartridge and publish it as `<name>`'s on-chain
/// public face — served to every visitor 24/7 with NO browser tab running.
/// Mirrors the browser studio's "publish app" exactly: setMetadata(app.wasm)
/// + setMetadata(public_face="app") in one sponsored Tempo tx.
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
    let addr = format!("0x{}", to_hex(&wallet::address(&signer)));

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
    let wasm = match localharness::rustlite::compile(&src) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("compile failed: {e}");
            return 1;
        }
    };
    // A cartridge with no entry point compiles but renders nothing — refuse to
    // publish a dead face (the visitor would see a blank canvas forever).
    if !cartridge_has_entry(&wasm) {
        eprintln!(
            "compiled cartridge has no `frame`/`render` export — it would render \
             nothing as a face; aborting before the on-chain write"
        );
        return 1;
    }
    // On-chain storage is metered per word; the studio caps published apps
    // at 16 KB. Mirror it so a too-big app fails locally, not after gas.
    if wasm.len() > PUBLISH_CAP {
        eprintln!(
            "compiled app is {} bytes; max {PUBLISH_CAP} to publish on-chain",
            wasm.len()
        );
        return 1;
    }

    let id = match registry::id_of_name(name).await {
        Ok(i) if i != 0 => i,
        _ => {
            eprintln!("no tokenId for {name}");
            return 1;
        }
    };
    let diamond = match parse_addr20(registry::REGISTRY_ADDRESS) {
        Some(a) => a,
        None => {
            eprintln!("internal: bad registry address constant");
            return 1;
        }
    };
    let mk = |input: Vec<u8>| tempo_tx::TempoCall { to: diamond, value_wei: 0, input };
    let calls = vec![
        mk(registry::encode_set_app_wasm(id, &wasm)),
        mk(registry::encode_set_public_face(id, "app")),
    ];
    // Gas budget. setMetadata stores the wasm bytes ON-CHAIN at ~7.6k gas/BYTE
    // (measured via debug_traceTransaction: a 476-byte app's storage call used
    // 3.61M gas — the same byte-storage inefficiency as the FeedbackFacet, NOT
    // the ~430k a replay misleadingly reports). Budget ~1.2M base (the
    // public_face call + AA settlement) + 8.5k/byte with headroom. Sponsor pays
    // only consumed gas. Practically this caps useful apps at a couple KB.
    let gas = 1_200_000 + (wasm.len() as u128) * 8_500;

    let sponsor = match load_sponsor() {
        Ok(s) => s,
        Err(code) => return code,
    };
    println!("publishing {} bytes as the public face of {name}.localharness.xyz …", wasm.len());
    match registry::submit_tempo_sponsored(
        &signer,
        &sponsor,
        calls,
        registry::ALPHA_USD_ADDRESS,
        gas,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ published — https://{name}.localharness.xyz/ now serves your app");
            println!("  to every visitor, 24/7, with no browser tab running.");
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("publish failed: {e}");
            1
        }
    }
}

/// System prompt for a target that hasn't published a persona on-chain.
pub(crate) fn default_persona(name: &str) -> String {
    format!(
        "You are {name}, an autonomous agent on localharness reachable at \
         {name}.localharness.xyz. Another agent is contacting you over the \
         network. Answer concisely and in character as {name}. You have not \
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
    let addr = format!("0x{}", to_hex(&wallet::address(&signer)));

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
    let diamond = match parse_addr20(registry::REGISTRY_ADDRESS) {
        Some(a) => a,
        None => {
            eprintln!("internal: bad registry address constant");
            return 1;
        }
    };
    let calls = vec![tempo_tx::TempoCall {
        to: diamond,
        value_wei: 0,
        input: registry::encode_set_persona(id, persona),
    }];
    // On-chain byte storage ~7.6k gas/byte (same as app/html); base + 8.5k/byte.
    let gas = 1_200_000 + (persona.len() as u128) * 8_500;

    let sponsor = match load_sponsor() {
        Ok(s) => s,
        Err(code) => return code,
    };
    println!("publishing {}-byte persona for {name}.localharness.xyz …", persona.len());
    match registry::submit_tempo_sponsored(
        &signer,
        &sponsor,
        calls,
        registry::ALPHA_USD_ADDRESS,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_create_args_name_only_and_with_persona() {
        let (n, p) = parse_create_args(&args(&["alice"])).unwrap();
        assert_eq!(n, "alice");
        assert_eq!(p, None);

        let (n, p) = parse_create_args(&args(&["alice", "--persona", "you", "are", "alice"]))
            .unwrap();
        assert_eq!(n, "alice");
        assert_eq!(p.as_deref(), Some("you are alice"));
    }

    #[test]
    fn parse_create_args_rejects_bad_forms() {
        assert!(parse_create_args(&args(&[])).is_err()); // no name
        assert!(parse_create_args(&args(&["alice", "--persona"])).is_err()); // empty persona
        assert!(parse_create_args(&args(&["alice", "bob"])).is_err()); // stray positional
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
        assert!(wasm.len() <= PUBLISH_CAP);
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
        let bitmask = localharness::rustlite::compile(include_str!("../../../bitmask.rl")).unwrap();
        assert!(cartridge_has_entry(&bitmask));

        // Malformed / truncated bytes never panic and report no entry.
        assert!(!cartridge_has_entry(b""));
        assert!(!cartridge_has_entry(b"\0asm")); // header only
        assert!(!cartridge_has_entry(b"\0asm\x01\0\0\0\x07\xff")); // bogus section size
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
        // Path-shaped: separators or known source/text extensions.
        assert!(looks_like_path("persona.txt"));
        assert!(looks_like_path("prompts/agent.md"));
        assert!(looks_like_path("C:\\agents\\bob.prompt"));
        assert!(looks_like_path("./x.rl"));
        // Plain prose persona text is NOT a path.
        assert!(!looks_like_path("You are bob, a helpful agent"));
        assert!(!looks_like_path("bob"));
    }

    #[test]
    fn resolve_persona_arg_literal_text_passthrough() {
        // A non-path-shaped, unreadable string is the persona text verbatim —
        // it must NOT touch the filesystem error path.
        let p = resolve_persona_arg("You are bob, answer tersely").unwrap();
        assert_eq!(p, "You are bob, answer tersely");
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
