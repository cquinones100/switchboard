// The phone app's service worker: what makes it installable, and the Web
// Push it shows when a tab starts waiting on you. A push names the tab
// alone; tapping it opens the app on that tab.
self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

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
      icon: "/icon.svg",
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
