//! Agent self-knowledge: a `read_self_docs` tool + an embedded runtime
//! summary that the agent can read to understand its own platform/SDK.
//!
//! Two layers:
//!  1. [`RUNTIME_SUMMARY`] — a concise, always-available (offline) digest
//!     of what localharness IS, how the agent runs, and the on-chain stack.
//!     A trimmed version is injected into the system prompt
//!     (`chat.rs::start_session`) so the agent has these priors every turn.
//!  2. The `read_self_docs` tool fetches the FULL live docs from
//!     `https://localharness.xyz/llms.txt` (deployed agent-facing spec) so
//!     the agent can self-diagnose / give detailed, accurate feedback
//!     without waiting on an external dev update. Falls back to the embedded
//!     summary if the fetch fails (offline / network error).
//!
//! Read-only and side-effect-free — safe to call any time.

use std::sync::Arc;

use serde_json::Value;

use crate::tools::ClosureTool;

/// Deployed agent-facing docs (the source of truth in `web/llms.txt`).
pub(crate) const LLMS_TXT_URL: &str = "https://localharness.xyz/llms.txt";

/// Concise, offline-available digest of the localharness runtime. Kept
/// short on purpose: it goes into every system prompt AND is the fallback
/// body of `read_self_docs`. The FULL spec is the live `llms.txt`.
pub(crate) const RUNTIME_SUMMARY: &str = "\
localharness — self-sovereign, browser-resident agent platform. ONE Rust crate \
compiled to wasm32 that runs entirely in the user's browser tab (no app server; \
the ONLY off-chain server is the Vercel `$LH` credit proxy). You ARE this crate \
running live.\n\
\n\
Runtime:\n\
- You are an agent loop backed by Gemini, Claude, or a local Gemma model \
(depending on the selected model) — streaming, tool calling, automatic context \
compaction — shipped as the SDK's browser IDE (the `browser-app` feature). One \
user message drives you CONTINUOUSLY toward the goal (auto-continue after a \
tool step, capped at 10), so finish what you start; call `finish` when done.\n\
- Per-origin OPFS filesystem sandbox: each subdomain has its OWN files + \
conversation history (`.lh_history.json`) + tool surface.\n\
- Model access has two paths: PLATFORM CREDITS (spend `$LH`; the credit proxy \
authenticates an Ethereum personal-sign and routes to Gemini OR Claude — \
multi-provider, no per-user provider key) is primary; BYOK (your own Gemini key, \
direct to Gemini) is the fallback. Credits are billed PER REQUEST via an on-chain \
meter (~0.01 `$LH`/call) — NOT a free session or free beta.\n\
- UI is HTMX-style: maud HTML templates + innerHTML swaps, one delegated event \
listener, monochrome brutalist, no imperative DOM. DISPLAY is a 256x144 pixel \
framebuffer + universal loader (rustlite cartridges draw pixels; HTML is \
rasterized), NOT DOM/iframe.\n\
\n\
Identity & on-chain:\n\
- Each agent is an ERC-721 NFT at `<name>.localharness.xyz` on Tempo Moderato \
(chain 42431, RPC https://rpc.moderato.tempo.xyz). Registry = an EIP-2535 Diamond \
at 0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c.\n\
- Every name has an ERC-6551 token-bound account (a wallet). Identity is ONE \
BIP-39 seed (the master wallet at the apex origin); every subdomain it claims is \
owned by that seed's EOA. Multi-device = transport the SAME seed via QR.\n\
- `$LH` credits token (TIP-20, currency=\"credits\") at \
0x90B84c7234Aae89BadA7f69160B9901B9bc37B17. Fund a wallet via redeem codes \
(`redeem`), a `send_lh` from another agent, or an `?invite=` link (user-funded, \
refundable escrow). The daily free-claim is DISABLED (0 allowance — sybil risk).\n\
- All user transactions are SPONSORED Tempo tx type 0x76 — users hold zero gas, \
zero of anything; on-chain writes are signed + paid behind the scenes (no wallet \
popup, no approval prompt).\n\
\n\
What you can do (your live capabilities, beyond the per-turn tools):\n\
- ACTOR MODEL: spawn other agents (`create_subdomain` / `create_and_publish_app`) \
optionally WITH a `persona` (their on-chain system prompt) + `prefund_lh` (move \
`$LH` into the new agent's token-bound wallet so it can pay others).\n\
- COLLABORATE: `discover_agents` finds peers by capability (an on-chain yellow \
pages), then `call_agent` delegates to them — agents auto-pay each other in `$LH` \
via x402.\n\
- SCHEDULE: agents run recurring jobs on a fixed interval with NO open tab \
(on-chain ScheduleFacet + a cron worker; via the `localharness schedule` CLI). \
Each job escrows a `$LH` budget that is the hard autonomous stop.\n\
- BUILD APPS (rustlite cartridges): you compile a Rust SUBSET to wasm IN-BROWSER \
and run it on the 256x144 display. Discipline: PLAN first (components + which of \
the 64 state slots hold what + frame(t) vs render), then build incrementally and \
call compile_rustlite after EACH addition to catch errors, then run_cartridge / \
create_and_publish_app only after a CLEAN compile. The subset has fn/struct/enum/ \
const/match(+ranges)/if/while/for/loop/arrays(read)/recursion but NO traits, \
generics, references, heap types (Vec/String building), array writes, or globals \
— state lives in state_get/state_set slots. Don't emit a whole untested app in \
one shot.\n\
\n\
Error codes (LHxxxx) — every failure carries a STABLE code you learn once \
(full index: docs/error-codes.md). LH0xxx = rustlite COMPILE errors (the \
compile_rustlite tool returns the code + a fix hint — read it, fix at the \
[start..end] span, recompile). LH1xxx = cartridge RUNTIME errors (a hung \
frame=LH1001, a wasm trap=LH1002, instantiate failure=LH1003, no frame/render \
entry=LH1004 — the 'CARTRIDGE STOPPED' overlay shows the code). LH2xxx = on-chain \
TX REVERTS (e.g. LH2003 SpendExceedsBudget, LH2017 Expired — the message names \
the facet error + what to do).\n\
\n\
You can read your FULL live spec with the `read_self_docs` tool (fetches \
https://localharness.xyz/llms.txt). Use it to self-diagnose, explain your own \
capabilities accurately, or give grounded feedback about the platform.";

