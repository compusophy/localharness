//! Declarative tool-execution policy engine.
//!
//! Policies are matched against incoming `ToolCall`s and combined according
//! to a strict precedence table. The Python SDK encodes the same rule:
//!
//! ```text
//!   priority   bucket
//!   --------   ----------------------------------------
//!     0        specific-tool DENY
//!     1        specific-tool ASK_USER
//!     2        specific-tool APPROVE
//!     3        wildcard ("*") DENY
//!     4        wildcard ASK_USER
//!     5        wildcard APPROVE
//! ```
//!
//! Within a bucket the first registered policy whose `when` predicate fires
//! wins. The engine short-circuits at the first match.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;

use crate::error::{Error, Result};
use crate::hooks::{OperationContext, PreToolCallDecideHook};
use crate::types::{HookResult, ToolCall};

/// What a policy decides about a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Allow the tool call to proceed.
    Approve,
    /// Block the tool call.
    Deny,
    /// Prompt the user for confirmation.
    AskUser,
}

/// A closure that tests whether a policy applies to a given tool call.
pub type Predicate = Arc<dyn Fn(&ToolCall) -> bool + Send + Sync>;
/// A closure that prompts the user and returns `true` if approved.
pub type AskUserHandler = Arc<dyn Fn(&ToolCall) -> bool + Send + Sync>;

/// A declarative rule governing whether a tool call is allowed.
pub struct Policy {
    /// Tool name this policy targets, or `"*"` for wildcard.
    pub tool: String,
    /// What to do when the policy matches.
    pub decision: Decision,
    /// Optional predicate; when `None` the policy always matches.
    pub when: Option<Predicate>,
    /// Handler called when `decision` is `AskUser`.
    pub ask_user: Option<AskUserHandler>,
    /// Human-readable policy name for diagnostics.
    pub name: String,
}

impl Policy {
    /// Create an approval policy for a specific tool.
    pub fn allow(tool: impl Into<String>) -> Self {
        Self {
            tool: tool.into(),
            decision: Decision::Approve,
            when: None,
            ask_user: None,
            name: "allow".to_string(),
        }
    }

    /// Create a denial policy for a specific tool.
    pub fn deny(tool: impl Into<String>) -> Self {
        Self {
            tool: tool.into(),
            decision: Decision::Deny,
            when: None,
            ask_user: None,
            name: "deny".to_string(),
        }
    }

    /// Create an ask-user policy with a confirmation handler.
    pub fn ask(tool: impl Into<String>, handler: AskUserHandler) -> Self {
        Self {
            tool: tool.into(),
            decision: Decision::AskUser,
            when: None,
            ask_user: Some(handler),
            name: "ask".to_string(),
        }
    }

    /// Attach a predicate that narrows when this policy fires.
    pub fn with_predicate(mut self, predicate: Predicate) -> Self {
        self.when = Some(predicate);
        self
    }

    /// Set the diagnostic name for this policy.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// True if this policy targets all tools (`"*"`).
    pub fn is_wildcard(&self) -> bool {
        self.tool == "*"
    }
}

/// Wildcard policy that approves every tool call.
pub fn allow_all() -> Policy {
    Policy::allow("*").with_name("allow_all")
}

/// Wildcard policy that denies every tool call.
pub fn deny_all() -> Policy {
    Policy::deny("*").with_name("deny_all")
}

// =============================================================================
// Path containment
// =============================================================================

