// Tests for bin/slack-relay. Run with: node --test test/*.test.js
//
// Nothing here touches your real tabs, Slack, or ~/.claude. Before the relay is
// loaded, HOME points at a temporary directory and TMUX at a tmux server of
// the tests' own, which every plain `tmux` command then talks to; the tests
// refuse to run unless that is so. Its "Claude" panes run a copy of cat named
// like a Claude Code version, so the relay takes them for Claude and typing
// into one just echoes. Slack is a stand-in put in place of the relay's Web
// API calls and file uploads, the Socket Mode connection is never opened, and
// `claude` on the PATH is a script that fails, so no real session can start
// by accident.

const { test, after, beforeEach } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync, spawn } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "slack-relay-test-")));
const home = path.join(tmp, "home");
const fakeBin = path.join(tmp, "bin");
const fakeClaude = path.join(fakeBin, "2.1.0");
const socket = path.join(tmp, "tmux.sock");
fs.mkdirSync(path.join(home, ".claude/sessions"), { recursive: true });
fs.mkdirSync(path.join(home, ".claude/projects/-work"), { recursive: true });
fs.mkdirSync(fakeBin);
fs.copyFileSync("/bin/cat", fakeClaude);
// A copied system binary is killed on launch until it is signed again.
execFileSync("codesign", ["-s", "-", "-f", fakeClaude], { stdio: "ignore" });
fs.writeFileSync(path.join(fakeBin, "claude"), "#!/bin/sh\necho 'no real claude in tests' >&2\nexit 1\n", { mode: 0o755 });

const realTmux = process.env.TMUX;
delete process.env.TMUX;
execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "-s", "test", "-x", "200", "-y", "50", fakeClaude]);
execFileSync("tmux", ["-S", socket, "set", "-g", "default-command", fakeClaude]);
const serverPid = execFileSync("tmux", ["-S", socket, "display", "-p", "#{pid}"], { encoding: "utf8" }).trim();
Object.assign(process.env, {
  HOME: home,
  TMUX: `${socket},${serverPid},0`,
  TMUX_TMPDIR: tmp,
  PATH: `${fakeBin}:${process.env.PATH}`,
  LC_ALL: "en_US.UTF-8",
});
delete process.env.TMUX_PANE;

const tmux = (...args) => execFileSync("tmux", args, { encoding: "utf8" }).trimEnd();
assert.equal(tmux("display", "-p", "#{socket_path}"), socket, "tmux is not the tests' own server");
assert.notEqual(tmux("display", "-p", "#{socket_path}"), realTmux?.split(",")[0]);
assert.equal(os.homedir(), home);

const relay = require("../bin/slack-relay");
assert.ok(relay.STATE_DIR.startsWith(home));

// What the Slack stand-in was asked to post, react with, edit and upload, in
// order, and every call it got. A test that needs another answer from Slack
// sets `answers[method]` to a function of the call's parameters.
let posts;
let reactions;
let updates;
let uploads;
let views;
let calls;
let answers;
let nextTs;
beforeEach(() => {
  posts = [];
  reactions = [];
  updates = [];
  uploads = [];
  views = [];
  calls = [];
  answers = {};
  nextTs = Math.floor(Date.now() / 1000);
  relay.slack.channel = "D1";
  relay.slack.owner = "U1";
  relay.slack.api = async (method, params = {}) => {
    calls.push({ method, params });
    const channel = params.channel ?? params.channel_id;
    if (channel !== undefined && channel !== relay.slack.channel) throw new Error(`${method}: slack-relay only uses its DM, not ${channel}`);
    if (answers[method]) return answers[method](params);
    if (method === "chat.postMessage") {
      const ts = `${nextTs++}.000100`;
      posts.push({ body: params.text, thread: params.thread_ts, broadcast: Boolean(params.reply_broadcast), ts });
      return { ok: true, ts };
    }
    if (method === "reactions.add") {
      reactions.push({ ts: params.timestamp, emoji: params.name });
      return { ok: true };
    }
    if (method === "chat.update") {
      updates.push({ ts: params.ts, body: params.text });
      return { ok: true, ts: params.ts };
    }
    if (method === "views.publish") {
      views.push(params.view);
      return { ok: true };
    }
    if (method === "chat.delete" || method === "reactions.remove") return { ok: true };
    if (method === "chat.scheduleMessage") return { ok: true, scheduled_message_id: `Q${calls.filter((c) => c.method === method).length}` };
    if (method === "chat.deleteScheduledMessage") return { ok: true };
    if (method === "chat.scheduledMessages.list") return { ok: true, scheduled_messages: [] };
    if (method === "files.getUploadURLExternal") return { ok: true, upload_url: "https://files.example/upload", file_id: "F1" };
    if (method === "files.completeUploadExternal") {
      // Shared together: each of the files the call names was the last uploaded.
      uploads.slice(-params.files.length).forEach((upload) => (upload.share ??= params));
      return { ok: true };
    }
    throw new Error(`this test makes no ${method} call`);
  };
  relay.slack.get = async () => {
    throw new Error("this test downloads nothing");
  };
  relay.slack.put = async (url, content) => {
    uploads.push({ url, content: content.toString("utf8") });
  };
  // Away from the computer, so what the relay says on its own goes out at
  // once; the tests about being at the computer say so.
  relay.presence.idleMs = relay.AWAY_AFTER_MS;
  relay.presence.held = [];
  relay.presence.slackUntil = 0;
  relay.presence.readIdle = async () => relay.presence.idleMs;
  relay.presence.locked = false;
  relay.presence.readLocked = async () => relay.presence.locked;
  relay.ownerOnly();
});

after(() => {
  try {
    execFileSync("tmux", ["-S", socket, "kill-server"], { stdio: "ignore" });
  } catch {}
  fs.rmSync(tmp, { recursive: true, force: true });
});

test("chunks splits long text at line breaks and keeps code blocks whole", () => {
  assert.deepEqual(relay.chunks("short"), ["short"]);
  const text = "intro\n```\n" + Array.from({ length: 400 }, (_, i) => `line ${i} of the output`).join("\n") + "\n```\nend";
  const parts = relay.chunks(text);
  assert.ok(parts.length > 1);
  parts.forEach((part) => {
    assert.ok(part.length <= relay.MAX_POST_CHARS + 4);
    assert.equal((part.match(/```/g) ?? []).length % 2, 0, "every part opens and closes its code block");
  });
  assert.equal(parts.join("\n").replace(/\n```\n```\n/g, "\n"), text);
});

test("turnFrom shows the last prompt, the reply, and a tool count", () => {
  const file = path.join(home, ".claude/projects/-work/turn.jsonl");
  const sent = new Date(Date.now() - 5 * 60_000).toISOString();
  fs.writeFileSync(file, [
    { type: "user", message: { content: "an earlier prompt" } },
    { type: "assistant", message: { content: [{ type: "text", text: "earlier reply" }] } },
    { type: "user", timestamp: sent, message: { content: [{ type: "text", text: "run   the\ntests" }] } },
    { type: "user", isMeta: true, message: { content: "meta" } },
    { type: "assistant", message: { content: [{ type: "text", text: "Running them." }, { type: "tool_use", id: "t1" }] } },
    { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "t1", content: "ok" }] } },
    { type: "assistant", isSidechain: true, message: { content: [{ type: "text", text: "a subagent" }] } },
    { type: "assistant", message: { content: [{ type: "text", text: "All pass." }] } },
  ].map((entry) => JSON.stringify(entry)).join("\n"));

  assert.equal(relay.turnFrom(file), "_(prompt sent 5m ago)_\n> run   the\n> tests\n\nRunning them.\n\nAll pass.\n\n_(ran 1 tool)_");
  assert.ok(relay.turnFrom(file, "run the tests"));
  assert.equal(relay.turnFrom(file, "something else"), undefined);
  assert.equal(relay.turnFrom(path.join(tmp, "missing.jsonl")), undefined);
});

test("transcriptFor follows parked sessions to the end of the chain", () => {
  const window = tmux("new-window", "-d", "-P", "-F", "#{window_id}", fakeClaude);
  const panePid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  const sessions = path.join(home, ".claude/sessions");
  // The tab's session was parked into job A, which was itself parked into job
  // B. The jobs need live processes; this test's own will do.
  fs.writeFileSync(path.join(sessions, `${panePid}.json`), JSON.stringify({ pid: Number(panePid), sessionId: "tab", parkedJobId: "A" }));
  fs.writeFileSync(path.join(sessions, "900001.json"), JSON.stringify({ pid: process.pid, sessionId: "job-a", jobId: "A", parkedJobId: "B" }));
  fs.writeFileSync(path.join(sessions, "900002.json"), JSON.stringify({ pid: process.pid, sessionId: "job-b", jobId: "B" }));
  // A dead job with the same ID is not the one.
  fs.writeFileSync(path.join(sessions, "900003.json"), JSON.stringify({ pid: 999999, sessionId: "dead", jobId: "B" }));
  ["tab", "job-a", "job-b", "dead"].forEach((id) => fs.writeFileSync(path.join(home, `.claude/projects/-work/${id}.jsonl`), ""));

  assert.equal(relay.transcriptFor(window), path.join(home, ".claude/projects/-work/job-b.jsonl"));

  // A chain that loops back on itself ends rather than spinning.
  fs.writeFileSync(path.join(sessions, "900002.json"), JSON.stringify({ pid: process.pid, sessionId: "job-b", jobId: "B", parkedJobId: "A" }));
  assert.equal(relay.transcriptFor(window), path.join(home, ".claude/projects/-work/job-b.jsonl"));

  ["900001", "900002", "900003", panePid].forEach((pid) => fs.rmSync(path.join(sessions, `${pid}.json`)));
  tmux("kill-window", "-t", window);
});

test("a top-level message sent to a tab with the picker is typed into it and goes on in the tab's home thread", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  // tmux takes a moment to see the pane's command as the fake Claude.
  await new Promise((resolve) => setTimeout(resolve, 300));
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  const ts = `${Math.floor(Date.now() / 1000)}.000001`;

  // Naming no tab, it is only offered to them; the tab's button sends it.
  await relay.handle(state, { ts, text: "hello from slack", thread_ts: null });
  assert.doesNotMatch(tmux("capture-pane", "-p", "-t", window), /hello from slack/, "not typed anywhere yet");
  const picker = calls.find((c) => c.method === "chat.postMessage" && /Send it to a tab/.test(c.params.text));
  const button = picker.params.blocks[1].elements.find((b) => JSON.parse(b.value).w === window);
  await relay.handlePress(state, { value: JSON.parse(button.value), message: { ts: posts[0].ts, text: posts[0].body } });
  await new Promise((resolve) => setTimeout(resolve, 200));

  assert.match(tmux("capture-pane", "-p", "-t", window), /hello from slack/);
  // The tab's home thread starts; nothing else is posted until the answer,
  // which goes there and to the main DM.
  assert.equal(posts.length, 2);
  const homeThread = posts[1].ts;
  assert.match(posts[1].body, new RegExp(`^🤖 \\*\\*Tab ${index}, .*\\*\\*\\n⚪ Idle\\n\\nThis is its thread`));
  assert.equal(posts[1].thread, undefined);
  assert.equal(state.homes[window].ts, homeThread);
  assert.equal(state.alerts[homeThread].window, window);
  assert.equal(state.following[window].broadcast, true);
  assert.equal(state.following[window].text, "hello from slack");
  assert.equal(state.following[window].thread, homeThread);
  assert.equal(state.following[window].request, ts);
  tmux("kill-window", "-t", window);
});

test("a reply under an alert goes to the alert's tab", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  // tmux takes a moment to see the pane's command as the fake Claude.
  await new Promise((resolve) => setTimeout(resolve, 300));
  const alert = `${Math.floor(Date.now() / 1000) - 60}.000100`;
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: { [alert]: { window, index: Number(index), name: "work" } }, reported: {}, following: {} };

  await relay.handle(state, { ts: `${Math.floor(Date.now() / 1000)}.000002`, text: "2", thread_ts: alert });
  await new Promise((resolve) => setTimeout(resolve, 200));

  assert.match(tmux("capture-pane", "-p", "-t", window), /^2$/m);
  assert.equal(posts.length, 0, "👀 says it was taken; nothing is posted");
  assert.equal(state.following[window].thread, alert);
  tmux("kill-window", "-t", window);
});

test("unrecognised messages, ones naming a tab by number, and old ones are not typed anywhere", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  // tmux takes a moment to see the pane's command as the fake Claude.
  await new Promise((resolve) => setTimeout(resolve, 300));
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  const now = Math.floor(Date.now() / 1000);

  await relay.handle(state, { ts: `${now}.000003`, text: `l: yes`, thread_ts: null });
  await relay.handle(state, { ts: `${now}.000004`, text: `${index}: yes`, thread_ts: null });
  await relay.handle(state, { ts: `${now - 600}.000005`, text: "yes", thread_ts: null });
  await relay.handle(state, { ts: `${now}.000006`, text: "just chatting", thread_ts: "1791500000.000200" });
  await new Promise((resolve) => setTimeout(resolve, 200));

  assert.match(posts[0].body, /That doesn't name a tab or a command\. \*\*Send it to a tab\?\*\*/);
  // A button per tab, to send it there.
  const picker = calls.find((c) => c.method === "chat.postMessage" && c.params.text === posts[0].body).params.blocks[1];
  assert.equal(picker.type, "actions");
  assert.ok(picker.elements.some((b) => b.text.text.startsWith(`${index} `) && JSON.parse(b.value).w === window));
  assert.match(posts[1].body, /That doesn't name a tab or a command/, "a number in front names no tab");
  assert.match(posts[2].body, /I missed this while I wasn't running/);
  assert.match(posts[3].body, /This thread isn't linked to a tab/);
  assert.equal(posts[3].thread, "1791500000.000200");
  assert.doesNotMatch(tmux("capture-pane", "-p", "-t", window), /yes|chatting/);
  assert.deepEqual(state.following, {});
  tmux("kill-window", "-t", window);
});

test("status lists the Claude tabs with their states", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  // tmux takes a moment to see the pane's command as the fake Claude.
  await new Promise((resolve) => setTimeout(resolve, 300));
  tmux("set", "-p", "-t", window, "@agent_state", "waiting");
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };

  await relay.handle(state, { ts: `${Math.floor(Date.now() / 1000)}.000007`, text: "status", thread_ts: null });

  assert.match(posts[0].body, new RegExp(`\\| \\*\\*${index}\\*\\* \\| [^|]* \\| 🔴 \\*\\*waiting on you\\*\\* \\|`));
  assert.equal(posts[0].thread, undefined);
  tmux("kill-window", "-t", window);
});

test("new: starts a tab running claude with the prompt", async () => {
  const before = tmux("list-windows", "-F", "#{window_id}").split("\n");
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };

  const ts = await relay.handle(state, { ts: `${Math.floor(Date.now() / 1000)}.000008`, text: "new: fix the 'login' bug", thread_ts: null });
  await new Promise((resolve) => setTimeout(resolve, 200));

  const window = tmux("list-windows", "-F", "#{window_id}").split("\n").find((id) => !before.includes(id));
  assert.ok(window);
  assert.match(tmux("capture-pane", "-p", "-t", window), /claude 'fix the '\\''login'\\'' bug'/);
  assert.equal(state.alerts[ts].window, window);
  tmux("kill-window", "-t", window);
});

test("newEvents carries on from the saved offset and leaves half-written lines", () => {
  fs.writeFileSync(relay.EVENTS_FILE, `${JSON.stringify({ event: "done", time: 1 })}\n${JSON.stringify({ event: "waiting", time: 2 })}\n{"event":"do`);
  const state = { eventsOffset: 0 };

  assert.deepEqual(relay.newEvents(state).map((e) => e.event), ["done", "waiting"]);
  const offset = state.eventsOffset;
  assert.deepEqual(relay.newEvents(state), []);

  // A restarted relay, with the offset from its state file, reads only what came since.
  fs.appendFileSync(relay.EVENTS_FILE, `ne","time":3}\n`);
  const restarted = { eventsOffset: offset };
  assert.deepEqual(relay.newEvents(restarted).map((e) => e.time), [3]);

  // A file shorter than the offset is a new one, read from the start.
  fs.writeFileSync(relay.EVENTS_FILE, `${JSON.stringify({ event: "done", time: 4 })}\n`);
  assert.deepEqual(relay.newEvents(restarted).map((e) => e.time), [4]);
  fs.rmSync(relay.EVENTS_FILE);
});

test("followUps reports a finished turn once, in the request's thread", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  // tmux takes a moment to see the pane's command as the fake Claude.
  await new Promise((resolve) => setTimeout(resolve, 300));
  const transcript = path.join(home, ".claude/projects/-work/followed.jsonl");
  fs.writeFileSync(transcript, [
    { type: "user", message: { content: "deploy it" } },
    { type: "assistant", message: { content: [{ type: "text", text: "Deployed." }] } },
  ].map((entry) => JSON.stringify(entry)).join("\n"));
  const now = Math.floor(Date.now() / 1000);
  fs.writeFileSync(relay.EVENTS_FILE, [
    // Before the request was sent, so not its answer.
    { event: "done", time: now - 120, transcript_path: transcript },
    // Another session's turn.
    { event: "done", time: now, transcript_path: path.join(tmp, "other.jsonl") },
    { event: "done", time: now, transcript_path: transcript },
  ].map((entry) => `${JSON.stringify(entry)}\n`).join(""));
  const thread = "1791500000.000300";
  const state = {
    dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, eventsOffset: 0,
    following: { [window]: { thread, index: Number(index), name: "work", text: "deploy it", sentAt: Date.now() - 5_000, screen: "", changedAt: 0 } },
  };

  await relay.followUps(state);
  await relay.followUps(state);

  assert.equal(posts.length, 1);
  assert.equal(posts[0].thread, thread);
  assert.equal(posts[0].body, `🤖 tab ${index}, work is done:\n> deploy it\n\nDeployed.`);
  assert.deepEqual(state.following, {});
  assert.equal(state.alerts[posts[0].ts].window, window);
  fs.rmSync(relay.EVENTS_FILE);
  tmux("kill-window", "-t", window);
});

test("a wait shows the screen, so a permission prompt can be answered", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  // tmux takes a moment to see the pane's command as the fake Claude.
  await new Promise((resolve) => setTimeout(resolve, 300));
  tmux("send-keys", "-t", window, "-l", "Do you want to proceed? 1. Yes 2. No");
  await new Promise((resolve) => setTimeout(resolve, 200));
  const transcript = path.join(home, ".claude/projects/-work/waiting.jsonl");
  fs.writeFileSync(transcript, JSON.stringify({ type: "user", message: { content: "delete the branch" } }));
  fs.writeFileSync(relay.EVENTS_FILE, `${JSON.stringify({ event: "waiting", time: Math.floor(Date.now() / 1000), transcript_path: transcript })}\n`);
  const state = {
    dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, eventsOffset: 0,
    following: { [window]: { thread: null, index: Number(index), name: "work", text: "delete the branch", sentAt: Date.now() - 5_000, screen: "", changedAt: 0 } },
  };

  await relay.followUps(state);

  assert.equal(posts.length, 1);
  assert.match(posts[0].body, /is waiting on you/);
  assert.match(posts[0].body, /On screen now:\n```\n[\s\S]*Do you want to proceed\? 1\. Yes 2\. No[\s\S]*\n```$/);
  fs.rmSync(relay.EVENTS_FILE);
  tmux("kill-window", "-t", window);
});

test("alertTabs alerts once per state change, with the screen for a wait", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  // tmux takes a moment to see the pane's command as the fake Claude.
  await new Promise((resolve) => setTimeout(resolve, 300));
  tmux("set", "-p", "-t", window, "@agent_state", "waiting");
  tmux("set", "-p", "-t", window, "@agent_since", "100");
  tmux("send-keys", "-t", window, "-l", "Allow this edit? 1. Yes 2. No");
  await new Promise((resolve) => setTimeout(resolve, 200));
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  const panePid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  fs.writeFileSync(path.join(home, ".claude/sessions", `${panePid}.json`), JSON.stringify({ pid: Number(panePid), sessionId: "alerted" }));
  fs.writeFileSync(path.join(home, ".claude/projects/-work/alerted.jsonl"), JSON.stringify({ type: "user", message: { content: "edit the config" } }));

  await relay.alertTabs(state);
  await relay.alertTabs(state);
  tmux("set", "-p", "-t", window, "@agent_state", "done");
  tmux("set", "-p", "-t", window, "@agent_since", "200");
  await relay.alertTabs(state);

  // The home thread, then each alert in it with a one-line notice also in the main DM.
  assert.equal(posts.length, 4);
  const homeThread = posts[0].ts;
  assert.match(posts[0].body, /This is its thread/);
  assert.ok(state.threads[homeThread]);
  assert.match(posts[1].body, new RegExp(`^🤖 \\*\\*Tab ${index}, .*\\*\\* is \\*\\*waiting on you\\*\\*`));
  assert.match(posts[1].body, /> edit the config/);
  assert.match(posts[1].body, /On screen now:\n```\n[\s\S]*Allow this edit\? 1\. Yes 2\. No/);
  assert.equal(posts[1].thread, homeThread);
  assert.equal(posts[1].broadcast, false);
  assert.match(posts[2].body, new RegExp(`^🤖 \\*\\*Tab ${index}, .*\\*\\* is \\*\\*waiting on you\\*\\*\\.$`));
  assert.equal(posts[2].thread, homeThread);
  assert.equal(posts[2].broadcast, true);
  assert.match(posts[3].body, /has finished/);
  assert.doesNotMatch(posts[3].body, /On screen now:/);
  assert.equal(posts[3].broadcast, false, "a finished turn gets no line in the main DM");
  assert.equal(state.alerts[posts[1].ts].window, window);
  assert.equal(state.alerts[posts[2].ts].thread, homeThread);
  fs.rmSync(path.join(home, ".claude/sessions", `${panePid}.json`));
  tmux("kill-window", "-t", window);
});

test("alertTabs tells you when a loop tab waits, not each time it finishes a tick", async () => {
  const window = tmux("new-window", "-d", "-P", "-F", "#{window_id}", fakeClaude);
  // tmux takes a moment to see the pane's command as the fake Claude.
  await new Promise((resolve) => setTimeout(resolve, 300));
  tmux("set", "-w", "-t", window, "@loop_tab", "1");
  tmux("set", "-p", "-t", window, "@agent_state", "done");
  tmux("set", "-p", "-t", window, "@agent_since", "100");
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  const panePid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  fs.writeFileSync(path.join(home, ".claude/sessions", `${panePid}.json`), JSON.stringify({ pid: Number(panePid), sessionId: "looping" }));
  fs.writeFileSync(path.join(home, ".claude/projects/-work/looping.jsonl"), JSON.stringify({ type: "user", message: { content: "/slack-updates" } }));

  await relay.alertTabs(state);
  assert.equal(posts.length, 0);
  tmux("set", "-p", "-t", window, "@agent_state", "waiting");
  tmux("set", "-p", "-t", window, "@agent_since", "200");
  await relay.alertTabs(state);

  assert.ok(posts.some((p) => /is \*\*waiting on you\*\*/.test(p.body)));
  fs.rmSync(path.join(home, ".claude/sessions", `${panePid}.json`));
  tmux("kill-window", "-t", window);
});

test("a long post goes out in parts, all in the same thread", async () => {
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {} };
  const text = Array.from({ length: 300 }, (_, i) => `line ${i} of a long answer`).join("\n");

  const first = await relay.post(state, text, "1791500000.000400");

  assert.ok(posts.length > 1);
  assert.equal(first, posts[0].ts);
  posts.forEach((p) => assert.equal(p.thread, "1791500000.000400"));
  assert.equal(posts.map((p) => p.body).join("\n"), text);
  // No link previews, which would bury a short reply under a PR's card.
  calls.filter((c) => c.method === "chat.postMessage").forEach((c) => assert.equal(c.params.unfurl_links, false));
});

test("state, log and their directory are owner-only", () => {
  fs.chmodSync(relay.STATE_DIR, 0o755);
  relay.saveState({ threads: {} });
  relay.log("a line for the log");
  relay.ownerOnly();

  assert.equal(fs.statSync(relay.STATE_DIR).mode & 0o777, 0o700);
  assert.equal(fs.statSync(relay.STATE_FILE).mode & 0o777, 0o600);
  assert.equal(fs.statSync(relay.LOG_FILE).mode & 0o777, 0o600);
  assert.match(fs.readFileSync(relay.LOG_FILE, "utf8"), /^\S+Z a line for the log$/m);
});

test("the log says where a post went, and writes a failure that repeats only once a while", async () => {
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {} };
  await relay.post(state, "to the DM");
  await relay.post(state, "to a thread", "1791500000.000100");
  await relay.post(state, "to both", "1791500000.000100", { broadcast: true });
  [1, 2, 3].forEach(() => relay.log("dashboard failed: views.publish: not_enabled (log test)"));
  const lines = fs.readFileSync(relay.LOG_FILE, "utf8").split("\n");
  assert.ok(lines.some((l) => /posted 9 characters in the main DM \(notifies\)$/.test(l)));
  assert.ok(lines.some((l) => /posted 11 characters in a thread$/.test(l)));
  assert.ok(lines.some((l) => /posted 7 characters in a thread and the main DM \(notifies\)$/.test(l)));
  assert.equal(lines.filter((l) => l.includes("(log test)")).length, 1, "the repeats counted, not written");
});

test("lastError finds the reason for the latest crash in the log", () => {
  const since = Date.now();
  fs.appendFileSync(relay.LOG_FILE, `${new Date(since - 60_000).toISOString()} Error: an older crash\n`);
  assert.equal(relay.lastError(since), undefined);
  fs.appendFileSync(relay.LOG_FILE, `${new Date(since + 1).toISOString()} Error: crashed: TypeError: x is undefined\n`);
  assert.equal(relay.lastError(since), "TypeError: x is undefined");
});

test("a message queued behind a busy tab's turn gets ⏳, and nothing is posted", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  // tmux takes a moment to see the pane's command as the fake Claude.
  await new Promise((resolve) => setTimeout(resolve, 300));
  tmux("set", "-p", "-t", window, "@agent_state", "working");
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  const ts = `${Math.floor(Date.now() / 1000)}.000009`;
  const linked = `${Math.floor(Date.now() / 1000) - 30}.000500`;
  state.alerts[linked] = { window, index: Number(index), name: "work" };

  await relay.handle(state, { ts, text: "and also this", thread_ts: linked });

  assert.deepEqual(reactions, [{ ts, emoji: "hourglass_flowing_sand" }]);
  assert.equal(posts.length, 0, "nothing posted");
  assert.equal(state.following[window].request, ts);
  assert.equal(state.following[window].taken, false);
  tmux("kill-window", "-t", window);
});

test("once the tab takes your message: 👀, and a working note with Progress and Stop until the answer", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  tmux("set", "-p", "-t", window, "@agent_state", "working");
  const transcript = path.join(home, ".claude/projects/-work/taken.jsonl");
  fs.writeFileSync(transcript, JSON.stringify({ type: "user", message: { content: "rename   the\nmodule" } }));
  const now = Math.floor(Date.now() / 1000);
  fs.writeFileSync(relay.EVENTS_FILE, [
    { event: "prompt", time: now, transcript_path: transcript, prompt: "something else" },
    { event: "prompt", time: now, transcript_path: transcript, prompt: "rename   the\nmodule" },
  ].map((entry) => `${JSON.stringify(entry)}\n`).join(""));
  const request = `${now - 5}.000100`;
  const state = {
    dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, eventsOffset: 0,
    following: { [window]: { thread: request, request, index: Number(index), name: "work", text: "rename the module", sentAt: Date.now() - 5_000, screen: "", changedAt: 0, taken: false, warned: false } },
  };

  await relay.followUps(state);
  await relay.followUps(state);

  assert.deepEqual(reactions, [{ ts: request, emoji: "eyes" }]);
  assert.equal(state.following[window].taken, true);
  // One working note, where you asked, with Progress and Stop.
  assert.equal(posts.length, 1);
  assert.equal(posts[0].body, "🤖 _Working on it._");
  assert.equal(posts[0].thread, request);
  const note = calls.find((c) => c.method === "chat.postMessage").params.blocks[1].elements;
  assert.deepEqual(note.map((b) => [b.text.text, JSON.parse(b.value).c]), [["Progress", "progress"], ["Stop", "esc"]]);
  assert.equal(state.following[window].working, posts[0].ts);

  // The answer: the note goes, and the turn is posted.
  fs.appendFileSync(transcript, `\n${JSON.stringify({ type: "assistant", message: { content: [{ type: "text", text: "Renamed." }] } })}`);
  fs.appendFileSync(relay.EVENTS_FILE, `${JSON.stringify({ event: "done", time: now + 1, transcript_path: transcript })}\n`);
  await relay.followUps(state);
  assert.deepEqual(calls.filter((c) => c.method === "chat.delete").map((c) => c.params.ts), [posts[0].ts]);
  assert.match(posts.at(-1).body, /is done:[\s\S]*Renamed\./);
  fs.rmSync(relay.EVENTS_FILE);
  tmux("kill-window", "-t", window);
});

test("an idle tab that hasn't taken the message is reported once, with its screen", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  tmux("send-keys", "-t", window, "-l", "Trust this folder? 1. Yes 2. No");
  await new Promise((resolve) => setTimeout(resolve, 200));
  const request = `${Math.floor(Date.now() / 1000) - 40}.000100`;
  const state = {
    dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, eventsOffset: 0,
    following: { [window]: { thread: request, request, index: Number(index), name: "work", text: "start", sentAt: Date.now() - relay.PICKUP_WARN_MS - 1_000, screen: "", changedAt: 0, taken: false, warned: false } },
  };

  await relay.followUps(state);
  await relay.followUps(state);

  assert.equal(posts.length, 1);
  assert.equal(posts[0].thread, request);
  assert.match(posts[0].body, new RegExp(`tab ${index}, work hasn't taken your message after 30 seconds`));
  assert.match(posts[0].body, /On screen now:[\s\S]*Trust this folder\? 1\. Yes 2\. No/);
  assert.deepEqual(reactions, []);
  tmux("kill-window", "-t", window);
});

test("agent-state.sh records a taken prompt for the relay, and nothing for a tool call", () => {
  fs.writeFileSync(relay.EVENTS_FILE, "");
  const hook = path.join(__dirname, "../tmux/agent-state.sh");
  const env = { ...process.env, HOME: home };
  delete env.TMUX;
  delete env.TMUX_PANE;

  execFileSync("bash", [hook, "set", "working"], { env, input: JSON.stringify({ hook_event_name: "UserPromptSubmit", session_id: "s1", transcript_path: "/t.jsonl", cwd: "/w", prompt: "fix it" }) });
  execFileSync("bash", [hook, "set", "working"], { env, input: JSON.stringify({ hook_event_name: "PostToolUse", session_id: "s1", transcript_path: "/t.jsonl", cwd: "/w" }) });

  const events = fs.readFileSync(relay.EVENTS_FILE, "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line));
  assert.equal(events.length, 1);
  assert.equal(events[0].event, "prompt");
  assert.equal(events[0].prompt, "fix it");
  assert.equal(events[0].transcript_path, "/t.jsonl");
  fs.rmSync(relay.EVENTS_FILE);
});

test("esc presses Escape in a working tab and stops following it", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  tmux("set", "-p", "-t", window, "@agent_state", "working");
  const state = {
    dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {},
    following: { [window]: { thread: null, index: Number(index), name: "work", text: "long job", sentAt: Date.now(), screen: "", changedAt: 0 } },
  };

  const linked = `${Math.floor(Date.now() / 1000) - 30}.000500`;
  state.alerts[linked] = { window, index: Number(index), name: "work" };
  await relay.handle(state, { ts: `${Math.floor(Date.now() / 1000)}.000010`, text: "esc", thread_ts: linked });
  await new Promise((resolve) => setTimeout(resolve, 200));

  assert.match(tmux("capture-pane", "-p", "-t", window), /\^\[/);
  assert.match(posts[0].body, new RegExp(`^🤖 Pressed Escape in tab ${index}, `));
  assert.deepEqual(state.following, {});
  assert.equal(state.alerts[posts[0].ts].window, window);
  tmux("kill-window", "-t", window);
});

test("esc under an alert interrupts that tab, and leaves an idle tab alone", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const alert = `${Math.floor(Date.now() / 1000) - 60}.000100`;
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: { [alert]: { window, index: Number(index), name: "work" } }, reported: {}, following: {} };

  await relay.handle(state, { ts: `${Math.floor(Date.now() / 1000)}.000011`, text: "esc", thread_ts: alert });
  tmux("set", "-p", "-t", window, "@agent_state", "waiting");
  await relay.handle(state, { ts: `${Math.floor(Date.now() / 1000)}.000012`, text: "ESC", thread_ts: alert });
  await new Promise((resolve) => setTimeout(resolve, 200));

  assert.match(posts[0].body, /isn't in the middle of anything, so I didn't press Escape/);
  assert.equal(posts[0].thread, alert);
  assert.match(posts[1].body, /Pressed Escape in tab \d+, work/);
  assert.equal((tmux("capture-pane", "-p", "-t", window).match(/\^\[/g) ?? []).length, 1);
  tmux("kill-window", "-t", window);
});

test("a finished turn whose reply isn't in the transcript yet waits for it", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const transcript = path.join(home, ".claude/projects/-work/late.jsonl");
  fs.writeFileSync(transcript, `${JSON.stringify({ type: "user", message: { content: "give me the list" } })}\n`);
  fs.writeFileSync(relay.EVENTS_FILE, `${JSON.stringify({ event: "done", time: Math.floor(Date.now() / 1000), transcript_path: transcript })}\n`);
  const thread = `${Math.floor(Date.now() / 1000) - 5}.000100`;
  const state = {
    dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, eventsOffset: 0,
    following: { [window]: { thread, index: Number(index), name: "work", text: "give me the list", sentAt: Date.now() - 5_000, screen: "", changedAt: 0 } },
  };

  // The Stop hook has fired, but the reply hasn't been written yet.
  await relay.followUps(state);
  assert.equal(posts.length, 0);

  fs.appendFileSync(transcript, `${JSON.stringify({ type: "assistant", message: { content: [{ type: "text", text: "1. Verify" }] } })}\n`);
  await relay.followUps(state);

  assert.equal(posts.length, 1);
  assert.equal(posts[0].body, `🤖 tab ${index}, work is done:\n> give me the list\n\n1. Verify`);
  assert.deepEqual(state.following, {});

  // A reply that never comes is posted as it is once the wait is over.
  fs.writeFileSync(transcript, `${JSON.stringify({ type: "user", message: { content: "and the rest" } })}\n`);
  fs.appendFileSync(relay.EVENTS_FILE, `${JSON.stringify({ event: "done", time: Math.floor(Date.now() / 1000), transcript_path: transcript })}\n`);
  state.following[window] = { thread, index: Number(index), name: "work", text: "and the rest", sentAt: Date.now() - 5_000, screen: "", changedAt: 0 };
  await relay.followUps(state);
  assert.equal(posts.length, 1);
  state.following[window].ended.at -= 61_000;
  await relay.followUps(state);
  assert.equal(posts.length, 2);
  assert.match(posts[1].body, /\(no reply yet\)$/);
  fs.rmSync(relay.EVENTS_FILE);
  tmux("kill-window", "-t", window);
});

test("progress summarizes what a tab is doing without typing into it", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  tmux("set", "-p", "-t", window, "@agent_state", "working");
  const panePid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  fs.writeFileSync(path.join(home, ".claude/sessions", `${panePid}.json`), JSON.stringify({ pid: Number(panePid), sessionId: "busy" }));
  fs.writeFileSync(path.join(home, ".claude/projects/-work/busy.jsonl"), [
    { type: "user", timestamp: new Date(Date.now() - 4 * 60_000).toISOString(), message: { content: "add the progress command" } },
    { type: "assistant", message: { content: [{ type: "text", text: "Reading the relay first." }, { type: "tool_use", id: "t1", name: "Read", input: { file_path: "bin/slack-relay" } }] } },
    { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "t1", content: "..." }] } },
    { type: "assistant", message: { content: [{ type: "tool_use", id: "t2", name: "Bash", input: { command: "node --test", description: "Run the tests" } }] } },
    { type: "user", timestamp: new Date().toISOString(), message: { content: [{ type: "tool_result", tool_use_id: "t2", is_error: true, content: "1 failing" }] } },
  ].map((entry) => JSON.stringify(entry)).join("\n"));
  // A stand-in summarizer that keeps what it was given and answers.
  const summaryBin = path.join(tmp, "summary-bin");
  const given = path.join(tmp, "summary-input.txt");
  fs.mkdirSync(summaryBin, { recursive: true });
  fs.writeFileSync(path.join(summaryBin, "claude"), `#!/bin/sh\ncat > '${given}'\necho '{"result":"• Running the tests, one is failing.","total_cost_usd":0.0002}'\n`, { mode: 0o755 });
  const alert = `${Math.floor(Date.now() / 1000) - 60}.000100`;
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: { [alert]: { window, index: Number(index), name: "work" } }, reported: {}, following: {} };
  const request = `${Math.floor(Date.now() / 1000)}.000013`;
  const path_ = process.env.PATH;
  process.env.PATH = `${summaryBin}:${path_}`;
  try {
    await relay.handle(state, { ts: request, text: "progress", thread_ts: alert });
  } finally {
    process.env.PATH = path_;
  }

  assert.deepEqual(reactions, [{ ts: request, emoji: "eyes" }]);
  assert.equal(posts.length, 1);
  assert.equal(posts[0].thread, alert);
  assert.match(posts[0].body, new RegExp(`^🤖 tab ${index}, work is working, on a turn started 4m ago, last activity just now, 2 tools so far\\.\\n• Running the tests, one is failing\\.$`));
  const input = fs.readFileSync(given, "utf8");
  assert.match(input, /The prompt: add the progress command/);
  assert.match(input, /Read: bin\/slack-relay/);
  assert.match(input, /Bash: Run the tests/);
  assert.match(input, /\(that failed: 1 failing\)/);
  assert.doesNotMatch(tmux("capture-pane", "-p", "-t", window), /progress/);
  assert.deepEqual(state.following, {});
  fs.rmSync(path.join(home, ".claude/sessions", `${panePid}.json`));
  tmux("kill-window", "-t", window);
});

