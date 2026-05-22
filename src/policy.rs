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

use crate::error::Result;
use crate::hooks::{OperationContext, PreToolCallDecideHook};
use crate::types::{HookResult, ToolCall};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Approve,
    Deny,
    AskUser,
}

pub type Predicate = Arc<dyn Fn(&ToolCall) -> bool + Send + Sync>;
pub type AskUserHandler = Arc<dyn Fn(&ToolCall) -> bool + Send + Sync>;

pub struct Policy {
    pub tool: String,
    pub decision: Decision,
    pub when: Option<Predicate>,
    pub ask_user: Option<AskUserHandler>,
    pub name: String,
}

impl Policy {
    pub fn allow(tool: impl Into<String>) -> Self {
        Self {
            tool: tool.into(),
            decision: Decision::Approve,
            when: None,
            ask_user: None,
            name: "allow".to_string(),
        }
    }

    pub fn deny(tool: impl Into<String>) -> Self {
        Self {
            tool: tool.into(),
            decision: Decision::Deny,
            when: None,
            ask_user: None,
            name: "deny".to_string(),
        }
    }

    pub fn ask(tool: impl Into<String>, handler: AskUserHandler) -> Self {
        Self {
            tool: tool.into(),
            decision: Decision::AskUser,
            when: None,
            ask_user: Some(handler),
            name: "ask".to_string(),
        }
    }

    pub fn with_predicate(mut self, predicate: Predicate) -> Self {
        self.when = Some(predicate);
        self
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn is_wildcard(&self) -> bool {
        self.tool == "*"
    }
}

pub fn allow_all() -> Policy {
    Policy::allow("*").with_name("allow_all")
}

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
    let canonical = match dunce::canonicalize(&absolute) {
        Ok(p) => p,
        Err(_) => absolute,
    };
    Ok(canonical)
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

/// Builds policies that deny file-tool calls outside the given workspaces.
pub fn workspace_only(workspaces: Vec<PathBuf>) -> Vec<Policy> {
    let workspaces = Arc::new(workspaces);
    let predicate: Predicate = {
        let workspaces = workspaces.clone();
        Arc::new(move |tc: &ToolCall| {
            let Some(p) = tc.canonical_path.as_ref() else {
                // No path: caller can't be claiming a workspace violation.
                return false;
            };
            !workspaces.iter().any(|w| is_path_in_workspace(p, w))
        })
    };

    vec![
        Policy::deny("view_file")
            .with_predicate(predicate.clone())
            .with_name("workspace_only:view_file"),
        Policy::deny("create_file")
            .with_predicate(predicate.clone())
            .with_name("workspace_only:create_file"),
        Policy::deny("edit_file")
            .with_predicate(predicate)
            .with_name("workspace_only:edit_file"),
    ]
}

// =============================================================================
// Evaluation
// =============================================================================

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
    fn workspace_predicate_blocks_outside() {
        let cwd = std::env::current_dir().unwrap();
        let ws = vec![cwd.clone()];
        let policies = workspace_only(ws);
        let mut outside = call("view_file");
        outside.canonical_path = Some("/totally/elsewhere/file.txt".to_string());
        let result = evaluate(&policies, &outside);
        assert!(!result.allow, "outside path should be denied: {:?}", result);
    }
}
