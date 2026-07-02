//! Identity + seed flows — the consolidated onboarding front door, apex/tenant
//! identity creation, seed import, and seed reveal. Bodies moved verbatim out
//! of the `dispatch` arms in `events/mod.rs` (one home per domain); the
//! single-flight `ONBOARD_*` guards stay in `events/mod.rs`.

use crate::app::{dom, templates};

use super::{
    defer_onboard_repaint, onboard_flow_begin, set_onboard_name, take_onboard_name,
    warn_if_storage_volatile,
};

/// [`Action::RevealSeed`](super::Action::RevealSeed).
///
/// Local-first (local-seed-per-origin, see `verify::local_master`):
/// when THIS origin holds the seed — apex always, a subdomain
/// after `seed_pull` — read the mnemonic straight off the cached
/// `APP.wallet`. The signer-iframe round-trip is only the
/// fallback for a seedless tenant origin: its `lh-reveal-seed`
/// handler is apex-origin-only and the iframe is partitioned
/// (dead) on mobile, so it must never be the primary path.
pub(super) fn reveal_seed_pressed() {
    let phrase = crate::app::APP.with(|cell| {
        cell.borrow()
            .wallet
            .as_ref()
            .map(|w| w.mnemonic.to_string())
    });
    if let Some(p) = phrase {
        dom::swap_inner(
            "seed-reveal",
            &templates::seed_phrase(&p).into_string(),
        );
    } else if !matches!(crate::app::tenant::current(), crate::app::tenant::Host::Apex) {
        dom::swap_inner(
            "seed-reveal",
            "<span style=\"color:var(--muted)\">fetching…</span>",
        );
        wasm_bindgen_futures::spawn_local(async move {
            match crate::app::verify::reveal_seed_via_iframe().await {
                Ok(phrase) => dom::swap_inner(
                    "seed-reveal",
                    &templates::seed_phrase(&phrase).into_string(),
                ),
                Err(err) => dom::swap_inner(
                    "seed-reveal",
                    &maud::html! {
                        span style="color:var(--error)" { "reveal failed: " (err) }
                        button type="button" data-action="reveal-seed" class="ghost" { "retry" }
                    }
                    .into_string(),
                ),
            }
        });
    }
}

/// [`Action::OnboardCreate`](super::Action::OnboardCreate).
///
/// Consolidated front door: the visitor picked a name AND hit CREATE.
/// 1) Validate + STASH the name (the checkout card replaces the input,
///    so it must survive the async flow); the live check already keeps
///    the button disabled for invalid/taken names, so this is a guard.
/// 2) Generate the keypair IN MEMORY ONLY (persisted on confirmed
///    payment, not here — the owner's rule + the iOS borrow-panic fix).
/// 3) Run the $2 checkout. Once paid, `credits::persist_seed_and_claim`
///    claims the stashed name and redirects into the agent's chat — no
///    second step, no shown-once seed page.
pub(super) fn onboard_create_pressed() {
    let raw = dom::input_by_id("apex-input")
        .map(|i| i.value())
        .unwrap_or_default();
    let cleaned = crate::app::tenant::sanitize(&raw);
    if cleaned.len() < 3 || cleaned.len() > 32 {
        return;
    }
    let Some(flow_guard) = onboard_flow_begin() else {
        return;
    };
    set_onboard_name(&cleaned);
    // INSTANT FEEDBACK: swap the form for the inline checkout card before
    // any await ($2 = 200 $LH at $1 = 100 $LH). The card carries the Stripe
    // mount ids; the spawned work below fills it.
    dom::swap_outer(
        "apex-onboard",
        &templates::onboard_checkout().into_string(),
    );
    wasm_bindgen_futures::spawn_local(async move {
        let _flow_guard = flow_guard;
        match crate::app::wallet_store::generate_in_memory().await {
            Err(err) => {
                // Restore the form (with the typed name preserved) so the
                // user can retry; drop the stale stash.
                let _ = take_onboard_name();
                dom::swap_outer(
                    "apex-onboard",
                    &crate::landing::create_wallet_cta().into_string(),
                );
                if let Some(input) = dom::input_by_id("apex-input") {
                    input.set_value(&cleaned);
                    super::claim::on_apex_input();
                }
                dom::swap_inner(
                    "onboard-msg",
                    &dom::msg_span(dom::Msg::Error, &format!("create failed: {err}")),
                );
            }
            Ok(_) => {
                // Identity in memory → drive the $2 checkout into the inline
                // card (mints 200 $LH); on a confirmed mint the poll persists
                // the seed, claims the stashed name, and redirects to chat.
                super::credits::buy_lh_pressed(true);
            }
        }
    });
}