test("progress falls back to the last few steps when the summary fails", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const panePid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  fs.writeFileSync(path.join(home, ".claude/sessions", `${panePid}.json`), JSON.stringify({ pid: Number(panePid), sessionId: "busy2" }));
  fs.writeFileSync(path.join(home, ".claude/projects/-work/busy2.jsonl"), [
    { type: "user", message: { content: "deploy" } },
    { type: "assistant", timestamp: new Date().toISOString(), message: { content: [{ type: "tool_use", id: "t1", name: "Bash", input: { command: "make deploy" } }] } },
  ].map((entry) => JSON.stringify(entry)).join("\n"));
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };

  // The tests' own `claude` always fails.
  const linked = `${Math.floor(Date.now() / 1000) - 30}.000500`;
  state.alerts[linked] = { window, index: Number(index), name: "work" };
  await relay.handle(state, { ts: `${Math.floor(Date.now() / 1000)}.000014`, text: "progress", thread_ts: linked });

  assert.match(posts[0].body, /is idle, last activity just now, 1 tool so far\.\nI couldn't summarize it, so here is what it did last:\n• Bash: make deploy$/);
  fs.rmSync(path.join(home, ".claude/sessions", `${panePid}.json`));
  tmux("kill-window", "-t", window);
});

test("the Home tab is published, then again only when it changes or you open it", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  tmux("set", "-p", "-t", window, "@agent_state", "done");
  tmux("set", "-p", "-t", window, "@agent_since", String(Math.floor(Date.now() / 1000) - 120));
  const home = "1791500000.000300";
  const state = {
    dm: { channel: "D1", user: "U1", url: "https://example.slack.com/" }, threads: { [home]: home }, alerts: {}, reported: {}, following: {},
    homes: { [window]: { ts: home, pid: "1" } },
    // The dashboard message the relay used to pin in the DM.
    dashboard: { ts: "1791500000.000900", body: "" },
  };
  const tabSection = (view) => view.blocks.find((b) => b.type === "section" && b.text.text.startsWith(`*${index}*`));

  await relay.refreshDashboard(state, { quick: true });
  await relay.refreshDashboard(state, { quick: true });
  assert.deepEqual(calls.filter((c) => c.method === "chat.delete").map((c) => c.params.ts), ["1791500000.000900"], "the old message, once");
  assert.equal(state.dashboard, undefined);
  assert.equal(views.length, 1, "nothing changed the second time");
  assert.equal(calls.find((c) => c.method === "views.publish").params.user_id, "U1");
  assert.equal(views[0].type, "home");
  assert.match(tabSection(views[0]).text.text, /✅ finished for 2m/);
  // Under it, only Open; its thread and Close are on the screen it opens.
  const row = views[0].blocks[views[0].blocks.indexOf(tabSection(views[0])) + 1];
  assert.equal(row.type, "actions");
  assert.deepEqual(row.elements.map((b) => b.text.text), ["Open"]);
  const screenButtons = relay.tabScreen(state, relay.claudeTabs().find((t) => t.id === window)).blocks.find((b) => b.type === "actions").elements;
  assert.equal(screenButtons[2].text.text, "Go to thread");
  assert.equal(screenButtons[2].url, "https://example.slack.com/archives/D1/p1791500000000300?thread_ts=1791500000.000300&cid=D1");
  assert.equal(screenButtons.at(-1).style, "danger");
  assert.equal(screenButtons.at(-1).confirm.confirm.text, "Close");

  tmux("set", "-p", "-t", window, "@agent_state", "waiting");
  await relay.refreshDashboard(state, { quick: true });
  assert.equal(views.length, 2);
  assert.match(tabSection(views[1]).text.text, /🔴 \*waiting on you\*/);

  await relay.refreshDashboard(state, { quick: true, opened: true });
  assert.equal(views.length, 3, "opening the Home tab shows it as it is now");
  tmux("kill-window", "-t", window);
});

test("optionsIn reads a prompt's numbered choices from the bottom of the screen", () => {
  const permission = [
    "● I'll push the branch.",
    "1. this numbered list is Claude's reply, not the prompt",
    "2. so it is ignored",
    "────────────────────────────────────────",
    " Bash command",
    "   git push origin feature",
    " Do you want to proceed?",
    " ❯ 1. Yes",
    "   2. Yes, and don't ask again for git push commands in /work",
    "   3. No, and tell Claude what to do differently (esc)",
  ].join("\n");
  assert.deepEqual(relay.optionsIn(permission), {
    question: "Bash command git push origin feature Do you want to proceed?",
    options: [
      { n: 1, label: "Yes" },
      { n: 2, label: "Yes, and don't ask again for git push commands in /work" },
      { n: 3, label: "No, and tell Claude what to do differently (esc)" },
    ],
  });

  // A question with a description under each option, in a box.
  const question = [
    "╭──────────────────────────╮",
    "│ Which database for tests? │",
    "│ ❯ 1. Postgres             │",
    "│      The same as production│",
    "│   2. SQLite               │",
    "│      Faster, but different│",
    "╰──────────────────────────╯",
  ].join("\n");
  assert.deepEqual(relay.optionsIn(question).options, [{ n: 1, label: "Postgres" }, { n: 2, label: "SQLite" }]);
  assert.match(relay.optionsIn(question).question, /Which database for tests\?/);

  assert.equal(relay.optionsIn("● Done.\n1. one thing\nthe end"), undefined);
});

test("an answer to a waiting tab's choice presses just the number", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  ["Do you want to proceed?", "❯ 1. Yes", "  2. Yes, and don't ask again", "  3. No (esc)"].forEach((line) => {
    tmux("send-keys", "-t", window, "-l", line);
    tmux("send-keys", "-t", window, "Enter");
  });
  await new Promise((resolve) => setTimeout(resolve, 200));
  tmux("set", "-p", "-t", window, "@agent_state", "waiting");
  const alert = `${Math.floor(Date.now() / 1000) - 30}.000100`;
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: { [alert]: { window, index: Number(index), name: "work" } }, reported: {}, following: {} };

  await relay.handle(state, { ts: `${Math.floor(Date.now() / 1000)}.000016`, text: "yes, and", thread_ts: alert });
  await new Promise((resolve) => setTimeout(resolve, 200));

  const lines = tmux("capture-pane", "-p", "-t", window).split("\n").filter((line) => line.trim());
  assert.equal(lines.at(-1), "2");
  assert.equal(lines.filter((line) => line === "2").length, 1, "no Enter after the number");
  assert.match(posts[0].body, new RegExp(`^🤖 Chose 2\\. Yes, and don't ask again in tab ${index}, work\\.`));
  assert.equal(state.following[window].taken, true);
  tmux("kill-window", "-t", window);
});

test("milestones ping a tab's thread on a commit, a push and failing tests", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const panePid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  fs.writeFileSync(path.join(home, ".claude/sessions", `${panePid}.json`), JSON.stringify({ pid: Number(panePid), sessionId: "shipping" }));
  const transcript = path.join(home, ".claude/projects/-work/shipping.jsonl");
  fs.writeFileSync(transcript, `${JSON.stringify({ type: "user", message: { content: "an old turn" } })}\n`);
  const thread = `${Math.floor(Date.now() / 1000) - 60}.000100`;
  const state = {
    dm: { channel: "D1", user: "U1" }, reported: {}, following: {},
    threads: { [thread]: thread },
    alerts: { [thread]: { window, index: Number(index), name: "work" } },
  };
  try {
    // First sight: followed from here, the old turn is not news.
    await relay.milestones(state);
    assert.equal(posts.length, 0);

    fs.appendFileSync(transcript, [
      { type: "assistant", cwd: tmp, message: { content: [{ type: "tool_use", id: "c1", name: "Bash", input: { command: "git commit -m 'Add two'" } }] } },
      { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "c1", content: "[feature abc1234] Add two\n 1 file changed" }] } },
      { type: "assistant", message: { content: [{ type: "tool_use", id: "t1", name: "Bash", input: { command: "pnpm test" } }] } },
      { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "t1", is_error: true, content: "FAIL app.test.js\nTests: 1 failed, 4 passed" }] } },
      { type: "assistant", message: { content: [{ type: "tool_use", id: "t2", name: "Bash", input: { command: "pnpm test" } }] } },
      { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "t2", content: "Tests: 5 passed" }] } },
      { type: "assistant", message: { content: [{ type: "tool_use", id: "p1", name: "Bash", input: { command: "git push -u origin feature" } }] } },
      { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "p1", content: "To github.com:o/r.git\n * [new branch]      feature -> feature" }] } },
    ].map((entry) => `${JSON.stringify(entry)}\n`).join(""));
    await relay.milestones(state);

    // Pings name the tab as it is called now.
    const name = tmux("display", "-p", "-t", window, "#{window_name}");
    assert.deepEqual(posts.slice(1).map((p) => p.body), [
      `🤖 tab ${index}, ${name}: committed \`abc1234\` Add two on \`feature\``,
      `🤖 tab ${index}, ${name}: pushed \`feature\``,
    ], "a failing test run isn't news");
    // In the tab's home thread, which the first of them started.
    const homeThread = state.homes[window].ts;
    assert.match(posts[0].body, /This is its thread/);
    posts.slice(1).forEach((p) => assert.equal(p.thread, homeThread));
    assert.deepEqual(posts.slice(1).map((p) => p.broadcast), [false, false]);
  } finally {
    fs.rmSync(path.join(home, ".claude/sessions", `${panePid}.json`));
    tmux("kill-window", "-t", window);
  }
});

test("the Home tab has a line for every tab, finished ones too, written again only when it changes", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const panePid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  tmux("set", "-p", "-t", window, "@agent_state", "done");
  tmux("set", "-p", "-t", window, "@agent_since", String(Math.floor(Date.now() / 1000) - 60));
  fs.writeFileSync(path.join(home, ".claude/sessions", `${panePid}.json`), JSON.stringify({ pid: Number(panePid), sessionId: "summed" }));
  const transcript = path.join(home, ".claude/projects/-work/summed.jsonl");
  fs.writeFileSync(transcript, `${JSON.stringify({ type: "user", timestamp: new Date().toISOString(), message: { content: "write the explainer" } })}\n`);
  // A claude that answers with one line and counts how often it was asked.
  const asked = path.join(tmp, "asked");
  const stub = path.join(fakeBin, "claude");
  const failing = fs.readFileSync(stub, "utf8");
  fs.writeFileSync(stub, `#!/bin/sh\ncat >/dev/null\necho x >> '${asked}'\necho '{"result":"Published the explainer; nothing left to do.","total_cost_usd":0.0002}'\n`);
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  const line = () => views.at(-1).blocks.find((b) => b.type === "section" && b.text.text.startsWith(`*${index}*`)).text.text;
  fs.rmSync(relay.USAGE_FILE, { force: true });
  try {
    await relay.refreshDashboard(state, { heartbeat: true });
    // What it cost, from what claude reported, at the bottom.
    assert.equal(views.at(-1).blocks.at(-1).elements[0].text, "Haiku today: 1 call, $0.00");
    assert.equal(relay.haikuToday().calls, 1);
    assert.equal(relay.haikuToday().cost, 0.0002);
    assert.match(line(), /✅ finished[\s\S]*_Published the explainer; nothing left to do\._/);
    assert.equal(fs.readFileSync(asked, "utf8").split("\n").filter(Boolean).length, 1);

    // Nothing changed: no new line. Kept in the state file, so a restart doesn't ask again either.
    await relay.refreshDashboard(state, { heartbeat: true });
    assert.equal(state.doing[window].pid, panePid);
    await relay.refreshDashboard(JSON.parse(JSON.stringify(state)), { heartbeat: true });
    assert.equal(fs.readFileSync(asked, "utf8").split("\n").filter(Boolean).length, 1);

    // Its state changed: written again, once DOING_EVERY_MS have passed.
    tmux("set", "-p", "-t", window, "@agent_state", "working");
    await relay.refreshDashboard(state, { heartbeat: true });
    assert.equal(fs.readFileSync(asked, "utf8").split("\n").filter(Boolean).length, 1);
    state.doing[window].at -= relay.DOING_EVERY_MS;
    await relay.refreshDashboard(state, { heartbeat: true });
    assert.equal(fs.readFileSync(asked, "utf8").split("\n").filter(Boolean).length, 2);

    // It worked on, looked at again within DOING_EVERY_MS: not written again
    // until that has passed.
    fs.appendFileSync(transcript, `${JSON.stringify({ type: "user", timestamp: new Date(Date.now() + 5_000).toISOString(), message: { content: "and the diagram" } })}\n`);
    await relay.refreshDashboard(state, { heartbeat: true });
    assert.equal(fs.readFileSync(asked, "utf8").split("\n").filter(Boolean).length, 2);
    state.doing[window].at -= relay.DOING_EVERY_MS;
    await relay.refreshDashboard(state, { heartbeat: true });
    assert.equal(fs.readFileSync(asked, "utf8").split("\n").filter(Boolean).length, 3);

    // A closed tab's line goes.
    tmux("kill-window", "-t", window);
    await relay.refreshDashboard(state, { heartbeat: true });
    assert.equal(state.doing[window], undefined);
  } finally {
    fs.writeFileSync(stub, failing);
    fs.rmSync(asked, { force: true });
    fs.rmSync(transcript, { force: true });
    fs.rmSync(path.join(home, ".claude/sessions", `${panePid}.json`), { force: true });
    try { tmux("kill-window", "-t", window); } catch {}
  }
});

test("the Home tab puts what needs you first: waiting, then finished, longest first, then working, then idle", () => {
  const row = (index, state, since) => ({ id: `@${index}`, index, name: `tab${index}`, state, since: String(since), pid: "1", choices: [] });
  const rows = [row(1, "idle", 100), row(2, "working", 200), row(3, "done", 300), row(4, "waiting", 400), row(5, "done", 250), row(6, "waiting", 350), row(7, "working", 150)];
  const view = relay.dashboardView({ dm: { channel: "D1" }, closedHomes: {} }, rows);
  const order = view.blocks.flatMap((block) =>
    block.type === "header" ? [block.text.text] : block.type === "section" && /^\*\d+\*/.test(block.text.text) ? [Number(block.text.text.match(/^\*(\d+)\*/)[1])] : [],
  );
  assert.deepEqual(order, ["Claude tabs", "Needs you (4)", 6, 4, 5, 3, "Working (2)", 2, 7, "Idle (1)", 1]);

  // A group with nothing in it isn't shown.
  const calm = relay.dashboardView({ dm: { channel: "D1" }, closedHomes: {} }, [row(1, "idle", 100)]);
  assert.deepEqual(calm.blocks.filter((block) => block.type === "header").map((block) => block.text.text), ["Claude tabs", "Idle (1)"]);
});

test("a stopped relay marks the Home tab out of date", async () => {
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };

  await relay.refreshDashboard(state, { quick: true, stopped: true });

  assert.match(views[0].blocks.find((b) => b.block_id === "checkin").text.text, /^⚪ The relay stopped at .+, so this is out of date/);
  assert.equal(views[0].blocks.find((b) => b.block_id === "checkin").accessory, undefined, "no Refresh with nothing to refresh it");
});

test("a home thread stays watched past the usual limits", () => {
  const now = Math.floor(Date.now() / 1000);
  const homeThread = `${now - 3 * 24 * 3600}.000100`;
  const state = {
    threads: { [homeThread]: homeThread },
    alerts: { [homeThread]: { window: "@5" } },
    homes: { "@5": { ts: homeThread, pid: "1" } },
  };
  Array.from({ length: 40 }, (_, i) => `${now - i}.000100`).forEach((ts) => relay.watchThread(state, ts));

  assert.ok(state.threads[homeThread]);
  assert.ok(state.alerts[homeThread]);
  assert.equal(Object.keys(state.threads).length, 31);
});

test("the answer to a request sent from the main DM goes to the main DM too, with no extra line", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const transcript = path.join(home, ".claude/projects/-work/homed.jsonl");
  fs.writeFileSync(transcript, [
    { type: "user", message: { content: "ship it" } },
    { type: "assistant", message: { content: [{ type: "text", text: "Shipped." }] } },
  ].map((entry) => JSON.stringify(entry)).join("\n"));
  fs.writeFileSync(relay.EVENTS_FILE, `${JSON.stringify({ event: "done", time: Math.floor(Date.now() / 1000), transcript_path: transcript })}\n`);
  const homeThread = `${Math.floor(Date.now() / 1000) - 60}.000100`;
  const state = {
    dm: { channel: "D1", user: "U1" }, threads: { [homeThread]: homeThread }, alerts: {}, reported: {}, eventsOffset: 0,
    homes: { [window]: { ts: homeThread, pid: "1" } },
    following: { [window]: { thread: homeThread, index: Number(index), name: "work", text: "ship it", sentAt: Date.now() - 5_000, screen: "", changedAt: 0, broadcast: true } },
  };

  await relay.followUps(state);

  assert.equal(posts.length, 1);
  assert.equal(posts[0].body, `🤖 tab ${index}, work is done:\n> ship it\n\nShipped.`);
  assert.equal(posts[0].thread, homeThread);
  assert.equal(posts[0].broadcast, true);
  fs.rmSync(relay.EVENTS_FILE);
  tmux("kill-window", "-t", window);
});

