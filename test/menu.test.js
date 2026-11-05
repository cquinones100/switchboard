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
