// Tests for tmux/settings.sh and lib/settings.js, the workspace's settings.
// Run with: node --test test/*.test.js
//
// A throwaway tmux server and a temporary settings file, so nothing reads or
// changes your tmux or your settings.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const repo = path.join(__dirname, "..");
const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "settings-")));
const socket = path.join(tmp, "tmux.sock");
const file = path.join(tmp, "settings.json");
execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "-s", "settings", "cat"]);
const serverPid = execFileSync("tmux", ["-S", socket, "display", "-p", "#{pid}"], { encoding: "utf8" }).trim();
const env = { ...process.env, HOME: tmp, TMUX: `${socket},${serverPid},0`, SWITCHBOARD_DIR: repo, SWITCHBOARD_SETTINGS: file };
after(() => {
  execFileSync("tmux", ["-S", socket, "kill-server"]);
  fs.rmSync(tmp, { recursive: true, force: true });
});

test("settings.sh sets every option from its default or your file, and set writes only what differs from the default", () => {
  const show = (option) => execFileSync("tmux", ["-S", socket, "show", "-gv", option], { encoding: "utf8" }).trim();
  const schema = JSON.parse(fs.readFileSync(path.join(repo, "tmux/settings.json"), "utf8"));

  // Every option has a category, a label, a description and a valid default.
  schema.forEach((o) => {
    assert.ok(o.key && o.category && o.label && o.description, o.key);
    assert.ok(o.choices ? o.choices.some((c) => c.value === o.default) : typeof o.default === "boolean", o.key);
  });

  // No file: the defaults. A value of the wrong kind is ignored.
  execFileSync("bash", [path.join(repo, "tmux/settings.sh"), "apply", "--load"], { env });
  assert.equal(show("@setting_notifications"), "on");
  assert.equal(show("@setting_update_check"), "hourly");
  assert.equal(show("status"), "on", "one status line unless the second is turned on");
  fs.writeFileSync(file, JSON.stringify({ second_status: false, save_contents: false, theme: "purple", notifications: "nope" }));
  execFileSync("bash", [path.join(repo, "tmux/settings.sh"), "apply", "--load"], { env });
  assert.equal(show("status"), "on");
  assert.equal(show("@resurrect-capture-pane-contents"), "off");
  assert.equal(show("@setting_theme"), "dark");
  assert.equal(show("@setting_notifications"), "on");

  // set keeps the file to what differs from the defaults, and applies it.
  execFileSync("bash", [path.join(repo, "tmux/settings.sh"), "set", "confirm_close", "off"], { env });
  assert.equal(show("@setting_confirm_close"), "off");
  execFileSync("bash", [path.join(repo, "tmux/settings.sh"), "set", "second_status", "on"], { env });
  assert.equal(show("status"), "2");
  assert.deepEqual(JSON.parse(fs.readFileSync(file, "utf8")), { save_contents: false, theme: "purple", notifications: "nope", confirm_close: false, second_status: true });
  assert.throws(() => execFileSync("bash", [path.join(repo, "tmux/settings.sh"), "set", "update_check", "weekly"], { env, stdio: "pipe" }));

  // lib/settings.js reads the same file the same way.
  const read = JSON.parse(execFileSync(process.execPath, ["-e", "console.log(JSON.stringify(require(process.argv[1]).readSettings().map((o) => [o.key, o.value])))", path.join(repo, "lib/settings.js")], { env, encoding: "utf8" }));
  assert.deepEqual(Object.fromEntries(read).confirm_close, false);
  assert.deepEqual(Object.fromEntries(read).theme, "dark");
  assert.deepEqual(Object.fromEntries(read).notifications, true);
});

test("with the github setting off, keys tagged #github only say why, once, and leave the palette, and #tracker ones are kept", () => {
  const tmux = (...args) => execFileSync("tmux", ["-S", socket, ...args], { encoding: "utf8" }).trim();
  fs.writeFileSync(file, "{}");
  tmux("bind", "-N", "View the PR #github #pr", "-n", "M-v", "display-message", "pr");
  tmux("bind", "-N", "View the ticket #tracker #ticket", "-n", "M-i", "display-message", "ticket");
  tmux("bind", "-T", "palette", "-N", "Copy the PR number #github #copy", "p", "display-message", "copied");
  execFileSync("bash", [path.join(repo, "tmux/settings.sh"), "apply", "--load"], { env });
  assert.match(tmux("list-keys", "-T", "root"), /M-v +display-message pr/);

  execFileSync("bash", [path.join(repo, "tmux/settings.sh"), "set", "github", "off"], { env, stdio: "ignore" });
  assert.match(tmux("list-keys", "-T", "root"), /M-v +if-shell -F "#\{@tip_seen_github_off\}" '' "run-shell -b '~\/\.config\/tmux\/tip\.sh show github_off/);
  assert.match(tmux("list-keys", "-T", "palette"), / p +if-shell -F "#\{@tip_seen_github_off\}"/);
  assert.doesNotMatch(tmux("list-keys", "-N", "-T", "root"), /#github/);
  assert.match(tmux("list-keys", "-T", "root"), /M-i +display-message ticket/);
});