test("progress all summarizes every tab at once, or lists their facts if the summary fails", async () => {
  const [first, firstIndex] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  const [second, secondIndex] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  tmux("set", "-p", "-t", first, "@agent_state", "waiting");
  tmux("set", "-p", "-t", second, "@agent_state", "working");
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  const request = `${Math.floor(Date.now() / 1000)}.000018`;

  // The tests' own claude always fails, so this is the fallback.
  await relay.handle(state, { ts: request, text: "progress all", thread_ts: null });

  assert.deepEqual(reactions, [{ ts: request, emoji: "eyes" }]);
  assert.match(posts[0].body, /^🤖 \d+ Claude tabs?:\nI couldn't summarize them, so here is where each one is:\n/);
  assert.match(posts[0].body, new RegExp(`• tab ${firstIndex}, [^\\n]* is \\*\\*waiting on you\\*\\*`));
  assert.match(posts[0].body, new RegExp(`• tab ${secondIndex}, [^\\n]* is working`));

  const fleetBin = path.join(tmp, "fleet-bin");
  const given = path.join(tmp, "fleet-input.txt");
  fs.mkdirSync(fleetBin, { recursive: true });
  fs.writeFileSync(path.join(fleetBin, "claude"), `#!/bin/sh\ncat > '${given}'\necho '{"result":"• Tab ${firstIndex}, x: waiting on you to approve a push.","total_cost_usd":0.0002}'\n`, { mode: 0o755 });
  const path_ = process.env.PATH;
  process.env.PATH = `${fleetBin}:${path_}`;
  try {
    await relay.handle(state, { ts: `${Math.floor(Date.now() / 1000)}.000019`, text: "progress", thread_ts: null });
  } finally {
    process.env.PATH = path_;
  }

  assert.match(posts[1].body, new RegExp(`\\n• Tab ${firstIndex}, x: waiting on you to approve a push\\.$`));
  const input = fs.readFileSync(given, "utf8");
  assert.match(input, new RegExp(`## tab ${firstIndex}, [^\\n]* is \\*\\*waiting on you\\*\\*`));
  assert.match(input, new RegExp(`## tab ${secondIndex}, [^\\n]* is working`));
  tmux("kill-window", "-t", first);
  tmux("kill-window", "-t", second);
});

test("a working tab with nothing new for ten minutes is noted once per stall", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  tmux("set", "-p", "-t", window, "@agent_state", "working");
  tmux("send-keys", "-t", window, "-l", "✻ Running pnpm test… (11m 2s)");
  const panePid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  fs.writeFileSync(path.join(home, ".claude/sessions", `${panePid}.json`), JSON.stringify({ pid: Number(panePid), sessionId: "stalled" }));
  const transcript = path.join(home, ".claude/projects/-work/stalled.jsonl");
  const stale = new Date(Date.now() - relay.STUCK_MS - 60_000).toISOString();
  fs.writeFileSync(transcript, [
    { type: "user", timestamp: stale, message: { content: "run the suite" } },
    { type: "assistant", timestamp: stale, message: { content: [{ type: "tool_use", id: "b1", name: "Bash", input: { command: "pnpm test" } }] } },
    // Bookkeeping Claude Code writes to an idle session: not activity.
    { type: "pr-link", timestamp: new Date().toISOString() },
  ].map((entry) => JSON.stringify(entry)).join("\n"));
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  try {
    await relay.stuckTabs(state);
    await relay.stuckTabs(state);

    // The home thread, the note in it, and a line in the main DM, once.
    assert.equal(posts.length, 3);
    assert.match(posts[1].body, new RegExp(`^🤖 tab ${index}, [^:]*: it's marked working, but nothing has happened for 11m\\.`));
    assert.match(posts[1].body, /What it did last:\n• Bash: pnpm test/);
    assert.match(posts[1].body, /On screen now:\n```\n[\s\S]*Running pnpm test/);
    assert.equal(posts[2].broadcast, true);
    assert.match(posts[2].body, /may be stuck: nothing new for 11m\.$/);

    // It moves on, then stalls again: noted again.
    fs.appendFileSync(transcript, `\n${JSON.stringify({ type: "user", timestamp: new Date(Date.now() - relay.STUCK_MS - 30_000).toISOString(), message: { content: [{ type: "tool_result", tool_use_id: "b1", content: "still going" }] } })}`);
    await relay.stuckTabs(state);
    assert.equal(posts.length, 5);

    // Not working any more: nothing.
    tmux("set", "-p", "-t", window, "@agent_state", "done");
    await relay.stuckTabs(state);
    assert.equal(posts.length, 5);
    assert.deepEqual(state.stuck, {});

    // Idle a long while, then sent a prompt: marked working before the prompt
    // reaches the transcript, which is no stall.
    tmux("set", "-p", "-t", window, "@agent_state", "working");
    tmux("set", "-p", "-t", window, "@agent_since", String(Math.floor(Date.now() / 1000)));
    await relay.stuckTabs(state);
    assert.equal(posts.length, 5, "not stuck: it only just started");
  } finally {
    fs.rmSync(path.join(home, ".claude/sessions", `${panePid}.json`));
    tmux("kill-window", "-t", window);
  }
});

test("a turn ending at a tool call isn't reported until its last words are written", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const transcript = path.join(home, ".claude/projects/-work/midturn.jsonl");
  fs.writeFileSync(transcript, [
    { type: "user", message: { content: "is idle mode worth it?" } },
    { type: "assistant", message: { content: [{ type: "text", text: "Before answering, I'll check the log." }, { type: "tool_use", id: "g1", name: "Bash", input: { command: "grep" } }] } },
    { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "g1", content: "no matches" }] } },
  ].map((entry) => `${JSON.stringify(entry)}\n`).join(""));
  fs.writeFileSync(relay.EVENTS_FILE, `${JSON.stringify({ event: "done", time: Math.floor(Date.now() / 1000), transcript_path: transcript })}\n`);
  const thread = `${Math.floor(Date.now() / 1000) - 5}.000100`;
  const state = {
    dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, eventsOffset: 0,
    following: { [window]: { thread, index: Number(index), name: "work", text: "is idle mode worth it?", sentAt: Date.now() - 5_000, screen: "", changedAt: 0 } },
  };

  // The Stop hook has fired, but the transcript stops after the tool's result.
  await relay.followUps(state);
  assert.equal(posts.length, 0);
  assert.equal(relay.turnWritten(transcript), false);

  fs.appendFileSync(transcript, `${JSON.stringify({ type: "assistant", message: { content: [{ type: "text", text: "Mostly, yes." }] } })}\n`);
  assert.equal(relay.turnWritten(transcript), true);
  await relay.followUps(state);

  assert.equal(posts.length, 1);
  assert.match(posts[0].body, /Before answering, I'll check the log\.\n\nMostly, yes\.\n\n_\(ran 1 tool\)_$/);
  fs.rmSync(relay.EVENTS_FILE);
  tmux("kill-window", "-t", window);
});

test("agent-state.sh marks the tab a background job was parked from", async () => {
  const [window] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const panePid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  const sessions = path.join(home, ".claude/sessions");
  // The tab's Claude is parked into job J; the job runs elsewhere, with no tmux.
  fs.writeFileSync(path.join(sessions, `${panePid}.json`), JSON.stringify({ pid: Number(panePid), sessionId: "front", kind: "interactive", parkedJobId: "J" }));
  fs.writeFileSync(path.join(sessions, "900010.json"), JSON.stringify({ pid: 900010, sessionId: "background", kind: "bg", jobId: "J" }));
  const hook = path.join(__dirname, "../tmux/agent-state.sh");
  // TMUX names the tests' own server; there is no TMUX_PANE, as in a job.
  const env = { ...process.env, HOME: home };
  delete env.TMUX_PANE;
  try {
    execFileSync("bash", [hook, "set", "working"], { env, input: JSON.stringify({ hook_event_name: "PostToolUse", session_id: "background" }) });
    assert.equal(tmux("display", "-p", "-t", window, "#{@agent_state}"), "working");

    // A session parked nowhere is left alone.
    execFileSync("bash", [hook, "set", "waiting"], { env, input: JSON.stringify({ session_id: "somewhere-else" }) });
    assert.equal(tmux("display", "-p", "-t", window, "#{@agent_state}"), "working");
  } finally {
    ["900010", panePid].forEach((pid) => fs.rmSync(path.join(sessions, `${pid}.json`)));
    tmux("kill-window", "-t", window);
  }
});

test("agent-state.sh ignores a headless Claude's hooks, though it names the tab's pane", async () => {
  const window = tmux("new-window", "-d", "-P", "-F", "#{window_id}", fakeClaude);
  await new Promise((resolve) => setTimeout(resolve, 300));
  const [pane, panePid] = tmux("display", "-p", "-t", window, "#{pane_id}\t#{pane_pid}").split("\t");
  const sessions = path.join(home, ".claude/sessions");
  fs.writeFileSync(path.join(sessions, `${panePid}.json`), JSON.stringify({ pid: Number(panePid), sessionId: "own", entrypoint: "cli" }));
  // A review loop's script starts a headless Claude, which inherits TMUX_PANE.
  fs.writeFileSync(path.join(sessions, "900020.json"), JSON.stringify({ pid: 900020, sessionId: "headless", entrypoint: "sdk-cli" }));
  const hook = path.join(__dirname, "../tmux/agent-state.sh");
  const env = { ...process.env, HOME: home, TMUX_PANE: pane };
  try {
    execFileSync("bash", [hook, "set", "working"], { env, input: JSON.stringify({ hook_event_name: "PostToolUse", session_id: "own" }) });
    execFileSync("bash", [hook, "set", "done"], { env, input: JSON.stringify({ session_id: "headless" }) });
    assert.equal(tmux("display", "-p", "-t", window, "#{@agent_state}"), "working", "the headless turn ending changes nothing");
  } finally {
    ["900020", panePid].forEach((pid) => fs.rmSync(path.join(sessions, `${pid}.json`)));
    tmux("kill-window", "-t", window);
  }
});

test("a lagging transcript is waited for until its last text is the hook's final message", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const transcript = path.join(home, ".claude/projects/-work/lagging.jsonl");
  // Written up to a text whose tool call hasn't been written yet: it ends in
  // text, but not the turn's.
  fs.writeFileSync(transcript, [
    { type: "user", message: { content: "reorder the commits" } },
    { type: "assistant", message: { content: [{ type: "text", text: "Moving main now:" }] } },
  ].map((entry) => `${JSON.stringify(entry)}\n`).join(""));
  fs.writeFileSync(relay.EVENTS_FILE, `${JSON.stringify({ event: "done", time: Math.floor(Date.now() / 1000), transcript_path: transcript, last: "Both are done." })}\n`);
  const thread = `${Math.floor(Date.now() / 1000) - 5}.000100`;
  const state = {
    dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, eventsOffset: 0,
    following: { [window]: { thread, index: Number(index), name: "work", text: "reorder the commits", sentAt: Date.now() - 5_000, screen: "", changedAt: 0 } },
  };

  await relay.followUps(state);
  assert.equal(posts.length, 0, "ends in text, but not the final message");

  fs.appendFileSync(transcript, [
    { type: "assistant", message: { content: [{ type: "tool_use", id: "m1", name: "Bash", input: { command: "git branch -f main" } }] } },
    { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "m1", content: "" }] } },
    { type: "assistant", message: { content: [{ type: "text", text: "Both are done." }] } },
  ].map((entry) => `${JSON.stringify(entry)}\n`).join(""));
  await relay.followUps(state);

  assert.equal(posts.length, 1);
  assert.match(posts[0].body, /Moving main now:\n\nBoth are done\.\n\n_\(ran 1 tool\)_$/);
  fs.rmSync(relay.EVENTS_FILE);
  tmux("kill-window", "-t", window);
});

test("a transcript that never catches up is posted with the hook's final message added", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const transcript = path.join(home, ".claude/projects/-work/stuck-behind.jsonl");
  fs.writeFileSync(transcript, [
    { type: "user", message: { content: "summarize it" } },
    { type: "assistant", message: { content: [{ type: "text", text: "Reading the log first." }] } },
  ].map((entry) => `${JSON.stringify(entry)}\n`).join(""));
  fs.writeFileSync(relay.EVENTS_FILE, `${JSON.stringify({ event: "done", time: Math.floor(Date.now() / 1000), transcript_path: transcript, last: "Here is the summary." })}\n`);
  const state = {
    dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, eventsOffset: 0,
    following: { [window]: { thread: null, index: Number(index), name: "work", text: "summarize it", sentAt: Date.now() - 5_000, screen: "", changedAt: 0 } },
  };

  await relay.followUps(state);
  assert.equal(posts.length, 0);
  state.following[window].ended.at -= 61_000;
  await relay.followUps(state);

  assert.equal(posts[0].body, `🤖 tab ${index}, work is done:\n> summarize it\n\nReading the log first.\n\nHere is the summary.`);
  fs.rmSync(relay.EVENTS_FILE);
  tmux("kill-window", "-t", window);
});

test("agent-state.sh gives the relay a finished turn's final message", () => {
  fs.writeFileSync(relay.EVENTS_FILE, "");
  const env = { ...process.env, HOME: home };
  delete env.TMUX;
  delete env.TMUX_PANE;

  execFileSync("bash", [path.join(__dirname, "../tmux/agent-state.sh"), "set", "done"], {
    env,
    input: JSON.stringify({ hook_event_name: "Stop", session_id: "s2", transcript_path: "/t2.jsonl", cwd: "/w", last_assistant_message: "All done." }),
  });

  const [event] = fs.readFileSync(relay.EVENTS_FILE, "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line));
  assert.equal(event.event, "done");
  assert.equal(event.last, "All done.");
  fs.rmSync(relay.EVENTS_FILE);
});

test("a test run is no milestone, and a command that never ran isn't a failure", () => {
  const line = relay.milestoneLine;
  assert.deepEqual(relay.milestoneKinds("pnpm test"), []);
  assert.deepEqual(relay.milestoneKinds("node --test test/*.test.js | grep '^#'"), []);
  // Blocked by a hook, declined, or not even parsed: nothing ran.
  const blocked = "PreToolUse:Bash hook error: [bash ~/.claude/hooks/enforce-git-hooks-run.sh]: Do not skip git hooks";
  assert.equal(line("commit", blocked, true, "git commit -n -m x"), undefined);
  assert.equal(line("push", "The user doesn't want to proceed with this tool use.", true, "git push"), undefined);
  // A push into a local repository, or a quiet one that says nothing of where
  // it went, is no news; one to GitHub is.
  assert.equal(line("push", "To /tmp/test/origin.git\n * [new branch]      main -> main", false, "git push origin main"), undefined);
  assert.equal(line("push", "", false, "git -C clone push -q origin main"), undefined);
  assert.equal(line("push", "To github.com:o/r.git\n   abc..def  main -> main", false, "git push"), "pushed `main`");
  assert.equal(line("commit", "(eval):14: unmatched `", true, "node -e '...' && git commit -m x"), undefined);
  // A commit git itself refused is still one that failed.
  assert.equal(line("commit", "pre-commit: autoMode is staged\nerror: the hook refused", true, "git commit -m x"), "a commit failed: error: the hook refused");
});

test("milestones count git's own push and commit, not a subcommand's, nor text that mentions them", () => {
  const kinds = relay.milestoneKinds;

  assert.deepEqual(kinds("git push -u origin feature"), ["push"]);
  assert.deepEqual(kinds("git -C ~/repo push"), ["push"]);
  assert.deepEqual(kinds("git -c user.name=x commit -m 'Fix'"), ["commit"]);
  assert.deepEqual(kinds("git add . && git commit -m x && git push"), ["push", "commit"]);
  assert.deepEqual(kinds("git stash push -q -m reorder"), []);
  assert.deepEqual(kinds("git stash push -m x && git commit --amend"), ["commit"]);
  assert.deepEqual(kinds("cd ~/repo && git push"), ["push"]);
  assert.deepEqual(kinds("x=$(git commit -m y)"), ["commit"]);
  assert.deepEqual(kinds("gh pr create --fill"), ["pr"]);
  // Only mentioned: in a quoted string, a heredoc's body, or a grep.
  assert.deepEqual(kinds('echo "Allow git push?"'), []);
  assert.deepEqual(kinds("cat > t.js <<'EOF'\nconst prompt = 'Allow git push?';\ngit push\nEOF\nnode t.js"), []);
  assert.deepEqual(kinds("git log | grep 'git push'"), []);
  assert.deepEqual(kinds("cat <<EOF > notes\ngh pr create\nEOF\ngit commit -m notes"), ["commit"]);
});

test("optionsIn ignores a file preview above a real Write prompt", () => {
  // Captured from Claude Code's prompt to create a file.
  const screen = [
    "⏺ Write(hello.txt)",
    "─".repeat(80),
    " Create file",
    " hello.txt",
    "╌".repeat(80),
    "  1 hi",
    "╌".repeat(80),
    " Do you want to create hello.txt?",
    " ❯ 1. Yes",
    "   2. Yes, and allow Claude to edit files in its ~/.claude folder for this session",
    "   3. No",
    " Esc to cancel · Tab to amend",
  ].join("\n");

  assert.deepEqual(relay.optionsIn(screen), {
    question: "Do you want to create hello.txt?",
    options: [
      { n: 1, label: "Yes" },
      { n: 2, label: "Yes, and allow Claude to edit files in its ~/.claude folder for this session" },
      { n: 3, label: "No" },
    ],
  });
});

test("agent-state.sh keeps a wait's start time when it is marked waiting again", async () => {
  const [window] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const pane = tmux("display", "-p", "-t", window, "#{pane_id}");
  const env = { ...process.env, HOME: home, TMUX_PANE: pane };
  const hook = path.join(__dirname, "../tmux/agent-state.sh");
  try {
    execFileSync("bash", [hook, "set", "waiting"], { env, input: "{}" });
    const since = tmux("display", "-p", "-t", pane, "#{@agent_since}");
    await new Promise((resolve) => setTimeout(resolve, 1_100));
    // The Notification hook, seconds later, for the same prompt.
    execFileSync("bash", [hook, "set", "waiting"], { env, input: "{}" });
    assert.equal(tmux("display", "-p", "-t", pane, "#{@agent_since}"), since);

    execFileSync("bash", [hook, "set", "working"], { env, input: "{}" });
    assert.equal(tmux("display", "-p", "-t", pane, "#{@agent_state}"), "working");
    assert.notEqual(tmux("display", "-p", "-t", pane, "#{@agent_since}"), since);
  } finally {
    tmux("kill-window", "-t", window);
  }
});

test("screen cuts Claude Code's full-width rules to a few characters", async () => {
  const [window] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  tmux("send-keys", "-t", window, "-l", `${"─".repeat(150)} and ${"╌".repeat(40)} but ab-cd stays`);
  await new Promise((resolve) => setTimeout(resolve, 200));

  assert.equal(relay.screen(window), "─── and ╌╌╌ but ab-cd stays");
  tmux("kill-window", "-t", window);
});

test("dialogIn reads the dialogs Claude Code shows before a session starts", () => {
  // Both captured from real tabs.
  const trust = [
    "─".repeat(60),
    " Accessing workspace:",
    " /work/new-repo",
    " Quick safety check: Is this a project you created or one you trust?",
    " Claude Code'll be able to read, edit, and execute files here.",
    " Security guide",
    " ❯ No, exit",
    "   Yes, I trust this folder",
    " Enter to confirm · Esc to cancel",
  ].join("\n");
  assert.deepEqual(relay.dialogIn(trust), {
    question: "Quick safety check: Is this a project you created or one you trust? Claude Code'll be able to read, edit, and execute files here. Security guide",
    options: [{ n: 1, label: "No, exit" }, { n: 2, label: "Yes, I trust this folder" }],
    cursor: 1,
  });

  const mcp = [
    "  New MCP server found in this project: some-server",
    "  MCP servers may execute code or access system resources. All tool calls require approval.",
    "    Use this MCP server",
    "    Use this and all future MCP servers in this project",
    "  ❯ Continue without using this MCP server",
    "  Enter to confirm · Esc to cancel",
  ].join("\n");
  const found = relay.dialogIn(mcp);
  assert.deepEqual(found.options.map((o) => o.label), [
    "Use this MCP server",
    "Use this and all future MCP servers in this project",
    "Continue without using this MCP server",
  ]);
  assert.equal(found.cursor, 3);

  // Not at the bottom of the screen: an old dialog, already answered.
  assert.equal(relay.dialogIn(`${trust}\n● Working on it\n> next\n✻ Thinking`), undefined);
});

test("a dialog before a session starts is alerted once and answered with arrow keys", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  [" Is this a project you trust?", " ❯ No, exit", "   Yes, I trust this folder", " Enter to confirm · Esc to cancel"].forEach((line) => {
    tmux("send-keys", "-t", window, "-l", line);
    tmux("send-keys", "-t", window, "Enter");
  });
  await new Promise((resolve) => setTimeout(resolve, 200));
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };

  await relay.alertTabs(state);
  await relay.alertTabs(state);

  // The tab's thread, the alert in it, and a notice in the main DM, once.
  assert.equal(posts.length, 3);
  assert.match(posts[1].body, new RegExp(`^🤖 \\*\\*Tab ${index}, .*\\*\\* is showing a dialog before its session starts`));
  assert.match(posts[1].body, /\*\*Is this a project you trust\?\*\*\n1\. No, exit \(selected\)\n2\. Yes, I trust this folder\n/);
  assert.equal(posts[2].broadcast, true);

  await relay.handle(state, { ts: `${Math.floor(Date.now() / 1000)}.000020`, text: "yes", thread_ts: posts[1].thread });
  await new Promise((resolve) => setTimeout(resolve, 200));

  assert.match(posts.at(-1).body, /^🤖 Chose "Yes, I trust this folder" in tab/);
  // cat shows the Down arrow as ^[[B; the Enter after it moves the cursor to
  // a new line below.
  const lines = tmux("capture-pane", "-p", "-t", window).split("\n");
  const arrow = lines.findIndex((line) => line === "^[[B");
  assert.ok(arrow >= 0);
  assert.equal(lines.filter((line) => line.includes("^[[B")).length, 1);
  assert.ok(Number(tmux("display", "-p", "-t", window, "#{cursor_y}")) > arrow);
  tmux("kill-window", "-t", window);
});

test("a tab with no state is checked for a dialog every few seconds, not every second", async () => {
  const [window] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };

  // Checked while the screen is still blank, then the dialog appears.
  await relay.alertTabs(state);
  [" Is this a project you trust?", " ❯ No, exit", "   Yes, I trust this folder", " Enter to confirm · Esc to cancel"].forEach((line) => {
    tmux("send-keys", "-t", window, "-l", line);
    tmux("send-keys", "-t", window, "Enter");
  });
  await new Promise((resolve) => setTimeout(resolve, 200));
  await relay.alertTabs(state);
  assert.equal(posts.length, 0, "a second later, the screen isn't read again");

  relay.dialogChecked.set(window, Date.now() - relay.DIALOG_CHECK_MS);
  await relay.alertTabs(state);
  assert.ok(posts.some((p) => /is showing a dialog before its session starts/.test(p.body)));
  tmux("kill-window", "-t", window);
});

test("Claude started under a Node wrapper, as claude-here starts it, is still the tab's Claude", async () => {
  // tmux reports such a pane as node; Claude is the wrapper's child.
  const wrapper = path.join(tmp, "wrapper.js");
  fs.writeFileSync(wrapper, `require("node:child_process").spawn(${JSON.stringify(fakeClaude)}, { stdio: "inherit" }).on("exit", () => process.exit());`);
  const window = tmux("new-window", "-d", "-P", "-F", "#{window_id}", `${process.execPath} ${wrapper}`);
  const plain = tmux("new-window", "-d", "-P", "-F", "#{window_id}", `${process.execPath} -e "setInterval(() => {}, 1000)"`);
  await new Promise((resolve) => setTimeout(resolve, 600));
  const [pane, tty] = tmux("display", "-p", "-t", window, "#{pane_id}\t#{pane_tty}").split("\t");
  const claudePid = execFileSync("ps", ["-t", tty.replace("/dev/", ""), "-o", "pid=,comm="], { encoding: "utf8" })
    .split("\n")
    .find((line) => line.trim().endsWith("2.1.0"))
    .trim()
    .split(/\s+/)[0];
  const sessionFile = path.join(home, ".claude/sessions", `${claudePid}.json`);
  fs.writeFileSync(sessionFile, JSON.stringify({ pid: Number(claudePid), sessionId: "wrapped" }));
  fs.writeFileSync(path.join(home, ".claude/projects/-work/wrapped.jsonl"), "");
  const hook = path.join(__dirname, "../tmux/agent-state.sh");
  const env = { ...process.env, HOME: home, TMUX_PANE: pane, AGENT_SEEN_DELAY: "0" };
  try {
    assert.equal(tmux("display", "-p", "-t", window, "#{pane_current_command}"), "node");
    const tab = relay.claudeTabs().find((t) => t.id === window);
    assert.equal(tab?.pane, pane);
    assert.equal(relay.claudeTabs().find((t) => t.id === plain), undefined, "a node program with no Claude isn't a tab");
    assert.equal(relay.transcriptFor(window), path.join(home, ".claude/projects/-work/wrapped.jsonl"));

    execFileSync("bash", [hook, "set", "working"], { env });
    assert.equal(tmux("display", "-p", "-t", window, "#{@agent}"), "working", "agent-state.sh keeps its state");

    // Claude gone from it, and the next change in the window drops its state.
    fs.rmSync(sessionFile);
    tmux("set", "-p", "-t", pane, "@agent_state", "waiting");
    execFileSync("bash", [hook, "seen", window], { env });
    assert.equal(tmux("display", "-p", "-t", window, "#{@agent_state}"), "");
  } finally {
    fs.rmSync(sessionFile, { force: true });
    [window, plain].forEach((id) => tmux("kill-window", "-t", id));
  }
});

