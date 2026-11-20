// The phone app (src/phone), boards 164 to 169: the drawer of Claude tabs,
// Needs you first; a tab's channel of recent turns with the prompt it waits
// on as a card of buttons; the composer; pairing. Everything shown comes from
// the computer's own answers and is put on the page as text, never as HTML.
"use strict";

const app = document.getElementById("app");
const state = {
  me: null,
  tabs: [],
  live: false,
  lastHeard: 0,
  open: null, // the pane's number of the tab open, or null for the drawer
  messages: [],
  answered: null, // what was answered last, for the folded card
  sent: "", // the prompt (and its ticks) an answer was sent for, whose buttons wait
  error: "",
};

function el(tag, attrs, ...kids) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v === undefined || v === null || v === false) continue;
    if (k === "class") node.className = v;
    else if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
    else node.setAttribute(k, v === true ? "" : v);
  }
  for (const kid of kids.flat()) {
    if (kid === null || kid === undefined || kid === false) continue;
    node.append(kid instanceof Node ? kid : document.createTextNode(String(kid)));
  }
  return node;
}

async function api(path, body) {
  const res = await fetch(path, {
    method: body === undefined ? "GET" : "POST",
    headers: body === undefined ? {} : { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
    credentials: "same-origin",
  });
  let data = null;
  try {
    data = await res.json();
  } catch (_) {
    data = null;
  }
  if (res.status === 403) {
    await start();
    throw new Error("refused");
  }
  if (!res.ok) throw new Error((data && data.error) || res.statusText);
  return data;
}

const number = (tab) => String(tab.pane || "").replace(/^%/, "");
const hue = (tab) => "n" + (((Number(tab.index) || 1) - 1) % 11 + 1);

function ago(ms) {
  if (!ms) return "";
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (s < 60) return "now";
  if (s < 3600) return Math.round(s / 60) + "m ago";
  if (s < 86400) return Math.round(s / 3600) + "h ago";
  return new Date(ms).toLocaleDateString();
}

function markOf(tab) {
  if (tab.state === "waiting") return ["●", "needs"];
  if (tab.state === "done") return ["✓", "passed"];
  if (tab.state === "working" || tab.state === "background") return ["⠹", "working"];
  return ["○", "idle"];
}

function doing(tab) {
  if (tab.state === "waiting") return tab.prompt ? "asks: " + tab.prompt.question : tab.wait || "waiting on you";
  if (tab.state === "done") return "finished " + ago(tab.since ? tab.since * 1000 : 0);
  if (tab.state === "working" || tab.state === "background") return tab.wait || (tab.turn && tab.turn.prompt) || "working";
  return "idle " + ago(tab.since ? tab.since * 1000 : 0).replace(" ago", "");
}

// ── Pairing (board 168) and refusals (board 169) ──

function refused(title, words) {
  app.replaceChildren(
    el("div", { class: "screen" }, el("div", { class: "center" }, el("h1", {}, title), el("p", {}, words))),
  );
}

async function pair() {
  let code;
  try {
    code = await api("/api/pair", {});
  } catch (e) {
    return refused("Couldn't start pairing", String(e.message || e));
  }
  const computer = state.me.computer;
  app.replaceChildren(
    el(
      "div",
      { class: "screen" },
      el("div", { class: "head" }, el("span", { class: "title" }, "Switchboard ", el("span", { class: "where" }, computer))),
      el(
        "div",
        { class: "center" },
        el("h1", {}, "Pair this phone"),
        el("p", {}, "Type this code in Switchboard on " + computer + ", or run ", el("code", {}, "switchboard phone pair " + code.code.replace(" ", "")), " there:"),
        el("div", { class: "code" }, code.code),
        el("p", {}, "⠹ Waiting for the code on " + computer),
        el("p", { class: "faint" }, "The code lasts 5 minutes."),
      ),
    ),
  );
  const look = async () => {
    let r;
    try {
      r = await api("/api/pair");
    } catch (_) {
      return setTimeout(look, 2000);
    }
    if (r.state === "paired") {
      await start();
      toast("Paired with " + computer);
    } else if (r.state === "refused") {
      refused("Pairing refused", "It was refused in Switchboard, or its code went untyped for 5 minutes.");
      app.querySelector(".center").append(el("button", { class: "big", onclick: () => pair() }, "Try again"));
    } else {
      setTimeout(look, 1500);
    }
  };
  setTimeout(look, 1500);
}

function toast(text) {
  const t = el("div", { class: "toast" }, el("span", { class: "ok" }, "✓"), text);
  document.body.append(t);
  setTimeout(() => t.remove(), 3000);
}

// ── The drawer (board 164) ──

function groups() {
  const needs = [], working = [], idle = [];
  for (const t of state.tabs) {
    if (t.needs) needs.push(t);
    else if (t.state === "working" || t.state === "background") working.push(t);
    else idle.push(t);
  }
  return [
    ["Needs you", "y", needs],
    ["Working", "b", working],
    ["Idle", "d", idle],
  ].filter(([, , tabs]) => tabs.length);
}

function header(...rest) {
  return el(
    "div",
    { class: "head" },
    ...rest,
    state.live ? el("span", { class: "chip g" }, "live") : el("span", { class: "chip r" }, "offline"),
  );
}

function drawer() {
  const list = el("div", { class: "drawer" + (state.live ? "" : " off") });
  const all = groups();
  if (!all.length) list.append(el("div", { class: "empty" }, "No tab is running Claude."));
  for (const [name, tone, tabs] of all) {
    const g = el("div", { class: "group" }, el("div", { class: "label " + tone }, el("span", {}, name), el("span", {}, tabs.length)));
    for (const t of tabs) {
      const [m, kind] = markOf(t);
      g.append(
        el(
          "button",
          { class: "row", onclick: () => openTab(number(t)) },
          el("span", { class: "mark " + kind }, m),
          el("span", { class: "name" }, el("span", { class: "num " + hue(t) }, t.index), t.name),
          el("span", { class: "sub" }, doing(t)),
        ),
      );
    }
    list.append(g);
  }
  app.replaceChildren(
    el(
      "div",
      { class: "screen" },
      header(el("span", { class: "title" }, "Switchboard ", el("span", { class: "where" }, state.me.computer))),
      state.live ? null : el("div", { class: "offline-line" }, "Last heard " + ago(state.lastHeard) + " · trying again"),
      list,
    ),
  );
}

// ── A tab's channel (board 165), its prompt card (166) and composer (167) ──

const current = () => state.tabs.find((t) => number(t) === state.open);

async function openTab(n) {
  state.open = n;
  state.messages = [];
  state.answered = null;
  state.error = "";
  history.replaceState(null, "", "#" + n);
  render();
  await loadTurns();
  const tab = current();
  if (tab && tab.state === "done") api("/api/tabs/" + n + "/seen", {}).catch(() => {});
}

async function loadTurns() {
  const n = state.open;
  if (!n) return;
  try {
    const r = await api("/api/tabs/" + n + "/turns");
    if (state.open !== n) return;
    state.messages = r.messages || [];
  } catch (_) {
    return;
  }
  render(true);
}

function message(m, i, all) {
  if (m.who === "tool") return el("div", { class: "msg" }, el("span"), el("div", { class: "tool" }, m.text));
  const prev = all[i - 1];
  const same = prev && prev.who === m.who && prev.who !== "tool";
  const who = m.who === "you" ? "You" : "Claude";
  const long = m.text.split("\n").length > 10 || m.text.length > 700;
  const body = el("div", { class: "body" + (long ? " folded" : "") }, m.text);
  const more = long
    ? el("button", { class: "more", onclick: (e) => { body.classList.remove("folded"); e.target.remove(); } }, "Show all")
    : null;
  return el(
    "div",
    { class: "msg" },
    same ? el("span") : el("span", { class: "avatar " + m.who }, who[0]),
    same ? null : el("div", {}, el("span", { class: "who" }, who), el("span", { class: "when" }, ago(m.at))),
    body,
    more,
  );
}

function card(tab) {
  const p = tab.prompt;
  if (!p) {
    if (state.answered) return el("div", { class: "card" }, el("div", { class: "answered" }, state.answered));
    return null;
  }
  const options = p.options.filter((o) => o.n !== p.free);
  const waiting = state.sent === promptKey(p);
  const c = el(
    "div",
    { class: "card" },
    el("div", { class: "top" }, el("span", {}, "● Waiting on you"), p.multi ? el("span", { class: "chip y" }, "choose several") : null),
    el("div", { class: "question" }, p.question || "Claude asks"),
    options.map((o, i) =>
      el(
        "button",
        {
          class: "option" + (i === 0 && !p.multi ? " lit" : "") + (o.checked ? " ticked" : ""),
          onclick: () => answer(String(o.n)),
          disabled: waiting,
        },
        el("span", { class: "n" }, o.n + "."),
        o.label,
      ),
    ),
    p.multi ? el("button", { class: "option lit", onclick: () => answer("submit"), disabled: waiting }, "Submit") : null,
    state.error ? el("div", { class: "error" }, state.error) : null,
  );
  return c;
}

// A prompt as it stands: its id and which options are ticked, so a tick
// that changes nothing else still lets the next tap through.
const promptKey = (p) => p.id + ":" + p.options.map((o) => (o.checked ? 1 : 0)).join("");

async function answer(reply) {
  state.error = "";
  const tab = current();
  const p = tab && tab.prompt;
  if (!p) return;
  // One answer for the prompt shown: its buttons wait for the next one.
  state.sent = promptKey(p);
  render();
  try {
    const r = await api("/api/tabs/" + state.open + "/answer", { reply, prompt: p.id });
    state.answered = r.said + " · from your phone";
  } catch (e) {
    state.error = String(e.message || e);
    state.sent = "";
  }
  render();
}

function composer(tab) {
  const p = tab.prompt;
  const free = p && p.free;
  const blocked = p && !free;
  const field = el("textarea", {
    rows: 1,
    placeholder: blocked ? "Answer the question above" : free ? "Your own answer" : "Message " + tab.index + " " + tab.name,
    disabled: blocked,
  });
  const send = el("button", { class: "send", "aria-label": "Send" }, "➤");
  field.addEventListener("input", () => {
    send.classList.toggle("lit", field.value.trim() !== "");
    field.style.height = "auto";
    field.style.height = field.scrollHeight + "px";
  });
  send.addEventListener("click", async () => {
    const text = field.value.trim();
    if (!text) return;
    send.disabled = true;
    try {
      if (free) await answer(text);
      else {
        await api("/api/tabs/" + state.open + "/message", { text });
        state.messages.push({ who: "you", text, at: Date.now() });
        render(true);
      }
      field.value = "";
    } catch (e) {
      state.error = String(e.message || e);
      render();
    } finally {
      send.disabled = false;
    }
  });
  return el("div", { class: "composer" }, el("span", { class: "plus", "aria-hidden": "true" }, "+"), field, send);
}

function channel(stick) {
  const tab = current();
  if (!tab) {
    state.open = null;
    return drawer();
  }
  const [m, kind] = markOf(tab);
  const old = app.querySelector(".channel");
  const atEnd = !old || old.scrollTop + old.clientHeight >= old.scrollHeight - 40;
  const kept = old ? old.scrollTop : 0;
  const draft = app.querySelector(".composer textarea");
  const typed = draft ? draft.value : "";
  const list = el(
    "div",
    { class: "channel" },
    state.messages.map(message),
    card(tab),
    tab.state === "working" || tab.state === "background" ? el("div", { class: "doing" }, "⠹ " + doing(tab)) : null,
  );
  const chip = tab.state === "waiting" ? ["waiting", "y"] : tab.state === "done" ? ["done", "g"] : tab.state === "idle" ? ["idle", ""] : ["working", "b"];
  app.replaceChildren(
    el(
      "div",
      { class: "screen" },
      el(
        "div",
        { class: "head" },
        el("button", { class: "menu", "aria-label": "Tabs", onclick: () => { state.open = null; history.replaceState(null, "", "#"); render(); } }, "☰"),
        el("span", { class: "mark " + kind }, m),
        el("span", { class: "title" }, el("span", { class: "num " + hue(tab) }, tab.index), " ", tab.name),
        el("span", { class: "chip " + chip[1] }, chip[0]),
      ),
      list,
      composer(tab),
    ),
  );
  const field = app.querySelector(".composer textarea");
  if (typed && field && !field.disabled) {
    field.value = typed;
    field.dispatchEvent(new Event("input"));
  }
  list.scrollTop = stick || atEnd ? list.scrollHeight : kept;
}

function render(stick) {
  if (!state.me || !state.me.paired) return;
  if (state.open) channel(stick);
  else drawer();
}

// ── The stream and Web Push ──

function listen() {
  const source = new EventSource("/api/events");
  source.onmessage = (event) => {
    let data;
    try {
      data = JSON.parse(event.data);
    } catch (_) {
      return;
    }
    const before = current();
    state.tabs = data.tabs || [];
    state.live = !!data.live;
    if (state.live) state.lastHeard = Date.now();
    const after = current();
    const turned = before && after && JSON.stringify(before.turn) !== JSON.stringify(after.turn);
    if (after && after.prompt) state.answered = null;
    render();
    if (turned) loadTurns();
  };
  source.onerror = () => {
    state.live = false;
    render();
  };
}

async function offerPush() {
  if (!("serviceWorker" in navigator) || !("PushManager" in window)) return;
  const registration = await navigator.serviceWorker.register("/sw.js");
  navigator.serviceWorker.addEventListener("message", (event) => {
    if (event.data && event.data.open) openTab(String(event.data.open));
  });
  if (Notification.permission === "denied") return;
  const subscribe = async () => {
    const { key } = await api("/api/push/key");
    const raw = atob(key.replace(/-/g, "+").replace(/_/g, "/") + "===".slice((key.length + 3) % 4));
    const bytes = Uint8Array.from(raw, (c) => c.charCodeAt(0));
    const sub =
      (await registration.pushManager.getSubscription()) ||
      (await registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: bytes }));
    await api("/api/push/subscribe", sub.toJSON());
  };
  if (Notification.permission === "granted") return subscribe().catch(() => {});
  // Asked only on a tap, as iOS requires for an installed app.
  const ask = el("button", { class: "big", onclick: async () => {
    ask.remove();
    if ((await Notification.requestPermission()) === "granted") subscribe().catch(() => {});
  } }, "Notify me when a tab waits on me");
  const drawerEnd = () => {
    const d = app.querySelector(".drawer");
    if (d && !d.contains(ask)) d.append(ask);
  };
  drawerEnd();
  new MutationObserver(drawerEnd).observe(app, { childList: true });
}

async function start() {
  try {
    const res = await fetch("/api/me", { credentials: "same-origin" });
    if (!res.ok) return refused("Not for this device", "This page answers one person's devices alone.");
    state.me = await res.json();
  } catch (_) {
    return refused("Can't reach Switchboard", "Check that this phone is on your tailnet.");
  }
  if (!state.me.paired) {
    if (state.me.revoked) {
      refused("This phone was unpaired", "Its pairing was taken away on the computer. Pair it again to carry on.");
      app.querySelector(".center").append(el("button", { class: "big", onclick: () => pair() }, "Pair again"));
      return;
    }
    return pair();
  }
  const r = await api("/api/tabs").catch(() => ({ tabs: [], live: false }));
  state.tabs = r.tabs || [];
  state.live = !!r.live;
  state.lastHeard = Date.now();
  const wanted = decodeURIComponent(location.hash.slice(1));
  if (wanted && state.tabs.some((t) => number(t) === wanted)) openTab(wanted);
  else render();
  listen();
  offerPush().catch(() => {});
}

start();
