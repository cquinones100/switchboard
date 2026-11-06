// Tests for tmux/open-url.sh, which every link Switchboard opens goes
// through, and the Settings view's Default Chrome profile row
// (lib/sidebar/own-config.js).
// Run with: node --test test/*.test.js
//
// A throwaway tmux server, a temporary HOME and an `open` on the PATH that
// only writes down what it was given, so nothing opens a browser or reads
// your browser.json.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const repo = path.join(__dirname, "..");
const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "open-url-")));
const socket = path.join(tmp, "tmux.sock");
const bin = path.join(tmp, "bin");
const opened = path.join(tmp, "opened");
fs.mkdirSync(bin);
fs.writeFileSync(path.join(bin, "open"), `#!/bin/sh\nprintf '%s\\n' "$*" >> ${JSON.stringify(opened)}\n`, { mode: 0o755 });
fs.mkdirSync(path.join(tmp, ".config/switchboard"), { recursive: true });
execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "-s", "open-url", "cat"]);
const serverPid = execFileSync("tmux", ["-S", socket, "display", "-p", "#{pid}"], { encoding: "utf8" }).trim();
const env = { ...process.env, HOME: tmp, PATH: `${bin}:${process.env.PATH}`, TMUX: `${socket},${serverPid},0` };
const home = process.env.HOME;
after(() => {
  process.env.HOME = home;
  execFileSync("tmux", ["-S", socket, "kill-server"]);
  fs.rmSync(tmp, { recursive: true, force: true });
});

const browser = (data) => fs.writeFileSync(path.join(tmp, ".config/switchboard/browser.json"), JSON.stringify(data));
const openUrl = (url) => {
  fs.rmSync(opened, { force: true });
  execFileSync("bash", [path.join(repo, "tmux/open-url.sh"), url], { env });
  return fs.readFileSync(opened, "utf8").trim();
};

test("open-url.sh opens a link in its site's profile, else the default profile, else the default browser", () => {
  browser({ hosts: { "linear.app": "Profile 2" }, default: "Profile 1" });
  assert.equal(openUrl("https://linear.app/x/issue/ABC-1"), "-na Google Chrome --args --profile-directory=Profile 2 https://linear.app/x/issue/ABC-1");
  assert.equal(openUrl("https://github.com/o/r/pull/12345"), "-na Google Chrome --args --profile-directory=Profile 1 https://github.com/o/r/pull/12345");

  browser({ hosts: { "linear.app": "Profile 2" } });
  assert.equal(openUrl("https://github.com/o/r"), "https://github.com/o/r");

  // The setting off: every link in the default browser.
  browser({ hosts: { "linear.app": "Profile 2" }, default: "Profile 1" });
  execFileSync("tmux", ["-S", socket, "set", "-g", "@setting_chrome_profiles", "off"]);
  assert.equal(openUrl("https://linear.app/x"), "https://linear.app/x");
  execFileSync("tmux", ["-S", socket, "set", "-gu", "@setting_chrome_profiles"]);

  // Outside tmux, from the settings file.
  const settings = path.join(tmp, "settings.json");
  fs.writeFileSync(settings, JSON.stringify({ chrome_profiles: false }));
  const outside = { ...env, TMUX: "", TMUX_TMPDIR: tmp, SWITCHBOARD_SETTINGS: settings };
  fs.rmSync(opened, { force: true });
  execFileSync("bash", [path.join(repo, "tmux/open-url.sh"), "https://linear.app/x"], { env: outside });
  assert.equal(fs.readFileSync(opened, "utf8").trim(), "https://linear.app/x");
});

test("the Default Chrome profile row lists Chrome's profiles and writes the one picked", () => {
  process.env.HOME = tmp;
  const { ownItems, writeOwn, chromeProfiles } = require("../lib/sidebar/own-config");
  const localState = path.join(tmp, "Library/Application Support/Google/Chrome/Local State");
  assert.equal(chromeProfiles(), undefined, "no Chrome, no profiles");

  fs.mkdirSync(path.dirname(localState), { recursive: true });
  fs.writeFileSync(localState, JSON.stringify({ profile: { info_cache: { "Profile 10": { name: "Ten" }, "Profile 2": { name: "Work" }, Default: { name: "Person 1" } } } }));
  assert.deepEqual(chromeProfiles(), [
    { dir: "Default", name: "Person 1" },
    { dir: "Profile 2", name: "Work" },
    { dir: "Profile 10", name: "Ten" },
  ]);

  browser({ hosts: { "linear.app": "Profile 2" } });
  const row = () => ownItems().find((item) => item.setting === "own:browser.json:default");
  assert.equal(row().value, "not set");
  assert.equal(row().own.kind, "profile");

  assert.equal(writeOwn("browser.json", "default", "profile", "Profile 2"), undefined);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(tmp, ".config/switchboard/browser.json"), "utf8")), { hosts: { "linear.app": "Profile 2" }, default: "Profile 2" });
  assert.equal(row().value, "Work", "shown by the name Chrome gives it");

  writeOwn("browser.json", "default", "profile", "");
  assert.equal(row().value, "not set");
});