/// Normalize a path. On Windows/macOS we lower-case the canonical form for
/// comparisons because those filesystems are case-insensitive. Symlinks are
/// resolved; relative paths are joined with the current dir before
/// canonicalization.
pub fn secure_normalize_path(path: impl AsRef<Path>) -> Result<PathBuf> {
    let path = path.as_ref();
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    if let Ok(p) = dunce::canonicalize(&absolute) {
        return Ok(p);
    }
    // Target can't be canonicalized (e.g. a not-yet-created file). Resolve
    // the PARENT instead — that collapses `..` and follows symlinks — and
    // re-join the final component, so containment always compares a
    // canonical path against a canonical workspace (and `<ws>/../../etc`
    // resolves to its real, outside location rather than slipping past the
    // component check).
    if let (Some(parent), Some(file)) = (absolute.parent(), absolute.file_name()) {
        if let Ok(canon_parent) = dunce::canonicalize(parent) {
            return Ok(canon_parent.join(file));
        }
    }
    // Even the parent doesn't resolve. Refuse to fall back to a path that
    // still carries `..` / `.` traversal components — fail closed.
    if absolute.components().any(|c| {
        matches!(
            c,
            std::path::Component::ParentDir | std::path::Component::CurDir
        )
    }) {
        return Err(Error::other(
            "path contains unresolved `..`/`.` traversal components",
        ));
    }
    Ok(absolute)
}

/// Returns `true` if `target` is under `workspace`. Component-wise comparison
/// — never a string prefix — to defeat `/foo/bar-evil` vs `/foo/bar`.
pub fn is_path_in_workspace(target: impl AsRef<Path>, workspace: impl AsRef<Path>) -> bool {
    let (Ok(t), Ok(w)) = (
        secure_normalize_path(target.as_ref()),
        secure_normalize_path(workspace.as_ref()),
    ) else {
        return false;
    };
    let case_insensitive = cfg!(any(windows, target_os = "macos"));
    let t_comps: Vec<_> = t.components().collect();
    let w_comps: Vec<_> = w.components().collect();
    if t_comps.len() < w_comps.len() {
        return false;
    }
    t_comps
        .iter()
        .zip(w_comps.iter())
        .all(|(a, b)| component_eq(a, b, case_insensitive))
}

fn component_eq(
    a: &std::path::Component<'_>,
    b: &std::path::Component<'_>,
    case_insensitive: bool,
) -> bool {
    let as_str = |c: &std::path::Component<'_>| c.as_os_str().to_string_lossy().into_owned();
    let (sa, sb) = (as_str(a), as_str(b));
    if case_insensitive {
        sa.eq_ignore_ascii_case(&sb)
    } else {
        sa == sb
    }
}

/// Every built-in filesystem tool that takes a path the model controls.
/// `workspace_only` must attach a containment deny to ALL of them —
/// missing one (historically `delete_file` / `rename_file` and the
/// traversal tools) leaves a full escape hatch.
const FS_TOOLS: &[&str] = &[
    "view_file",
    "create_file",
    "edit_file",
    "delete_file",
    "rename_file",
    "list_directory",
    "find_file",
    "search_directory",
];

/// Extract every filesystem path a built-in fs tool call operates on, so
/// containment can check ALL of them — notably `rename_file`'s `from`
/// AND `to`, both of which must stay inside the workspace. Reads the raw
/// arg strings; canonicalisation + containment are
/// [`is_path_in_workspace`]'s job.
fn fs_paths_from_args(tool: &str, args: &serde_json::Value) -> Vec<String> {
    let get = |k: &str| args.get(k).and_then(|v| v.as_str()).map(str::to_string);
    match tool {
        "rename_file" => get("from").into_iter().chain(get("to")).collect(),
        // All other fs tools operate on a single `path` (a file for
        // view/create/edit/delete, a directory for list/find/search).
        _ => get("path").into_iter().collect(),
    }
}

/// Builds policies that deny file-tool calls outside the given
/// workspaces. Covers every tool in `FS_TOOLS` and fails **closed**:
/// an fs tool call whose paths can't be resolved, or that touches any
/// path outside every workspace, is denied. `rename_file` is checked on
/// both `from` and `to`.
pub fn workspace_only(workspaces: Vec<PathBuf>) -> Vec<Policy> {
    let workspaces = Arc::new(workspaces);
    FS_TOOLS
        .iter()
        .map(|tool| {
            let workspaces = workspaces.clone();
            let predicate: Predicate = Arc::new(move |tc: &ToolCall| {
                let paths = fs_paths_from_args(&tc.name, &tc.args);
                if paths.is_empty() {
                    // A path-bearing fs tool with no usable path arg is
                    // malformed — fail closed (deny) rather than waving it
                    // through as the old `canonical_path: None` branch did.
                    return true;
                }
                // Deny if ANY operand escapes every configured workspace.
                paths
                    .iter()
                    .any(|p| !workspaces.iter().any(|w| is_path_in_workspace(p, w)))
            });
            Policy::deny(*tool)
                .with_predicate(predicate)
                .with_name(format!("workspace_only:{tool}"))
        })
        .collect()
}

