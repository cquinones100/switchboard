// Tests for tmux/continuum-save.sh, which puts tmux-continuum's autosave back
// in status-right (the health light's part in it is in health.test.js). Run with:
// node --test test/*.test.js
//
// A throwaway tmux server and a temporary HOME holding a stand-in for
// continuum's save script, so nothing reaches your tmux or your saved tabs.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync, spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "csave-")));
const socket = path.join(tmp, "tmux.sock");
const plugin = path.join(tmp, ".config/tmux/plugins/tmux-continuum");
const saveScript = path.join(plugin, "scripts/continuum_save.sh");
const job = `#(${saveScript})`;
fs.mkdirSync(path.dirname(saveScript), { recursive: true });
fs.writeFileSync(saveScript, "#!/bin/sh\n", { mode: 0o755 });

const env = { ...process.env, HOME: tmp, TMUX: `${socket},0,0` };
delete env.SWITCHBOARD_TRY_OF;
execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "-s", "csave", "cat"], { env: { ...env, TMUX: "" } });
const tmux = (...args) => spawnSync("tmux", ["-S", socket, ...args], { encoding: "utf8", env });
const option = (name) => tmux("show", "-gqv", name).stdout.replace(/\n$/, "");
const script = path.join(__dirname, "../tmux/continuum-save.sh");
const run = (args = [], extra = {}) => spawnSync("bash", [script, ...args], { encoding: "utf8", env: { ...env, ...extra } });
after(() => {
  tmux("kill-server");
  fs.rmSync(tmp, { recursive: true, force: true });
});

test("continuum-save.sh puts the save job back once, ahead of what status-right holds", () => {
  tmux("set", "-g", "@continuum-save-last-timestamp", "1000");
  tmux("set", "-g", "status-right", "right");
  assert.equal(run(["--check"]).status, 1, "--check says it would put it back");
  assert.equal(option("status-right"), "right", "and changes nothing");

  assert.equal(run().status, 0);
  assert.equal(option("status-right"), `${job}right`);
  assert.equal(run().status, 0);
  assert.equal(option("status-right"), `${job}right`, "a second run adds no second job");
  assert.equal(run(["--check"]).status, 0);
});

test("continuum-save.sh leaves a server that never saved alone but with --start, and a try's alone", () => {
  tmux("set", "-gu", "@continuum-save-last-timestamp");
  tmux("set", "-g", "status-right", "");
  assert.equal(run().status, 2);
  assert.equal(option("status-right"), "", "a save would write over the tabs saved before");

  tmux("set", "-g", "@continuum-save-last-timestamp", "1000");
  assert.equal(run([], { SWITCHBOARD_TRY_OF: "/Users/someone" }).status, 2);
  assert.equal(option("status-right"), "");

  tmux("set", "-gu", "@continuum-save-last-timestamp");
  const before = Math.floor(Date.now() / 1000);
  assert.equal(run(["--start"]).status, 0);
  assert.equal(option("status-right"), job);
  assert.ok(Number(option("@continuum-save-last-timestamp")) >= before, "its first save a save interval from now");
});

test("continuum-save.sh --started restores the last save and starts the saves once on Switchboard's own server, and only there", async () => {
  // A server named switchboard, as tmux -L switchboard's socket is, and a
  // stand-in for resurrect's restore script that counts its runs.
  const at = path.join(tmp, "sockets/switchboard");
  fs.mkdirSync(path.dirname(at), { recursive: true });
  const restored = path.join(tmp, "restored");
  const restore = path.join(tmp, "restore.sh");
  fs.writeFileSync(restore, `#!/bin/sh\necho >> '${restored}'\n`, { mode: 0o755 });
  execFileSync("tmux", ["-S", at, "-f", "/dev/null", "new-session", "-d", "cat"], { env: { ...env, TMUX: "" } });
  const own = (...args) => spawnSync("tmux", ["-S", at, ...args], { encoding: "utf8", env });
  const ownEnv = { TMUX: `${at},0,0` };
  try {
    own("set", "-g", "status-right", "");
    own("set", "-g", "@resurrect-restore-script-path", restore);
    own("set", "-g", "@setting_auto_restore", "on");

    // On tmux's default server it is as with no flag: a server that never saved is left alone.
    own("set", "-g", "@setting_tmux_server", "default");
    assert.equal(run(["--started"], ownEnv).status, 2);
    assert.equal(own("show", "-gqv", "status-right").stdout.trim(), "");

    own("set", "-g", "@setting_tmux_server", "switchboard");
    const before = Math.floor(Date.now() / 1000);
    assert.equal(run(["--started"], ownEnv).status, 0);
    assert.equal(own("show", "-gqv", "status-right").stdout.trim(), job, "the saves start");
    assert.ok(Number(own("show", "-gqv", "@continuum-save-last-timestamp").stdout) >= before, "the first a save interval from now");
    await new Promise((resolve) => setTimeout(resolve, 1600));
    assert.equal(fs.readFileSync(restored, "utf8"), "\n", "the last save is restored, after tmux.conf has loaded");

    // A reload of tmux.conf restores nothing again.
    assert.equal(run(["--started"], ownEnv).status, 0);
    await new Promise((resolve) => setTimeout(resolve, 1600));
    assert.equal(fs.readFileSync(restored, "utf8"), "\n");
  } finally {
    own("kill-server");
  }
});
