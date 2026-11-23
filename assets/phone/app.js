// The phone app (src/phone), boards 164 to 169 and 173 to 176: the drawer of Claude
// tabs, Needs you first, with its search; a tab's channel of recent turns
// with the prompt it waits on as a card of buttons; the composer; pairing.
// Everything shown comes from the computer's own answers and is put on the
// page as text, never as HTML: Claude's replies are Markdown, parsed by the
// vendored markdown-it (raw HTML off) and built into elements from its
// tokens (`markdown`), so nothing in a reply is read as HTML. Images from the photo library (board 174)
// wait over the composer and go with the message, kept on the computer first.
"use strict";

// The commit of the build serving the page, put in as it is served
// (src/phone/build.rs).
const BUILD = "dev";

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
  turnsLoaded: false, // the open tab's turns were read, so a message drawn since arrived while it was open
  sent: null, // { pane, key }: the prompt (and its ticks) an answer was sent for, whose buttons wait
  chosen: [], // { file, url }: the images chosen from the photo library, waiting over the composer
  full: null, // the name of the image shown whole, or null
  whole: false, // the open tab's name shown whole at the top, until another tab or the drawer shows
  error: "",
  explained: "", // promptKey's id and the option's number whose explainer shows, kept across redraws
  turnKeys: [], // the keys of the open tab's turns last drawn, oldest first (`turns`)
  unfolded: new Set(), // the open tab's messages opened with Show all (`messageKey`), and turns' steps opened (`turns`), kept across redraws
  close: true, // whether Close tab is offered (Settings' phone_close_tabs), from the stream
  menu: null, // { pane, asking }: the tab whose menu shows, and whether Close tab's question or the reminder's sheet does
  reminders: true, // whether Remind me is offered (Settings' reminders), from the stream
  remindTimes: [], // { label, clock }: the reminder box's times, with the clock time each means now, from the stream
  haptics: true, // whether the app buzzes (Settings' phone_haptics), from the stream
  views: null, // mission control's views for the panel, from the stream; null while Settings' phone_views is off
  panel: null, // { view }: the panel showing, on a view's items (its key) or the list (null)
  refreshed: {}, // when PRs and CI were last asked again from the panel, by view
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

// A short tap of feedback (Settings' phone_haptics): `navigator.vibrate`
// where the browser has it, as Android's does; else a click on the label of a
// switch made for it and removed at once, which iOS 18 and later answer with a
// haptic tick, as the ios-haptics library does. Safari exposes no `switch`
// property to test for, and the click is harmless where nothing answers it.
function buzz() {
  if (!state.haptics) return;
  if (typeof navigator.vibrate === "function") {
    navigator.vibrate(10);
    return;
  }
  const input = el("input", { type: "checkbox", switch: true, tabindex: "-1", "aria-hidden": "true" });
  const label = el("label", { "aria-hidden": "true" }, input);
  label.style.display = "none"; // through the CSSOM: the CSP refuses an inline style attribute
  document.body.append(label);
  label.click();
  label.remove();
}

// The controlling service worker's build: "none" while no worker controls
// the page, "unknown" for one too old to answer, null until it answers.
let workerBuild = null;

// Each request says which builds the page and its worker are, so the
// computer's log says when a phone runs one from before a restart.
function buildHeader() {
  return { "X-Switchboard-Build": "page=" + BUILD + (workerBuild ? " worker=" + workerBuild : "") };
}

async function askWorkerBuild() {
  const worker = navigator.serviceWorker && navigator.serviceWorker.controller;
  const answer = worker ? await askWorker(worker, { build: true }).catch(() => ({})) : {};
  // A worker that was replaced while it was asked answers for nothing: the
  // ask the change started says what controls the page now.
  if (worker !== (navigator.serviceWorker && navigator.serviceWorker.controller)) return;
  workerBuild = worker ? String(answer.build || "unknown") : "none";
  redrawBuildLine();
}

// The server's build read again, for a page left open while Switchboard
// restarted on a new one: as it comes back and as a new worker takes over.
async function readServerBuild() {
  const me = await api("/api/me").catch(() => null);
  if (!me || !me.build || !state.me) return;
  state.me.build = me.build;
  redrawBuildLine();
}

// From the page's first moment, since a new worker can take over before the
// page has asked the computer anything.
if (typeof navigator !== "undefined" && navigator.serviceWorker) {
  navigator.serviceWorker.addEventListener("controllerchange", () => {
    askWorkerBuild().catch(() => {});
    readServerBuild();
  });
}

function redrawBuildLine() {
  const line = app.querySelector(".build-line");
  if (line) line.replaceWith(buildLine());
}

// The dim line at the end of the drawer: the app's build, short, or, while
// the worker or the page isn't the server's build (the page's, from a server
// that names none), which is behind and how to take up the new one. A new
// worker takes over an open page as it installs, so either can be behind.
function buildLine() {
  const server = state.me && state.me.build;
  const workerBehind = !!workerBuild && workerBuild !== "none" && workerBuild !== (server || BUILD);
  const pageBehind = !!server && BUILD !== server;
  const short = BUILD.split("+").map((c) => c.slice(0, 7)).join("+");
  const behind = workerBehind ? "worker" : pageBehind ? "app" : null;
  return el("div", { class: "build-line" + (behind ? " behind" : "") }, behind ? behind + " behind, close and reopen the app" : "build " + short);
}

