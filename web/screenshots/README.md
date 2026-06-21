# README screenshots — canonical spec + capture runbook

The PNGs in this directory are the ONLY screenshots the project README links.
This file is the canonical list of WHAT to capture and the repeatable process for
HOW. No puppeteer, no npm deps — captures are driven by the Claude-in-Chrome MCP
against the **live prod site**.

## Hard rules (do not regress)

- **NO cartridge screenshots in the README.** The owner has removed
  `readyup`/`fractal`/`tetris`/`cartridge` shots from the README more than once;
  re-adding them is a hard no. The README "See it" section is app-UI shots only.
  (`scripts/render-screenshots.mjs`, the cartridge-PNG generator, was deleted for
  this reason — do not re-create it or its output.)
- **Uniform resolution.** Every shot in the README MUST be the SAME pixel
  dimensions. Capture all of them in ONE session at ONE viewport and crop every
  shot to the SAME fixed rectangle, so the README row never looks ragged.
- **Light mode + mobile.** Live site with `?theme=light&preview=mobile` (light
  palette + the 390px `#root` column; `src/app/mod.rs::apply_render_modes`).
- **Credits path only.** Never show the BYOK / api-key / local-Gemma UI.
- **Public data only.** Truncated wallet addresses, `$LH` balances, and on-chain
  personas are fine. **NEVER capture the "buy $LH" form** — Stripe Elements
  auto-fills the owner's saved payment method (email + bank/card). The on-ramp is
  prose in the README, not a screenshot.
- **Live prod, not localhost.** `https://localharness.xyz` reflects the deployed
  bundle, which may lead the local `web/pkg` build.

## Canonical shot list (app UI only)

| File | URL | Steps | README slot |
|---|---|---|---|
| `home.png` | `localharness.xyz` | land | **own** — identity + owned agents |
| `chat.png` | `<name>.localharness.xyz` | scroll transcript to a tool exchange | **chat** — streaming + inline tool cards |
| `studio.png` | `<name>.localharness.xyz` | gear → AGENT tab | **configure** — model, persona, x402 price |
| `account.png` | `<name>.localharness.xyz` | gear → ACCOUNT tab | **credits** — `$LH`, redeem, invite |

All four are owner-authenticated views. Keep the README table in sync with this list.

## How to capture (Claude-in-Chrome MCP → GIF → crop)

The desktop window is maximized and can't be shrunk to a phone size, so capture
the full viewport and crop the centered `#root` column. MCP inline screenshots
aren't written to disk, so route through `gif_creator` — `export {download:true}`
writes a real file to `~/Downloads`.

Per shot:
1. `gif_creator start_recording`
2. `navigate` to `https://<host>/?theme=light&preview=mobile` (+ any clicks to
   reach the state — open the gear, switch tabs, scroll the transcript via JS).
3. one or more `computer wait` actions — **`wait` is what records a settled
   frame** (a bare `screenshot`/`click` records 0 frames; `navigate` records a
   mid-load frame). The LAST frame is the loaded, light-themed view.
4. `gif_creator stop_recording`, then `export {download:true, options:{all
   overlays false, quality:1}}` → `~/Downloads/<name>.gif`.
5. Crop with `scripts/crop-mobile-shot.ps1`.

**To guarantee uniform resolution:** capture all four in one session (so the
viewport doesn't change between them — `innerWidth` can fluctuate 1568↔1920),
measure `#root` once, and crop EVERY shot to the SAME `-ViewportW/-RootLeft/
-ColumnW`. The admin modal (`studio`/`account`) is wider than the 390px column
and offset left, so pick a `-ColumnW` (with `-Margin`) wide enough to hold the
modal, and use that same width for the page shots too. Verify all four output
PNGs report identical dimensions before committing.

```powershell
pwsh scripts/crop-mobile-shot.ps1 -Gif "$HOME/Downloads/<name>.gif" `
  -Out web/screenshots/<name>.png -ViewportW <innerWidth> -RootLeft <left> -ColumnW <width> -Margin <m>
```

## Gotchas

- **Animated views time out `Page.captureScreenshot`** (the chat's rAF loop). The
  `gif_creator` `wait`-frame path still captures; a bare `computer screenshot` may
  throw "renderer frozen" — rely on the GIF.
- **Chrome taints `foreignObject`→canvas**, so in-page DOM-to-PNG (`toDataURL`) is
  impossible; the GIF route is the only disk path for DOM views.
