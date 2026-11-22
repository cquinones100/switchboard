// The phone app (src/phone), boards 164 to 169 and 173 to 176: the drawer of Claude
// tabs, Needs you first, with its search; a tab's channel of recent turns
// with the prompt it waits on as a card of buttons; the composer; pairing.
// Everything shown comes from the computer's own answers and is put on the
// page as text, never as HTML: Claude's replies are Markdown, parsed by the
// vendored markdown-it (raw HTML off) and built into elements from its
// tokens (`markdown`), so nothing in a reply is read as HTML. Images from the photo library (board 174)
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
// where the browser has it, as Android's does; else, where inputs take
// `switch` (Safari, whose iOS 18 and later answer a switch toggled during a
// tap with a haptic tick), a click on a hidden switch's label; else nothing.
let haptic = null; // the hidden switch's label, made on the first buzz that needs it

function buzz() {
  if (!state.haptics) return;
  if (typeof navigator.vibrate === "function") {
    navigator.vibrate(10);
    return;
  }
  const hasSwitch = typeof HTMLInputElement !== "undefined" && "switch" in HTMLInputElement.prototype;
  if (!hasSwitch) return;
  if (!haptic) {
    const input = el("input", { type: "checkbox", switch: true, tabindex: "-1", "aria-hidden": "true" });
    haptic = el("label", { class: "haptic", "aria-hidden": "true" }, input);
    document.body.append(haptic);
  }
  haptic.click();
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

function toast(text, failed) {
  const t = el("div", { class: "toast" }, failed ? el("span", { class: "bad" }, "✕") : el("span", { class: "ok" }, "✓"), text);
  document.body.append(t);
  setTimeout(() => t.remove(), 3000);
}

// ── The drawer (board 164) and its search (board 173) ──

// The words typed in the search, case aside.
const wordsOf = (q) => q.toLowerCase().split(/\s+/).filter(Boolean);
const holds = (text, words) => words.every((w) => String(text || "").toLowerCase().includes(w));
// Whether a tab's number, name, repository, branch or what it is doing hold
// every word typed, as the sidebar's / matches.
const matches = (tab, words) => holds([tab.index, tab.name, tab.repo, tab.branch, doing(tab), reminded(tab) || ""].join(" "), words);

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
  if (!words.length || holds([tab.index, tab.name, doing(tab), reminded(tab) || ""].join(" "), words)) return null;
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
        held(
          el(
            "button",
            { class: "row" + (state.menu && state.menu.pane === number(t) ? " held" : ""), "aria-haspopup": "menu", "data-pane": number(t) },
            el("span", { class: "mark " + kind }, m),
            el("span", { class: "name" }, el("span", { class: "num " + hue(t) }, lit(String(t.index), words)), lit(t.name || "", words)),
            el("span", { class: "sub" }, lit(doing(t), words)),
            reminded(t) ? el("span", { class: "sub remind" }, el("span", { class: "clock", "aria-label": "Reminder" }, "⏰"), " ", lit(reminded(t), words)) : null,
            where(t, words),
          ),
          t,
        ),
      );
    }
    list.append(g);
  }
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
  if (menuLayer) menuLayer.remove();
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
  drawer();
  document.addEventListener("keydown", menuKeys);
  showMenu(tab);
}

function layer(box) {
  if (menuLayer) menuLayer.remove();
  menuLayer = el("div", { class: "veil", onclick: (e) => { if (e.target === menuLayer) closeMenu(); } }, box);
  document.body.append(menuLayer);
}

function boxHead(title) {
  return el("div", { class: "box-head" }, el("span", { class: "box-title" }, title), el("button", { class: "box-close", "aria-label": "Close", onclick: () => closeMenu() }, "×"));
}

