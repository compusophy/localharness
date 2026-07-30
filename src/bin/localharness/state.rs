//! `lessons` / `skills` / `state` — an agent's LEARNED state from the terminal.
//!
//! The blobs were already READ here (`call` folds them into the headless system
//! prompt), but only the browser could ever WRITE them, so a CLI-only agent
//! could never record what it learned. These commands close that half, reusing
//! the same pure cores the browser tools use (`localharness::lessons` /
//! `::skills`) so a lesson written from a terminal is byte-identical to one
//! written in a tab.
//!
//! ⛔ NEVER batch the three slots into one sponsored tx. `setMetadata` costs
//! ~8.5k gas/BYTE, so a full lessons (2000B) + skills (4000B) + persona (4096B)
//! bundle asks for ~89M gas and the mainnet relay rejects anything over
//! `MAX_GAS_LIMIT = 50M` (proxy/api/sponsor.ts). One tx per changed slot: each
//! fits alone (18M / 35M / 36M), a batch never does.

use crate::{
    bytes_to_hex_str, lessons, load_name_signer, load_sponsor, parse_address, registry, skills,
    take_value_flag, tempo_tx, wallet,
};

/// Bundle format version — stamped into an export, required on import so a
/// future layout change fails loudly instead of writing garbage on-chain.
const BUNDLE_VERSION: u64 = 1;

/// Pull `--<flag> <value>` from anywhere in the args, printing the usage line
/// and mapping any parse failure to exit 2 (the CLI's arg-error code).
fn flag(args: &[String], name: &str, usage: &str) -> Result<(Option<String>, Vec<String>), i32> {
    take_value_flag(args, name, usage).map_err(|e| {
        eprintln!("{e}");
        2
    })
}

/// `localharness lessons <name> [--add <text>]` — show what an agent has
/// learned, or merge one new lesson into its on-chain blob.
pub(crate) async fn lessons_cmd(args: &[String]) -> i32 {
    const USAGE: &str = "usage: localharness lessons <name> [--add <lesson>]";
    let (add, rest) = match flag(args, "--add", USAGE) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let Some(name) = rest.first().cloned() else {
        eprintln!("{USAGE}");
        return 2;
    };

    let id = match token_id(&name).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let existing = read_slot(id, Slot::Lessons).await.unwrap_or_default();

    let Some(lesson) = add else {
        if existing.is_empty() {
            println!("{name} has recorded no lessons yet");
        } else {
            for (i, line) in existing.lines().enumerate() {
                println!("{:>2}. {line}", i + 1);
            }
        }
        return 0;
    };

    // The core owns dedup, the 10-line window, the 240-char truncation and the
    // 2000-byte cap — and returns the blob UNCHANGED when the lesson is empty
    // or already known, which is exactly the "don't pay gas" signal.
    let merged = lessons::merge_lesson(&existing, &lesson);
    if merged == existing {
        println!("already recorded (or empty) — nothing to write");
        return 0;
    }
    write_slot(&name, id, Slot::Lessons, &merged).await
}

