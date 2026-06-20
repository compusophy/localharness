# README screenshots — canonical spec + capture runbook

The shots in this directory are the ONLY screenshots the project README links.
They are regenerated wholesale whenever the UI moves; this file is the canonical
list of WHAT to capture and the hardened, repeatable process for HOW. No
puppeteer, no npm deps (that harness was removed 2026-06-20) — captures are
driven by the Claude-in-Chrome MCP against the **live prod site** plus one
zero-dependency Node renderer for cartridges.

## Conventions (non-negotiable)

- **Light mode + mobile.** Every shot uses the live site with `?theme=light&preview=mobile`
  (light palette + the 390px `#root` column). Set in `src/app/mod.rs::apply_render_modes`.
- **Credits path only.** Never show the BYOK / api-key / local-Gemma UI.
- **Public data only.** Truncated wallet addresses, `$LH` balances, and on-chain
  personas are public and fine. **NEVER capture the "buy $LH" form** — Stripe
  Elements auto-fills the owner's saved payment method (email + bank/card). The
  on-ramp is described in prose in the README, not screenshotted.
- **Live prod, not localhost.** `https://localharness.xyz` reflects the deployed
  bundle, which may lead the local `web/pkg` build. Capture against prod.

## Canonical shot list

| File | URL | Auth | Steps | Shows | README slot |
|---|---|---|---|---|---|
| `home.png` | `localharness.xyz` | owner | land | identity + owned agents + "create" | **own** |
| `chat.png` | `<name>.localharness.xyz` | owner | scroll transcript to a tool exchange | streaming chat + inline tool cards (`list_subdomains`, `finish`) | **chat** |
| `studio.png` | `<name>.localharness.xyz` | owner | gear → AGENT tab | model toggle, persona, x402 price | **configure** |
| `account.png` | `<name>.localharness.xyz` | owner | gear → ACCOUNT tab | wallet, `$LH` balance, redeem, invite | **credits** |
| `readyup.png` `fractal.png` `tetris.png` | — (rendered) | — | `node scripts/render-screenshots.mjs` | cartridges = apps compiled in-browser | **ship** |

Add a row here before adding a shot; keep the README table in sync.

## How to capture

### Cartridges (deterministic, no browser)

```sh
node scripts/render-screenshots.mjs   # → readyup/fractal/tetris.png (phone-framed)
```

Renders the real published cartridge framebuffers through the same
`web/cartridge-worker.js` host the browser uses. No network, no flake.

### App DOM views (Claude-in-Chrome MCP → GIF → crop)

The desktop window is maximized, so it can't be shrunk to a phone size; instead
we capture the full viewport and crop the centered `#root` column. The MCP's
inline screenshots aren't written to disk, so we route through `gif_creator`,
whose `export {download:true}` writes a real file to `~/Downloads`.

Per shot:
1. `gif_creator start_recording`
2. `navigate` to `https://<host>/?theme=light&preview=mobile` (+ any clicks to
   reach the state — open the gear, switch tabs, scroll the transcript via JS)
3. one or more `computer wait` actions — **`wait` is what records a settled
   frame** (a bare `screenshot`/`click` records 0 frames; `navigate` records a
   mid-load frame). The LAST frame is the loaded, light-themed view.
4. `gif_creator stop_recording` then `export {download:true, options:{all
   overlays false, quality:1}}` → `~/Downloads/<name>.gif`
5. Measure the crop box in the page, then crop:
   ```powershell
   # measure first (rootLeft varies; the admin modal is wider than #root):
   #   document.getElementById('root').getBoundingClientRect()  -> left,width
   #   innerWidth   (fluctuates 1568↔1920 — the capture is always 1568px wide)
   pwsh scripts/crop-mobile-shot.ps1 -Gif "$HOME/Downloads/<name>.gif" `
     -Out web/screenshots/<name>.png -ViewportW <innerWidth> -RootLeft <left> -ColumnW <width> -Margin 6
   ```

### Gotchas (learned the hard way)

- **Animated views time out `Page.captureScreenshot`** (the chat's rAF loop).
  The `gif_creator` `wait`-frame path still captures; a bare `computer screenshot`
  may throw "renderer frozen" — retry once the view is static, or rely on the GIF.
- **`innerWidth` fluctuates 1568↔1920**; the GIF is always 1568px wide. Always
  pass the JS-measured `innerWidth` as `-ViewportW` so the crop scale is right.
- **The admin modal is wider than the 390px column** and offset left — measure
  the panel's actual box (e.g. `#invite-section`) and pass `-RootLeft/-ColumnW`.
- **Chrome taints `foreignObject`→canvas**, so in-page DOM-to-PNG (`toDataURL`)
  is impossible; the GIF route is the only disk path for DOM views.
