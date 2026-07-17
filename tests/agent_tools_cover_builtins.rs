//! The tool lists must not drift apart again (telemetry #76).
//!
//! `AGENT_TOOLS` (the published surface) and `BuiltinTool::ALL` (the builtin
//! enum) are hand-kept. The admin allowlist grid renders their UNION and SAVES
//! what's ticked; `closure_tool_allowed` then denies any tool the saved list
//! doesn't NAME. So a tool missing from the grid isn't a doc nit — ticking
//! everything and pressing save silently revokes it. These guards pin the
//! invariants that make the grid's union trustworthy.

use localharness::agent_tools::AGENT_TOOLS;

fn all_names() -> Vec<&'static str> {
    AGENT_TOOLS.iter().flat_map(|(_, tools)| tools.iter().copied()).collect()
}

/// No tool may appear twice — the grid would render two checkboxes for it and
/// the "every box ticked = unrestricted" count would never reconcile.
#[test]
fn agent_tools_has_no_duplicates() {
    let names = all_names();
    let mut seen = std::collections::HashSet::new();
    for name in &names {
        assert!(seen.insert(*name), "{name} is listed twice in AGENT_TOOLS");
    }
}

/// Every group carries tools, and every name is a plausible wire name — the
/// grid's `data-tool` attribute and the saved allowlist match on it verbatim.
#[test]
fn agent_tool_names_are_wire_shaped() {
    for (group, tools) in AGENT_TOOLS {
        assert!(!tools.is_empty(), "tool group {group:?} is empty");
        for name in *tools {
            assert!(
                !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{name:?} in {group:?} is not a snake_case wire name"
            );
        }
    }
}

/// The plan checklist is what keeps a multi-phase run alive (telemetry
/// #75/#69/#67). If it ever falls out of the published surface it also falls out
/// of the allowlist grid, and a restrictive save would strip it.
#[test]
fn update_plan_is_a_published_tool() {
    assert!(all_names().contains(&"update_plan"));
}