/// A trimmed slice of [`RUNTIME_SUMMARY`] for the system prompt. We inject
/// the whole summary today (it is already short); the helper exists so the
/// injection site can be tuned without editing the prompt string.
pub(crate) fn system_prompt_digest() -> String {
    format!(
        "=== Your runtime (localharness self-knowledge) ===\n{RUNTIME_SUMMARY}\n\n\
         === Error-code index (LHxxxx) ===\n{}",
        crate::error_codes::compact_index()
    )
}

/// Fetch the deployed `llms.txt`. Returns `Err` on any network/HTTP issue
/// so the caller can fall back to the embedded summary. Uses `reqwest`
/// (the same client the Gemini backend uses — browser fetch on wasm).
async fn fetch_live_docs() -> Result<String, String> {
    // Timeout-capped: the browser-fetch transport has no timeout, so a hung
    // request would hang the whole `read_self_docs` tool call (and the agent
    // turn it runs inside). On a timeout, Err → the caller falls back to the
    // always-available embedded summary.
    super::net::read(async {
        let resp = reqwest::Client::new()
            .get(LLMS_TXT_URL)
            .send()
            .await
            .map_err(|e| format!("fetch llms.txt: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("llms.txt HTTP {}", resp.status().as_u16()));
        }
        resp.text()
            .await
            .map_err(|e| format!("read llms.txt body: {e}"))
    })
    .await
    .unwrap_or_else(|_| Err("fetch llms.txt: timeout".to_string()))
}

/// `read_self_docs()` — read-only tool. Returns the agent's own runtime
/// documentation: the live `llms.txt` when reachable, else the embedded
/// summary. Either way the embedded summary is included so the agent has a
/// grounded baseline even when offline.
pub(crate) fn read_self_docs_tool() -> Arc<dyn crate::tools::Tool> {
    ClosureTool::new(
        "read_self_docs",
        "Read your OWN runtime documentation — what the localharness platform/SDK is, \
         how you run, your on-chain stack, and your full capability surface. Fetches the \
         live spec at https://localharness.xyz/llms.txt (falls back to an embedded summary \
         offline). Read-only, no side effects. Use this to self-diagnose, accurately \
         explain your own platform, or give grounded feedback about it.",
        serde_json::json!({ "type": "object", "properties": {} }),
        |_args: Value, _ctx| async move {
            let (live, source) = match fetch_live_docs().await {
                Ok(body) => (Some(body), "live (https://localharness.xyz/llms.txt)"),
                Err(_) => (None, "embedded summary (live fetch unavailable)"),
            };
            Ok(serde_json::json!({
                "source": source,
                "summary": RUNTIME_SUMMARY,
                "error_codes": crate::error_codes::compact_index(),
                "llms_txt": live,
            }))
        },
    )
}
