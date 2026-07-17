//! The canonical agent tool surface — ONE list, read by every consumer.
//!
//! Lives here rather than in `docs_manifest` because that module is
//! `wallet` + non-wasm only (it renders testnet chain strings, which must stay
//! out of the prod browser bundle). The tool NAMES have no such constraint, and
//! the admin allowlist grid needs them on wasm: sourcing that grid from a
//! hand-kept second list is what let it drift to 19 of ~90 tools and silently
//! revoke the rest on save (telemetry #76).

/// The canonical agent tool surface, grouped by family. Single-sourced here so
/// every doc renders the SAME list. (A future enhancement can derive these from
/// the builtin/platform registries; for now single-sourcing the LIST is the
/// win.) Each entry is `(group, [tool, ...])`.
pub const AGENT_TOOLS: &[(&str, &[&str])] = &[
    (
        "Filesystem (OPFS sandbox)",
        &[
            "list_directory",
            "view_file",
            "find_file",
            "search_directory",
            "create_file",
            "edit_file",
            "delete_file",
            "rename_file",
        ],
    ),
    (
        "Platform / subdomains",
        &[
            "create_subdomain",
            "batch_create_subdomains",
            "create_and_publish_app",
            "publish_app_to",
            "publish_public_face",
            "list_subdomains",
            "release_subdomain",
            "bulk_release_subdomains",
        ],
    ),
    (
        "Agents / orchestration",
        &[
            "call_agent",
            "discover_agents",
            "consult_model",
            "start_subagent",
            "spawn_recursive_subagent",
            "schedule_task",
            "cancel_task",
        ],
    ),
    (
        "Payments / economy",
        &[
            "send_lh",
            "batch_send_lh",
            "check_balances",
            "query_balance",
            "post_bounty",
            "claim_bounty",
            "submit_result",
            "accept_result",
            "discover_bounties",
            "create_guild",
            "invite_to_guild",
            "fund_guild",
            "spend_treasury",
            "set_role",
            "company_status",
            "found_company",
            "attest",
            "propose_measure",
            "cast_vote",
            "execute_proposal",
            "list_proposals",
        ],
    ),
    (
        "Self-edit / learning",
        &[
            "set_persona",
            "update_plan",
            "record_lesson",
            "consolidate_lessons",
            "set_lessons",
            "create_skill",
            "list_skills",
            "delete_skill",
        ],
    ),
    (
        "Build / run",
        &[
            "compile_rustlite",
            "run_cartridge",
            "render_html",
            "run_wasm_cli",
            "execute_script",
            "generate_image",
        ],
    ),
    (
        "Multi-chain reads",
        &["evm_chains", "evm_balance", "resolve_ens", "evm_call"],
    ),
    (
        "Grounding / I/O",
        &[
            "web_fetch",
            "notify",
            "list_notifications",
            "clear_notifications",
            "submit_feedback",
            "read_self_docs",
            "current_time",
            "ask_question",
            "finish",
            "dwell",
            "clear_context",
            "compact_context",
        ],
    ),
];
