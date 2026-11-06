// Tests for Switchboard on a tmux server of its own (the tmux_server setting):
// tmux/switchboard-tmux.sh, and agent-state.sh leaving every other tmux
// server alone. Run with: node --test test/*.test.js
//
// Two throwaway tmux servers, one named switchboard and one on the default
// socket, both under a temporary TMUX_TMPDIR, and a temporary HOME, so nothing
// reaches your tmux, your settings or your Claude sessions.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "tmux-server-")));
const home = path.join(tmp, "home");
const settings = path.join(home, ".config/switchboard/settings.json");
const sessions = path.join(home, ".claude/sessions");
fs.mkdirSync(sessions, { recursive: true });
fs.mkdirSync(path.dirname(settings), { recursive: true });
// A pane counts as Claude by its command's name, Claude Code's version.
const fakeBin = path.join(tmp, "bin");
const fakeClaude = path.join(fakeBin, "2.1.0");
fs.mkdirSync(fakeBin);
fs.copyFileSync("/bin/cat", fakeClaude);
// A copied system binary is killed on launch until it is signed again.
execFileSync("codesign", ["-s", "-", "-f", fakeClaude], { stdio: "ignore" });

const env = { ...process.env, HOME: home, TMUX_TMPDIR: tmp };
for (const name of ["TMUX", "TMUX_PANE", "SWITCHBOARD_SETTINGS", "SWITCHBOARD_TRY_OF"]) delete env[name];
const on = (name) => (...args) => execFileSync("tmux", ["-L", name, ...args], { env, encoding: "utf8" }).replace(/\n$/, "");
const own = on("switchboard");
const plain = on("default");
own("-f", "/dev/null", "new-session", "-d", "-s", "main", fakeClaude);
plain("-f", "/dev/null", "new-session", "-d", "-s", "mine", fakeClaude);
const where = (server) => {
  const [socket, pid, pane] = server("display", "-p", "#{socket_path}\t#{pid}\t#{pane_id}").split("\t");
  return { socket, pane, tmux: `${socket},${pid},0` };
};
after(() => {
  for (const server of [own, plain]) {
    try {
      server("kill-server");
    } catch {}
  }
  fs.rmSync(tmp, { recursive: true, force: true });
});

const setting = (value) => fs.writeFileSync(settings, JSON.stringify(value === undefined ? {} : { tips: false, tmux_server: value }));
const helper = path.join(__dirname, "../tmux/switchboard-tmux.sh");
const hook = path.join(__dirname, "../tmux/agent-state.sh");
const working = (extra, session = "s") =>
  execFileSync("bash", [hook, "set", "working"], { env: { ...env, ...extra }, input: JSON.stringify({ hook_event_name: "PostToolUse", session_id: session }) });

test("switchboard-tmux.sh runs tmux on the server the tmux_server setting names, from any tmux or none", () => {
  const ownAt = where(own);
  const plainAt = where(plain);
  const socketFrom = (extra) => execFileSync("bash", [helper, "display", "-p", "#{socket_path}"], { env: { ...env, ...extra }, encoding: "utf8" }).trim();

  setting(undefined);
  assert.equal(socketFrom({}), plainAt.socket, "tmux's default server, with no setting");
  assert.equal(socketFrom({ TMUX: ownAt.tmux }), ownAt.socket, "or the one it runs in, as tmux finds it");

  setting("switchboard");
  assert.equal(socketFrom({}), ownAt.socket);
  assert.equal(socketFrom({ TMUX: plainAt.tmux }), ownAt.socket, "from inside another tmux server too");

  setting("nonsense");
  assert.equal(socketFrom({}), plainAt.socket, "a name the setting doesn't offer is the default");
});

test("agent-state.sh leaves a Claude in another tmux server alone with Switchboard on one of its own", () => {
  const ownAt = where(own);
  const plainAt = where(plain);
  setting("switchboard");
  working({ TMUX: plainAt.tmux, TMUX_PANE: plainAt.pane });
  assert.equal(plain("display", "-p", "-t", plainAt.pane, "#{@agent_state}"), "", "a plain tmux server is left as it is");
  working({ TMUX: ownAt.tmux, TMUX_PANE: ownAt.pane });
  assert.equal(own("display", "-p", "-t", ownAt.pane, "#{@agent_state}"), "working", "Switchboard's own is marked");
  // And its pane with its session, as a session starts, for the sidebar.
  const started = (extra, session) =>
    execFileSync("bash", [hook, "set", "clear"], { env: { ...env, ...extra }, input: JSON.stringify({ hook_event_name: "SessionStart", source: "startup", session_id: session }) });
  started({ TMUX: ownAt.tmux, TMUX_PANE: ownAt.pane }, "ours");
  assert.equal(own("display", "-p", "-t", ownAt.pane, "#{@agent_session}"), "ours");
  started({ TMUX: plainAt.tmux, TMUX_PANE: plainAt.pane }, "theirs");
  assert.equal(plain("display", "-p", "-t", plainAt.pane, "#{@agent_session}"), "");
  own("set", "-pu", "-t", ownAt.pane, "@agent_session");

  // On tmux's default server, as before: whichever server the session is in.
  setting(undefined);
  working({ TMUX: plainAt.tmux, TMUX_PANE: plainAt.pane });
  assert.equal(plain("display", "-p", "-t", plainAt.pane, "#{@agent_state}"), "working");
  for (const [server, at] of [[own, ownAt], [plain, plainAt]]) server("set", "-pu", "-t", at.pane, "@agent_state");
});

test("agent-state.sh looks for a parked job's tab on Switchboard's own server", () => {
  const ownAt = where(own);
  const panePid = own("display", "-p", "-t", ownAt.pane, "#{pane_pid}");
  // The tab's Claude is parked into job J; the job runs elsewhere, with no tmux.
  fs.writeFileSync(path.join(sessions, `${panePid}.json`), JSON.stringify({ pid: Number(panePid), sessionId: "front", kind: "interactive", parkedJobId: "J" }));
  fs.writeFileSync(path.join(sessions, "900010.json"), JSON.stringify({ pid: 900010, sessionId: "background", kind: "bg", jobId: "J" }));
  try {
    setting("switchboard");
    working({}, "background");
    assert.equal(own("display", "-p", "-t", ownAt.pane, "#{@agent_state}"), "working");
    execFileSync("bash", [hook, "set", "working"], { env, input: JSON.stringify({ hook_event_name: "UserPromptSubmit", prompt: "go on", session_id: "background" }) });
    assert.equal(own("display", "-p", "-t", ownAt.pane, "#{@agent_session}"), "", "a parked job's session names no pane, so marks none");
  } finally {
    ["900010", panePid].forEach((pid) => fs.rmSync(path.join(sessions, `${pid}.json`)));
    own("set", "-pu", "-t", ownAt.pane, "@agent_state");
  }
});
