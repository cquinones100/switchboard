// The phone app's service worker: what makes it installable, the Web Push
// it shows when a tab starts waiting on you, and a copy of the app's own
// files, so it opens without the computer and says it is offline. A push
// names the tab alone; tapping it opens the app on that tab.
//
// iOS freezes the installed app's page in the background, so a message
// posted to it as a push is tapped can be lost, and a window opened for a
// push may start at "/" rather than the address asked for. So a tapped
// push's tab is kept (in a cache of its own, since the worker itself may be
// stopped before the page asks), for the page to ask for as it starts and
// each time it comes back, until the page says it took it.
//
// The copy holds the page's files alone, never what is under /api/ (your
// tabs, their turns, pairing), which is always asked of the computer.
const CACHE = "switchboard-phone-v1";
const TAPPED = "switchboard-phone-tapped";
const TAPPED_KEY = "/tapped";
const SHELL = [
  "/",
  "/app.js",
  "/markdown-it.min.js",
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
      for (const name of await caches.keys()) if (name !== CACHE && name !== TAPPED) await caches.delete(name);
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

// The tab of the push tapped last, with when it was tapped.
async function keepTapped(tab, at) {
  const cache = await caches.open(TAPPED);
  await cache.put(TAPPED_KEY, new Response(JSON.stringify({ tab, at })));
}

// The kept tab, read and forgotten one at a time.
let serial = Promise.resolve();
function inTurn(work) {
  const done = (async () => {
    await serial;
    return work();
  })();
  serial = done.catch(() => null);
  return done;
}

const readTapped = () =>
  inTurn(async () => {
    const kept = await (await caches.open(TAPPED)).match(TAPPED_KEY);
    try {
      return kept ? await kept.json() : null;
    } catch (_) {
      return null;
    }
  });

// Forgotten once the page says it took it, so it opens once, and an answer
// the page gave up waiting for leaves it for the next ask.
const forgetTapped = (at) =>
  inTurn(async () => {
    const cache = await caches.open(TAPPED);
    const kept = await cache.match(TAPPED_KEY);
    const tapped = kept ? await kept.json().catch(() => null) : null;
    if (!tapped || tapped.at === at) await cache.delete(TAPPED_KEY);
  });

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const tab = event.notification.data && event.notification.data.tab;
  const url = tab ? "/#" + encodeURIComponent(tab) : "/";
  event.waitUntil(
    (async () => {
      // A tap still opens the app when the tab can't be kept.
      const at = Date.now();
      if (tab) await keepTapped(tab, at).catch(() => {});
      const open = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
      for (const client of open) {
        if ("focus" in client) {
          // For a page that is running; one waking up asks for the tab.
          client.postMessage({ open: tab, at });
          const focused = await client.focus();
          if (tab && "navigate" in client) await client.navigate(url).catch(() => {});
          return focused;
        }
      }
      return self.clients.openWindow(url);
    })(),
  );
});

// The page asking for a tapped push's tab, answered on the port it sent, and
// saying it took it.
self.addEventListener("message", (event) => {
  const data = event.data || {};
  if (data.took) return event.waitUntil(forgetTapped(data.took).catch(() => {}));
  if (!data.tapped || !event.ports[0]) return;
  const port = event.ports[0];
  event.waitUntil(
    (async () => {
      try {
        port.postMessage((await readTapped()) || {});
      } catch (_) {
        port.postMessage({});
      }
    })(),
  );
});
