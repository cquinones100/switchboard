// The phone app's service worker: what makes it installable, the Web Push
// it shows when a tab starts waiting on you, and a copy of the app's own
// files, so it opens without the computer and says it is offline. A push
// names the tab alone; tapping it opens the app on that tab.
//
// The copy holds the page's files alone, never what is under /api/ (your
// tabs, their turns, pairing), which is always asked of the computer.
const CACHE = "switchboard-phone-v1";
const SHELL = [
  "/",
  "/app.js",
  "/app.css",
  "/manifest.webmanifest",
  "/icon.svg",
  "/icon-180.png",
  "/icon-192.png",
  "/icon-512.png",
];

self.addEventListener("install", (event) => {
  event.waitUntil(
    (async () => {
      const cache = await caches.open(CACHE);
      // One file that can't be fetched now is fetched as the app asks for it.
      await Promise.all(SHELL.map((path) => cache.add(path).catch(() => {})));
      await self.skipWaiting();
    })(),
  );
});

self.addEventListener("activate", (event) =>
  event.waitUntil(
    (async () => {
      for (const name of await caches.keys()) if (name !== CACHE) await caches.delete(name);
      await self.clients.claim();
    })(),
  ),
);

// Whether a request is one of the app's own files to keep a copy of.
const isShell = (url) => url.origin === self.location.origin && SHELL.includes(url.pathname);

// The computer first, so a new build's files arrive as soon as it is
// reached; the copy when it can't be.
self.addEventListener("fetch", (event) => {
  const request = event.request;
  const url = new URL(request.url);
  if (request.method !== "GET" || url.pathname.startsWith("/api/") || !isShell(url)) return;
  event.respondWith(
    (async () => {
      const cache = await caches.open(CACHE);
      try {
        const fresh = await fetch(request);
        if (fresh.ok) await cache.put(url.pathname, fresh.clone());
        return fresh;
      } catch (error) {
        const kept = await cache.match(url.pathname);
        if (kept) return kept;
        throw error;
      }
    })(),
  );
});

self.addEventListener("push", (event) => {
  let data = {};
  try {
    data = event.data ? event.data.json() : {};
  } catch (_) {
    data = {};
  }
  const title = data.title || "A tab";
  event.waitUntil(
    self.registration.showNotification(title, {
      body: "Waiting on you",
      tag: "tab-" + (data.tab || ""),
      data: { tab: data.tab || "" },
      icon: "/icon-192.png",
    }),
  );
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const tab = event.notification.data && event.notification.data.tab;
  const url = tab ? "/#" + encodeURIComponent(tab) : "/";
  event.waitUntil(
    (async () => {
      const open = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
      for (const client of open) {
        if ("focus" in client) {
          client.postMessage({ open: tab });
          return client.focus();
        }
      }
      return self.clients.openWindow(url);
    })(),
  );
});
