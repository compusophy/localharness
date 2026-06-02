//! Chat-turn orchestration. Driven by the `send` action in
//! [`super::events`]; entirely HTMX-style — every UI mutation is a
//! `swap_inner` / `append_html` on a targeted `id=`. We never walk the
//! DOM looking for nodes; element identity is established up-front via
//! ids we allocate and templates we render.

use std::collections::VecDeque;
use std::rc::Rc;

use futures_util::StreamExt;
use maud::html;
use wasm_bindgen::JsValue;

use crate::policy;
use crate::tools::ClosureTool;
use crate::{Agent, CapabilitiesConfig, GeminiAgentConfig, StreamChunk};

use super::dom;
use super::templates;
use super::APP;

thread_local! {
    /// True while a turn is streaming — guards against starting a second
    /// turn (e.g. pressing Enter again mid-turn).
    static TURN_ACTIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Set by the stop button; the stream loop checks it each chunk and
    /// breaks, cooperatively ending the turn.
    static TURN_CANCEL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Request cooperative cancellation of the running turn (the stop
/// button). Two layers: `TURN_CANCEL` breaks the UI's chunk loop, and
/// `agent.cancel_turn()` stops the *producer* — the detached task driving
/// the agent loop — so it stops calling the model and running tools
/// instead of finishing the turn in the background while the UI moves on.
pub(crate) fn request_stop_turn() {
    TURN_CANCEL.with(|c| c.set(true));
    if let Some(agent) = APP.with(|cell| cell.borrow().agent.clone()) {
        agent.cancel_turn();
    }
}

/// RAII cleanup for a turn: clears the active/cancel flags and restores
/// the send button (if the stop button is currently shown) on every
/// exit path, including early returns and future cancellation.
struct TurnGuard;
impl Drop for TurnGuard {
    fn drop(&mut self) {
        TURN_ACTIVE.with(|c| c.set(false));
        TURN_CANCEL.with(|c| c.set(false));
        if dom::by_id("terminal-stop").is_some() {
            dom::swap_outer("terminal-stop", &templates::send_button().into_string());
        }
    }
}

/// Driven by the `send` data-action. Reads the prompt + key, lazily
/// (re)starts the session, then streams a turn through the Agent.
pub(crate) async fn run_send() {
    let Some(prompt_area) = dom::textarea_by_id("prompt") else {
        dom::set_status("internal: #prompt textarea missing", true);
        return;
    };

    // The api key input lives in the admin dropdown — only present in
    // the DOM when admin is open. Fall back to sessionStorage (sync)
    // and then OPFS (async) so the user can send without keeping
    // admin open just to host the input field.
    // Resolve how this turn reaches the model: platform credits (proxy
    // auth token + proxy base URL) or BYOK (the stored Gemini key).
    let access = match resolve_credit_access().await {
        Some(a) => a,
        None => {
            super::show_api_key_modal();
            return;
        }
    };
    // Credits mode (base_url set): ensure a session is open so the proxy
    // accepts the call. Free in beta; silent best-effort.
    if access.base_url.is_some() {
        ensure_credit_session().await;
    }
    let key = access.cfg_auth;

    let prompt = prompt_area.value().trim().to_string();
    if prompt.is_empty() {
        // Silent no-op — no explanatory validation text. (on-chain feedback)
        return;
    }

    // One turn at a time. A second send (e.g. Enter pressed again mid-
    // turn) is ignored rather than racing a concurrent stream.
    if TURN_ACTIVE.with(|c| c.get()) {
        return;
    }
    TURN_ACTIVE.with(|c| c.set(true));
    TURN_CANCEL.with(|c| c.set(false));
    // From here, every return path resets the flags + restores the
    // send button via Drop.
    let _turn_guard = TurnGuard;

    // Payment gate. If the agent's owner has set a per-turn price AND
    // we know this visitor is *not* the owner, collect payment via
    // the cross-origin iframe signer before the LLM call runs.
    // Owner-of-the-agent always sends free; verification-pending /
    // unregistered / failed states fall through without charging.
    match collect_payment_if_required().await {
        Ok(None) => {} // free or no gate
        Ok(Some(tx_hash)) => {
            dom::set_status(
                &format!("payment received ({}); sending…", short_hash(&tx_hash)),
                false,
            );
        }
        Err(err) => {
            dom::set_status(&format!("payment failed: {err}"), true);
            return;
        }
    }

    // Cache the BYOK key so a refresh doesn't lose it. Credits tokens
    // rotate per resolve and carry nothing worth caching.
    if access.base_url.is_none() {
        if let Ok(Some(storage)) = dom::session_storage() {
            let _ = storage.set_item("gemini_api_key", &key);
        }
    }

    // Lazily start the session if we have none, or the identity changed.
    let session_needs_start = APP.with(|cell| {
        let app = cell.borrow();
        app.agent.is_none() || app.session_key.as_deref() != Some(access.identity.as_str())
    });
    if session_needs_start {
        if let Err(err) = start_session(&key, access.base_url.clone(), &access.identity).await {
            dom::set_status(&format!("session start failed: {err:?}"), true);
            return;
        }
    }

    let Some(agent) = APP.with(|cell| cell.borrow().agent.clone()) else {
        dom::set_status("internal: agent not set after start_session", true);
        return;
    };

    // Allocate ids for the user turn, assistant turn, and first text
    // segment up front. Element identity is fixed before we touch the DOM.
    let (user_turn_id, assistant_turn_id, mut seg_id) = APP.with(|cell| {
        let mut app = cell.borrow_mut();
        (app.alloc_id(), app.alloc_id(), app.alloc_id())
    });

    dom::append_html(
        "transcript",
        &templates::turn(user_turn_id, "user", html! { (prompt) }, false).into_string(),
    );
    dom::append_html(
        "transcript",
        &templates::turn(
            assistant_turn_id,
            "assistant",
            templates::text_segment(seg_id, ""),
            true,
        )
        .into_string(),
    );
    dom::scroll_to_bottom("transcript");

    let assistant_body_id = format!("turn-body-{assistant_turn_id}");

    // Clear the prompt, keep focus.
    prompt_area.set_value("");
    let _ = prompt_area.focus();
    // No "thinking…" — the assistant turn renders with the .streaming
    // class while in flight, which adds its own "· streaming" suffix
    // to the role line. That's enough feedback.

    // FIFO of pending tool-block ids. The Gemini backend emits
    // ToolCall/ToolResult pairs sequentially (one result per call,
    // in order), so popping the front always matches.
    let mut pending_tools: VecDeque<u32> = VecDeque::new();
    // (seg_id, accumulated_raw_text) for every text segment we render
    // this turn — used for markdown rendering at end-of-stream.
    let mut text_segments: Vec<(u32, String)> = vec![(seg_id, String::new())];

    // Timing: ms since epoch is precise enough for ttft/total pills.
    let t0 = js_sys::Date::now();
    let mut t_first_chunk: Option<f64> = None;

    let response = match agent.chat(prompt).await {
        Ok(r) => r,
        Err(err) => {
            report_turn_error("agent.chat", &format!("{err}"), assistant_turn_id);
            return;
        }
    };
    let mut cursor = response.chunks();

    // Swap the send arrow for a stop button now that we're streaming.
    dom::swap_outer("terminal-send", &templates::stop_button().into_string());

    while let Some(item) = cursor.next().await {
        // Honor a stop request (checked per chunk — cooperative). The
        // post-turn block appends a redirect prompt once the loop exits.
        if TURN_CANCEL.with(|c| c.get()) {
            break;
        }
        if t_first_chunk.is_none() {
            t_first_chunk = Some(js_sys::Date::now());
        }
        match item {
            Ok(StreamChunk::Text { text, .. }) => {
                if !text.is_empty() {
                    let (cur_id, cur_text) = text_segments
                        .last_mut()
                        .expect("text_segments seeded at start of turn");
                    cur_text.push_str(&text);
                    let inner = html! { (cur_text) }.into_string();
                    dom::swap_inner(&format!("seg-{cur_id}"), &inner);
                    dom::scroll_to_bottom("transcript");
                }
            }
            Ok(StreamChunk::ToolCall(call)) => {
                let tool_seg_id = APP.with(|cell| cell.borrow_mut().alloc_id());
                dom::append_html(
                    &assistant_body_id,
                    &templates::tool_call_block(tool_seg_id, &call).into_string(),
                );
                pending_tools.push_back(tool_seg_id);

                // Open a fresh text segment for whatever the model
                // says after the tool call (it usually says nothing
                // until the result comes back, but if it does, this
                // is where it lands).
                seg_id = APP.with(|cell| cell.borrow_mut().alloc_id());
                text_segments.push((seg_id, String::new()));
                dom::append_html(
                    &assistant_body_id,
                    &templates::text_segment(seg_id, "").into_string(),
                );
                // Keep the newest content in view — tool-call blocks used to
                // append below the fold without scrolling (text chunks did).
                dom::scroll_to_bottom("transcript");
            }
            Ok(StreamChunk::ToolResult(result)) => {
                if let Some(tool_seg_id) = pending_tools.pop_front() {
                    let result_target = format!("tool-{tool_seg_id}-result");
                    dom::swap_inner(
                        &result_target,
                        &templates::tool_call_result(&result).into_string(),
                    );
                    dom::scroll_to_bottom("transcript");
                }
            }
            Ok(StreamChunk::Thought { .. }) => {
                // Thoughts intentionally not surfaced (yet).
            }
            Err(err) => {
                report_turn_error("stream", &format!("{err}"), assistant_turn_id);
                return;
            }
        }
    }

    // Streaming finished (or was stopped) — flip the stop button back
    // to send now, rather than waiting for the post-turn save/refresh.
    dom::swap_outer("terminal-stop", &templates::send_button().into_string());

    // Stream done — re-render each text segment as markdown so the
    // user sees formatted output instead of raw md syntax.
    for (id, raw) in &text_segments {
        if raw.is_empty() {
            continue;
        }
        let html_str = templates::rendered_markdown(raw).into_string();
        dom::swap_inner(&format!("seg-{id}"), &html_str);
    }

    mark_turn_done(assistant_turn_id);

    // Stash cumulative token usage for the admin Usage tab.
    if let Some(total) = agent.cumulative_usage().total_token_count {
        APP.with(|cell| cell.borrow_mut().total_tokens = total.max(0) as u64);
    }

    // If the user hit stop, append a short redirect prompt so the turn
    // ends on a question rather than a half-finished thought.
    if TURN_CANCEL.with(|c| c.get()) {
        let note_id = APP.with(|cell| cell.borrow_mut().alloc_id());
        dom::append_html(
            "transcript",
            &templates::turn(
                note_id,
                "assistant",
                templates::text_segment(note_id, "Stopped. What should I do instead?"),
                false,
            )
            .into_string(),
        );
        dom::scroll_to_bottom("transcript");
    }

    APP.with(|cell| cell.borrow_mut().turn_count += 1);
    let turn_count = APP.with(|cell| cell.borrow().turn_count);

    // No status write on success — keep the terminal silent. (Per
    // the minimalism pass; ttft/total metrics still computed for
    // anyone who wants to grep the wasm but not surfaced in chrome.)
    let _t_end = js_sys::Date::now();
    let _ = (t0, t_first_chunk, turn_count);

    // Persist the new history snapshot, then refresh the panel so
    // any tool-created files (and the history marker itself) show up.
    super::history::save_from_agent().await;
    super::opfs::refresh().await;
}

/// Surface a turn failure. If it looks like an auth / API-key problem
/// (the most common first-run failure), reopen the key modal so the user
/// can fix it, rather than leaving a cryptic error in the status line.
fn report_turn_error(context: &str, err: &str, assistant_turn_id: u32) {
    mark_turn_done(assistant_turn_id);
    let lower = err.to_lowercase();
    let looks_like_auth = lower.contains("api key")
        || lower.contains("api_key")
        || lower.contains("401")
        || lower.contains("403")
        || lower.contains("permission_denied")
        || lower.contains("unauthenticated");
    if looks_like_auth {
        dom::set_status("API key rejected — check your Gemini key.", true);
        super::show_api_key_modal();
    } else {
        dom::set_status(&format!("{context}: {err}"), true);
    }
}

pub(crate) async fn start_session(
    key: &str,
    base_url: Option<url::Url>,
    identity: &str,
) -> Result<(), JsValue> {
    // System instruction — the agent needs to know what it's running
    // inside and what its filesystem looks like. Without this, prompts
    // like "what is pricing" produce blind tool calls because the
    // model has no priors about the localharness environment.
    let host = super::tenant::current();
    let agent_name = match &host {
        super::tenant::Host::Tenant(name) => name.clone(),
        _ => "this agent".to_string(),
    };
    let system_instructions = format!(
        "You are {agent_name}, a browser-resident assistant running inside \
         the localharness platform — a Rust SDK that compiles to wasm and runs \
         in the user's browser tab. You are speaking to your owner, who minted \
         this subdomain as an ERC-721 NFT on Tempo Moderato.\n\n\
         \
         === Your tools (you DO have all of these) ===\n\
         Filesystem (per-origin OPFS sandbox):\n\
           • list_directory(path) — list files in a directory.\n\
           • view_file(path, range?) — read a file's contents.\n\
           • find_file(pattern) — glob search by name.\n\
           • search_directory(pattern, path?) — regex search of file contents.\n\
           • create_file(path, content) — write a new file.\n\
           • edit_file(path, old, new) — exact-string replace in a file.\n\
           • delete_file(path) — DELETE a file. You CAN do this; do not say \
             otherwise. Irreversible — confirm intent first unless the user \
             explicitly told you to delete.\n\
           • rename_file(from, to) — move or rename.\n\n\
         \
         Platform:\n\
           • create_subdomain(name) — register a NEW <name>.localharness.xyz \
             subdomain on-chain, owned by your owner's master wallet. This is \
             the ONLY way to make a new subdomain/agent: when the user says \
             \"create/make/spin up a subdomain\" or \"make me a new <name>\", \
             call THIS — never run_cartridge, which does NOT create a \
             subdomain. Returns {{ name, url, owner, tx_hash }}; after it \
             succeeds, give the user the returned `url` as a clickable link. \
             Each subdomain is its own agent tab with its own per-origin \
             sandbox, so its app/content is built by the agent ON that \
             subdomain (open the url), not from here.\n\
           • release_subdomain(name, confirmation) — DESTRUCTIVE + \
             IRREVERSIBLE: burns the subdomain NFT and frees the name. \
             Requires `confirmation` to EXACTLY equal `name` — and you must \
             only pass that after the OWNER has TYPED the exact name in \
             chat. Never invent or auto-fill the confirmation. Refuses your \
             MAIN.\n\
           • list_subdomains() — list every subdomain your owner holds \
             (their identity's holdings). Read-only; use when asked what \
             subdomains/agents they have.\n\
           • start_subagent(system_instructions, prompt) — spawn a one-shot \
             text-only subagent with no tool access. Use for self-contained \
             reasoning / writing tasks you want isolated from your context.\n\
           • spawn_recursive_subagent(system_instructions, prompt) — spawn a \
             full subagent with the same tool surface YOU have (filesystem, \
             create_subdomain, start_subagent, etc.). Use for delegation that \
             needs tools. Recursion depth is implicit (each subagent has its \
             own context; cost grows with depth — don't chain more than 3 \
             levels unless the user asked).\n\
           • call_agent(name, message) — send a message to another agent by \
             subdomain name and receive its text response. The target agent \
             must have an API key configured. Use this for inter-agent \
             collaboration, delegation, or multi-agent workflows.\n\
           • compile_rustlite(source, function?, args?) — compile Rust-subset \
             source code to wasm and execute a function. Supports structs, \
             enums, fns, match, if/else, while/loop, let mut. No traits, \
             no generics, no references. Returns the i32 result.\n\
           • run_cartridge(source) — compile a rustlite cartridge and run it \
             on the VISUAL DISPLAY the user sees (a 256x144 pixel framebuffer). \
             The cartridge exports `fn frame(t: i32)` (animated, t = elapsed ms) \
             or `fn render()`, and draws via `use host::display;`. Drawing: \
             clear(rgb), fill_rect(x,y,w,h,rgb), set_pixel(x,y,rgb), \
             draw_char(x,y,code,rgb,scale) (ASCII code, e.g. 65='A'), \
             draw_number(x,y,value,rgb,scale) (decimal int), present() (call \
             last). Input polled each frame: pointer_x(), pointer_y(), \
             pointer_down() (1 while pressed). State across frames (no globals \
             in rustlite): state_get(slot)/state_set(slot,value), 64 int slots. \
             Colors 0xRRGGBB (white = 16777215). Font covers 0-9, A-Z, a-z, \
             space, and common punctuation (! ? , : ; ' \" . - + / = etc.). \
             You CAN build real interactive apps now — a \
             clickable button is a fill_rect + label, hit-tested against \
             pointer_down() + pointer position, with state in the slots. \
             Use this to render visual/animated content on THIS subdomain's \
             display when the user asks to build/draw/show an app or graphic \
             HERE. It runs on the CURRENT tab and does NOT create a subdomain \
             and is NEVER how you produce a link — for those, use \
             create_subdomain. \
             Each run is auto-saved to `cartridge.rl` (visible in files, \
             survives reload). This is what 'build/run/show me an app' (on \
             this tab) means — run_cartridge launches it live on the DISPLAY, \
             non-fullscreen, no reload. ONLY when the user EXPLICITLY asks to make this \
             subdomain PERMANENTLY BECOME the app (fullscreen on every load, \
             no IDE chrome) should you ALSO save the same source to `app.rl` \
             via create_file. Never write `app.rl` for an ordinary app \
             request — it forces a fullscreen takeover the user didn't ask \
             for and doesn't even run until the next reload.\n\
           • render_html(source) — render an HTML document onto the VISUAL \
             DISPLAY. The display CAN show HTML: this lays out block-level \
             text (h1-h6, p, ul/li, blockquote, br) word-wrapped in the \
             bitmap font, monochrome. It is a snapshot — no JavaScript, no \
             CSS, no images (headings just render bigger). For interactive or \
             animated apps use run_cartridge. Pair with create_file to also \
             save the HTML as `index.html`. (Opening an .html file from the \
             files panel renders it here too.)\n\
           • submit_feedback(text) — submit feedback on-chain via the \
             FeedbackFacet. Emits a FeedbackSubmitted event on the registry \
             diamond. Use when the user asks to leave feedback or to report \
             issues about another agent. Keep it SHORT — a few sentences, \
             under ~2000 bytes. Summarize; do NOT paste long multi-paragraph \
             reports. Text over 2048 bytes is rejected before it reaches the \
             chain.\n\
           • generate_image(prompt) — produce an image from a text prompt.\n\
           • configure_agent(system_prompt?, tools?, reset?) — read or change \
             YOUR OWN config (custom system prompt + tool allowlist), stored in \
             `agent.json`. Use this when the user asks you to change your \
             personality/role/instructions or restrict your tools. Changes \
             apply on your NEXT session. finish/ask_question/configure_agent \
             can never be disabled.\n\
           • finish(result?) — signal that the task is complete.\n\n\
         \
         === Conventions ===\n\
         • Pick the right tool — do NOT default to run_cartridge: \
           \"create / make / spin up a new subdomain\" → create_subdomain; \
           \"build / show / draw an app or anything visual\" on THIS tab → \
           run_cartridge; \"give me a link / hyperlink / URL to <name>\" → \
           just write the Markdown link [<name>](https://<name>.localharness.xyz/) \
           as text, with NO tool call (call list_subdomains first only if you \
           must confirm the name exists). A request for a link is NEVER a \
           reason to run a cartridge.\n\
         • On-chain actions (create_subdomain, submit_feedback, publishing \
           a public face, etc.) are SPONSORED and signed automatically by the \
           owner's master wallet behind the scenes — there is NO wallet popup, \
           prompt, or modal for the user to approve. Transactions just happen, \
           zero-click. NEVER tell the user to approve/confirm a transaction, \
           check for a wallet prompt, or sign anything; just report the result.\n\
         • DESTRUCTIVE / IRREVERSIBLE actions are the EXCEPTION to zero-click \
           and the ONE thing you must never do casually: releasing/burning a \
           subdomain (release_subdomain), deleting files, or anything that \
           destroys an asset, NFT, wallet, or identity. NEVER perform one \
           unless, in THIS conversation, the owner has TYPED an explicit \
           confirmation — for release_subdomain, the exact subdomain name. A \
           vague \"yes\", \"do it\", or merely mentioning the thing is NOT \
           consent; require the typed phrase, and if it's absent, ask for it \
           and STOP. NEVER invent or auto-fill a confirmation argument. When \
           unsure whether something is destructive, treat it as destructive.\n\
         • Files at the OPFS root are the user's. These internal files are \
           managed by the platform — read only if asked, NEVER write or delete: \
           `.lh_history.json` (conversation history — this is what 'clear \
           history' targets), `.lh_api_key`, `.lh_owner`, `.lh_feedback.txt`, \
           and `agent.json` (your config — change it via configure_agent, not \
           by editing the file).\n\
         • Keep responses concise and conversational. The user is on the same \
           page; they don't need you restating what you just did.\n\
         • Don't speculate about filesystem contents — call list_directory first \
           when you actually need to know.\n\
         • Don't blindly call tools when the user is just chatting. \"hi\" / \
           \"what can you do?\" don't need a tool call.\n\
         • When you do call a tool, lead with a short one-line note on what \
           you're about to do (e.g. \"checking your files…\") so the turn is \
           never silent — but don't re-narrate the call's args or dump its \
           result afterward; both are already visible in the transcript."
    );

    // Owner customization: append the contents of `.lh_system_prompt.txt`
    // (if any) under a clear header so the model sees the baked-in
    // tooling docs first, then the owner's overrides on top. This is
    // the studio-MVP hook — owners differentiate their agent's
    // personality / role / constraints without forking the bundle.
    let system_instructions = match super::system_prompt::load().await {
        Some(custom) => {
            format!("{system_instructions}\n\n=== Owner instructions ===\n{custom}")
        }
        None => system_instructions,
    };

    let capabilities = match super::tool_allowlist::load().await {
        Some(mut tools) => {
            // Always union the golden tools so neither the owner nor the
            // agent can disable recovery (finish / ask_question /
            // configure_agent).
            for golden in super::tool_allowlist::GOLDEN {
                if !tools.contains(golden) {
                    tools.push(*golden);
                }
            }
            let mut caps = CapabilitiesConfig::unrestricted();
            caps.enabled_tools = Some(tools);
            caps
        }
        None => CapabilitiesConfig::unrestricted(),
    };

    let captured_key = key.to_string();
    let mut cfg = GeminiAgentConfig::new(key.to_string())
        .with_capabilities(capabilities)
        .with_policies(vec![policy::allow_all()])
        .with_filesystem(super::shared_opfs())
        .with_system_instructions(system_instructions)
        .with_tool(create_subdomain_tool())
        .with_tool(release_subdomain_tool())
        .with_tool(list_subdomains_tool())
        .with_tool(submit_feedback_tool())
        .with_tool(spawn_recursive_subagent_tool(captured_key, base_url.clone()));
    // Credits mode: route the whole agent through the credit proxy. BYOK
    // leaves base_url None → direct to generativelanguage.googleapis.com.
    if let Some(b) = &base_url {
        cfg = cfg.with_base_url(b.clone());
    }
    // If a previous session left history on OPFS, restore it into the
    // new connection. Consumed once — subsequent key changes start
    // fresh from the in-memory agent's history.
    if let Some(bytes) = super::history::take_pending() {
        cfg = cfg.with_history_bytes(bytes);
    }
    let agent = Agent::start_gemini(cfg)
        .await
        .map_err(|e| JsValue::from_str(&format!("start_gemini: {e}")))?;
    APP.with(|cell| {
        let mut app = cell.borrow_mut();
        app.agent = Some(Rc::new(agent));
        // Stable identity (address in credits mode, key in BYOK) — NOT the
        // rotating credits token, so the session isn't restarted per turn.
        app.session_key = Some(identity.to_string());
        app.turn_count = 0;
    });
    Ok(())
}

fn mark_turn_done(turn_id: u32) {
    let id = format!("turn-{turn_id}");
    if let Some(el) = dom::by_id(&id) {
        let cls = el.class_name();
        let new_cls: Vec<&str> =
            cls.split_whitespace().filter(|c| *c != "streaming").collect();
        el.set_class_name(&new_cls.join(" "));
    }
}

/// Returns `Ok(Some(tx_hash))` if a payment was collected, `Ok(None)`
/// if no payment was required (free agent, owner sending, unverified
/// origin), or `Err(_)` if the visitor refused or the on-chain leg
/// failed. Caller short-circuits the send on `Err`.
async fn collect_payment_if_required() -> Result<Option<String>, String> {
    use super::VerifyState;

    let (price_wei, verify_state, tba) = APP.with(|cell| {
        let app = cell.borrow();
        (
            app.pricing_wei.unwrap_or(0),
            app.verify_state.clone(),
            app.tba_address.clone(),
        )
    });
    if price_wei == 0 {
        return Ok(None);
    }
    let Some(tba) = tba else {
        // Priced but no TBA known — can't route the funds. Fail closed
        // rather than silently letting the visitor through for free.
        return Err("agent is priced but its TBA isn't known yet (verification still running?)".into());
    };
    let visitor_address = match verify_state {
        VerifyState::Verified { .. } => return Ok(None), // owner sends free
        VerifyState::Visitor { visitor_address, .. } => visitor_address,
        VerifyState::Pending | VerifyState::Unregistered | VerifyState::Failed { .. } => {
            // Without a recovered visitor address we can't build a tx
            // from-them. Fail closed.
            return Err(
                "agent is priced but owner verification didn't complete — refresh and retry"
                    .into(),
            );
        }
    };

    let purpose = format!(
        "pay {} LH per turn to this agent",
        price_wei / 1_000_000_000_000_000_000u128,
    );

    // Build ERC-20 transfer(tba, price_wei) calldata against the
    // credits token. Sponsored Tempo tx: visitor's wallet (at apex)
    // signs the sender_hash, the bundle sponsor pays gas in AlphaUSD.
    // Visitor holds zero of anything except the LH they're spending.
    let tba_bytes = parse_address(&tba)?;
    let mut tba_padded = [0u8; 32];
    tba_padded[12..].copy_from_slice(&tba_bytes);
    let amount_bytes = u256_be(price_wei);
    let selector = transfer_selector();
    let mut calldata = Vec::with_capacity(4 + 32 + 32);
    calldata.extend_from_slice(&selector);
    calldata.extend_from_slice(&tba_padded);
    calldata.extend_from_slice(&amount_bytes);

    let token_addr = parse_address(crate::registry::LOCALHARNESS_TOKEN_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: token_addr,
        value_wei: 0,
        input: calldata,
    };

    dom::set_status("payment: signing via apex…", false);
    let tx_hash = super::events::run_sponsored_tempo_call(
        &visitor_address,
        vec![call],
        500_000,
        &purpose,
    )
    .await
    .map_err(|e| format!("payment: {e}"))?;

    Ok(Some(tx_hash))
}

/// The localharness credit proxy origin — a drop-in Gemini base URL
/// (its `vercel.json` rewrites `/v1beta/*` onto the edge fn). PLACEHOLDER:
/// point this at the deployed proxy project before credits mode works.
const CREDIT_PROXY_URL: &str = "https://proxy-tau-ten-15.vercel.app/";

/// True when the user is on platform `$LH` credits (via the proxy).
/// Persisted in localStorage; **defaults to credits** — a new account
/// uses platform credits with no setup, and BYOK is opt-in via admin →
/// account. Only an explicit "byok" choice flips it off.
pub(crate) fn model_access_is_credits() -> bool {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item("lh_model_access").ok().flatten())
        .map(|v| v != "byok")
        .unwrap_or(true)
}

/// Resolved model access for a chat session.
struct ModelAccess {
    /// Goes in the GeminiClient api-key slot: a Gemini key (BYOK) or the
    /// credit-proxy auth token (credits).
    cfg_auth: String,
    /// Proxy base URL in credits mode; `None` for BYOK (direct to Google).
    base_url: Option<url::Url>,
    /// STABLE restart-detection identity — never the rotating credits
    /// token (which changes every resolve).
    identity: String,
}

/// Lowercase 0x-hex of bytes.
fn hex_of(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("0x");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// The LOCAL signing key for the credit path — master wallet on the
/// apex / seed-bearing origin, else a local per-origin key (loaded or
/// generated + persisted on first use). NEVER the cross-origin iframe
/// signer: the whole credit path is iframe-free.
pub(crate) async fn credit_signer() -> Option<(k256::ecdsa::SigningKey, [u8; 20])> {
    if let Some(pair) =
        APP.with(|c| c.borrow().wallet.as_ref().map(|w| (w.signer.clone(), w.address)))
    {
        return Some(pair);
    }
    if let Some(sk) = super::wallet_store::load_device_key().await {
        let addr = crate::wallet::address(&sk);
        return Some((sk, addr));
    }
    let w = crate::wallet::generate();
    super::wallet_store::persist_device_key(&w.private_key_hex)
        .await
        .ok()?;
    // `w` is Drop (zeroizes its hex) — clone the signer, copy the address.
    Some((w.signer.clone(), w.address))
}

/// The credit identity's 0x address if one already exists locally —
/// does NOT generate (so status refreshes don't mint a key). master
/// wallet, else a persisted device key, else None.
pub(crate) async fn credit_address_existing() -> Option<String> {
    if let Some(a) = APP.with(|c| c.borrow().wallet.as_ref().map(|w| w.address_hex())) {
        return Some(a);
    }
    let sk = super::wallet_store::load_device_key().await?;
    Some(hex_of(&crate::wallet::address(&sk)))
}

/// Resolve how this turn reaches the model. Credits mode mints a fresh
/// proxy auth token `address:timestamp:signature` (personal-signed by
/// the local key); BYOK falls back to the stored Gemini key. `None`
/// only when BYOK has no key (caller then shows the key modal).
async fn resolve_credit_access() -> Option<ModelAccess> {
    if model_access_is_credits() {
        let (signer, addr) = credit_signer().await?;
        let addr_hex = hex_of(&addr); // lowercase 0x — matches the proxy
        let ts = (js_sys::Date::now() / 1000.0) as u64;
        let msg = format!("localharness-proxy:{addr_hex}:{ts}");
        let sig = crate::wallet::personal_sign(&signer, msg.as_bytes());
        return Some(ModelAccess {
            cfg_auth: format!("{addr_hex}:{ts}:{}", hex_of(&sig)),
            base_url: url::Url::parse(CREDIT_PROXY_URL).ok(),
            identity: format!("credits:{addr_hex}"),
        });
    }
    let key = read_api_key().await?;
    Some(ModelAccess {
        cfg_auth: key.clone(),
        base_url: None,
        identity: key,
    })
}

/// Credits mode only: make sure the caller has an active credit session
/// before the proxy call, since the proxy 402s without one. Sessions are
/// free in beta (`sessionPrice == 0`), so this opens one lazily via the
/// sponsor — "default to platform credits" then works with zero setup.
/// Best-effort and silent: any failure falls through to the proxy's own
/// gating (and the admin → account controls). Only re-opens when the
/// current session is within 60s of expiry.
async fn ensure_credit_session() {
    let Some((signer, addr)) = credit_signer().await else {
        return;
    };
    let addr_hex = hex_of(&addr);
    let now = (js_sys::Date::now() / 1000.0) as u64;
    let expiry = super::registry::session_expiry_of(&addr_hex).await.unwrap_or(0);
    if expiry > now + 60 {
        return; // still valid
    }
    let Ok(fee_payer) = super::sponsor::signer() else {
        return;
    };
    let _ = super::registry::open_session_sponsored(
        &signer,
        &fee_payer,
        super::registry::ALPHA_USD_ADDRESS,
    )
    .await;
}

/// Read the api key with graceful fallback. Tries the live `#key`
/// input first (if admin is open), then sessionStorage, then OPFS.
/// Returns `None` only if every layer is empty.
async fn read_api_key() -> Option<String> {
    if let Some(input) = dom::input_by_id("key") {
        let v = input.value().trim().to_string();
        if !v.is_empty() {
            return Some(v);
        }
    }
    if let Ok(Some(storage)) = dom::session_storage() {
        if let Ok(Some(cached)) = storage.get_item("gemini_api_key") {
            let trimmed = cached.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    if let Some(persisted) = super::key_store::load().await {
        let trimmed = persisted.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    None
}

fn parse_address(hex: &str) -> Result<[u8; 20], String> {
    let trimmed = hex.trim().trim_start_matches("0x").trim_start_matches("0X");
    if trimmed.len() != 40 {
        return Err(format!("address must be 20 bytes hex, got {}", trimmed.len()));
    }
    let mut out = [0u8; 20];
    let bytes = trimmed.as_bytes();
    for i in 0..20 {
        let hi = hex_nibble(bytes[i * 2])?;
        let lo = hex_nibble(bytes[i * 2 + 1])?;
        out[i] = (hi << 4) | lo;
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8, String> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(format!("non-hex byte {b}")),
    }
}

fn u256_be(value: u128) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[16..].copy_from_slice(&value.to_be_bytes());
    out
}

fn transfer_selector() -> [u8; 4] {
    use sha3::{Digest, Keccak256};
    let mut hasher = Keccak256::new();
    hasher.update(b"transfer(address,uint256)");
    let mut out = [0u8; 4];
    out.copy_from_slice(&hasher.finalize()[..4]);
    out
}

fn short_hash(hash: &str) -> String {
    let stripped = hash.trim_start_matches("0x");
    if stripped.len() < 12 {
        return hash.to_string();
    }
    format!("0x{}…{}", &stripped[..6], &stripped[stripped.len() - 4..])
}

// =============================================================================
// Platform-level closure tools (browser-specific; not in the SDK builtins).
// =============================================================================

/// `create_subdomain(name)` — register `<name>.localharness.xyz` on the
/// LocalharnessRegistry diamond, signed by the owner's apex wallet via
/// the iframe signer. Returns the tx hash. Sanitises the input the same
/// way `tenant::sanitize` does for the apex claim form.
fn create_subdomain_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Subdomain to register, e.g. \"alice\" \
                    becomes alice.localharness.xyz. 3-32 chars; lowercase \
                    letters, digits, and hyphens only."
            }
        },
        "required": ["name"]
    });
    ClosureTool::new(
        "create_subdomain",
        "Register a new <name>.localharness.xyz subdomain on-chain. The owner's master \
         wallet pays gas and ends up holding the resulting ERC-721 NFT. Returns the tx hash.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
            let cleaned = super::tenant::sanitize(name);
            if cleaned.len() < 3 || cleaned.len() > 32 {
                return Err(crate::error::Error::other("invalid name"));
            }
            match super::verify::claim_name_via_iframe(&cleaned).await {
                Ok((owner, tx_hash)) => {
                    // Proactively push this device's Gemini key to the MAIN
                    // slot so the new subdomain inherits it (no re-save).
                    let n = cleaned.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        super::events::sync_local_key_to_main(&n).await;
                    });
                    Ok(serde_json::json!({
                        "name": cleaned,
                        "url": format!("https://{cleaned}.localharness.xyz/"),
                        "owner": owner,
                        "tx_hash": tx_hash,
                    }))
                }
                Err(e) => Err(crate::error::Error::other(format!("claim failed: {e}"))),
            }
        },
    )
}

