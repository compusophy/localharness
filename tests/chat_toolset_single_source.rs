//! Source guard (R4): `src/app/chat/session.rs` must assemble the chat tool
//! surface in ONE place — the `chat_toolset` fn + the `wire_shared_session!`
//! macro — never per-backend lists again (the Anthropic/Gemini branches once
//! carried two hand-copied ~70-tool lists that drifted).
//!
//! `src/app` is wasm32-only (`cfg(all(feature="browser-app", target_arch="wasm32"))`)
//! so this can't unit-test the assembly on a native target; like
//! `data_action_dispatch.rs` it checks the SOURCE as text, which runs natively
//! on every `cargo test`. Skips cleanly if `src/app` isn't present.

use std::path::Path;

#[test]
fn session_assembles_tools_in_one_place() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app/chat/session.rs");
    if !p.exists() {
        eprintln!("skip: {} not present (packaged crate?)", p.display());
        return;
    }
    let src = std::fs::read_to_string(&p).expect("read src/app/chat/session.rs");

    // Exactly ONE `.with_tool(` call site — the loop inside `wire_shared_session!`.
    // A second site means a backend branch grew its own tool registration again.
    let with_tool_sites = src.matches(".with_tool(").count();
    assert_eq!(
        with_tool_sites, 1,
        "session.rs must register tools ONLY via the wire_shared_session! loop \
         (found {with_tool_sites} `.with_tool(` sites) — add new chat tools to \
         chat_toolset(), not to a backend branch"
    );

    // Exactly ONE toolset definition, consumed by BOTH backend branches.
    assert_eq!(src.matches("fn chat_toolset(").count(), 1, "one chat_toolset definition");
    let mentions = src.matches("chat_toolset(").count();
    assert!(
        mentions >= 3, // the definition + one call site per backend branch
        "both backend branches must build their tools via chat_toolset() \
         (found {mentions} `chat_toolset(` occurrences, need the def + 2 calls)"
    );

    // Spot-check the single list is still populated (extractor sanity).
    for t in ["create_subdomain_tool()", "spawn_recursive_subagent_tool(key"] {
        assert_eq!(src.matches(t).count(), 1, "`{t}` must appear exactly once (in chat_toolset)");
    }
}
