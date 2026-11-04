// Tests for tmux/window-options.sh, which carries a tab's reminder across a
// restart that tmux-resurrect brings the tabs back from. Run with:
// node --test test/*.test.js
//
// It runs against throwaway tmux servers and a temporary resurrect directory,
// so it never reads your tmux or your saves.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "winopts-")));
const data = path.join(tmp, "data");
const resurrect = path.join(data, "tmux/resurrect");
fs.mkdirSync(resurrect, { recursive: true });
const socket = path.join(tmp, "tmux.sock");
const tmux = (...args) => execFileSync("tmux", ["-S", socket, ...args], { encoding: "utf8" }).trimEnd();
after(() => {
  try {
    tmux("kill-server");
  } catch {}
  fs.rmSync(tmp, { recursive: true, force: true });
});

const run = (action) =>
  execFileSync("bash", [path.join(__dirname, "../tmux/window-options.sh"), action], {
    encoding: "utf8",
    env: { ...process.env, HOME: tmp, XDG_DATA_HOME: data, TMUX: `${socket},0,0` },
  });
// A server laid out as resurrect lays one out: session "work", windows 1 to 3.
const start = () => {
  tmux("-f", "/dev/null", "new-session", "-d", "-s", "work", "cat");
  tmux("set", "-g", "base-index", "1");
  tmux("move-window", "-s", "work:0", "-t", "work:1");
  tmux("new-window", "-d", "-t", "work:2", "cat");
  tmux("new-window", "-d", "-t", "work:3", "cat");
};
const saveAs = (name) => {
  fs.writeFileSync(path.join(resurrect, name), "");
  fs.rmSync(path.join(resurrect, "last"), { force: true });
  fs.symlinkSync(name, path.join(resurrect, "last"));
};

test("a tab's reminder comes back after a restart, on the same tab, and only from the save restored", () => {
  start();
  tmux("set", "-w", "-t", "work:2", "@remind_at", "1800000000");
  tmux("set", "-w", "-t", "work:2", "@remind_note", "check the deploy again");
  tmux("set", "-w", "-t", "work:3", "@remind_at", "1800000100");
  tmux("set", "-w", "-t", "work:3", "@remind_said", "1800000100");
  saveAs("tmux_resurrect_20261104T170000.txt");
  run("save");
  const saved = fs.readFileSync(path.join(resurrect, "window-options.tsv"), "utf8").split("\n").filter(Boolean);
  assert.equal(saved[0], "tmux_resurrect_20261104T170000.txt");
  assert.equal(saved.length, 3, "only the windows with a reminder");
  assert.equal(fs.statSync(path.join(resurrect, "window-options.tsv")).mode & 0o777, 0o600, "owner-only");

  // The restart: a new server with the same tabs, one holding a note set since.
  tmux("kill-server");
  start();
  tmux("set", "-w", "-t", "work:3", "@remind_note", "set after the save");
  run("restore");
  assert.equal(tmux("show", "-wqv", "-t", "work:3", "@remind_note"), "", "an option saved empty is unset");
  const option = (window, name) => tmux("show", "-wqv", "-t", window, name);
  assert.equal(option("work:2", "@remind_at"), "1800000000");
  assert.equal(option("work:2", "@remind_note"), "check the deploy again");
  assert.equal(option("work:2", "@remind_said"), "", "nothing set that wasn't");
  assert.equal(option("work:3", "@remind_at"), "1800000100", "an empty note between keeps the columns in place");
  assert.equal(option("work:3", "@remind_said"), "1800000100");
  assert.equal(option("work:1", "@remind_at"), "");

  // Restored from another save than the one the file was written for: left alone.
  tmux("kill-server");
  start();
  saveAs("tmux_resurrect_20261104T180000.txt");
  run("restore");
  assert.equal(option("work:2", "@remind_at"), "", "a file from another save isn't applied");

  // Resurrect's folder named by its @resurrect-dir, as resurrect reads it.
  const custom = path.join(tmp, "custom");
  fs.mkdirSync(custom);
  fs.symlinkSync("tmux_resurrect_20261104T190000.txt", path.join(custom, "last"));
  tmux("set", "-g", "@resurrect-dir", "$HOME/custom");
  tmux("set", "-w", "-t", "work:2", "@remind_at", "1800000200");
  run("save");
  assert.match(fs.readFileSync(path.join(custom, "window-options.tsv"), "utf8"), /^tmux_resurrect_20261104T190000\.txt\nwork\t2\t1800000200/, "kept in the folder resurrect saves to");
});