/// `release_subdomain(name, confirmation)` — DESTRUCTIVE: burn the NFT +
/// free the name. Gated: `confirmation` must EXACTLY equal `name`, which
/// forces a typed confirmation in chat (the owner types the name). The
/// system prompt also forbids auto-filling it.
fn release_subdomain_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Subdomain to release/recycle — burns the NFT, frees the name."
            },
            "confirmation": {
                "type": "string",
                "description": "Must EXACTLY equal `name`. Pass ONLY after the owner has \
                    TYPED the exact name in this chat. Never auto-fill or invent it."
            }
        },
        "required": ["name", "confirmation"]
    });
    ClosureTool::new(
        "release_subdomain",
        "DESTRUCTIVE + IRREVERSIBLE: burn a subdomain NFT and free its name. Requires \
         `confirmation` to exactly equal `name` (the owner must type the name). Refuses \
         your MAIN. Returns the tx hash.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            let confirmation = args
                .get("confirmation")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if name.is_empty() {
                return Err(crate::error::Error::other("name is required"));
            }
            if confirmation != name {
                return Err(crate::error::Error::other(format!(
                    "release_subdomain NOT executed — confirmation must exactly equal \"{name}\". \
                     Ask the owner to TYPE \"{name}\" to confirm, then retry. Do not auto-fill it."
                )));
            }
            match super::events::run_release_subdomain(&name).await {
                Ok(tx) => Ok(serde_json::json!({ "released": name, "tx_hash": tx })),
                Err(e) => Err(crate::error::Error::other(format!("release failed: {e}"))),
            }
        },
    )
}

