// Tests for tmux/health.sh, the status line's health light. Run with:
// node --test test/*.test.js
//
// It runs against a throwaway tmux server and a temporary HOME, so it never
// reads your tmux, your sidebar daemon or your relay.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "health-")));
const home = path.join(tmp, "home");
const socket = path.join(tmp, "tmux.sock");
fs.mkdirSync(path.join(home, ".local/state/tmux-sidebar"), { recursive: true });
fs.mkdirSync(path.join(home, ".local/state/slack-relay"), { recursive: true });
execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "-s", "health", "cat"]);
const tmux = (...args) => execFileSync("tmux", ["-S", socket, ...args], { encoding: "utf8" }).trimEnd();
const serverPid = tmux("display", "-p", "#{pid}");
after(() => {
  tmux("kill-server");
  fs.rmSync(tmp, { recursive: true, force: true });
});

test("the light is dark while all is well, and names each thing that quietly stopped", () => {
  const light = () =>
    execFileSync("bash", [path.join(__dirname, "../tmux/health.sh")], {
      encoding: "utf8",
      env: { ...process.env, HOME: home, TMUX: `${socket},${serverPid},0` },
    });
  const sidebarLock = path.join(home, ".local/state/tmux-sidebar/daemon.pid");
  const relayLock = path.join(home, ".local/state/slack-relay/relay.lock");
  const events = path.join(home, ".local/state/slack-relay/events.jsonl");
  const now = Math.floor(Date.now() / 1000);

  // All well: a save a minute ago, the sidebar daemon (this test's process) running, the relay stopped on purpose.
  tmux("set", "-g", "@continuum-save-last-timestamp", String(now - 60));
  fs.writeFileSync(sidebarLock, String(process.pid));
  assert.equal(light(), "");

  // Saves stopped twelve minutes ago, continuum's job still in status-right.
  tmux("set", "-g", "status-right", "#(continuum_save.sh)");
  tmux("set", "-g", "@continuum-save-last-timestamp", String(now - 12 * 60));
  assert.equal(light(), "#[fg=colour196,bold] ● saves 12m old #[default]");
  tmux("set", "-g", "@continuum-save-last-timestamp", String(now));

  // The sidebar daemon gone, and the relay crashed (its events file left behind).
  fs.writeFileSync(sidebarLock, "999999");
  fs.writeFileSync(events, "");
  fs.writeFileSync(relayLock, "999999 slack-relay");
  assert.equal(light(), "#[fg=colour196,bold] ● sidebar down, relay down #[default]");

  // The sidebar turned off isn't down; the relay running isn't either.
  tmux("set", "-g", "@sidebar_on", "off");
  fs.writeFileSync(relayLock, `${process.pid} slack-relay`);
  assert.equal(light(), "");
});

test("a Claude connector that wasn't connected when a feature last tried lights it, for two hours", () => {
  const light = () =>
    execFileSync("bash", [path.join(__dirname, "../tmux/health.sh")], {
      encoding: "utf8",
      env: { ...process.env, HOME: home, TMUX: `${socket},${serverPid},0` },
    });
  const now = Math.floor(Date.now() / 1000);
  tmux("set", "-g", "@continuum-save-last-timestamp", String(now - 60));
  tmux("set", "-g", "@sidebar_on", "off");
  const down = path.join(home, ".local/state/tmux-sidebar/connectors-down");
  fs.writeFileSync(down, `Slack\t${now - 60}\nFathom\t${now - 3 * 3600}\n`);
  assert.match(light(), /Slack not connected/);
  assert.doesNotMatch(light(), /Fathom/, "an old failure has had its chance to be seen");
  fs.writeFileSync(down, "");
  assert.equal(light(), "");
});

test("saves stopped with continuum's job gone from status-right say the autosave is off, and the light puts it back", async () => {
  const env = { ...process.env, HOME: home, TMUX: `${socket},${serverPid},0` };
  delete env.SWITCHBOARD_TRY_OF;
  const light = () => execFileSync("bash", [path.join(__dirname, "../tmux/health.sh")], { encoding: "utf8", env });
  // A stand-in for continuum's save script, and continuum-save.sh where setup links it.
  const save = path.join(home, ".config/tmux/plugins/tmux-continuum/scripts/continuum_save.sh");
  fs.mkdirSync(path.dirname(save), { recursive: true });
  fs.writeFileSync(save, "#!/bin/sh\n", { mode: 0o755 });
  fs.symlinkSync(path.join(__dirname, "../tmux/continuum-save.sh"), path.join(home, ".config/tmux/continuum-save.sh"));
  tmux("set", "-g", "@sidebar_on", "off");
  tmux("set", "-g", "status-right", "");
  tmux("set", "-g", "@continuum-save-last-timestamp", String(Math.floor(Date.now() / 1000) - 10 * 60));
  try {
    assert.equal(light(), "#[fg=colour196,bold] ● saves 10m old (autosave off) #[default]");
    for (let i = 0; i < 50 && tmux("show", "-gqv", "status-right") !== `#(${save})`; i++) await new Promise((r) => setTimeout(r, 100));
    assert.equal(tmux("show", "-gqv", "status-right"), `#(${save})`);
    assert.equal(light(), "#[fg=colour196,bold] ● saves 10m old #[default]", "with the job back, only how old the save is");
  } finally {
    fs.rmSync(path.join(home, ".config/tmux"), { recursive: true, force: true });
    tmux("set", "-g", "@continuum-save-last-timestamp", String(Math.floor(Date.now() / 1000)));
  }
});

test("a script in ~/switchboard's tmux/ or bin/ with no link says setup is out of date", () => {
  const light = () =>
    execFileSync("bash", [path.join(__dirname, "../tmux/health.sh")], {
      encoding: "utf8",
      env: { ...process.env, HOME: home, TMUX: `${socket},${serverPid},0` },
    });
  tmux("set", "-g", "@sidebar_on", "off");
  tmux("set", "-g", "@continuum-save-last-timestamp", String(Math.floor(Date.now() / 1000)));
  for (const dir of ["switchboard/tmux", "switchboard/bin", ".config/tmux", ".local/bin"]) fs.mkdirSync(path.join(home, dir), { recursive: true });
  try {
    fs.writeFileSync(path.join(home, "switchboard/tmux/new.sh"), "");
    fs.writeFileSync(path.join(home, "switchboard/bin/tool"), "", { mode: 0o755 });
    fs.writeFileSync(path.join(home, "switchboard/bin/README.md"), "");
    assert.equal(light(), "#[fg=colour196,bold] ● setup out of date #[default]");
    fs.symlinkSync(path.join(home, "switchboard/tmux/new.sh"), path.join(home, ".config/tmux/new.sh"));
    assert.match(light(), /setup out of date/, "a script in bin/ still isn't linked");
    fs.symlinkSync(path.join(home, "switchboard/bin/tool"), path.join(home, ".local/bin/tool"));
    assert.equal(light(), "", "and a file in bin/ that isn't a script needs no link");
  } finally {
    for (const dir of ["switchboard", ".config/tmux", ".local/bin"]) fs.rmSync(path.join(home, dir), { recursive: true, force: true });
  }
});
