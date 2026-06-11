// localharness service worker — PWA installability + Web Push.
//
// DELIBERATELY NO CACHING. A previous `max-age=immutable` on /pkg/* made
// redeploys invisible until a hard reload (see CLAUDE.md gotcha); a caching
// service worker would reintroduce exactly that footgun one layer deeper.
// The fetch handler below is a NO-OP (it never calls respondWith), so every
// request takes the browser's normal network path + vercel.json's
// `max-age=0, must-revalidate` headers. It exists only because Chrome's
// installability heuristics want a fetch listener. Do NOT add caching here
// without revisiting that history.
//
// What this worker IS for:
//   * making <name>.localharness.xyz installable (manifest + SW + icons),
//   * `push`  — Web Push with the tab CLOSED, sent by the proxy's scheduler
//               worker (proxy/api/scheduler.ts) when a scheduled job runs;
//               payload is JSON {title, body},
//   * showing in-tab agent `notify(...)` tool notifications via
//               registration.showNotification (the page Notification
//               constructor throws on Android — the SW path is the one that
//               works everywhere).

self.addEventListener('install', () => {
  // Activate a new deploy's worker immediately — no caches to migrate.
  self.skipWaiting();
});

self.addEventListener('activate', (event) => {
  event.waitUntil(self.clients.claim());
});

// No-op pass-through: presence satisfies installability; absence of
// respondWith keeps the default network behavior (zero cache surface).
self.addEventListener('fetch', () => {});

self.addEventListener('push', (event) => {
  let payload = {};
  try {
    payload = event.data ? event.data.json() : {};
  } catch {
    // Non-JSON payload — surface it as the body rather than dropping it.
    payload = { body: event.data ? event.data.text() : '' };
  }
  const title = typeof payload.title === 'string' && payload.title ? payload.title : 'localharness';
  const body = typeof payload.body === 'string' ? payload.body : '';
  event.waitUntil(
    self.registration.showNotification(title, {
      body,
      icon: '/icons/icon-192.png',
      badge: '/icons/icon-192.png',
    }),
  );
});

self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  event.waitUntil(
    (async () => {
      const wins = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
      for (const client of wins) {
        if ('focus' in client) return client.focus();
      }
      return self.clients.openWindow('/');
    })(),
  );
});