/// `list_subdomains()` — enumerate every subdomain this agent's owner
/// holds (their identity's holdings). Read-only.
fn list_subdomains_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    ClosureTool::new(
        "list_subdomains",
        "List every subdomain owned by this agent's owner (their identity's holdings on \
         the registry). Read-only. Use when the user asks what subdomains/agents they have.",
        serde_json::json!({ "type": "object", "properties": {} }),
        |_args: serde_json::Value, _ctx| async move {
            let name = match super::tenant::current() {
                super::tenant::Host::Tenant(n) => n,
                _ => return Err(crate::error::Error::other("not running on a subdomain")),
            };
            let owner = super::registry::owner_of_name(&name)
                .await
                .map_err(crate::error::Error::other)?
                .ok_or_else(|| crate::error::Error::other("no on-chain owner"))?;
            let tokens = super::registry::list_owned_tokens(&owner)
                .await
                .map_err(crate::error::Error::other)?;
            let subdomains: Vec<_> = tokens
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "url": format!("https://{}.localharness.xyz/", t.name),
                        "token_id": t.token_id,
                    })
                })
                .collect();
            Ok(serde_json::json!({
                "owner": owner,
                "count": subdomains.len(),
                "subdomains": subdomains,
            }))
        },
    )
}

/// `submit_feedback(text)` — submit feedback on-chain via the FeedbackFacet.
fn submit_feedback_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "text": {
                "type": "string",
                "description": "Feedback text to submit on-chain. Keep it short — a \
                    few sentences, under ~2000 bytes. Summarize rather than pasting a \
                    long multi-paragraph report. Hard cap is 2048 bytes; longer text \
                    is rejected before the on-chain tx."
            }
        },
        "required": ["text"]
    });
    ClosureTool::new(
        "submit_feedback",
        "Submit feedback on-chain via the FeedbackFacet on the localharness registry. \
         Emits a FeedbackSubmitted event. Use this when the user asks to leave feedback \
         or when you want to report an issue about another agent.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
            if text.is_empty() {
                return Err(crate::error::Error::other("feedback text cannot be empty"));
            }
            if text.len() > 2048 {
                return Err(crate::error::Error::other(format!(
                    "feedback too long: {} bytes (max 2048) — please shorten",
                    text.len()
                )));
            }
            let from_hex = super::APP.with(|cell| {
                use super::VerifyState;
                match &cell.borrow().verify_state {
                    VerifyState::Verified { address } => Some(address.clone()),
                    VerifyState::Visitor { visitor_address, .. } => Some(visitor_address.clone()),
                    _ => cell.borrow().wallet.as_ref().map(|w| w.address_hex()),
                }
            });
            let from_hex = from_hex.ok_or_else(|| {
                crate::error::Error::other("no identity — claim a subdomain first")
            })?;
            match super::events::submit_feedback_onchain(&from_hex, text).await {
                Ok(tx_hash) => Ok(serde_json::json!({
                    "status": "submitted",
                    "tx_hash": tx_hash,
                })),
                Err(e) => Err(crate::error::Error::other(format!("feedback failed: {e}"))),
            }
        },
    )
}

