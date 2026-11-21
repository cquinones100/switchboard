// The phone app (src/phone), boards 164 to 169, 173 and 174: the drawer of Claude
// tabs, Needs you first, with its search; a tab's channel of recent turns
// with the prompt it waits on as a card of buttons; the composer; pairing.
// Everything shown comes from the computer's own answers and is put on the
// page as text, never as HTML. Images from the photo library (board 174)
// wait over the composer and go with the message, kept on the computer first.
"use strict";

const app = document.getElementById("app");
const state = {
  me: null,
  tabs: [],
  live: false,
  lastHeard: 0,
  unreached: false, // the computer couldn't be reached as the app opened
  reconnectingUntil: 0, // while the stream is opened again after the page slept, "Reconnecting" rather than "offline"
  open: null, // the pane's number of the tab open, or null for the drawer
  messages: [],
  answered: null, // what was answered last, for the folded card
  search: "", // what is typed in the drawer's search, kept while a tab is open
  sent: null, // { pane, key }: the prompt (and its ticks) an answer was sent for, whose buttons wait
  chosen: [], // { file, url }: the images chosen from the photo library, waiting over the composer
  full: null, // the name of the image shown whole, or null
  error: "",
  explained: "", // promptKey's id and the option's number whose explainer shows, kept across redraws
  unfolded: new Set(), // the open tab's messages opened with Show all (`messageKey`), kept across redraws
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

// ── The drawer (board 164) and its search (board 173) ──

// The words typed in the search, case aside.
const wordsOf = (q) => q.toLowerCase().split(/\s+/).filter(Boolean);
const holds = (text, words) => words.every((w) => String(text || "").toLowerCase().includes(w));
// Whether a tab's number, name, repository, branch or what it is doing hold
// every word typed, as the sidebar's / matches.
const matches = (tab, words) => holds([tab.index, tab.name, tab.repo, tab.branch, doing(tab)].join(" "), words);

function groups(words) {
  const needs = [], working = [], idle = [];
  for (const t of state.tabs) {
    if (!matches(t, words || [])) continue;
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

const reconnecting = () => !state.live && Date.now() < state.reconnectingUntil;

function header(...rest) {
  return el(
    "div",
    { class: "head" },
    ...rest,
    state.live
      ? el("span", { class: "chip g" }, "live")
      : reconnecting()
        ? el("span", { class: "chip y" }, "reconnecting")
        : el("span", { class: "chip r" }, "offline"),
  );
}

// Text with every place a word typed appears lit.
function lit(text, words) {
  const on = [...text].map(() => false);
  const lower = text.toLowerCase();
  for (const w of words) {
    for (let i = lower.indexOf(w); i >= 0; i = lower.indexOf(w, i + 1)) {
      for (let j = i; j < i + w.length; j++) on[j] = true;
    }
  }
  const parts = [];
  [...text].forEach((c, i) => {
    const last = parts[parts.length - 1];
    if (last && last[0] === on[i]) last[1] += c;
    else parts.push([on[i], c]);
  });
  return parts.map(([hit, part]) => (hit ? el("span", { class: "hit" }, part) : part));
}

// A tab matched by its repository or branch alone says which, under what
// it is doing.
function where(tab, words) {
  if (!words.length || holds([tab.index, tab.name, doing(tab)].join(" "), words)) return null;
  const by = [["repository ", tab.repo], ["branch ", tab.branch]].filter(([, v]) => v && words.some((w) => v.toLowerCase().includes(w)));
  return el("span", { class: "sub where-line" }, by.flatMap(([what, v], i) => [i ? " · " : "", el("span", { class: "faint" }, what), ...lit(v, words)]));
}

// The search box, made once and kept across the drawer's redraws, so what
// is typed and the keyboard stay while the server's updates come in.
let searchBox = null;
function search() {
  if (searchBox) return searchBox;
  const field = el("input", { type: "search", placeholder: "Search tabs", autocomplete: "off", autocapitalize: "off", spellcheck: "false", "aria-label": "Search tabs" });
  const clear = el("button", { class: "clear", "aria-label": "Clear the search" }, "×");
  const sync = () => clear.classList.toggle("shown", state.search !== "");
  field.addEventListener("input", () => {
    state.search = field.value;
    sync();
    drawer();
  });
  clear.addEventListener("click", () => clearSearch());
  searchBox = el("div", { class: "search" }, el("span", { class: "prompt" }, "›"), field, clear);
  searchBox.show = () => {
    if (field.value !== state.search) field.value = state.search;
    sync();
  };
  searchBox.focusField = () => field.focus();
  return searchBox;
}

function clearSearch() {
  state.search = "";
  drawer();
  search().focusField();
}

function drawerList() {
  const words = wordsOf(state.search);
  const list = el("div", { class: "drawer" + (state.live ? "" : " off") });
  const all = groups(words);
  if (!state.tabs.length) list.append(el("div", { class: "empty" }, "No tab is running Claude."));
  else if (!all.length) {
    list.append(
      el("div", { class: "empty" }, "No tab matches ", el("b", {}, state.search.trim())),
      el("button", { class: "big again", onclick: () => clearSearch() }, "Clear the search"),
    );
  }
  for (const [name, tone, tabs] of all) {
    const g = el("div", { class: "group" }, el("div", { class: "label " + tone }, el("span", {}, name), el("span", {}, tabs.length)));
    for (const t of tabs) {
      const [m, kind] = markOf(t);
      g.append(
        el(
          "button",
          { class: "row", onclick: () => openTab(number(t)) },
          el("span", { class: "mark " + kind }, m),
          el("span", { class: "name" }, el("span", { class: "num " + hue(t) }, lit(String(t.index), words)), lit(t.name || "", words)),
          el("span", { class: "sub" }, lit(doing(t), words)),
          where(t, words),
        ),
      );
    }
    list.append(g);
  }
  return list;
}

function drawer() {
  const box = search();
  box.show();
  const head = header(el("span", { class: "title" }, "Switchboard ", el("span", { class: "where" }, state.me.computer)));
  const offline = el("div", { class: "offline-slot" }, state.live ? null : el("div", { class: "offline-line" }, reconnecting() ? "Reconnecting…" : "Last heard " + ago(state.lastHeard) + " · trying again"));
  const list = drawerList();
  const screen = app.querySelector(".screen.drawer-screen");
  // The drawer already showing: everything but the search box drawn again,
  // since taking a focused field out of the page closes the keyboard.
  if (screen && screen.contains(box)) {
    const old = screen.querySelector(".drawer");
    const kept = old ? old.scrollTop : 0;
    screen.querySelector(".head").replaceWith(head);
    screen.querySelector(".offline-slot").replaceWith(offline);
    if (old) old.replaceWith(list);
    else screen.append(list);
    list.scrollTop = kept;
    return;
  }
  app.replaceChildren(el("div", { class: "screen drawer-screen" }, head, box, offline, list));
}

// ── A tab's channel (board 165), its prompt card (166) and composer (167) ──

const current = () => state.tabs.find((t) => number(t) === state.open);

function drop(i) {
  URL.revokeObjectURL(state.chosen[i].url);
  state.chosen.splice(i, 1);
}

async function openTab(n) {
  state.open = n;
  state.messages = [];
  state.answered = null;
  state.error = "";
  state.unfolded = new Set();
  while (state.chosen.length) drop(0);
  state.full = null;
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

const messageKey = (m) => m.who + ":" + m.at + ":" + m.text.length;

function message(m, i, all) {
  if (m.who === "tool") return el("div", { class: "msg" }, el("span"), el("div", { class: "tool" }, m.text));
  const prev = all[i - 1];
  const same = prev && prev.who === m.who && prev.who !== "tool";
  const who = m.who === "you" ? "You" : "Claude";
  const when = el("span", { class: "when" }, ago(m.at));
  when.at = m.at;
  const key = messageKey(m);
  const folded = (m.text.split("\n").length > 10 || m.text.length > 700) && !state.unfolded.has(key);
  const body = m.text ? el("div", { class: "body" + (folded ? " folded" : "") }, m.text) : null;
  const images = (m.images || []).length
    ? el(
        "div",
        { class: "shots" },
        m.images.map((name) =>
          el(
            "button",
            { class: "shot", "aria-label": "Show the image", onclick: () => { state.full = name; render(); } },
            el("img", { src: imageUrl(name), alt: "An image you sent", loading: "lazy" }),
          ),
        ),
      )
    : null;
  const more = folded
    ? el("button", {
        class: "more",
        onclick: (e) => {
          state.unfolded.add(key);
          body.classList.remove("folded");
          e.target.remove();
        },
      }, "Show all")
    : null;
  return el(
    "div",
    { class: "msg" },
    same ? el("span") : el("span", { class: "avatar " + m.who }, who[0]),
    same ? null : el("div", {}, el("span", { class: "who" }, who), when),
    body,
    more,
    images,
  );
}

const imageUrl = (name) => "/api/images/" + encodeURIComponent(name);

// An image shown whole, over the channel, until its × or a tap.
function full() {
  if (!state.full) return null;
  const close = () => { state.full = null; render(); };
  return el(
    "div",
    { class: "full", onclick: close },
    el("button", { class: "close", "aria-label": "Close" }, "×"),
    el("img", { src: imageUrl(state.full), alt: "An image you sent" }),
  );
}

// One chosen image kept on the computer for the tab; its name.
async function upload(n, file) {
  const res = await fetch("/api/tabs/" + n + "/image", {
    method: "POST",
    headers: { "Content-Type": file.type || "application/octet-stream" },
    body: file,
    credentials: "same-origin",
  });
  if (res.status === 403) {
    await start();
    throw new Error("refused");
  }
  if (res.status === 413) throw new Error("an image can't be over 20 MB");
  let data = null;
  try {
    data = await res.json();
  } catch (_) {
    data = null;
  }
  if (!res.ok) throw new Error((data && data.error) || res.statusText);
  return data.id;
}

function card(tab) {
  const p = tab.prompt;
  if (!p) {
    if (state.answered) return el("div", { class: "card" }, el("div", { class: "answered" }, state.answered));
    return null;
  }
  const options = p.options.filter((o) => o.n !== p.free);
  const waiting = !!state.sent && state.sent.pane === tab.pane && state.sent.key === promptKey(p);
  const c = el(
    "div",
    { class: "card" },
    el("div", { class: "top" }, el("span", {}, "● Waiting on you"), p.multi ? el("span", { class: "chip y" }, "choose several") : null),
    el("div", { class: "question" }, p.question || "Claude asks"),
    options.map((o, i) => choice(p, o, i, waiting)),
    p.multi ? el("button", { class: "option lit", onclick: () => answer("submit"), disabled: waiting }, "Submit") : null,
    state.error ? el("div", { class: "error" }, state.error) : null,
  );
  return c;
}

// One choice: its button, which answers, and, when the question explains
// it, an info button beside it that shows the explainer under it and hides
// it again, answering nothing.
function choice(p, o, i, waiting) {
  const button = el(
    "button",
    {
      class: "option" + (i === 0 && !p.multi ? " lit" : "") + (o.checked ? " ticked" : ""),
      onclick: () => answer(String(o.n)),
      disabled: waiting,
    },
    el("span", { class: "n" }, o.n + "."),
    o.label,
  );
  if (!o.detail) return button;
  const key = p.id + ":" + o.n;
  const open = state.explained === key;
  const info = el(
    "button",
    {
      class: "info" + (open ? " lit" : ""),
      "aria-label": open ? "Hide what this choice means" : "Show what this choice means",
      "aria-expanded": String(open),
      onclick: () => {
        state.explained = open ? "" : key;
        render();
      },
    },
    "i",
  );
  return el(
    "div",
    { class: "choice" },
    el("div", { class: "row" }, button, info),
    open ? el("div", { class: "explainer" }, o.detail) : null,
  );
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
  state.sent = { pane: tab.pane, key: promptKey(p) };
  render();
  try {
    const r = await api("/api/tabs/" + state.open + "/answer", { reply, prompt: p.id });
    state.answered = r.said + " · from your phone";
  } catch (e) {
    state.error = String(e.message || e);
    state.sent = null;
  }
  render();
}

// The composer, made as a tab opens and kept until another does, so its
// field keeps its focus, what is typed and the cursor through the redraws;
// `sync` follows the prompt the tab waits on, the images chosen and the error.
function composer() {
  const field = el("textarea", { rows: 1 });
  const send = el("button", { class: "send", "aria-label": "Send" }, "➤");
  // The photo library alone: images, several at once, no camera.
  const picker = el("input", { type: "file", accept: "image/png,image/jpeg,image/gif,image/webp", multiple: true, hidden: true });
  picker.addEventListener("change", () => {
    for (const file of picker.files) state.chosen.push({ file, url: URL.createObjectURL(file) });
    picker.value = "";
    render();
  });
  const plus = el("button", { class: "plus", "aria-label": "Add images from your photos", onclick: () => picker.click() }, "+");
  const free = () => {
    const tab = current();
    return !!(tab && tab.prompt && tab.prompt.free);
  };
  const ready = () => field.value.trim() !== "" || (!free() && state.chosen.length > 0);
  const resize = () => {
    send.classList.toggle("lit", ready());
    field.style.height = "auto";
    field.style.height = field.scrollHeight + "px";
  };
  field.addEventListener("input", resize);
  send.addEventListener("click", async () => {
    const text = field.value.trim();
    if (!ready()) return;
    const answering = free();
    send.disabled = true;
    state.error = "";
    // Taken out of the composer as it is sent, so a second tap can't send
    // it again; given back if it fails.
    const chosen = answering ? [] : state.chosen;
    field.value = "";
    resize();
    if (!answering) state.chosen = [];
    const n = state.open;
    try {
      if (answering) await answer(text);
      else {
        const images = [];
        for (const c of chosen) images.push(await upload(n, c.file));
        await api("/api/tabs/" + n + "/message", { text, images });
        for (const c of chosen) URL.revokeObjectURL(c.url);
        state.messages.push({ who: "you", text, images, at: Date.now() });
        render(true);
      }
    } catch (e) {
      state.error = String(e.message || e);
      field.value = text;
      resize();
      if (state.open === n) state.chosen = chosen.concat(state.chosen);
      render();
    } finally {
      send.disabled = false;
    }
  });
  let tray = el("div");
  let error = el("div");
  const box = el("div", { class: "composing" }, tray, error, el("div", { class: "composer" }, plus, picker, field, send));
  box.sync = (tab) => {
    const p = tab.prompt;
    const free = p && p.free;
    field.placeholder = p && !free ? "Answer the question above" : free ? "Your own answer" : "Message " + tab.index + " " + tab.name;
    field.disabled = !!(p && !free);
    plus.disabled = !!p;
    const newTray = state.chosen.length
      ? el(
          "div",
          { class: "tray" },
          state.chosen.map((c, i) =>
            el(
              "div",
              { class: "shot" },
              el("img", { src: c.url, alt: "An image to send" }),
              el("button", { class: "drop", "aria-label": "Drop this image", onclick: () => { drop(i); render(); } }, "×"),
            ),
          ),
        )
      : el("div");
    tray.replaceWith(newTray);
    tray = newTray;
    const newError = state.error && !p ? el("div", { class: "error" }, state.error) : el("div");
    error.replaceWith(newError);
    error = newError;
    send.classList.toggle("lit", ready());
  };
  return box;
}

function channelHead(tab) {
  const [m, kind] = markOf(tab);
  const chip = tab.state === "waiting" ? ["waiting", "y"] : tab.state === "done" ? ["done", "g"] : tab.state === "idle" ? ["idle", ""] : ["working", "b"];
  return el(
    "div",
    { class: "head" },
    el("button", { class: "menu", "aria-label": "Tabs", onclick: () => { state.open = null; history.replaceState(null, "", "#"); render(); } }, "☰"),
    el("span", { class: "mark " + kind }, m),
    el("span", { class: "title" }, el("span", { class: "num " + hue(tab) }, tab.index), " ", tab.name),
    el("span", { class: "chip " + chip[1] }, chip[0]),
  );
}

// The open tab's screen, kept while it is open: a redraw puts in a new head,
// prompt card, "doing" line and image shown whole, the messages only when
// the turns changed, and leaves the composer where it is, since taking a
// focused field out of the page closes the keyboard.
let shown = null; // { open, screen, head, list, messages, drawn, count, now, composer, full }

function channel(stick) {
  const tab = current();
  if (!tab) {
    state.open = null;
    return drawer();
  }
  const kept = shown && shown.open === state.open && shown.screen.parentNode === app ? shown : null;
  const atEnd = !kept || kept.list.scrollTop + kept.list.clientHeight >= kept.list.scrollHeight - 40;
  const top = kept ? kept.list.scrollTop : 0;
  if (!kept) {
    const messages = el("div", { class: "messages" });
    const now = el("div", { class: "now" });
    const list = el("div", { class: "channel" }, messages, now);
    const head = el("div", { class: "head" });
    const box = composer();
    const whole = el("div");
    shown = { open: state.open, screen: el("div", { class: "screen" }, head, list, box, whole), head, list, messages, drawn: null, count: 0, now, composer: box, full: whole };
    app.replaceChildren(shown.screen);
  }
  const head = channelHead(tab);
  shown.head.replaceWith(head);
  shown.head = head;
  if (shown.drawn !== state.messages || shown.count !== state.messages.length) {
    shown.messages.replaceChildren(...state.messages.map(message));
    shown.drawn = state.messages;
    shown.count = state.messages.length;
  } else {
    // Kept, so their times ("now", "3m ago") are worked out again in place.
    for (const when of shown.messages.querySelectorAll(".when")) when.textContent = ago(when.at);
  }
  const working = tab.state === "working" || tab.state === "background";
  shown.now.replaceChildren(...[card(tab), working ? el("div", { class: "doing" }, "⠹ " + doing(tab)) : null].filter(Boolean));
  shown.composer.sync(tab);
  const whole = full() || el("div");
  shown.full.replaceWith(whole);
  shown.full = whole;
  shown.list.scrollTop = stick || atEnd ? shown.list.scrollHeight : top;
}

function render(stick) {
  if (!state.me || !state.me.paired) return;
  if (state.open) channel(stick);
  else drawer();
}

// ── The stream and Web Push ──

let source = null;

function listen() {
  if (source) source.close();
  source = new EventSource("/api/events");
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
    // The prompt answered gone, or another in its place: its buttons are live again.
    if (state.sent) {
      const answered = state.tabs.find((t) => t.pane === state.sent.pane);
      if (!answered || !answered.prompt || promptKey(answered.prompt) !== state.sent.key) state.sent = null;
    }
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

// iOS doesn't reopen the stream of a page that slept, so coming back to the
// app (or back online) closes it and opens it again at once, and reads the
// tabs again, saying "Reconnecting" meanwhile rather than "offline".
const RECONNECTING_MS = 5000;

async function reconnect() {
  state.reconnectingUntil = Date.now() + RECONNECTING_MS;
  render();
  setTimeout(render, RECONNECTING_MS + 100);
  listen();
  try {
    const r = await api("/api/tabs");
    state.tabs = r.tabs || [];
    state.live = !!r.live;
    if (state.live) state.lastHeard = Date.now();
    render();
    if (state.open) loadTurns();
  } catch (_) {
    // The stream says when the computer is back.
  }
}

// Back to the app: reconnected when paired, else started again when it
// couldn't reach the computer as it opened.
function cameBack() {
  if (document.visibilityState === "hidden") return;
  if (state.me && state.me.paired) reconnect();
  else if (state.unreached) reach();
}

document.addEventListener("visibilitychange", cameBack);
window.addEventListener("pageshow", (event) => {
  // A page shown the first time is the one start() is opening.
  if (event.persisted) cameBack();
});
window.addEventListener("online", cameBack);

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
  new MutationObserver(drawerEnd).observe(app, { childList: true, subtree: true });
}

// While the computer is out of reach, one try at a time and one retry
// waiting: the app coming back meanwhile tries now rather than adding a loop.
let reaching = false;
let retry = null;

async function reach() {
  if (reaching) return;
  reaching = true;
  clearTimeout(retry);
  try {
    await start();
  } finally {
    reaching = false;
  }
}

async function start() {
  try {
    const res = await fetch("/api/me", { credentials: "same-origin" });
    if (!res.ok) return refused("Not for this device", "This page answers one person's devices alone.");
    state.me = await res.json();
  } catch (_) {
    // Opened from the service worker's copy with the computer out of reach:
    // tried again as the app comes back, goes online, and every few seconds.
    state.unreached = true;
    refused("Offline", "Can't reach Switchboard: check that this phone and the computer are on your tailnet. Trying again…");
    clearTimeout(retry);
    retry = setTimeout(() => state.unreached && reach(), RECONNECTING_MS);
    return;
  }
  state.unreached = false;
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

// Safari pinches to zoom with these whatever the viewport says.
for (const name of ["gesturestart", "gesturechange"]) {
  document.addEventListener(name, (event) => event.preventDefault(), { passive: false });
}

start();
