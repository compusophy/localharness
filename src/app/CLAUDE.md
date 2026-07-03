# src/app — browser IDE subsystem spec

> Module-owned context (auto-loaded when an agent works in `src/app/`). The root
> `CLAUDE.md` is a whole-repo map; THIS file is the source of truth for the
> browser UI so no one has to re-derive it from the CSS again. Keep it tight;
> when you change a UI subsystem, update the matching section HERE in the same
> commit. `feature=browser-app` + wasm32 only.

## Hard rules (non-negotiable — the user enforces these)
- **No imperative DOM.** All HTML from `maud` templates (`templates.rs`); the only
  DOM ops are `dom::{swap_inner,swap_outer,append_html,set_attr}` at fixed ids
  (HTMX-style fragment swaps). ONE delegated `click/keydown/submit/input` listener
  set in `events/mod.rs` dispatches by `data-action`/`data-arg`. Zero `Closure::wrap`
  outside those listeners + the few platform shims (visualViewport, etc.).
- **Monochrome brutalist.** Tokens only (`--fg --bg --muted --border --accent --panel
  --panel-2`), IBM Plex Mono, NO rounded-by-default, NO colored accents, NO shadows
  as chrome, NO emojis. The ONE intentional color is the stop button red (#d83a3a).
- **No iframes** for in-app rendering. **No JS alert/confirm/prompt** — use a
  template-swapped panel with inline [confirm]/[cancel].
- **DISPLAY is a framebuffer** (canvas pixel buffer + worker), not DOM/iframe.

## THE ONE-BOX INPUT RULE (this has caused repeated rage — obey it)
Every input/field must read as **exactly ONE box**, and on focus **the FIELD
highlights, never its container**. Concretely:
- The text field carries the border; it highlights via `input:focus,textarea:focus
  { border-color: var(--accent) }` (styles.css). Set `outline:none` on the field so
  the border is the only ring.
- Inputs/textareas are DELIBERATELY EXCLUDED from the global `:focus-visible`
  outline rule (styles.css) — that outline drew a SECOND box *inside* the lit field.
  Do NOT re-add `input`/`textarea` to it.
- A container that holds a field (a modal/popover) must NOT also light up on focus
  (no `:focus-within { border-color }` competing with the field). Its border is
  STATIC. (#64/rawfeedback: "highlighted container inside a highlighted container".)

## Overlay / modal system (notif bell · feedback · admin cog)
Three header overlays, now UNIFIED (do not re-fork them into separate behaviors):
- **Markup** (`templates.rs`): each is a panel inside its header wrap
  (`.notif-bell-wrap` / `.feedback-bug-wrap` / `.header-admin`). Admin's visible box
  is `.admin-dialog.admin-sheet` inside `#header-admin-panel`.
- **Positioning** (`styles.css`, the "UNIFIED HEADER MODALS" rule): all three are
  `position:fixed`, CENTERED (`top/left:50% + translate(-50%,-50%)`), clamped
  (`width:min(360px,100vw-2pad)`, `max-height:100dvh-6pad`), one z-layer
  (`--z-menu`). They are NOT anchored under their trigger button (that offset ran
  the notif panel off-screen-left). They stay DOM children of their wrap so the
  click-outside check still works; `fixed` only relocates them visually.
- **Mutual exclusion + toggle** (`events/admin.rs`): exactly one open at a time.
  Each open path calls `close_all_header_overlays()` first (closes notif + feedback
  + admin + brand menu — all idempotent hidden-swaps). Each is a real toggle
  (second tap closes). `header_admin_open()` = `#admin-dialog` present.
- **Dismiss**: outside-click + ESC in `events/mod.rs` use `.closest(".*-wrap")`.
  Brand menu is a native `<details>`; we clear its `open` attr ourselves.
- If you add a 4th overlay: give it a `*-wrap`, add it to `close_all_header_overlays`,
  reuse the unified CSS rule + `--z-menu`, make it a real toggle.

**Admin cog = a chromeless disclosure SHEET** (`templates::admin_dropdown_{apex,tenant}`,
`.admin-dialog.admin-sheet`) — NO title bar, NO × (it GENERALIZES from the notif/feedback
panels; dismiss is outside-click · ESC · a second cog tap, not a close button). Tab-less:
identity + the `$LH` balance (`admin_balance_line`, `#credits-balance`) sit at REST; every
other concern collapses into a native `<details>` group via `admin_group(id,label,body)` —
apex: funds · devices · app & display · security; tenant: a `#financial-slot` head + agent
(model · public face · a `<details>` "advanced" sub-group = persona/x402/allowlist) · funds ·
app & display · security. Disclosure is the native `<details>` itself (no `Action`/handler,
no `Closure`); the collapsed body stays in the DOM so `header_admin_toggle`'s async prefill
ids still resolve. Apex `security` is `has_wallet`-gated (it shares `#import-slot` with the
no-wallet identity path). This REPLACED the old `.admin-tabbed` tabs + `ShowAdminTab` /
`Reveal`+`HideSecurity` swaps; the on-chain enable-notifications + test buttons and the
feedback-on-chain toggle were CUT (push rides the bell and is fully OFF-CHAIN now).

## Turn-status / stage painter (`chat/stage.rs` + `turn_stage.rs`)
Pending-turn cue: ONE pulsing glyph in `#turn-status` (header) + a `data-stage` word
on the pending bubble (`::before{content:attr(data-stage)}`). `begin()` paints an
immediate "starting" so the bubble is never blank before the first real stage
(mobile flicker, #58/T25); `enter()` repaints only on stage change; `end()` clears.
The pure state machine is `crate::turn_stage` (native-tested).

## Mount routing (`mod.rs::mount`) — brief (full detail in root CLAUDE.md)
`?signer=1`/`?rpc=1` → headless chromes (early return; NOT framed). Else classify via
`tenant::current()` → Apex (identity-gated) / Tenant (owner-verify) / Other.
`apply_render_modes` runs first: theme + the MOBILE-FIRST frame (desktop defaults to
the 9:16 `preview-mobile` column; real <=600px phones + signer/rpc excluded).
Keyboard occlusion on mobile is handled by `install_keyboard_viewport_fix`
(visualViewport → `--lh-vh`/`--lh-vv-top`/`.lh-kb`).

## Files (orientation)
`mod.rs` mount/routing · `templates.rs` ALL maud HTML · `dom.rs` swap shims ·
`events/` the delegated listeners + `Action` enum + per-domain handlers
(`admin.rs` owns the overlays; `identity.rs` the onboarding/seed flows —
dispatch arms stay one-line delegations) · `chat/` the turn loop + stage painter (session.rs
assembles the tool surface ONCE — `chat_toolset()` + `wire_shared_session!` feed
every backend branch; add new chat tools there, never per-backend; source guard
`tests/chat_toolset_single_source.rs`; router_wire.rs = the INTENT-ROUTER gate:
`run_send` classifies each message via the `crate::router` pure core BEFORE any
metered work — exact-allowlist free routes only (balance/credits reads, the
files/display/terminal toggles, a tiny docs FAQ), everything else untouched;
'!' prefix always forces the model; the gate is DEFAULT OFF (opt-in per
session via `/router on`, sessionStorage `lh_router_on`, default pinned by
`router::router_enabled` natively) until a browser tab-E2E pass; free turns
are transcript-only, never in agent history; widen the
free tier ONLY by adding exact phrases to `router::FREE_PHRASES`) ·
`notifications.rs` bell + push (per-device `dev` dedup; enrollment is OFF-CHAIN —
POST /api/push-sub to the proxy's GitHub store, NEVER a sponsored on-chain write:
that path failed with "insufficient funds" for unfunded mainnet users) ·
`display/` framebuffer
(`mod.rs` run/launch surface + re-exports; `worker.rs` spawn/watchdog/RUN_GEN/
RUN_OUTCOME + the onmessage router; `surface.rs` canvas mount + overlay chrome +
pointer state + embed-card plumbing + broadcast-composer UI; `bridge/` one module
per host capability — feed/compose/http/mp/chat/audio — thread_local state
module-private per bridge). The pure HTML→framebuffer rasterizer is hoisted to
`crate::html_fb` (native-tested).