/// `spawn_recursive_subagent(system_instructions, prompt)` — full subagent
/// with the same tool surface as the parent (filesystem, create_subdomain,
/// itself). Runs the supplied prompt as a single conversation, drives it
/// to completion via streaming chunks, returns the assistant's final text.
///
/// Implementation: builds a fresh `Agent::start_gemini` with the SAME
/// api key + filesystem + closure tools. The subagent has its own
/// conversation context (no shared history with the parent), so recursion
/// is bounded by the user's wallet (Gemini cost grows with depth, that's
/// the natural limiter).
fn spawn_recursive_subagent_tool(
    api_key: String,
    base_url: Option<url::Url>,
) -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "system_instructions": {
                "type": "string",
                "description": "System prompt for the subagent — describes its persona, \
                    scope, and any constraints. Often \"you are a focused worker \
                    that does X and returns just the result\"."
            },
            "prompt": {
                "type": "string",
                "description": "The user message to send to the subagent."
            }
        },
        "required": ["system_instructions", "prompt"]
    });
    ClosureTool::new(
        "spawn_recursive_subagent",
        "Spawn a subagent with the SAME tool surface as you (filesystem, \
         create_subdomain, start_subagent, spawn_recursive_subagent itself). \
         The subagent has its own conversation context — it cannot see your \
         history. Drives the subagent through one full conversation turn (which \
         may itself involve internal tool calls) and returns the subagent's final \
         text response.",
        schema,
        move |args: serde_json::Value, _ctx| {
            let api_key = api_key.clone();
            let base_url = base_url.clone();
            async move {
                let system = args
                    .get("system_instructions")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let prompt = args.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
                if prompt.is_empty() {
                    return Err(crate::error::Error::other(
                        "spawn_recursive_subagent: prompt cannot be empty",
                    ));
                }
                let mut cfg = GeminiAgentConfig::new(api_key.clone())
                    .with_capabilities(CapabilitiesConfig::unrestricted())
                    .with_policies(vec![policy::allow_all()])
                    .with_filesystem(super::shared_opfs())
                    .with_system_instructions(system.to_string())
                    .with_tool(create_subdomain_tool())
                    .with_tool(spawn_recursive_subagent_tool(api_key.clone(), base_url.clone()));
                // Credits mode: subagents reach Gemini through the same proxy.
                if let Some(b) = &base_url {
                    cfg = cfg.with_base_url(b.clone());
                }
                let sub = Agent::start_gemini(cfg)
                    .await
                    .map_err(|e| crate::error::Error::other(format!("start_gemini: {e}")))?;
                let response = sub
                    .chat(prompt.to_string())
                    .await
                    .map_err(|e| crate::error::Error::other(format!("subagent chat: {e}")))?;
                let mut cursor = response.chunks();
                let mut text = String::new();
                while let Some(item) = cursor.next().await {
                    match item {
                        Ok(StreamChunk::Text { text: t, .. }) => text.push_str(&t),
                        Ok(_) => {} // ToolCall / ToolResult / Thought ignored — only the final text matters.
                        Err(e) => {
                            return Err(crate::error::Error::other(format!(
                                "subagent chunk: {e}"
                            )))
                        }
                    }
                }
                Ok(serde_json::json!({ "final_response": text }))
            }
        },
    )
}