test("a done tab stays done when passed over, and is seen once it stays in front", async (t) => {
  // A client attached, as when you are looking at it. Control mode needs no
  // terminal.
  const viewer = spawn("tmux", ["-S", socket, "-C", "attach"], { stdio: ["pipe", "ignore", "ignore"] });
  t.after(() => viewer.kill());
  await new Promise((resolve) => setTimeout(resolve, 300));
  assert.notEqual(tmux("display", "-p", "#{session_attached}"), "0");
  const home2 = tmux("display", "-p", "#{window_id}");
  const window = tmux("new-window", "-P", "-F", "#{window_id}", fakeClaude);
  await new Promise((resolve) => setTimeout(resolve, 300));
  const [pane, tty] = tmux("display", "-p", "-t", window, "#{pane_id}\t#{pane_tty}").split("\t");
  const claudePid = execFileSync("ps", ["-t", tty.replace("/dev/", ""), "-o", "pid=,comm="], { encoding: "utf8" })
    .split("\n")
    .find((line) => line.trim().endsWith("2.1.0"))
    .trim()
    .split(/\s+/)[0];
  const sessionFile = path.join(home, ".claude/sessions", `${claudePid}.json`);
  fs.writeFileSync(sessionFile, JSON.stringify({ pid: Number(claudePid), sessionId: "passed-over" }));
  t.after(() => {
    fs.rmSync(sessionFile, { force: true });
    tmux("kill-window", "-t", window);
  });
  tmux("set", "-p", "-t", pane, "@agent_state", "done");
  tmux("set", "-p", "-t", pane, "@agent_since", "1");
  const hook = path.join(__dirname, "../tmux/agent-state.sh");
  const env = { ...process.env, HOME: home, AGENT_SEEN_DELAY: "0.3" };

  // Focused, then left within the delay: still done.
  const passing = spawn("bash", [hook, "seen", window], { env });
  tmux("select-window", "-t", home2);
  await new Promise((resolve) => passing.on("exit", resolve));
  assert.equal(tmux("display", "-p", "-t", window, "#{@agent_state}"), "done");

  // Focused and kept in front: seen.
  tmux("select-window", "-t", window);
  execFileSync("bash", [hook, "seen", window], { env });
  assert.equal(tmux("display", "-p", "-t", window, "#{@agent_state}"), "");
  tmux("select-window", "-t", home2);
});

test("a tab goes by its whole name, not the one cut short to fit the tab bar", async () => {
  const window = tmux("new-window", "-d", "-P", "-F", "#{window_id}", "-n", "#1234 ABC-12: Filter and pag...", fakeClaude);
  await new Promise((resolve) => setTimeout(resolve, 300));
  try {
    assert.equal(relay.claudeTabs().find((t) => t.id === window).name, "#1234 ABC-12: Filter and pag...", "the tab's own name until window-names.sh has run");
    tmux("set", "-w", "-t", window, "@tab_label", "#1234 ABC-12: Filter and page the idea list in place");
    assert.equal(relay.claudeTabs().find((t) => t.id === window).name, "#1234 ABC-12: Filter and page the idea list in place");
  } finally {
    tmux("kill-window", "-t", window);
  }
});

test("transcriptFor keeps its answer until the session file changes", () => {
  const window = tmux("new-window", "-d", "-P", "-F", "#{window_id}", fakeClaude);
  const panePid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  const sessionFile = path.join(home, ".claude/sessions", `${panePid}.json`);
  ["before-clear", "after-clear"].forEach((id) => fs.writeFileSync(path.join(home, `.claude/projects/-work/${id}.jsonl`), ""));
  fs.writeFileSync(sessionFile, JSON.stringify({ pid: Number(panePid), sessionId: "before-clear" }));
  // Whole seconds, which setting it back below keeps exactly.
  const written = new Date(Math.floor(Date.now() / 1000) * 1000);
  fs.utimesSync(sessionFile, written, written);

  assert.equal(relay.transcriptFor(window), path.join(home, ".claude/projects/-work/before-clear.jsonl"));

  // The same file, as far as its modified time says: the answer is kept.
  fs.writeFileSync(sessionFile, JSON.stringify({ pid: Number(panePid), sessionId: "after-clear" }));
  fs.utimesSync(sessionFile, written, written);
  assert.equal(relay.transcriptFor(window), path.join(home, ".claude/projects/-work/before-clear.jsonl"));

  // Rewritten, as /clear does: it is looked up again.
  const later = new Date(written.getTime() + 5_000);
  fs.utimesSync(sessionFile, later, later);
  assert.equal(relay.transcriptFor(window), path.join(home, ".claude/projects/-work/after-clear.jsonl"));

  // Gone, as when the session ends: nothing.
  fs.rmSync(sessionFile);
  assert.equal(relay.transcriptFor(window), undefined);
  tmux("kill-window", "-t", window);
});

test("new: starts a Claude in a named repository, worktree or directory, and only on one match", async () => {
  const repo = path.join(tmp, "myapp");
  fs.mkdirSync(repo);
  const git = (...args) => execFileSync("git", ["-C", repo, "-c", "user.name=Test", "-c", "user.email=test@example.com", ...args], { stdio: "ignore" });
  git("init", "-q", "-b", "main");
  git("commit", "-q", "--allow-empty", "-m", "Start");
  git("worktree", "add", "-q", "-b", "abc-214-make-the-thing", path.join(tmp, "myapp-abc-214"));
  git("worktree", "add", "-q", "-b", "abc-300-other-thing", path.join(tmp, "myapp-abc-300"));
  // An open tab in the repository is how the relay knows it is in use.
  const [inRepo] = tmux("new-window", "-d", "-c", repo, "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  const ask = (text) => relay.handle(state, { ts: `${Math.floor(Date.now() / 1000)}.${String(Math.random()).slice(2, 8)}`, text, thread_ts: null });
  const opened = () => tmux("list-windows", "-F", "#{window_id}").split("\n");

  const before = opened();
  await ask("new ABC-214: make the thing");
  const window = opened().find((id) => !before.includes(id));
  await new Promise((resolve) => setTimeout(resolve, 300));
  assert.equal(tmux("display", "-p", "-t", window, "#{pane_current_path}"), fs.realpathSync(path.join(tmp, "myapp-abc-214")));
  assert.match(tmux("capture-pane", "-p", "-t", window), /claude 'make the thing'/);
  assert.match(posts.at(-1).body, /Started a new Claude in tab \d+, in `.*myapp-abc-214` \(myapp, abc-214-make-the-thing\)\./);

  const beforeRepo = opened();
  await ask("new myapp: look around");
  const repoWindow = opened().find((id) => !beforeRepo.includes(id));
  await new Promise((resolve) => setTimeout(resolve, 300));
  assert.equal(tmux("display", "-p", "-t", repoWindow, "#{pane_current_path}"), fs.realpathSync(repo));
  assert.match(posts.at(-1).body, /\(myapp, main\)/);

  const beforeErrors = opened();
  await ask("new abc: which one?");
  assert.match(posts.at(-1).body, /`abc` matches 2 places, so I didn't start anything/);
  await ask("new zzz: nothing");
  assert.match(posts.at(-1).body, /Nothing matches `zzz`/);
  await ask("new ~/no-such-dir: nothing");
  assert.match(posts.at(-1).body, /There's no directory `~\/no-such-dir`/);
  assert.deepEqual(opened(), beforeErrors, "no tab opened for an unclear place");

  [window, repoWindow, inRepo].forEach((id) => tmux("kill-window", "-t", id));
});

test("no stuck notes in the first ten minutes after the Mac wakes", async () => {
  const [window] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  tmux("set", "-p", "-t", window, "@agent_state", "working");
  const panePid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  fs.writeFileSync(path.join(home, ".claude/sessions", `${panePid}.json`), JSON.stringify({ pid: Number(panePid), sessionId: "slept" }));
  const transcript = path.join(home, ".claude/projects/-work/slept.jsonl");
  // Nothing since the Mac went to sleep, hours ago.
  fs.writeFileSync(transcript, JSON.stringify({ type: "user", timestamp: new Date(Date.now() - 5 * 60 * 60_000).toISOString(), message: { content: "long job" } }));
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  try {
    relay.life.wokeAt = Date.now() - 60_000;
    await relay.stuckTabs(state);
    assert.equal(posts.length, 0, "it just woke up");

    relay.life.wokeAt = Date.now() - relay.STUCK_MS - 1_000;
    await relay.stuckTabs(state);
    assert.ok(posts.some((p) => /may be stuck/.test(p.body)), "still nothing ten minutes after waking");
  } finally {
    relay.life.wokeAt = 0;
    fs.rmSync(path.join(home, ".claude/sessions", `${panePid}.json`));
    tmux("kill-window", "-t", window);
  }
});

test("the dispatcher handles each message once, in order, and moves the cursor past everything", async () => {
  const thread = `${Math.floor(Date.now() / 1000) - 60}.000100`;
  const state = { dm: { channel: "D1", user: "U1" }, cursor: "1791700000.000050", threads: { [thread]: thread }, alerts: {}, reported: {}, following: {} };
  const { processMessages, settled } = relay.dispatcher(state);
  const now = Math.floor(Date.now() / 1000);

  // Two overlapping checks: the second sees one message again, and the
  // messages arrive out of order.
  processMessages([
    { ts: `${now}.000002`, text: "cleanup", byClaude: false, thread_ts: null },
    { ts: `${now}.000001`, text: "help", byClaude: false, thread_ts: null },
  ]);
  processMessages([
    { ts: `${now}.000002`, text: "cleanup", byClaude: false, thread_ts: null },
    { ts: `${now}.000003`, text: "🤖 the relay's own post", byClaude: true, thread_ts: null },
    { ts: `${now}.000004`, text: "status", byClaude: false, thread_ts: thread },
  ]);
  await settled();

  assert.deepEqual(posts.map((p) => p.body.split("\n")[0]), ["🤖 **What I understand**", "🤖 No closed tabs have threads left to delete.", "🤖 **Status**"]);
  assert.equal(state.cursor, `${now}.000003`, "past the relay's own post too");
  assert.equal(state.threads[thread], `${now}.000004`);
});

test("a message that fails is answered with the error, and the next one still runs", async () => {
  const state = { dm: { channel: "D1", user: "U1" }, cursor: "1791700000.000050", threads: {}, alerts: {}, reported: {}, following: {} };
  const { processMessages, settled } = relay.dispatcher(state);
  const now = Math.floor(Date.now() / 1000);

  processMessages([
    // No text to read, so handling it fails.
    { ts: `${now}.000005`, text: undefined, byClaude: false, thread_ts: "1791700000.000060" },
    { ts: `${now}.000006`, text: "cleanup", byClaude: false, thread_ts: null },
  ]);
  await settled();

  assert.match(posts[0].body, /^🤖 That failed: /);
  assert.equal(posts[0].thread, "1791700000.000060");
  assert.equal(posts.at(-1).body, "🤖 No closed tabs have threads left to delete.");
});

test("the watchdog restarts a crashed relay, telling it why, until it ends cleanly", async () => {
  // A stand-in relay that logs a crash and exits 1 twice, then exits 0.
  const fake = path.join(tmp, "fake-relay.js");
  const runs = path.join(tmp, "fake-relay-runs.txt");
  fs.writeFileSync(runs, "");
  fs.writeFileSync(fake, `const fs = require("fs");
const n = fs.readFileSync(${JSON.stringify(runs)}, "utf8").split("\\n").filter(Boolean).length + 1;
fs.appendFileSync(${JSON.stringify(runs)}, JSON.stringify(process.env.SLACK_RELAY_RESTARTED ?? "") + "\\n");
if (n <= 2) {
  fs.appendFileSync(${JSON.stringify(relay.LOG_FILE)}, new Date().toISOString() + " Error: crashed: boom " + n + "\\n");
  process.exit(1);
}
process.exit(0);
`);
  let downs = 0;

  const code = await relay.watch({ command: [fake], delays: [10, 10, 10, 10, 10], down: async () => downs++ });

  assert.equal(code, 0);
  assert.equal(downs, 0);
  assert.deepEqual(fs.readFileSync(runs, "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line)), ["", "boom 1", "boom 2"]);
});

test("the watchdog gives up after five crashes and says the relay is down", async () => {
  const fake = path.join(tmp, "always-crashes.js");
  fs.writeFileSync(fake, `require("fs").appendFileSync(${JSON.stringify(relay.LOG_FILE)}, new Date().toISOString() + " Error: crashed: no network\\n"); process.exit(1);\n`);
  fs.writeFileSync(relay.EVENTS_FILE, "");
  const said = [];

  const code = await relay.watch({ command: [fake], delays: [10, 10, 10, 10, 10], down: async (why) => said.push(why) });

  assert.equal(code, 1);
  assert.deepEqual(said, ["no network"]);
  assert.equal(fs.existsSync(relay.EVENTS_FILE), false, "nothing will read the hooks' events now");
});

test("the watchdog doesn't restart a relay that refused to start", async () => {
  const fake = path.join(tmp, "refuses.js");
  fs.writeFileSync(fake, "process.exit(3);\n");
  let downs = 0;

  const code = await relay.watch({ command: [fake], delays: [10], down: async () => downs++ });

  assert.equal(code, 3);
  assert.equal(downs, 0);
});


test("only the owner's messages, in the app's DM and workspace, are acted on", () => {
  const state = { dm: { channel: "D1", user: "U1", team: "T1" } };
  const event = { type: "message", channel: "D1", user: "U1", ts: "1791500000.000100", text: "3: run the tests", client_msg_id: "c1" };

  assert.deepEqual(relay.messageFrom(state, event, "T1"), { ts: "1791500000.000100", text: "3: run the tests", byClaude: false, thread_ts: null });
  assert.equal(relay.messageFrom(state, { ...event, thread_ts: "1791400000.000100" }, "T1").thread_ts, "1791400000.000100");
  // A thread's first message carries its own ts as thread_ts: it is top level.
  assert.equal(relay.messageFrom(state, { ...event, thread_ts: event.ts }, "T1").thread_ts, null);

  assert.equal(relay.messageFrom(state, { ...event, user: "U2" }, "T1"), undefined, "someone else in the workspace");
  assert.equal(relay.messageFrom(state, { ...event, channel: "D2" }, "T1"), undefined, "another conversation");
  assert.equal(relay.messageFrom(state, event, "T2"), undefined, "another workspace");
  assert.equal(relay.messageFrom(state, { ...event, bot_id: "B1" }, "T1"), undefined, "a bot, the relay's own posts included");
  assert.equal(relay.messageFrom(state, { ...event, subtype: "message_changed" }, "T1"), undefined, "an edit");
  assert.equal(relay.messageFrom(state, { ...event, subtype: "message_deleted" }, "T1"), undefined, "a deletion");
  assert.ok(relay.messageFrom(state, { ...event, subtype: "file_share" }, "T1"), "a message with a file");

  // Posted as you by an app holding your user token, such as the Slack
  // connector a Claude session uses: your member ID, but not typed by you.
  const { client_msg_id, ...posted } = event;
  assert.equal(relay.messageFrom(state, posted, "T1"), undefined, "no client_msg_id: posted through the API");
  assert.equal(relay.messageFrom(state, { ...event, app_id: "A0CLAUDE" }, "T1"), undefined, "posted by an app");
  assert.equal(relay.messageFrom(state, { ...event, bot_profile: { app_id: "A0CLAUDE" } }, "T1"), undefined, "posted by an app, its profile attached");

  // A reaction, even yours, controls nothing: an app can react as you too.
  const reaction = { type: "reaction_added", user: "U1", reaction: "+1", item: { type: "message", channel: "D1", ts: "1791500000.000100" } };
  assert.equal(relay.messageFrom(state, reaction, "T1"), undefined);
  assert.equal(relay.answerReactions, undefined, "no reaction handling at all");
});

test("the text you typed is what reaches a tab, without Slack's escapes and link brackets", () => {
  assert.equal(relay.plainText("if a &lt; b &amp;&amp; c &gt; d"), "if a < b && c > d");
  assert.equal(relay.plainText("see <https://github.com/o/r/pull/12>"), "see https://github.com/o/r/pull/12");
  assert.equal(relay.plainText("see <https://example.com|example.com>"), "see example.com");
  assert.equal(relay.plainText("see <https://example.com/a|the docs>"), "see https://example.com/a");
});

test("the API client form-encodes, waits out rate limits, reports Slack's refusals, and writes only to the DM", async () => {
  const http = require("node:http");
  const seen = [];
  let limited = false;
  const server = http.createServer((req, res) => {
    let body = "";
    req.on("data", (chunk) => (body += chunk));
    req.on("end", () => {
      seen.push({ url: req.url, auth: req.headers.authorization, body: new URLSearchParams(body) });
      if (req.url === "/chat.postMessage" && !limited) {
        limited = true;
        res.writeHead(429, { "retry-after": "0" });
        return res.end();
      }
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify(req.url === "/chat.postMessage" ? { ok: true, ts: "1.2" } : { ok: false, error: "not_authed" }));
    });
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const saved = { base: relay.slack.base, botToken: relay.slack.botToken };
  Object.assign(relay.slack, { base: `http://127.0.0.1:${server.address().port}/`, botToken: "xoxb-test", channel: "D1" });
  try {
    const sent = await relay.apiCall("chat.postMessage", { channel: "D1", text: "hi & bye", thread_ts: undefined, files: [{ id: "F1" }] });
    assert.equal(sent.ts, "1.2");
    assert.equal(seen.length, 2, "asked again after the rate limit");
    assert.equal(seen[1].auth, "Bearer xoxb-test");
    assert.equal(seen[1].body.get("text"), "hi & bye");
    assert.equal(seen[1].body.has("thread_ts"), false);
    assert.equal(seen[1].body.get("files"), '[{"id":"F1"}]');

    await assert.rejects(relay.apiCall("auth.test"), /auth\.test: not_authed/);
    await assert.rejects(relay.apiCall("chat.postMessage", { channel: "C0OTHER", text: "x" }), /only uses its DM, not C0OTHER/);
    await assert.rejects(relay.apiCall("files.completeUploadExternal", { channel_id: "C0OTHER" }), /only uses its DM/);
    await assert.rejects(relay.apiCall("views.publish", { user_id: "U0SOMEONE", view: {} }), /only shows its Home tab to its owner/);
    assert.equal(seen.length, 3, "a call to another conversation never leaves the Mac");
  } finally {
    Object.assign(relay.slack, saved);
    server.close();
  }
});

test("catching up finds only the owner's messages sent since the cursor and each thread's last reply", async () => {
  const state = { dm: { channel: "D1", user: "U1", team: "T1" }, cursor: "1791500000.000100", threads: { "1791500000.000050": "1791500000.000060" } };
  answers["conversations.history"] = (params) => {
    assert.equal(params.oldest, "1791500000.000100");
    return {
      ok: true,
      messages: [
        { user: "U1", ts: "1791500000.000300", text: "status", client_msg_id: "c1" },
        { user: "U2", ts: "1791500000.000250", text: "1: rm -rf ~", client_msg_id: "c2" },
        { user: "U1", ts: "1791500000.000260", text: "1: rm -rf ~", app_id: "A0CLAUDE" },
        { user: "B0BOT", bot_id: "B1", ts: "1791500000.000200", text: "🤖 Relay listening." },
      ],
    };
  };
  answers["conversations.replies"] = (params) => {
    assert.equal(params.ts, "1791500000.000050");
    return {
      ok: true,
      messages: [
        { user: "U1", ts: "1791500000.000050", text: "the thread's own message", client_msg_id: "c3" },
        { user: "U1", ts: "1791500000.000060", thread_ts: "1791500000.000050", text: "already handled", client_msg_id: "c4" },
        { user: "U1", ts: "1791500000.000400", thread_ts: "1791500000.000050", text: "yes", client_msg_id: "c5" },
      ],
    };
  };

  const messages = await relay.catchUp(state);

  assert.deepEqual(messages.map((m) => [m.text, m.thread_ts]), [["status", null], ["yes", "1791500000.000050"]]);
});

test("a reply that isn't one of a waiting prompt's options types nothing", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  ["Allow rm -rf build?", "❯ 1. Yes", "  2. No, and tell Claude what to do differently"].forEach((line) => {
    tmux("send-keys", "-t", window, "-l", line);
    tmux("send-keys", "-t", window, "Enter");
  });
  await new Promise((resolve) => setTimeout(resolve, 200));
  tmux("set", "-p", "-t", window, "@agent_state", "waiting");
  const alert = `${Math.floor(Date.now() / 1000) - 30}.000100`;
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, reported: {}, following: {}, alerts: { [alert]: { window, index: Number(index), name: "work" } } };
  const before = tmux("capture-pane", "-p", "-t", window);

  await relay.handle(state, { ts: `${Math.floor(Date.now() / 1000)}.000020`, text: "cancel", thread_ts: alert });
  await new Promise((resolve) => setTimeout(resolve, 200));

  assert.match(posts[0].body, /is showing a choice, and that isn't one of its options, so I didn't type anything/);
  assert.match(posts[0].body, /1\. Yes\n2\. No, and tell Claude/);
  assert.equal(tmux("capture-pane", "-p", "-t", window), before, "nothing typed, no Enter");
  assert.deepEqual(state.following, {});
  tmux("kill-window", "-t", window);
});

test("a turn ending in the tab in front of you still goes in its thread, from its hook event", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const panePid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  fs.writeFileSync(path.join(home, ".claude/sessions", `${panePid}.json`), JSON.stringify({ pid: Number(panePid), sessionId: "in-front" }));
  const transcript = path.join(home, ".claude/projects/-work/in-front.jsonl");
  fs.writeFileSync(transcript, `${JSON.stringify({ type: "user", message: { content: "check the build" }, timestamp: new Date().toISOString() })}\n`);
  // Focused, so agent-state.sh cleared it rather than mark it done.
  const event = { event: "done", time: Math.floor(Date.now() / 1000), transcript_path: transcript, last: "The build passes." };
  // The tests' tmux has no client attached, so the tab is marked as the one in front of you.
  const inFront = () => relay.claudeTabs().map((t) => (t.id === window ? { ...t, focused: true } : t));
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  try {
    // At the computer: in its thread all the same, so the thread is complete.
    relay.presence.idleMs = 0;
    await relay.alertTabs(state, inFront(), [event]);
    await relay.alertTabs(state, inFront(), [event]);
    const alert = posts.find((p) => /has finished/.test(p.body));
    assert.match(alert.body, new RegExp(`^🤖 \\*\\*Tab ${index}, .*\\*\\* has finished`));
    assert.match(alert.body, /> check the build\n\nThe build passes\./);
    assert.equal(alert.thread, state.homes[window].ts, "in the tab's thread");
    assert.equal(alert.broadcast, false, "not in the main DM");
    assert.equal(posts.filter((p) => /has finished/.test(p.body)).length, 1, "once");
  } finally {
    fs.rmSync(path.join(home, ".claude/sessions", `${panePid}.json`));
    tmux("kill-window", "-t", window);
  }
});

test("a turn says how full its context is, and a tab past 90% is told once, with a Compact button", async () => {
  const [window] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const panePid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  fs.writeFileSync(path.join(home, ".claude/sessions", `${panePid}.json`), JSON.stringify({ pid: Number(panePid), sessionId: "nearly-full" }));
  const transcript = path.join(home, ".claude/projects/-work/nearly-full.jsonl");
  fs.writeFileSync(transcript, `${JSON.stringify({ type: "user", message: { content: "check the build" }, timestamp: new Date().toISOString() })}\n`);
  const contextFile = path.join(home, ".local/state/tmux-sidebar/context/nearly-full");
  fs.mkdirSync(path.dirname(contextFile), { recursive: true });
  const inFront = () => relay.claudeTabs().map((t) => (t.id === window ? { ...t, focused: true } : t));
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  const now = Math.floor(Date.now() / 1000);
  try {
    relay.presence.idleMs = 0;
    fs.writeFileSync(contextFile, "92\tOpus 5.5 (1M context)");
    await relay.alertTabs(state, inFront(), [{ event: "done", time: now, transcript_path: transcript, last: "The build passes." }]);
    assert.match(posts.find((p) => /has finished/.test(p.body)).body, /_92% context used · Opus 5\.5 \(1M context\)_$/);
    const warnings = () => posts.filter((p) => /of its context/.test(p.body));
    assert.equal(warnings().length, 1);
    assert.match(warnings()[0].body, /has used \*\*92%\*\* of its context/);
    assert.equal(warnings()[0].thread, state.homes[window].ts, "in the tab's thread");
    const warned = calls.find((c) => c.method === "chat.postMessage" && /of its context/.test(c.params.text));
    assert.match(JSON.stringify(warned.params.blocks), /Compact/);
    assert.match(JSON.stringify(warned.params.blocks), /\\"skill\\":\\"compact\\"/);

    fs.writeFileSync(contextFile, "95\tOpus 5.5 (1M context)");
    await relay.alertTabs(state, inFront(), [{ event: "done", time: now + 1, transcript_path: transcript, last: "Still passing." }]);
    assert.equal(warnings().length, 1, "not again while it stays above");

    fs.writeFileSync(contextFile, "30\tOpus 5.5 (1M context)");
    await relay.alertTabs(state, inFront(), [{ event: "done", time: now + 2, transcript_path: transcript, last: "Compacted." }]);
    fs.writeFileSync(contextFile, "91\tOpus 5.5 (1M context)");
    await relay.alertTabs(state, inFront(), [{ event: "done", time: now + 3, transcript_path: transcript, last: "Full again." }]);
    assert.equal(warnings().length, 2, "again once it has dropped below and passed it again");
  } finally {
    fs.rmSync(path.join(home, ".claude/sessions", `${panePid}.json`));
    fs.rmSync(contextFile);
    tmux("kill-window", "-t", window);
  }
});
test("slack-relay starts from outside tmux, in the running tmux server", () => {
  // Outside tmux, tmux finds its server by the default socket under
  // TMUX_TMPDIR, which is the tests' own directory; it is pointed at the
  // tests' server, so this can't reach yours.
  assert.equal(process.env.TMUX_TMPDIR, tmp);
  const dir = path.join(tmp, `tmux-${process.getuid()}`);
  fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
  fs.symlinkSync(socket, path.join(dir, "default"));
  const saved = process.env.TMUX;
  delete process.env.TMUX;
  const before = execFileSync("tmux", ["-S", socket, "list-windows", "-a", "-F", "#{window_id}"], { encoding: "utf8" }).trim().split("\n");
  try {
    assert.equal(execFileSync("tmux", ["display", "-p", "#{socket_path}"], { encoding: "utf8" }).trim(), socket, "the tests' server");
    relay.start(fakeClaude);
    const added = execFileSync("tmux", ["-S", socket, "list-windows", "-a", "-F", "#{window_id}\t#{@tab_name}"], { encoding: "utf8" }).trim().split("\n").filter((line) => !before.includes(line.split("\t")[0]));
    assert.equal(added.length, 1, "the relay's tab");
    assert.equal(added[0].split("\t")[1], "Slack relay");
    execFileSync("tmux", ["-S", socket, "kill-window", "-t", added[0].split("\t")[0]]);
  } finally {
    process.env.TMUX = saved;
    fs.rmSync(path.join(dir, "default"), { force: true });
  }
});

test("slack-relay starts the relay again in its tab when the one there has stopped", () => {
  // The relay's tab, left at a shell (cat stands in) by a relay that stopped.
  const window = tmux("new-window", "-d", "-P", "-F", "#{window_id}", fakeClaude);
  tmux("set-option", "-w", "-t", window, "@slack_relay", "1");
  const pane = () => tmux("display", "-p", "-t", window, "#{pane_pid}");
  const before = pane();
  try {
    // A live relay holds the lock: its tab is only gone to.
    fs.writeFileSync(relay.LOCK_FILE, `${process.pid} slack-relay\n`);
    relay.start(fakeClaude);
    assert.equal(pane(), before);

    // A stale lock: the relay is started again in the same tab.
    fs.writeFileSync(relay.LOCK_FILE, "999999 slack-relay\n");
    relay.start(fakeClaude);
    assert.notEqual(pane(), before);
    assert.equal(tmux("display", "-p", "-t", window, "#{pane_current_command}"), "2.1.0", "the stand-in, not the relay");
    assert.equal(tmux("list-windows", "-F", "#{@slack_relay}").split("\n").filter((m) => m === "1").length, 1, "no second relay tab");
  } finally {
    fs.rmSync(relay.LOCK_FILE, { force: true });
    tmux("kill-window", "-t", window);
  }
});

test("while you're at the computer, threads get everything at once and only the main DM's lines wait", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  ["Allow rm -rf build?", "❯ 1. Yes", "  2. No"].forEach((line) => {
    tmux("send-keys", "-t", window, "-l", line);
    tmux("send-keys", "-t", window, "Enter");
  });
  await new Promise((resolve) => setTimeout(resolve, 200));
  tmux("set", "-p", "-t", window, "@agent_state", "waiting");
  tmux("set", "-p", "-t", window, "@agent_since", String(Math.floor(Date.now() / 1000)));
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  const tab = { id: window, index: Number(index), name: "work" };
  relay.presence.idleMs = 0;

  await relay.alertTabs(state);
  await relay.ping(state, tab, "tests failed: `pnpm test`", { broadcast: true });
  await relay.ping(state, tab, "pushed `feature`");
  const home = state.homes[window].ts;
  // In the thread now: the wait with its options, the failure, the push.
  assert.ok(posts.some((p) => p.thread === home && /is \*\*waiting on you\*\*\. Reply here/.test(p.body)));
  assert.ok(posts.some((p) => p.thread === home && /tests failed/.test(p.body)));
  assert.ok(posts.some((p) => p.thread === home && /pushed `feature`/.test(p.body)));
  assert.ok(posts.every((p) => !p.broadcast), "nothing in the main DM yet");
  assert.equal(relay.presence.held.length, 2, "the wait's line and the failure's");

  relay.presence.idleMs = relay.AWAY_AFTER_MS;
  await relay.presence.flush();
  const lines = posts.filter((p) => p.broadcast).map((p) => p.body);
  assert.equal(lines.length, 2);
  assert.match(lines[0], /is \*\*waiting on you\*\*\.$/);
  assert.match(lines[1], /tests failed/);
  assert.equal(relay.presence.held.length, 0);

  // Away: a failure is one message, in the thread and the main DM at once.
  await relay.ping(state, tab, "tests failed again", { broadcast: true });
  assert.equal(posts.at(-1).broadcast, true);
  tmux("kill-window", "-t", window);
});
test("a locked screen counts as away at once, however recently a key was pressed", async () => {
  relay.presence.idleMs = 1_000;
  assert.equal(relay.presence.away(), false);
  relay.presence.locked = true;
  assert.equal(relay.presence.away(), true);
  assert.equal(require("../lib/presence").awayBy({ idleMs: 1_000, locked: true }, 600_000), true, "the sidebar's reading agrees");
  assert.equal(require("../lib/presence").awayBy({ idleMs: undefined, locked: undefined }, 600_000), false, "unknown is here");
});

test("writing from Slack counts as away for a while, unless you wrote it on the Mac", async () => {
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  relay.presence.idleMs = 0;
  relay.presence.held.push(async () => relay.post(state, "🤖 held while you were here"));

  // Typed in Slack on the Mac: the keyboard was used a moment ago.
  relay.presence.readIdle = async () => 1_000;
  await relay.presence.fromSlack();
  assert.equal(relay.presence.away(), false);
  assert.equal(posts.length, 0);

  // From your phone: the Mac has been untouched for longer than that.
  relay.presence.readIdle = async () => relay.TYPED_HERE_MS + 1_000;
  await relay.presence.fromSlack();
  assert.equal(relay.presence.away(), true, "away, though the Mac isn't idle for 30 seconds yet");
  assert.equal(posts[0].body, "🤖 held while you were here", "what was held goes out");
  assert.ok(relay.presence.slackUntil > Date.now() + relay.SLACK_AWAY_MS - 1_000);

  // The window runs out: back to the Mac's idle time.
  relay.presence.slackUntil = Date.now() - 1;
  assert.equal(relay.presence.away(), false);
});

test("posts and edits go out as Markdown blocks, with the same words for notifications", async () => {
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {} };
  const body = "🤖 Tab 3 has finished.\n## Summary\n**Done**: opened [PR #7](https://github.com/o/r/pull/7)";

  const ts = await relay.post(state, body, "1791500000.000400");
  await relay.update(state, ts, body);

  const [sent, edited] = calls.filter((c) => c.method === "chat.postMessage" || c.method === "chat.update").map((c) => c.params);
  assert.deepEqual(sent.blocks, [{ type: "markdown", text: body }]);
  assert.equal(sent.text, body);
  assert.deepEqual(edited.blocks, [{ type: "markdown", text: body }]);
});

test("an image you attach is downloaded and its path typed into the tab with your text", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const alert = `${Math.floor(Date.now() / 1000) - 30}.000100`;
  const state = { dm: { channel: "D1", user: "U1", team: "T1" }, threads: {}, alerts: { [alert]: { window, index: Number(index), name: "work" } }, reported: {}, following: {} };
  const fetched = [];
  relay.slack.get = async (url) => {
    fetched.push(url);
    return Buffer.from("PNG bytes");
  };
  // As Slack sends it: your message, with the file attached.
  const message = relay.messageFrom(state, {
    type: "message", subtype: "file_share", channel: "D1", user: "U1", client_msg_id: "c1",
    ts: `${Math.floor(Date.now() / 1000)}.000040`, thread_ts: alert, text: "what's wrong here?",
    files: [{ id: "F0SHOT", name: "screen shot.png", mimetype: "image/png", size: 9, url_private_download: "https://files.slack.com/files-pri/T1-F0SHOT/download/screen_shot.png" }],
  }, "T1");

  await relay.handle(state, message);
  await new Promise((resolve) => setTimeout(resolve, 200));

  const saved = path.join(relay.FILES_DIR, "F0SHOT-screen-shot.png");
  assert.deepEqual(fetched, ["https://files.slack.com/files-pri/T1-F0SHOT/download/screen_shot.png"]);
  assert.equal(fs.readFileSync(saved, "utf8"), "PNG bytes");
  assert.equal(fs.statSync(saved).mode & 0o777, 0o600);
  assert.match(tmux("capture-pane", "-p", "-J", "-t", window), new RegExp(`what's wrong here\\? \\[Image: ${saved.replace(/[.]/g, "\\.")}\\]`));
  assert.equal(state.following[window].thread, alert);
  tmux("kill-window", "-t", window);
});