/// `localharness skills <name> [--set <skill> <instructions> | --rm <skill>]`.
pub(crate) async fn skills_cmd(args: &[String]) -> i32 {
    const USAGE: &str = "usage: localharness skills <name> \
         [--set <skill> <instructions...> | --rm <skill> | --export <dir>]";
    let (set, rest) = match flag(args, "--set", USAGE) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let (rm, rest) = match flag(&rest, "--rm", USAGE) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let (export, rest) = match flag(&rest, "--export", USAGE) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let Some(name) = rest.first().cloned() else {
        eprintln!("{USAGE}");
        return 2;
    };

    let id = match token_id(&name).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let existing = read_slot(id, Slot::Skills).await.unwrap_or_default();

    if let Some(skill) = rm {
        let (updated, removed) = skills::remove(&existing, &skill);
        if !removed {
            println!("{name} has no skill named {skill}");
            return 0;
        }
        return write_slot(&name, id, Slot::Skills, &updated).await;
    }

    if let Some(skill) = set {
        // `--set <skill>` takes the REST of the line as the instructions, so
        // `--set deploy run the release script` needs no quoting.
        let instructions = rest[1..].join(" ");
        if instructions.trim().is_empty() {
            eprintln!("{USAGE}");
            return 2;
        }
        let updated = skills::upsert(&existing, &skill, &instructions);
        if updated == existing {
            println!("{skill} is already defined exactly that way — nothing to write");
            return 0;
        }
        return write_slot(&name, id, Slot::Skills, &updated).await;
    }

    // `--export <dir>`: write each skill as an agentskills.io SKILL.md folder
    // (`<dir>/<skill-name>/SKILL.md`), so this agent's learned skills load
    // unmodified in any skills-compatible harness — Claude Code, Codex,
    // Cursor, Copilot, goose, … Pure read; works on any agent's published
    // skills, not just your own.
    if let Some(dir) = export {
        let parsed = skills::parse(&existing);
        if parsed.is_empty() {
            println!("{name} has defined no skills yet — nothing to export");
            return 0;
        }
        let count = parsed.len();
        for skill in &parsed {
            let folder = std::path::Path::new(&dir).join(skills::skill_dir_name(&skill.name));
            if let Err(e) = std::fs::create_dir_all(&folder) {
                eprintln!("creating {}: {e}", folder.display());
                return 1;
            }
            let path = folder.join("SKILL.md");
            if let Err(e) = std::fs::write(&path, skills::to_skill_md(skill)) {
                eprintln!("writing {}: {e}", path.display());
                return 1;
            }
            println!("  wrote {}", path.display());
        }
        println!("✓ exported {count} skill(s) — point any Agent-Skills-compatible harness at {dir}");
        return 0;
    }

    let parsed = skills::parse(&existing);
    if parsed.is_empty() {
        println!("{name} has defined no skills yet");
        return 0;
    }
    for skill in parsed {
        println!("{}: {}", skill.name, skill.instructions);
    }
    0
}

/// `localharness state <name> [--out <file>] [--in <file|dir>] [--dir <dir>]`
/// — export or import the portable agent-state bundle (persona + lessons +
/// skills). `--out` writes the single-file JSON bundle; `--dir` writes the
/// `.agent/` DIRECTORY layout (the cross-harness shape from
/// `design/harness-standard.md`: manifest.json + persona.md + lessons.md +
/// skills/<slug>/SKILL.md). `--in` accepts either — a bundle file or an
/// `.agent/` directory.
pub(crate) async fn state_cmd(args: &[String]) -> i32 {
    const USAGE: &str =
        "usage: localharness state <name> [--out <file>] [--dir <dir>] [--in <file|dir>]";
    let (out, rest) = match flag(args, "--out", USAGE) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let (dir, rest) = match flag(&rest, "--dir", USAGE) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let (input, rest) = match flag(&rest, "--in", USAGE) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let Some(name) = rest.first().cloned() else {
        eprintln!("{USAGE}");
        return 2;
    };
    let id = match token_id(&name).await {
        Ok(id) => id,
        Err(code) => return code,
    };

    match (input, dir) {
        (Some(path), _) if std::path::Path::new(&path).is_dir() => {
            import_state_dir(&name, id, &path).await
        }
        (Some(path), _) => import_state(&name, id, &path).await,
        (None, Some(d)) => export_state_dir(&name, id, &d).await,
        (None, None) => export_state(&name, id, out.as_deref()).await,
    }
}