async function api(path, body) {
  const res = await fetch(path, {
    method: body === undefined ? "GET" : "POST",
    headers: { ...(body === undefined ? {} : { "Content-Type": "application/json" }), ...buildHeader() },
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

// The desktop's spinner (switchboard-ui's `text::SPINNER`), a frame every
// 120ms, read from the clock so a mark drawn again stays in step; every
// `.spin` on the page turns with it, and with reduced motion it holds still.
// Whether the page is drawn by a browser, which animates; the tests run this
// file in Node over a page of plain objects, where nothing moves or turns.
const moves = typeof requestAnimationFrame === "function" && typeof getComputedStyle === "function";
const SPINNER = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_MS = 120;
const spinFrame = () => (moves && !still() ? SPINNER[Math.floor(Date.now() / SPINNER_MS) % SPINNER.length] : "⠹");
const spinner = () => el("span", { class: "spin" }, spinFrame());
if (moves) {
  setInterval(() => {
    if (document.hidden || still()) return;
    const frame = spinFrame();
    for (const node of document.querySelectorAll(".spin")) node.textContent = frame;
  }, SPINNER_MS);
}

// A tab's mark as the sidebar draws it, in its tone, turning where the
// sidebar's does (`kit::spins`): the server sends its badge's.
// A server older than the page sends no badge: then the mark its state gives.
function markOf(tab) {
  if (tab.mark === undefined) {
    if (tab.pinned) return ["⚑", "pinned"];
    if (tab.state === "waiting") return ["●", "needs"];
    if (tab.state === "done") return ["✓", "passed"];
    if (tab.state === "working" || tab.state === "background") return [spinFrame(), "working spin"];
    return ["○", "idle"];
  }
  if (tab.spins) return [spinFrame(), tab.tone + " spin"];
  return [tab.mark, tab.tone];
}

// How long until a reminder, as the sidebar says it: in 45m, in 1h 20m, in 3d.
function until(at) {
  const m = Math.max(1, Math.ceil((at * 1000 - Date.now()) / 60000));
  if (m < 60) return "in " + m + "m";
  if (m < 1440) return "in " + Math.floor(m / 60) + "h" + (m % 60 ? " " + (m % 60) + "m" : "");
  return "in " + Math.round(m / 1440) + "d";
}

// A tab's reminder in words, as the sidebar says it: the time left and the
// note, or, come due, what it is for; null with none.
function reminded(tab) {
  const r = tab.remind;
  if (!r) return null;
  if (r.due) return r.note ? "reminder: " + r.note : "reminder";
  return until(r.at) + (r.note ? " · " + r.note : "");
}

// What the drawer says a tab is doing: the question a waiting tab asks, else
// the sidebar's words for it.
function sayingOf(tab) {
  if (tab.state === "waiting" && tab.prompt) return "asks: " + tab.prompt.question;
  return tab.words === undefined ? doing(tab) : tab.words;
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
        el("p", {}, spinner(), " Waiting for the code on " + computer),
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

function toast(text, failed, hint) {
  const t = el("div", { class: "toast" + (hint ? " hint" : "") }, hint ? null : failed ? el("span", { class: "bad" }, "✕") : el("span", { class: "ok" }, "✓"), text);
  document.body.append(t);
  setTimeout(() => leave(t), 3000);
}

// ── Motion (board 184): the timings are app.css's variables ──

const still = () => window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
const EASE = "cubic-bezier(.2, .8, .2, 1)";
const ms = (name) => (moves ? parseFloat(getComputedStyle(document.documentElement).getPropertyValue(name)) || 0 : 0);

// A node marked going plays app.css's leaving animation for it, then is
// taken out of the page and `then` runs; a node already out of the page, or
// one whose animation never ends, goes after the longest one would take.
function leave(node, then) {
  if (!moves) {
    node.remove();
    if (then) then();
    return;
  }
  // Going, it takes no tap or key, so a closing box's button can't be pressed twice.
  node.inert = true;
  let gone = false;
  const done = () => {
    if (gone) return;
    gone = true;
    node.remove();
    if (then) then();
  };
  node.classList.add("going");
  node.addEventListener("animationend", done);
  setTimeout(done, ms("--quick") + 100);
}

// Animations a redraw would cut short, by what they animate: the stream
// redraws the card, its explainers and the tray of images from scratch, so a
// node drawn again mid-animation takes it up where it was, its CSS animation
// started that long ago, rather than jumping to its end.
const playing = new Map(); // key → { cls, at, lasts }
function play(key, cls, lasts) {
  if (moves) playing.set(key, { cls, at: Date.now(), lasts });
}
function underWay(key) {
  const p = playing.get(key);
  if (p && Date.now() - p.at < p.lasts) return p;
  playing.delete(key);
  return null;
}
function resume(node, key) {
  const p = underWay(key);
  if (!p) return node;
  node.classList.add(p.cls);
  node.style.animationDelay = -(Date.now() - p.at) + "ms";
  return node;
}

// The colors a tab's mark or chip had last time it was drawn, by what it is
// (`row-mark:<pane>`, `head-chip:<pane>`), so one drawn again in a new
// state cross-fades from them; a tab that moved to Needs you pulses its mark once.
const looks = new Map();
function follow(node, id, needs) {
  const look = node.className;
  const was = looks.get(id);
  looks.set(id, { look, needs });
  if (!moves || !was) return node;
  // Moving to Needs you pulses the mark whether or not the mark changes (a
  // reminder coming due on an idle tab keeps its ○).
  const pulses = needs && !was.needs && node.classList.contains("mark") && !still();
  if (was.look === look && !pulses) return node;
  requestAnimationFrame(() => {
    if (!node.isConnected) return;
    if (pulses) node.animate([{ transform: "none" }, { transform: "scale(1.6)" }, { transform: "none" }], { duration: 2 * ms("--move"), easing: EASE });
    if (was.look === look) return;
    const probe = el(node.tagName, { class: was.look, style: "position:absolute;visibility:hidden" });
    document.body.append(probe);
    const from = getComputedStyle(probe);
    const to = getComputedStyle(node);
    node.animate(
      [{ color: from.color, backgroundColor: from.backgroundColor }, { color: to.color, backgroundColor: to.backgroundColor }],
      { duration: ms("--move"), easing: EASE },
    );
    probe.remove();
  });
  return node;
}

// The drawer and a tab's channel slide over each other as one opens the
// other: a tab comes in from the right over the drawer, and goes back out
// to the right off it; with reduced motion they cross-fade.
function slide(back, swap) {
  if (!moves) return swap();
  const old = app.querySelector(".screen:not(.going)");
  swap();
  const now = app.querySelector(".screen:not(.going)");
  if (!old || !now || old === now || !now.animate) return;
  old.classList.add("going");
  if (back) now.after(old);
  else now.before(old);
  const timing = { duration: ms("--slide"), easing: EASE };
  const away = still() ? { opacity: 0 } : { transform: back ? "translateX(100%)" : "translateX(-30%)", opacity: back ? 1 : 0.6 };
  const from = still() ? { opacity: 0 } : { transform: back ? "translateX(-30%)" : "translateX(100%)", opacity: back ? 0.6 : 1 };
  old.animate([{ transform: "none", opacity: 1 }, away], timing).onfinish = () => old.remove();
  now.animate([from, { transform: "none", opacity: 1 }], timing);
}

// ── The drawer (board 164) and its search (board 173) ──

// The words typed in the search, case aside.
const wordsOf = (q) => q.toLowerCase().split(/\s+/).filter(Boolean);
const holds = (text, words) => words.every((w) => String(text || "").toLowerCase().includes(w));
// Whether a tab's number, name, repository, branch or what it is doing hold
// every word typed, as the sidebar's / matches.
const matches = (tab, typed) => holds([tab.index, tab.name, tab.repo, tab.branch, sayingOf(tab), reminded(tab) || ""].join(" "), typed);

// The sidebar's groups holding a tab that matches, in its order: the server
// sends the tabs in it, each with its group. A server older than the page
// sends none, so its tabs go under Needs you, Working and Idle by their state.
const OLD_GROUPS = { needs: "Needs you", working: "Working", idle: "Idle" };
function groupOf(t) {
  if (t.stage !== undefined) return [t.stage, t.group, t.group_tone];
  const stage = t.needs ? "needs" : t.state === "working" || t.state === "background" ? "working" : "idle";
  return [stage, OLD_GROUPS[stage], stage];
}

function groups(typed) {
  const all = [];
  for (const t of state.tabs) {
    if (!matches(t, typed || [])) continue;
    const [stage, name, tone] = groupOf(t);
    const group = all.find((g) => g.stage === stage);
    if (group) group.tabs.push(t);
    else all.push({ stage, name, tone, tabs: [t] });
  }
  const old = Object.keys(OLD_GROUPS);
  if (state.tabs.some((t) => t.stage === undefined)) all.sort((a, b) => old.indexOf(a.stage) - old.indexOf(b.stage));
  return all;
}

// The groups folded on this phone, by stage, kept across reloads; nothing
// of them reaches the computer.
const FOLDS = "drawer-folds";
const folds = (() => {
  try {
    return new Set(JSON.parse(localStorage.getItem(FOLDS) || "[]"));
  } catch (_) {
    return new Set();
  }
})();

function foldGroup(stage) {
  if (folds.has(stage)) folds.delete(stage);
  else folds.add(stage);
  try {
    localStorage.setItem(FOLDS, JSON.stringify([...folds]));
  } catch (_) {}
  drawer();
}

const reconnecting = () => !state.live && Date.now() < state.reconnectingUntil;

function header(...rest) {
  return el(
    "div",
    { class: "head" },
    ...rest,
    follow(
      state.live
        ? el("span", { class: "chip g" }, "live")
        : reconnecting()
          ? el("span", { class: "chip y" }, "reconnecting")
          : el("span", { class: "chip r" }, "offline"),
      "live",
    ),
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
  if (!words.length || holds([tab.index, tab.name, sayingOf(tab), reminded(tab) || ""].join(" "), words)) return null;
  const by = [["repository ", tab.repo], ["branch ", tab.branch]].filter(([, v]) => v && words.some((w) => v.toLowerCase().includes(w)));
  return el("span", { class: "sub where-line" }, by.flatMap(([what, v], i) => [i ? " · " : "", el("span", { class: "faint" }, what), ...lit(v, words)]));
}

// The search box, made once and kept across the drawer's redraws, so what
// is typed and the keyboard stay while the server's updates come in.
let searchBox = null;
function search() {
  if (searchBox) return searchBox;
  const field = el("input", { type: "search", placeholder: "Search tabs", autocomplete: "off", autocapitalize: "off", spellcheck: "false", "aria-label": "Search tabs" });
  const clear = el("button", { class: "icon ghost clear", "aria-label": "Clear the search" }, "×");
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
  for (const { stage, name, tone, tabs } of all) {
    // While the search has words, every group with a match shows open, and
    // its heading folds nothing, so no fold changes out of sight.
    const searching = words.length > 0;
    const folded = !searching && folds.has(stage);
    const g = el(
      "div",
      { class: "group" },
      el(
        searching ? "div" : "button",
        searching
          ? { class: "label " + tone, "data-stage": stage }
          : { class: "label fold " + tone, "aria-expanded": String(!folded), "data-stage": stage, onclick: () => foldGroup(stage) },
        el("span", {}, (folded ? "▸ " : "▾ ") + name),
        el("span", {}, tabs.length),
      ),
    );
    for (const t of folded ? [] : tabs) {
      const [m, kind] = markOf(t);
      g.append(
        held(
          el(
            "button",
            { class: "row" + (state.menu && state.menu.pane === number(t) ? " held" : ""), "aria-haspopup": "menu", "data-pane": number(t) },
            follow(el("span", { class: "mark " + kind }, m), "row-mark:" + number(t), t.needs),
            el("span", { class: "name" }, el("span", { class: "num " + hue(t) }, lit(String(t.index), words)), lit(t.name || "", words)),
            el("span", { class: "sub" }, lit(sayingOf(t), words)),
            reminded(t) ? el("span", { class: "sub remind" }, el("span", { class: "clock", "aria-label": "Reminder" }, "⏰"), " ", lit(reminded(t), words)) : null,
            where(t, words),
          ),
          t,
        ),
      );
    }
    list.append(g);
  }
  list.append(buildLine());
  return list;
}

// ── A tab's menu, from a long press on its row (board 175) ──

const HOLD_MS = 500;

// The one hold under way, kept for the whole page rather than a row:
// the drawer is drawn again whenever the workspace changes, so the row a
// finger went down on may be gone by the time it lifts or moves.
const hold = { timer: null, start: null, swallow: false };

function letGo() {
  clearTimeout(hold.timer);
  hold.timer = null;
}

document.addEventListener("pointerdown", () => { hold.swallow = false; }, true);
for (const name of ["pointerup", "pointercancel"]) document.addEventListener(name, letGo, true);
document.addEventListener(
  "pointermove",
  (e) => { if (hold.timer && Math.hypot(e.clientX - hold.start[0], e.clientY - hold.start[1]) > 10) letGo(); },
  true,
);
// The click a long press ends with opens nothing and closes nothing.
document.addEventListener(
  "click",
  (e) => {
    if (!hold.swallow) return;
    hold.swallow = false;
    e.stopPropagation();
    e.preventDefault();
  },
  true,
);

// A row's tap opens its tab, or, while a menu shows, closes it; holding
// it, or the context menu event a screen reader's More or a keyboard's
// menu key sends, opens its menu.
function held(row, tab) {
  row.addEventListener("pointerdown", (e) => {
    letGo();
    hold.start = [e.clientX, e.clientY];
    hold.timer = setTimeout(() => {
      hold.timer = null;
      if (state.open || state.menu) return;
      buzz();
      hold.swallow = true;
      openMenu(number(tab));
    }, HOLD_MS);
  });
  row.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    // Sent by a long press too, on Android, whose click then waits.
    const isLongPress = !!hold.timer;
    if (isLongPress && !state.menu) buzz();
    if (isLongPress) hold.swallow = true;
    letGo();
    if (!state.menu) openMenu(number(tab));
  });
  row.addEventListener("click", () => (state.menu ? closeMenu() : openTab(number(tab))));
  return row;
}

// The layer over the page the menu and its question are drawn in, on the
// body so the drawer's redraws leave it be; a tap on it outside the box,
// or Escape, closes it.
let menuLayer = null;

function closeMenu() {
  state.menu = null;
  if (menuLayer) leave(menuLayer);
  menuLayer = null;
  document.removeEventListener("keydown", menuKeys);
  if (!state.open) drawer();
}

function menuKeys(e) {
  if (e.key === "Escape") closeMenu();
}

function openMenu(pane) {
  const tab = state.tabs.find((t) => number(t) === pane);
  if (!tab) return;
  state.menu = { pane, asking: false };
  if (!state.open) drawer();
  document.addEventListener("keydown", menuKeys);
  showMenu(tab);
}

// The veil fades in with the box rising into it; a box that takes the place
// of another (the menu's question, its sheet) rises in the veil already there.
function layer(box) {
  if (menuLayer) return menuLayer.replaceChildren(box);
  menuLayer = el("div", { class: "veil", onclick: (e) => { if (e.target === menuLayer) closeMenu(); } }, box);
  document.body.append(menuLayer);
}

function boxHead(title) {
  return el("div", { class: "box-head" }, el("span", { class: "box-title" }, title), el("button", { class: "icon", "aria-label": "Close", onclick: () => closeMenu() }, "×"));
}

// What a tab's menu offers: Mark seen on a finished tab, Pin it (Unpin it
// on a pinned tab), Clear the reminder on a tab with one, Remind me (board
// 176), then Close tab in the failed tone, as it ends the tab.
function menuChoices(tab) {
  const n = number(tab);
  const choices = [];
  if (tab.state === "done") choices.push(["Mark seen", "", () => markSeen(n)]);
  choices.push([tab.pinned ? "Unpin it" : "Pin it", "", () => pin(n, !tab.pinned)]);
  if (state.reminders && tab.remind) choices.push(["Clear the reminder", "", () => clearReminder(n)]);
  if (state.reminders) choices.push(["Remind me", "", () => remindSheet(n)]);
  if (state.close) choices.push(["Close tab", " r", () => ask(n)]);
  return choices;
}

// The kit's box under the row, or, from a channel's ⋯, under its header,
// where Mission control leads it (board 187), set off by a rule, while the
// panel is on.
function showMenu(tab) {
  const n = number(tab);
  const options = menuChoices(tab).map(([words, tone, act]) => el("button", { class: "menu-option" + tone, role: "menuitem", onclick: act }, words));
  if (state.open && state.views) {
    options.unshift(el("button", { class: "menu-option mc", role: "menuitem", onclick: () => openPanel() }, el("span", {}, "Mission control"), el("span", { class: "go" }, "›")));
  }
  if (!options.length) return closeMenu();
  const box = el("div", { class: "box", role: "menu", "aria-label": "Tab " + tab.index + " " + (tab.name || "") }, boxHead("Tab " + tab.index + " · " + (tab.name || "")), options);
  layer(box);
  const row = app.querySelector && app.querySelector(state.open ? ".screen:not(.going) .head" : '.row[data-pane="' + n + '"]');
  if (row && row.getBoundingClientRect) {
    const r = row.getBoundingClientRect();
    const below = r.bottom + 4;
    const tall = 44 + options.length * 44 + 10;
    box.style.top = below + tall > window.innerHeight ? Math.max(8, r.top - tall) + "px" : below + "px";
  }
  if (options[0].focus) options[0].focus();
}

// The question Close tab asks first, as every yes/no question asks (board
// 186): [Y/n] after it, No at the left and Close (the yes) lit at the right;
// only Close closes the tab.
function ask(n) {
  const tab = state.tabs.find((t) => number(t) === n);
  if (!tab) return closeMenu();
  state.menu = { pane: n, asking: true };
  const yes = el("button", { class: "option lit", onclick: () => closeTab(n) }, "Close");
  const box = el(
    "div",
    { class: "box ask", role: "alertdialog", "aria-label": "Close tab" },
    boxHead("Close tab"),
    el("div", { class: "question" }, el("b", {}, "Close tab " + tab.index + " " + (tab.name || "") + "? "), el("span", { class: "dim" }, "[Y/n]")),
    el("div", { class: "dim" }, "Its Claude and its panes end."),
    el("div", { class: "answers" }, el("button", { class: "option", onclick: () => closeMenu() }, "No"), yes),
  );
  layer(box);
  if (yes.focus) yes.focus();
}

// The sheet Remind me opens at the screen's foot (board 176): When, the
// workspace's five times each with the clock time it means, or one typed,
// which the computer reads; What for, optional. Set, at the right as the
// affirmative choice is, sets it, a typed time over a chosen one; a time the
// computer can't read is answered under the field and the sheet stays open.
// On a tab with a reminder it is Change the reminder, its note filled in.
function remindSheet(n) {
  const tab = state.tabs.find((t) => number(t) === n);
  if (!tab) return closeMenu();
  state.menu = { pane: n, asking: true };
  const change = !!tab.remind;
  let chosen = "";
  const typed = el("input", { type: "text", class: "field", placeholder: "45m, 3pm, tue 9am, nov 20", autocomplete: "off", autocapitalize: "off", spellcheck: "false", "aria-label": "Or type a time" });
  const note = el("input", { type: "text", class: "field", placeholder: "a note for when it comes due", autocomplete: "off", "aria-label": "What for" });
  note.value = change ? tab.remind.note || "" : "";
  const refused = el("div", { class: "refused", role: "alert" });
  // The chosen time lit, a yellow › before it, as a menu's chosen option.
  const offered = state.remindTimes;
  const times = offered.map((t) =>
    el("button", { class: "menu-option time", role: "radio", "aria-checked": "false", onclick: () => choose(t.label) }, el("span", {}, t.label), el("span", { class: "clock-time" }, t.clock)),
  );
  function choose(label) {
    light(label);
    typed.value = "";
  }
  function light(label) {
    chosen = label;
    times.forEach((o, i) => {
      const on = offered[i].label === label;
      o.className = "menu-option time" + (on ? " chosen" : "");
      o.setAttribute("aria-checked", String(on));
    });
  }
  const setIt = () => setReminder(n, (typed.value || "").trim() || chosen, note.value || "", refused);
  const set = el("button", { class: "option lit", onclick: setIt }, "Set");
  typed.addEventListener("keydown", (e) => { if (e.key === "Enter") setIt(); });
  // A time typed takes the place of the one chosen, which goes dark.
  typed.addEventListener("input", () => { if (chosen) light(""); });
  const box = el(
    "div",
    { class: "box sheet", role: "dialog", "aria-label": (change ? "Change the reminder for tab " : "Remind me about tab ") + tab.index + " " + (tab.name || "") },
    boxHead(change ? "Change the reminder" : "Remind me"),
    el("div", { class: "question" }, el("b", {}, (change ? "for tab " : "about tab ") + tab.index + " " + (tab.name || ""))),
    change ? el("div", { class: "dim" }, "⏰ " + reminded(tab)) : null,
    el("div", { class: "label d" }, "When"),
    el("div", { class: "times", role: "radiogroup", "aria-label": "When" }, times),
    el("div", { class: "input" }, el("span", { class: "prompt" }, "›"), typed),
    refused,
    el("div", { class: "label d" }, el("span", {}, "What for"), el("span", { class: "faint" }, "optional")),
    el("div", { class: "input" }, el("span", { class: "prompt" }, "›"), note),
    el("div", { class: "answers" }, el("button", { class: "option", onclick: () => closeMenu() }, "Cancel"), set),
  );
  layer(box);
}

async function setReminder(n, when, note, refused) {
  if (!when) {
    refused.replaceChildren(el("span", {}, "Choose a time, or type one."));
    return;
  }
  try {
    const r = await api("/api/tabs/" + n + "/remind", { when, note });
    closeMenu();
    toast(r.said);
  } catch (e) {
    refused.replaceChildren(el("span", {}, String(e.message || e)));
  }
}

async function clearReminder(n) {
  closeMenu();
  try {
    const r = await api("/api/tabs/" + n + "/remind", { clear: true });
    toast(r.said);
  } catch (e) {
    toast(String(e.message || e), true);
  }
}

async function markSeen(n) {
  closeMenu();
  try {
    await api("/api/tabs/" + n + "/seen", {});
  } catch (e) {
    toast(String(e.message || e), true);
  }
}

async function pin(n, on) {
  closeMenu();
  try {
    await api("/api/tabs/" + n + "/pin", { pin: on });
  } catch (e) {
    toast(String(e.message || e), true);
  }
}

async function closeTab(n) {
  closeMenu();
  try {
    const r = await api("/api/tabs/" + n + "/close", {});
    state.tabs = state.tabs.filter((t) => number(t) !== n);
    render();
    toast(r.said);
  } catch (e) {
    toast(String(e.message || e), true);
  }
}

function drawer() {
  state.whole = false;
  const { box, head, offline, list } = drawerParts();
  const screen = app.querySelector(".screen.drawer-screen:not(.going)");
  // The drawer already showing: everything but the search box drawn again,
  // since taking a focused field out of the page closes the keyboard.
  if (screen && screen.contains(box)) {
    const old = screen.querySelector(".drawer");
    const kept = old ? old.scrollTop : 0;
    // The heading or row with the keyboard's focus keeps it in the list drawn
    // again, so Enter on a heading can fold and unfold it in turn.
    const had = document.activeElement;
    const focused = old && had && old.contains(had) && had.getAttribute ? ["data-stage", "data-pane"].find((k) => had.getAttribute(k) !== null) : null;
    const key = focused ? "[" + focused + "=\"" + had.getAttribute(focused) + "\"]" : null;
    screen.querySelector(".head").replaceWith(head);
    screen.querySelector(".offline-slot").replaceWith(offline);
    if (old) old.replaceWith(list);
    else screen.append(list);
    list.scrollTop = kept;
    const again = key && list.querySelector(key);
    if (again) again.focus({ preventScroll: true });
    return;
  }
  app.replaceChildren(drawerScreen({ box, head, offline, list }));
}

function drawerScreen({ box, head, offline, list } = drawerParts()) {
  return el("div", { class: "screen drawer-screen" }, head, box, offline, list);
}

function drawerParts() {
  const box = search();
  box.show();
  const head = header(el("span", { class: "title" }, "Switchboard ", el("span", { class: "where" }, state.me.computer)));
  const offline = el("div", { class: "offline-slot" }, state.live ? null : el("div", { class: "offline-line" }, reconnecting() ? "Reconnecting…" : "Last heard " + ago(state.lastHeard) + " · trying again"));
  return { box, head, offline, list: drawerList() };
}

// ── A tab's channel (board 165), its prompt card (166) and composer (167) ──

const current = () => state.tabs.find((t) => number(t) === state.open);

// The tab's name at the top, cut to one line; a tap shows a name too long for
// it whole, wrapped, and a second folds it back.
function title(tab) {
  const node = el(
    "button",
    { class: "title" + (state.whole ? " whole" : ""), "aria-expanded": String(state.whole) },
    el("span", { class: "num " + hue(tab) }, tab.index),
    " ",
    tab.name,
  );
  node.addEventListener("click", () => {
    const cut = node.scrollWidth > node.clientWidth;
    if (!state.whole && !cut) return;
    state.whole = !state.whole;
    render();
  });
  return node;
}

function drop(i) {
  URL.revokeObjectURL(state.chosen[i].url);
  state.chosen.splice(i, 1);
}

async function openTab(n) {
  if (state.menu) closeMenu();
  // A view open over it (a push tapped meanwhile) closes first, its step in
  // the page's history taken back, so the tab is named on the page's own.
  const overAView = history.state && history.state.view;
  if (overAView) return closePanel(() => openTab(n));
  state.open = n;
  state.messages = [];
  state.turnsLoaded = false;
  state.answered = null;
  state.error = "";
  state.unfolded = new Set();
  state.turnKeys = [];
  while (state.chosen.length) drop(0);
  state.full = null;
  state.whole = false;
  history.replaceState(null, "", "#" + n);
  slide(false, () => render());
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
    state.turnsLoaded = true;
  } catch (_) {
    return;
  }
  render(true);
}

const messageKey = (m) => m.who + ":" + m.at + ":" + m.text.length;

// markdown-it (assets/phone/markdown-it.min.js, loaded before this file),
// with raw HTML off, so `<img onerror=…>` stays text, and bare http and https
// addresses made links; null where it isn't loaded, which leaves replies as text.
const md = typeof markdownit === "function" ? markdownit({ html: false, linkify: true }) : null;
// Only an address written with its scheme is made a link, never a file name
// such as README.md, which linkify-it would read as a site.
if (md) md.linkify.set({ fuzzyLink: false, fuzzyEmail: false });

// The page's elements for a reply's Markdown, built from markdown-it's
// tokens with `el`, every piece of text a text node: a table in a box, its
// cells wrapping to the screen's width, and a link only for http and https, opening in the
// browser and shown by its text; any other link is its text alone. Its
// `lines` are the lines it shows (a paragraph, a heading, a list item, a
// table row, a line of code), for the fold.
function markdown(text) {
  const root = el("div", { class: "body md" });
  root.lines = 0;
  const LINE = ["paragraph_open", "heading_open", "tr_open", "hr", "hardbreak"];
  const stack = [root];
  const top = () => stack[stack.length - 1];
  const opened = (t) => {
    if (t.hidden) return [top(), top()]; // a tight list's paragraph
    if (t.type === "table_open") {
      const table = el("table");
      return [el("div", { class: "table" }, table), table];
    }
    if (t.type === "link_open") {
      const href = t.attrGet("href") || "";
      const node = /^https?:\/\//i.test(href)
        ? el("a", { href, target: "_blank", rel: "noopener noreferrer" })
        : el("span", { class: "link" });
      return [node, node];
    }
    const align = (t.attrGet("style") || "").replace("text-align:", "");
    const node = el(t.tag, { class: align || null, start: t.type === "ordered_list_open" ? t.attrGet("start") : null });
    return [node, node];
  };
  const leaf = (t) => {
    if (t.type === "code_inline") return el("code", {}, t.content);
    if (t.type === "fence" || t.type === "code_block") return el("pre", {}, el("code", {}, t.content.replace(/\n$/, "")));
    if (t.type === "softbreak") return "\n";
    if (t.type === "hardbreak") return el("br");
    if (t.type === "hr") return el("hr");
    if (t.type === "image") return t.content; // its alt text: no image from elsewhere
    return t.content || null;
  };
  const walk = (tokens) => {
    for (const t of tokens) {
      if (LINE.includes(t.type)) root.lines += 1;
      if (t.type === "fence" || t.type === "code_block") root.lines += t.content.replace(/\n$/, "").split("\n").length;
      if (t.type === "inline") walk(t.children || []);
      else if (t.nesting === 1) {
        const [outer, inner] = opened(t);
        if (outer !== top()) top().append(outer);
        stack.push(inner);
      } else if (t.nesting === -1) stack.pop();
      else {
        const kid = leaf(t);
        if (kid !== null) top().append(kid instanceof Node ? kid : document.createTextNode(kid));
      }
    }
  };
  walk(md.parse(text, {}));
  return root;
}

// The messages as the channel draws them: each turn (what you said, or
// another tab's Claude sent, and what followed it) with everything Claude did before its final reply (its
// tool calls and the text it wrote between them) folded into one `steps`
// row, the reply after its last tool call left open. The newest turn of a
// working tab has no final reply yet, so all of it folds, and its row shows
// the latest step. A turn with no tool calls has no row; one opened shows
// its steps after the row. A turn ended by `switchboard respawn` keeps the
// text before that call open as its reply, after the row.
// What starts a turn: what you said, or what another tab's Claude sent,
// both shown; Claude Code's wake-up for a finished background task
// (`task`), shown as a dim line; or a subagent's report back (`turn`),
// which starts one but isn't drawn.
const starts = (m) => m.who === "you" || m.who === "tab" || m.who === "task" || m.who === "turn";
// A turn that ends by running `switchboard respawn` (queue-and-clean's
// last step) ends its session there, so the text written just before that
// call is the turn's reply.
const respawns = (m) => m.who === "tool" && /^Bash: (.*\s)?switchboard respawn\b/.test(m.text);

function turns(messages, working) {
  const groups = [];
  // Another tab's message that came in while Claude worked (`queued`) was
  // queued into the running turn, as the relay reads it, so it sits among
  // that turn's steps rather than starting one.
  for (const m of messages) {
    if ((starts(m) && !m.queued) || !groups.length) groups.push([]);
    groups[groups.length - 1].push(m);
  }
  // Each turn's key is what you said, which stays put as Claude adds steps.
  // The first group's message may have left the 80 the computer sends: it
  // keeps the key its turn had when last drawn, the newest of those keys no
  // turn drawn now has (older turns have left the window whole), since its
  // first step moves as the window does. A message you just sent (`sent`)
  // is the phone's copy until the computer's replaces it, so its key is
  // never carried.
  const keys = groups.map((turn) => (!starts(turn[0]) ? "" : (turn[0].sent ? "sent:" : "steps:") + messageKey(turn[0])));
  if (groups.length && !keys[0]) {
    const gone = state.turnKeys.filter((k) => k.startsWith("steps:") && !keys.includes(k));
    keys[0] = gone.length ? gone[gone.length - 1] : "steps:" + messageKey(groups[0][0]);
  }
  state.turnKeys = keys;
  return groups.flatMap((turn, g) => {
    const opened = starts(turn[0]) ? 1 : 0;
    const said = opened && turn[0].who !== "turn" ? [turn[0]] : [];
    const rest = turn.slice(opened);
    const last = rest.map((m) => m.who).lastIndexOf("tool");
    if (last < 0) return [...said, ...rest];
    let replied = last;
    while (respawns(rest[last]) && replied > 0 && rest[replied - 1].who === "claude") replied -= 1;
    const reply = rest.slice(replied, last);
    const unanswered = !reply.length && working && g === groups.length - 1;
    const steps = unanswered ? rest : [...rest.slice(0, replied), ...rest.slice(last, last + 1)];
    // The pictures Claude showed stay in sight with the steps folded.
    const looks = steps.filter((m) => m.who === "look");
    const key = keys[g];
    const open = state.unfolded.has(key);
    const row = {
      who: "steps",
      key,
      open,
      count: steps.filter((m) => m.who === "tool").length,
      latest: unanswered ? steps[steps.length - 1].text : "",
    };
    return [...said, row, ...(open ? steps : looks), ...reply, ...(unanswered ? [] : rest.slice(last + 1))];
  });
}

// A turn's steps folded into one row, a button that opens them and folds
// them again, kept with the open tab's messages opened (`state.unfolded`).
function stepsRow(m) {
  const words = (m.open ? "▾ " : "▸ ") + m.count + (m.count === 1 ? " step" : " steps");
  const node = el(
    "button",
    {
      class: "steps",
      "aria-expanded": String(m.open),
      onclick: () => {
        if (m.open) state.unfolded.delete(m.key);
        else state.unfolded.add(m.key);
        if (shown) shown.drawn = null;
        render();
        const again = shown && Array.from(shown.messages.querySelectorAll("button.steps")).find((b) => b.key === m.key);
        if (again) again.focus();
      },
    },
    words,
    m.latest ? el("span", { class: "latest" }, " · " + m.latest.split("\n")[0]) : null,
  );
  node.key = m.key;
  return el("div", { class: "msg" }, el("span"), node);
}

function message(m, i, all) {
  if (m.who === "steps") return stepsRow(m);
  if (m.who === "tool") return el("div", { class: "msg" }, el("span"), el("div", { class: "tool" }, m.text));
  if (m.who === "task") return el("div", { class: "msg" }, el("span"), el("div", { class: "task" }, m.text));
  const prev = all[i - 1];
  // One header over messages in a row only from the same sender: two
  // tabs' messages in a row each keep theirs (board 182). A look is
  // Claude's showing pictures: Claude's, as its words are.
  const speaker = (x) => (x.who === "look" ? "claude" : x.who);
  const same = prev && prev.who !== "tool" && prev.who !== "steps" && speaker(prev) === speaker(m) && (m.who !== "tab" || prev.from === m.from);
  const who = m.who === "you" ? "You" : m.who === "tab" ? m.from || "Another tab" : "Claude";
  const name = m.who === "tab" && m.tab
    ? el("span", { class: "who" }, el("span", { class: "num " + hue({ index: m.tab }) }, String(m.tab)), who.slice(String(m.tab).length))
    : el("span", { class: "who" }, who);
  const initial = m.who === "tab" ? (m.tab ? String(m.tab) : "↪") : who[0];
  const when = el("span", { class: "when" }, ago(m.at));
  when.at = m.at;
  const key = messageKey(m);
  const body = !m.text ? null : m.who !== "you" && md ? markdown(m.text) : el("div", { class: "body" }, m.text);
  // Rendered, a link's address, the Markdown's marks and the blank lines
  // between its blocks aren't counted.
  const shown = body ? body.textContent : "";
  const lines = body && body.lines !== undefined ? body.lines : m.text.split("\n").length;
  const long = lines > 10 || shown.length > 700;
  const folded = long && !state.unfolded.has(key);
  if (folded) body.classList.add("folded");
  const images = (m.images || []).length
    ? el(
        "div",
        { class: "shots" },
        m.images.map((name, n) => {
          const title = (m.titles || [])[n];
          const shot = el(
            "button",
            { class: "shot", "aria-label": "Show " + (title ? title : "the image") + " whole", onclick: () => { state.full = name; render(); } },
            el("img", { src: imageUrl(name), alt: m.who === "you" ? "An image you sent" : "A picture Claude showed", loading: "lazy" }),
          );
          return title ? el("figure", { class: "titled" }, shot, el("figcaption", {}, title)) : shot;
        }),
      )
    : null;
  // Show all opens a long message whole, and Show less, in its place, folds
  // it back, the message's top kept in view.
  const more = long
    ? el("button", {
        class: "more",
        onclick: () => {
          const opening = !state.unfolded.has(key);
          if (opening) state.unfolded.add(key);
          else state.unfolded.delete(key);
          body.classList.toggle("folded", !opening);
          more.textContent = opening ? "Show less" : "Show all";
          const node = !opening && more.closest && more.closest(".msg");
          // Its top hidden above the channel (under the header, not only off the screen).
          const list = node && node.closest(".channel");
          const listTop = list ? list.getBoundingClientRect().top : 0;
          if (node && node.getBoundingClientRect().top < listTop) node.scrollIntoView({ block: "start" });
        },
      }, folded ? "Show all" : "Show less")
    : null;
  return el(
    "div",
    { class: "msg" },
    same ? el("span") : el("span", { class: "avatar " + speaker(m) }, initial),
    same ? null : el("div", {}, name, when),
    body,
    more,
    images,
  );
}

const imageUrl = (name) => "/api/images/" + encodeURIComponent(name);

// An image shown whole, over the channel, until its × or a tap while it is
// at its fitted size. It zooms and pans, the one thing in the app that does:
// two fingers pinch it about the point between them (a trackpad's pinch in
// Chrome, a wheel with Ctrl, in the preview), one finger (or the mouse) drags it while
// zoomed, and a double tap zooms to 2.5× there or back to fitted. The zoom is
// a transform alone, so the page never reflows, and the image's edges are
// held at the screen's once it is bigger than the screen.
const ZOOM_MOST = 5;
const ZOOM_TAPPED = 2.5;
const DOUBLE_TAP = 300; // ms between a double tap's taps
const TAP_SLOP = 10; // px a tap may move and still be a tap

function full() {
  if (!state.full) return null;
  const name = state.full;
  const close = () => {
    if (tapped) clearTimeout(tapped.timer);
    state.full = null;
    render();
  };
  const img = el("img", { src: imageUrl(state.full), alt: "The image, whole", draggable: "false" });
  const box = el(
    "div",
    { class: "full" },
    el("button", { class: "icon close", "aria-label": "Close", onclick: close }, "×"),
    img,
  );
  // The image's place at its fitted size, read whenever it is there.
  let base = null; // { left, top, width, height }
  let s = 1;
  let tx = 0;
  let ty = 0;
  const fitted = () => s === 1;
  const measure = () => {
    if (!fitted() || !img.getBoundingClientRect) return;
    const r = img.getBoundingClientRect();
    // Not yet loaded, it has no size to zoom about.
    if (!r.width || !r.height) return;
    base = { left: r.left, top: r.top, width: r.width, height: r.height };
  };
  // Where an edge of the image sits on one axis: held to the screen's edges
  // while bigger than it, else centered where it was fitted, moving toward the
  // screen's middle as it grows so nothing jumps as it fills the screen.
  const edge = (wanted, start, size, screen) => {
    const now = size * s;
    if (now >= screen) return Math.min(0, Math.max(screen - now, wanted));
    const off = start - (screen - size) / 2;
    return (screen - now) / 2 + (screen > size ? (off * (screen - now)) / (screen - size) : 0);
  };
  const draw = () => {
    img.style.transform = fitted() ? "" : `translate(${tx}px, ${ty}px) scale(${s})`;
  };
  // Zoom to `to` keeping the image's point under (x, y) at (x2, y2).
  const zoom = (to, x, y, x2 = x, y2 = y) => {
    if (!base) return;
    const u = (x - base.left - tx) / s;
    const v = (y - base.top - ty) / s;
    s = Math.min(ZOOM_MOST, Math.max(1, to));
    if (s < 1.001) s = 1;
    pan(x2 - base.left - u * s, y2 - base.top - v * s);
  };
  const pan = (x, y) => {
    tx = edge(base.left + x, base.left, base.width, window.innerWidth) - base.left;
    ty = edge(base.top + y, base.top, base.height, window.innerHeight) - base.top;
    if (fitted()) tx = ty = 0;
    draw();
  };
  const settle = (on) => img.classList.toggle("settle", on);

  // The pointers down, by id, and the gesture they started.
  const down = new Map();
  let gesture = null; // { s, tx, ty, x, y, apart } as it started
  let moved = false;
  let tapped = null; // { x, y, at, timer }: a tap that may become a double tap
  const begin = () => {
    measure();
    const [a, b] = [...down.values()];
    gesture = b
      ? { s, tx, ty, x: (a.x + b.x) / 2, y: (a.y + b.y) / 2, apart: Math.hypot(a.x - b.x, a.y - b.y) || 1 }
      : { s, tx, ty, x: a.x, y: a.y };
  };
  box.addEventListener("pointerdown", (e) => {
    if (e.target.closest("button")) return;
    if (!down.size) moved = false;
    // A finger down again starts a gesture, never the tap before it closing.
    if (tapped && down.size) {
      clearTimeout(tapped.timer);
      tapped = null;
    }
    settle(false);
    down.set(e.pointerId, { x: e.clientX, y: e.clientY, x0: e.clientX, y0: e.clientY });
    if (box.setPointerCapture) box.setPointerCapture(e.pointerId);
    begin();
  });
  box.addEventListener("pointermove", (e) => {
    const p = down.get(e.pointerId);
    if (!p || !gesture || !base) return;
    p.x = e.clientX;
    p.y = e.clientY;
    if (Math.hypot(p.x - p.x0, p.y - p.y0) > TAP_SLOP) moved = true;
    const [a, b] = [...down.values()];
    if (b) {
      const x = (a.x + b.x) / 2;
      const y = (a.y + b.y) / 2;
      const to = (gesture.s * Math.hypot(a.x - b.x, a.y - b.y)) / gesture.apart;
      // From the gesture's start, so the fingers' point stays under them.
      [s, tx, ty] = [gesture.s, gesture.tx, gesture.ty];
      zoom(to, gesture.x, gesture.y, x, y);
    } else if (!fitted()) {
      pan(gesture.tx + p.x - gesture.x, gesture.ty + p.y - gesture.y);
    }
  });
  const up = (e) => {
    if (!down.delete(e.pointerId)) return;
    if (down.size) begin();
    else gesture = null;
  };
  box.addEventListener("pointerup", up);
  box.addEventListener("pointercancel", up);

  // A trackpad's pinch reaches a desktop browser as a wheel with Ctrl.
  box.addEventListener("wheel", (e) => {
    if (!e.ctrlKey) return;
    e.preventDefault();
    // Without the double tap's easing first, so the image is measured where it ends.
    settle(false);
    measure();
    zoom(s * Math.exp(-e.deltaY / 100), e.clientX, e.clientY);
  }, { passive: false });

  // A tap closes it while fitted, once it is plainly not a double tap.
  box.addEventListener("click", (e) => {
    if (e.target.closest("button") || moved) return;
    const at = e.timeStamp;
    if (tapped && at - tapped.at < DOUBLE_TAP && Math.hypot(e.clientX - tapped.x, e.clientY - tapped.y) < 40) {
      clearTimeout(tapped.timer);
      tapped = null;
      measure();
      settle(true);
      if (fitted()) zoom(ZOOM_TAPPED, e.clientX, e.clientY);
      else zoom(1, e.clientX, e.clientY);
      return;
    }
    if (tapped) clearTimeout(tapped.timer);
    tapped = { x: e.clientX, y: e.clientY, at, timer: setTimeout(() => {
      tapped = null;
      if (fitted() && state.full === name) close();
    }, DOUBLE_TAP) };
  });
  return box;
}

// One chosen image kept on the computer for the tab; its name.
async function upload(n, file) {
  const res = await fetch("/api/tabs/" + n + "/image", {
    method: "POST",
    headers: { "Content-Type": file.type || "application/octet-stream", ...buildHeader() },
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
  const going = !open && underWay("explain:" + key);
  const explainer = open || going ? resume(el("div", { class: "explainer" }, o.detail), "explain:" + key) : null;
  const info = el(
    "button",
    {
      class: "icon info" + (open ? " lit" : ""),
      "aria-label": open ? "Hide what this choice means" : "Show what this choice means",
      "aria-expanded": String(open),
      onclick: () => {
        state.explained = open ? "" : key;
        play("explain:" + key, open ? "going" : "coming", ms("--quick"));
        render();
        // Closing, it is drawn going until its slide ends, then left out.
        if (open && moves) setTimeout(render, ms("--quick"));
      },
    },
    "i",
  );
  return el("div", { class: "choice" }, el("div", { class: "row" }, button, info), explainer);
}

// A prompt as it stands: its id and which options are ticked, so a tick
// that changes nothing else still lets the next tap through.
const promptKey = (p) => p.id + ":" + p.options.map((o) => (o.checked ? 1 : 0)).join("");

async function answer(reply) {
  state.error = "";
  const tab = current();
  const p = tab && tab.prompt;
  if (!p) return;
  // Ticking an option of a question that takes several answers buzzes nothing; its Submit does.
  const isTick = p.multi && p.options.some((o) => String(o.n) === reply);
  if (!isTick) buzz();
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
  // Message text, not a form's field: no name AutoFill could fill with a
  // contact or address, with the keyboard's suggestions and corrections kept.
  const field = el("textarea", {
    rows: 1,
    name: "message",
    autocomplete: "off",
    autocorrect: "on",
    autocapitalize: "sentences",
    spellcheck: "true",
  });
  const send = el("button", { class: "icon send", "aria-label": "Send" }, "➤");
  // The photo library alone: images, several at once, no camera.
  const kinds = ["image/png", "image/jpeg", "image/gif", "image/webp"];
  const picker = el("input", { type: "file", accept: kinds.join(","), multiple: true, hidden: true });
  picker.addEventListener("change", () => {
    for (const file of picker.files) state.chosen.push({ file, url: URL.createObjectURL(file) });
    picker.value = "";
    render();
  });
  const plus = el("button", { class: "icon plus", "aria-label": "Add images from your photos", onclick: () => picker.click() }, "+");
  const free = () => {
    const tab = current();
    return !!(tab && tab.prompt && tab.prompt.free);
  };
  const ready = () => field.value.trim() !== "" || (!free() && state.chosen.length > 0);
  // CSS grows the box with its text (`field-sizing: content`). Where that
  // isn't supported it is measured here, which forces a layout of the whole
  // page, its channel included, so only when the text's line count could
  // have changed: a newline more or fewer, or a line's length crossing a
  // quarter of the characters the box fits across, so a wrap waits at most
  // a few characters for the box to grow. The width is read only as the box
  // is measured.
  const sizedByCss = !!globalThis.CSS?.supports("field-sizing", "content");
  let measured = { key: null, step: 1 };
  const keyOf = (step) => field.value.split("\n").map((line) => Math.floor(line.length / step)).join();
  const fit = () => {
    if (sizedByCss || keyOf(measured.step) === measured.key) return;
    field.style.height = "auto";
    field.style.height = field.scrollHeight + "px";
    // About half an em a character, less the box's padding.
    const perLine = Math.floor((field.clientWidth - 28) / 8);
    const step = Math.max(1, Math.floor(perLine / 4)) || 1;
    measured = { key: keyOf(step), step };
  };
  const resize = () => {
    send.classList.toggle("lit", ready());
    fit();
  };
  field.addEventListener("input", resize);
  // Images pasted join the ones + chose; the clipboard's text, if any, still
  // pastes as the browser would. A prompt's own answer takes no images.
  field.addEventListener("paste", (e) => {
    if (free()) return;
    const files = [...(e.clipboardData?.items || [])]
      .filter((item) => item.kind === "file" && kinds.includes(item.type))
      .map((item) => item.getAsFile())
      .filter(Boolean);
    if (!files.length) return;
    for (const file of files) state.chosen.push({ file, url: URL.createObjectURL(file) });
    render();
  });
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
        buzz();
        const images = [];
        for (const c of chosen) images.push(await upload(n, c.file));
        await api("/api/tabs/" + n + "/message", { text, images });
        for (const c of chosen) URL.revokeObjectURL(c.url);
        state.messages.push({ who: "you", text, images, at: Date.now(), sent: true });
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
  let trayShown = new Set();
  let error = el("div");
  const box = el("div", { class: "composing" }, tray, error, el("div", { class: "composer" }, plus, picker, field, send));
  box.sync = (tab) => {
    const p = tab.prompt;
    const free = p && p.free;
    field.placeholder = p && !free ? "Answer the question above" : free ? "Your own answer" : "Message " + tab.index + " " + tab.name;
    field.disabled = !!(p && !free);
    plus.disabled = !!p;
    // A thumbnail added grows in, and one dropped shrinks away before it goes.
    const newTray = state.chosen.length
      ? el(
          "div",
          { class: "tray" },
          state.chosen.map((c) => {
            if (!trayShown.has(c.url)) play("shot:" + c.url, "coming", ms("--quick"));
            const gone = () => {
              const i = state.chosen.indexOf(c);
              if (i >= 0) drop(i);
              render();
            };
            const shot = el(
              "div",
              { class: "shot" },
              el("img", { src: c.url, alt: "An image to send" }),
              el("button", {
                class: "drop",
                "aria-label": "Drop this image",
                onclick: () => {
                  if (!moves) return gone();
                  play("shot:" + c.url, "going", ms("--quick"));
                  render();
                  setTimeout(gone, ms("--quick"));
                },
              }),
            );
            resume(shot, "shot:" + c.url);
            return shot;
          }),
        )
      : el("div");
    trayShown = new Set(state.chosen.map((c) => c.url));
    tray.replaceWith(newTray);
    tray = newTray;
    const newError = state.error && !p ? el("div", { class: "error" }, state.error) : el("div");
    error.replaceWith(newError);
    error = newError;
    send.classList.toggle("lit", ready());
  };
  return box;
}

function toDrawer() {
  state.open = null;
  history.replaceState(null, "", "#");
  render();
}

function channelHead(tab) {
  const [m, kind] = markOf(tab);
  const chip = tab.state === "waiting" ? ["waiting", "y"] : tab.state === "done" ? ["done", "g"] : tab.state === "idle" ? ["idle", ""] : ["working", "b"];
  return el(
    "div",
    { class: "head" + (state.whole ? " whole" : "") },
    el("button", { class: "icon menu", "aria-label": "Tabs", onclick: () => slide(true, toDrawer) }, "☰"),
    follow(el("span", { class: "mark " + kind }, m), "head-mark:" + number(tab), tab.needs),
    title(tab),
    follow(el("span", { class: "chip " + chip[1] }, chip[0]), "head-chip:" + number(tab)),
    menuChoices(tab).length || state.views ? el("button", { class: "icon tab-menu", "aria-label": "Tab " + tab.index + "'s menu", "aria-haspopup": "menu", onclick: () => openMenu(number(tab)) }, "⋯") : null,
  );
}

// The open tab's screen, kept while it is open: a redraw puts in a new head,
// prompt card and "doing" line, an image shown whole only when another is
// shown (so one zoomed stays zoomed), the messages only when
// the turns changed, and leaves the composer where it is, since taking a
// focused field out of the page closes the keyboard.
let shown = null; // { open, screen, head, list, messages, drawn, count, working, arrived, now, cardKey, composer, full, fullName }

function channel(stick) {
  const tab = current();
  if (!tab) {
    state.open = null;
    return drawer();
  }
  const kept = shown && shown.open === state.open && shown.screen.parentNode === app && !shown.screen.classList.contains("going") ? shown : null;
  const atEnd = !kept || kept.list.scrollTop + kept.list.clientHeight >= kept.list.scrollHeight - 40;
  const top = kept ? kept.list.scrollTop : 0;
  if (!kept) {
    const messages = el("div", { class: "messages" });
    const now = el("div", { class: "now" });
    const list = el("div", { class: "channel" }, messages, now);
    const head = el("div", { class: "head" });
    const box = composer();
    const whole = el("div");
    shown = { open: state.open, screen: el("div", { class: "screen channel-screen" }, head, list, box, whole), head, list, messages, drawn: null, count: 0, arrived: null, now, cardKey: "", composer: box, full: whole, fullName: null };
    swipeable(shown.screen);
    app.replaceChildren(shown.screen);
  }
  const head = channelHead(tab);
  shown.head.replaceWith(head);
  shown.head = head;
  const working = tab.state === "working" || tab.state === "background";
  // A tab in the background (a Monitor, a /loop between runs) has ended its turn.
  const answering = tab.state === "working";
  if (shown.drawn !== state.messages || shown.count !== state.messages.length || shown.working !== answering) {
    // A message or row not drawn before arrived while the channel was open,
    // and rises in; the first load, a redraw of the same turns and steps
    // opened or folded move nothing.
    const before = shown.arrived ? new Map(shown.arrived) : null;
    const drawn = turns(state.messages, answering);
    const nodes = drawn.map((m, i, all) => {
      const node = message(m, i, all);
      const k = arrivalKey(m);
      const left = before && before.get(k);
      if (moves && before && !left) node.classList.add("coming");
      if (left) before.set(k, left - 1);
      return node;
    });
    shown.messages.replaceChildren(...nodes);
    shown.drawn = state.messages;
    shown.count = state.messages.length;
    shown.working = answering;
    shown.arrived = null;
    if (state.turnsLoaded) {
      shown.arrived = new Map();
      for (const m of [...state.messages, ...drawn.filter((d) => d.who === "steps")]) shown.arrived.set(arrivalKey(m), (shown.arrived.get(arrivalKey(m)) || 0) + 1);
    }
  } else {
    // Kept, so their times ("now", "3m ago") are worked out again in place.
    for (const when of shown.messages.querySelectorAll(".when")) when.textContent = ago(when.at);
  }
  const newCard = card(tab);
  const cardKey = tab.prompt ? "prompt:" + tab.prompt.id : newCard ? "answered" : "";
  const oldCard = shown.now.querySelector(".card");
  const wasTall = oldCard ? oldCard.getBoundingClientRect().height : 0;
  shown.now.replaceChildren(...[newCard, working ? el("div", { class: "doing" }, spinner(), " " + doing(tab)) : null].filter(Boolean));
  if (moves && kept && newCard && cardKey !== shown.cardKey) {
    const folds = cardKey === "answered" && shown.cardKey && shown.cardKey.startsWith("prompt:") && wasTall;
    if (folds) shown.folding = { at: Date.now(), wasTall };
    else play("card:" + cardKey, "coming", ms("--move"));
  }
  // Drawn again mid-way, the card carries on rising or folding.
  if (newCard) resume(newCard, "card:" + cardKey);
  const foldGone = shown.folding ? Date.now() - shown.folding.at : Infinity;
  if (newCard && cardKey === "answered" && foldGone < ms("--move")) fold(newCard, shown.folding.wasTall, foldGone);
  shown.cardKey = cardKey;
  shown.composer.sync(tab);
  // The same image stays as it was, zoomed or not, as the channel updates.
  if (shown.fullName !== state.full) {
    const whole = full() || el("div");
    shown.full.replaceWith(whole);
    shown.full = whole;
    shown.fullName = state.full;
  }
  shown.list.scrollTop = stick || atEnd ? shown.list.scrollHeight : top;
}

// What tells a message from the others for its arrival: who sent it and its
// words, not its time, which the copy shown as it was sent has its own of.
const arrivalKey = (m) => (m.who === "steps" ? "steps\u0000" + m.key : m.who + "\u0000" + (m.text || "") + "\u0000" + (m.images || m.looks || []).join(" "));

// An answered card folds from the prompt's height down to its "You answered"
// line, its words fading in; with reduced motion it only fades.
function fold(card, wasTall, gone) {
  if (!moves || !card.animate) return;
  const timing = { duration: ms("--move"), easing: EASE };
  const tall = card.getBoundingClientRect().height;
  const at = (a) => { a.currentTime = gone; };
  if (!still() && tall) {
    card.style.transformOrigin = "top";
    at(card.animate([{ transform: "scaleY(" + wasTall / tall + ")" }, { transform: "none" }], timing));
  }
  for (const kid of card.children) at(kid.animate([{ opacity: 0 }, { opacity: 1 }], timing));
}

// Swiping right in a channel (board 164's second phone): the drawer drawn
// under the channel once the touch moves right more than up or down, the
// channel following the finger. Let go past a third of the screen's width,
// or flicked right, it opens the drawer as ☰ does; otherwise it slides back.
// A touch in the composer's field, its tray of images or an image shown
// whole is theirs.
const SWIPE_DECIDES = 10; // px moved before a touch counts as sideways or not
const SWIPE_FLICK = 0.5; // px a millisecond rightward that open the drawer however far it went
const SWIPE_FLICK_MS = 100; // a flick lets go within this of its last move
let swipe = null; // { screen, x, y, dx, at, speed, under, wanted, buzzed, sliding }: the touch under way, or the slide after it

function swipeable(screen) {
  screen.addEventListener("touchstart", (e) => {
    if (swipe && swipe.sliding) return;
    // A second finger mid-swipe sends the channel back, as a cancel does.
    if (swipe && swipe.under) return end({ type: "touchcancel" });
    swipe = null;
    if (e.touches.length !== 1 || e.target.closest("textarea, .tray, .full")) return;
    const t = e.touches[0];
    swipe = { screen, x: t.clientX, y: t.clientY, dx: 0, at: e.timeStamp, speed: 0, under: null, wanted: null };
  }, { passive: true });
  screen.addEventListener("touchmove", (e) => {
    if (!swipe || swipe.sliding || swipe.screen !== screen || e.touches.length !== 1) return;
    const t = e.touches[0];
    const dx = t.clientX - swipe.x;
    const dy = t.clientY - swipe.y;
    if (!swipe.under) {
      if (Math.max(Math.abs(dx), Math.abs(dy)) < SWIPE_DECIDES) return;
      const stillOpen = state.open && screen.parentNode === app;
      if (dx <= Math.abs(dy) || !stillOpen) {
        swipe = null;
        return;
      }
      swipe.under = drawerScreen();
      // A second finger on the drawer uncovered ends it too.
      swipe.under.addEventListener("touchstart", () => end({ type: "touchcancel" }), { passive: true });
      app.insertBefore(swipe.under, screen);
      app.classList.add("swiping");
    }
    e.preventDefault();
    const elapsed = e.timeStamp - swipe.at;
    if (elapsed > 0) swipe.speed = (dx - swipe.dx) / elapsed;
    swipe.at = e.timeStamp;
    swipe.dx = Math.max(0, dx);
    // Once a swipe, as it crosses where letting go opens the drawer.
    if (!swipe.buzzed && swipe.dx > window.innerWidth / 3) {
      swipe.buzzed = true;
      buzz();
    }
    screen.style.transform = "translateX(" + swipe.dx + "px)";
  }, { passive: false });
  const end = (e) => {
    if (!swipe || swipe.sliding || swipe.screen !== screen) return;
    const done = swipe;
    swipe = null;
    if (!done.under) return;
    const opens = e.type === "touchend" && (done.dx > window.innerWidth / 3 || (done.speed > SWIPE_FLICK && e.timeStamp - done.at < SWIPE_FLICK_MS));
    // With reduced motion, the channel fades out as the drawer opens, or is
    // put back at once, rather than sliding.
    const lasts = still() ? (opens ? ms("--quick") : 0) : ms("--slide");
    if (still()) {
      screen.style.transition = "opacity " + lasts + "ms " + EASE;
      screen.style.transform = "";
      if (opens) screen.style.opacity = "0";
    } else {
      screen.style.transition = "transform " + lasts + "ms " + EASE;
      screen.style.transform = opens ? "translateX(100%)" : "";
    }
    // Held off until the slide ends, so no redraw lands mid-slide.
    swipe = { screen, under: done.under, wanted: done.wanted, sliding: true };
    setTimeout(() => {
      const wanted = swipe.wanted;
      swipe = null;
      app.classList.remove("swiping");
      screen.style.transition = "";
      screen.style.transform = "";
      screen.style.opacity = "";
      if (opens) {
        screen.remove();
        toDrawer();
        return;
      }
      done.under.remove();
      if (wanted) render(wanted.stick);
    }, lasts);
  };
  screen.addEventListener("touchend", end);
  screen.addEventListener("touchcancel", end);
}

function render(stick) {
  if (!state.me || !state.me.paired) return;
  if (swipe && swipe.under) {
    swipe.wanted = { stick: !!stick || !!(swipe.wanted && swipe.wanted.stick) };
    return;
  }
  if (state.open) channel(stick);
  else drawer();
}

// ── Mission control's panel (boards 185 to 187) ──
// A panel swiped in from the right edge, on a channel or the drawer, or
// opened from a channel's ⋯: mission control's Landing, Autopilot, PRs, CI
// Notifications and Usage views as the computer sends them (src/phone/mission.rs),
// each a 56 row with what needs you counted; a tap on one slides its items
// in full screen, over the panel, under mission control's groups, with ‹
// (a swipe right from its left edge, or the phone's back) back to the
// panel's list; a tap on an item opens its menu,
// the options the phone can do: going to a tab opens its channel, opening
// or copying a link happens here, and the rest runs on the computer, asked
// first where the option says so. A swipe right, or a tap on the dimmed
// strip at its left, puts it away. Off with Settings' phone_views, the
// stream carries no views and none of it shows.

const EDGE = 24; // px from the right edge a swipe starts the panel from
const REFRESH_MS = 60000; // the least time between asking PRs or CI again
const HINT = "mc-hint-shown"; // the toast was shown, kept on this phone
let panelLayer = null; // { layer, panel, strip, screen }: the panel while it shows, screen the view open over it
let edge = null; // { x, y, dx, at, speed, decided, opening }: an edge swipe under way

// Whether the toast was shown, kept on this phone alone; a page without
// storage shows it each time.
function hinted(set) {
  try {
    if (set) localStorage.setItem(HINT, "1");
    return !!localStorage.getItem(HINT);
  } catch (_) {
    return false;
  }
}

const viewOf = (key) => (state.views || []).find((v) => v.key === key);

// The panel put away, and the history step an open view added taken back
// before `then` runs, so a tab it opens is named on the page's own step.
function closePanel(then) {
  state.panel = null;
  if (panelLayer) {
    const { layer, panel } = panelLayer;
    panelLayer = null;
    slidePanel(layer, panel, false, () => layer.remove());
  }
  unwind(then);
}

let unwinding = null; // what runs once the view's history step is taken back

// Takes back the step in the page's history that opening a view added, then
// runs `then`; at once when there is none.
function unwind(then) {
  const pushed = history.state && history.state.view && typeof history.back === "function";
  if (!pushed) return then && then();
  if (unwinding) return;
  unwinding = then || (() => {});
  history.back();
}

// The panel slides in or out as swipeable slides a channel: over --slide, or,
// with Reduce Motion, fading.
function slidePanel(layer, panel, opens, then) {
  const lasts = still() ? ms("--quick") : ms("--slide");
  const strip = layer.querySelector(".panel-strip");
  if (still()) {
    // Where a swipe left it, it comes to rest at once and only fades.
    panel.style.transform = "";
    if (strip) strip.style.opacity = "";
    layer.style.transition = "opacity " + lasts + "ms " + EASE;
    layer.style.opacity = opens ? "1" : "0";
  } else {
    panel.style.transition = "transform " + lasts + "ms " + EASE;
    panel.style.transform = opens ? "" : "translateX(100%)";
    if (strip) {
      strip.style.transition = "opacity " + lasts + "ms " + EASE;
      strip.style.opacity = opens ? "1" : "0";
    }
  }
  setTimeout(() => {
    panel.style.transition = "";
    if (strip) strip.style.transition = "";
    if (then) then();
  }, lasts);
}

function panelLayerNode() {
  const strip = el("div", { class: "panel-strip", onclick: () => closePanel() });
  const panel = el("div", { class: "panel", role: "dialog", "aria-label": "Mission control" });
  const layer = el("div", { class: "panel-layer" }, strip, panel);
  panelSwipe(panel, 0, () => closePanel(), () => slidePanel(panelLayer.layer, panel, true));
  return { layer, panel, strip, screen: null };
}

// Opened from ⋯ or a swipe that went far enough: slid in from where it is.
function openPanel(from) {
  if (!state.views) return;
  hinted(true);
  if (state.menu) closeMenu();
  state.panel = { view: null };
  if (!panelLayer) {
    panelLayer = from || panelLayerNode();
    if (!from) {
      panelLayer.panel.style.transform = still() ? "" : "translateX(100%)";
      if (still()) panelLayer.layer.style.opacity = "0";
      panelLayer.strip.style.opacity = still() ? "" : "0";
      document.body.append(panelLayer.layer);
      // Laid out at its start before it slides.
      void panelLayer.panel.offsetWidth;
    }
  }
  drawPanel();
  slidePanel(panelLayer.layer, panelLayer.panel, true);
}

// The panel on its list of views, and the view open over it, if any, each
// keeping where it was scrolled to as the stream redraws them.
function drawPanel() {
  if (!panelLayer || !state.panel) return;
  const view = state.panel.view && viewOf(state.panel.view);
  if (state.panel.view && !view) {
    closeView();
    unwind();
  }
  const head = el(
    "div",
    { class: "head" },
    el("button", { class: "icon", "aria-label": "Close mission control", onclick: () => closePanel() }, "›"),
    el("span", { class: "title" }, "Mission control"),
  );
  fill(panelLayer.panel, head, viewList(), "");
  if (!view) return;
  if (!panelLayer.screen) {
    const screen = el("div", { class: "panel-view", role: "dialog", "aria-label": view.title });
    panelSwipe(screen, EDGE, () => back(), () => slideView(screen, true));
    panelLayer.screen = screen;
    panelLayer.layer.append(screen);
    fill(screen, viewHead(view), viewItems(view), view.key);
    slideView(screen, true, true);
    return;
  }
  fill(panelLayer.screen, viewHead(view), viewItems(view), view.key);
}

function fill(node, head, list, key) {
  const old = node.querySelector(".panel-list");
  const kept = old && old.viewKey === key ? old.scrollTop : 0;
  list.viewKey = key;
  node.replaceChildren(head, list);
  list.scrollTop = kept;
}

// A view slides in from the right over the panel as it opens and out as
// it closes, over --slide as the panel does, at once with Reduce Motion.
function slideView(screen, opens, from) {
  if (still()) {
    screen.style.transform = opens ? "" : "translateX(100%)";
    return;
  }
  if (from) {
    screen.style.transform = "translateX(100%)";
    // Laid out at its start before it slides.
    void screen.offsetWidth;
  }
  const lasts = ms("--slide");
  screen.style.transition = "transform " + lasts + "ms " + EASE;
  screen.style.transform = opens ? "" : "translateX(100%)";
  setTimeout(() => (screen.style.transition = ""), lasts);
}

// Back from a view, taking back the step opening it added to the page's
// history, as the phone's own back does.
function back() {
  unwind(() => closeView());
}

// The view put away, the panel showing on its list under it.
function closeView() {
  if (state.panel) state.panel.view = null;
  if (!panelLayer || !panelLayer.screen) return;
  const screen = panelLayer.screen;
  panelLayer.screen = null;
  slideView(screen, false);
  if (still()) screen.remove();
  else setTimeout(() => screen.remove(), ms("--slide"));
}

function viewHead(view) {
  return el(
    "div",
    { class: "head" },
    el("button", { class: "icon", "aria-label": "Back to the views", onclick: () => back() }, "‹"),
    el("span", { class: "title" }, view.title),
    view.acts && view.acts.length
      ? el("button", { class: "icon tab-menu", "aria-label": view.title + "'s menu", "aria-haspopup": "menu", onclick: () => viewMenu(view.key) }, "⋯")
      : null,
  );
}

// The views, each a row: its name, a line of what it holds, and what needs
// you counted at the right with the first one's mark and tone, no count
// when nothing does.
function viewList() {
  const list = el("div", { class: "panel-list drawer" + (state.live ? "" : " off") });
  if (!state.views.length) list.append(el("div", { class: "empty" }, "No view of mission control's shows now."));
  for (const v of state.views) {
    list.append(
      el(
        "button",
        { class: "row view-row", "data-view": v.key, onclick: () => openView(v.key) },
        el("span", { class: "mark " + (v.tone || "idle") }, v.mark || "○"),
        el("span", { class: "name" }, v.title),
        v.needs ? el("span", { class: "count " + (v.tone || "") }, v.needs) : el("span", {}),
        el("span", { class: "sub" }, v.line),
      ),
    );
  }
  return list;
}

function openView(key) {
  state.panel.view = key;
  // A step in the page's history, so the phone's back gesture closes it.
  if (typeof history.pushState === "function") history.pushState({ view: key }, "");
  drawPanel();
  // PRs and CI read again as they open, at most once a minute, so they are
  // current while the computer's lookups pause with you away.
  const v = viewOf(key);
  const again = v && (v.acts || []).find((a) => /^Read .* again now$/.test(a.label) && !a.ask);
  const last = state.refreshed[key] || 0;
  if (again && Date.now() - last > REFRESH_MS) {
    state.refreshed[key] = Date.now();
    api("/api/views/" + encodeURIComponent(key) + "/act", { label: again.label }).catch(() => {});
  }
}

const TONE_OF_GROUP = (items) => (items[0] && items[0].tone) || "idle";

// A view's items under its groups, in its order and tones, or its own words
// for having none, needing setting up, or its last look failing.
function viewItems(view) {
  if (view.key === "usage") return usageItems(view);
  const list = el("div", { class: "panel-list drawer" + (state.live ? "" : " off") });
  const items = view.groups.reduce((n, g) => n + g.items.length, 0);
  if (view.setup) list.append(el("div", { class: "empty" }, view.setup));
  else if (!items) list.append(el("div", { class: "empty" }, view.loaded === false ? "Loading…" : view.empty || "Nothing here."));
  if (view.error) list.append(el("div", { class: "empty refused" }, view.error));
  if (view.setup) return list;
  for (const g of view.groups) {
    const group = el("div", { class: "group" });
    if (g.title) group.append(el("div", { class: "label " + TONE_OF_GROUP(g.items) }, el("span", {}, g.title), el("span", {}, g.items.length)));
    for (const item of g.items) {
      group.append(
        el(
          "button",
          { class: "row item-row", "data-item": item.id, "aria-haspopup": "menu", onclick: () => itemMenu(view.key, item.id) },
          el("span", { class: "mark " + item.tone }, item.mark),
          el("span", { class: "name" }, item.title),
          el("span", { class: "when " + (item.tone === "idle" ? "" : item.tone) }, item.time ? ago(item.time).replace(" ago", "") : ""),
          item.detail ? el("span", { class: "sub" }, item.detail) : null,
        ),
      );
    }
    list.append(group);
  }
  return list;
}

const dollars = (n) => "$" + Number(n || 0).toFixed(2);

// The Usage view (src/phone/views.rs's usage_json), drawn from its numbers:
// each limit a meter of the kit (board 184) in its tone, with its percent,
// when it resets and, when its pace runs over, where that pace ends up;
// today's and the week's spend; today's top tabs by spend, a tap on an
// open one opening its channel. Nothing on it acts on the computer.
function usageItems(view) {
  const list = el("div", { class: "panel-list drawer usage" + (state.live ? "" : " off") });
  const u = view.usage;
  if (!u) {
    list.append(el("div", { class: "empty" }, view.loaded === false ? "Loading…" : view.empty || "Nothing here."));
    if (view.error) list.append(el("div", { class: "empty refused" }, view.error));
    return list;
  }
  if (u.limits.length) {
    const group = el("div", { class: "group" }, el("div", { class: "label idle" }, el("span", {}, "Limits"), el("span", {})));
    for (const l of u.limits) {
      const fill = el("span", { class: "fill " + l.tone });
      fill.style.width = Math.max(0, Math.min(100, l.used)) + "%";
      const meter = el("span", { class: "meter" }, fill);
      if (l.pace !== null && l.pace !== undefined) {
        const tick = el("span", { class: "tick" });
        tick.style.left = Math.min(100, l.pace) + "%";
        meter.append(tick);
      }
      const words = [l.resets];
      if (l.over) words.push("at this pace, " + l.pace + "% by then");
      group.append(
        el(
          "div",
          { class: "limit", "data-limit": l.key },
          el("span", { class: "name" }, l.name),
          el("span", { class: "pct " + l.tone }, l.used + "%"),
          meter,
          el("span", { class: "sub" + (l.over ? " needs" : "") }, words.join(" · ")),
        ),
      );
    }
    list.append(group);
  }
  list.append(
    el(
      "div",
      { class: "group" },
      el("div", { class: "label idle" }, el("span", {}, "Spend"), el("span", {})),
      el("div", { class: "spend" }, el("span", {}, "Today"), el("b", {}, dollars(u.today))),
      el("div", { class: "spend" }, el("span", {}, "This week"), el("b", {}, dollars(u.week))),
    ),
  );
  if (u.tabs.length) {
    const group = el("div", { class: "group" }, el("div", { class: "label idle" }, el("span", {}, "Today's tabs"), el("span", {})));
    for (const t of u.tabs) {
      const n = t.pane ? String(t.pane).replace(/^%/, "") : null;
      const open = n && state.tabs.some((x) => number(x) === n);
      group.append(
        el(
          open ? "button" : "div",
          { class: "row usage-tab", onclick: open ? () => closePanel(() => openTab(n)) : null },
          el("span", { class: "mark idle" }, open ? "●" : "○"),
          el("span", { class: "name" }, t.index ? el("span", { class: "num" }, t.index) : null, t.name),
          el("span", { class: "when" }, dollars(t.spent)),
        ),
      );
    }
    list.append(group);
  }
  return list;
}

const itemOf = (view, id) => {
  for (const g of (view && view.groups) || []) {
    const found = g.items.find((i) => i.id === id);
    if (found) return found;
  }
  return null;
};

// What a menu offered, so one opened before the computer changed it is
// closed rather than acted on.
const optionsKey = (options) => JSON.stringify(options || []);

function itemMenu(key, id) {
  const item = itemOf(viewOf(key), id);
  if (!item) return;
  if (!item.options.length) return toast("Nothing to do with it from the phone.");
  state.menu = { panel: true, view: key, id, key: optionsKey(item.options), asking: false };
  document.addEventListener("keydown", menuKeys);
  showOptions(item.title, item.options, (o) => runOption(key, id, o));
}

function viewMenu(key) {
  const view = viewOf(key);
  if (!view || !view.acts.length) return;
  state.menu = { panel: true, view: key, id: null, key: optionsKey(view.acts), asking: false };
  document.addEventListener("keydown", menuKeys);
  showOptions(view.title, view.acts, (o) => runOption(key, null, o));
}

function showOptions(title, options, pick) {
  const buttons = options.map((o) => el("button", { class: "menu-option" + (/^Take it out|^Close/.test(o.label) ? " r" : ""), role: "menuitem", onclick: () => pick(o) }, o.shown || o.label));
  const box = el("div", { class: "box options", role: "menu", "aria-label": title }, boxHead(title), buttons);
  layer(box);
  if (buttons[0] && buttons[0].focus) buttons[0].focus();
}

// What the phone does itself told to the computer, which puts a
// notification away after it and records what it led to, as mission
// control does after any pick; nothing is said of it.
function told(key, id, o) {
  if (id === null) return;
  api("/api/views/" + encodeURIComponent(key) + "/item", { id, label: o.label }).catch(() => {});
}

function runOption(key, id, o) {
  if (o.go || o.open || o.copy) told(key, id, o);
  if (o.go) {
    closeMenu();
    const n = String(o.go).replace(/^%/, "");
    closePanel(() => {
      if (state.tabs.some((t) => number(t) === n)) openTab(n);
      else toast("That tab isn't running Claude.", true);
    });
    return;
  }
  if (o.open) {
    closeMenu();
    window.open(o.open, "_blank", "noopener");
    return;
  }
  if (o.copy) {
    closeMenu();
    copy(o.copy);
    return;
  }
  if (o.ask) return askFirst(key, id, o);
  send(key, id, o);
}

async function copy(text) {
  try {
    await navigator.clipboard.writeText(text);
    toast("Copied " + text);
  } catch (_) {
    toast("Couldn't copy it.", true);
  }
}

// The kit's question (board 186): [Y/n], No at the left and Yes lit at the
// right, Yes the default.
function askFirst(key, id, o) {
  state.menu.asking = true;
  const title = (o.shown || o.label).replace(/…$/, "");
  const yes = el("button", { class: "option lit", onclick: () => send(key, id, o) }, "Yes");
  const box = el(
    "div",
    { class: "box ask", role: "alertdialog", "aria-label": title },
    boxHead(title),
    el("div", { class: "question" }, el("b", {}, o.ask + " "), el("span", { class: "dim" }, "[Y/n]")),
    el("div", { class: "answers" }, el("button", { class: "option", onclick: () => closeMenu() }, "No"), yes),
  );
  layer(box);
  if (yes.focus) yes.focus();
}

async function send(key, id, o) {
  closeMenu();
  const path = "/api/views/" + encodeURIComponent(key) + (id === null ? "/act" : "/item");
  const body = id === null ? { label: o.label } : { id, label: o.label };
  try {
    const r = await api(path, body);
    buzz();
    toast(r.said || "Done.");
  } catch (e) {
    toast(String(e.message || e), true);
  }
}

// A menu the computer has changed under: its item gone or its options not
// what they were, said and closed.
function staleMenu() {
  const m = state.menu;
  if (!m || !m.panel) return;
  const view = viewOf(m.view);
  const options = !view ? null : m.id === null ? view.acts : (itemOf(view, m.id) || {}).options;
  if (!options || optionsKey(options) !== m.key) {
    closeMenu();
    toast("That has changed since the menu opened.", true);
  }
}

// A swipe right on the panel, or from an open view's left edge (starting
// within `within` px of it; 0 for anywhere), follows the finger and, let go
// past a third of its width or flicked, puts it `away`, else lets it `stay`;
// a swipe left from the right edge, on a channel or the drawer, brings the
// panel in under the finger.
function panelSwipe(panel, within, away, stay) {
  let s = null;
  panel.addEventListener("touchstart", (e) => {
    if (e.touches.length !== 1) return (s = null);
    const t = e.touches[0];
    if (within && t.clientX > within) return (s = null);
    s = { x: t.clientX, y: t.clientY, dx: 0, at: e.timeStamp, speed: 0, decided: false };
  }, { passive: true });
  panel.addEventListener("touchmove", (e) => {
    if (!s || e.touches.length !== 1) return;
    const t = e.touches[0];
    const dx = t.clientX - s.x;
    const dy = t.clientY - s.y;
    if (!s.decided) {
      if (Math.max(Math.abs(dx), Math.abs(dy)) < SWIPE_DECIDES) return;
      if (dx <= Math.abs(dy)) return (s = null);
      s.decided = true;
    }
    e.preventDefault();
    const elapsed = e.timeStamp - s.at;
    if (elapsed > 0) s.speed = (dx - s.dx) / elapsed;
    s.at = e.timeStamp;
    s.dx = Math.max(0, dx);
    panel.style.transform = "translateX(" + s.dx + "px)";
  }, { passive: false });
  const end = (e) => {
    const done = s;
    s = null;
    if (!done || !done.decided || !panelLayer) return;
    const gone = e.type === "touchend" && (done.dx > panel.offsetWidth / 3 || (done.speed > SWIPE_FLICK && e.timeStamp - done.at < SWIPE_FLICK_MS));
    if (gone) away();
    else stay();
  };
  panel.addEventListener("touchend", end);
  panel.addEventListener("touchcancel", end);
}

let edgeFollowed = false;
function edgeSwipe() {
  if (edgeFollowed) return;
  edgeFollowed = true;
  document.addEventListener("touchstart", (e) => {
    edge = null;
    if (!state.views || panelLayer || state.menu || state.full || e.touches.length !== 1) return;
    if (e.target && e.target.closest && e.target.closest("textarea, .tray, .full")) return;
    const t = e.touches[0];
    if (t.clientX < window.innerWidth - EDGE) return;
    edge = { x: t.clientX, y: t.clientY, dx: 0, at: e.timeStamp, speed: 0, decided: false, opening: null };
  }, { passive: true });
  document.addEventListener("touchmove", (e) => {
    if (!edge || e.touches.length !== 1) return;
    const t = e.touches[0];
    const dx = t.clientX - edge.x;
    const dy = t.clientY - edge.y;
    if (!edge.decided) {
      if (Math.max(Math.abs(dx), Math.abs(dy)) < SWIPE_DECIDES) return;
      if (-dx <= Math.abs(dy)) return (edge = null);
      edge.decided = true;
      edge.opening = panelLayerNode();
      edge.opening.panel.style.transform = "translateX(100%)";
      edge.opening.strip.style.opacity = "0";
      state.panel = { view: null };
      panelLayer = edge.opening;
      drawPanel();
      document.body.append(edge.opening.layer);
    }
    e.preventDefault();
    const elapsed = e.timeStamp - edge.at;
    if (elapsed > 0) edge.speed = (dx - edge.dx) / elapsed;
    edge.at = e.timeStamp;
    edge.dx = Math.min(0, dx);
    const width = edge.opening.panel.offsetWidth || window.innerWidth;
    edge.opening.panel.style.transform = "translateX(" + Math.max(0, width + edge.dx) + "px)";
    edge.opening.strip.style.opacity = String(Math.min(1, -edge.dx / width));
  }, { passive: false });
  const end = (e) => {
    const done = edge;
    edge = null;
    if (!done || !done.decided) return;
    const width = done.opening.panel.offsetWidth || window.innerWidth;
    const opens = e.type === "touchend" && (-done.dx > width / 3 || (-done.speed > SWIPE_FLICK && e.timeStamp - done.at < SWIPE_FLICK_MS));
    if (opens) {
      buzz();
      openPanel(done.opening);
    } else {
      closePanel();
    }
  };
  document.addEventListener("touchend", end);
  document.addEventListener("touchcancel", end);
  // The phone's back with a view open goes back to the panel's list.
  window.addEventListener("popstate", () => {
    const then = unwinding;
    unwinding = null;
    if (then) then();
    else if (state.panel && state.panel.view) closeView();
  });
}

// The first time the app opens with the panel never opened, once.
function hintPanel() {
  if (!state.views || hinted()) return;
  hinted(true);
  toast("← Swipe from the right for mission control", false, true);
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
    // A new turn, or pictures Claude showed with `look`, which come apart from its words.
    const said = (t) => JSON.stringify([t.turn, t.looks || []]);
    const turned = before && after && said(before) !== said(after);
    if (after && after.prompt) state.answered = null;
    state.close = data.close !== false;
    state.haptics = data.haptics !== false;
    state.reminders = data.reminders !== false;
    state.remindTimes = data.remind_times || state.remindTimes;
    state.views = data.views || null;
    if (state.menu && state.menu.panel) staleMenu();
    else if (state.menu && !state.tabs.some((t) => number(t) === state.menu.pane)) closeMenu();
    if (!state.views) closePanel();
    render();
    drawPanel();
    if (turned) loadTurns();
    tabsRead();
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

// `posted` is the tap a push's message named ({ tab, at }), for when the
// worker keeps none.
async function reconnect(posted) {
  tabsFresh = false;
  wantTapped(posted);
  state.reconnectingUntil = Date.now() + RECONNECTING_MS;
  render();
  setTimeout(render, RECONNECTING_MS + 100);
  listen();
  askWorkerBuild().catch(() => {});
  readServerBuild();
  try {
    const r = await api("/api/tabs");
    state.tabs = r.tabs || [];
    state.views = r.views || null;
    state.live = !!r.live;
    if (state.live) state.lastHeard = Date.now();
    render();
    if (state.open) loadTurns();
    tabsRead();
  } catch (_) {
    // The stream says when the computer is back, and brings the tabs.
  }
}

// A push tapped while the page was frozen or closed: the service worker keeps
// its tab until the page says it took it, and the page asks for it as it
// starts or comes back, once it has read the tabs since (by asking, or from
// the stream), never against the tabs it held while it slept. One tapped over a minute ago
// is left, so coming back later stays where it was; one for a tab since
// closed shows the drawer.
const TAPPED_MS = 60000;
let tookTapped = 0; // when the tapped push the page opened last was tapped
let tabsFresh = false; // the tabs were read since the page started or came back
let pendingTap = null; // { posted }: a tapped push's tab to ask for once they are

function wantTapped(posted) {
  pendingTap = { posted: posted || (pendingTap && pendingTap.posted) };
  if (tabsFresh) tabsRead();
}

function tabsRead() {
  tabsFresh = true;
  if (!pendingTap) return;
  const { posted } = pendingTap;
  pendingTap = null;
  openTapped(posted);
}

// The worker's answer to `message`, on a port of its own; {} when it gives
// none in two seconds.
function askWorker(worker, message) {
  return new Promise((resolve) => {
    const channel = new MessageChannel();
    const done = (answer) => {
      clearTimeout(timer);
      channel.port1.close();
      resolve(answer || {});
    };
    const timer = setTimeout(done, 2000);
    channel.port1.onmessage = (event) => done(event.data);
    worker.postMessage(message, [channel.port2]);
  });
}

async function openTapped(posted) {
  const worker = navigator.serviceWorker && navigator.serviceWorker.controller;
  const kept = worker ? await askWorker(worker, { tapped: true }).catch(() => ({})) : {};
  // The tap a message named when the worker kept none (it couldn't, or an
  // older worker doesn't answer).
  const tapped = kept.tab ? kept : posted || {};
  if (!tapped.tab) return;
  // Taken off what the worker keeps only now, so an answer that came too late
  // leaves it for the next ask.
  if (worker) worker.postMessage({ took: tapped.at });
  const isFresh = Date.now() - tapped.at <= TAPPED_MS;
  const isOpenedAlready = tapped.at === tookTapped;
  if (!isFresh || isOpenedAlready) return;
  tookTapped = tapped.at;
  const tab = String(tapped.tab);
  if (state.tabs.some((t) => number(t) === tab)) {
    // A window opened for the push opened it from its address already.
    if (state.open !== tab) openTab(tab);
  } else {
    state.open = null;
    history.replaceState(null, "", "#");
    render();
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
    // Read the tabs again first, since a page that slept holds old ones; the
    // tab is taken from what the worker keeps, so it opens once.
    // The tap's time is the worker's, so it matches what the worker keeps;
    // an older worker's message has none.
    const data = event.data || {};
    if (data.open) reconnect({ tab: String(data.open), at: data.at || Date.now() });
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
    if (!d || d.contains(ask)) return;
    const line = d.querySelector(".build-line");
    if (line) d.insertBefore(ask, line);
    else d.append(ask);
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
  if (workerBuild === null) askWorkerBuild().catch(() => {});
  try {
    const res = await fetch("/api/me", { credentials: "same-origin", headers: buildHeader() });
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
  tabsFresh = false;
  const r = await api("/api/tabs").catch(() => null);
  const hasTabs = r !== null;
  state.tabs = (r && r.tabs) || [];
  state.live = !!(r && r.live);
  state.close = !r || r.close !== false;
  state.haptics = !r || r.haptics !== false;
  state.views = (r && r.views) || null;
  state.lastHeard = Date.now();
  const wanted = decodeURIComponent(location.hash.slice(1));
  if (wanted && state.tabs.some((t) => number(t) === wanted)) openTab(wanted);
  else render();
  listen();
  edgeSwipe();
  hintPanel();
  offerPush().catch(() => {});
  // Then a tapped push's tab, since a window opened for one may start at "/",
  // once the tabs are read.
  wantTapped();
  if (hasTabs) tabsRead();
}

// The keyboard open, marked on the page so the screen leaves out the
// home-indicator inset the keyboard covers: iOS keeps safe-area-inset-bottom
// the same either way, so it is told by the visual viewport, which the
// keyboard shortens, against the window, which it leaves alone.
function followKeyboard() {
  const keyboardOpen = Math.round(window.visualViewport.height) < window.innerHeight;
  document.documentElement.classList.toggle("keyboard", keyboardOpen);
}
if (window.visualViewport) {
  window.visualViewport.addEventListener("resize", followKeyboard);
  followKeyboard();
}

// Safari pinches to zoom with these whatever the viewport says.
for (const name of ["gesturestart", "gesturechange"]) {
  document.addEventListener(name, (event) => event.preventDefault(), { passive: false });
}

start();
