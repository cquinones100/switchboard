// The phone preview's frame (src/phone/preview.rs): the app in an iframe at
// a phone's size, its safe-area insets set as the variables the preview's
// app.css reads in place of env(), a keyboard that shortens it, buttons
// that change the made-up tabs, and the app reloaded, on the same tab, as
// its files are saved.
"use strict";

// Each phone's screen in CSS pixels, its insets with the keyboard down, and
// its keyboard's height.
const PHONES = [
  { name: "iPhone SE", width: 375, height: 667, top: 20, bottom: 0, keys: 260, island: false },
  { name: "iPhone 16", width: 393, height: 852, top: 59, bottom: 34, keys: 336, island: true },
  { name: "iPhone 16 Pro Max", width: 440, height: 956, top: 62, bottom: 34, keys: 346, island: true },
];

const size = document.getElementById("size");
const keyboard = document.getElementById("keyboard");
const phone = document.getElementById("phone");
const frame = document.getElementById("app");
const keys = document.getElementById("keys");
const saidLine = document.getElementById("said");

// What this viewer chose last, a per-viewer convenience.
const chosen = JSON.parse(localStorage.getItem("phone-preview") || "{}");
PHONES.forEach((p, i) => size.append(new Option(p.name, String(i))));
size.value = String(PHONES.findIndex((p) => p.name === chosen.phone) >= 0 ? PHONES.findIndex((p) => p.name === chosen.phone) : 1);
keyboard.checked = !!chosen.keyboard;

const current = () => PHONES[Number(size.value)];

// The insets on the app's page: the keyboard covers the home indicator, so
// with it open the bottom inset is 0, as on a phone.
function insets() {
  const p = current();
  const doc = frame.contentDocument;
  if (!doc || !doc.documentElement) return;
  const root = doc.documentElement.style;
  root.setProperty("--safe-area-inset-top", p.top + "px");
  root.setProperty("--safe-area-inset-bottom", (keyboard.checked ? 0 : p.bottom) + "px");
  root.setProperty("--safe-area-inset-left", "0px");
  root.setProperty("--safe-area-inset-right", "0px");
}

function lay() {
  const p = current();
  phone.style.width = p.width + "px";
  phone.style.height = p.height + "px";
  frame.style.height = (keyboard.checked ? p.height - p.keys : p.height) + "px";
  keys.style.height = p.keys + "px";
  keys.hidden = !keyboard.checked;
  phone.classList.toggle("with-island", p.island);
  phone.classList.toggle("keyboard-open", keyboard.checked);
  phone.classList.toggle("with-indicator", p.bottom > 0);
  insets();
  localStorage.setItem("phone-preview", JSON.stringify({ phone: p.name, keyboard: keyboard.checked }));
}

size.addEventListener("change", lay);
keyboard.addEventListener("change", lay);
frame.addEventListener("load", insets);
lay();

function say(text, failed) {
  saidLine.textContent = text;
  saidLine.classList.toggle("bad", !!failed);
}

for (const button of document.querySelectorAll("button[data-do]")) {
  button.addEventListener("click", async () => {
    try {
      const res = await fetch("/preview/" + button.dataset.do, { method: "POST" });
      const data = await res.json();
      say(data.said || data.error, !res.ok);
    } catch (e) {
      say("The preview isn't running", true);
    }
  });
}

// A saved file reloads the app where it is: its #<pane> keeps the tab open.
const reloads = new EventSource("/preview/reload");
reloads.onmessage = (event) => {
  frame.contentWindow.location.reload();
  say("Reloaded after " + (event.data || "a change"));
};