test("only images, from Slack's file host, under the size limit, are fetched; an ID alone is looked up", async () => {
  const state = { dm: { channel: "D1", user: "U1" } };
  const fetched = [];
  relay.slack.get = async (url) => {
    fetched.push(url);
    return Buffer.from("x");
  };
  const image = { id: "F1", name: "a.png", mimetype: "image/png", size: 1, url: "https://files.slack.com/files-pri/T1-F1/a.png" };

  assert.match((await relay.download({ ...image, mimetype: "application/zip", name: "a.zip" })).error, /isn't an image or video I can pass on/);
  assert.match((await relay.download({ ...image, size: relay.MAX_FILE_BYTES + 1 })).error, /larger than 20 MB/);
  assert.match((await relay.download({ ...image, url: "https://evil.example/a.png" })).error, /isn't on Slack's file host/);
  assert.match((await relay.download({ ...image, url: "http://files.slack.com/a.png" })).error, /isn't on Slack's file host/);
  assert.deepEqual(fetched, [], "nothing fetched for any of those");

  answers["files.info"] = (params) => ({ ok: true, file: { ...image, id: params.file, url_private_download: image.url, url: undefined } });
  const { path: saved } = await relay.download({ id: "F2" });
  assert.equal(path.basename(saved), "F2-a.png");
  assert.deepEqual(fetched, [image.url]);
});

test("a turn's new images are shared under it, and nothing else that only looks like one", async () => {
  const work = path.join(tmp, "imgwork");
  fs.mkdirSync(path.join(work, "out"), { recursive: true });
  const png = Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), Buffer.from("rest")]);
  const sent = Date.now() - 10_000;
  // Made during the turn: one named by absolute path, one relative to the session's directory.
  fs.writeFileSync(path.join(work, "shot.png"), png);
  fs.writeFileSync(path.join(work, "out/chart.png"), png);
  // Made during the turn but named only by its tools, as a scratch image is.
  fs.writeFileSync(path.join(work, "probe.png"), png);
  // Mentioned, but not shareable: older than the turn, not really an image, one you sent, and missing.
  fs.writeFileSync(path.join(work, "old.png"), png);
  fs.utimesSync(path.join(work, "old.png"), new Date(sent - 60_000), new Date(sent - 60_000));
  fs.writeFileSync(path.join(work, "fake.png"), "not an image");
  fs.mkdirSync(relay.FILES_DIR, { recursive: true });
  fs.writeFileSync(path.join(relay.FILES_DIR, "F9-mine.png"), png);
  const transcript = path.join(home, ".claude/projects/-work/images.jsonl");
  fs.writeFileSync(transcript, [
    { type: "user", message: { content: "take screenshots" }, timestamp: new Date(sent).toISOString(), cwd: work },
    { type: "assistant", cwd: work, message: { content: [{ type: "tool_use", id: "t1", name: "Bash", input: { command: `capture > ${path.join(work, "shot.png")} && crop > probe.png` } }] } },
    { type: "user", cwd: work, message: { content: [{ type: "tool_result", tool_use_id: "t1", content: "Saved out/chart.png and probe.png" }] } },
    { type: "assistant", cwd: work, message: { content: [{ type: "text", text: `Done: ${path.join(work, "shot.png")}, out/chart.png. Also see old.png, fake.png, ${path.join(relay.FILES_DIR, "F9-mine.png")} and gone.png.` }] } },
  ].map((entry) => JSON.stringify(entry)).join("\n"));

  assert.deepEqual(relay.turnMedia(transcript), [path.join(work, "shot.png"), path.join(work, "out/chart.png")], "not probe.png, which only the tools named");

  const state = { dm: { channel: "D1", user: "U1" } };
  await relay.shareMedia(state, transcript, "1791500000.000700");
  assert.deepEqual(uploads.map((u) => u.share.thread_ts), ["1791500000.000700", "1791500000.000700"]);
  assert.deepEqual(calls.filter((c) => c.method === "files.getUploadURLExternal").map((c) => c.params.filename), ["shot.png", "chart.png"]);
  assert.equal(uploads[0].content.length, png.length);
  assert.equal(calls.filter((c) => c.method === "files.completeUploadExternal").length, 1, "shared together, in one message");

  // Many at once: up to MAX_SHARED, ten to a message.
  const many = Array.from({ length: 13 }, (_, i) => path.join(work, `diff-${i + 1}.png`));
  many.forEach((file) => fs.writeFileSync(file, png));
  fs.appendFileSync(transcript, "\n" + JSON.stringify({ type: "assistant", cwd: work, message: { content: [{ type: "text", text: `The diff, in order: ${many.join(", ")}.` }] } }));
  assert.equal(relay.turnMedia(transcript).length, 15);
  calls.length = 0;
  await relay.shareMedia(state, transcript, "1791500000.000700");
  assert.deepEqual(calls.filter((c) => c.method === "files.completeUploadExternal").map((c) => c.params.files.length), [10, 5]);
});

test("a tall image is shared as slices about as tall as it is wide, in order, its pixels kept", async () => {
  const zlib = require("node:zlib");
  // A 10x35 RGBA image, each row stored with one of PNG's five filters in turn,
  // as Chrome mixes them, written by hand so the slicer's reading is checked.
  const width = 10;
  const height = 35;
  const pixel = (x, y) => [(x * 25) & 255, (y * 7) & 255, (x * y) & 255, 255];
  const rows = Array.from({ length: height }, (_, y) => Buffer.from(Array.from({ length: width }, (_, x) => pixel(x, y)).flat()));
  const filtered = rows.map((row, y) => {
    const filter = y % 5;
    const above = y ? rows[y - 1] : Buffer.alloc(row.length);
    const out = Buffer.alloc(row.length);
    for (let i = 0; i < row.length; i++) {
      const a = i >= 4 ? row[i - 4] : 0;
      const b = above[i];
      const c = i >= 4 ? above[i - 4] : 0;
      const p = a + b - c;
      const paeth = Math.abs(p - a) <= Math.abs(p - b) && Math.abs(p - a) <= Math.abs(p - c) ? a : Math.abs(p - b) <= Math.abs(p - c) ? b : c;
      out[i] = (row[i] - [0, a, b, (a + b) >> 1, paeth][filter]) & 255;
    }
    return Buffer.concat([Buffer.from([filter]), out]);
  });
  const chunk = (type, data) => {
    const head = Buffer.alloc(8);
    head.writeUInt32BE(data.length, 0);
    head.write(type, 4, "latin1");
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(zlib.crc32(Buffer.concat([head.subarray(4), data])), 0);
    return Buffer.concat([head, data, crc]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  const png = Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk("IHDR", ihdr), chunk("IDAT", zlib.deflateSync(Buffer.concat(filtered))), chunk("IEND", Buffer.alloc(0))]);

  const parts = relay.sliced({ filename: "diff.png", content: png });
  assert.deepEqual(parts.map((p) => p.filename), ["diff-part1of4.png", "diff-part2of4.png", "diff-part3of4.png", "diff-part4of4.png"]);
  // Even slices (9, 9, 9, 8 rows), and every pixel where it was.
  const back = parts.flatMap((part) => {
    const read = relay.readPng(part.content);
    assert.equal(read.width, width);
    const raw = zlib.inflateSync(Buffer.concat(read.idat));
    return Array.from({ length: read.height }, (_, y) => raw.subarray(y * (width * 4 + 1) + 1, (y + 1) * (width * 4 + 1)));
  });
  assert.deepEqual(parts.map((p) => relay.readPng(p.content).height), [9, 9, 9, 8]);
  assert.deepEqual(back.map((row) => [...row]), rows.map((row) => [...row]));
  // Not tall enough, or not a PNG it reads: as it is.
  assert.equal(relay.sliced({ filename: "wide.png", content: relay.writePng(35, 6, rows.slice(0, 10).map((r) => Buffer.concat([r, r, r, r.subarray(0, 20)]))) }).length, 1);
  assert.deepEqual(relay.sliced({ filename: "x.jpg", content: Buffer.from("not a png") }).map((p) => p.filename), ["x.jpg"]);
});

test("a video you attach is typed into the tab by its path and then its stills, in order", { skip: !fs.existsSync("/opt/homebrew/bin/ffmpeg") && "needs ffmpeg" }, async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const alert = `${Math.floor(Date.now() / 1000) - 30}.000100`;
  const state = { dm: { channel: "D1", user: "U1", team: "T1" }, threads: {}, alerts: { [alert]: { window, index: Number(index), name: "work" } }, reported: {}, following: {} };
  // A real three-second video, made by ffmpeg, as what Slack hands over.
  const recording = path.join(tmp, "recording.mp4");
  execFileSync("ffmpeg", ["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=duration=3:size=1920x1080:rate=30", "-pix_fmt", "yuv420p", recording]);
  relay.slack.get = async () => fs.readFileSync(recording);
  const message = relay.messageFrom(state, {
    type: "message", subtype: "file_share", channel: "D1", user: "U1", client_msg_id: "c1",
    ts: `${Math.floor(Date.now() / 1000)}.000041`, thread_ts: alert, text: "it flickers here",
    files: [{ id: "F0REC", name: "flicker.mp4", mimetype: "video/mp4", size: fs.statSync(recording).size, url_private_download: "https://files.slack.com/files-pri/T1-F0REC/download/flicker.mp4" }],
  }, "T1");

  try {
    await relay.handle(state, message);
    await new Promise((resolve) => setTimeout(resolve, 300));

    const saved = path.join(relay.FILES_DIR, "F0REC-flicker.mp4");
    const stills = Array.from({ length: relay.FRAMES }, (_, i) => `${saved}-frame-0${i + 1}.jpg`);
    stills.forEach((still) => assert.ok(fs.existsSync(still), `${path.basename(still)} was taken`));
    assert.equal(execFileSync("sips", ["-g", "pixelWidth", stills[0]], { encoding: "utf8" }).match(/pixelWidth: (\d+)/)[1], "1280", "scaled down to be read, not kept full size");
    // Compared without spaces, which the pane adds and drops where a long line
    // wraps, and only as far as the second still: the stand-in Claude reads a
    // line as a terminal does, cut at 1024 characters, which the real one isn't.
    const typed = tmux("capture-pane", "-p", "-J", "-t", window).replace(/\s/g, "");
    const expected = `it flickers here [Video: ${saved}] Stills from it, in order: [Image: ${stills[0]}] [Image: ${stills[1]}]`;
    assert.ok(typed.includes(expected.replace(/\s/g, "")), typed.slice(-700));
    assert.ok(!posts.some((p) => /take stills/.test(p.body)));

    // A file that only claims to be a video gets no stills, and you are told.
    relay.slack.get = async () => Buffer.from("not a video");
    await relay.handle(state, { ...message, ts: `${Math.floor(Date.now() / 1000)}.000042`, files: [{ ...message.files[0], id: "F0BAD", name: "bad.mp4" }] });
    await new Promise((resolve) => setTimeout(resolve, 300));
    assert.ok(posts.some((p) => /couldn.t take stills from bad\.mp4, so the tab gets only its path/.test(p.body)));
  } finally {
    tmux("kill-window", "-t", window);
  }
});

test("a video is fetched up to its own size limit; a turn's new videos are shared under it like its images", async () => {
  const video = { id: "F3", name: "a.mp4", mimetype: "video/mp4", size: relay.MAX_FILE_BYTES + 1, url: "https://files.slack.com/files-pri/T1-F3/a.mp4" };
  relay.slack.get = async () => Buffer.from("x");
  assert.equal((await relay.download({ ...video, mimetype: "image/png", name: "a.png" })).error, "a.png is larger than 20 MB.");
  assert.equal((await relay.download({ ...video, size: relay.MAX_VIDEO_BYTES + 1 })).error, "a.mp4 is larger than 200 MB.");
  assert.ok((await relay.download(video)).path, "a video over the image limit is still fetched");

  const work = path.join(tmp, "videowork");
  fs.mkdirSync(work, { recursive: true });
  const sent = Date.now() - 10_000;
  // MP4 and WebM by their first bytes, and one that is only named like a video.
  fs.writeFileSync(path.join(work, "run.mp4"), Buffer.concat([Buffer.from([0, 0, 0, 0x20]), Buffer.from("ftypisom rest")]));
  fs.writeFileSync(path.join(work, "trace.webm"), Buffer.concat([Buffer.from([0x1a, 0x45, 0xdf, 0xa3]), Buffer.from("rest")]));
  fs.writeFileSync(path.join(work, "fake.mov"), "not a video");
  const transcript = path.join(home, ".claude/projects/-work/videos.jsonl");
  fs.writeFileSync(transcript, [
    { type: "user", message: { content: "record the flow" }, timestamp: new Date(sent).toISOString(), cwd: work },
    { type: "assistant", cwd: work, message: { content: [{ type: "text", text: "Recorded run.mp4 and trace.webm; fake.mov is not one." }] } },
  ].map((entry) => JSON.stringify(entry)).join("\n"));

  assert.deepEqual(relay.turnMedia(transcript), [path.join(work, "run.mp4"), path.join(work, "trace.webm")]);
  await relay.shareMedia({ dm: { channel: "D1", user: "U1" } }, transcript, "1791500000.000800");
  assert.deepEqual(calls.filter((call) => call.method === "files.getUploadURLExternal").map((call) => call.params.filename), ["run.mp4", "trace.webm"]);
});

test("status and threads link each tab's thread", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  const [other, otherIndex] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const home = "1791500000.000300";
  const state = {
    dm: { channel: "D1", user: "U1", url: "https://example.slack.com/" },
    threads: { [home]: home }, alerts: {}, reported: {}, following: {},
    homes: { [window]: { ts: home, pid: "1" } },
  };
  const link = "https://example.slack.com/archives/D1/p1791500000000300?thread_ts=1791500000.000300&cid=D1";

  await relay.handle(state, { ts: `${Math.floor(Date.now() / 1000)}.000050`, text: "status", thread_ts: null });
  assert.match(posts[0].body, /\| Tab \| Name \| State \| For \| Context \| Thread \|/);
  assert.ok(posts[0].body.includes(`| **${index}** |`) && posts[0].body.includes(`[open](${link})`));
  assert.match(posts[0].body, new RegExp(`\\| \\*\\*${otherIndex}\\*\\* \\|[^\\n]*\\| — \\|`), "no thread yet");

  await relay.handle(state, { ts: `${Math.floor(Date.now() / 1000)}.000051`, text: "threads", thread_ts: null });
  assert.ok(posts[1].body.startsWith("🤖 **Threads**"));
  assert.match(posts[1].body, /\| Tab \| Name \| State \| Thread \|/);
  assert.ok(posts[1].body.includes(`| **${index}** |`) && posts[1].body.includes(`[open](${link})`));
  assert.match(posts[1].body, new RegExp(`_No thread yet for [^:]*\\btab ${otherIndex}\\b[^:]*: open it on the Home tab and send it a Message to start one\\._$`));
  [window, other].forEach((id) => tmux("kill-window", "-t", id));
});

test("a thread's link opens at its newest message, yours or the relay's", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const home = "1791500000.000300";
  const state = {
    dm: { channel: "D1", user: "U1", url: "https://example.slack.com/" },
    threads: { [home]: home }, alerts: {}, reported: {}, following: {},
    homes: { [window]: { ts: home, pid: "1" } },
  };
  const linkTo = (ts) => `https://example.slack.com/archives/D1/p${ts.replace(".", "")}?thread_ts=${home}&cid=D1`;

  // Nothing newer known: the thread's own first message.
  assert.equal(relay.threadLink(state, home, "open"), `[open](${linkTo(home)})`);

  // The relay posts in it, in two parts: the last part is the newest.
  const text = Array.from({ length: 300 }, (_, i) => `line ${i}`).join("\n");
  await relay.post(state, text, home);
  assert.equal(relay.threadLink(state, home, "open"), `[open](${linkTo(posts.at(-1).ts)})`);

  // Then you reply there, and that is the newest.
  const { processMessages, settled } = relay.dispatcher(state);
  const reply = `${Number(posts.at(-1).ts.split(".")[0]) + 5}.000100`;
  processMessages([{ ts: reply, text: "status", thread_ts: home, byClaude: false }]);
  await settled();
  const newest = [reply, posts.at(-1).ts].sort((x, y) => Number(y) - Number(x))[0];
  assert.equal(relay.threadLink(state, home, "open"), `[open](${linkTo(newest)})`);
  assert.ok(Number(state.latest[home]) >= Number(reply), "your reply counts");
  tmux("kill-window", "-t", window);
});

test("a turn you asked for from Slack is posted once, not also alerted", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const panePid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  fs.writeFileSync(path.join(home, ".claude/sessions", `${panePid}.json`), JSON.stringify({ pid: Number(panePid), sessionId: "asked" }));
  const transcript = path.join(home, ".claude/projects/-work/asked.jsonl");
  fs.writeFileSync(transcript, [
    { type: "user", message: { content: "how does it work" }, timestamp: new Date().toISOString() },
    { type: "assistant", message: { content: [{ type: "text", text: "Like this." }] } },
  ].map((entry) => JSON.stringify(entry)).join("\n"));
  // Working when the pass begins, as the snapshot shows it.
  tmux("set", "-p", "-t", window, "@agent_state", "working");
  tmux("set", "-p", "-t", window, "@agent_since", String(Math.floor(Date.now() / 1000) - 30));
  const home_ = "1791500000.000300";
  const state = {
    dm: { channel: "D1", user: "U1" }, threads: { [home_]: home_ }, alerts: {}, reported: {},
    homes: { [window]: { ts: home_, pid: "1" } },
    following: { [window]: { thread: home_, index: Number(index), name: "work", text: "how does it work", sentAt: Date.now() - 5_000, screen: "", changedAt: 0, taken: true, broadcast: true } },
  };
  const tabs = relay.claudeTabs();
  // Then the turn ends: the hook's event, and the tab marked done.
  const events = [{ event: "done", time: Math.floor(Date.now() / 1000), transcript_path: transcript, last: "Like this." }];
  tmux("set", "-p", "-t", window, "@agent_state", "done");
  tmux("set", "-p", "-t", window, "@agent_since", String(Math.floor(Date.now() / 1000)));
  try {
    // As the relay's pass does it, while you're away.
    await relay.followUps(state, tabs, events);
    await relay.alertTabs(state, relay.claudeTabs(), events);
    await relay.alertTabs(state, relay.claudeTabs(), events);

    assert.equal(posts.length, 1, posts.map((p) => p.body.split("\n")[0]).join(" / "));
    assert.match(posts[0].body, /is done:/);
  } finally {
    fs.rmSync(path.join(home, ".claude/sessions", `${panePid}.json`));
    tmux("kill-window", "-t", window);
  }
});

test("only your presses, on the relay's DM, count; a link button is left to Slack", () => {
  const state = { dm: { channel: "D1", user: "U1", team: "T1" } };
  const payload = {
    type: "block_actions", user: { id: "U1", team_id: "T1" }, team: { id: "T1" }, container: { channel_id: "D1", message_ts: "1.2" },
    message: { ts: "1.2", thread_ts: "1.1", text: "🤖 hi" }, actions: [{ value: JSON.stringify({ a: "cmd", w: "@1", c: "esc" }) }],
  };

  assert.deepEqual(relay.pressFrom(state, payload), { value: { a: "cmd", w: "@1", c: "esc" }, triggerId: undefined, message: { ts: "1.2", thread_ts: "1.1", text: "🤖 hi" } });
  assert.equal(relay.pressFrom(state, { ...payload, user: { id: "U2", team_id: "T1" } }), undefined, "someone else");
  assert.equal(relay.pressFrom(state, { ...payload, container: { channel_id: "C1" } }), undefined, "another conversation");
  assert.equal(relay.pressFrom(state, { ...payload, team: { id: "T2" } }), undefined, "another workspace");
  assert.equal(relay.pressFrom(state, { ...payload, actions: [{ url: "https://example.slack.com/x" }] }), undefined, "a link");
  assert.equal(relay.pressFrom(state, { ...payload, actions: [{ value: "not json" }] }), undefined);
});

test("a button made for one tab answers that tab, even in a thread now linked to another", async () => {
  const [mine, mineIndex] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  const [other, otherIndex] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  ["Allow rm -rf build?", "❯ 1. Yes", "  2. No"].forEach((line) => {
    tmux("send-keys", "-t", mine, "-l", line);
    tmux("send-keys", "-t", mine, "Enter");
  });
  await new Promise((resolve) => setTimeout(resolve, 200));
  tmux("set", "-p", "-t", mine, "@agent_state", "waiting");
  tmux("set", "-p", "-t", mine, "@agent_since", "1791500400");
  // The button was posted in this thread for one tab; the thread has since
  // been linked to the other.
  const thread = `${Math.floor(Date.now() / 1000) - 30}.000400`;
  const state = {
    dm: { channel: "D1", user: "U1" }, threads: { [thread]: thread }, reported: {}, following: {},
    alerts: { [thread]: { window: other, index: Number(otherIndex), name: "other" } },
  };
  const tab = relay.claudeTabs().find((t) => t.id === mine);
  const yes = relay.optionButtons(tab, relay.optionsIn(relay.screen(mine)))[0];
  try {
    await relay.handlePress(state, { value: yes.value, message: { ts: `${Math.floor(Date.now() / 1000)}.000401`, thread_ts: thread, text: "🤖 Tab, waiting" } });
    await new Promise((resolve) => setTimeout(resolve, 200));
    assert.equal(tmux("capture-pane", "-p", "-t", mine).split("\n").filter((line) => line.trim()).at(-1), "1", "the tab it was made for got the answer");
    assert.doesNotMatch(tmux("capture-pane", "-p", "-t", other), /1/, "the thread's other tab got nothing");
    assert.match(posts.at(-1).body, new RegExp(`in tab ${mineIndex}, `));
  } finally {
    [mine, other].forEach((w) => tmux("kill-window", "-t", w));
  }
});

