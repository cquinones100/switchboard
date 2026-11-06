// Tests for tmux/menu.sh, what the bash menus in popups share to take keys
// and clicks. Run with: node --test test/*.test.js

const { test } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const path = require("node:path");

const read = (input) =>
  execFileSync("bash", ["-c", `. ${JSON.stringify(path.join(__dirname, "../tmux/menu.sh"))}; while menu_read; do printf '%s|' "$MENU_KEY"; done`], { input, encoding: "utf8", env: { ...process.env, TMUX: "" } });

test("menu_read says what was pressed or clicked: keys, arrows, the wheel, a left click's row and column", () => {
  assert.equal(read("jk\n"), "j|k|enter|");
  assert.equal(read("\x1b[B\x1b[A"), "j|k|");
  assert.equal(read("\x1b[<0;12;7M\x1b[<0;12;7m"), "click 7 12|none|", "a click, and its release, which means nothing");
  assert.equal(read("\x1b[<64;3;3M\x1b[<65;3;3M"), "k|j|", "the wheel moves");
  assert.equal(read("q"), "quit|");
  assert.equal(read("\x1b"), "quit|", "Esc alone");
});

test("menu_read reads Tab and Shift-Tab as tab and btab, which go round a menu's options", () => {
  assert.equal(read("\t\x1b[Z "), "tab|btab|space|");
});

test("in easy mode, menu_read draws the close button at the top row's right end before each read, and a click on it is quit", () => {
  const out = execFileSync("bash", ["-c", `. ${JSON.stringify(path.join(__dirname, "../tmux/menu.sh"))}; MENU_CLOSE=77; while menu_read; do printf '%s|' "$MENU_KEY"; done`], {
    input: "\x1b[<0;77;1M\x1b[<0;79;1M\x1b[<0;80;1M\x1b[<0;78;2M",
    encoding: "utf8",
    env: { ...process.env, TMUX: "" },
  });
  assert.match(out, /\x1b\[1;77H.* × /, "drawn at the column MENU_CLOSE names, on the top row");
  assert.equal(out.replace(/\x1b7.*?\x1b8/g, ""), "quit|quit|click 1 80|click 2 78|", "its three cells close; the edge and the row under it don't");
});

test("menu_stops counts the close button as a Tab stop after a menu's options only while it is drawn", () => {
  const stops = (close) => execFileSync("bash", ["-c", `. ${JSON.stringify(path.join(__dirname, "../tmux/menu.sh"))}; MENU_CLOSE=${close}; menu_stops 3`], { encoding: "utf8" }).trim();
  assert.equal(stops(""), "3");
  assert.equal(stops("77"), "4");
});

test("menu_any_key closes on one key or a whole click, reading what else it sent, and says when the input has ended", () => {
  const out = execFileSync("bash", ["-c", `. ${JSON.stringify(path.join(__dirname, "../tmux/menu.sh"))}; menu_any_key && printf 'closed|'; IFS= read -rsn 1 next; printf '%s|' "$next"; menu_any_key || printf 'ended'`], {
    input: "\x1b[<0;77;1M",
    encoding: "utf8",
    env: { ...process.env, TMUX: "" },
  });
  assert.equal(out.replace(/\x1b\[\?100[06][hl]/g, ""), "closed||ended", "the click's bytes are all read, none left for what follows");
});