// =============================================================================
// Evaluation
// =============================================================================

/// Evaluate `policies` against `call` using the precedence table. Returns the decision.
pub fn evaluate(policies: &[Policy], call: &ToolCall) -> HookResult {
    if policies.is_empty() {
        return HookResult::allow_with("no policies configured");
    }

    let mut buckets: [Vec<&Policy>; 6] = Default::default();
    for p in policies {
        if p.tool != call.name && !p.is_wildcard() {
            continue;
        }
        let idx = match (p.decision, p.is_wildcard()) {
            (Decision::Deny, false) => 0,
            (Decision::AskUser, false) => 1,
            (Decision::Approve, false) => 2,
            (Decision::Deny, true) => 3,
            (Decision::AskUser, true) => 4,
            (Decision::Approve, true) => 5,
        };
        buckets[idx].push(p);
    }

    for bucket in &buckets {
        for p in bucket {
            let matches = p.when.as_ref().map(|pred| pred(call)).unwrap_or(true);
            if !matches {
                continue;
            }
            return match p.decision {
                Decision::Deny => HookResult::deny(format!("denied by policy '{}'", p.name)),
                Decision::Approve => {
                    HookResult::allow_with(format!("approved by policy '{}'", p.name))
                }
                Decision::AskUser => match &p.ask_user {
                    Some(handler) => {
                        let approved = handler(call);
                        if approved {
                            HookResult::allow_with(format!(
                                "user approved via policy '{}'",
                                p.name
                            ))
                        } else {
                            HookResult::deny(format!("user denied via policy '{}'", p.name))
                        }
                    }
                    None => HookResult::deny(format!(
                        "policy '{}' marked ask_user but no handler",
                        p.name
                    )),
                },
            };
        }
    }

    HookResult::deny("no matching policy")
}

// =============================================================================
// Hook adapter
// =============================================================================

/// Returns a `PreToolCallDecideHook` that enforces the policy list. Mirrors
/// `policy.enforce(active_policies)` in Python.
pub fn enforce(policies: Vec<Policy>) -> Arc<dyn PreToolCallDecideHook> {
    Arc::new(PolicyEnforcer {
        policies: RwLock::new(policies),
    })
}

struct PolicyEnforcer {
    policies: RwLock<Vec<Policy>>,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl PreToolCallDecideHook for PolicyEnforcer {
    fn name(&self) -> &str {
        "policy::enforce"
    }
    async fn run(&self, _ctx: &OperationContext, call: &ToolCall) -> Result<HookResult> {
        let policies = self.policies.read();
        Ok(evaluate(&policies, call))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str) -> ToolCall {
        ToolCall {
            name: name.to_string(),
            args: serde_json::json!({}),
            id: None,
            canonical_path: None,
        }
    }

    fn call_args(name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            name: name.to_string(),
            args,
            id: None,
            canonical_path: None,
        }
    }

    #[test]
    fn specific_deny_beats_wildcard_allow() {
        let policies = vec![
            allow_all(),
            Policy::deny("run_command").with_name("block_commands"),
        ];
        assert!(!evaluate(&policies, &call("run_command")).allow);
        assert!(evaluate(&policies, &call("view_file")).allow);
    }

    #[test]
    fn empty_policies_means_allow() {
        let policies: Vec<Policy> = Vec::new();
        assert!(evaluate(&policies, &call("anything")).allow);
    }

