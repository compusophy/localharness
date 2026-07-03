//! Browser wiring for the INTENT ROUTER ([`crate::router`]) — the free/metered
//! cost gate `run_send` consults BEFORE any metered work (before
//! `resolve_credit_access` mints a proxy auth token, before the payment gate,
//! before the model call). The pure classification core + its conservatism
//! tests live at the crate root; THIS file only paints answers and dispatches
//! existing UI toggles.
//!
//! Free-routed turns are LOCAL-ONLY artifacts: they never enter the agent's
//! history (nothing to bill, nothing for the model to see), so they don't
//! survive a reload and a follow-up model turn won't have them in context —
//! the [`crate::router::FREE_ROUTE_FOOTER`] on every card tells the user how
//! to reach the model instead.
//!
//! Per-session opt-in: `/router on` (sessionStorage `lh_router_on`; default
//! OFF until the browser paths get a tab-E2E pass — `/router off` reverts).

use maud::html;

use crate::router::{
    docs_answer, parse_router_cmd, strip_bang, FreeAction, HeuristicClassifier,
    IntentClassifier, Route, RouterCmd, UiCommand, FREE_ROUTE_FOOTER,
};

use super::super::{dom, templates, APP};

/// sessionStorage key for the per-session opt-IN ("1" = router enabled).
const ROUTER_ON_KEY: &str = "lh_router_on";

/// What `run_send` should do with the message after the gate.
pub(super) enum PreRoute {
    /// Answered / dispatched locally for free — `run_send` stops here.
    Handled,
    /// Proceed with the normal metered turn using this prompt (the `'!'`
    /// force-prefix, if any, already stripped).
    SendToModel(String),
}

/// The gate. Order: `/router` commands first (always live, even when the
/// router is off — they're how it comes on), then the opt-in check, then the
/// conservative classifier.
pub(super) async fn pre_route(prompt: &str) -> PreRoute {
    if let Some(cmd) = parse_router_cmd(prompt) {
        apply_cmd(cmd);
        return PreRoute::Handled;
    }
    if !enabled() {
        return PreRoute::SendToModel(prompt.to_string());
    }
    match HeuristicClassifier.classify(prompt) {
        Route::Metered => PreRoute::SendToModel(strip_bang(prompt).to_string()),
        Route::Free(action) => {
            run_free(prompt, action).await;
            PreRoute::Handled
        }
    }
}

/// Is the gate on for this session? **Default OFF** (opt-in via `/router on`)
/// until the browser paths are tab-E2E'd; the decision itself is the pure
/// [`crate::router::router_enabled`] so the default is pinned natively.
fn enabled() -> bool {
    let Ok(Some(storage)) = dom::session_storage() else { return false };
    let flag = storage.get_item(ROUTER_ON_KEY).ok().flatten();
    crate::router::router_enabled(flag.as_deref())
}

/// Apply a `/router on|off|status` command; feedback via the status line
/// (transient, like other command acknowledgements).
fn apply_cmd(cmd: RouterCmd) {
    let storage = dom::session_storage().ok().flatten();
    match cmd {
        RouterCmd::Off => {
            if let Some(s) = &storage {
                let _ = s.remove_item(ROUTER_ON_KEY);
            }
            dom::set_status(
                "intent router OFF (the default) — every message goes to the model \
                 (metered). '/router on' opts this session in.",
                false,
            );
        }
        RouterCmd::On => {
            if let Some(s) = &storage {
                let _ = s.set_item(ROUTER_ON_KEY, "1");
            }
            dom::set_status(
                "intent router ON for this session — obvious balance/files/display/docs \
                 messages are answered free. '!' prefix forces the model; '/router off' \
                 reverts to the default (off).",
                false,
            );
        }
        RouterCmd::Status => {
            let state = if enabled() { "ON (opted in this session)" } else { "OFF (the default)" };
            dom::set_status(
                &format!(
                    "intent router: {state}. Free tiers when on: balance/credits, open \
                     files/display/terminal, a small docs FAQ. '!' prefix forces the \
                     model; '/router on|off' toggles."
                ),
                false,
            );
        }
    }
}

/// Paint the user bubble + a free assistant answer (no agent, no meter).
/// Mirrors the transcript shapes `run_send` paints so a free turn is visually
/// indistinguishable from a chat turn.
async fn run_free(prompt: &str, action: FreeAction) {
    let (user_turn_id, assistant_turn_id) = APP.with(|cell| {
        let mut app = cell.borrow_mut();
        (app.alloc_id(), app.alloc_id())
    });
    dom::append_html(
        "transcript",
        &templates::turn(user_turn_id, "user", html! { (prompt) }, false).into_string(),
    );
    dom::scroll_to_bottom("transcript");

    let answer = match action {
        FreeAction::BalanceQuery => balance_answer().await,
        FreeAction::DocsAnswer(topic) => docs_answer(topic).to_string(),
        FreeAction::UiCommand(cmd) => run_ui_command(cmd).await,
    };
    let body = format!("{answer}\n\n{FREE_ROUTE_FOOTER}");
    dom::append_html(
        "transcript",
        &templates::turn(
            assistant_turn_id,
            "assistant",
            templates::rendered_markdown(&body),
            false,
        )
        .into_string(),
    );
    dom::scroll_to_bottom("transcript");
}

/// The SAME data the admin credits pill shows (`events::refresh_credits_pill`):
/// spendable `$LH` = wallet balance + per-request meter for the credit
/// identity, 2-decimal. Timeout-capped reads; soft-fails to a plain message.
async fn balance_answer() -> String {
    let Some(addr) = super::credit_address_existing().await else {
        return "No credit identity on this device yet — create or import an \
                identity first (the apex page, or the admin panel)."
            .to_string();
    };
    let wallet = crate::app::net::read(crate::registry::token_balance_of(&addr))
        .await
        .ok()
        .and_then(Result::ok);
    let meter = crate::app::net::read(crate::registry::credit_balance_of(&addr))
        .await
        .ok()
        .and_then(Result::ok);
    match (wallet, meter) {
        (Some(wallet), Some(meter)) => {
            let total = wallet + meter;
            let whole = total / 1_000_000_000_000_000_000u128;
            let cents = (total % 1_000_000_000_000_000_000u128) / 10_000_000_000_000_000u128;
            format!(
                "Balance: **{whole}.{cents:02} $LH** spendable (wallet + meter) for \
                 `{addr}`."
            )
        }
        _ => "Couldn't read the balance right now (RPC timeout) — check the admin \
              panel, or retry."
            .to_string(),
    }
}

/// Dispatch the existing UI toggle a free-routed command maps to, and return
/// the confirmation line for the answer card. These are the SAME handlers the
/// header buttons drive (`events::dispatch`) — toggles, so repeating the
/// command closes what it opened.
async fn run_ui_command(cmd: UiCommand) -> String {
    match cmd {
        UiCommand::OpenFiles => {
            crate::app::opfs::toggle_files_modal().await;
            "Toggled the files modal (say it again — or tap its ×/outside — to close)."
                .to_string()
        }
        UiCommand::OpenDisplay => {
            crate::app::opfs::toggle_display();
            "Toggled the display overlay (× or the same command closes it).".to_string()
        }
        UiCommand::OpenTerminal => {
            crate::app::cli::toggle_terminal();
            "Toggled the terminal overlay (× or the same command closes it).".to_string()
        }
    }
}