test("a waiting tab's alert has its options as buttons, and a press answers only that prompt", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  ["Allow rm -rf build?", "❯ 1. Yes", "  2. No"].forEach((line) => {
    tmux("send-keys", "-t", window, "-l", line);
    tmux("send-keys", "-t", window, "Enter");
  });
  await new Promise((resolve) => setTimeout(resolve, 200));
  tmux("set", "-p", "-t", window, "@agent_state", "waiting");
  tmux("set", "-p", "-t", window, "@agent_since", "1791500200");
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };

  await relay.alertTabs(state);
  const sent = calls.filter((c) => c.method === "chat.postMessage").find((c) => /is \*\*waiting on you\*\*\. Reply here/.test(c.params.text));
  const buttons = sent.params.blocks[1].elements;
  assert.deepEqual(buttons.map((b) => b.text.text), ["1. Yes", "2. No"]);
  assert.equal(buttons[0].style, "primary");
  const yes = JSON.parse(buttons[0].value);
  const alertTs = posts.find((p) => p.body === sent.params.text).ts;
  const message = { ts: alertTs, thread_ts: posts.find((p) => p.body === sent.params.text).thread, text: sent.params.text };

  // Made for an earlier wait: refused, nothing pressed.
  await relay.handlePress(state, { value: { ...yes, s: "1791500100" }, message });
  assert.match(posts.at(-1).body, /That choice isn't showing any more/);
  // Made for another prompt: refused too.
  await relay.handlePress(state, { value: { ...yes, k: "0000000000000000" }, message });
  assert.match(posts.at(-1).body, /That choice isn't showing any more/);
  assert.notEqual(tmux("capture-pane", "-p", "-t", window).split("\n").filter((line) => line.trim()).at(-1), "1");

  await relay.handlePress(state, { value: yes, message });
  await new Promise((resolve) => setTimeout(resolve, 200));
  assert.equal(tmux("capture-pane", "-p", "-t", window).split("\n").filter((line) => line.trim()).at(-1), "1");
  const edited = updates.find((u) => u.ts === alertTs);
  assert.match(edited.body, /_Chose \*\*1\. Yes\*\*\._$/);
  assert.equal(calls.find((c) => c.method === "chat.update" && c.params.ts === alertTs).params.blocks.length, 1, "no buttons left to press twice");
  tmux("kill-window", "-t", window);
});

test("a finished turn offers Skill, and nothing that answers for you, even when Claude ended by asking", async () => {
  const [window] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const panePid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  fs.writeFileSync(path.join(home, ".claude/sessions", `${panePid}.json`), JSON.stringify({ pid: Number(panePid), sessionId: "asking" }));
  const transcript = path.join(home, ".claude/projects/-work/asking.jsonl");
  const turn = (said) => fs.writeFileSync(transcript, [
    { type: "user", message: { content: "fix the build" } },
    { type: "assistant", message: { content: [{ type: "text", text: said }] } },
  ].map((entry) => JSON.stringify(entry)).join("\n"));
  const finish = async (since) => {
    tmux("set", "-p", "-t", window, "@agent_state", "done");
    tmux("set", "-p", "-t", window, "@agent_since", String(since));
    await relay.alertTabs(state);
    const sent = calls.filter((c) => c.method === "chat.postMessage" && /has finished\. Reply here/.test(c.params.text)).at(-1);
    return { sent, buttons: sent.params.blocks[1].elements };
  };
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  try {
    turn("Fixed the build. Every check passes.");
    const plain = await finish(Math.floor(Date.now() / 1000));
    assert.deepEqual(plain.buttons.map((b) => b.text.text), ["Skill"], "nothing running to stop, nothing asked");

    turn("I found two more failures. **Want me to fix them too?**");
    const asked = await finish(Math.floor(Date.now() / 1000) + 1);
    assert.deepEqual(asked.buttons.map((b) => b.text.text), ["Skill"], "a question is answered in the thread");
  } finally {
    fs.rmSync(path.join(home, ".claude/sessions", `${panePid}.json`));
    tmux("kill-window", "-t", window);
  }
});
test("a finished turn that asks gets the replies you'd tap, from Haiku's reading of it, and a stale one isn't typed", async () => {
  const [window] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const panePid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  fs.writeFileSync(path.join(home, ".claude/sessions", `${panePid}.json`), JSON.stringify({ pid: Number(panePid), sessionId: "quick" }));
  const transcript = path.join(home, ".claude/projects/-work/quick.jsonl");
  const turn = (said) => fs.writeFileSync(transcript, [
    { type: "user", message: { content: "fix the build" } },
    { type: "assistant", message: { content: [{ type: "text", text: said }] } },
  ].map((entry) => JSON.stringify(entry)).join("\n"));
  // A stand-in Haiku that answers what the test says, and counts its calls.
  const bin = path.join(tmp, "quick-bin");
  const answer = path.join(tmp, "quick-answer.json");
  const asked = path.join(tmp, "quick-asked");
  fs.mkdirSync(bin, { recursive: true });
  fs.writeFileSync(path.join(bin, "claude"), `#!/bin/sh\ncat >/dev/null\necho x >> '${asked}'\ncat '${answer}'\n`, { mode: 0o755 });
  const haiku = (reply) => fs.writeFileSync(answer, JSON.stringify({ result: JSON.stringify(reply), total_cost_usd: 0.0001 }));
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  const tab = () => relay.claudeTabs().find((t) => t.id === window);
  const pathBefore = process.env.PATH;
  process.env.PATH = `${bin}:${pathBefore}`;
  try {
    // Asking whether to go on: Yes, go ahead, before Skill.
    turn("Fixed it, and found two more failures. Want me to fix them too?");
    haiku({ kind: "permission", options: [] });
    const first = await relay.post(state, "🤖 The turn", undefined, { buttons: relay.turnButtons(tab()) });
    await relay.offerQuickReplies(state, tab(), first, "Fixed it, and found two more failures. Want me to fix them too?");
    const offered = calls.filter((call) => call.method === "chat.update").at(-1).params;
    assert.equal(offered.ts, first);
    assert.deepEqual(offered.blocks[1].elements.map((b) => b.text.text), ["Yes, go ahead", "Skill"]);

    // Asking you to pick: a button per option.
    haiku({ kind: "choice", options: ["Postgres", "SQLite"] });
    assert.deepEqual((await relay.quickReplies(tab(), "Which should back it?\n1. Postgres\n2. SQLite")).map((b) => b.text), ["Postgres", "SQLite"]);
    // A question only your words answer: nothing.
    haiku({ kind: "none", options: [] });
    assert.deepEqual(await relay.quickReplies(tab(), "What should the new page be called?"), []);
    // No question at all: Haiku isn't even asked.
    const calledBefore = fs.readFileSync(asked, "utf8").split("\n").filter(Boolean).length;
    assert.deepEqual(await relay.quickReplies(tab(), "Fixed the build. Every check passes."), []);
    assert.equal(fs.readFileSync(asked, "utf8").split("\n").filter(Boolean).length, calledBefore);

    // Tapped: typed into the tab as your reply.
    const yes = offered.blocks[1].elements[0];
    await relay.handlePress(state, { value: JSON.parse(yes.value), message: { ts: first, thread_ts: undefined, text: "🤖 The turn" } });
    await new Promise((resolve) => setTimeout(resolve, 300));
    assert.match(tmux("capture-pane", "-p", "-t", window), /Yes, go ahead/);
    assert.match(updates.at(-1).body, /_Replied \*\*Yes, go ahead\*\*\._/);

    // Claude has said more since: the old reply isn't typed.
    turn("Done with those too. All green.");
    await relay.handlePress(state, { value: JSON.parse(yes.value), message: { ts: first, thread_ts: undefined, text: "🤖 The turn" } });
    assert.match(posts.at(-1).body, /Claude has said more since, so that reply is out of date/);
  } finally {
    process.env.PATH = pathBefore;
    fs.rmSync(path.join(home, ".claude/sessions", `${panePid}.json`), { force: true });
    tmux("kill-window", "-t", window);
  }
});

test("a message that names no tab can be sent to one with a tap", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  const ts = `${Math.floor(Date.now() / 1000)}.000060`;

  await relay.handle(state, { ts, text: "run the linter please", thread_ts: null });
  const picker = calls.find((c) => c.method === "chat.postMessage").params;
  const button = picker.blocks[1].elements.find((b) => JSON.parse(b.value).w === window);
  await relay.handlePress(state, { value: JSON.parse(button.value), message: { ts: posts[0].ts, text: picker.text } });
  await new Promise((resolve) => setTimeout(resolve, 200));

  assert.match(tmux("capture-pane", "-p", "-t", window), /run the linter please/);
  assert.match(updates.find((u) => u.ts === posts[0].ts).body, new RegExp(`^🤖 Sent to \\*\\*tab ${index}, `));
  assert.equal(state.following[window].request, ts, "your message gets the reactions");
  // The same button again: the message is spent.
  await relay.handlePress(state, { value: JSON.parse(button.value), message: { ts: posts[0].ts, text: picker.text } });
  assert.match(posts.at(-1).body, /I no longer have that message/);
  tmux("kill-window", "-t", window);
});

test("your message's reaction shows where it is: 👀 once taken, ✅ once answered", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const transcript = path.join(home, ".claude/projects/-work/lifecycle.jsonl");
  fs.writeFileSync(transcript, [
    { type: "user", message: { content: "ship it" } },
    { type: "assistant", message: { content: [{ type: "text", text: "Shipped." }] } },
  ].map((entry) => JSON.stringify(entry)).join("\n"));
  const request = `${Math.floor(Date.now() / 1000)}.000070`;
  const state = {
    dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {},
    following: { [window]: { thread: "1791500000.000100", index: Number(index), name: "work", text: "ship it", request, sentAt: Date.now() - 5_000, screen: "", changedAt: 0, taken: false } },
  };

  await relay.followUps(state, relay.claudeTabs(), [{ event: "done", time: Math.floor(Date.now() / 1000), transcript_path: transcript }]);

  assert.deepEqual(reactions.map((r) => r.emoji), ["eyes", "white_check_mark"]);
  assert.ok(reactions.every((r) => r.ts === request));
  assert.deepEqual(calls.filter((c) => c.method === "reactions.remove").map((c) => c.params.name), ["hourglass_flowing_sand", "eyes"]);
  tmux("kill-window", "-t", window);
});

test("a tab's thread header follows its name and state, and says when its session has ended", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const pid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  const home = "1791500000.000300";
  const tab = relay.claudeTabs().find((t) => t.id === window);
  const state = { dm: { channel: "D1", user: "U1" }, threads: { [home]: home }, alerts: {}, homes: { [window]: { ts: home, pid, label: `${index}, ${tab.name}`, shown: relay.headerKey(tab) } } };

  await relay.refreshHomes(state, relay.claudeTabs());
  assert.equal(updates.length, 0, "unchanged");

  tmux("rename-window", "-t", window, "ABC-214: Make the thing");
  await relay.refreshHomes(state, relay.claudeTabs());
  assert.equal(updates[0].ts, home);
  assert.match(updates[0].body, new RegExp(`^🤖 \\*\\*Tab ${index}, ABC-214: Make the thing\\*\\*\\n⚪ Idle\\n\\nThis is its thread`));
  const buttons = calls.filter((c) => c.method === "chat.update").at(-1).params.blocks[1].elements;
  assert.deepEqual(buttons.map((b) => b.text.text), ["Progress", "Skill", "Rename", "Close"]);

  // A new state: the header says so, and since when.
  tmux("set", "-p", "-t", window, "@agent_state", "waiting");
  tmux("set", "-p", "-t", window, "@agent_since", String(Math.floor(Date.now() / 1000)));
  await relay.refreshHomes(state, relay.claudeTabs());
  assert.match(updates.at(-1).body, /\n🔴 \*\*Waiting on you\*\* since \d{1,2}:\d{2}/);

  // The window now runs another session: the old thread is marked closed.
  state.homes[window].pid = "999999";
  await relay.refreshHomes(state, relay.claudeTabs());
  assert.match(updates.at(-1).body, /^🤖 ~~Tab .*~~\n⚫ Closed at /);
  const closedButtons = calls.filter((c) => c.method === "chat.update").at(-1).params.blocks[1].elements;
  assert.deepEqual(closedButtons.map((b) => b.text.text), ["Delete thread"], "only the offer to delete it");
  assert.ok(closedButtons[0].confirm, "which Slack confirms");
  assert.equal(state.homes[window], undefined);
  assert.ok(state.closedHomes[home], "recorded as a thread that may be deleted");
  tmux("kill-window", "-t", window);
});

