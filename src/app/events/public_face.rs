//! Public face — publish the subdomain face choice (directory / app / html).

use crate::encoding::parse_address;

use crate::app::dom;
use crate::filesystem::Filesystem;

/// Set this subdomain's public face on-chain — `"directory"`, `"app"`, or
/// `"html"`. The choice lives under `keccak256("localharness.public_face")`
/// so every visitor honours it. For `"app"`/`"html"` we also publish the
/// device's local `app.rl`/`index.html` content in the SAME sponsored tx
/// (owner-signed through the apex iframe), so the chosen face is live
/// immediately. Owner-only.
pub(super) async fn run_set_public_face(choice: &str) {
    let msg = "publish-app-msg";
    let set_err = |m: &str| {
        dom::swap_inner(msg, &dom::msg_span(dom::Msg::Error, m));
    };

    let name = match crate::app::tenant::current() {
        crate::app::tenant::Host::Tenant(n) => n,
        _ => {
            set_err("only on a subdomain");
            return;
        }
    };

    // The verified-EOA address IF this device verified as the on-chain
    // owner directly. May be None when the owner is a TBA we sign for
    // (consolidation) — that path is decided in the submit branch below.
    let verified_eoa = crate::app::APP.with(|cell| {
        use crate::app::VerifyState;
        match &cell.borrow().verify_state {
            VerifyState::Verified { address } => Some(address.clone()),
            _ => None,
        }
    });

    let id = match crate::app::registry::id_of_name(&name).await {
        Ok(id) if id != 0 => id,
        _ => {
            set_err("name isn't registered on-chain");
            return;
        }
    };

    let registry_addr = match parse_address(crate::app::registry::REGISTRY_ADDRESS) {
        Ok(a) => a,
        Err(e) => {
            set_err(&e);
            return;
        }
    };
    let mk = |input: Vec<u8>| crate::tempo_tx::TempoCall {
        to: registry_addr,
        value_wei: 0,
        input,
    };

    // Build the call batch + gas estimate for the chosen face. Storing
    // `bytes` costs ~20k gas per 32-byte word (cold SSTORE) on top of the
    // ~275k Tempo sponsorship + base call.
    let (calls, gas): (Vec<crate::tempo_tx::TempoCall>, u128) = match choice {
        "directory" => (
            vec![mk(crate::app::registry::encode_set_public_face(id, "directory"))],
            500_000,
        ),
        "app" => {
            let fs = crate::app::shared_opfs();
            let src = match fs.read("app.rl").await {
                Ok(b) if !b.is_empty() => String::from_utf8_lossy(&b).into_owned(),
                _ => {
                    set_err("no app.rl on this device — build one first (run_cartridge)");
                    return;
                }
            };
            let wasm = match crate::rustlite::compile(&src) {
                Ok(w) => w,
                Err(e) => {
                    set_err(&format!("compile: {e}"));
                    return;
                }
            };
            if wasm.len() > 16_384 {
                set_err("app wasm too large to publish (max 16 KB)");
                return;
            }
            (
                vec![
                    mk(crate::app::registry::encode_set_app_wasm(id, &wasm)),
                    mk(crate::app::registry::encode_set_public_face(id, "app")),
                ],
                // setMetadata stores bytes on-chain at ~7.6k gas/BYTE (measured
                // via debug_traceTransaction, 2026-06-03; same byte-storage cost
                // as the FeedbackFacet). The old `1.3M + words*40k` (~1.25k/byte)
                // was ~6x too low and OOG-reverted any non-trivial app publish.
                1_200_000 + wasm.len() as u128 * 8_500,
            )
        }
        "html" => {
            let fs = crate::app::shared_opfs();
            let html = match fs.read("index.html").await {
                Ok(b) if !b.is_empty() => b,
                _ => {
                    set_err("no index.html on this device — create one first");
                    return;
                }
            };
            if html.len() > 24_576 {
                set_err("index.html too large to publish (max 24 KB)");
                return;
            }
            (
                vec![
                    mk(crate::app::registry::encode_set_public_html(id, &html)),
                    mk(crate::app::registry::encode_set_public_face(id, "html")),
                ],
                // Same ~7.6k gas/byte on-chain storage cost as the app branch.
                1_200_000 + html.len() as u128 * 8_500,
            )
        }
        _ => {
            set_err("unknown public face");
            return;
        }
    };

    dom::swap_inner(msg, "<span style=\"color:var(--muted)\">saving…</span>");

    // Decide the execution path from the on-chain owner:
    //  - owner is a TBA this device signs for (consolidation) → execute
    //    the setMetadata batch THROUGH the TBA, signed by our local key.
    //  - owner is our verified EOA → direct sponsored call (existing).
    let on_chain_owner = match crate::app::registry::owner_of_name(&name).await {
        Ok(Some(o)) => o,
        _ => {
            set_err("name isn't registered on-chain");
            return;
        }
    };
    let local = crate::app::chat::credit_signer().await;
    let is_signer = match &local {
        Some((_, addr)) => {
            let addr_hex = format!(
                "0x{}",
                addr.iter().map(|b| format!("{b:02x}")).collect::<String>()
            );
            crate::app::registry::is_authorized_signer(&on_chain_owner, &addr_hex)
                .await
                .unwrap_or(false)
        }
        None => false,
    };
    let result = if is_signer {
        let (signer, _) = local.unwrap();
        let fee_payer = match crate::app::sponsor::signer() {
            Ok(s) => s,
            Err(e) => {
                set_err(&e);
                return;
            }
        };
        let token_id = match crate::app::registry::tba_token_id_of(&on_chain_owner).await {
            Ok(t) => t,
            Err(e) => {
                set_err(&e);
                return;
            }
        };
        let targets: Vec<([u8; 20], Vec<u8>)> =
            calls.iter().map(|c| (registry_addr, c.input.clone())).collect();
        crate::app::registry::tba_execute_batch_sponsored(
            &signer,
            &fee_payer,
            token_id,
            &on_chain_owner,
            &targets,
            crate::app::registry::ALPHA_USD_ADDRESS,
            gas + 800_000,
        )
        .await
    } else if let Some(owner_hex) =
        verified_eoa.filter(|a| a.eq_ignore_ascii_case(&on_chain_owner))
    {
        super::run_sponsored_tempo_call(&owner_hex, calls, gas, "public face").await
    } else {
        set_err("verify as owner first");
        return;
    };
    match result {
        Ok(_tx) => {
            let head = if choice == "directory" {
                "public face → directory ✓".to_string()
            } else {
                format!("published ✓ — {name}.localharness.xyz")
            };
            dom::swap_inner(
                msg,
                &maud::html! {
                    span style="color:var(--fg)" {
                        (head) " "
                        a href=(format!("https://{name}.localharness.xyz/"))
                          target="_blank" rel="noopener" style="color:var(--accent)" {
                            "open →"
                        }
                    }
                }
                .into_string(),
            );
            super::admin::refresh_public_face_status().await;
        }
        Err(e) => set_err(&format!("failed: {e}")),
    }
}
