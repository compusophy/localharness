// Bootstraps the wasm bundle. Kept as an external module (not inline) so
// the Content-Security-Policy can use `script-src 'self'` without
// `'unsafe-inline'`. All application code lives in the wasm bundle; this
// is the only JavaScript in the project.
//
// The import is DYNAMIC so a 404 of the shim itself (mid-deploy) rejects
// inside the try below — a static `import` fails module resolution before
// this file ever evaluates, leaving the static "loading…" shell up forever
// with every click silently vanishing. Dynamic same-origin import is still
// covered by `script-src 'self'` (no eval). Remaining gap: if boot.js
// itself fails to load, nothing here runs — only the static shell shows.
// PWA service worker (web/sw.js): installability + Web Push. Registered
// FIRST and fire-and-forget so a wasm boot failure can't block install, and
// a SW failure can't block boot. The worker does NO caching (no-op fetch
// handler) — see the header comment in sw.js before changing that.
try {
  if ("serviceWorker" in navigator) {
    navigator.serviceWorker.register("./sw.js").catch(() => {});
  }
} catch {
  /* non-secure context / exotic browser — installability is best-effort */
}

// Stash Chrome's install prompt so the APP can offer an [install] button
// (admin → account) instead of making the user dig through browser menus.
// The event only fires when the PWA is installable AND not yet installed;
// the wasm side reads window.__lhInstall via js_sys when the button is
// pressed and calls .prompt() on it (the click is the user gesture).
window.__lhInstall = null;
window.addEventListener("beforeinstallprompt", (e) => {
  e.preventDefault();
  window.__lhInstall = e;
});
window.addEventListener("appinstalled", () => {
  window.__lhInstall = null;
});

// Per-build cache-buster. Chrome's WebAssembly compiled-module code cache is
// keyed on the wasm URL, and serves a STALE compiled module for the unchanged
// /pkg/localharness_bg.wasm path even under `max-age=0, must-revalidate` — so a
// redeploy did not reach a returning visitor until a hard reload. Appending a
// per-build token (stamped by scripts/build-web.sh from the wasm content hash;
// "0" in dev) makes each deploy a NEW url = a guaranteed cache miss = fresh
// wasm. A query string never changes which static file Vercel serves, so it
// cannot 404. Bust the shim AND the wasm (the shim drops the query when it
// resolves the wasm relative to import.meta.url, so the wasm url is passed
// explicitly to init).
const LH_BUILD = "a90432f5ae08";
try {
  const mod = await import("./pkg/localharness.js?v=" + LH_BUILD);
  // Object form (not a bare string) — the bare-path arg is deprecated in this
  // wasm-bindgen and warns in the console; `{ module_or_path }` is the current API.
  await mod.default({ module_or_path: "./pkg/localharness_bg.wasm?v=" + LH_BUILD });
  // Web Push → in-app inbox relay: sw.js posts {type:'lh-push', title, body}
  // to open pages when a push arrives; hand it to the wasm side so the header
  // bell inbox + badge update live. Registered HERE (the project's one JS
  // file) — the app's no-per-element-closure rule stays intact in Rust.
  if ("serviceWorker" in navigator && typeof mod.push_arrived === "function") {
    navigator.serviceWorker.addEventListener("message", (e) => {
      const d = e.data;
      if (d && d.type === "lh-push") {
        try {
          mod.push_arrived(String(d.title || ""), String(d.body || ""));
        } catch {}
      }
    });
  }
} catch (e) {
  // Boot failed (wasm/shim fetch 404 mid-deploy, instantiation failure,
  // network drop). Swap #root to a minimal monochrome failure line —
  // textContent only, never innerHTML — and stamp data-lh-error so smoke
  // tooling can tell boot-FAILED from still-booting (data-lh-ready is the
  // success marker and never appears on this path).
  console.error("localharness boot failed:", e);
  document.documentElement.dataset.lhError = "1";
  const root = document.getElementById("root");
  if (root) root.textContent = "failed to load — reload to retry";
}