/// Export the `.agent/` DIRECTORY layout — the cross-harness portable-state
/// shape. The directory is a DERIVED VIEW of the chain-anchored slots (chain
/// stays canonical); it never carries key material. Skills are written as
/// agentskills.io `SKILL.md` folders, loadable by any Agent-Skills harness.
async fn export_state_dir(name: &str, id: u64, dir: &str) -> i32 {
    let root = std::path::Path::new(dir);
    if let Err(e) = std::fs::create_dir_all(root.join("skills")) {
        eprintln!("creating {dir}: {e}");
        return 1;
    }
    let manifest = serde_json::json!({
        "localharness_agent_state": BUNDLE_VERSION,
        "name": name,
        "chain": {
            "id": registry::CHAIN_ID(),
            "diamond": registry::REGISTRY_ADDRESS(),
            "token_id": id,
        },
    });
    let write = |path: std::path::PathBuf, text: String| -> Result<(), i32> {
        std::fs::write(&path, text).map_err(|e| {
            eprintln!("writing {}: {e}", path.display());
            1
        })
    };
    let pretty = serde_json::to_string_pretty(&manifest).unwrap_or_else(|_| "{}".into());
    if let Err(c) = write(root.join("manifest.json"), format!("{pretty}\n")) {
        return c;
    }
    let persona = read_slot(id, Slot::Persona).await.unwrap_or_default();
    let lessons_blob = read_slot(id, Slot::Lessons).await.unwrap_or_default();
    if let Err(c) = write(root.join("persona.md"), persona) {
        return c;
    }
    if let Err(c) = write(root.join("lessons.md"), lessons_blob) {
        return c;
    }
    let skills_blob = read_slot(id, Slot::Skills).await.unwrap_or_default();
    let mut count = 0;
    for skill in skills::parse(&skills_blob) {
        let folder = root.join("skills").join(skills::skill_dir_name(&skill.name));
        if let Err(e) = std::fs::create_dir_all(&folder) {
            eprintln!("creating {}: {e}", folder.display());
            return 1;
        }
        if let Err(c) = write(folder.join("SKILL.md"), skills::to_skill_md(&skill)) {
            return c;
        }
        count += 1;
    }
    println!("✓ wrote {name}'s .agent/ state to {dir} (persona, lessons, {count} skill(s))");
    0
}

/// Import from an `.agent/` directory: version-gate the manifest, reassemble
/// the skills blob from `skills/*/SKILL.md`, then run the SAME diff-write path
/// as the file bundle (sanitize both sides, one sponsored tx per changed slot).
async fn import_state_dir(name: &str, id: u64, dir: &str) -> i32 {
    let root = std::path::Path::new(dir);
    let manifest_text = match std::fs::read_to_string(root.join("manifest.json")) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{dir}/manifest.json: {e}");
            return 2;
        }
    };
    let manifest: serde_json::Value = match serde_json::from_str(&manifest_text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{dir}/manifest.json is not valid JSON: {e}");
            return 2;
        }
    };
    match manifest.get("localharness_agent_state").and_then(|v| v.as_u64()) {
        Some(BUNDLE_VERSION) => {}
        Some(v) => {
            eprintln!("{dir} is .agent/ version {v}; this build writes version {BUNDLE_VERSION}");
            return 2;
        }
        None => {
            eprintln!("{dir}/manifest.json is not a localharness .agent/ manifest");
            return 2;
        }
    }
    let read_opt = |p: std::path::PathBuf| std::fs::read_to_string(p).unwrap_or_default();
    let persona = read_opt(root.join("persona.md"));
    let lessons_blob = read_opt(root.join("lessons.md"));
    let mut parsed_skills = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root.join("skills")) {
        let mut dirs: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        dirs.sort();
        for d in dirs {
            let doc = read_opt(d.join("SKILL.md"));
            if let Some(s) = skills::from_skill_md(&doc) {
                parsed_skills.push(s);
            }
        }
    }
    let skills_blob = skills::serialize(&parsed_skills);
    let bundle = serde_json::json!({
        "localharness_agent_state": BUNDLE_VERSION,
        "name": name,
        "persona": persona,
        "lessons": lessons_blob,
        "skills": if parsed_skills.is_empty() { String::new() } else { skills_blob },
    });
    import_bundle_slots(name, id, &bundle, dir).await
}

