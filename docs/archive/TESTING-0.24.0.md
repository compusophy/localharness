# Human-tester checklist — localharness 0.24.0

Everything in this release that a machine **could** verify (builds, unit tests,
`forge build`, the proof-of-spec cartridge gate) is green. This checklist covers
what only a human + a real browser/device can confirm. Mark each `[ ]` → `[x]`.

Live site: <https://localharness.xyz>. Use a subdomain you own (e.g.
`yourname.localharness.xyz`) logged in as the owner unless noted. Hard-reload
(Ctrl/Cmd-Shift-R) once first so you're on the 0.24.0 bundle (footer shows
`0.24.0`).

---

## A. Shipped + live (test in the browser)

### A1 · Context tools (`clear_context` / `compact_context`) — feedback #7
- [ ] Send a few messages, then type **"clear the context"**. The agent calls
      `clear_context`; when the turn ends the transcript **vanishes instantly**
      (no page refresh).
- [ ] Reload the page → the chat is **still empty** (OPFS history was wiped).
- [ ] Build up a long conversation, then **"compact the context"**. Older turns
      **collapse on screen** into a `[compacted prior context]` summary; recent
      turns stay verbatim.
- [ ] Switch the model (Gemini ↔ Claude in the admin → model tab) and repeat
      "clear the context" — both backends honour it.

### A2 · Batch create / bulk release — feedback #3, #5/6
- [ ] Ask **"make me 3 subdomains: alpha-test, beta-test, gamma-test"**. The
      agent calls `batch_create_subdomains` → **one** tx; all three appear in
      `list_subdomains`. (Taken/invalid names are reported as `skipped`, not an
      error.)
- [ ] Ask **"release all my non-main subdomains"**. The agent calls
      `bulk_release_subdomains`, **lists the names it will burn first**, and asks
      you to type a **single** master confirmation phrase (NOT each name). After
      you type it → one tx burns them; your MAIN is untouched.
- [ ] ⚠️ **Gas note:** the batch gas (`1.5M/name` create, `250k/burn`) is
      *estimated*, not `cast`-measured. Try a small batch (2–3) first; if a big
      batch reverts, that's the under-budget case — tell me and I'll measure.

### A3 · Feedback → admin tab — feedback #14
- [ ] The **feedback button is gone from the header**.
- [ ] Open the admin modal → there's a **feedback tab**; submitting still posts
      on-chain (and the rate-limit/OPFS mirror still work).

### A4 · Mobile header on keyboard — feedback #9 (needs a real phone)
- [ ] On a phone, open a subdomain, tap the chat input so the keyboard opens.
      The **header stays visible** (doesn't scroll off the top). Test both
      Android/Chrome and iOS/Safari if you can.

### A5 · Credits cross-subdomain — feedback #8 (needs ≥2 subdomains, same owner)
- [ ] On your **main**, confirm you have `$LH` credits (Usage tab).
- [ ] From the **same device/owner**, open an **alt** subdomain you own. The
      Usage tab should now show the **same credits** (not 0), and a chat turn
      should be gated against that balance (not a fresh empty key).
- [ ] (Before this fix it showed 0 on the alt — that's the bug we fixed.)

### A6 · Agent-list loading speed — registry batch fix
- [ ] Load a view that lists your agents (`list_subdomains`, or the directory
      landing). With ~30 names in the registry it should resolve in **well under
      a second** (was ~5s — a sequential RPC per token).

### A7 · Cartridge audio — feedback #12a
- [ ] Run a cartridge that calls `host::audio` (e.g. a `frame` that does
      `audio::tone(440, 200, 0)` on a key/tap). You should **hear a tone**.
      `tone`/`tone_at`/`noise`/`stop`/`set_volume` are the surface.
- [ ] ⚠️ Audio uses two **deprecated** web-sys methods (`stop_with_when`,
      `set_onended`) — works today, flagged for a cleanup.

### A8 · Cartridge 3D — feedback #12b
- [ ] Run a cartridge using `display::fill_triangle(...)` + `display::draw_line(...)`.
      Triangles fill (flat) and lines draw on the 256×144 framebuffer.
- [ ] (The depth-buffered `fill_triangle_z` was deferred to a v2 packed ABI —
      it is NOT in this build, by design. Painter's-order sorting works for now.)

---

## B. Built but NOT in the shipping bundle (dev-only)

### B1 · Local Gemma backend (feature `local`)
The browser bundle ships **without** `local` (burn would 10× the page weight, and
in-tab inference is unvalidated). To try it:
- [ ] Build locally: `wasm-pack build . --target web --out-dir web/pkg --release
      --no-default-features --features browser-app,local`, serve `web/`.
- [ ] In the admin model tab, pick **Local (Gemma)** → download (~570MB to OPFS)
      → send a prompt. **This is the unvalidated-in-browser path** — report
      whether it (a) connects to WebGPU, (b) produces coherent text, (c) is
      tolerably fast. (Native validation already passed: it generated
      "The capital of France is Paris…".)

---

## C. Foundation only — NOT yet testable

### C1 · Agent teams + P2P sync (WebRTC, on-chain signaling)
- The on-chain layer (`TeamFacet`, `SignalingFacet`) is **forge-verified but NOT
  cut into the diamond**, and the browser Layer-5 orchestration + UI is unbuilt.
- **Nothing to test yet.** When the driver lands, the test will be: two of your
  devices `announce` under a topic, `offer`/`answer` over `SignalingFacet`, open
  a WebRTC channel, and sync one file. Flagged here so it's not mistaken for a
  shipped feature.

---

## Known residual issues (already logged, not blockers)
- 25 `clippy` lints in the **feature-gated** `browser-app`/`local` code (mostly
  the workflow-generated audio/3D) — not in the published default crate; a
  quality cleanup.
- Cartridge audio's two deprecated web-sys methods (A7).
- Batch-tx gas formulas are estimates, not `cast`-measured (A2).

Found a problem? Note the section (e.g. "A5") + what you saw; that maps straight
to the fix.