/// [`Action::CreateIdentity`](super::Action::CreateIdentity).
///
/// Apex: generate locally + bootstrap-fund + re-paint.
/// Tenant: route through the apex signer iframe so the wallet
/// lands at apex OPFS, then re-paint tenant chrome so
/// verification picks up the new owner.
/// SINGLE-FLIGHT: ignore re-presses while a flow runs.
pub(super) fn create_identity_pressed() {
    let Some(flow_guard) = onboard_flow_begin() else {
        return;
    };
    dom::swap_inner(
        "identity-msg",
        "<span style=\"color:var(--muted)\">generating identity…</span>",
    );
    // Volatile-storage (incognito) warning is surfaced AFTER the seed
    // write completes (see `warn_if_storage_volatile` calls below), NOT
    // concurrently: racing `storage_is_volatile()`'s `navigator.storage`
    // round-trip against `create_and_persist`'s first OPFS write
    // interleaved two tasks on the single-thread executor and tripped the
    // iOS "RefCell already borrowed" panic at the seed write. The warning
    // is about BACKING UP a freshly-minted seed, so showing it once the
    // seed exists is equally timely.
    match crate::app::tenant::current() {
        crate::app::tenant::Host::Apex => {
            wasm_bindgen_futures::spawn_local(async move {
                let _flow_guard = flow_guard;
                // Bounded: a wedged storage write must surface as an
                // error, not an eternal "generating identity…" (the
                // iPhone stuck-create report).
                match crate::app::net::with_timeout(
                    15_000,
                    crate::app::wallet_store::create_and_persist(),
                )
                .await
                {
                    Err(_) => {
                        dom::swap_inner(
                            "identity-msg",
                            &dom::msg_span(
                                dom::Msg::Error,
                                "create timed out — reload and try again",
                            ),
                        );
                        return;
                    }
                    Ok(Err(err)) => {
                        dom::swap_inner(
                            "identity-msg",
                            &dom::msg_span(dom::Msg::Error, &format!("create failed: {err}")),
                        );
                        return;
                    }
                    Ok(Ok(_)) => {}
                }
                // Seed is now safely persisted — only NOW probe storage
                // durability (sequentially, never racing the write) and
                // surface the incognito back-up warning if volatile.
                warn_if_storage_volatile().await;
                // Progress BEFORE the repaint: paint_apex does on-chain
                // reads that can be slow on mobile — the identity is
                // already safe at this point and the user should know.
                dom::swap_inner(
                    "identity-msg",
                    "<span style=\"color:var(--muted)\">identity created — loading…</span>",
                );
                // Run the repaint on a FRESH executor tick, not inline.
                // `paint_apex` itself `spawn_local`s several sub-tasks and
                // awaits OPFS/RPC; awaiting it from inside THIS onboarding
                // task deepens the wasm-bindgen single-thread executor's
                // poll chain, and on iOS WebKit's microtask timing that
                // re-enters `Task::run` for an already-borrowed task → the
                // "RefCell already borrowed" panic. Deferring lets this
                // task fully return (releasing the executor borrow) before
                // the heavy paint runs. The flow guard rides along so a
                // re-press is still blocked until the surface is up.
                defer_onboard_repaint(_flow_guard, async {
                    crate::app::paint_apex(crate::app::tenant::Host::Apex).await;
                });
            });
        }
        host => {
            wasm_bindgen_futures::spawn_local(async move {
                let flow_guard = flow_guard;
                match crate::app::verify::create_wallet_via_iframe(false).await {
                    Ok(_addr) => {
                        // Seed persisted — probe durability now, never
                        // racing the write (the iOS borrow-panic class).
                        warn_if_storage_volatile().await;
                        if let crate::app::tenant::Host::Tenant(name) = &host {
                            // Defer the repaint to a fresh executor tick —
                            // same iOS re-entrant-`Task::run` hazard as the
                            // apex branch above (paint_tenant spawns + awaits
                            // OPFS/RPC). The guard rides along.
                            let host = host.clone();
                            let name = name.clone();
                            defer_onboard_repaint(flow_guard, async move {
                                crate::app::paint_tenant(host, name).await;
                            });
                        }
                    }
                    Err(err) => {
                        dom::swap_inner(
                            "identity-msg",
                            &dom::msg_span(dom::Msg::Error, &format!("create failed: {err}")),
                        );
                    }
                }
            });
        }
    }
}

/// [`Action::ImportSeed`](super::Action::ImportSeed).
pub(super) fn import_seed_pressed() {
    let phrase = dom::textarea_by_id("import-seed")
        .map(|t| t.value())
        .unwrap_or_default();
    if phrase.split_whitespace().count() != 12 {
        dom::swap_inner(
            "seed-msg",
            "<span style=\"color:var(--error)\">expected exactly 12 words</span>",
        );
        return;
    }
    // Apex: write directly to apex OPFS, re-paint apex.
    // Tenant: write THIS origin's OPFS directly too — a tenant
    // import intentionally affects only this origin (that IS the
    // local-seed-per-origin model; other origins adopt the seed
    // via the apex QR `?adopt=1` flow or `seed_pull`). The old
    // signer-iframe route always failed here: the iframe's
    // `lh-import-seed` handler is apex-origin-only and the iframe
    // itself is partitioned (dead) on mobile.
    match crate::app::tenant::current() {
        crate::app::tenant::Host::Apex => {
            wasm_bindgen_futures::spawn_local(async move {
                match crate::app::wallet_store::import(&phrase).await {
                    Ok(_) => {
                        crate::app::paint_apex(crate::app::tenant::Host::Apex).await;
                    }
                    Err(err) => {
                        dom::swap_inner(
                            "seed-msg",
                            &dom::msg_span(dom::Msg::Error, &format!("import failed: {err}")),
                        );
                    }
                }
            });
        }
        host => {
            wasm_bindgen_futures::spawn_local(async move {
                match crate::app::wallet_store::import(&phrase).await {
                    Ok(wallet) => {
                        crate::app::APP
                            .with(|cell| cell.borrow_mut().wallet = Some(wallet));
                        if let crate::app::tenant::Host::Tenant(name) = &host {
                            crate::app::paint_tenant(host.clone(), name.clone()).await;
                        }
                    }
                    Err(err) => {
                        dom::swap_inner(
                            "seed-msg",
                            &dom::msg_span(dom::Msg::Error, &format!("import failed: {err}")),
                        );
                    }
                }
            });
        }
    }
}