/// Read all three slots and emit the bundle as JSON. Pure read — no key, no
/// gas. NEVER carries key material: the bundle is public on-chain state, and
/// the private key stays in `~/.localharness/keys/` where only its device
/// can reach it.
async fn export_state(name: &str, id: u64, out: Option<&str>) -> i32 {
    let bundle = serde_json::json!({
        "localharness_agent_state": BUNDLE_VERSION,
        "name": name,
        "persona": read_slot(id, Slot::Persona).await.unwrap_or_default(),
        "lessons": read_slot(id, Slot::Lessons).await.unwrap_or_default(),
        "skills": read_slot(id, Slot::Skills).await.unwrap_or_default(),
    });
    let text = serde_json::to_string_pretty(&bundle).unwrap_or_else(|_| "{}".into());
    match out {
        Some(path) => match std::fs::write(path, format!("{text}\n")) {
            Ok(()) => {
                println!("✓ wrote {name}'s agent state to {path}");
                0
            }
            Err(e) => {
                eprintln!("writing {path}: {e}");
                1
            }
        },
        None => {
            println!("{text}");
            0
        }
    }
}

/// Write every slot in the bundle that DIFFERS from what is on-chain, as one
/// sponsored tx each (see the module note on why this must never be batched).
/// Both sides are sanitized through the pure cores before comparing, so a blob
/// that differs only in normalization is not paid for twice.
async fn import_state(name: &str, id: u64, path: &str) -> i32 {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("reading {path}: {e}");
            return 1;
        }
    };
    let bundle: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{path} is not valid JSON: {e}");
            return 2;
        }
    };
    match bundle.get("localharness_agent_state").and_then(|v| v.as_u64()) {
        Some(BUNDLE_VERSION) => {}
        Some(v) => {
            eprintln!("{path} is bundle version {v}; this build writes version {BUNDLE_VERSION}");
            return 2;
        }
        None => {
            eprintln!("{path} is not a localharness agent-state bundle");
            return 2;
        }
    }
    import_bundle_slots(name, id, &bundle, path).await
}

/// The shared diff-write half of both import shapes (file bundle + `.agent/`
/// directory): sanitize both sides through the pure cores, write ONE sponsored
/// tx per slot that actually differs (see the module note on why never batched).
async fn import_bundle_slots(
    name: &str,
    id: u64,
    bundle: &serde_json::Value,
    source: &str,
) -> i32 {
    let mut wrote = 0;
    let mut failed = 0;
    for slot in [Slot::Persona, Slot::Lessons, Slot::Skills] {
        let Some(raw) = bundle.get(slot.field()).and_then(|v| v.as_str()) else {
            continue;
        };
        let desired = slot.sanitize(raw);
        if desired.is_empty() {
            continue; // absent/blank in the bundle = leave the slot alone
        }
        let existing = slot.sanitize(&read_slot(id, slot).await.unwrap_or_default());
        if desired == existing {
            println!("{} unchanged — skipping", slot.field());
            continue;
        }
        if write_slot(name, id, slot, &desired).await == 0 {
            wrote += 1;
        } else {
            failed += 1;
        }
    }
    if failed > 0 {
        eprintln!("{wrote} slot(s) written, {failed} failed");
        return 1;
    }
    println!("✓ {name} imported {wrote} slot(s) from {source}");
    0
}

/// The three on-chain agent-state slots. Each knows its bundle field, its
/// `setMetadata` encoder, and how to sanitize a blob through the pure core
/// that owns its format.
#[derive(Clone, Copy)]
enum Slot {
    Persona,
    Lessons,
    Skills,
}