test("a message sent mid-turn is answered with that turn, not the screen", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const transcript = path.join(home, ".claude/projects/-work/queued.jsonl");
  // As Claude Code writes it: the turn's prompt, then the message you sent
  // while it worked, queued into the same turn, then the answer.
  fs.writeFileSync(transcript, [
    { type: "user", message: { content: "Put together an explainer" } },
    { type: "assistant", message: { content: [{ type: "tool_use", id: "t1", name: "Write", input: { file_path: "/tmp/x.html" } }] } },
    { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "t1", content: "ok" }] } },
    { type: "attachment", attachment: { type: "queued_command", prompt: "Don’t forget to cover security", commandMode: "prompt" } },
    { type: "attachment", attachment: { type: "queued_command", prompt: "<agent-message from=\"a1\">\n[Subagent hand-back] The report follows:\n  No findings.\n</agent-message>", commandMode: "prompt", origin: { kind: "peer", from: "a1" } } },
    { type: "attachment", attachment: { type: "queued_command", prompt: "<task-notification>\n<task-id>a1</task-id>\n<summary>Agent finished</summary>\n</task-notification>", commandMode: "task-notification", origin: { kind: "task-notification" } } },
    { type: "assistant", message: { content: [{ type: "text", text: "Published, with a security section." }] } },
  ].map((entry) => JSON.stringify(entry)).join("\n"));

  const turn = relay.turnFrom(transcript, "Don’t forget to cover security");
  assert.ok(turn, "the queued message belongs to this turn");
  assert.match(turn, /> Put together an explainer\n> Don’t forget to cover security\n\nPublished, with a security section\./);
  assert.doesNotMatch(turn, /agent-message|task-notification|Subagent/, "a subagent's report and a task's notice are not quoted as yours");
  const reported = path.join(home, ".claude/projects/-work/reported.jsonl");
  fs.writeFileSync(reported, [
    { type: "user", origin: { kind: "peer", handbackReportText: "No findings." }, message: { content: "Another Claude session sent a message:\n<agent-message from=\"a1\">\n[Subagent hand-back] The report follows:\n  No findings.\n</agent-message>" } },
    { type: "assistant", message: { content: [{ type: "text", text: "Clean, pushing." }] } },
  ].map((entry) => JSON.stringify(entry)).join("\n"));
  assert.match(relay.turnFrom(reported), /> An agent reported back: No findings\.\n\nClean, pushing\./, "a turn a subagent's report started reads as that report");
  assert.equal(relay.turnFrom(transcript, "something never sent"), undefined);

  // The relay typed the queued message; its answer is the turn, not the screen.
  const state = {
    dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {},
    following: { [window]: { thread: "1791500000.000100", index: Number(index), name: "work", text: "Don’t forget to cover security", sentAt: Date.now() - 5_000, screen: "", changedAt: 0, taken: true } },
  };
  await relay.followUps(state, relay.claudeTabs(), [{ event: "done", time: Math.floor(Date.now() / 1000), transcript_path: transcript, last: "Published, with a security section." }]);
  assert.equal(posts.length, 1);
  assert.match(posts[0].body, /is done:[\s\S]*Published, with a security section\./);
  assert.doesNotMatch(posts[0].body, /```/, "not a screen capture");
  tmux("kill-window", "-t", window);
});

test("a tab's screen you have closed is forgotten without an error in the log", async () => {
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  relay.openScreens.set("VCLOSED", { w: "@999", p: "1", state: "idle", since: "", at: Date.now() });
  answers["views.update"] = () => {
    throw new Error("views.update: not_found");
  };
  await relay.refreshScreens(state, []);
  assert.equal(relay.openScreens.has("VCLOSED"), false);
  const logged = fs.existsSync(relay.LOG_FILE) ? fs.readFileSync(relay.LOG_FILE, "utf8") : "";
  assert.doesNotMatch(logged, /screen update failed: views\.update: not_found/);
});

test("Open on the Home tab opens a screen for exactly that tab, and only for you", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const pid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  const state = { dm: { channel: "D1", user: "U1", team: "T1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  const press = (view, value, user = "U1") => ({ type: "block_actions", user: { id: user, team_id: "T1" }, team: { id: "T1" }, trigger_id: "TR1", container: { type: "view", view_id: view.id }, view, actions: [{ value: JSON.stringify(value) }] });
  const home = { id: "VHOME", type: "home" };
  const modal = { id: "VTAB", type: "modal" };

  // Each Home row has an Open button for its own tab and process.
  answers["views.open"] = (params) => (views.push(params.view), { ok: true, view: { id: "VTAB" } });
  await relay.refreshDashboard(state, { quick: true, opened: true });
  const row = views.at(-1).blocks.find((b) => b.type === "actions" && JSON.parse(b.elements[0].value).w === window);
  assert.equal(row.elements[0].text.text, "Open");
  assert.deepEqual(JSON.parse(row.elements[0].value), { a: "open", w: window, p: pid });

  // Someone else's press, a screen's button pressed on the Home tab, and Open
  // pressed on a screen all count for nothing.
  assert.equal(relay.pressFrom(state, press(home, { a: "open", w: window, p: pid }, "U2")), undefined);
  assert.equal(relay.pressFrom(state, press(home, { a: "screen-close", w: window, p: pid })), undefined);
  assert.equal(relay.pressFrom(state, press(home, { a: "close", w: window, p: pid })), undefined, "Close is on the screen now, not the Home tab");
  assert.equal(relay.pressFrom(state, press(modal, { a: "open", w: window, p: pid })), undefined);
  assert.equal(relay.pressFrom(state, press(modal, { a: "close", w: window, p: pid })), undefined);

  await relay.handlePress(state, relay.pressFrom(state, press(home, JSON.parse(row.elements[0].value))));
  const opened = calls.find((c) => c.method === "views.open");
  assert.equal(opened.params.trigger_id, "TR1");
  const screen = opened.params.view;
  assert.equal(screen.type, "modal");
  assert.ok(screen.title.text.startsWith(`${index} · `), "the tab's number and name");
  assert.ok(screen.title.text.length <= 24, "Slack's limit");
  assert.deepEqual(JSON.parse(screen.private_metadata), { w: window, p: pid });
  const buttons = screen.blocks.find((b) => b.type === "actions").elements;
  assert.deepEqual(buttons.map((b) => b.text.text), ["Message", "Skill", "Progress", "Rename", "Close"], "no Stop for an idle tab, no thread it hasn't got");
  assert.ok(buttons.every((b) => JSON.parse(b.value).w === window && JSON.parse(b.value).p === pid));
  assert.ok(buttons.at(-1).confirm, "Close asks once more");

  // A screen drawn for an earlier tab in this window acts on nothing.
  await relay.handlePress(state, relay.pressFrom(state, press(modal, { a: "screen-close", w: window, p: "999999" })));
  assert.ok(tmux("list-windows", "-F", "#{window_id}").split("\n").includes(window));
  assert.match(calls.at(-1).params.view.blocks[0].text.text, /That tab has closed/);

  // Stop on an idle tab says so on the screen, and presses nothing.
  answers["views.update"] = () => ({ ok: true });
  await relay.handlePress(state, relay.pressFrom(state, press(modal, { a: "screen-stop", w: window, p: pid })));
  const stopped = calls.filter((c) => c.method === "views.update").at(-1).params;
  assert.equal(stopped.view_id, "VTAB");
  assert.match(stopped.view.blocks.at(-1).text.text, /isn't in the middle of anything/);

  // Close from the screen closes the tab and says so there, and in its thread.
  const thread = `${Math.floor(Date.now() / 1000)}.000300`;
  state.homes = { [window]: { ts: thread, pid } };
  await relay.handlePress(state, relay.pressFrom(state, press(modal, { a: "screen-close", w: window, p: pid })));
  assert.ok(!tmux("list-windows", "-F", "#{window_id}").split("\n").includes(window), "closed");
  assert.equal(posts.at(-1).thread, thread);
  assert.ok(views.length >= 2, "the Home tab redrawn");
  assert.match(calls.filter((c) => c.method === "views.update").at(-1).params.view.blocks[0].text.text, new RegExp(`^Closed tab ${index}, `));
});

test("Rename on a tab's screen asks for the name, and Save renames that tab", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const pid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  tmux("set-option", "-w", "-t", window, "@tab_name", "old name");
  const state = { dm: { channel: "D1", user: "U1", team: "T1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  answers["views.push"] = () => ({ ok: true });
  answers["views.update"] = () => ({ ok: true });

  await relay.handlePress(state, relay.pressFrom(state, { type: "block_actions", user: { id: "U1", team_id: "T1" }, team: { id: "T1" }, trigger_id: "TR2", container: { type: "view", view_id: "VTAB" }, view: { id: "VTAB", type: "modal" }, actions: [{ value: JSON.stringify({ a: "screen-rename", w: window, p: pid }) }] }));
  const pushed = calls.find((c) => c.method === "views.push").params;
  assert.equal(pushed.trigger_id, "TR2");
  assert.equal(pushed.view.callback_id, "rename");
  assert.equal(pushed.view.blocks[0].element.initial_value, "old name", "starts with the name it has");

  const save = (name, user = "U1") => ({ type: "view_submission", user: { id: user, team_id: "T1" }, team: { id: "T1" }, view: { ...pushed.view, root_view_id: "VTAB", state: { values: { name: { name: { value: name } } } } } });
  assert.equal(relay.submissionFrom(state, save("hijacked", "U2")), undefined, "only yours");
  assert.equal(relay.submissionFrom(state, { ...save("x"), view: { ...save("x").view, callback_id: "other" } }), undefined);

  await relay.handleSubmission(state, relay.submissionFrom(state, save("  new   name ")));
  assert.equal(tmux("show-options", "-wqv", "-t", window, "@tab_name"), "new name");
  const redrawn = calls.filter((c) => c.method === "views.update").at(-1).params;
  assert.equal(redrawn.view_id, "VTAB", "the tab's screen under it");
  assert.match(redrawn.view.blocks.at(-1).text.text, new RegExp(`Renamed \\*tab ${index}\\* to \\*new name\\*`));

  // Empty gives the tab its own name back.
  await relay.handleSubmission(state, relay.submissionFrom(state, save("")));
  assert.equal(tmux("show-options", "-wqv", "-t", window, "@tab_name"), "");
  tmux("kill-window", "-t", window);
});

test("a tab's screen shows a waiting prompt's options, answers only that prompt, and is drawn again when the tab changes", async () => {
  const [window] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const pid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  ["Allow rm -rf build?", "❯ 1. Yes", "  2. No"].forEach((line) => {
    tmux("send-keys", "-t", window, "-l", line);
    tmux("send-keys", "-t", window, "Enter");
  });
  await new Promise((resolve) => setTimeout(resolve, 200));
  tmux("set", "-p", "-t", window, "@agent_state", "waiting");
  tmux("set", "-p", "-t", window, "@agent_since", "1791500200");
  const home = `${Math.floor(Date.now() / 1000)}.000400`;
  const state = { dm: { channel: "D1", user: "U1", team: "T1", url: "https://example.slack.com/" }, threads: { [home]: home }, alerts: {}, reported: {}, following: {}, homes: { [window]: { ts: home, pid } } };
  const press = (value) => ({ type: "block_actions", user: { id: "U1", team_id: "T1" }, team: { id: "T1" }, trigger_id: "TR1", container: { type: "view", view_id: "VTAB" }, view: { id: "VTAB", type: "modal" }, actions: [{ value: JSON.stringify(value) }] });
  answers["views.open"] = () => ({ ok: true, view: { id: "VTAB" } });
  answers["views.update"] = () => ({ ok: true });
  try {
    await relay.handlePress(state, { value: { a: "open", w: window, p: pid }, view: { id: "VHOME" }, triggerId: "TR0" });
    const screen = calls.find((c) => c.method === "views.open").params.view;
    assert.equal(screen.notify_on_close, true, "Slack says when it is closed");
    const choices = screen.blocks.filter((b) => b.type === "actions")[0].elements;
    assert.deepEqual(choices.map((b) => b.text.text), ["1. Yes", "2. No"]);
    const yes = JSON.parse(choices[0].value);
    assert.equal(yes.a, "screen-opt");
    assert.ok(relay.pressFrom(state, press(yes)), "yours, from the screen");
    assert.ok(relay.openScreens.has("VTAB"));

    // Made for an earlier wait: refused, nothing pressed.
    await relay.handlePress(state, relay.pressFrom(state, press({ ...yes, s: "1791500100" })));
    assert.match(calls.filter((c) => c.method === "views.update").at(-1).params.view.blocks.at(-1).text.text, /That choice isn't showing any more/);
    assert.notEqual(tmux("capture-pane", "-p", "-t", window).split("\n").filter((line) => line.trim()).at(-1), "1");

    await relay.handlePress(state, relay.pressFrom(state, press(yes)));
    await new Promise((resolve) => setTimeout(resolve, 200));
    assert.equal(tmux("capture-pane", "-p", "-t", window).split("\n").filter((line) => line.trim()).at(-1), "1", "only the number, as a reply would be");
    assert.ok(posts.some((p) => p.thread === home && p.body === "🤖 _From the Home tab:_ chose **1. Yes**"), "said in its thread");
    assert.match(calls.filter((c) => c.method === "views.update").at(-1).params.view.blocks.at(-1).text.text, /^Chose \*1\. Yes\*\. What happens next goes in <https:/);

    // Nothing changed: not drawn again. Its state changed: drawn again, without the options.
    const drawn = calls.filter((c) => c.method === "views.update").length;
    await relay.refreshScreens(state);
    assert.equal(calls.filter((c) => c.method === "views.update").length, drawn);
    tmux("set", "-p", "-t", window, "@agent_state", "working");
    await relay.refreshScreens(state);
    const redrawn = calls.filter((c) => c.method === "views.update").at(-1).params;
    assert.equal(redrawn.view_id, "VTAB");
    assert.match(redrawn.view.blocks[0].text.text, /🔵 working/);
    assert.ok(!redrawn.view.blocks.some((b) => /choose/.test(b.text?.text ?? "")));

    // Closed in Slack: no longer drawn.
    relay.forgetScreen("VTAB");
    tmux("set", "-p", "-t", window, "@agent_state", "done");
    await relay.refreshScreens(state);
    assert.equal(calls.filter((c) => c.method === "views.update").at(-1).params, redrawn);
  } finally {
    relay.openScreens.clear();
    tmux("kill-window", "-t", window);
  }
});

test("a tab's screen shows Claude's Markdown as the mrkdwn a modal renders", () => {
  assert.equal(
    relay.asMrkdwn("## Done\n\nIt now *names* the tab:\n\n- **Title:** \"3 · x\" & <y>\n  - see [the PR](https://example.com/1)\n\nNow `a **b** <c>`.\n```\nif (a < b) **x**\n```"),
    "*Done*\n\nIt now _names_ the tab:\n\n• *Title:* \"3 · x\" &amp; &lt;y&gt;\n  • see <https://example.com/1|the PR>\n\nNow `a **b** &lt;c&gt;`.\n```\nif (a &lt; b) **x**\n```",
  );
});

test("a tab's screen shows a Markdown table as a bullet a row, mrkdwn having no tables", () => {
  assert.equal(
    relay.asMrkdwn("Today:\n\n| Time | Event | Length |\n|:---|---|--:|\n| 12:00 PM | [Huddle](https://example.com/a) | 30m |\n| 1:00 PM | Standup | `15m` |\n\nDone."),
    "Today:\n\n• *12:00 PM* · <https://example.com/a|Huddle> · 30m\n• *1:00 PM* · Standup · `15m`\n\nDone.",
  );
});

test("Message on a tab's screen types it into the tab, as a reply in its thread", async () => {
  const [window] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const pid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  const state = { dm: { channel: "D1", user: "U1", team: "T1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  answers["views.push"] = () => ({ ok: true });
  answers["views.update"] = () => ({ ok: true });
  try {
    await relay.handlePress(state, { value: { a: "screen-message", w: window, p: pid }, view: { id: "VTAB" }, triggerId: "TR3" });
    const pushed = calls.find((c) => c.method === "views.push").params.view;
    assert.equal(pushed.callback_id, "message");
    assert.match(pushed.blocks[0].label.text, new RegExp(`^Message to tab \\d+, `), "names the tab in full");
    assert.ok(pushed.title.text.length <= 24);
    const send = (text, user = "U1") => ({ type: "view_submission", user: { id: user, team_id: "T1" }, team: { id: "T1" }, view: { ...pushed, root_view_id: "VTAB", state: { values: { text: { text: { value: text } } } } } });
    assert.equal(relay.submissionFrom(state, send("rm -rf /", "U2")), undefined, "only yours");

    await relay.handleSubmission(state, relay.submissionFrom(state, send("run the tests")));
    await new Promise((resolve) => setTimeout(resolve, 600));
    assert.match(tmux("capture-pane", "-p", "-t", window), /run the tests/);
    const home = state.homes[window].ts;
    assert.ok(posts.some((p) => p.ts === home), "a thread made for it");
    assert.ok(posts.some((p) => p.thread === home && p.body === "🤖 _From the Home tab:_ run the tests"));
    assert.equal(state.following[window].thread, home, "its answer goes in that thread");
    assert.match(calls.filter((c) => c.method === "views.update").at(-1).params.view.blocks.at(-1).text.text, /^Sent\. The answer goes in /);
  } finally {
    tmux("kill-window", "-t", window);
  }
});

test("New tab on the Home tab starts Claude with the prompt, from you only", async () => {
  const state = { dm: { channel: "D1", user: "U1", team: "T1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  answers["views.open"] = () => ({ ok: true, view: { id: "VNEW" } });
  const home = { type: "block_actions", user: { id: "U1", team_id: "T1" }, team: { id: "T1" }, trigger_id: "TR5", container: { type: "view", view_id: "VHOME" }, view: { id: "VHOME", type: "home" }, actions: [{ value: JSON.stringify({ a: "new" }) }] };
  await relay.handlePress(state, relay.pressFrom(state, home));
  const form = calls.find((c) => c.method === "views.open").params.view;
  assert.equal(form.callback_id, "new");
  // Where is picked from the menu, with the field beside it left empty.
  const start = (place, prompt, user = "U1") => ({
    type: "view_submission", user: { id: user, team_id: "T1" }, team: { id: "T1" },
    view: { ...form, state: { values: { place: { place: { selected_option: { value: place } } }, typed: { typed: { value: null } }, prompt: { prompt: { value: prompt } } } } },
  });
  assert.equal(relay.submissionFrom(state, start(tmp, "hi", "U2")), undefined, "only yours");

  const before = tmux("list-windows", "-F", "#{window_id}").split("\n");
  await relay.handleSubmission(state, relay.submissionFrom(state, start(tmp, "fix the flaky test")));
  const added = tmux("list-windows", "-F", "#{window_id}").split("\n").filter((w) => !before.includes(w));
  try {
    assert.equal(added.length, 1);
    assert.equal(tmux("display", "-p", "-t", added[0], "#{pane_current_path}"), tmp);
    await new Promise((resolve) => setTimeout(resolve, 300));
    assert.match(tmux("capture-pane", "-p", "-t", added[0]), /claude 'fix the flaky test'/);
    assert.match(posts.at(-1).body, /^🤖 Started a new Claude in tab \d+/);
    assert.equal(posts.at(-1).thread, undefined, "at the top level, as its thread");
  } finally {
    added.forEach((w) => tmux("kill-window", "-t", w));
  }
});

test("New tab's Where menu lists every checkout and directory Claude has run in that still exists, at its top level, newest first, by repository", () => {
  const repo = path.join(tmp, "menu-repo");
  const worktree = path.join(tmp, "menu-worktree");
  const plain = path.join(tmp, "menu-plain");
  fs.mkdirSync(path.join(repo, "sub"), { recursive: true });
  fs.mkdirSync(plain, { recursive: true });
  execFileSync("git", ["init", "-q", "-b", "main", repo]);
  execFileSync("git", ["-C", repo, "-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "-q", "--allow-empty", "-m", "first"]);
  execFileSync("git", ["-C", repo, "worktree", "add", "-q", "-b", "abc-12-thing", worktree]);
  // One session in each, a day apart, oldest first, and one where the directory is gone.
  const projects = path.join(home, ".claude/projects/-menu");
  fs.mkdirSync(projects, { recursive: true });
  [repo, path.join(repo, "sub"), worktree, plain, path.join(tmp, "menu-gone")].forEach((dir, i) => {
    const file = path.join(projects, `menu-${i}.jsonl`);
    fs.writeFileSync(file, `${JSON.stringify({ type: "user", cwd: dir, message: { content: "hi" } })}\n`);
    const at = new Date(Date.now() - (10 - i) * 86_400_000);
    fs.utimesSync(file, at, at);
  });
  try {
    const groups = relay.placeChoices();
    const mine = groups.find((g) => g.group === "menu-repo");
    // The session in sub counts for the main checkout, the more recent of the two.
    assert.deepEqual(mine.choices.map((choice) => [choice.label, choice.value]), [
      ["⎇ menu-worktree", worktree],
      ["menu-repo", repo],
    ]);
    assert.equal(groups.at(-1).group, "Other directories", "directories outside a repository come last");
    assert.ok(groups.at(-1).choices.some((choice) => choice.value === plain));
    assert.ok(!groups.flatMap((g) => g.choices).some((choice) => choice.value.endsWith("menu-gone")), "a directory that's gone isn't offered");

    const form = relay.newScreen();
    const menu = form.blocks.find((block) => block.block_id === "place").element;
    assert.equal(menu.type, "static_select");
    assert.deepEqual(menu.option_groups.find((g) => g.label.text === "menu-repo").options.map((o) => o.value), [worktree, repo]);
    assert.ok(form.blocks.some((block) => block.block_id === "typed"), "and a field for anywhere else");
  } finally {
    fs.rmSync(projects, { recursive: true, force: true });
    execFileSync("git", ["-C", repo, "worktree", "remove", "--force", worktree]);
  }
});

test("a tab's skills come from its session's list and from your and its project's skill folders", async () => {
  const project = path.join(tmp, "skills-project");
  fs.mkdirSync(path.join(project, ".claude/commands"), { recursive: true });
  fs.writeFileSync(path.join(project, ".claude/commands/deploy.md"), "---\ndescription: Deploy this project\n---\nDeploy it.\n");
  fs.mkdirSync(path.join(home, ".claude/skills/shape"), { recursive: true });
  fs.writeFileSync(path.join(home, ".claude/skills/shape/SKILL.md"), "---\nname: shape\ndescription: \"Shape a ticket\"\nargument-hint: \"[idea or ticket ID]\"\ndisable-model-invocation: true\n---\n");
  const [window] = tmux("new-window", "-d", "-c", project, "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const panePid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  fs.writeFileSync(path.join(home, ".claude/sessions", `${panePid}.json`), JSON.stringify({ pid: Number(panePid), sessionId: "skilled" }));
  const transcript = path.join(home, ".claude/projects/-work/skilled.jsonl");
  const listing = (isInitial, names, content) => JSON.stringify({ type: "attachment", attachment: { type: "skill_listing", isInitial, names, content } });
  fs.writeFileSync(transcript, [
    // An earlier session's list, replaced by the next full one.
    listing(true, ["gone"], "- gone: An old skill"),
    listing(true, ["review-loop", "aikido:scan"], "- review-loop: Review the branch with Codex\n- aikido:scan (aikido:aikido-scan): Scan for issues"),
    listing(false, ["pdf"], "- pdf: Work with PDFs"),
  ].join("\n"));
  try {
    const skills = relay.skillsFor(relay.claudeTabs().find((t) => t.id === window));
    // Yours, then the project's, then Claude Code's own commands that need no
    // answer, then the rest of what the session lists.
    assert.deepEqual(skills.map((s) => [s.source, s.name]), [["yours", "shape"], ["project", "deploy"], ["claude", "clear"], ["claude", "compact"], ["claude", "context"], ["claude", "cost"], ["claude", "usage"], ["session", "aikido:scan"], ["session", "pdf"], ["session", "review-loop"]]);
    assert.equal(skills.find((s) => s.name === "compact").hint, "[what the summary should keep]");

    // The ones you use most go first, most used first, each use counting for
    // less the longer ago it was: deploy's 40 uses were months ago. A skill
    // used once, or one this tab can't run, isn't among them.
    const day = 86_400_000;
    fs.writeFileSync(path.join(home, ".claude.json"), JSON.stringify({
      skillUsage: {
        "review-loop": { usageCount: 50, lastUsedAt: Date.now() - day },
        pdf: { usageCount: 3, lastUsedAt: Date.now() },
        deploy: { usageCount: 40, lastUsedAt: Date.now() - 200 * day },
        shape: { usageCount: 1, lastUsedAt: Date.now() },
        gone: { usageCount: 100, lastUsedAt: Date.now() },
      },
    }));
    const ranked = relay.skillsFor(relay.claudeTabs().find((t) => t.id === window));
    assert.deepEqual(ranked.map((s) => [s.source, s.name]), [["frequent", "review-loop"], ["frequent", "pdf"], ["frequent", "deploy"], ["yours", "shape"], ["claude", "clear"], ["claude", "compact"], ["claude", "context"], ["claude", "cost"], ["claude", "usage"], ["session", "aikido:scan"]]);
    const menu = relay.skillScreen(relay.claudeTabs().find((t) => t.id === window), "screen").blocks.find((block) => block.block_id === "skill").element;
    assert.deepEqual(menu.option_groups.map((g) => g.label.text), ["Frequently used", "Yours", "Claude Code", "Plugins and built-in"]);
    assert.equal(skills.find((s) => s.name === "review-loop").description, "Review the branch with Codex");
    assert.equal(skills.find((s) => s.name === "aikido:scan").description, "Scan for issues");
    assert.equal(skills.find((s) => s.name === "deploy").description, "Deploy this project");
    assert.equal(skills.find((s) => s.name === "shape").description, "Shape a ticket", "only you can invoke it, so the session's list leaves it out");
    assert.equal(skills.find((s) => s.name === "shape").hint, "[idea or ticket ID]", "from its argument-hint");
    assert.equal(skills.find((s) => s.name === "deploy").hint, "", "a skill that doesn't say has none");

    // Picked, the form says what it takes, and keeps what was typed.
    const picked = relay.skillScreen(relay.claudeTabs().find((t) => t.id === window), "screen", "shape", "the export runs twice");
    const args = picked.blocks.find((block) => block.block_id === "args");
    assert.equal(args.label.text, "Arguments");
    assert.match(args.hint.text, /^\/shape takes \[idea or ticket ID\]\. /);
    assert.equal(args.element.placeholder.text, "[idea or ticket ID]");
    assert.equal(args.element.initial_value, "the export runs twice");
    assert.equal(picked.blocks.find((block) => block.block_id === "skill").element.initial_option.value, "shape");
  } finally {
    fs.rmSync(path.join(home, ".claude.json"), { force: true });
    fs.rmSync(path.join(home, ".claude/skills"), { recursive: true, force: true });
    fs.rmSync(path.join(home, ".claude/sessions", `${panePid}.json`), { force: true });
    fs.rmSync(transcript, { force: true });
    tmux("kill-window", "-t", window);
  }
});

test("Skill under a finished turn opens a menu of the tab's skills, and Run types the skill into the tab", async () => {
  // In the tests' own directory, so no real .claude folder above it is read.
  const [window] = tmux("new-window", "-d", "-c", tmp, "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const pid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  fs.writeFileSync(path.join(home, ".claude/sessions", `${pid}.json`), JSON.stringify({ pid: Number(pid), sessionId: "runner" }));
  const transcript = path.join(home, ".claude/projects/-work/runner.jsonl");
  fs.writeFileSync(transcript, JSON.stringify({ type: "attachment", attachment: { type: "skill_listing", isInitial: true, names: ["review-loop"], content: "- review-loop: Review the branch" } }));
  const thread = `${Math.floor(Date.now() / 1000)}.000500`;
  const state = { dm: { channel: "D1", user: "U1", team: "T1" }, threads: { [thread]: thread }, alerts: { [thread]: { window, index: 1, name: "x" } }, reported: {}, following: {}, homes: { [window]: { ts: thread, pid } } };
  answers["views.open"] = () => ({ ok: true, view: { id: "VSKILL" } });
  answers["views.update"] = () => ({ ok: true });
  try {
    const button = relay.turnButtons(relay.claudeTabs().find((t) => t.id === window), "Done.").find((b) => b.text === "Skill");
    const press = { type: "block_actions", user: { id: "U1", team_id: "T1" }, team: { id: "T1" }, trigger_id: "TR9", container: { channel_id: "D1", message_ts: "1.2" }, message: { ts: "1.2", thread_ts: thread, text: "🤖 done" }, actions: [{ value: JSON.stringify(button.value) }] };
    await relay.handlePress(state, relay.pressFrom(state, press));
    const opened = calls.find((c) => c.method === "views.open").params;
    assert.equal(opened.trigger_id, "TR9");
    const form = opened.view;
    assert.equal(form.callback_id, "skill");
    assert.deepEqual(form.blocks[0].element.option_groups.map((g) => [g.label.text, g.options.map((o) => o.value)]), [["Claude Code", ["clear", "compact", "context", "cost", "usage"]], ["Plugins and built-in", ["review-loop"]]]);
    assert.equal(form.blocks[0].element.option_groups[1].options[0].description.text, "Review the branch");
    assert.ok(!form.blocks.some((b) => b.block_id === "typed"), "everything fits in the menu");

    // Picking a skill draws the form again, saying what it takes, for you only
    // and only from the Skill form's own menu.
    const pick = (skill, { user = "U1", callback = "skill" } = {}) => ({ type: "block_actions", user: { id: user, team_id: "T1" }, team: { id: "T1" }, trigger_id: "TR10", container: { type: "view", view_id: "VSKILL" }, view: { ...form, callback_id: callback, type: "modal", id: "VSKILL", hash: "H1", state: { values: { args: { args: { value: "ABC-12" } } } } }, actions: [{ type: "static_select", action_id: "skill", block_id: "skill", selected_option: { value: skill } }] });
    assert.equal(relay.pressFrom(state, pick("review-loop", { user: "U2" })), undefined, "not yours");
    assert.equal(relay.pressFrom(state, pick("review-loop", { callback: "rename" })), undefined, "not the Skill form");
    assert.equal(form.blocks[0].dispatch_action, true, "a pick is sent at once");
    await relay.handlePress(state, relay.pressFrom(state, pick("review-loop")));
    const redrawn = calls.find((c) => c.method === "views.update").params;
    assert.equal(redrawn.view_id, "VSKILL");
    assert.equal(redrawn.hash, "H1", "refused by Slack if the form changed since");
    const argsField = redrawn.view.blocks.find((b) => b.block_id === "args");
    assert.match(argsField.hint.text, /^\/review-loop doesn't say what it takes\./, "the session's list carries no hint");
    assert.equal(argsField.element.initial_value, "ABC-12", "what was typed stays");
    calls.length = 0;

    const run = (skill, args, user = "U1") => ({ type: "view_submission", user: { id: user, team_id: "T1" }, team: { id: "T1" }, view: { ...form, id: "VSKILL", root_view_id: "VSKILL", state: { values: { skill: { skill: { type: "static_select", selected_option: { value: skill } } }, args: { args: { value: args } } } } } });

    // More skills than a menu holds: the rest are cut, and a field takes any name.
    const many = path.join(tmp, "many-skills");
    Array.from({ length: relay.MAX_SKILLS + 5 }, (_, i) => path.join(many, ".claude/skills", `s${String(i).padStart(3, "0")}`)).forEach((dir) => {
      fs.mkdirSync(dir, { recursive: true });
      fs.writeFileSync(path.join(dir, "SKILL.md"), "---\ndescription: x\n---\n");
    });
    const [crowded] = tmux("new-window", "-d", "-c", many, "-P", "-F", "#{window_id}", fakeClaude).split("\t");
    await new Promise((resolve) => setTimeout(resolve, 300));
    const big = relay.skillScreen(relay.claudeTabs().find((t) => t.id === crowded), "screen");
    tmux("kill-window", "-t", crowded);
    assert.equal(big.blocks[0].element.option_groups.reduce((n, g) => n + g.options.length, 0), relay.MAX_SKILLS);
    assert.match(big.blocks.find((b) => b.block_id === "typed").label.text, new RegExp(`^Or another skill \\(${5 + relay.BUILT_IN.length} more aren't in the menu\\)$`), "Claude Code's commands come before what is cut");
    assert.equal(big.blocks[0].optional, true, "either one will do");
    assert.equal(relay.submissionFrom(state, run("review-loop", "", "U2")), undefined, "only yours");

    // Not a skill name: nothing typed.
    await relay.handleSubmission(state, relay.submissionFrom(state, run("rm -rf /", "")));
    assert.ok(!posts.some((p) => /Skill:/.test(p.body)));

    await relay.handleSubmission(state, relay.submissionFrom(state, run("review-loop", "ABC-12")));
    await new Promise((resolve) => setTimeout(resolve, 600));
    assert.match(tmux("capture-pane", "-p", "-t", window), /\/review-loop ABC-12/, "typed as you would at the keyboard");
    assert.ok(posts.some((p) => p.thread === thread && p.body === "🤖 _Skill:_ `/review-loop ABC-12`"), "said in its thread");
    assert.ok(!calls.some((c) => c.method === "views.update"), "opened from the thread, so no screen to draw again");
  } finally {
    fs.rmSync(path.join(home, ".claude/sessions", `${pid}.json`), { force: true });
    fs.rmSync(transcript, { force: true });
    tmux("kill-window", "-t", window);
  }
});

test("a restart edits the welcome back rather than posting another, unless it is old or the relay crashed", async () => {
  const recent = `${Math.floor(Date.now() / 1000) - 3600}.000100`;
  const state = { dm: { channel: "D1", user: "U1", team: "T1" }, threads: {}, alerts: {}, welcome: recent };

  // Started again on purpose: the same message, now listening, read from now on.
  const from = await relay.sayListening(state, relay.welcomeText(""), "");
  assert.equal(posts.length, 0);
  assert.equal(updates.at(-1).ts, recent);
  assert.match(updates.at(-1).body, /^🤖 \*\*Relay listening\*\* since /);
  assert.equal(state.welcome, recent);
  assert.ok(Math.abs(Number(from) - Date.now() / 1000) < 5, "nothing before now is read again");

  // After a crash, a new message: it's news.
  await relay.sayListening(state, relay.welcomeText("exit code 1"), "exit code 1");
  assert.match(posts.at(-1).body, /^🤖 \*\*Relay restarted\*\* after a crash/);
  assert.equal(state.welcome, posts.at(-1).ts);

  // A day-old welcome is too far up the DM to edit.
  state.welcome = `${Math.floor(Date.now() / 1000) - 25 * 3600}.000100`;
  const kickoff = await relay.sayListening(state, relay.welcomeText(""), "");
  assert.equal(kickoff, posts.at(-1).ts);
  assert.equal(state.welcome, kickoff);

  // And one that can't be edited (deleted, say) is posted again.
  state.welcome = recent;
  answers["chat.update"] = () => {
    throw new Error("chat.update: message_not_found");
  };
  const again = await relay.sayListening(state, relay.welcomeText(""), "");
  assert.equal(again, posts.at(-1).ts);
  assert.match(relay.stoppedText(), /^🤖 \*\*Relay stopped\*\* at \d{1,2}:\d\d/);
});