// The kit's box under the row: Mark seen on a finished tab, Clear the
// reminder on a tab with one, Remind me (board 176), then Close tab in the
// failed tone, as it ends the tab.
function showMenu(tab) {
  const n = number(tab);
  const options = [];
  if (tab.state === "done") options.push(el("button", { class: "menu-option", role: "menuitem", onclick: () => markSeen(n) }, "Mark seen"));
  if (state.reminders && tab.remind) options.push(el("button", { class: "menu-option", role: "menuitem", onclick: () => clearReminder(n) }, "Clear the reminder"));
  if (state.reminders) options.push(el("button", { class: "menu-option", role: "menuitem", onclick: () => remindSheet(n) }, "Remind me"));
  if (state.close) options.push(el("button", { class: "menu-option r", role: "menuitem", onclick: () => ask(n) }, "Close tab"));
  if (!options.length) return closeMenu();
  const box = el("div", { class: "box", role: "menu", "aria-label": "Tab " + tab.index + " " + (tab.name || "") }, boxHead("Tab " + tab.index + " · " + (tab.name || "")), options);
  layer(box);
  const row = app.querySelector && app.querySelector('.row[data-pane="' + n + '"]');
  if (row && row.getBoundingClientRect) {
    const r = row.getBoundingClientRect();
    const below = r.bottom + 4;
    const tall = 48 + options.length * 44;
    box.style.top = below + tall > window.innerHeight ? Math.max(8, r.top - tall) + "px" : below + "px";
  }
  if (options[0].focus) options[0].focus();
}

// The question Close tab asks first, as every yes/no question asks: [Y/n]
// after it, Close (the yes) first and lit; only Close closes the tab.
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
    el("div", { class: "answers" }, yes, el("button", { class: "option", onclick: () => closeMenu() }, "No")),
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
  state.open = n;
  state.messages = [];
  state.answered = null;
  state.error = "";
  state.unfolded = new Set();
  state.turnKeys = [];
  while (state.chosen.length) drop(0);
  state.full = null;
  state.whole = false;
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

// markdown-it (assets/phone/markdown-it.min.js, loaded before this file),
// with raw HTML off, so `<img onerror=…>` stays text, and bare http and https
// addresses made links; null where it isn't loaded, which leaves replies as text.
const md = typeof markdownit === "function" ? markdownit({ html: false, linkify: true }) : null;
// Only an address written with its scheme is made a link, never a file name
// such as README.md, which linkify-it would read as a site.
if (md) md.linkify.set({ fuzzyLink: false, fuzzyEmail: false });

// The page's elements for a reply's Markdown, built from markdown-it's
// tokens with `el`, every piece of text a text node: a table in a box that
// scrolls sideways, and a link only for http and https, opening in the
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
// its steps after the row.
// What starts a turn: what you said, or what another tab's Claude sent,
// both shown; or a subagent's report back (`turn`), which starts one
// but isn't drawn.
const starts = (m) => m.who === "you" || m.who === "tab" || m.who === "turn";

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
    const unanswered = working && g === groups.length - 1;
    const steps = unanswered ? rest : rest.slice(0, last + 1);
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
    return [...said, row, ...(open ? steps : looks), ...rest.slice(steps.length)];
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
  const folded = (lines > 10 || shown.length > 700) && !state.unfolded.has(key);
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
    same ? el("span") : el("span", { class: "avatar " + speaker(m) }, initial),
    same ? null : el("div", {}, name, when),
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
    el("img", { src: imageUrl(state.full), alt: "The image, whole" }),
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
  const send = el("button", { class: "send", "aria-label": "Send" }, "➤");
  // The photo library alone: images, several at once, no camera.
  const kinds = ["image/png", "image/jpeg", "image/gif", "image/webp"];
  const picker = el("input", { type: "file", accept: kinds.join(","), multiple: true, hidden: true });
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
    el("button", { class: "menu", "aria-label": "Tabs", onclick: toDrawer }, "☰"),
    el("span", { class: "mark " + kind }, m),
    title(tab),
    el("span", { class: "chip " + chip[1] }, chip[0]),
  );
}

