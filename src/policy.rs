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
/// A closure that decides whether a matching tool call is approved: return
/// `true` to approve, `false` to deny. It is the SDK's human-in-the-loop hook —
/// the SDK calls it where a confirmation prompt would go; the closure itself
/// returns the decision (it does not prompt).
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

    /// Create an ask-user policy: `handler` decides each matching call, returning
    /// `true` to approve or `false` to deny (see [`AskUserHandler`]). This is the
    /// human-in-the-loop hook — surface the [`ToolCall`] to the user however you
    /// like and return their choice.
    ///
    /// # Examples
    /// ```
    /// use std::sync::Arc;
    /// use localharness::policy::{Policy, AskUserHandler};
    ///
    /// // Approve read-only tools, deny writes. A real handler would prompt the user.
    /// let handler: AskUserHandler = Arc::new(|call| !call.name.contains("_file"));
    /// let _policy = Policy::ask("edit_file", handler);
    /// ```
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

    // -------------------------------------------------------------------------
    // Sandbox-escape coverage. These exercise `is_path_in_workspace` /
    // `secure_normalize_path` directly (pure-path logic, no policy plumbing)
    // so each escape vector is asserted in isolation, plus a couple driven
    // through the full `evaluate` gate. Filesystem cases (sibling prefix,
    // symlink) build a real temp tree but stay fast + self-cleaning.
    // -------------------------------------------------------------------------

    fn unique_tmp(label: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("lh_pol_{label}_{}", uuid::Uuid::new_v4()));
        p
    }

    /// The CLASSIC prefix-matching bug: workspace `/foo/proj`, target
    /// `/foo/proj-evil/secret`. A naive `target.starts_with(workspace)`
    /// string match would WRONGLY admit the sibling. The component-wise
    /// comparison must reject it. Covered for: the file existing, a ghost
    /// file under an existing sibling parent, and a ghost path whose parent
    /// is also missing (so `secure_normalize_path` falls back to the raw
    /// absolute path — the prefix trap is most tempting there).
    #[test]
    fn sibling_with_shared_prefix_is_outside() {
        let ws = unique_tmp("ws");
        std::fs::create_dir_all(&ws).unwrap();
        // Sibling sharing the workspace's full path as a string prefix.
        let sibling = PathBuf::from(format!("{}-evil", ws.display()));
        std::fs::create_dir_all(&sibling).unwrap();
        let secret = sibling.join("secret.txt");
        std::fs::write(&secret, b"top secret").unwrap();

        assert!(
            !is_path_in_workspace(&secret, &ws),
            "existing sibling `<ws>-evil/secret.txt` must be OUTSIDE (prefix-match trap)"
        );
        assert!(
            !is_path_in_workspace(sibling.join("ghost.txt"), &ws),
            "ghost file under existing sibling parent must be OUTSIDE"
        );
        assert!(
            !is_path_in_workspace(
                PathBuf::from(format!("{}-evil/no/such/dir/ghost.txt", ws.display())),
                &ws
            ),
            "ghost file whose parent is also missing must be OUTSIDE (raw-absolute fallback)"
        );
        // Sanity: a real child IS inside.
        assert!(
            is_path_in_workspace(ws.join("child.txt"), &ws),
            "a genuine child must be inside"
        );

        std::fs::remove_dir_all(&ws).ok();
        std::fs::remove_dir_all(&sibling).ok();
    }

    /// `..` segments that ESCAPE the workspace are rejected even when the
    /// target doesn't exist (the parent is canonicalized, collapsing `..`).
    /// `..` segments that STAY inside are allowed — a legit operation.
    #[test]
    fn dotdot_escapes_denied_but_inward_dotdot_allowed() {
        let ws = unique_tmp("dd");
        std::fs::create_dir_all(ws.join("sub")).unwrap();
        std::fs::write(ws.join("inside.txt"), b"x").unwrap();

        // ws/sub/../inside.txt collapses to ws/inside.txt → inside.
        let inward = ws.join("sub").join("..").join("inside.txt");
        assert!(
            is_path_in_workspace(&inward, &ws),
            "`..` that stays within the workspace must be allowed"
        );

        // ws/../<escape> climbs out → outside.
        let outward = ws.join("..").join("lh_pol_escape_target");
        assert!(
            !is_path_in_workspace(&outward, &ws),
            "`..` that climbs out of the workspace must be denied"
        );

        // A long climb to a real system path is denied.
        assert!(
            !is_path_in_workspace(
                format!("{}/../../../../../../etc/passwd", ws.display()),
                &ws
            ),
            "deep `..` traversal to /etc/passwd must be denied"
        );

        std::fs::remove_dir_all(&ws).ok();
    }

    /// Windows-style backslash traversal (`<ws>\..\..\Windows\System32`) must
    /// be rejected. `dunce::canonicalize` + component comparison treat `\` as
    /// a separator on Windows; on Unix the whole thing is one weird filename
    /// under the (existing) workspace, which is *inside* — so only assert the
    /// escape on Windows, where the separator is real.
    #[test]
    #[cfg(windows)]
    fn windows_backslash_traversal_denied() {
        let ws = unique_tmp("bs");
        std::fs::create_dir_all(&ws).unwrap();
        let bs = "\\";
        let trav = format!(
            "{ws}{bs}..{bs}..{bs}Windows{bs}System32{bs}config{bs}SAM",
            ws = ws.display()
        );
        assert!(
            !is_path_in_workspace(&trav, &ws),
            "backslash `..` traversal must escape-deny on Windows"
        );
        std::fs::remove_dir_all(&ws).ok();
    }

    /// On a case-insensitive filesystem (Windows / macOS) a path that differs
    /// only in case from the workspace must still be recognized as INSIDE, so
    /// an attacker can't dodge containment by changing case. On a
    /// case-sensitive FS it's legitimately a different (nonexistent) path; we
    /// only assert the case-insensitive platforms.
    #[test]
    #[cfg(any(windows, target_os = "macos"))]
    fn case_variant_still_inside_on_case_insensitive_fs() {
        let ws = unique_tmp("case");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("inside.txt"), b"x").unwrap();
        let upper =
            PathBuf::from(ws.display().to_string().to_uppercase()).join("inside.txt");
        assert!(
            is_path_in_workspace(&upper, &ws),
            "case-variant path must still resolve as inside the workspace"
        );
        std::fs::remove_dir_all(&ws).ok();
    }

    /// An empty path string must NOT be treated as inside any workspace —
    /// `secure_normalize_path("")` joins it onto cwd (yielding cwd), so the
    /// only risk is mis-treating it as the workspace root. Assert it's denied
    /// for a workspace that is NOT the cwd.
    #[test]
    fn empty_path_is_not_inside_arbitrary_workspace() {
        let ws = unique_tmp("empty");
        std::fs::create_dir_all(&ws).unwrap();
        assert!(
            !is_path_in_workspace("", &ws),
            "empty path must not be admitted into an arbitrary workspace"
        );
        std::fs::remove_dir_all(&ws).ok();
    }

    /// `rename_file` must check BOTH operands. The existing
    /// `workspace_checks_both_rename_operands` covers an escaping `to`; this
    /// covers an escaping `from` (exfiltrate-by-moving-out is symmetric — a
    /// rename FROM outside INTO the workspace would let the agent pull an
    /// arbitrary file in). Both directions must be denied.
    #[test]
    fn rename_denies_when_from_escapes() {
        let cwd = std::env::current_dir().unwrap();
        let policies = ws_policies(cwd);
        let escape_src = call_args(
            "rename_file",
            serde_json::json!({ "from": "/etc/passwd", "to": "stolen.txt" }),
        );
        assert!(
            !evaluate(&policies, &escape_src).allow,
            "rename whose `from` is outside the workspace must be denied"
        );
    }

    /// A symlink that lives INSIDE the workspace but points OUTSIDE must not
    /// become an escape hatch: accessing a file *through* it has to be denied.
    /// `secure_normalize_path` canonicalizes (resolving the symlink) BEFORE the
    /// containment check, so the resolved real path lands outside and is
    /// rejected. Symlink creation needs privileges on Windows (Developer Mode)
    /// — when it fails we skip rather than fail, so the test is a real proof on
    /// Unix/CI and a no-op elsewhere.
    #[test]
    fn symlink_inside_pointing_outside_is_denied() {
        let ws = unique_tmp("symws");
        let outside = unique_tmp("symout");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), b"TOPSECRET").unwrap();

        let link = ws.join("escape_link");
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(&outside, &link).is_ok();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&outside, &link).is_ok();
        #[cfg(not(any(windows, unix)))]
        let made = false;

        if made {
            let via_link = link.join("secret.txt");
            assert!(
                !is_path_in_workspace(&via_link, &ws),
                "reading an outside file through an in-workspace symlink must be denied \
                 (canonicalize resolves the link before the containment check)"
            );
        } else {
            eprintln!(
                "skipping symlink escape assertion: could not create a symlink \
                 (needs privileges / Developer Mode on Windows)"
            );
        }

        std::fs::remove_dir_all(&ws).ok();
        std::fs::remove_dir_all(&outside).ok();
    }
}