test("the welcome message offers Home first, then buttons for the commands, answered at the top level", async () => {
  // Without the app's ID there is no Home link yet.
  assert.deepEqual(relay.welcomeButtons({ team: "T1" }).map((b) => b.text), ["Status", "Threads", "Progress", "New tab", "Help", "Stop relay"]);
  const buttons = relay.welcomeButtons({ team: "T1", appId: "A1" });
  assert.deepEqual(buttons.map((b) => b.text), ["Home", "Status", "Threads", "Progress", "New tab", "Help", "Stop relay"]);
  assert.equal(buttons[0].url, "slack://app?team=T1&id=A1&tab=home");
  assert.ok(buttons.at(-1).confirm, "Stop asks first");

  const state = { dm: { channel: "D1", user: "U1", team: "T1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  const welcome = `${Math.floor(Date.now() / 1000)}.000600`;
  const press = (value, user = "U1") => ({ type: "block_actions", user: { id: user, team_id: "T1" }, team: { id: "T1" }, container: { channel_id: "D1", message_ts: welcome }, message: { ts: welcome, text: "🤖 Relay listening" }, actions: [{ value: JSON.stringify(value) }] });
  assert.equal(relay.pressFrom(state, press(buttons[1].value, "U2")), undefined, "only yours");

  await relay.handlePress(state, relay.pressFrom(state, press(buttons[1].value)));
  assert.match(posts.at(-1).body, /^🤖 \*\*Status\*\*/);
  assert.equal(posts.at(-1).thread, undefined, "at the top level, as if typed there");

  // Only the welcome's own commands.
  const before = posts.length;
  await relay.handlePress(state, relay.pressFrom(state, press({ a: "do", c: "new: rm -rf ~" })));
  assert.equal(posts.length, before);
});

test("a waiting tab's notice carries its options; answering one clears the rest, and a prompt answered at the computer leaves no buttons", async () => {
  const [window] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const ask = (since) => {
    ["Allow rm -rf build?", "❯ 1. Yes", "  2. No"].forEach((line) => {
      tmux("send-keys", "-t", window, "-l", line);
      tmux("send-keys", "-t", window, "Enter");
    });
    tmux("set", "-p", "-t", window, "@agent_state", "waiting");
    tmux("set", "-p", "-t", window, "@agent_since", since);
  };
  ask("1791500200");
  await new Promise((resolve) => setTimeout(resolve, 200));
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  try {
    await relay.alertTabs(state);
    const withButtons = calls.filter((c) => c.method === "chat.postMessage" && c.params.blocks?.[1]?.type === "actions");
    const alert = withButtons.find((c) => /is \*\*waiting on you\*\*\. Reply here/.test(c.params.text));
    const notice = withButtons.find((c) => c.params.reply_broadcast);
    assert.ok(notice, "the notice in the main DM has the buttons too");
    assert.deepEqual(notice.params.blocks[1].elements.map((b) => b.text.text), ["1. Yes", "2. No"]);
    const noticeTs = posts.find((p) => p.broadcast).ts;
    const alertTs = posts.find((p) => p.body === alert.params.text).ts;

    // Tapped on the notice: typed, and the alert in the thread loses its buttons.
    const yes = JSON.parse(notice.params.blocks[1].elements[0].value);
    await relay.handlePress(state, { value: yes, message: { ts: noticeTs, thread_ts: posts.find((p) => p.broadcast).thread, text: notice.params.text } });
    await new Promise((resolve) => setTimeout(resolve, 200));
    assert.equal(tmux("capture-pane", "-p", "-t", window).split("\n").filter((line) => line.trim()).at(-1), "1");
    const cleared = calls.filter((c) => c.method === "chat.update" && c.params.ts === alertTs).at(-1).params;
    assert.match(cleared.text, /_Answered: 1\. Yes_$/);
    assert.equal(cleared.blocks.length, 1, "no buttons left on the alert");
    assert.match(updates.find((u) => u.ts === noticeTs).body, /_Chose \*\*1\. Yes\*\*\._$/);

    // The next prompt is answered at the computer: its buttons go.
    tmux("send-keys", "-t", window, "C-l");
    ask("1791500300");
    await new Promise((resolve) => setTimeout(resolve, 200));
    state.following = {};
    await relay.alertTabs(state);
    const second = posts.filter((p) => !p.broadcast).at(-1).ts;
    tmux("set", "-p", "-t", window, "@agent_state", "working");
    await relay.alertTabs(state);
    const gone = calls.filter((c) => c.method === "chat.update" && c.params.ts === second).at(-1).params;
    assert.match(gone.text, /_This prompt is no longer showing\._$/);
    assert.equal(gone.blocks.length, 1);
    assert.equal(state.choices[window], undefined);
  } finally {
    tmux("kill-window", "-t", window);
  }
});

test("text that isn't an option at a prompt is refused with the options as buttons", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  ["Allow rm -rf build?", "❯ 1. Yes", "  2. No"].forEach((line) => {
    tmux("send-keys", "-t", window, "-l", line);
    tmux("send-keys", "-t", window, "Enter");
  });
  tmux("set", "-p", "-t", window, "@agent_state", "waiting");
  tmux("set", "-p", "-t", window, "@agent_since", "1791500200");
  await new Promise((resolve) => setTimeout(resolve, 200));
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  const linked = `${Math.floor(Date.now() / 1000) - 30}.000500`;
  state.alerts[linked] = { window, index: Number(index), name: "work" };
  try {
    await relay.handle(state, { ts: `${Math.floor(Date.now() / 1000)}.000700`, text: "cancel that", thread_ts: linked });
    const refusal = calls.filter((c) => c.method === "chat.postMessage").at(-1).params;
    assert.match(refusal.text, /isn't one of its options, so I didn't type anything\.[\s\S]*Tap an option/);
    assert.deepEqual(refusal.blocks[1].elements.map((b) => b.text.text), ["1. Yes", "2. No"]);
    assert.notEqual(tmux("capture-pane", "-p", "-t", window).split("\n").filter((line) => line.trim()).at(-1), "cancel that");
  } finally {
    tmux("kill-window", "-t", window);
  }
});

test("optionsIn reads Claude's own questions: one at a time under their names, with several answers ticked, and a free answer", () => {
  // As Claude Code draws AskUserQuestion with two questions, the first one up.
  const first = [
    "←  ☐ Color  ☐ Toppings  ✔ Submit  →",
    "Color?",
    "❯ 1. Red",
    "     Choose red",
    "  2. Blue",
    "  3. Type something.",
    "────────────────────",
    "  4. Chat about this",
    "Enter to select · Tab/Arrow keys to navigate · Esc to cancel",
  ].join("\n");
  assert.deepEqual(relay.optionsIn(first), {
    question: "Color?",
    options: [{ n: 1, label: "Red" }, { n: 2, label: "Blue" }, { n: 3, label: "Type something." }, { n: 4, label: "Chat about this" }],
    free: 3,
    cursor: 1,
  });
  // The second takes several answers; one typed in the free option replaces its label.
  const second = [
    "←  ☒ Color  ☒ Toppings  ✔ Submit  →",
    "Toppings?",
    "  1. [✔] Cheese",
    "         Add cheese",
    "  2. [ ] Olives",
    "  3. [✔] Ketchup",
    "❯    Submit",
    "────────────────────",
    "  4. Chat about this",
  ].join("\n");
  const found = relay.optionsIn(second);
  assert.deepEqual(found, {
    question: "Toppings?",
    options: [{ n: 1, label: "Cheese", checked: true }, { n: 2, label: "Olives", checked: false }, { n: 3, label: "Ketchup", checked: true }, { n: 4, label: "Chat about this" }],
    multi: true,
    free: 3,
    cursor: "submit",
  });
  assert.match(relay.promptText(found), /^\*\*Toppings\?\*\*\n1\. ☑ Cheese\n2\. ☐ Olives\n3\. ☑ Ketchup\n4\. Chat about this\n_Tap options to tick or untick them, then Submit choices/);

  assert.deepEqual(relay.answerFor(found, "done"), { kind: "submit" });
  assert.deepEqual(relay.answerFor(found, "1, 2").options.map((o) => o.n), [1, 2]);
  assert.deepEqual(relay.answerFor(found, "oli").options.map((o) => o.n), [2]);
  assert.equal(relay.answerSaid(relay.answerFor(found, "1 2")), "Unticked 1. Cheese, ticked 2. Olives");
  assert.deepEqual(relay.answerFor(found, "anchovies"), { kind: "free", text: "anchovies" });
  assert.deepEqual(relay.answerFor(relay.optionsIn(first), "3"), { kind: "free", text: "3" }, "the free option takes text, not its number");
  // A permission prompt has no free option, so other text is no answer.
  assert.equal(relay.answerFor(relay.optionsIn("Allow it?\n❯ 1. Yes\n  2. No"), "maybe"), undefined);
});

test("answering one of several questions posts the next with its buttons; ticks, Submit and a free answer press the right keys", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const show = (lines) => lines.forEach((line) => {
    tmux("send-keys", "-t", window, "-l", line);
    tmux("send-keys", "-t", window, "Enter");
  });
  show(["←  ☐ Color  ☐ Toppings  ✔ Submit  →", "Color?", "❯ 1. Red", "  2. Blue", "  3. Type something.", "────────────", "  4. Chat about this"]);
  tmux("set", "-p", "-t", window, "@agent_state", "waiting");
  tmux("set", "-p", "-t", window, "@agent_since", "1791500300");
  await new Promise((resolve) => setTimeout(resolve, 200));
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  const posted = () => calls.filter((call) => call.method === "chat.postMessage").at(-1).params;
  const linked = `${Math.floor(Date.now() / 1000) - 30}.000500`;
  state.alerts[linked] = { window, index: Number(index), name: "work" };
  try {
    // Red is picked, and Claude Code shows the second question in the same
    // wait. Drawn in one go, before the relay's first look after its settle.
    setTimeout(() => show(["←  ☒ Color  ☐ Toppings  ✔ Submit  →", "Toppings?", "❯ 1. [ ] Cheese", "  2. [ ] Olives", "  3. [ ] Type something", "     Submit", "────────────", "  4. Chat about this"]), 50);
    await relay.handle(state, { ts: `${Math.floor(Date.now() / 1000)}.000710`, text: "1", thread_ts: linked });
    assert.match(posted().text, /^🤖 Chose 1\. Red in tab \d+, .*\. It's asking next:\n\n\*\*Toppings\?\*\*\n1\. ☐ Cheese/);
    assert.deepEqual(posted().blocks[1].elements.map((b) => b.text.text), ["1. ☐ Cheese", "2. ☐ Olives", "4. Chat about this", "Submit choices"]);
    assert.equal(state.following[window], undefined, "still waiting on you, so not followed yet");

    // Two ticks at once, then your own answer typed into the free row, reached
    // with the arrow keys from the first row, then Submit, three rows down.
    await relay.handle(state, { ts: `${Math.floor(Date.now() / 1000)}.000711`, text: "1 2", thread_ts: linked });
    assert.match(posted().text, /^🤖 Ticked 1\. Cheese, ticked 2\. Olives in tab/);
    tmux("send-keys", "-t", window, "Enter");
    await relay.handle(state, { ts: `${Math.floor(Date.now() / 1000)}.000712`, text: "anchovies", thread_ts: linked });
    tmux("send-keys", "-t", window, "Enter");
    await relay.handle(state, { ts: `${Math.floor(Date.now() / 1000)}.000713`, text: "submit", thread_ts: linked });
    const typed = tmux("capture-pane", "-p", "-t", window).split("\n").filter((line) => line.trim());
    assert.ok(typed.includes("12"), "both numbers pressed");
    assert.ok(typed.includes("^[[B^[[Banchovies"), typed.slice(-6).join("\n"));
    assert.ok(typed.some((line) => line.startsWith("^[[B^[[B^[[B")), "down to Submit, then Enter");
    assert.match(posted().text, /^🤖 Submitted your choices in tab/);
  } finally {
    tmux("kill-window", "-t", window);
  }
});

test("a waiting tab's options are on its Home tab row, and a tap there answers it", async () => {
  const [window] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const pid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  ["Allow rm -rf build?", "❯ 1. Yes", "  2. No"].forEach((line) => {
    tmux("send-keys", "-t", window, "-l", line);
    tmux("send-keys", "-t", window, "Enter");
  });
  tmux("set", "-p", "-t", window, "@agent_state", "waiting");
  tmux("set", "-p", "-t", window, "@agent_since", "1791500200");
  await new Promise((resolve) => setTimeout(resolve, 200));
  const state = { dm: { channel: "D1", user: "U1", team: "T1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  try {
    await relay.refreshDashboard(state, { quick: true, opened: true });
    const row = views.at(-1).blocks.find((b) => b.type === "actions" && JSON.parse(b.elements[0].value).w === window);
    assert.deepEqual(row.elements.map((b) => b.text.text), ["Open", "1. Yes", "2. No"]);
    const yes = JSON.parse(row.elements[1].value);
    assert.equal(yes.a, "home-opt");
    const press = (value) => ({ type: "block_actions", user: { id: "U1", team_id: "T1" }, team: { id: "T1" }, trigger_id: "TR1", container: { type: "view", view_id: "VHOME" }, view: { id: "VHOME", type: "home" }, actions: [{ value: JSON.stringify(value) }] });

    // For an earlier wait: nothing pressed, and the Home tab drawn again.
    const drawn = views.length;
    await relay.handlePress(state, relay.pressFrom(state, press({ ...yes, s: "1791500100" })));
    assert.notEqual(tmux("capture-pane", "-p", "-t", window).split("\n").filter((line) => line.trim()).at(-1), "1");
    assert.equal(views.length, drawn + 1);

    await relay.handlePress(state, relay.pressFrom(state, press(yes)));
    await new Promise((resolve) => setTimeout(resolve, 200));
    assert.equal(tmux("capture-pane", "-p", "-t", window).split("\n").filter((line) => line.trim()).at(-1), "1");
    assert.ok(posts.some((p) => p.thread === state.homes[window].ts && p.body === "🤖 _From the Home tab:_ chose **1. Yes**"));
  } finally {
    tmux("kill-window", "-t", window);
  }
});

test("Rename on a thread header opens the form there, and the result is said in the thread", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const pid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  const home = `${Math.floor(Date.now() / 1000)}.000800`;
  const state = { dm: { channel: "D1", user: "U1", team: "T1" }, threads: { [home]: home }, alerts: { [home]: { window, index: Number(index), name: "x" } }, reported: {}, following: {}, homes: { [window]: { ts: home, pid } } };
  answers["views.open"] = () => ({ ok: true, view: { id: "VREN" } });
  try {
    const rename = relay.headerButtons(relay.claudeTabs().find((t) => t.id === window)).find((b) => b.text === "Rename");
    const press = { type: "block_actions", user: { id: "U1", team_id: "T1" }, team: { id: "T1" }, trigger_id: "TR7", container: { channel_id: "D1", message_ts: home }, message: { ts: home, text: "🤖 Tab" }, actions: [{ value: JSON.stringify(rename.value) }] };
    await relay.handlePress(state, relay.pressFrom(state, press));
    const form = calls.find((c) => c.method === "views.open").params.view;
    assert.equal(form.callback_id, "rename");
    assert.equal(JSON.parse(form.private_metadata).from, "thread");
    const save = { type: "view_submission", user: { id: "U1", team_id: "T1" }, team: { id: "T1" }, view: { ...form, id: "VREN", root_view_id: "VREN", state: { values: { name: { name: { value: "deploy check" } } } } } };
    await relay.handleSubmission(state, relay.submissionFrom(state, save));
    assert.equal(tmux("show-options", "-wqv", "-t", window, "@tab_name"), "deploy check");
    assert.ok(posts.some((p) => p.thread === home && /Renamed \*\*tab \d+\*\* to \*\*deploy check\*\*/.test(p.body)));
    assert.ok(!calls.some((c) => c.method === "views.update"), "no screen under it to draw again");
  } finally {
    tmux("kill-window", "-t", window);
  }
});

test("a tab keeps the Claude pane it was first seen with, whatever is opened or used beside it", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  const claude = tmux("display", "-p", "-t", window, "#{pane_id}");
  const claudePid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  // A shell-like pane beside it, and the one you used last.
  const other = tmux("split-window", "-d", "-t", window, "-P", "-F", "#{pane_id}", "sleep 600");
  tmux("select-pane", "-t", other);
  await new Promise((resolve) => setTimeout(resolve, 300));
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  try {
    assert.equal(tmux("display", "-p", "-t", window, "#{pane_id}"), other, "the active pane isn't Claude");
    const tab = relay.claudeTabs().find((t) => t.id === window);
    assert.equal(tab.pane, claude);
    assert.equal(tab.pid, claudePid, "its process is Claude's, not the active pane's");
    assert.equal(relay.claudePane(window).pane, claude);

    const linked = `${Math.floor(Date.now() / 1000) - 30}.000901`;
    state.alerts[linked] = { window, index: Number(index), name: "work" };
    await relay.handle(state, { ts: `${Math.floor(Date.now() / 1000)}.000900`, text: "run the tests", thread_ts: linked });
    await new Promise((resolve) => setTimeout(resolve, 600));
    assert.match(tmux("capture-pane", "-p", "-t", claude), /run the tests/, "typed into Claude");
    assert.match(relay.screen(window), /run the tests/, "and its screen is Claude's");

    assert.equal(tmux("show-options", "-wqv", "-t", window, "@claude_pane"), claude, "recorded on the window");

    // Another Claude opened beside it, even one waiting on you, doesn't take the tab.
    const second = tmux("split-window", "-d", "-t", window, "-P", "-F", "#{pane_id}", fakeClaude);
    await new Promise((resolve) => setTimeout(resolve, 300));
    tmux("set", "-p", "-t", second, "@agent_state", "waiting");
    const still = relay.claudeTabs().find((t) => t.id === window);
    assert.equal(still.pane, claude);
    assert.equal(still.state, "idle", "the tab's own pane's state");
    assert.equal(still.pids.length, 2);

    // Its own pane closed: the other Claude pane takes the tab, and is recorded.
    tmux("kill-pane", "-t", claude);
    const after = relay.claudeTabs().find((t) => t.id === window);
    assert.equal(after.pane, second);
    assert.equal(tmux("show-options", "-wqv", "-t", window, "@claude_pane"), second);
  } finally {
    tmux("kill-window", "-t", window);
  }
});

test("a closed tab's thread is deleted on request: the app's messages and files go, your replies stay, and a live tab's thread can't be", async () => {
  // Recent, since threads older than a day stop being watched.
  const now = Math.floor(Date.now() / 1000);
  const closed = `${now - 60}.001000`;
  const live = `${now - 30}.002000`;
  const reply = `${now - 50}.001100`;
  const state = {
    dm: { channel: "D1", user: "U1", team: "T1", bot: "UBOT" }, threads: { [closed]: closed, [live]: live }, alerts: { [closed]: { window: "@9" } },
    reported: {}, following: {}, closedHomes: { [closed]: { label: "3, done work", at: Date.now() } },
  };
  answers["conversations.replies"] = (params) => ({
    ok: true,
    messages: params.ts === closed
      ? [
          { ts: closed, bot_id: "B1", text: "🤖 ~~Tab 3~~" },
          { ts: reply, bot_id: "B1", text: "🤖 diff", files: [{ id: "F9" }] },
          { ts: `${now - 40}.001200`, user: "U1", text: "thanks" },
        ]
      : [{ ts: live, bot_id: "B1", text: "🤖 Tab 4" }],
  });
  answers["files.delete"] = () => ({ ok: true });

  // Not a closed tab's thread: nothing happens.
  await relay.handlePress(state, { value: { a: "delete-thread", t: live }, message: { ts: live, text: "" } });
  assert.ok(!calls.some((c) => c.method === "chat.delete" || c.method === "conversations.replies"));

  // `cleanup` only offers a button, which Slack confirms.
  await relay.handle(state, { ts: `${Math.floor(Date.now() / 1000)}.001300`, text: "cleanup", thread_ts: null });
  const offer = calls.filter((c) => c.method === "chat.postMessage").at(-1).params;
  assert.match(offer.text, /\*\*1 closed tab has a thread\.\*\* Delete them\?/);
  const button = offer.blocks[1].elements[0];
  assert.equal(button.text.text, "Delete 1 closed thread");
  assert.ok(button.confirm);
  assert.ok(!calls.some((c) => c.method === "chat.delete"), "nothing deleted yet");

  await relay.handlePress(state, { value: JSON.parse(button.value), message: { ts: offer.ts, text: offer.text } });
  const deleted = calls.filter((c) => c.method === "chat.delete").map((c) => c.params.ts);
  assert.deepEqual(deleted, [reply, closed], "the app's reply, then the header; not your reply");
  assert.deepEqual(calls.filter((c) => c.method === "files.delete").map((c) => c.params.file), ["F9"]);
  assert.deepEqual(state.closedHomes, {});
  assert.equal(state.threads[closed], undefined);
  assert.equal(state.threads[live], live);
  assert.equal(state.alerts[closed], undefined);
  assert.match(posts.at(-1).body, /^🤖 Deleted 1 closed thread\. 1 of your own replies stayed/);
  assert.ok(views.length >= 1, "the Home tab drawn again");
});

test("last activity is the last conversation entry, not Claude Code's bookkeeping", () => {
  const transcript = path.join(home, ".claude/projects/-work/bookkeeping.jsonl");
  const said = Date.now() - 11 * 60 * 60_000;
  fs.writeFileSync(transcript, [
    { type: "user", timestamp: new Date(said - 60_000).toISOString(), message: { content: "ship it" } },
    { type: "assistant", timestamp: new Date(said).toISOString(), message: { content: [{ type: "text", text: "Shipped." }] } },
    { type: "permission-mode", timestamp: new Date().toISOString() },
    { type: "pr-link", timestamp: new Date().toISOString() },
  ].map((entry) => JSON.stringify(entry)).join("\n"));

  assert.equal(relay.lastActivityAt(transcript), said);
  assert.equal(relay.lastActivityAt(path.join(home, "missing.jsonl")), undefined);
});

test("a long message, kept by Claude Code as pasted text, is still matched to its answer", () => {
  const transcript = path.join(home, ".claude/projects/-work/pasted.jsonl");
  const typed = "Here is what tab 2 said: it published the page, then asked whether to merge. What do you think we should do next?";
  // As Claude Code stores a long paste: wrapped, with the ID on both tags.
  fs.writeFileSync(transcript, [
    { type: "user", message: { content: `\n\n<pasted_content id="5a03">\n${typed}\n</pasted_content id="5a03">\n` } },
    { type: "assistant", message: { content: [{ type: "text", text: "Rebase first, then merge." }] } },
  ].map((entry) => JSON.stringify(entry)).join("\n"));

  const turn = relay.turnFrom(transcript, typed);
  assert.ok(turn, "matched despite the wrapper");
  assert.match(turn, /^> Here is what tab 2 said:/m, "quoted without the tags");
  assert.doesNotMatch(turn, /pasted_content/);
  assert.match(turn, /Rebase first, then merge\./);
});

test("a multi-line message reaches the tab with its line breaks, as one paste", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };

  const linked = `${Math.floor(Date.now() / 1000) - 30}.000500`;
  state.alerts[linked] = { window, index: Number(index), name: "work" };
  await relay.handle(state, { ts: `${Math.floor(Date.now() / 1000)}.000090`, text: "first line\nsecond line\n\n- a list item", thread_ts: linked });
  await new Promise((resolve) => setTimeout(resolve, 300));

  const lines = tmux("capture-pane", "-p", "-t", window).split("\n");
  ["first line", "second line", "- a list item"].forEach((line) => assert.ok(lines.includes(line), `"${line}" on a line of its own`));
  assert.ok(!tmux("list-buffers", "-F", "#{buffer_name}").split("\n").includes("slack-relay"), "the relay's buffer is gone");
  tmux("kill-window", "-t", window);
});

test("a turn in a tab you aren't looking at is posted once, not once from its event and again when marked done", async () => {
  const [window] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const panePid = tmux("display", "-p", "-t", window, "#{pane_pid}");
  fs.writeFileSync(path.join(home, ".claude/sessions", `${panePid}.json`), JSON.stringify({ pid: Number(panePid), sessionId: "elsewhere" }));
  const transcript = path.join(home, ".claude/projects/-work/elsewhere.jsonl");
  fs.writeFileSync(transcript, `${JSON.stringify({ type: "user", message: { content: "run it" }, timestamp: new Date().toISOString() })}\n`);
  const time = Math.floor(Date.now() / 1000);
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };
  try {
    // The hook's event lands while tmux still says working...
    tmux("set", "-p", "-t", window, "@agent_state", "working");
    await relay.alertTabs(state, relay.claudeTabs(), [{ event: "done", time, transcript_path: transcript, last: "Ran it." }]);
    // ...then the tab is marked done.
    tmux("set", "-p", "-t", window, "@agent_state", "done");
    tmux("set", "-p", "-t", window, "@agent_since", String(time));
    await relay.alertTabs(state, relay.claudeTabs(), []);

    assert.equal(posts.filter((p) => /has finished/.test(p.body)).length, 1);
  } finally {
    fs.rmSync(path.join(home, ".claude/sessions", `${panePid}.json`));
    tmux("kill-window", "-t", window);
  }
});

test("an offline note stays scheduled ten minutes ahead, and is renewed, said back, or cancelled", async () => {
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {} };

  await relay.renewOfflineNote(state);
  const first = calls.find((c) => c.method === "chat.scheduleMessage").params;
  assert.equal(first.channel, "D1");
  assert.ok(Math.abs(first.post_at - (Date.now() + relay.OFFLINE_AFTER_MS) / 1000) < 5, "ten minutes out");
  assert.match(first.text, /\*\*The relay is offline\.\*\* Your Mac last checked in at /);
  assert.equal(state.offlineNote.id, "Q1");

  // Renewed: the next one first, then the old one goes, so one is always there.
  await relay.renewOfflineNote(state);
  assert.equal(state.offlineNote.id, "Q2");
  assert.deepEqual(calls.filter((c) => c.method === "chat.deleteScheduledMessage").map((c) => c.params.scheduled_message_id), ["Q1"]);

  // Notes left behind by a delete that failed go too, but not the new one.
  answers["chat.scheduledMessages.list"] = () => ({ ok: true, scheduled_messages: [{ id: "QSTRAY" }, { id: "Q3" }] });
  await relay.renewOfflineNote(state);
  assert.equal(state.offlineNote.id, "Q3");
  assert.deepEqual(calls.filter((c) => c.method === "chat.deleteScheduledMessage").map((c) => c.params.scheduled_message_id), ["Q1", "Q2", "QSTRAY"]);

  // Refused for too many scheduled: they are cleared, and it is scheduled again.
  let refused = false;
  answers["chat.scheduleMessage"] = () => {
    if (!refused) {
      refused = true;
      throw new Error("chat.scheduleMessage: restricted_too_many");
    }
    return { ok: true, scheduled_message_id: "Q9" };
  };
  answers["chat.scheduledMessages.list"] = () => ({ ok: true, scheduled_messages: [{ id: "QSTRAY2" }] });
  await relay.renewOfflineNote(state);
  assert.equal(state.offlineNote.id, "Q9");
  assert.ok(calls.some((c) => c.method === "chat.deleteScheduledMessage" && c.params.scheduled_message_id === "QSTRAY2"));
  delete answers["chat.scheduleMessage"];
  delete answers["chat.scheduledMessages.list"];

  // Back after a short gap: Slack never sent it, so nothing to say.
  await relay.sayBackOnline(state);
  assert.equal(posts.length, 0);
  // Back after it was sent: say so, and for how long.
  state.offlineNote.at = Date.now() - 2 * 60 * 60_000 - 13 * 60_000;
  await relay.sayBackOnline(state);
  assert.match(posts[0].body, /^🤖 ✅ \*\*Back online\*\*, after 2h 13m offline/);

  // A clean stop: cancelled.
  await relay.cancelOfflineNote(state);
  assert.equal(calls.filter((c) => c.method === "chat.deleteScheduledMessage").at(-1).params.scheduled_message_id, "Q9");
  assert.equal(state.offlineNote, undefined);
});

test("the Home tab says when the Mac last checked in, in a time Slack keeps counting", async () => {
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {}, reported: {}, following: {} };

  await relay.refreshDashboard(state, { quick: true });
  await relay.refreshDashboard(state, { quick: true, heartbeat: true });

  assert.equal(views.length, 2, "published every minute, changed or not");
  const checkin = views[0].blocks.find((b) => b.block_id === "checkin");
  const line = checkin.text.text;
  assert.match(line, /^🟢 Your Mac checked in <!date\^\d+\^\{ago\}\|at .+>\.$/);
  const at = Number(line.match(/<!date\^(\d+)\^/)[1]);
  assert.ok(Math.abs(at - Date.now() / 1000) < 5);

  // Refresh beside it publishes it again at once, and only you can press it.
  assert.equal(checkin.accessory.text.text, "Refresh");
  const press = (user) => ({ type: "block_actions", user: { id: user }, container: { type: "view", view_id: "VHOME" }, view: { id: "VHOME", type: "home" }, actions: [{ value: checkin.accessory.value }] });
  assert.equal(relay.pressFrom(state, press("U2")), undefined);
  await relay.handlePress(state, relay.pressFrom(state, press("U1")));
  assert.equal(views.length, 3);
});

test("a background task's notification shows as its summary, not as a prompt you sent", () => {
  const transcript = path.join(home, ".claude/projects/-work/notified.jsonl");
  const notice = "<task-notification>\n<task-id>b3f4mns8a</task-id>\n<tool-use-id>toolu_01</tool-use-id>\n<output-file>/private/tmp/claude-501/tasks/b3f4mns8a.output</output-file>\n<status>completed</status>\n<summary>Background command \"Run the review, round 1\" completed (exit code 0)</summary>\n</task-notification>";
  fs.writeFileSync(transcript, [
    { type: "user", timestamp: new Date().toISOString(), message: { content: "start the review" } },
    { type: "assistant", message: { content: [{ type: "text", text: "Started it in the background." }] } },
    { type: "user", timestamp: new Date().toISOString(), origin: { kind: "task-notification" }, promptSource: "system", message: { content: notice } },
    { type: "assistant", message: { content: [{ type: "text", text: "Round 1 passed; starting round 2." }] } },
  ].map((entry) => JSON.stringify(entry)).join("\n"));
  try {
    const last = relay.lastPrompt(transcript);
    assert.equal(last.background, true);
    assert.equal(last.prompt, 'Background command "Run the review, round 1" completed (exit code 0)');
    const turn = relay.turnFrom(transcript);
    assert.match(turn, /^_\(a background task finished just now\)_\n> Background command "Run the review, round 1" completed/);
    assert.match(turn, /Round 1 passed; starting round 2\./);
    assert.doesNotMatch(turn, /task-notification|output-file|\/private\/tmp/, "none of the raw block");
  } finally {
    fs.rmSync(transcript, { force: true });
  }
});

test("a request is matched when it was sent along with text left in the input box", () => {
  // An earlier message whose Enter was lost, sent in front of this one.
  const prompt = "Actually make that I’m going to your ticketActually, make that a linear ticket";
  assert.equal(relay.promptMatches(prompt, "Actually, make that a linear ticket"), true);
  assert.equal(relay.promptMatches(prompt, "Actually make that I’m going to your ticket"), true);
  assert.equal(relay.promptMatches(prompt, "run the tests"), false);
});

test("rename in a tab's thread names it as M-n does, and clears it", async () => {
  const [window, index] = tmux("new-window", "-d", "-P", "-F", "#{window_id}\t#{window_index}", fakeClaude).split("\t");
  await new Promise((resolve) => setTimeout(resolve, 300));
  const alert = `${Math.floor(Date.now() / 1000) - 60}.000300`;
  const state = { dm: { channel: "D1", user: "U1" }, threads: { [alert]: alert }, alerts: { [alert]: { window, index: Number(index), name: "work" } }, reported: {}, following: {} };
  const now = () => `${Math.floor(Date.now() / 1000)}.${String(Math.random()).slice(2, 8)}`;
  const tabName = () => tmux("show", "-wqv", "-t", window, "@tab_name");

  await relay.handle(state, { ts: now(), text: "rename Billing   fix", thread_ts: alert });
  assert.equal(tabName(), "Billing fix");
  assert.match(posts.at(-1).body, new RegExp(`^🤖 Renamed \\*\\*tab ${index}\\*\\* to \\*\\*Billing fix\\*\\*\\.`));
  assert.equal(posts.at(-1).thread, alert);

  // Naming a tab by number does nothing to it.
  await relay.handle(state, { ts: now(), text: `rename ${index} Elsewhere`, thread_ts: null });
  assert.equal(tabName(), "Billing fix");

  // No name: back to its own.
  await relay.handle(state, { ts: now(), text: "rename", thread_ts: alert });
  assert.equal(tabName(), "");
  assert.match(posts.at(-1).body, /has its own name again/);
  tmux("kill-window", "-t", window);
});

test("notify waits for the relay to post a notice in the DM as the app, at the top level, and says when it failed", async () => {
  const state = { dm: { channel: "D1", user: "U1" }, threads: {}, alerts: {} };
  fs.writeFileSync(relay.LOCK_FILE, `${process.pid} slack-relay\n`);
  const relayLoop = setInterval(() => relay.deliverOutbox(state), 50);
  try {
    const ts = await relay.notify("Triggered: disk usage on db-1", { from: "oncall" });
    assert.equal(posts.length, 1);
    assert.equal(posts[0].ts, ts);
    assert.equal(posts[0].thread, undefined);
    assert.equal(posts[0].body, "🔔 **oncall**\nTriggered: disk usage on db-1");
    assert.deepEqual(fs.readdirSync(relay.OUTBOX_DIR), []);

    answers["chat.postMessage"] = () => {
      throw new Error("chat.postMessage: ratelimited");
    };
    await assert.rejects(relay.notify("again", { from: "oncall" }), /ratelimited/);
    await assert.rejects(relay.notify("x", { from: "oncall; rm" }), /--from/);
  } finally {
    clearInterval(relayLoop);
  }

  // No relay running: refused at once, with nothing left queued.
  fs.rmSync(relay.LOCK_FILE);
  await assert.rejects(relay.notify("hello"), /isn't running/);
  assert.deepEqual(fs.readdirSync(relay.OUTBOX_DIR), []);
});
