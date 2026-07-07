# Cross-Platform Rust Architecture — Research & Plan

> **Status: RESEARCH + PLAN, not executed.** Planning input for **Fable** to act on
> after the usage reset. No refactor here. No second `create`. One repo, one core.
>
> **Provenance:** produced by a 5-lane current-web research sweep (workflow
> `wf_7ef75b5f-ea8`, run 2026-07-07) — every framework/version/platform claim was
> WebSearch/WebFetch-sourced and cited, *not* recalled from a model's training
> (which lags; e.g. it would have said "latest Gemini is 2.5"). Even so: the web
> moves faster than any snapshot. **Fable: re-verify the load-bearing version facts
> (Tauri, Dioxus, egui, Slint, the Vercel Rust runtime, iOS WebGPU) before
> committing — they were true on 2026-07-07 and no earlier.** Full cited lane briefs
> are archived in the run transcript.

---

## 0. The goal (in the maintainer's terms)

> "The EXACT SAME app on: website desktop, phone browser (all kinds), iOS native
> (Swift? idk), a native Android app (AVOID EXPO), and headless CLI — plus our host
> of API routes. Rust to the full extent, JavaScript minimized to near-zero.
> Isolate the platform layer; don't shittify the clean core; no full rewrite if we
> can help it (real refactors are Fable's). API routes should be first-class
> citizens, because the clients are mostly just using APIs."

That instinct is architecturally correct and the research backs every piece of it.
The rest of this doc turns it into a concrete, phased, low-blast-radius plan.

---

## 1. First-principles framing: three rings, thin edges

Reason from the data-flow, not from "what framework." Everything localharness does
is: **pure logic** ⟶ **an API surface** ⟶ **a thin per-platform shell that draws it
and takes input.** That's three rings, and the maintainer's rules ("isolate, don't
shittify, no second create") are exactly "keep the platform mess at the outer ring."

```
        ┌──────────────────────────────────────────────────────────┐
        │  RING 3 — SHELLS (thin, platform-owned, the ONLY non-core  │
        │  code)   CLI · web-wasm · iOS · Android · desktop          │
        │        each = a small adapter: draw a frame, feed events   │
        └───────────────▲───────────────────────▲──────────────────┘
                        │ same API, same types    │
        ┌───────────────┴───────────────────────┴──────────────────┐
        │  RING 2 — API (Rust)  lh-server: one portable axum Router  │
        │  + lh-api-types: serde DTOs shared by server AND clients   │
        └───────────────▲───────────────────────────────────────────┘
                        │ direct crate dep
        ┌───────────────┴───────────────────────────────────────────┐
        │  RING 1 — CORE (Rust)  the `localharness` crate as-is:      │
        │  SDK · backends · wallet · registry · framebuffer/compose   │
        │  — pure, platform-agnostic, UNTOUCHED by any of this        │
        └────────────────────────────────────────────────────────────┘
```

Consequences that fall straight out of the picture:

- **"API routes are first-class" = Ring 2 is a real, standalone thing** (a portable
  `axum::Router` in a library crate), not a folder of TypeScript handlers. Every
  client — CLI, web, iOS, Android — is a thin consumer of the *same* Router and the
  *same* `serde` types. This is the single highest-leverage change and it's
  independent of the whole UI question.
- **"Clients are just using APIs" is literally true** once Ring 2 exists: a shell's
  job shrinks to (draw the current state, send user events, call the API). The CLI
  is already exactly this. "The CLI is just a version of the CLI" — right; it's a
  Ring-3 shell that happens to render to a terminal.
- **"Isolate, don't shittify, no second create"** = all platform-specific code lives
  in Ring 3 (per-shell) and the Ring-2 host adapters. Ring 1 never learns what
  platform it's on. One Cargo workspace, one core crate, N thin shells. No fork.

---

## 2. Where we are today (grounded, not guessed)

- **JS/TS ≈ 24.5K LOC, and it is *concentrated*, not diffuse rot:**
  - `proxy/*.ts` **≈ 13.7K** — the off-chain server (LLM inference, MCP, cron,
    web-push, telemetry, sponsor relay). *The biggest single target.*
  - `scripts/*.mjs` **≈ 8K** — test/QA tooling. Low priority; not shipped.
  - shipped browser JS **≈ 2.9K** — `cartridge-worker.js` (1913 LOC, the bulk),
    `boot.js` (wasm loader), `sw.js` (push/PWA), `stripe-embed.js` (third-party
    Stripe SDK, JS-only).
  - **The load-bearing insight:** `cartridge-worker.js` is *hand-ported from Rust
    `src/compose.rs`* and parity-tested. You are paying a maintenance tax in the
    language you hate to keep a JS copy of Rust logic in sync. That's the most
    deletable JS in the repo (Phase 2).
- **The iOS "gate" was never a rendering problem.** `templates.rs:1245` only fires
  `ios_unavailable()` for `fresh && is_ios()` — a bare "not available on iOS" at
  *fresh onboarding*. Per CLAUDE.md the real cause is **Safari/WebKit cross-origin
  storage partitioning** breaking the seed/identity flow (the `apex/?signer=1`
  iframe sees an empty OPFS; `seed_pull.rs` is the workaround). **iOS was closed off
  because of storage/identity, not pixels.** This single fact reshapes the plan:
  the thing that fixes iOS is a **single-origin container**, which is exactly what a
  native app gives you for free.
- **localharness already made the framebuffer bet.** It rasterizes HTML→pixels on a
  `<canvas>` (`html_fb`), runs cartridges off-main-thread, and has a "no imperative
  DOM" rule. That is *philosophically the same* as the pure-Rust-GPU UI family
  below — localharness is already 60% of the way down that road, which makes the
  min-JS endgame far cheaper for it than for anyone starting fresh.

---

## 3. What the research found (condensed + cited; full briefs in the run transcript)

### 3a. iOS is openable — and going *native* sidesteps the exact bug that closed it
A native WKWebView shell serves the app from **one stable origin**
(`tauri://localhost`), which **eliminates the cross-origin storage-partitioning bug
that broke the signer flow** — the `seed_pull.rs` class of workarounds simply
evaporates. It also escapes Safari's PWA cage (7-day storage eviction, ~50 MB caps)
and unlocks **native APNs push** (which is *also* what Apple's Guideline 4.2 review
wants to see). Sources: [WebKit storage policy](https://webkit.org/blog/14403/updates-to-storage-policy/),
[Tauri custom-origin](https://github.com/tauri-apps/tauri/discussions/4912),
[MagicBell PWA-iOS limits 2026](https://www.magicbell.com/blog/pwa-ios-limitations-safari-support-complete-guide).
**The real engineering cost of iOS is not the shell — it's collapsing the
multi-origin `*.localharness.xyz` identity model into in-app routing under one
native origin** (interacts with `tenant::current()`, the signer flow, on-chain owner
verification). Mostly upside (kills the hacks), but it's the work item to budget.

### 3b. Two UI-render families — and localharness is already in the second one
- **DOM family** (Leptos 0.8, Dioxus-web 0.7, Yew 0.22, Sycamore): render the real
  browser DOM from Rust via `wasm-bindgen`. Great *web* feel (a11y/IME/text/SEO free)
  but **web-only** — native mobile requires wrapping a WebView. Doesn't reduce JS to
  a floor; it *relocates* JS into the framework runtime.
- **Canvas/GPU family** (egui 0.35, Slint 1.15, Makepad 1.0): paint their own pixels
  via wgpu/WebGL. **One Rust binary genuinely runs on web-wasm + iOS + Android +
  desktop with near-zero JS** (a `<canvas>` + a wasm loader). This is the *same
  primitive localharness already built.* Sources: [egui](https://github.com/emilk/egui),
  [Slint 1.15](https://slint.dev/blog/slint-1.15-released), [Makepad](https://github.com/makepad/makepad),
  [2025 Rust GUI survey](https://www.boringcactus.com/2025/04/13/2025-survey-of-rust-gui-libraries.html).

### 3c. Tauri v2 (fast, but webview) vs pure-Rust-wgpu (min-JS, but younger)
- **Tauri v2** (stable, mobile GA, `2.9.6` Dec 2025): wraps your **existing web
  bundle** in a native shell → iOS + Android + desktop with **no UI rewrite** and
  ~zero hand-written Swift/Kotlin (thin plugins only, for push/keys). **But the UI
  stays a system WebView = JavaScript engine** — it's the max-Rust-*logic* answer,
  not the min-JS answer. It's the *packaging* ring, orthogonal to which UI you draw.
  Sources: [Tauri 2.0](https://v2.tauri.app/blog/tauri-20/),
  [App Store distribute](https://v2.tauri.app/distribute/app-store/).
- **Pure-Rust wgpu** (Makepad/Robrix, Bevy): one Rust binary renders itself on all
  targets, **near-zero JS**, the true continuation of localharness's framebuffer.
  Robrix ("pure Rust, 7 platforms, no platform-specific code") is the closest
  precedent; **Bevy (wgpu) apps have shipped to the iOS App Store** — existence proof
  the model clears review. Sources: [robrix.app](https://robrix.app/),
  [bevy-in-app](https://github.com/jinleili/bevy-in-app),
  [Makepad 1.0 HN](https://news.ycombinator.com/item?id=43971829).

### 3d. API-first in Rust is a clean, low-risk win — the language flip is now free
**Vercel shipped an *official Rust runtime* (public beta, Dec 2025)** on Fluid
compute with HTTP response streaming — each `api/*.rs` becomes a function, Axum is a
first-class template. So the proxy rewrite is a near file-for-file port that **keeps
the exact `cd proxy && vercel --prod` deploy story** while deleting all 13.7K LOC of
TS. Sources: [Vercel Rust runtime](https://vercel.com/docs/functions/runtimes/rust),
[changelog](https://vercel.com/changelog/rust-runtime-now-in-public-beta-for-vercel-functions).
- Architecture: a **`lh-server` library crate** holding one portable `axum::Router`
  with all handler logic, mounted by (a) thin `vercel_runtime` per-route shims and
  (b) a standalone `axum::serve` binary. Host becomes a swappable detail (Fly.io /
  Cloudflare Containers as pre-wired escape hatches — no handler rewrite).
- **`lh-api-types` crate** = every request/response as a `serde` DTO, imported
  *directly* by the Rust web app / CLI / future native cores; **ts-rs or Specta**
  emits `.ts` for whatever JS survives (**Specta also emits Swift** → feeds an
  iOS-native client), **utoipa** publishes an OpenAPI spec so any external agent can
  consume the API. Sources: [Specta](https://specta.dev/docs), [utoipa](https://github.com/juhaku/utoipa).
- **Biggest cleanup:** the sponsor relay is **already Rust in your crate**
  (`registry::sponsor_relay`); `proxy/api/sponsor.ts` is a *reimplementation* of code
  you own. Porting *deletes the duplication* and a class of drift bugs.
- Every subsystem has a mature crate: inference = `reqwest` + SSE passthrough (reuse
  `src/backends/*`); MCP = official `rmcp` SDK; push = `web-push` crate (VAPID);
  cron = same Vercel-cron-invokes-a-function model; telemetry = `reqwest`/`octocrab`.

### 3e. The irreducible JS floor (2026) is small but nonzero
What *cannot* be Rust today: (1) a **~10–30-line wasm bootstrap** shim
(`wasm-bindgen --target web/module`, no bundler); (2) the **Service Worker** script —
its top-level `push`/`fetch` registrations must run *synchronously* before async
wasm instantiate finishes, so the SW stays a thin JS shim that delegates into wasm
(push itself is trending to **declarative JSON, no JS**, on iOS 18.4+); (3) **one
`<canvas>` + one hidden `<input>`** (egui's `TextAgent` handles IME/mobile-keyboard
in-library — the reason a canvas app is "near-zero DOM," not "zero DOM"); (4) small
**worker loader stubs**. Everything else — rendering, input, OPFS, clipboard, WebRTC —
is reachable from Rust via `web-sys`. **Your ~2.9K shipped JS is *far* above this
floor; the bulk (`cartridge-worker.js`) is the deletable part.** Sources:
[egui web support](https://deepwiki.com/emilk/egui/3.3-web-platform-support),
[wasm-bindgen SW PoC](https://github.com/justinrubek/wasm-bindgen-service-worker),
[WebGPU in major browsers incl. iOS 26](https://web.dev/blog/webgpu-supported-major-browsers).

### 3f. Prior art + the honest ceiling
The pure-Rust/wgpu/one-codebase model **is real and shipping** (Robrix on 7
platforms; Bevy in the App Store). The honest ceilings, from the people who built
these: **accessibility becomes YOUR problem** on a canvas UI (AccessKit has *no web
adapter* yet — funding-gated → a pure-canvas app is a screen-reader black box today);
Makepad is judged **"not production-ready"** (weak IME/docs); Dioxus's webview mode
is production-viable but its *native* wgpu renderer (Blitz) is **experimental**;
Zed/GPUI is the cautionary tale of **hand-rolling per-OS renderers** (use wgpu's
abstraction, don't). And **no one could name a *pure-Rust* app currently live in the
iOS App Store** — Bevy games "have shipped" (unnamed), Robrix is blocked on *Apple
account approval, not tech*. Sources: [AccessKit](https://github.com/AccessKit/accesskit),
[boringcactus survey](https://www.boringcactus.com/2025/04/13/2025-survey-of-rust-gui-libraries.html),
[The Register — Zed Windows friction](https://www.theregister.com/2025/08/22/everything_is_different_on_windows/).

---

## 4. The recommendation — three phases, each independently shippable, no big-bang

The elegant part: **the render decision and the mobile decision are decoupled by
Tauri.** Tauri hosts *whatever* you draw, so you can ship iOS *now* on the current
UI and evolve the renderer toward pure-Rust *underneath it later* without redoing the
packaging. So we don't pick "webview vs framebuffer" — we *sequence* them.

### Phase 0 — API-first Rust (do this FIRST; independent of everything else)
Stand up the Ring-2 workspace: `lh-server` (portable Axum Router) + `lh-api-types`
(shared DTOs). Port `proxy/*.ts` → Rust on Vercel's Rust runtime, in the given
low-risk order (telemetry → push → cron → inference-SSE → MCP → **sponsor relay
last**, where it collapses into `registry::sponsor_relay`). **Payoff:** deletes 13.7K
LOC of TS, makes "API routes first-class" a structural fact, and makes the host
swappable — all with *zero* dependency on the UI/mobile question. **This is the
safest, highest-leverage move and should start on day one of the reset.**
- ⚠️ Spike **SSE streaming on `vercel_runtime` v2** before the inference port (docs
  confirm Fluid streaming but no explicit SSE example was found). Fallback: a
  long-running Axum host (Fly/CF-Containers) where streaming is trivial — and the
  `lh-server` Router makes that a config change, not a rewrite.

### Phase 1 — Tauri v2 shell → native iOS + Android + desktop, NOW
Wrap the **existing** wasm/maud app in Tauri v2. Ships to the friends' iPhones this
quarter with **no UI rewrite**. **Android first** (builds anywhere, permissive store,
lowest risk) to prove the shell, then **iOS** (needs a Mac builder + an APNs plugin +
a real offline state for 4.2 review). **The real work here is the identity refactor:
`*.localharness.xyz` multi-origin → in-app routing under one native origin** — which
*fixes* the very bug that closed iOS. New non-Rust code = a few thin Swift/Kotlin
plugin classes, quarantined in Ring 3. The core is untouched.

### Phase 2 — grow the framebuffer engine into the authoritative renderer (the JS kill)
*Under* the stable Tauri packaging, evolve the UI toward pure-Rust rendering:
1. **First, delete the parity tax:** move `compose.rs` logic *into wasm* so
   `cartridge-worker.js` drops from 1913 LOC to a ~50-line loader stub. Pure win, no
   UX change, kills the worst JS.
2. **Then (optional, bigger):** migrate the app UI itself to a pure-Rust
   wgpu/canvas renderer (egui-style, or the existing `html_fb` engine grown up), so
   "no imperative DOM" becomes *structurally true* and the same Rust binary draws
   identically on web-canvas + iOS + Android + desktop. WebGPU now ships on iOS 26;
   **keep the existing canvas2d/WebGL rasterizer as the mandatory fallback** for
   older devices.

**Why this order:** Phase 0 pays off immediately and de-risks the rest. Phase 1
unblocks iOS *fast* on proven tech (webview) and fixes the storage bug. Phase 2 is
where max-Rust/min-JS fully lands — but it's the immature, a11y-taxed part, so it
rides *last*, gradually, under a packaging layer that's already stable. **At no point
is there a big-bang rewrite; Ring 1 (the core) is reused throughout.**

---

## 5. The one real decision for you (render model) + my recommendation

Everything above is sequenced so you're never blocked — but there's one genuine fork
in **Phase 2**, and it turns on **who you're shipping to**:

- **Path A — framebuffer/wgpu (max-Rust, min-JS, your stated dream).** The endgame
  you want. Cost: **web accessibility is unsolved today** (AccessKit web adapter
  unfunded) — a pure-canvas app is invisible to VoiceOver/NVDA/TalkBack. Fine for a
  self-sovereign *agent IDE*; a liability for a *consumer marketing face*.
- **Path B — keep DOM for the public face, canvas for the app (hybrid).** localharness
  *already* splits "public face" vs "studio." Keep the marketing/landing/directory
  as light DOM (a11y + SEO where it matters), render the authenticated app as
  canvas (min-JS where it counts). This is the honest best-of-both and it fits the
  existing architecture.

**My recommendation: Path B (hybrid), and don't let it block Phases 0–1.** It's the
only option that reaches the JS floor *for the app* without making the marketing
surface a screen-reader black box, and it maps onto a split localharness already has.
Revisit "pure canvas everywhere" only if/when an AccessKit web adapter ships.

**On frameworks (Phase 2, when you get there):** the render-engine choice is
**egui vs Makepad vs growing your own `html_fb`.** egui = most batteries-included web
backend (IME/TextAgent solved), maps onto your canvas model with least friction.
Makepad = same GPU surface on web+iOS+Android natively (best "exact same app") but
younger/nightly-web/weaker IME. Your own `html_fb` = most control, most work. This is
a Phase-2 decision; **don't pick it now** — Phases 0–1 don't depend on it.

---

## 6. Honest risks & what could NOT be verified (read before committing)

- **No named pure-Rust app confirmed live in the iOS App Store.** Bevy games "have
  shipped" (unnamed); Robrix is blocked on Apple-account approval, not tech; no
  verified Tauri-v2 *iOS* App Store case study either. Treat App-Store viability as
  *demonstrated-possible, not proven*. **Highest-uncertainty item.** Check current
  Review Guidelines (4.2 webview-wrapper, 2.5.2 interpreted-code, 4.7 mini-apps)
  against a "Rust-renders-cartridges" model *before* building the iOS submission.
- **Vercel Rust runtime is public beta (Dec 2025)** — cold-start latency and API
  stability unproven; **SSE on `vercel_runtime` v2 is unverified** at code level.
  Spike both before the money-path relay and the inference proxy.
- **The iOS identity refactor (multi-origin → single-origin) is inferred, not sized.**
  It clearly interacts with `tenant::current()`, the signer flow, and owner
  verification. Net-positive (kills the partitioning hacks) but budget it as the main
  iOS cost, not the Tauri integration.
- **Pure-canvas accessibility is unsolved** (AccessKit web adapter funding-gated, no
  ETA) — the reason for the Path-B hybrid recommendation.
- **WebGPU on iOS 26 is real but fragile** (device-lost bugs on non-trivial passes;
  users below iOS 26 excluded). The WebGL/canvas2d fallback stays **mandatory** —
  localharness already has it.
- **Slint licensing** (GPLv3 / royalty-free / commercial tri-license) is a real
  governance question for a self-sovereign OSS platform — verify the mobile terms if
  Slint is ever considered.
- **Makepad web needs nightly Rust; its CJK/iOS IME quality is unverified** vs egui's
  (which was verified). Hands-on check required before choosing Makepad.

---

## 7. Open questions for the maintainer (surface, not blockers)

1. **Accessibility stance** — is a screen-reader-invisible canvas app acceptable for
   the *authenticated studio* (Path-B hybrid keeps the public face accessible)? This
   is the one decision that drives the Phase-2 render choice.
2. **iOS distribution** — real App Store listing (needs an Apple Developer account +
   the 4.2 review craft: native push + offline state), or sideload/TestFlight for
   "friends with iPhones" first? The latter unblocks the friends *without* the review
   gamble.
3. **Sequencing appetite** — do Phase 0 (Rust proxy) and Phase 1 (Tauri iOS) in
   parallel (independent), or serialize to keep one owner of main clean?
4. **Desktop** — is a desktop *app* actually wanted, or is "desktop web" enough?
   (Tauri gives desktop nearly free, but it's scope.)

---

## Appendix — framework matrix (2026-07-07 snapshot; re-verify)

| Framework | Ver | web-wasm | iOS native | Android native | Desktop | Render | Web JS |
|---|---|---|---|---|---|---|---|
| **egui/eframe** | 0.35 | ✅ WebGL2/WebGPU | ✅ maturing | ✅ maturing | ✅ | immediate-mode canvas | near-zero |
| **Slint** | 1.15 | ✅ WebGL | ✅ (1.12+) | ✅ | ✅ | GPU/CPU, DSL | near-zero |
| **Makepad** | 1.0 | ✅ (nightly) | ✅ | ✅ | ✅ | GPU shader/SDF | minimal |
| **Dioxus** | 0.7 | ✅ real DOM | ⚠️ WebView (Blitz emerging) | ⚠️ WebView | ✅ | DOM; WebView mobile | small |
| **Leptos** | 0.8 | ✅ real DOM/SSR | ❌ (via Tauri) | ❌ (via Tauri) | via Tauri | DOM reactive | minimal |
| **Tauri v2** | 2.9.6 | (shell) | ✅ shell | ✅ shell | ✅ shell | system WebView | hosts any UI |
| **Axum** (server) | — | — | — | — | — | — | Ring-2 default |

**Key sources** (all observed 2026-07-07): Dioxus 0.7, Tauri 2.0 + App-Store distribute,
Slint 1.15, egui repo, Makepad, Robrix/Project-Robius, Bevy-in-app, Vercel Rust runtime
+ changelog, Specta, WebKit WWDC25 (iOS 26 WebGPU), WebKit storage policy, AccessKit,
uniffi, the 2025 boringcactus Rust-GUI survey. URLs inline above; full cited briefs in
the `wf_7ef75b5f-ea8` run transcript.