impl Slot {
    fn field(self) -> &'static str {
        match self {
            Slot::Persona => "persona",
            Slot::Lessons => "lessons",
            Slot::Skills => "skills",
        }
    }

    /// Normalize a blob the way its core does, so comparisons are between
    /// canonical forms and a no-op write is recognized as one.
    fn sanitize(self, raw: &str) -> String {
        match self {
            Slot::Persona => raw.trim().to_string(),
            Slot::Lessons => lessons::replace_all(raw),
            Slot::Skills => {
                let parsed = skills::parse(raw);
                if parsed.is_empty() {
                    String::new()
                } else {
                    skills::serialize(&parsed)
                }
            }
        }
    }

    fn encode(self, id: u64, blob: &str) -> Vec<u8> {
        match self {
            Slot::Persona => registry::encode_set_persona(id, blob),
            Slot::Lessons => registry::encode_set_lessons(id, blob),
            Slot::Skills => registry::encode_set_skills(id, blob),
        }
    }
}

/// Read one slot; `None` on an RPC failure (reported), empty when unset.
async fn read_slot(id: u64, slot: Slot) -> Option<String> {
    let read = match slot {
        Slot::Persona => registry::persona_of(id).await,
        Slot::Lessons => registry::lessons_of(id).await,
        Slot::Skills => registry::skills_of(id).await,
    };
    match read {
        Ok(v) => Some(v.unwrap_or_default()),
        Err(e) => {
            eprintln!("RPC error reading {}: {e}", slot.field());
            None
        }
    }
}

/// Resolve `<name>` to its tokenId. Pure read — the show/export paths need no
/// key at all, so inspecting another agent's published state always works.
async fn token_id(name: &str) -> Result<u64, i32> {
    match registry::id_of_name(name).await {
        Ok(id) if id != 0 => Ok(id),
        Ok(_) => {
            eprintln!("{name} is not registered — run `localharness create {name}` first");
            Err(1)
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            Err(1)
        }
    }
}

/// Owner-gated sponsored write of ONE slot, mirroring `publish::set_persona`.
async fn write_slot(name: &str, id: u64, slot: Slot, blob: &str) -> i32 {
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
        input: slot.encode(id, blob),
    }];
    let sponsor = match load_sponsor() {
        Ok(s) => s,
        Err(code) => return code,
    };
    println!(
        "writing {}-byte {} for {name}.localharness.xyz …",
        blob.len(),
        slot.field()
    );
    match registry::submit_tempo_sponsored(
        &signer,
        &sponsor,
        calls,
        registry::ALPHA_USD_ADDRESS(),
        registry::set_metadata_gas(blob.len()),
    )
    .await
    {
        Ok(tx) => {
            println!("✓ {} updated", slot.field());
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("{} write failed: {e}", slot.field());
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Slot;

    /// A round-trip through `sanitize` is idempotent for every slot — that is
    /// what makes "desired == existing → skip the tx" a safe gas check rather
    /// than a coin flip on formatting.
    #[test]
    fn sanitize_is_idempotent() {
        let cases = [
            (Slot::Persona, "  be helpful  "),
            (Slot::Lessons, "one lesson\n\none lesson\ntwo\n"),
            (
                Slot::Skills,
                r#"[{"name":"Deploy ","instructions":"run it"}]"#,
            ),
        ];
        for (slot, raw) in cases {
            let once = slot.sanitize(raw);
            assert_eq!(once, slot.sanitize(&once), "{} not idempotent", slot.field());
        }
    }

    /// An unparseable or empty skills blob sanitizes to EMPTY, which the import
    /// path reads as "leave the slot alone" — never as "write `[]` on-chain".
    #[test]
    fn unusable_skills_blob_is_empty_not_a_write() {
        assert!(Slot::Skills.sanitize("not json").is_empty());
        assert!(Slot::Skills.sanitize("[]").is_empty());
        assert!(Slot::Lessons.sanitize("   \n  ").is_empty());
    }
}
