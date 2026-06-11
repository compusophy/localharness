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

try {
  const { default: init } = await import("./pkg/localharness.js");
  await init();
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