    #[test]
    fn autonomous_loop_deny_by_default_allowlist_enforces_at_dispatch() {
        // Roadmap Track B / Phase 0b: an autonomous QA agent registers
        // deny-by-default + an explicit allowlist of its read-only qa_* tools.
        // Anything off the list — a write tool, an off-list builtin, or a
        // model-hallucinated/injected tool name — is DENIED at dispatch. This
        // is what makes 0b's "custom tools require a policy" real enforcement
        // rather than a prompt-level honor system the model can be talked past.
        let policies = vec![
            deny_all(),
            Policy::allow("qa_compile"),
            Policy::allow("qa_chain"),
        ];
        assert!(evaluate(&policies, &call("qa_compile")).allow);
        assert!(evaluate(&policies, &call("qa_chain")).allow);
        assert!(!evaluate(&policies, &call("qa_publish")).allow, "off-list write tool denied");
        assert!(!evaluate(&policies, &call("run_command")).allow, "off-list builtin denied");
        assert!(!evaluate(&policies, &call("hallucinated_tool")).allow, "unknown tool denied");
    }

    // Realistic composition: a base allow + workspace containment denies.
    // `workspace_only` is deny-only, so on its own every non-matching call
    // hits the default-deny; in practice it's paired with an allow/approve
    // (the specific-deny still beats the wildcard-approve, bucket 0 < 5).
    fn ws_policies(cwd: PathBuf) -> Vec<Policy> {
        let mut v = vec![allow_all()];
        v.extend(workspace_only(vec![cwd]));
        v
    }

    #[test]
    fn workspace_allows_inside() {
        let cwd = std::env::current_dir().unwrap();
        let policies = ws_policies(cwd);
        // A relative path resolves under cwd → inside the workspace.
        let inside = call_args("view_file", serde_json::json!({ "path": "some_file.txt" }));
        assert!(evaluate(&policies, &inside).allow);
    }

    #[test]
    fn workspace_blocks_outside() {
        let cwd = std::env::current_dir().unwrap();
        let policies = ws_policies(cwd);
        let outside = call_args(
            "view_file",
            serde_json::json!({ "path": "/totally/elsewhere/file.txt" }),
        );
        assert!(!evaluate(&policies, &outside).allow);
    }

    #[test]
    fn workspace_covers_delete_file() {
        // Regression: delete_file used to have NO containment policy.
        let cwd = std::env::current_dir().unwrap();
        let policies = ws_policies(cwd);
        let del = call_args("delete_file", serde_json::json!({ "path": "/etc/shadow" }));
        assert!(!evaluate(&policies, &del).allow, "delete outside must be denied");
    }

    #[test]
    fn workspace_checks_both_rename_operands() {
        // Regression: rename_file (from/to) had no containment, and the
        // single-`path` extraction never saw its args.
        let cwd = std::env::current_dir().unwrap();
        let policies = ws_policies(cwd);

        let escape_dst = call_args(
            "rename_file",
            serde_json::json!({ "from": "a.txt", "to": "/etc/evil" }),
        );
        assert!(!evaluate(&policies, &escape_dst).allow, "rename to outside must be denied");

        let both_inside = call_args(
            "rename_file",
            serde_json::json!({ "from": "a.txt", "to": "b.txt" }),
        );
        assert!(evaluate(&policies, &both_inside).allow, "rename within ws should be allowed");
    }

    #[test]
    fn workspace_blocks_dotdot_traversal() {
        let cwd = std::env::current_dir().unwrap();
        let policies = ws_policies(cwd);
        let trav = call_args(
            "create_file",
            serde_json::json!({ "path": "../../../../etc/passwd" }),
        );
        assert!(!evaluate(&policies, &trav).allow, "`..` traversal must be denied");
    }

    #[test]
    fn workspace_fails_closed_on_missing_path() {
        // An fs tool call with no usable path arg is denied, not allowed —
        // even with a base allow present, the containment deny fires.
        let cwd = std::env::current_dir().unwrap();
        let policies = ws_policies(cwd);
        assert!(!evaluate(&policies, &call("delete_file")).allow);
    }
}