// The open tab's screen, kept while it is open: a redraw puts in a new head,
// prompt card, "doing" line and image shown whole, the messages only when
// the turns changed, and leaves the composer where it is, since taking a
// focused field out of the page closes the keyboard.
let shown = null; // { open, screen, head, list, messages, drawn, count, working, now, composer, full }

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
    shown.messages.replaceChildren(...turns(state.messages, answering).map(message));
    shown.drawn = state.messages;
    shown.count = state.messages.length;
    shown.working = answering;
  } else {
    // Kept, so their times ("now", "3m ago") are worked out again in place.
    for (const when of shown.messages.querySelectorAll(".when")) when.textContent = ago(when.at);
  }
  shown.now.replaceChildren(...[card(tab), working ? el("div", { class: "doing" }, "⠹ " + doing(tab)) : null].filter(Boolean));
  shown.composer.sync(tab);
  const whole = full() || el("div");
  shown.full.replaceWith(whole);
  shown.full = whole;
  shown.list.scrollTop = stick || atEnd ? shown.list.scrollHeight : top;
}

// Swiping right in a channel (board 164's second phone): the drawer drawn
// under the channel once the touch moves right more than up or down, the
// channel following the finger. Let go past a third of the screen's width,
// or flicked right, it opens the drawer as ☰ does; otherwise it slides back.
// A touch in the composer's field, its tray of images, a reply's table or
// code block or an image shown whole is theirs.
const SWIPE_DECIDES = 10; // px moved before a touch counts as sideways or not
const SWIPE_FLICK = 0.5; // px a millisecond rightward that open the drawer however far it went
const SWIPE_MS = 200;
const SWIPE_FLICK_MS = 100; // a flick lets go within this of its last move
let swipe = null; // { screen, x, y, dx, at, speed, under, wanted, buzzed, sliding }: the touch under way, or the slide after it

function swipeable(screen) {
  screen.addEventListener("touchstart", (e) => {
    if (swipe && swipe.sliding) return;
    // A second finger mid-swipe sends the channel back, as a cancel does.
    if (swipe && swipe.under) return end({ type: "touchcancel" });
    swipe = null;
    if (e.touches.length !== 1 || e.target.closest("textarea, .tray, pre, .table, .full")) return;
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
    screen.style.transition = "transform " + SWIPE_MS + "ms ease-out";
    screen.style.transform = opens ? "translateX(100%)" : "";
    // Held off until the slide ends, so no redraw lands mid-slide.
    swipe = { screen, under: done.under, wanted: done.wanted, sliding: true };
    setTimeout(() => {
      const wanted = swipe.wanted;
      swipe = null;
      app.classList.remove("swiping");
      screen.style.transition = "";
      screen.style.transform = "";
      if (opens) {
        screen.remove();
        toDrawer();
        return;
      }
      done.under.remove();
      if (wanted) render(wanted.stick);
    }, SWIPE_MS);
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
    if (state.menu && !state.tabs.some((t) => number(t) === state.menu.pane)) closeMenu();
    render();
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
  try {
    const r = await api("/api/tabs");
    state.tabs = r.tabs || [];
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

function askTapped(worker) {
  return new Promise((resolve) => {
    const channel = new MessageChannel();
    const done = (answer) => {
      clearTimeout(timer);
      channel.port1.close();
      resolve(answer || {});
    };
    const timer = setTimeout(done, 2000);
    channel.port1.onmessage = (event) => done(event.data);
    worker.postMessage({ tapped: true }, [channel.port2]);
  });
}

async function openTapped(posted) {
  const worker = navigator.serviceWorker && navigator.serviceWorker.controller;
  const kept = worker ? await askTapped(worker).catch(() => ({})) : {};
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
  tabsFresh = false;
  const r = await api("/api/tabs").catch(() => null);
  const hasTabs = r !== null;
  state.tabs = (r && r.tabs) || [];
  state.live = !!(r && r.live);
  state.close = !r || r.close !== false;
  state.haptics = !r || r.haptics !== false;
  state.lastHeard = Date.now();
  const wanted = decodeURIComponent(location.hash.slice(1));
  if (wanted && state.tabs.some((t) => number(t) === wanted)) openTab(wanted);
  else render();
  listen();
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
