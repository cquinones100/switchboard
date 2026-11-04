// Tests for tmux/new-tab.zsh, M-t's new tab, run with: node --test test/*.test.js
//
// The script runs from a copy beside stand-ins for the scripts it calls (the
// directory picker, the tab naming, Notes' move beside), against a throwaway
// tmux server with a temporary HOME, and a stand-in claude that keeps the
// arguments it was started with, so nothing reaches your tmux or Claude.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync, spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "new-tab-")));
const socket = path.join(tmp, "tmux.sock");
const bin = path.join(tmp, "bin");
const scripts = path.join(tmp, "scripts");
const work = path.join(tmp, "work");
[bin, scripts, work, path.join(tmp, "Notes")].forEach((dir) => fs.mkdirSync(dir, { recursive: true }));
const env = { ...process.env, HOME: tmp, NOTES_DIR: path.join(tmp, "Notes"), PATH: `${bin}:${process.env.PATH}`, TMUX: "" };
execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "-s", "t", "-x", "120", "-y", "30", "cat"], { env });
after(() => {
  spawnSync("tmux", ["-S", socket, "kill-server"], { env });
  fs.rmSync(tmp, { recursive: true, force: true });
});

fs.copyFileSync(path.join(__dirname, "../tmux/new-tab.zsh"), path.join(scripts, "new-tab.zsh"));
fs.chmodSync(path.join(scripts, "new-tab.zsh"), 0o755);
fs.writeFileSync(path.join(scripts, "pick-dir.zsh"), `#!/bin/sh\necho '${work}'\n`, { mode: 0o755 });
["window-names.sh", "note-tab.sh", "tab-name.zsh"].forEach((name) => fs.writeFileSync(path.join(scripts, name), "#!/bin/sh\nexit 0\n", { mode: 0o755 }));
// A stand-in claude that keeps the arguments it was started with, in a file
// named for how it was started: note.json with a note's system prompt (off.json
// for Plan.md's), prompt.json with a prompt, plain.json with neither.
fs.writeFileSync(path.join(bin, "claude"), `#!/usr/bin/env node\nrequire("fs").writeFileSync(require("path").join(${JSON.stringify(tmp)}, process.argv.length > 2 && process.argv[2] === "--append-system-prompt" ? (process.argv[3].includes("Plan.md") ? "off.json" : "note.json") : process.argv[2] ? "prompt.json" : "plain.json"), JSON.stringify(process.argv.slice(2)))\n`, { mode: 0o755 });

test("a note's new tab starts Claude knowing its note, added to the system prompt", () => {
  const ran = spawnSync("zsh", [path.join(scripts, "new-tab.zsh")], { env: { ...env, TMUX: `${socket},1,0`, NEW_TAB_NOTE: "Work/Plan $(x).md", NEW_TAB_NAME: "Plan $(x)" }, encoding: "utf8" });
  assert.equal(ran.status, 0, ran.stderr);
  const file = path.join(tmp, "note.json");
  const deadline = Date.now() + 20_000;
  while (!fs.existsSync(file) && Date.now() < deadline) spawnSync("sleep", ["0.1"]);
  const args = JSON.parse(fs.readFileSync(file, "utf8"));
  assert.equal(args[0], "--append-system-prompt");
  assert.match(args[1], /This tab is for the note "Plan \$\(x\)" in the user's notes app, the file ~\/Notes\/Work\/Plan \$\(x\)\.md/, "its title and file, as written");
  assert.match(args[1], /read it only when they ask you to/);
  assert.match(args[1], /@tab_note/, "and where to find it once renamed");
  assert.equal(args.length, 2, "and no prompt, so no turn is spent");
});

test("a tab with a prompt of its own still gets it, and a plain tab starts Claude bare", () => {
  const prompted = spawnSync("zsh", [path.join(scripts, "new-tab.zsh")], { env: { ...env, TMUX: `${socket},1,0`, NEW_TAB_PROMPT: "Read this thread" }, encoding: "utf8" });
  assert.equal(prompted.status, 0, prompted.stderr);
  const plain = spawnSync("zsh", [path.join(scripts, "new-tab.zsh")], { env: { ...env, TMUX: `${socket},1,0` }, encoding: "utf8" });
  assert.equal(plain.status, 0, plain.stderr);
  const deadline = Date.now() + 20_000;
  while (!(fs.existsSync(path.join(tmp, "prompt.json")) && fs.existsSync(path.join(tmp, "plain.json"))) && Date.now() < deadline) spawnSync("sleep", ["0.1"]);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(tmp, "prompt.json"), "utf8")), ["Read this thread"]);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(tmp, "plain.json"), "utf8")), []);
});

test("with the note_tab_prompt setting off, a note's tab starts Claude bare", () => {
  spawnSync("tmux", ["-S", socket, "set", "-g", "@setting_note_tab_prompt", "off"], { env });
  fs.rmSync(path.join(tmp, "plain.json"), { force: true });
  const ran = spawnSync("zsh", [path.join(scripts, "new-tab.zsh")], { env: { ...env, TMUX: `${socket},1,0`, NEW_TAB_NOTE: "Plan.md", NEW_TAB_NAME: "Plan" }, encoding: "utf8" });
  assert.equal(ran.status, 0, ran.stderr);
  const deadline = Date.now() + 20_000;
  while (!fs.existsSync(path.join(tmp, "plain.json")) && Date.now() < deadline) spawnSync("sleep", ["0.1"]);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(tmp, "plain.json"), "utf8")), [], "started bare");
  assert.equal(fs.existsSync(path.join(tmp, "off.json")), false, "no system prompt added");
  spawnSync("tmux", ["-S", socket, "set", "-gu", "@setting_note_tab_prompt"], { env });
});
