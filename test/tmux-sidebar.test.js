// Tests for bin/tmux-sidebar. Run with: node --test test/*.test.js
//
// Nothing here touches your real tabs or ~/.claude. Before the sidebar is
// loaded, HOME points at a temporary directory and TMUX at a tmux server of
// the tests' own, which every plain `tmux` command then talks to; the tests
// refuse to run unless that is so. The temporary directory is under /tmp
// rather than the system's temporary directory because a Unix socket's path
// can't be over 104 bytes on macOS. Its sidebar panes run the real zsh client
// against the tests' daemon.

const { test, after, beforeEach } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync, spawn } = require("node:child_process");
const fs = require("node:fs");
const net = require("node:net");
const os = require("node:os");
const path = require("node:path");

const tmp = fs.realpathSync(fs.mkdtempSync("/tmp/sbt-"));
const home = path.join(tmp, "home");
const socket = path.join(tmp, "tmux.sock");
fs.mkdirSync(path.join(home, ".claude/sessions"), { recursive: true });
fs.mkdirSync(path.join(home, ".claude/projects/-work"), { recursive: true });
fs.mkdirSync(path.join(home, ".config/switchboard/sidebar-plugins"), { recursive: true });

// HOME is changed before the server starts, so nothing run in its panes (the
// sidebar client starts tmux/sidebar.sh) ever sees your real one.
const realTmux = process.env.TMUX;
delete process.env.TMUX;
delete process.env.TMUX_PANE;
Object.assign(process.env, { HOME: home, TMUX_TMPDIR: tmp });
execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "-s", "test", "-x", "200", "-y", "50", "-c", tmp, "cat"]);
execFileSync("tmux", ["-S", socket, "set", "-g", "default-command", "cat"]);
execFileSync("tmux", ["-S", socket, "set", "-g", "base-index", "1"]);
const serverPid = execFileSync("tmux", ["-S", socket, "display", "-p", "#{pid}"], { encoding: "utf8" }).trim();
Object.assign(process.env, {
  HOME: home,
  TMUX: `${socket},${serverPid},0`,
  TMUX_TMPDIR: tmp,
  TMUX_SIDEBAR_STATE_DIR: path.join(tmp, "state"),
  TMUX_SIDEBAR_CLIENT: path.join(__dirname, "../tmux/sidebar-pane.zsh"),
  LC_ALL: "en_US.UTF-8",
});
delete process.env.TMUX_PANE;

const tmux = (...args) => execFileSync("tmux", args, { encoding: "utf8" }).trimEnd();
assert.equal(tmux("display", "-p", "#{socket_path}"), socket, "tmux is not the tests' own server");
assert.notEqual(tmux("display", "-p", "#{socket_path}"), realTmux?.split(",")[0]);
assert.equal(os.homedir(), home);

const sidebar = require("../bin/tmux-sidebar");
assert.ok(sidebar.SOCKET.startsWith(tmp));

const config = { ...sidebar.DEFAULTS };
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const until = async (check, ms = 3_000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const value = await check();
    if (value) return value;
    await wait(50);
  }
  return check();
};
const panes = (target) =>
  tmux("list-panes", "-t", target, "-F", "#{pane_id}\t#{@sidebar}\t#{pane_width}\t#{pane_left}\t#{pane_title}")
    .split("\n")
    .map((line) => {
      const [id, mark, width, left, title] = line.split("\t");
      return { id, sidebar: mark === "1", width: Number(width), left: Number(left), title };
    });

// Each test starts from one window holding one pane, the sidebar on.
beforeEach(() => {
  tmux("set", "-gu", "@sidebar_on");
  tmux("set", "-gu", "@sidebar_hold");
  const windows = tmux("list-windows", "-t", "test", "-F", "#{window_id}").split("\n");
  tmux("new-window", "-t", "test", "-c", tmp);
  windows.forEach((w) => tmux("kill-window", "-t", w));
  fs.rmSync(sidebar.CONFIG_FILE, { force: true });
});

after(() => {
  sidebar.stop();
  try {
    execFileSync("tmux", ["-S", socket, "kill-server"], { stdio: "ignore" });
  } catch {}
  fs.rmSync(tmp, { recursive: true, force: true });
});

test("keys are named the way tmux names them", () => {
  assert.deepEqual(sidebar.parseKeys("j\x1b[A\x1bp\r\t\x1b[Z\x0c\x1b ").map((k) => k.key), ["j", "Up", "M-p", "Enter", "Tab", "BTab", "C-l", "M-Space"]);
  // A lone Escape arrives on its own; with a key after it, it is Alt.
  assert.deepEqual(sidebar.parseKeys("\x1b").map((k) => k.key), ["Escape"]);
  assert.deepEqual(sidebar.parseKeys("\x1b\t").map((k) => k.key), ["M-Tab"]);
  assert.deepEqual(sidebar.parseKeys("\x1b[<0;5;7M")[0].mouse, { button: 0, x: 5, y: 7, press: true });
});

test("the last turn is the last prompt, what Claude said after it, and the tool call it waits on", () => {
  const lines = [
    { type: "user", message: { content: "first prompt" }, timestamp: "2026-01-01T00:00:00Z" },
    { type: "assistant", message: { content: [{ type: "text", text: "old reply" }] } },
    { type: "user", message: { content: "<command-message>shape</command-message><command-name>/shape</command-name><command-args>CHA-1</command-args>" }, timestamp: "2026-01-01T00:01:00Z" },
    { type: "assistant", message: { content: [{ type: "text", text: "Reading it." }, { type: "tool_use", id: "a", name: "Read", input: { file_path: "/x" } }] } },
    { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "a", content: "..." }] } },
    { type: "assistant", isSidechain: true, message: { content: [{ type: "text", text: "a subagent" }] } },
    { type: "assistant", message: { content: [{ type: "text", text: "Ready to run the tests." }, { type: "tool_use", id: "b", name: "Bash", input: { command: "npm test\nmore" } }] } },
  ];
  const turn = sidebar.parseTurn(lines.map((l) => JSON.stringify(l)).join("\n"));
  assert.equal(turn.prompt, "/shape CHA-1");
  assert.equal(turn.reply, "Ready to run the tests.");
  assert.equal(turn.tools, 2);
  assert.equal(turn.pending, "Bash: npm test");
  assert.equal(turn.at, Date.parse("2026-01-01T00:01:00Z"));
});

test("a tab's own Claude wins its pane over a headless one that names it too", () => {
  const pane = tmux("display", "-p", "-t", "test:", "#{pane_id}");
  // A review loop's script starts a headless Claude, which inherits the pane.
  fs.writeFileSync(path.join(home, ".claude/sessions/121.json"), JSON.stringify({ pid: 121, sessionId: "own", cwd: "/work", tmux: `test:@9.${pane}`, entrypoint: "cli" }));
  fs.writeFileSync(path.join(home, ".claude/sessions/999.json"), JSON.stringify({ pid: 999, sessionId: "headless", cwd: "/work", tmux: `test:@9.${pane}`, entrypoint: "sdk-cli" }));
  try {
    assert.equal(sidebar.sessionsByPane().get(pane).sessionId, "own");
  } finally {
    ["121", "999"].forEach((pid) => fs.rmSync(path.join(home, `.claude/sessions/${pid}.json`)));
  }
});

test("a tab's conversation is found from the session file that names its pane", () => {
  const pane = tmux("display", "-p", "-t", "test:", "#{pane_id}");
  fs.writeFileSync(path.join(home, ".claude/sessions/111.json"), JSON.stringify({ pid: 111, sessionId: "s1", cwd: "/work", tmux: `test:@9.${pane}`, name: "Login retry", nameSource: "user", startedAt: 1700000000000 }));
  fs.writeFileSync(path.join(home, ".claude/projects/-work/s1.jsonl"), `${JSON.stringify({ type: "user", message: { content: "fix it" } })}\n${JSON.stringify({ type: "assistant", message: { content: [{ type: "text", text: "Fixed." }] } })}\n`);
  try {
    const found = sidebar.sessionsByPane().get(pane);
    assert.deepEqual(found, { sessionId: "s1", cwd: "/work", name: "Login retry", startedAt: 1700000000000 });
    assert.equal(sidebar.lastTurn(sidebar.transcriptFor(found)).reply, "Fixed.");
  } finally {
    fs.rmSync(path.join(home, ".claude/sessions/111.json"));
  }
});

test("the latest reply is the last message Claude wrote text in, whole, even before a prompt it hasn't answered", () => {
  const lines = [
    { type: "user", message: { content: "first prompt" } },
    { type: "assistant", message: { id: "m1", content: [{ type: "text", text: "old reply" }] } },
    { type: "assistant", message: { id: "m2", content: [{ type: "text", text: "Done. Two things:" }] } },
    { type: "assistant", message: { id: "m2", content: [{ type: "text", text: "1. one\n2. two" }] } },
    { type: "assistant", message: { id: "m2", content: [{ type: "tool_use", id: "a", name: "Read", input: {} }] } },
    { type: "assistant", isSidechain: true, message: { id: "s", content: [{ type: "text", text: "a subagent" }] } },
    { type: "assistant", isApiErrorMessage: true, message: { id: "e", content: [{ type: "text", text: "API Error" }] } },
    { type: "assistant", message: { id: "x", model: "<synthetic>", content: [{ type: "text", text: "No response requested." }] } },
    { type: "user", message: { content: "a new prompt" } },
  ];
  assert.equal(sidebar.parseReply(lines.map((l) => JSON.stringify(l)).join("\n")), "Done. Two things:\n\n1. one\n2. two");
  assert.equal(sidebar.parseReply(JSON.stringify(lines[0])), undefined);
  const indented = { type: "assistant", message: { id: "m3", content: [{ type: "text", text: "\n    first()\n    second()\n" }] } };
  assert.equal(sidebar.parseReply(JSON.stringify(indented)), "    first()\n    second()", "a code block's indent is kept");
});

test("M-y's reader prints the reply of the Claude in a pane, and says by its exit code when there is none", () => {
  const pane = tmux("display", "-p", "-t", "test:", "#{pane_id}");
  const read = (target) => {
    try {
      return { out: execFileSync("node", [path.join(__dirname, "../lib/claude-reply.js"), target], { env: { ...process.env, HOME: home }, encoding: "utf8" }), code: 0 };
    } catch (error) {
      return { code: error.status };
    }
  };
  fs.writeFileSync(path.join(home, ".claude/sessions/112.json"), JSON.stringify({ pid: 112, sessionId: "s2", cwd: "/work", tmux: `test:@9.${pane}` }));
  try {
    fs.writeFileSync(path.join(home, ".claude/projects/-work/s2.jsonl"), `${JSON.stringify({ type: "user", message: { content: "fix it" } })}\n`);
    assert.equal(read(pane).code, 4);
    fs.appendFileSync(path.join(home, ".claude/projects/-work/s2.jsonl"), `${JSON.stringify({ type: "assistant", message: { content: [{ type: "text", text: "Fixed it." }] } })}\n`);
    assert.deepEqual(read(pane), { out: "Fixed it.", code: 0 });
    assert.equal(read("%999999").code, 3);
  } finally {
    fs.rmSync(path.join(home, ".claude/sessions/112.json"));
  }
});

test("tabs are ordered by what needs you: waiting longest, finished longest ago, working, then the rest", async () => {
  ["two", "three", "four", "five"].forEach(() => tmux("new-window", "-t", "test", "-c", tmp));
  const now = Math.floor(Date.now() / 1000);
  const mark = (index, state, since) => {
    tmux("set", "-p", "-t", `test:${index}`, "@agent_state", state);
    tmux("set", "-p", "-t", `test:${index}`, "@agent_since", String(since));
    tmux("setw", "-t", `test:${index}`, "@agent", state);
  };
  mark(1, "working", now - 10);
  mark(2, "done", now - 50);
  mark(3, "waiting", now - 20);
  mark(5, "waiting", now - 90);
  const snap = await sidebar.snapshot();
  const order = sidebar.grouped(sidebar.tabsFrom(snap)).flatMap((g) => g.tabs.map((t) => t.index));
  assert.deepEqual(order, [5, 3, 2, 1, 4]);
});

test("every window gets one sidebar on its left, and none once it is turned off", async () => {
  tmux("new-window", "-t", "test", "-c", tmp);
  let snap = await sidebar.snapshot();
  assert.equal(await sidebar.reconcile(snap, sidebar.tabsFrom(snap), config), true);
  tmux("list-windows", "-t", "test", "-F", "#{window_id}").split("\n").forEach((w) => {
    const [left, ...rest] = panes(w);
    assert.equal(left.sidebar, true);
    assert.equal(left.left, 0);
    assert.equal(left.width, config.width);
    assert.equal(rest.filter((p) => p.sidebar).length, 0);
    // Not dimmed like the panes without focus.
    assert.equal(tmux("show", "-pv", "-t", left.id, "window-style"), "bg=terminal");
  });
  // A second look finds nothing to do.
  snap = await sidebar.snapshot();
  assert.equal(await sidebar.reconcile(snap, sidebar.tabsFrom(snap), config), false);

  tmux("set", "-g", "@sidebar_on", "off");
  snap = await sidebar.snapshot();
  await sidebar.reconcile(snap, sidebar.tabsFrom(snap), config);
  assert.equal(tmux("list-panes", "-a", "-F", "#{@sidebar}").split("\n").filter((m) => m === "1").length, 0);
});

test("a sidebar restored as a plain shell gets its client back, and a tab left with only a sidebar closes", async () => {
  // As tmux-resurrect brings one back: a shell with the sidebar's title.
  const restored = tmux("split-window", "-hbf", "-d", "-l", "20", "-t", "test:", "-P", "-F", "#{pane_id}", "cat");
  tmux("select-pane", "-t", restored, "-T", sidebar.TITLE);
  tmux("last-pane", "-t", "test:");
  let snap = await sidebar.snapshot();
  await sidebar.reconcile(snap, sidebar.tabsFrom(snap), config);
  const [left] = panes("test:");
  assert.equal(left.id, restored);
  assert.equal(left.width, config.width);
  assert.match(tmux("display", "-p", "-t", restored, "#{pane_start_command}"), /sidebar-pane\.zsh/);

  const other = tmux("new-window", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  snap = await sidebar.snapshot();
  await sidebar.reconcile(snap, sidebar.tabsFrom(snap), config);
  const main = panes(other).find((p) => !p.sidebar);
  tmux("kill-pane", "-t", main.id);
  snap = await sidebar.snapshot();
  await sidebar.reconcile(snap, sidebar.tabsFrom(snap), config);
  assert.ok(!tmux("list-windows", "-a", "-F", "#{window_id}").split("\n").includes(other));
});

test("nothing is added while tmux-resurrect is restoring", async () => {
  tmux("set", "-g", "@sidebar_hold", String(Math.floor(Date.now() / 1000)));
  const snap = await sidebar.snapshot();
  assert.equal(await sidebar.reconcile(snap, sidebar.tabsFrom(snap), config), false);
  assert.equal(panes("test:").length, 1);
});

test("frames fill the pane exactly, however long the names", async () => {
  tmux("rename-window", "-t", "test:", "a very long name for a tab, longer than the sidebar is wide, so long it wraps more than three lines 🎉");
  const snap = await sidebar.snapshot();
  const tabs = sidebar.tabsFrom(snap);
  const { lines } = sidebar.sidebarLines({ tabs, sections: [], cols: 30, rows: 12, view: { focused: true, cursor: tabs[0].id, frame: 0, here: tabs[0].id } });
  assert.equal(lines.length, 12);
  lines.forEach((line) => assert.equal(sidebar.width(line), 30, JSON.stringify(line)));
  // The name wraps under itself, cut short after three lines.
  const text = lines.map((line) => line.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, ""));
  assert.match(text[3], / \d a very long name for a/);
  assert.match(text[4], /^.{7}tab, longer than the/);
  assert.match(text[5], /…/);
  assert.match(text[6], /cat/);
  const mission = sidebar.missionLines({ tabs, sections: [], cols: 100, rows: 30, view: { view: 0, cursor: 0, frame: 0 }, config });
  assert.equal(mission.length, 30);
  mission.forEach((line) => assert.equal(sidebar.width(line), 100, JSON.stringify(line)));
});

test("a plugin's items are fetched, kept when a later fetch fails, and only named plugins load", async () => {
  fs.writeFileSync(path.join(home, ".config/switchboard/sidebar-plugins/demo.js"), `
    let calls = 0;
    module.exports = {
      title: "Demo",
      sidebar: 2,
      async fetch({ options }) {
        if (++calls > 1) throw new Error("service down\\nstack");
        return [{ id: 1, title: "first " + options.word, state: "attention" }, { id: 2, title: "second", state: "idle" }];
      },
    };`);
  const logged = [];
  const sections = sidebar.loadPlugins({ plugins: { demo: { word: "hello" }, "../evil": {}, missing: {} } }, (...parts) => logged.push(parts.join(" ")));
  assert.deepEqual(sections.map((s) => s.name), ["demo"]);
  assert.ok(logged.some((line) => line.includes("missing")));
  let changed = 0;
  await sidebar.refreshSection(sections[0], () => changed++, () => {});
  assert.equal(sections[0].items[0].title, "first hello");
  await sidebar.refreshSection(sections[0], () => changed++, () => {});
  assert.equal(sections[0].error, "service down");
  assert.equal(sections[0].items.length, 2);
  assert.equal(changed, 2);

  const snap = await sidebar.snapshot();
  const tabs = sidebar.tabsFrom(snap);
  const { lines } = sidebar.sidebarLines({ tabs, sections, cols: 40, rows: 20, view: { focused: false, cursor: tabs[0].id, frame: 0, here: tabs[0].id } });
  const text = lines.join("\n").replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "");
  assert.match(text, /DEMO 2 ! 1/);
  assert.match(text, /first hello/);
});

// The daemon itself, with the sidebar panes running the real zsh client.
const plain = (text) => text.replace(/\x1b\][^\x07]*\x07|\x1b\[[0-9;?]*[A-Za-z]/g, "");
// What a mission control client's screen shows after every write it was sent:
// each write is a whole frame or only the rows that changed, each put in place.
const screen = (writes) => {
  const rows = [];
  for (const text of writes) for (const [, row, line] of text.matchAll(/\x1b\[(\d+);1H((?:(?!\x1b\[\d+;1H)[\s\S])*)/g)) rows[row - 1] = line;
  return plain(rows.join("\n"));
};
const sidebarPane = (target) => panes(target).find((p) => p.sidebar)?.id;

test("an idle tab says how long since its pane last printed anything", async () => {
  await new Promise((resolve) => setTimeout(resolve, 500));
  const tty = tmux("display", "-p", "-t", "test:", "#{pane_tty}");
  const then = new Date(Date.now() - 5 * 60_000);
  fs.utimesSync(tty, then, then);
  const snap = await sidebar.snapshot();
  const [tab] = sidebar.tabsFrom(snap);
  assert.equal(sidebar.stateWords(tab), "idle for 5m · cat");
  // A detail line too long for the sidebar breaks between its parts.
  const { lines } = sidebar.sidebarLines({ tabs: [{ ...tab, pr: "12345" }], sections: [], cols: 26, rows: 8, view: { focused: false, cursor: tab.id, frame: 0, here: tab.id } });
  const text = lines.map((line) => line.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "").trimEnd());
  assert.deepEqual(text.slice(4, 6), ["▌     idle for 5m · cat", "▌     #12345"]);
});

test("a tab is named after its own pane's directory, not its sidebar's, while the sidebar has focus", (t) => {
  const work = path.join(tmp, "named-work");
  const elsewhere = path.join(tmp, "named-elsewhere");
  [work, elsewhere].forEach((dir) => fs.mkdirSync(dir, { recursive: true }));
  const window = tmux("new-window", "-d", "-t", "test", "-c", work, "-P", "-F", "#{window_id}");
  t.after(() => tmux("kill-window", "-t", window));
  const side = tmux("split-window", "-hb", "-d", "-t", window, "-c", elsewhere, "-P", "-F", "#{pane_id}");
  tmux("set", "-p", "-t", side, "@sidebar", "1");
  tmux("select-pane", "-t", side);
  execFileSync("bash", [path.join(__dirname, "../tmux/window-names.sh"), "test"]);
  assert.equal(tmux("display", "-p", "-t", window, "#{@tab_label}"), "named-work");
});

test("a tab is named after its work, not the notes pane moved beside it, which names only a tab of its own", (t) => {
  const work = path.join(tmp, "notes-beside-work");
  const notesHome = path.join(tmp, "notes-home");
  [work, notesHome].forEach((dir) => fs.mkdirSync(dir, { recursive: true }));
  const window = tmux("new-window", "-d", "-t", "test", "-c", work, "-P", "-F", "#{window_id}");
  const alone = tmux("new-window", "-d", "-t", "test", "-c", notesHome, "-P", "-F", "#{window_id}");
  t.after(() => [window, alone].forEach((w) => tmux("kill-window", "-t", w)));
  const notes = tmux("split-window", "-h", "-d", "-t", window, "-c", notesHome, "-P", "-F", "#{pane_id}");
  tmux("set", "-p", "-t", notes, "@notes_pane", "1");
  tmux("select-pane", "-t", notes);
  tmux("set", "-p", "-t", tmux("display", "-p", "-t", alone, "#{pane_id}"), "@notes_pane", "1");
  execFileSync("bash", [path.join(__dirname, "../tmux/window-names.sh"), "test"]);
  assert.equal(tmux("display", "-p", "-t", window, "#{@tab_label}"), "notes-beside-work", "typing in the notes beside it");
  assert.equal(tmux("display", "-p", "-t", alone, "#{@tab_label}"), "notes-home");
});

test("a note's tab is the one linked to it: note-tab.sh goes there rather than opening another", async (t) => {
  const notesDir = fs.mkdtempSync(path.join(tmp, "note-tab-"));
  fs.writeFileSync(path.join(notesDir, "Plan.md"), "# Plan\n");
  const linked = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  t.after(() => tmux("kill-window", "-t", linked));
  tmux("setw", "-t", linked, "@tab_note", "Plan.md");
  const before = tmux("display", "-p", "-t", "test", "#{window_id}");
  assert.notEqual(before, linked);
  // A shell first, then Claude (node with a session file, as claude-pane.sh tells it).
  const claude = tmux("split-window", "-h", "-d", "-t", linked, "-P", "-F", "#{pane_id}", "node -e 'setTimeout(() => {}, 300000)'");
  await new Promise((resolve) => setTimeout(resolve, 500));
  const claudeTty = tmux("display", "-p", "-t", claude, "#{pane_tty}");
  const claudePid = execFileSync("ps", ["-t", claudeTty.replace("/dev/", ""), "-o", "pid=,comm="], { encoding: "utf8" }).split("\n").find((l) => /node/.test(l)).trim().split(/\s+/)[0];
  fs.mkdirSync(path.join(home, ".claude/sessions"), { recursive: true });
  const sessionFile = path.join(home, ".claude/sessions", `${claudePid}.json`);
  fs.writeFileSync(sessionFile, JSON.stringify({ pid: Number(claudePid), sessionId: "note-tab-test" }));
  t.after(() => fs.rmSync(sessionFile, { force: true }));
  execFileSync("bash", [path.join(__dirname, "../tmux/note-tab.sh"), "Plan.md"], { env: { ...process.env, NOTES_DIR: notesDir, NOTES_OPEN_COMMAND: "cat" } });
  assert.equal(tmux("display", "-p", "-t", "test", "#{window_id}"), linked, "brought to the front");
  // Notes beside it (a stand-in, cat), the cursor left in the tab's own pane.
  const beside = tmux("list-panes", "-t", linked, "-F", "#{pane_id} #{?@notes_pane,1,0}").split("\n").find((l) => l.endsWith(" 1"))?.split(" ")[0];
  assert.ok(beside, "Notes beside it");
  t.after(() => { try { tmux("kill-pane", "-t", beside); } catch {} });
  assert.equal(tmux("display", "-p", "-t", linked, "#{pane_id}"), claude, "the cursor in Claude's pane");
  assert.equal(tmux("display", "-p", "-t", beside, "#{pane_at_left}"), "0", "beside Claude, not the shell before it");
  // Going there again from Notes leaves Notes where it is, the cursor back in Claude.
  tmux("select-pane", "-t", beside);
  execFileSync("bash", [path.join(__dirname, "../tmux/note-tab.sh"), "Plan.md"], { env: { ...process.env, NOTES_DIR: notesDir, NOTES_OPEN_COMMAND: "cat" } });
  assert.equal(tmux("display", "-p", "-t", beside, "#{window_id}"), linked, "still beside it");
  assert.equal(tmux("display", "-p", "-t", linked, "#{pane_id}"), claude, "the cursor back in Claude");
  // With note_tab_beside off, only the tab.
  tmux("kill-pane", "-t", beside);
  tmux("set", "-g", "@setting_note_tab_beside", "off");
  t.after(() => tmux("set", "-gu", "@setting_note_tab_beside"));
  execFileSync("bash", [path.join(__dirname, "../tmux/note-tab.sh"), "Plan.md"], { env: { ...process.env, NOTES_DIR: notesDir, NOTES_OPEN_COMMAND: "cat" } });
  assert.equal(tmux("list-panes", "-t", linked).split("\n").length, 2, "no Notes beside it");
  // A note that isn't there opens nothing.
  const windows = tmux("list-windows", "-t", "test").split("\n").length;
  execFileSync("bash", [path.join(__dirname, "../tmux/note-tab.sh"), "Gone.md"], { env: { ...process.env, NOTES_DIR: notesDir } });
  assert.equal(tmux("list-windows", "-t", "test").split("\n").length, windows);
});

test("with no tab of its own, a note asks which of the open tabs is its, or a new one, and links the one picked", (t) => {
  const notesDir = fs.mkdtempSync(path.join(tmp, "note-choose-"));
  fs.writeFileSync(path.join(notesDir, "Plan.md"), "# Plan\n");
  const pick = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  const notes = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  t.after(() => [pick, notes].forEach((w) => tmux("kill-window", "-t", w)));
  tmux("setw", "-t", pick, "@tab_label", "pick-me");
  tmux("setw", "-t", notes, "@notes", "1");
  const bin = fs.mkdtempSync(path.join(tmp, "fzf-bin-"));
  const given = path.join(bin, "given");
  // A stand-in fzf: keeps the list, picks the tab named pick-me.
  fs.writeFileSync(path.join(bin, "fzf"), `#!/bin/sh\ntee '${given}' | grep 'pick-me'\n`, { mode: 0o755 });
  execFileSync("bash", [path.join(__dirname, "../tmux/note-tab.sh"), "choose", "Plan.md", ""], { env: { ...process.env, NOTES_DIR: notesDir, NOTES_OPEN_COMMAND: "cat", PATH: `${bin}:${process.env.PATH}` } });
  const offered = fs.readFileSync(given, "utf8").split("\n").filter(Boolean).map((line) => line.split("\t")[1]);
  assert.equal(offered[0], "+ New tab, in a directory you pick");
  assert.ok(offered.some((name) => /pick-me$/.test(name)), "the open tabs");
  assert.ok(!offered.some((name) => name.startsWith(tmux("display", "-p", "-t", notes, "#{window_index} "))), "not Notes' own");
  assert.equal(tmux("show", "-wqv", "-t", pick, "@tab_note"), "Plan.md", "the one picked is linked");
  assert.equal(tmux("display", "-p", "-t", "test", "#{window_id}"), pick, "and gone to");
});

test("a turn that ends with its pane no longer where the tab's name came from names the tabs again", async (t) => {
  const work = path.join(tmp, "renamed-work");
  fs.mkdirSync(work, { recursive: true });
  // agent-state.sh runs the installed scripts, as setup.sh links them.
  const installed = path.join(home, ".config/tmux");
  fs.mkdirSync(installed, { recursive: true });
  fs.readdirSync(path.join(__dirname, "../tmux"))
    .filter((name) => name.endsWith(".sh") && !fs.existsSync(path.join(installed, name)))
    .forEach((name) => fs.symlinkSync(path.join(__dirname, "../tmux", name), path.join(installed, name)));
  const window = tmux("new-window", "-d", "-t", "test", "-c", work, "-P", "-F", "#{window_id}");
  t.after(() => tmux("kill-window", "-t", window));
  const pane = tmux("display", "-p", "-t", window, "#{pane_id}");
  execFileSync("bash", [path.join(__dirname, "../tmux/window-names.sh"), "test"]);
  assert.equal(tmux("display", "-p", "-t", window, "#{@tab_path}"), work, "the name's directory is kept");
  // Named while a command had the pane somewhere else.
  tmux("setw", "-t", window, "@tab_path", "/elsewhere");
  tmux("rename-window", "-t", window, "elsewhere");

  execFileSync("bash", [path.join(__dirname, "../tmux/agent-state.sh"), "set", "done"], { env: { ...process.env, TMUX_PANE: pane }, input: JSON.stringify({ hook_event_name: "Stop", session_id: "s" }) });

  assert.ok(await until(() => tmux("display", "-p", "-t", window, "#{@tab_path}") === work), "named again from where the pane is");
  assert.equal(tmux("display", "-p", "-t", window, "#{window_name}"), "renamed-work");
});

test("compaction shows the tab working, keeps its state and loop through the session it restarts, a /compact you typed ends idle, and one while idle hands the tab back as it was", async (t) => {
  // A pane running node, as Claude started by claude-here reads (claude-pane.sh).
  const window = tmux("new-window", "-d", "-t", "test", "-P", "-F", "#{window_id}", "node -e 'setTimeout(() => {}, 300000)'");
  t.after(() => tmux("kill-window", "-t", window));
  const pane = tmux("display", "-p", "-t", window, "#{pane_id}");
  // And a session file for its process, as Claude Code writes one.
  const tty = tmux("display", "-p", "-t", pane, "#{pane_tty}");
  const pid = execFileSync("ps", ["-t", tty.replace("/dev/", ""), "-o", "pid=,comm="], { encoding: "utf8" }).split("\n").find((l) => /node/.test(l)).trim().split(/\s+/)[0];
  fs.mkdirSync(path.join(home, ".claude/sessions"), { recursive: true });
  const sessionFile = path.join(home, ".claude/sessions", `${pid}.json`);
  fs.writeFileSync(sessionFile, JSON.stringify({ pid: Number(pid), sessionId: "compact-test" }));
  t.after(() => fs.rmSync(sessionFile, { force: true }));
  const hook = (state, payload) => execFileSync("bash", [path.join(__dirname, "../tmux/agent-state.sh"), "set", state], { env: { ...process.env, TMUX_PANE: pane }, input: JSON.stringify({ session_id: "compact-test", ...payload }) });
  const opt = (name) => tmux("display", "-p", "-t", pane, `#{${name}}`);

  // Compacting in the middle of a review loop's turn.
  hook("working", { hook_event_name: "UserPromptSubmit", prompt: "/review-loop" });
  assert.equal(opt("@agent_loop"), "review");
  hook("working", { hook_event_name: "PreCompact", trigger: "auto" });
  assert.equal(opt("@agent_state"), "working");
  assert.equal(opt("@agent_activity"), "compacting context");
  hook("clear", { hook_event_name: "SessionStart", source: "compact" });
  assert.equal(opt("@agent_state"), "working", "the session compaction restarts keeps the tab working");
  assert.equal(opt("@agent_loop"), "review", "and its loop");
  hook("working", { hook_event_name: "PostCompact", trigger: "auto" });
  assert.equal(opt("@agent_state"), "working", "the turn goes on");
  assert.equal(opt("@agent_activity"), "", "no longer compacting");

  // Compacting while idle, as Claude Code does before the prompt cache
  // expires: the tab is handed back finished, as long ago as it was.
  hook("done", { hook_event_name: "Stop" });
  tmux("set", "-p", "-t", pane, "@agent_since", "1000");
  hook("working", { hook_event_name: "PreCompact", trigger: "auto" });
  assert.equal(opt("@agent_state"), "working", "working while it compacts");
  hook("clear", { hook_event_name: "SessionStart", source: "compact" });
  hook("working", { hook_event_name: "PostCompact", trigger: "auto" });
  assert.equal(opt("@agent_state"), "done", "finished again once it is done");
  assert.equal(opt("@agent_since"), "1000", "since when it finished");
  assert.equal(opt("@agent_activity"), "", "no longer compacting");
  hook("clear", { hook_event_name: "SessionEnd" });
  hook("working", { hook_event_name: "PreCompact", trigger: "auto" });
  hook("working", { hook_event_name: "PostCompact", trigger: "auto" });
  assert.equal(opt("@agent_state"), "", "and an idle one idle again");

  // A /compact you typed, on a tab with nothing going on.
  hook("clear", { hook_event_name: "SessionEnd" });
  assert.equal(opt("@agent_state"), "");
  hook("working", { hook_event_name: "PreCompact", trigger: "manual" });
  assert.equal(opt("@agent_state"), "working", "working while it compacts");
  hook("working", { hook_event_name: "PostCompact", trigger: "manual" });
  hook("clear", { hook_event_name: "SessionStart", source: "compact" });
  assert.equal(opt("@agent_state"), "", "idle once it is done");

  // A new session still starts clear.
  hook("working", { hook_event_name: "UserPromptSubmit", prompt: "hi" });
  hook("clear", { hook_event_name: "SessionStart", source: "startup" });
  assert.equal(opt("@agent_state"), "");
});

test("the status line is about the tab's own pane, not its sidebar, while the sidebar has focus", (t) => {
  const work = path.join(tmp, "status-work");
  const elsewhere = path.join(tmp, "status-elsewhere");
  [work, elsewhere].forEach((dir) => fs.mkdirSync(dir, { recursive: true }));
  const window = tmux("new-window", "-d", "-t", "test", "-c", work, "-P", "-F", "#{window_id}");
  t.after(() => tmux("kill-window", "-t", window));
  const side = tmux("split-window", "-hb", "-d", "-t", window, "-c", elsewhere, "-P", "-F", "#{pane_id}");
  tmux("set", "-p", "-t", side, "@sidebar", "1");
  tmux("select-pane", "-t", side);
  const line = (...flag) => execFileSync("bash", [path.join(__dirname, "../tmux/tab-status.sh"), side, elsewhere, "zsh", "/dev/null", ...flag], { encoding: "utf8" }).replace(/#\[[^\]]*\]/g, "");
  assert.match(line("1"), /^status-work /);
  assert.match(line(), /^status-elsewhere /, "without the flag, the pane it was given");
});

test("tabs with no Claude in them, such as the Slack relay or Notes, are Other, under Idle", () => {
  const base = { session: "$1", sessionName: "test", panes: [], sidebars: [], since: undefined };
  const tabs = [
    { ...base, id: "@1", index: 1, name: "Notes", state: "idle", command: "nvim" },
    { ...base, id: "@2", index: 2, name: "resting", state: "idle", command: "claude", claude: { sessionId: "s" } },
    { ...base, id: "@3", index: 3, name: "Slack relay", state: "idle", command: "node" },
  ];
  assert.deepEqual(sidebar.grouped(tabs).map((g) => [g.title, g.tabs.map((x) => x.name)]), [
    ["Idle", ["resting"]],
    ["Other", ["Notes", "Slack relay"]],
  ]);
  assert.deepEqual(sidebar.staged(tabs).map((g) => g.title), ["Idle", "Other"]);
});

test("a tab between a /loop's ticks is in Loops, after Idle and before Other, and one working is not", () => {
  const base = { session: "$1", sessionName: "test", panes: [], sidebars: [], since: undefined, command: "claude", claude: { sessionId: "s" } };
  const tabs = [
    { ...base, id: "@1", index: 1, name: "Calendar", state: "background", loop: { kind: "scheduled", prompt: "/today", at: Date.now() + 60_000 } },
    { ...base, id: "@2", index: 2, name: "resting", state: "idle" },
    { ...base, id: "@3", index: 3, name: "Notes", state: "idle", command: "nvim", claude: undefined },
    { ...base, id: "@4", index: 4, name: "busy", state: "working", loop: { kind: "scheduled", prompt: "/oncall", at: Date.now() + 60_000 } },
    { ...base, id: "@5", index: 5, name: "reviewing", state: "background", loop: { kind: "review", round: 1 } },
  ];
  assert.deepEqual(sidebar.grouped(tabs).map((g) => [g.title, g.tabs.map((x) => x.name)]), [
    ["Working", ["busy", "reviewing"]],
    ["Idle", ["resting"]],
    ["Loops", ["Calendar"]],
    ["Other", ["Notes"]],
  ]);
});

test("a frozen layout keeps each tab in its group and place while states change", () => {
  const base = { session: "$1", sessionName: "test", panes: [], sidebars: [], since: undefined, command: "zsh", claude: { sessionId: "s" } };
  const before = [
    { ...base, id: "@1", index: 1, name: "one", state: "idle" },
    { ...base, id: "@2", index: 2, name: "two", state: "done", since: 10 },
    { ...base, id: "@3", index: 3, name: "three", state: "working" },
    { ...base, id: "@4", index: 4, name: "four", state: "idle" },
  ];
  const frozen = sidebar.frozenFrom(before);
  // two was seen, four started working, one now waits, and five opened.
  const after = [
    { ...before[0], state: "waiting", since: 20 },
    { ...before[1], state: "idle", since: undefined },
    before[2],
    { ...before[3], state: "working" },
    { ...base, id: "@5", index: 5, name: "five", state: "waiting", since: 5 },
  ];
  assert.deepEqual(sidebar.grouped(after, frozen).map((g) => [g.title, g.tabs.map((x) => x.name)]), [
    ["Needs you", ["two", "five"]],
    ["Working", ["three"]],
    ["Idle", ["one", "four"]],
  ]);
  assert.deepEqual(sidebar.grouped(after).map((g) => [g.title, g.tabs.map((x) => x.name)]), [
    ["Needs you", ["five", "one"]],
    ["Working", ["three", "four"]],
    ["Idle", ["two"]],
  ], "unfrozen, the live order");
});

test("tabs whose PR is in the merge queue or merged get sections of their own, after the ones that need you", () => {
  const base = { session: "$1", sessionName: "test", panes: [], sidebars: [], since: undefined, command: "zsh", claude: { sessionId: "s" } };
  const tabs = [
    { ...base, id: "@1", index: 1, name: "idle one", state: "idle" },
    { ...base, id: "@2", index: 2, name: "queued third", state: "idle", pr: "12", merge: "queued", position: 3 },
    { ...base, id: "@3", index: 3, name: "queued first", state: "working", pr: "13", merge: "queued", position: 1 },
    { ...base, id: "@4", index: 4, name: "landed", state: "idle", pr: "14", merge: "merged" },
    { ...base, id: "@5", index: 5, name: "asking", state: "waiting", pr: "15", merge: "queued", position: 2 },
  ];
  assert.deepEqual(sidebar.grouped(tabs).map((g) => [g.title, g.tabs.map((x) => x.name)]), [
    ["Needs you", ["asking"]],
    ["In merge queue", ["queued first", "queued third"]],
    ["Merged", ["landed"]],
    ["Idle", ["idle one"]],
  ], "waiting stays in Needs you; a queued tab is in the queue even while Claude works; the queue is in queue order");
  assert.equal(sidebar.stateWords(tabs[1]), "in merge queue at 3");
  assert.equal(sidebar.stateWords(tabs[3]), "merged");
  assert.match(sidebar.stateWords(tabs[4]), /^waiting on you · in merge queue at 2$/);
  const { lines } = sidebar.sidebarLines({ tabs, sections: [], cols: 40, rows: 40, view: { focused: false, cursor: "@1", frame: 0, here: "@1" } });
  const text = lines.map((line) => line.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "")).join("\n");
  assert.match(text, /IN MERGE QUEUE 2/);
  assert.match(text, /MERGED 1/);
  assert.match(text, / » +2 queued third/, "an idle queued tab wears the queue's badge");
  assert.match(text, / ◆ +4 landed/);
});

test("tabs landing on the default branch or waiting their turn are a Landing group in the landing queue's order, which a waiting tab's menu changes", async () => {
  const dir = fs.realpathSync(fs.mkdtempSync(path.join(tmp, "landing-")));
  const env = { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  const repo = path.join(dir, "repo");
  execFileSync("git", ["init", "-q", "-b", "main", repo], { env });
  execFileSync("git", ["-C", repo, "commit", "-q", "--allow-empty", "-m", "start"], { env });
  const sessionFiles = [];
  const [one, two, three] = ["one", "two", "three"].map((name) => {
    const worktree = path.join(dir, name);
    execFileSync("git", ["-C", repo, "worktree", "add", "-q", "-b", name, worktree], { env });
    const file = path.join(home, ".claude/sessions", `landing-${name}.json`);
    fs.writeFileSync(file, JSON.stringify({ sessionId: `landing-${name}`, cwd: worktree, pid: process.pid }));
    sessionFiles.push(file);
    return worktree;
  });
  try {
    fs.mkdirSync(path.join(repo, ".git/land.lock"));
    fs.writeFileSync(path.join(repo, ".git/land.lock/holder"), JSON.stringify({ path: one, since: 1 }));
    fs.writeFileSync(path.join(repo, ".git/land.queue"), JSON.stringify([{ path: three, since: 2 }, { path: two, since: 3 }]));
    fs.mkdirSync(path.join(two, "sub"));
    const base = { session: "$1", sessionName: "test", panes: [], sidebars: [], since: undefined, command: "zsh", claude: { sessionId: "s" } };
    const tabs = [
      { ...base, id: "@1", index: 1, name: "elsewhere", state: "working", path: dir },
      { ...base, id: "@2", index: 2, name: "second", state: "working", path: path.join(two, "sub") },
      { ...base, id: "@3", index: 3, name: "lander", state: "working", path: one },
      { ...base, id: "@4", index: 4, name: "first", state: "background", background: 1, path: three },
    ];
    sidebar.landingOf(tabs);
    assert.deepEqual(sidebar.grouped(tabs).map((g) => [g.title, g.tabs.map((x) => x.name)]), [
      ["Working", ["elsewhere"]],
      ["Landing", ["lander", "first", "second"]],
    ], "the landing one first, then the queue in order, a tab in a worktree's subdirectory included");
    assert.equal(sidebar.stateWords(tabs[1]), "working · 3rd in line to land");
    assert.equal(sidebar.stateWords(tabs[2]), "working · landing");

    const labels = (tab) => sidebar.tabChoices({}, tab).map((c) => c.key);
    assert.ok(!labels(tabs[2]).some((key) => ["f", "u", "b"].includes(key)), "the landing one can't be moved");
    assert.ok(!labels(tabs[3]).includes("u") && labels(tabs[3]).includes("b"), "the first in line moves only down");
    await sidebar.tabChoices({}, tabs[1]).find((c) => c.key === "u").act();
    const order = JSON.parse(fs.readFileSync(path.join(repo, ".git/land.queue"), "utf8")).map((entry) => entry.path);
    assert.deepEqual(order, [two, three], "Move up swaps them");

    // Notes moved beside the landing tab and focused: the tab is found by its Claude's directory.
    const notesFocused = [{ ...tabs[2], landing: undefined, path: home, claude: { sessionId: "landing-one", cwd: one } }];
    sidebar.landingOf(notesFocused);
    assert.deepEqual(notesFocused[0].landing, { method: "direct", holding: true, line: 1 }, "focusing Notes beside it keeps its place");
    // A waiter whose Claude has gone leaves the group before the script reads the queue again.
    fs.rmSync(sessionFiles[2]);
    const after = tabs.map((tab) => ({ ...tab, landing: undefined }));
    sidebar.landingOf(after);
    assert.deepEqual(after.filter((tab) => tab.landing).map((tab) => [tab.name, tab.landing.line]), [["second", 2], ["lander", 1]]);
  } finally {
    sessionFiles.forEach((file) => fs.rmSync(file, { force: true }));
  }
});

test("the Landing view shows each repository's landing queue and merge queue as sections in queue order, only while something lands, and a row's menu moves a waiter", async (t) => {
  const dir = fs.realpathSync(fs.mkdtempSync(path.join(tmp, "landview-")));
  const env = { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  const repo = path.join(dir, "direct");
  execFileSync("git", ["init", "-q", "-b", "main", repo], { env });
  execFileSync("git", ["-C", repo, "commit", "-q", "--allow-empty", "-m", "start"], { env });
  const merging = path.join(dir, "merging");
  execFileSync("git", ["init", "-q", "-b", "main", merging], { env });
  execFileSync("git", ["-C", merging, "remote", "add", "origin", "git@github.com:acme/merging.git"], { env });
  const sessionFiles = [];
  const [one, two, three] = ["one", "two", "three"].map((name) => {
    const worktree = path.join(dir, name);
    execFileSync("git", ["-C", repo, "worktree", "add", "-q", "-b", name, worktree], { env });
    const file = path.join(home, ".claude/sessions", `landview-${name}.json`);
    fs.writeFileSync(file, JSON.stringify({ sessionId: `landview-${name}`, cwd: worktree, pid: process.pid }));
    sessionFiles.push(file);
    return worktree;
  });
  const byRepo = { "acme/merging": { url: "https://github.com/acme/merging/queue/main", entries: [
    { position: 2, state: "AWAITING_CHECKS", enqueuedAt: "2026-11-03T10:00:00Z", pullRequest: { number: 7, title: "Mine", url: "https://github.com/acme/merging/pull/7", author: { login: "me" } } },
    { position: 1, state: "MERGEABLE", enqueuedAt: "2026-11-03T09:00:00Z", pullRequest: { number: 6, title: "Theirs", url: "https://github.com/acme/merging/pull/6", author: { login: "someone" } } },
  ] } };
  try {
    const base = { session: "$1", sessionName: "test", panes: [], sidebars: [], since: undefined, command: "zsh", claude: { sessionId: "s" } };
    const tabs = [
      { ...base, id: "@1", index: 1, name: "lander", state: "working", path: one, phase: "pushing" },
      { ...base, id: "@2", index: 2, name: "waiter", state: "working", path: two },
      { ...base, id: "@3", index: 3, name: "queued pr", state: "idle", path: merging, pr: "7", merge: "queued", position: 2 },
    ];
    assert.equal(sidebar.landingView(tabs, sidebar.landingOf(tabs, byRepo.none)), undefined, "nothing landing, no view");
    assert.equal(sidebar.landingView(tabs, [], { keep: true }).empty, "Nothing is landing now.", "kept while in front, it says so");
    assert.equal(sidebar.landingView(tabs, [], { error: "gh isn't signed in" }).error, "gh isn't signed in", "shown while GitHub can't be asked, saying why");
    // Leaving a Landing view kept with nothing landing: the client's place is settled in the list without it.
    const stateTabs = sidebar.state.tabs;
    const stateLanding = sidebar.state.landing;
    Object.assign(sidebar.state, { tabs: [], landing: [] });
    t.after(() => Object.assign(sidebar.state, { tabs: stateTabs, landing: stateLanding }));
    const client = { view: { view: 0, cursor: 0 }, viewTitle: "Landing" };
    const kept = sidebar.viewsFor(client).map((v) => v.title);
    const at = kept.indexOf("Landing");
    assert.ok(at > 0, "kept while in front");
    client.view.view = at + 2;
    client.focus = { view: at + 2 };
    client.viewTitles = kept.join("\n");
    const next = kept[at + 1];
    const settled = sidebar.settleView(client);
    assert.equal(settled[client.view.view - 1].title, next, "the view moved to is the one acted on");
    assert.equal(settled[client.focus.view - 1].title, next, "and the one Tab's focus is on");
    assert.ok(!settled.some((v) => v.title === "Landing"), "and Landing is gone once left");

    fs.mkdirSync(path.join(repo, ".git/land.lock"));
    fs.writeFileSync(path.join(repo, ".git/land.lock/holder"), JSON.stringify({ path: one, since: 1 }));
    fs.writeFileSync(path.join(repo, ".git/land.queue"), JSON.stringify([{ path: two, since: 2 }, { path: three, since: 3 }]));
    const repos = sidebar.landingOf(tabs, byRepo);
    const view = sidebar.landingView(tabs, repos);
    assert.deepEqual(view.items.map((i) => [i.title, i.state]), [
      ["direct · straight to the default branch", "idle"],
      ["1. lander", "active"],
      ["2. waiter", "idle"],
      ["3. three", "idle"],
      ["acme/merging · merge queue", "idle"],
      ["1. #6 Theirs", "idle"],
      ["2. #7 Mine", "active"],
    ], "a section per repository, in queue order, a worktree no tab has by its folder");
    assert.equal(view.items[1].detail, "landing · pushing · tab 1");
    assert.equal(view.items[3].detail, "waiting · no tab");
    assert.deepEqual(tabs[2].landing, { method: "pr", line: 2, holding: false }, "a tab whose PR is queued has its place");
    assert.equal(sidebar.stateWords(tabs[2]), "2nd in line to merge");
    const merged = [{ ...tabs[2], landing: undefined, merge: "merged" }];
    sidebar.landingOf(merged, byRepo);
    assert.equal(merged[0].landing, undefined, "a PR since merged isn't landing, whatever the queue said last");
    assert.deepEqual(sidebar.grouped(tabs).map((g) => [g.title, g.tabs.map((x) => x.name)]), [["Landing", ["lander", "waiter", "queued pr"]]], "landing either way is one group");
    assert.ok(sidebar.builtInViews({ tabs, landing: repos }).map((v) => v.title).join(",").includes("CI,Landing"), "after CI");
    assert.ok(!sidebar.builtInViews({ tabs, landing: [] }).some((v) => v.title === "Landing"));

    const tabsBefore = sidebar.state.tabs;
    sidebar.state.tabs = tabs;
    t.after(() => (sidebar.state.tabs = tabsBefore));
    const choices = sidebar.itemChoices({}, view, view.items[3]).map((c) => c.key);
    assert.deepEqual(choices, ["f", "u", "x", "y"], "the last in line moves up or out, and has no tab to go to");
    await sidebar.itemChoices({}, view, view.items[3]).find((c) => c.key === "u").act();
    assert.deepEqual(JSON.parse(fs.readFileSync(path.join(repo, ".git/land.queue"), "utf8")).map((e) => e.path), [three, two], "Move it up reorders the landing queue");
    assert.ok(!sidebar.itemChoices({}, view, view.items[1]).some((c) => ["f", "u", "b", "x"].includes(c.key)), "the landing one stays where it is");
  } finally {
    sessionFiles.forEach((file) => fs.rmSync(file, { force: true }));
  }
});

test("tabs whose open PR has a failing check are a Build failing group of their own, an idle one wearing a red ✗", () => {
  const base = { session: "$1", sessionName: "test", panes: [], sidebars: [], since: undefined, command: "zsh", claude: { sessionId: "s" } };
  const tabs = [
    { ...base, id: "@1", index: 1, name: "green", state: "idle", pr: "11", prState: "OPEN", checks: { pass: 4, fail: 0, pending: 0 } },
    { ...base, id: "@7", index: 7, name: "red closed", state: "idle", pr: "17", prState: "CLOSED", checks: { pass: 0, fail: 1, pending: 0 } },
    { ...base, id: "@2", index: 2, name: "red idle", state: "idle", pr: "12", prState: "OPEN", checks: { pass: 3, fail: 1, pending: 0 } },
    { ...base, id: "@3", index: 3, name: "red working", state: "working", pr: "13", prState: "OPEN", checks: { pass: 1, fail: 2, pending: 1 } },
    { ...base, id: "@4", index: 4, name: "red asking", state: "waiting", pr: "14", prState: "OPEN", checks: { pass: 0, fail: 1, pending: 0 } },
    { ...base, id: "@5", index: 5, name: "red queued", state: "idle", pr: "15", prState: "OPEN", merge: "queued", position: 1, checks: { pass: 0, fail: 1, pending: 0 } },
    { ...base, id: "@6", index: 6, name: "red ci loop", state: "background", pr: "16", prState: "OPEN", background: 1, loop: { kind: "ci", round: 2 }, checks: { pass: 0, fail: 1, pending: 0 } },
  ];
  assert.deepEqual(sidebar.grouped(tabs).map((g) => [g.title, g.tabs.map((x) => x.name)]), [
    ["Needs you", ["red asking"]],
    ["Build failing", ["red working", "red ci loop", "red idle"]],
    ["In merge queue", ["red queued"]],
    ["Idle", ["green", "red closed"]],
  ], "waiting stays in Needs you and a queued PR in the queue; a failing build is its own group even while Claude works");
  assert.deepEqual(sidebar.staged(tabs).map((g) => [g.title, g.tabs.map((x) => x.name)]), [
    ["Needs you", ["red asking"]],
    ["Build failing", ["red working", "red idle"]],
    ["CI loop", ["red ci loop"]],
    ["In merge queue", ["red queued"]],
    ["Idle", ["green", "red closed"]],
  ], "in mission control a CI loop at work on it keeps its own stage");
  assert.equal(sidebar.staged([{ ...tabs[2], loopSkill: "ci" }])[0].title, "Build failing", "a CI loop that has stopped leaves it Build failing");
  const { lines } = sidebar.sidebarLines({ tabs, sections: [], cols: 40, rows: 40, view: { focused: false, cursor: "@1", frame: 0, here: "@1" } });
  const text = lines.map((line) => line.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "")).join("\n");
  assert.match(text, /✗ BUILD FAILING 3/, "its heading carries the failing mark");
  assert.match(text.split("\n")[0], /✗ 4/, "the counts at the top count the failing builds");
  const red = lines.find((line) => line.includes("✗1 failing") && line.includes("red working") === false && /\x1b\[38;5;160m\x1b\[1m✗\d failing/.test(line));
  assert.ok(red, "a failing count is in bold red even under a working tab");
  assert.match(text, / ✗ +2 red idle/, "an idle failing tab wears the failing badge");
  assert.match(text, /✗1 failing/);
  assert.doesNotMatch(text, / ✗ +3 red working/, "a working one keeps its spinner");
});

test("the PR board searches for merged PRs only every ten minutes, keeping the last answer between", async (t) => {
  const repo = path.join(tmp, "board-repo");
  fs.mkdirSync(repo, { recursive: true });
  execFileSync("git", ["init", "-q", repo]);
  execFileSync("git", ["-C", repo, "remote", "add", "origin", "git@github.com:acme/board.git"]);
  const bin = path.join(tmp, "board-bin");
  fs.mkdirSync(bin, { recursive: true });
  const asked = path.join(tmp, "board-asked");
  const pr = (number, state, mergedAt) => ({ number, title: `PR ${number}`, url: `https://github.com/acme/board/pull/${number}`, isDraft: false, state, mergedAt, reviewDecision: null, repository: { nameWithOwner: "acme/board" }, commits: { nodes: [] } });
  const answer = { data: { open: { nodes: [pr(1, "OPEN")] }, merged: { nodes: [pr(2, "MERGED", "2026-10-28T00:00:00Z")] } } };
  fs.writeFileSync(path.join(bin, "gh"), `#!/bin/sh\nprintf '%s\\n' "$@" >> '${asked}'\necho '${JSON.stringify(answer)}'\n`, { mode: 0o755 });
  const path0 = process.env.PATH;
  process.env.PATH = `${bin}:${path0}`;
  t.after(() => (process.env.PATH = path0));
  const tabs = [{ path: repo }];
  const titles = () => sidebar.board.columns.flatMap((c) => c.items.map((i) => i.title));
  const now = Date.parse("2026-10-29T12:00:00Z");

  await sidebar.fetchBoard(tabs, () => {}, () => {}, now);
  assert.match(fs.readFileSync(asked, "utf8"), /merged=is:pr is:merged/);
  fs.rmSync(asked);
  await sidebar.fetchBoard(tabs, () => {}, () => {}, now + 60_000);
  assert.doesNotMatch(fs.readFileSync(asked, "utf8"), /is:merged/, "a minute later, only the open PRs");
  assert.ok(titles().some((title) => title.includes("PR 2")), "the merged PR stays on the board");
  fs.rmSync(asked);
  await sidebar.fetchBoard(tabs, () => {}, () => {}, now + sidebar.MERGED_EVERY_MS);
  assert.match(fs.readFileSync(asked, "utf8"), /is:merged/, "ten minutes on, merged again");
});

test("the merge queues are each repository's in order, marking a tab's PR, and a repository GitHub fails to answer for keeps its last answer", async (t) => {
  const repo = path.join(tmp, "queue-repo");
  fs.mkdirSync(repo, { recursive: true });
  execFileSync("git", ["init", "-q", repo]);
  execFileSync("git", ["-C", repo, "remote", "add", "origin", "git@github.com:acme/queued.git"]);
  const other = path.join(tmp, "queue-other");
  fs.mkdirSync(other, { recursive: true });
  execFileSync("git", ["init", "-q", other]);
  execFileSync("git", ["-C", other, "remote", "add", "origin", "git@github.com:acme/noqueue.git"]);
  // A stand-in gh: acme/noqueue has no merge queue, acme/queued two PRs in it, 41 second.
  const bin = path.join(tmp, "queue-gh");
  fs.mkdirSync(bin, { recursive: true });
  const answer = { data: { r0: { mergeQueue: null }, r1: { mergeQueue: { url: "https://github.com/acme/queued/queue/main", entries: { nodes: [
    { position: 2, state: "UNMERGEABLE", enqueuedAt: "2026-11-03T10:00:00Z", estimatedTimeToMerge: 600, pullRequest: { number: 41, title: "Mine", url: "https://github.com/acme/queued/pull/41", author: { login: "me" } } },
    { position: 1, state: "AWAITING_CHECKS", enqueuedAt: "2026-11-03T09:00:00Z", estimatedTimeToMerge: 120, pullRequest: { number: 40, title: "Theirs", url: "https://github.com/acme/queued/pull/40", author: { login: "someone" } } },
  ] } } } } };
  fs.writeFileSync(path.join(bin, "gh"), `#!/bin/sh\necho '${JSON.stringify(answer)}'\n`, { mode: 0o755 });
  const path0 = process.env.PATH;
  process.env.PATH = `${bin}:${path0}`;
  t.after(() => (process.env.PATH = path0));
  const tabs = [{ id: "@3", index: 3, path: repo, pr: "41" }, { id: "@4", index: 4, path: other }];
  await sidebar.fetchQueues(tabs, () => {});
  const items = sidebar.queueItems(sidebar.queues.byRepo, tabs);
  assert.deepEqual(items.map((i) => [i.title, i.detail, i.state]), [
    ["1. #40 Theirs", "queued · awaiting checks · merges in ~2m · someone", "idle"],
    ["2. #41 Mine", "queued · can't merge · merges in ~10m · me · tab 3", "attention"],
  ]);
  assert.equal(items[1].queue.queueUrl, "https://github.com/acme/queued/queue/main");
  assert.deepEqual(sidebar.queueItems(sidebar.queues.byRepo, []).map((i) => i.queue.tab), [undefined, undefined], "the tab is matched as it is drawn");
  fs.writeFileSync(path.join(bin, "gh"), `#!/bin/sh\necho '${JSON.stringify({ data: { r0: { mergeQueue: null }, r1: null }, errors: [{ path: ["r1"], message: "Something went wrong" }] })}'\nexit 1\n`, { mode: 0o755 });
  await sidebar.fetchQueues(tabs, () => {});
  assert.equal(sidebar.queueItems(sidebar.queues.byRepo, tabs).length, 2, "a repository GitHub failed to answer for keeps its last answer");
  assert.equal(sidebar.queuesError(), undefined);
  sidebar.queues.byRepo = {};
  fs.writeFileSync(path.join(bin, "gh"), `#!/bin/sh\necho '${JSON.stringify({ data: { r0: { mergeQueue: null }, r1: null }, errors: [{ path: ["r1"], message: "Something went wrong" }] })}'\nexit 1\n`, { mode: 0o755 });
  await sidebar.fetchQueues(tabs, () => {});
  assert.equal(sidebar.queuesError(), "GitHub couldn't say what is in the merge queue of acme/queued.", "with nothing from before, it says so");
  Object.assign(sidebar.queues, { byRepo: {}, error: undefined, unanswered: [] });
});

test("the tabs' PRs are asked about in one query, a gone worktree's by its branch, and only real names reach it", async (t) => {
  const repo = path.join(tmp, "prs-repo");
  fs.mkdirSync(repo, { recursive: true });
  execFileSync("git", ["init", "-q", repo]);
  execFileSync("git", ["-C", repo, "remote", "add", "origin", "git@github.com:acme/widgets.git"]);
  // A stand-in gh: records its query, and answers that 12 is queued at 2, 13
  // merged, 14 neither, and the gone worktree's branch had PR 15, merged.
  const bin = path.join(tmp, "gh-bin");
  fs.mkdirSync(bin, { recursive: true });
  const asked = path.join(tmp, "gh-asked");
  const answer = { data: { r0: {
    a0: { number: 12, state: "OPEN", mergeQueueEntry: { position: 2 } },
    a1: { number: 13, state: "MERGED", mergeQueueEntry: null },
    a2: {
      number: 14, state: "OPEN", mergeQueueEntry: null, reviewDecision: "APPROVED", additions: 340, deletions: 80,
      commits: { nodes: [{ commit: { statusCheckRollup: { contexts: {
        checkRunCountsByState: [{ state: "SUCCESS", count: 10 }, { state: "SKIPPED", count: 2 }, { state: "FAILURE", count: 1 }, { state: "IN_PROGRESS", count: 3 }],
        statusContextCountsByState: [{ state: "PENDING", count: 0 }],
      } } } }] },
    },
    a3: { nodes: [{ number: 15, state: "MERGED", mergeQueueEntry: null }] },
  } } };
  fs.writeFileSync(path.join(bin, "gh"), `#!/bin/sh\nprintf '%s\\n' "$@" > '${asked}'\necho '${JSON.stringify(answer)}'\n`, { mode: 0o755 });
  const path0 = process.env.PATH;
  process.env.PATH = `${bin}:${path0}`;
  t.after(() => (process.env.PATH = path0));

  assert.equal(sidebar.repoOf(repo), "acme/widgets");
  // queue-and-clean deleted this one's worktree, so its label has no PR.
  const gone = { path: path.join(repo, ".claude/worktrees/feat-x") };
  const tabs = [...["12", "13", "14", "13"].map((pr) => ({ pr, path: repo })), gone];
  let changed = 0;
  await sidebar.fetchMerges(tabs, () => changed++);
  const query = fs.readFileSync(asked, "utf8");
  assert.match(query, /r0: repository\(owner: "acme", name: "widgets"\) \{ a0: pullRequest\(number: 12\).* a2: pullRequest\(number: 14\).* a3: pullRequests\(headRefName: "feat-x"/);
  assert.equal((query.match(/number: 13\)/g) || []).length, 1, "each PR asked about once");
  assert.equal(changed, 1);
  assert.deepEqual(sidebar.mergeOf(tabs[0]), { prState: "OPEN", merge: "queued", position: 2 });
  assert.deepEqual(sidebar.mergeOf(tabs[1]), { prState: "MERGED", merge: "merged" });
  // Its checks counted by outcome, its review and its size, for the cards.
  const facts = sidebar.mergeOf(tabs[2]);
  assert.deepEqual(facts, { prState: "OPEN", checks: { pass: 12, fail: 1, pending: 3 }, review: "APPROVED", size: { additions: 340, deletions: 80 } });
  assert.deepEqual(sidebar.prFacts(facts), ["✓12 ✗1 ⏳3", "approved", "+340 −80"]);
  assert.deepEqual(sidebar.mergeOf(gone), { prState: "MERGED", merge: "merged", pr: "15" }, "found by its branch, PR and all");
  // Shown with that PR, the tab is still found by its branch next time.
  assert.deepEqual(sidebar.mergeOf({ ...gone, pr: "15" }), { prState: "MERGED", merge: "merged", pr: "15" });
  // The same answers again change nothing.
  await sidebar.fetchMerges(tabs, () => changed++);
  assert.equal(changed, 1);

  assert.equal(
    sidebar.queryFor(['acme/widgets#1) { x } #', 'a"b/c#2', "acme/widgets@x\" }", "acme/widgets#7"]).query,
    `query { r0: repository(owner: "acme", name: "widgets") { a0: pullRequest(number: 7) { ${sidebar.PR_FIELDS} } } }`,
  );
});

test("a repository is found from its origin in every form git takes, SSH over port 443 included", () => {
  // GitHub's SSH over 443, for networks that block port 22, names a port
  // between the host and the path; a numeric owner in the scp form isn't one.
  const remotes = {
    "https://github.com/acme/widgets.git": "acme/widgets",
    "https://github.com/acme/widgets": "acme/widgets",
    "git@github.com:acme/widgets.git": "acme/widgets",
    "ssh://git@github.com/acme/widgets.git": "acme/widgets",
    "ssh://git@ssh.github.com:443/acme/widgets.git": "acme/widgets",
    "git@github.com:1234/widgets.git": "1234/widgets",
    "git@gitlab.com:acme/widgets.git": null,
    // A gist (a CLAUDE.md kept in one) is no repository GitHub's API knows.
    "https://gist.github.com/acme/0123abcd.git": null,
  };
  Object.entries(remotes).forEach(([url, want], i) => {
    const repo = path.join(tmp, `origin-form-${i}`);
    fs.mkdirSync(repo, { recursive: true });
    execFileSync("git", ["init", "-q", repo]);
    execFileSync("git", ["-C", repo, "remote", "add", "origin", url]);
    assert.equal(sidebar.repoOf(repo), want, url);
  });
});

test("a tab whose turn ended with background tasks still running reads as working in the background", () => {
  const file = path.join(home, ".claude/projects/-work/background.jsonl");
  const now = Date.now();
  const started = (id, ms) => JSON.stringify({ type: "assistant", timestamp: new Date(ms).toISOString(), message: { content: [{ type: "tool_use", id: `use-${id}`, name: "Bash", input: { command: "npm test", run_in_background: true } }] } }) + "\n" + JSON.stringify({ type: "user", timestamp: new Date(ms).toISOString(), message: { content: [{ type: "tool_result", tool_use_id: `use-${id}`, content: `Command running in background with ID: ${id}. Output is being written to: /tmp/x` }] } });
  const ended = (id) => JSON.stringify({ type: "user", timestamp: new Date(now).toISOString(), origin: { kind: "task-notification" }, message: { content: `<task-notification>\n<task-id>${id}</task-id>\n<status>completed</status>\n</task-notification>` } });
  fs.writeFileSync(file, [
    started("old1", now - 60_000), // before this process started: died with the last one
    started("done1", now - 5_000),
    ended("done1"),
    started("run1", now - 4_000),
    started("run2", now - 3_000),
  ].join("\n") + "\n");
  const since = now - 10_000;
  assert.equal(sidebar.backgroundTasks(file, since), 2);
  // Read as it grows: one more ends.
  fs.appendFileSync(file, ended("run2") + "\n");
  assert.equal(sidebar.backgroundTasks(file, since), 1);
  fs.appendFileSync(file, ended("run1") + "\n");
  assert.equal(sidebar.backgroundTasks(file, since), 0);

  // An end Claude Code writes as a queued command in an attachment entry, as
  // newer versions do, ends it too.
  fs.appendFileSync(file, started("run3", now - 2_000) + "\n");
  assert.equal(sidebar.backgroundTasks(file, since), 1);
  // A watch's event keeps a task going; text that only mentions <event> in a
  // summary doesn't.
  fs.appendFileSync(file, JSON.stringify({ type: "user", timestamp: new Date(now).toISOString(), origin: { kind: "task-notification" }, message: { content: "<task-notification>\n<task-id>run3</task-id>\n<summary>a line</summary>\n<event>NEW: x</event>\n</task-notification>" } }) + "\n");
  assert.equal(sidebar.backgroundTasks(file, since), 1);
  fs.appendFileSync(file, JSON.stringify({ type: "user", timestamp: new Date(now).toISOString(), origin: { kind: "task-notification" }, message: { content: "<task-notification>\n<task-id>run3</task-id>\n<summary>Monitor event: \"<status>completed</status>\"</summary>\n<event>checks=pending</event>\n</task-notification>" } }) + "\n");
  assert.equal(sidebar.backgroundTasks(file, since), 1, "a status inside a summary isn't the notification's own");
  fs.appendFileSync(file, JSON.stringify({ type: "user", timestamp: new Date(now).toISOString(), origin: { kind: "task-notification" }, message: { content: "<task-notification>\n<task-id>run3</task-id>\n<summary>Quoted output:\n<status>completed</status>\n</summary>\n<event>checks=pending\n<status>done</status></event>\n</task-notification>" } }) + "\n");
  assert.equal(sidebar.backgroundTasks(file, since), 1, "nor one on a line of its own inside a summary or an event");
  // A watch's event whose summary mentions the opening tag keeps it going.
  fs.appendFileSync(file, JSON.stringify({ type: "attachment", timestamp: new Date(now).toISOString(), attachment: { type: "queued_command", commandMode: "task-notification", prompt: "<task-notification>\n<task-id>run3</task-id>\n<summary>Found <task-notification> in output</summary>\n<event>NEW: x</event>\n</task-notification>" } }) + "\n");
  assert.equal(sidebar.backgroundTasks(file, since), 1);
  // One watch's end, with no status, sent with another's event: each read as its own.
  fs.appendFileSync(file, started("m1", now - 1_000) + "\n");
  fs.appendFileSync(file, JSON.stringify({ type: "user", timestamp: new Date(now).toISOString(), origin: { kind: "task-notification" }, message: { content: "<task-notification>\n<task-id>m1</task-id>\n<summary>Monitor source ended.</summary>\n</task-notification>\n<task-notification>\n<task-id>run3</task-id>\n<summary>Monitor event</summary>\n<event>changed</event>\n</task-notification>" } }) + "\n");
  assert.equal(sidebar.backgroundTasks(file, since), 1, "m1 ended, run3 still going");
  // A notification quoted in another's summary ends nothing; two sent together each do.
  fs.appendFileSync(file, started("other", now - 1_000) + "\n");
  fs.appendFileSync(file, JSON.stringify({ type: "user", timestamp: new Date(now).toISOString(), origin: { kind: "task-notification" }, message: { content: "<task-notification>\n<task-id>other</task-id>\n<status>completed</status>\n<summary>Quoted: <task-notification><task-id>run3</task-id><status>completed</status></task-notification></summary>\n</task-notification>" } }) + "\n");
  assert.equal(sidebar.backgroundTasks(file, since), 1);
  fs.appendFileSync(file, started("part1", now - 1_000) + "\n" + started("part2", now - 1_000) + "\n");
  fs.appendFileSync(file, JSON.stringify({ type: "user", timestamp: new Date(now).toISOString(), origin: { kind: "task-notification" }, message: { content: [{ type: "text", text: "<task-notification>\n<task-id>part1</task-id>\n<status>completed</status>\n</task-notification>" }, { type: "text", text: "<task-notification>\n<task-id>part2</task-id>\n<status>completed</status>\n</task-notification>" }] } }) + "\n");
  assert.equal(sidebar.backgroundTasks(file, since), 1, "two sent as separate parts of one message each end theirs");
  fs.appendFileSync(file, started("two1", now - 1_000) + "\n" + started("two2", now - 1_000) + "\n");
  fs.appendFileSync(file, JSON.stringify({ type: "user", timestamp: new Date(now).toISOString(), origin: { kind: "task-notification" }, message: { content: "<task-notification>\n<task-id>two1</task-id>\n<status>completed</status>\n</task-notification>\n<task-notification>\n<task-id>two2</task-id>\n<status>completed</status>\n</task-notification>" } }) + "\n");
  assert.equal(sidebar.backgroundTasks(file, since), 1);
  // A message typed mid-turn is queued the same way, marked as a prompt, and
  // one quoting a notification ends nothing, even starting with it.
  fs.appendFileSync(file, JSON.stringify({ type: "attachment", timestamp: new Date(now).toISOString(), attachment: { type: "queued_command", commandMode: "prompt", prompt: "Explain this: <task-notification><task-id>run3</task-id><status>completed</status></task-notification>" } }) + "\n");
  fs.appendFileSync(file, JSON.stringify({ type: "attachment", timestamp: new Date(now).toISOString(), attachment: { type: "queued_command", commandMode: "prompt", prompt: "<task-notification><task-id>run3</task-id><status>completed</status></task-notification>\nWhy?" } }) + "\n");
  assert.equal(sidebar.backgroundTasks(file, since), 1);
  fs.appendFileSync(file, JSON.stringify({ type: "attachment", timestamp: new Date(now).toISOString(), attachment: { type: "queued_command", commandMode: "task-notification", origin: { kind: "task-notification" }, prompt: "<task-notification>\n<task-id>run3</task-id>\n<status>completed</status>\n<summary>Background command \"Find <event> tags\" completed</summary>\n</task-notification>" } }) + "\n");
  assert.equal(sidebar.backgroundTasks(file, since), 0);

  // A subagent run in the foreground names its agentId once it has finished:
  // nothing is left running. One run in the background is.
  fs.appendFileSync(file, [
    JSON.stringify({ type: "assistant", timestamp: new Date(now).toISOString(), message: { content: [{ type: "tool_use", id: "use-fg", name: "Agent", input: { prompt: "look" } }] } }),
    JSON.stringify({ type: "user", timestamp: new Date(now).toISOString(), message: { content: [{ type: "tool_result", tool_use_id: "use-fg", content: "Done. agentId: fg1 (internal ID)" }] } }),
  ].join("\n") + "\n");
  assert.equal(sidebar.backgroundTasks(file, since), 0);
  fs.appendFileSync(file, [
    JSON.stringify({ type: "assistant", timestamp: new Date(now).toISOString(), message: { content: [{ type: "tool_use", id: "use-bg", name: "Agent", input: { prompt: "look", run_in_background: true } }] } }),
    JSON.stringify({ type: "user", timestamp: new Date(now).toISOString(), message: { content: [{ type: "tool_result", tool_use_id: "use-bg", content: "Started. agentId: bg1" }] } }),
  ].join("\n") + "\n");
  assert.equal(sidebar.backgroundTasks(file, since), 1);
  fs.appendFileSync(file, ended("bg1") + "\n");
  assert.equal(sidebar.backgroundTasks(file, since), 0);

  // A command that outran its timeout and was moved to the background runs on
  // too, and one whose output only quotes those words starts nothing.
  fs.appendFileSync(file, [
    JSON.stringify({ type: "user", timestamp: new Date(now).toISOString(), message: { content: [{ type: "tool_result", tool_use_id: "use-slow", content: "Command did not complete within its 120s timeout and was moved to the background (ID: slow1). Output is being written to: /tmp/x" }] } }),
    JSON.stringify({ type: "user", timestamp: new Date(now).toISOString(), message: { content: [{ type: "tool_result", tool_use_id: "use-grep", content: "log: Command did not complete within its 120s timeout and was moved to the background (ID: quoted1)" }] } }),
  ].join("\n") + "\n");
  assert.equal(sidebar.backgroundTasks(file, since), 1);
  fs.appendFileSync(file, ended("slow1") + "\n");
  assert.equal(sidebar.backgroundTasks(file, since), 0);

  const tab = { id: "@9", index: 9, name: "review", state: "background", background: 2, session: "$1", panes: [], sidebars: [] };
  assert.equal(sidebar.stateWords(tab), "working in background · 2 tasks");
  assert.deepEqual(sidebar.grouped([tab, { ...tab, id: "@8", index: 8, state: "done", background: 0 }]).map((g) => [g.title, g.tabs.map((x) => x.id)]), [
    ["Needs you", ["@8"]],
    ["Working", ["@9"]],
  ], "not one that needs you");
});

test("what changed on today's calendar since it was last seen: added, moved, renamed and gone, by Google ID when there is one", () => {
  const at = (h) => `2026-11-02T${String(h).padStart(2, "0")}:00:00.000Z`;
  const seen = { date: "2026-11-02", events: {
    a1: { title: "Standup", start: at(14), end: at(15), allDay: false },
    b2: { title: "Planning", start: at(16), end: at(17), allDay: false },
    c3: { title: "Retro", start: at(18), end: at(19), allDay: false },
    "Lunch@2026-11-02T12:00:00.000Z": { title: "Lunch", start: at(12), end: at(13), allDay: false },
  } };
  const day = { date: "2026-11-02", events: [
    { id: "a1", title: "Standup", start: at(15), end: at(16) },
    { id: "b2", title: "Sprint planning", start: at(16), end: at(17) },
    { id: "d4", title: "1:1", start: at(20), end: at(21) },
    { title: "Lunch", start: at(12), end: at(13) },
  ] };
  const said = sidebar.changesFrom(seen, day).map((c) => c.text.replace(/, .*| to .*/, ""));
  assert.deepEqual(said, ["Standup moved", "Planning renamed Sprint planning", "1:1 added", "Retro gone from the calendar"]);
  const series = { date: "2026-11-02", events: { "rec_20261102T140000Z": { title: "Sync", start: at(14), end: at(15), allDay: false } } };
  assert.deepEqual(sidebar.changesFrom(series, { date: "2026-11-02", events: [{ id: "rec_20261102T150000Z", title: "Sync", start: at(15), end: at(16) }] }).map((c) => c.text.replace(/ to .*/, "")), ["Sync moved"], "a repeating event moved to another time, as one");
  const oneOffs = { date: "2026-11-02", events: { "event_123@example.com": { title: "A", start: at(9), end: at(10), allDay: false } } };
  assert.deepEqual(sidebar.changesFrom(oneOffs, { date: "2026-11-02", events: [{ key: "event_456@example.com", title: "B", start: at(15), end: at(16) }] }).map((c) => c.text.replace(/, .*/, "")), ["B added", "A gone from the calendar"], "two one-off events stay two");
  assert.deepEqual(sidebar.changesFrom(seen, { date: day.date, events: Object.entries(seen.events).map(([key, e]) => ({ ...(key.includes("@") ? {} : { id: key }), ...e })) }), [], "the same day, nothing");
});

test("a turn an API error cut short is found in the transcript until the conversation goes on", () => {
  const file = path.join(home, ".claude/projects/-work/api-error.jsonl");
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const now = Date.now();
  const at = (ms) => new Date(now - ms).toISOString();
  const entries = [
    { type: "user", timestamp: at(9_000), message: { content: "fix the flaky test" } },
    { type: "assistant", timestamp: at(8_000), message: { content: [{ type: "text", text: "Looking." }] } },
    { type: "assistant", timestamp: at(7_000), isApiErrorMessage: true, error: "server_error", message: { content: [{ type: "text", text: "API Error: Connection lost mid-response. The response above may be incomplete." }] } },
  ];
  fs.writeFileSync(file, entries.map((entry) => JSON.stringify(entry)).join("\n") + "\n");
  assert.equal(sidebar.backgroundWork(file, now - 60_000).apiError, "API Error: Connection lost mid-response. The response above may be incomplete.");
  assert.equal(sidebar.backgroundWork(file, now - 60_000).apiErrorAt, Date.parse(at(7_000)), "and when it came");
  assert.equal(sidebar.backgroundWork(file, now - 1_000).apiError, undefined, "not one from before the session");
  fs.appendFileSync(file, JSON.stringify({ type: "progress", timestamp: at(3_000), data: { type: "agent_progress", message: { type: "assistant" } } }) + "\n");
  fs.appendFileSync(file, JSON.stringify({ type: "assistant", isSidechain: true, timestamp: at(2_000), message: { content: [{ type: "text", text: "a subagent" }] } }) + "\n");
  fs.appendFileSync(file, JSON.stringify({ type: "user", timestamp: at(2_600), message: { content: "<command-message>cost</command-message><command-name>/cost</command-name>" } }) + "\n");
  fs.appendFileSync(file, JSON.stringify({ type: "user", timestamp: at(2_500), message: { content: "<local-command-stdout>Total cost: $1</local-command-stdout>" } }) + "\n");
  fs.appendFileSync(file, JSON.stringify({ type: "user", isMeta: true, timestamp: at(2_400), message: { content: "Caveat" } }) + "\n");
  fs.appendFileSync(file, JSON.stringify({ type: "user", origin: { kind: "task-notification" }, timestamp: at(2_300), message: { content: "<task-notification>done</task-notification>" } }) + "\n");
  assert.ok(sidebar.backgroundWork(file, now - 60_000).apiError, "a subagent's lines, progress, a command's output, a note of Claude Code's or a task's report don't clear it");
  const nested = path.join(home, ".claude/projects/-work/nested-error.jsonl");
  fs.writeFileSync(nested, JSON.stringify({ type: "progress", timestamp: at(2_000), data: { type: "agent_progress", message: { type: "assistant", isApiErrorMessage: true } } }) + "\n");
  assert.equal(sidebar.backgroundWork(nested, now - 60_000).apiError, undefined, "nor does a subagent's error set one");
  fs.appendFileSync(file, JSON.stringify({ type: "user", timestamp: at(1_000), message: { content: "try again" } }) + "\n");
  assert.equal(sidebar.backgroundWork(file, now - 60_000).apiError, undefined, "said something after it, it is gone");
});

test("a turn interrupted on a permission request is found in the transcript until the conversation goes on", () => {
  const file = path.join(home, ".claude/projects/-work/interrupted.jsonl");
  const now = Date.now();
  const at = (ms) => new Date(now - ms).toISOString();
  const entries = [
    { type: "user", timestamp: at(9_000), message: { content: "push it" } },
    { type: "assistant", timestamp: at(8_000), message: { content: [{ type: "tool_use", id: "p1", name: "Bash", input: { command: "git push" } }] } },
    { type: "user", timestamp: at(7_000), message: { content: [{ type: "tool_result", tool_use_id: "p1", is_error: true, content: "The user doesn't want to proceed with this tool use." }] } },
    { type: "user", timestamp: at(7_000), message: { content: [{ type: "text", text: "[Request interrupted by user for tool use]" }] } },
  ];
  fs.writeFileSync(file, entries.map((entry) => JSON.stringify(entry)).join("\n") + "\n");
  assert.equal(sidebar.backgroundWork(file, now - 60_000).interruptedAt, Date.parse(at(7_000)));
  assert.equal(sidebar.backgroundWork(file, now - 1_000).interruptedAt, undefined, "not one from before the session");
  fs.appendFileSync(file, JSON.stringify({ type: "assistant", isSidechain: true, timestamp: at(6_000), message: { content: [{ type: "text", text: "a subagent" }] } }) + "\n");
  fs.appendFileSync(file, JSON.stringify({ type: "user", isMeta: true, timestamp: at(5_000), message: { content: "Caveat" } }) + "\n");
  fs.appendFileSync(file, JSON.stringify({ type: "user", timestamp: at(4_000), message: { content: [{ type: "tool_result", tool_use_id: "p2", content: "[Request interrupted by user for tool use]" }] } }) + "\n");
  assert.equal(sidebar.backgroundWork(file, now - 60_000).interruptedAt, Date.parse(at(7_000)), "a subagent's lines, a note of Claude Code's or a tool's result don't clear it");
  fs.appendFileSync(file, JSON.stringify({ type: "user", timestamp: at(1_000), message: { content: "push it to a branch instead" } }) + "\n");
  assert.equal(sidebar.backgroundWork(file, now - 60_000).interruptedAt, undefined, "said something after it, it is gone");
});

test("a tab left waiting on a permission request you denied is idle again, and its pane is cleared, unless it has moved on since", async (t) => {
  // A pane running node, as Claude started by claude-here reads (claude-pane.sh).
  const window = tmux("new-window", "-d", "-t", "test", "-P", "-F", "#{window_id}", "node -e 'setTimeout(() => {}, 300000)'");
  t.after(() => tmux("kill-window", "-t", window));
  const pane = tmux("display", "-p", "-t", window, "#{pane_id}");
  const tty = tmux("display", "-p", "-t", pane, "#{pane_tty}");
  const pid = execFileSync("ps", ["-t", tty.replace("/dev/", ""), "-o", "pid=,comm="], { encoding: "utf8" }).split("\n").find((l) => /node/.test(l)).trim().split(/\s+/)[0];
  const sessionFile = path.join(home, ".claude/sessions", `${pid}.json`);
  fs.writeFileSync(sessionFile, JSON.stringify({ pid: Number(pid), sessionId: "denied", cwd: "/work", tmux: `test:${window}.${pane}`, startedAt: Date.now() - 60_000 }));
  t.after(() => fs.rmSync(sessionFile, { force: true }));
  const file = path.join(home, ".claude/projects/-work/denied.jsonl");
  const hook = (state, payload) => execFileSync("bash", [path.join(__dirname, "../tmux/agent-state.sh"), "set", state], { env: { ...process.env, TMUX_PANE: pane }, input: JSON.stringify({ session_id: "denied", ...payload }) });
  const opt = (name) => tmux("display", "-p", "-t", pane, `#{${name}}`);
  const tab = async () => sidebar.tabsFrom(await sidebar.snapshot()).find((x) => x.id === window);
  const say = (entry) => fs.appendFileSync(file, JSON.stringify({ timestamp: new Date().toISOString(), ...entry }) + "\n");

  fs.writeFileSync(file, "");
  say({ type: "user", message: { content: "push it" } });
  hook("waiting", { hook_event_name: "PermissionRequest", tool_name: "Bash", tool_input: { command: "git push" } });
  assert.equal(opt("@agent_wait"), "permission: Bash git push");
  assert.equal((await tab()).state, "waiting", "with no interruption, it waits");

  say({ type: "user", message: { content: [{ type: "text", text: "[Request interrupted by user for tool use]" }] } });
  const handed = await tab();
  assert.equal(handed.state, "idle", "interrupted, it is idle");
  assert.equal(handed.wait, undefined, "waiting on nothing");
  assert.deepEqual(handed.interrupted, { pane, at: handed.since }, "for the daemon to clear the pane after");

  // A prompt typed since wins: the pane has moved on.
  tmux("set", "-p", "-t", pane, "@agent_since", String(handed.since + 5));
  execFileSync("bash", [path.join(__dirname, "../tmux/agent-state.sh"), "interrupted", pane, String(handed.since)]);
  assert.equal(opt("@agent_state"), "waiting", "a pane that has moved on is left alone");

  tmux("set", "-p", "-t", pane, "@agent_since", String(handed.since));
  execFileSync("bash", [path.join(__dirname, "../tmux/agent-state.sh"), "interrupted", pane, String(handed.since)]);
  assert.equal(opt("@agent_state"), "", "the pane is cleared");
  assert.equal(opt("@agent_wait"), "");
  assert.equal(tmux("display", "-p", "-t", window, "#{@agent}"), "", "and so is the window's");
});

test("a background command stays running though Claude Code copies its request, untimed, into the transcript after it", () => {
  const file = path.join(home, ".claude/projects/-work/blob.jsonl");
  const now = Date.now();
  const at = (ms) => new Date(now - ms).toISOString();
  const call = { type: "tool_use", id: "c1", name: "Bash", input: { command: "codex-review --base main", description: "review", run_in_background: true } };
  const started = { type: "tool_result", tool_use_id: "c1", content: "Command running in background with ID: rev1. Output is being written to: /tmp/x" };
  const entries = [
    { type: "assistant", timestamp: at(5_000), message: { content: [call] } },
    { type: "user", timestamp: at(4_000), message: { content: [started] } },
    // The request's copy, with no time of its own.
    { type: "api-request-blob", message: { content: [call, started] } },
  ];
  fs.writeFileSync(file, entries.map((entry) => JSON.stringify(entry)).join("\n") + "\n");
  const work = sidebar.backgroundWork(file, now - 60_000);
  assert.equal(work.count, 1, "still running");
  assert.equal(work.loop?.kind, "review", "and seen as the review loop");
});

test("a Monitor watch runs through its events until it expires, and a tab only watching is in Monitoring", () => {
  const file = path.join(home, ".claude/projects/-work/watch.jsonl");
  const now = Date.now();
  const at = (ms) => new Date(now - ms).toISOString();
  const event = { type: "user", timestamp: at(1_000), origin: { kind: "task-notification" }, message: { content: "<task-notification>\n<task-id>m1</task-id>\n<summary>Monitor event: \"CI\"</summary>\n<event>checks=pending</event>\n</task-notification>" } };
  const entries = [
    { type: "assistant", timestamp: at(5_000), message: { content: [{ type: "tool_use", id: "w1", name: "Monitor", input: { description: "CI on #12", command: "gh pr checks 12", timeout_ms: 1800000 } }] } },
    { type: "user", timestamp: at(5_000), message: { content: [{ type: "tool_result", tool_use_id: "w1", content: "Monitor started (task m1, expires in 30m unless the source ends first; you get one notice at expiry)" }] } },
    event,
  ];
  fs.writeFileSync(file, entries.map((entry) => JSON.stringify(entry)).join("\n") + "\n");
  assert.deepEqual(sidebar.backgroundWork(file, now - 60_000), { count: 1, monitors: ["CI on #12"] }, "an event doesn't end it");

  const tab = { id: "@9", index: 9, name: "watcher", state: "background", background: 1, monitors: ["CI on #12"], session: "$1", panes: [], sidebars: [] };
  assert.deepEqual(sidebar.grouped([tab]).map((g) => g.title), ["Monitoring"]);
  assert.equal(sidebar.stateWords(tab), "monitoring · CI on #12");
  assert.deepEqual(sidebar.grouped([{ ...tab, background: 2 }]).map((g) => g.title), ["Working"], "other work as well is working");

  // A command whose output only quotes those words starts nothing.
  fs.appendFileSync(file, [
    { type: "assistant", timestamp: at(500), message: { content: [{ type: "tool_use", id: "g1", name: "Bash", input: { command: "grep -h 'Monitor started' *.jsonl" } }] } },
    { type: "user", timestamp: at(500), message: { content: [{ type: "tool_result", tool_use_id: "g1", content: "Monitor started (task X, expires in 30m unless…)\nCommand running in background with ID: Y" }] } },
  ].map((entry) => JSON.stringify(entry)).join("\n") + "\n");
  assert.deepEqual(sidebar.backgroundWork(file, now - 60_000), { count: 1, monitors: ["CI on #12"] }, "quoted, not started");

  fs.appendFileSync(file, JSON.stringify({ type: "user", timestamp: at(0), origin: { kind: "task-notification" }, message: { content: "<task-notification>\n<task-id>m1</task-id>\n<summary>Monitor expired after 30m with no events delivered.</summary>\n</task-notification>" } }) + "\n");
  assert.deepEqual(sidebar.backgroundWork(file, now - 60_000), { count: 0, monitors: [] }, "its expiry ends it");
});

test("a loop running in the background reads as that loop, with its round: review, CI, queue-and-clean", () => {
  const file = path.join(home, ".claude/projects/-work/review.jsonl");
  const now = Date.now();
  const at = (ms) => new Date(now - ms).toISOString();
  const run = (useId, taskId, command, ms) => [
    { type: "assistant", timestamp: at(ms), message: { content: [{ type: "tool_use", id: useId, name: "Bash", input: { command, run_in_background: true } }] } },
    { type: "user", timestamp: at(ms), message: { content: [{ type: "tool_result", tool_use_id: useId, content: `Command running in background with ID: ${taskId}. Output is being written to: /tmp/x` }] } },
  ];
  const ended = (taskId) => ({ type: "user", timestamp: at(0), origin: { kind: "task-notification" }, message: { content: `<task-notification>\n<task-id>${taskId}</task-id>\n<status>completed</status>\n</task-notification>` } });
  const review = "node .claude/skills/review-loop/scripts/codex-review.cjs --base abc --no-fetch";
  const entries = [
    { type: "user", timestamp: at(9_000), message: { content: "<command-name>/review-loop</command-name>\n<command-args></command-args>" } },
    ...run("u1", "r1a", review, 8_000), ...run("u2", "r1b", review, 8_000),
    ended("r1a"), ended("r1b"),
    ...run("u3", "r2a", review, 4_000), ...run("u4", "r2b", review, 4_000),
  ];
  fs.writeFileSync(file, entries.map((entry) => JSON.stringify(entry)).join("\n") + "\n");
  assert.deepEqual(sidebar.backgroundWork(file, now - 60_000), { count: 2, loop: { kind: "review", round: 2 }, monitors: [] });

  // A CI loop next: its watch, started again after a fix, counts one a round.
  const ci = "info=$(gh pr view 12 --json state); checks=$(gh pr checks 12 --json name,bucket)";
  fs.appendFileSync(file, [
    ended("r2a"), ended("r2b"),
    { type: "user", timestamp: at(3_000), message: { content: "<command-name>/ci-loop</command-name>" } },
    ...run("u5", "ci1", ci, 2_500), ended("ci1"), ...run("u6", "ci2", ci, 1_000),
  ].map((entry) => JSON.stringify(entry)).join("\n") + "\n");
  assert.deepEqual(sidebar.backgroundWork(file, now - 60_000), { count: 1, loop: { kind: "ci", round: 2 }, monitors: [] });
  // queue-and-clean's watch reads the checks too, but is the queue's.
  fs.appendFileSync(file, [ended("ci2"), ...run("u7", "q1", "gh api graphql -f query='{ pullRequest { mergeQueueEntry { state } } }'; gh pr checks 12 --required", 500)].map((entry) => JSON.stringify(entry)).join("\n") + "\n");
  assert.deepEqual(sidebar.backgroundWork(file, now - 60_000), { count: 1, loop: { kind: "queue", round: 1 }, monitors: [] });
  // So is its landing's wait for the landing lock.
  fs.appendFileSync(file, [ended("q1"), ...run("u9", "q2", "node ~/.claude/skills/queue-and-clean/scripts/land-lock.js wait", 300)].map((entry) => JSON.stringify(entry)).join("\n") + "\n");
  assert.deepEqual(sidebar.backgroundWork(file, now - 60_000), { count: 1, loop: { kind: "queue", round: 2 }, monitors: [] });
  // Anything else is plain background work.
  fs.appendFileSync(file, [ended("q2"), ...run("u8", "x1", "sleep 100", 100)].map((entry) => JSON.stringify(entry)).join("\n") + "\n");
  assert.deepEqual(sidebar.backgroundWork(file, now - 60_000), { count: 1, monitors: [] });

  const tab = { id: "@9", index: 9, name: "review", state: "background", background: 2, loop: { kind: "review", round: 2 }, session: "$1", panes: [], sidebars: [] };
  assert.equal(sidebar.stateWords({ ...tab, loop: { kind: "ci", round: 3 } }), "CI loop · round 3");
  assert.equal(sidebar.stateWords({ ...tab, loop: { kind: "queue", round: 1 } }), "queue-and-clean · watching");
  assert.equal(sidebar.stateWords({ ...tab, loop: { kind: "queue", round: 1 }, phase: "waiting for the landing lock" }), "queue-and-clean · waiting for the landing lock", "a landing's step in place of watching");
  assert.equal(sidebar.stateWords({ ...tab, state: "working", since: undefined, loop: undefined, loopSkill: "queue", phase: "testing", activity: "Run the tests" }), "working · queue-and-clean · testing · Run the tests", "and while Claude works in the foreground");
  assert.equal(sidebar.stateWords(tab), "review loop · round 2");
  const { lines } = sidebar.sidebarLines({ tabs: [tab], sections: [], cols: 40, rows: 12, view: { focused: false, cursor: "@9", frame: 0, here: "@9" } });
  assert.match(lines.join("\n"), /\x1b\[48;5;97m/, "the review loop's own badge color");
});

test("mission control packs its stages side by side, each block under its own header", () => {
  const base = { session: "$1", panes: [], sidebars: [], command: "zsh", claude: { sessionId: "s" } };
  const tab = (index, name, extra) => ({ ...base, id: `@${index}`, index, name, state: "idle", ...extra });
  const tabs = [
    tab(1, "plain", {}),
    tab(2, "queued b", { merge: "queued", position: 2 }),
    tab(3, "reviewing", { state: "background", background: 2, loop: { kind: "review", round: 2 } }),
    tab(4, "busy", { state: "working" }),
    tab(5, "queued a", { merge: "queued", position: 1 }),
    tab(6, "asking", { state: "waiting" }),
    tab(7, "ci fixing", { state: "background", background: 1, loop: { kind: "ci", round: 1 } }),
    tab(8, "landed", { merge: "merged" }),
    tab(9, "watching the queue", { state: "background", background: 1, loop: { kind: "queue", round: 1 }, merge: "queued", position: 3 }),
  ];
  // Room for three cards a shelf, each box with its two columns of border:
  // stages pack side by side, one box each, and a stage that doesn't fit in
  // the room left starts the next shelf.
  const { order, shelves } = sidebar.missionGrid(tabs, 3 * 22, 20);
  assert.deepEqual(shelves.map((sh) => sh.blocks.map((b) => [b.stage.title, b.cards.map((i) => order[i].name)])), [
    [["Needs you", ["asking"]], ["Working", ["busy"]], ["Review loop", ["reviewing"]]],
    [["CI loop", ["ci fixing"]]],
    [["In merge queue", ["queued a", "queued b", "watching the queue"]]],
    [["Merged", ["landed"]], ["Idle", ["plain"]]],
  ], "a loop is its own stage, but a PR's place in the queue or merged comes first; the queue in queue order");
  // A stage too big for a row is one box of several rows, not two boxes.
  const many = Array.from({ length: 5 }, (_, i) => tab(20 + i, `idle ${i}`, {}));
  const big = sidebar.missionGrid(many, 3 * 22, 20);
  assert.deepEqual(big.shelves.map((sh) => sh.blocks.map((b) => [b.stage.title, b.across, b.rows])), [[["Idle", 3, 2]]]);
  assert.deepEqual(big.rows.map((r) => r.cards.length), [3, 2], "its rows are the rows the keys move through");
  const text = sidebar.missionLines({ tabs, sections: [], cols: 3 * config.cardWidth, rows: 200, view: { view: 0, cursor: 0, frame: 0 }, config })
    .map((line) => line.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "").trimEnd());
  // Each block boxed, its title on its top edge.
  const headers = text.filter((line) => line.startsWith("╭─ ")).map((line) => [...line.matchAll(/╭─ ([A-Z -]+ \d+(?: …)?) /g)].map((m) => m[1]));
  const tops = text.filter((line) => line.startsWith("╭─ ")).length;
  assert.equal(text.filter((line) => /^╰─+╯/.test(line)).length, tops, "every box closed");
  assert.deepEqual(headers, [
    ["NEEDS YOU 1", "WORKING 1", "REVIEW LOOP 1"],
    ["CI LOOP 1"],
    ["IN MERGE QUEUE 3"],
    ["MERGED 1", "IDLE 1"],
], "each stage boxed once, its title on its top edge");
  assert.match(text[0], /⋯ 4/, "the header counts the loops as working");
});

test("mission control's cards are as tall as their fullest one needs, emoji count as terminals draw them, and links show as their names", () => {
  const tabs = [1, 2].map((index) => ({ session: "$1", panes: [], sidebars: [], command: "zsh", id: `@${index}`, index, name: `quiet ${index}`, state: "idle", path: "/work" }));
  const mission = sidebar.missionLines({ tabs, sections: [], cols: 3 * config.cardWidth, rows: 60, view: { view: 0, cursor: 0, frame: 0 }, config });
  mission.forEach((line) => assert.equal(sidebar.width(line), 3 * config.cardWidth));
  const text = mission.map((line) => line.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "").trimEnd());
  assert.equal(text.findIndex((line) => /^╰─+╯/.test(line)), 8, "a shelf of quiet tabs is as short as a card can be, not stretched down the screen");
  assert.ok(text.slice(9, -1).every((line) => line === ""));
  assert.equal(sidebar.width("✅ done ⚠️"), 10, "symbols drawn as emoji take two columns, with or without their selector");
  assert.equal(
    sidebar.unlinked("Closed [ABC-2204](https://linear.app/acme/issue/ABC-2204/old) after [#4321](https://github.com/acme/app/pull/4321), see <https://acme.slack.com/archives/D0/p1> and https://linear.app/acme/issue/abc-7/x."),
    "Closed ABC-2204 after #4321, see acme.slack.com and ABC-7.",
  );
});

test("an idle Claude tab says only how long, and a PR its name already shows isn't said again", () => {
  const tab = { id: "@9", index: 9, session: "$1", panes: [], sidebars: [], name: "Fix #12 regressions", pr: "12", state: "idle", claude: { sessionId: "s" }, since: Math.floor(Date.now() / 1000) - 300 };
  assert.equal(sidebar.stateWords(tab), "idle for 5m");
  const { lines } = sidebar.sidebarLines({ tabs: [tab], sections: [], cols: 40, rows: 10, view: { focused: false, cursor: tab.id, frame: 0, here: tab.id } });
  assert.equal(lines.join("\n").replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "").match(/#12/g).length, 1);
});

test("a tab says what Claude is doing and what it waits on, as its hooks recorded them", async () => {
  const window = tmux("new-window", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  const pane = tmux("display", "-p", "-t", window, "#{pane_id}");
  const since = String(Math.floor(Date.now() / 1000) - 120);
  try {
    tmux("set", "-p", "-t", pane, "@agent_state", "working");
    tmux("set", "-p", "-t", pane, "@agent_since", since);
    tmux("set", "-p", "-t", pane, "@agent_activity", "Run the full test suite");
    tmux("setw", "-t", window, "@agent", "working");
    let tab = sidebar.tabsFrom(await sidebar.snapshot()).find((x) => x.id === window);
    assert.equal(tab.activity, "Run the full test suite");
    assert.equal(sidebar.stateWords(tab), "working 2m · Run the full test suite");

    // In a loop asked for earlier, Claude working between its rounds is the loop's.
    tmux("set", "-p", "-t", pane, "@agent_loop", "review");
    tab = sidebar.tabsFrom(await sidebar.snapshot()).find((x) => x.id === window);
    assert.equal(sidebar.stateWords(tab), "working 2m · review loop · Run the full test suite");
    assert.equal(sidebar.missionGrid([tab]).order.length, 1);
    assert.deepEqual(sidebar.missionGrid([tab], 100, 20).shelves[0].blocks.map((b) => b.stage.title), ["Review loop"]);
    tmux("set", "-pu", "-t", pane, "@agent_loop");

    tmux("set", "-p", "-t", pane, "@agent_state", "waiting");
    tmux("set", "-p", "-t", pane, "@agent_wait", "permission: Bash git push");
    tmux("setw", "-t", window, "@agent", "waiting");
    tab = sidebar.tabsFrom(await sidebar.snapshot()).find((x) => x.id === window);
    assert.equal(sidebar.stateWords(tab), "waiting on you 2m · permission: Bash git push");
  } finally {
    tmux("kill-window", "-t", window);
  }
});

test("pinned tabs come first in Needs you, ahead of ones waiting longer", () => {
  const base = { session: "$1", sessionName: "test", panes: [], sidebars: [], claude: { sessionId: "s" } };
  const tabs = [
    { ...base, id: "@1", index: 1, name: "waiting long", state: "waiting", since: 10 },
    { ...base, id: "@2", index: 2, name: "pinned", state: "idle", pinned: true },
    { ...base, id: "@3", index: 3, name: "done", state: "done", since: 20 },
  ];
  assert.deepEqual(sidebar.grouped(tabs)[0].tabs.map((x) => x.name), ["pinned", "waiting long", "done"]);
  assert.deepEqual(sidebar.staged(tabs)[0].tabs.map((x) => x.name), ["pinned", "waiting long", "done"]);
});

test("a tab pinned by hand needs you, whatever Claude is doing in it, until it is unpinned", async () => {
  const window = tmux("new-window", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  try {
    tmux("setw", "-t", window, "@pinned", "1");
    let tab = sidebar.tabsFrom(await sidebar.snapshot()).find((x) => x.id === window);
    assert.equal(tab.pinned, true);
    assert.deepEqual(sidebar.grouped([tab]).map((g) => g.title), ["Needs you"]);
    assert.deepEqual(sidebar.missionGrid([tab], 100, 20).shelves[0].blocks.map((b) => b.stage.title), ["Needs you"]);
    assert.match(sidebar.stateWords(tab), /^pinned · /);
    assert.match(sidebar.sidebarLines({ tabs: [tab], sections: [], cols: 40, rows: 10, view: { focused: false, cursor: window, frame: 0, here: window } }).lines.join(""), /📌/);
    // Waiting or finished, it keeps its pin, its words saying the rest.
    ["waiting", "done", "working"].forEach((state) => {
      const lines = sidebar.sidebarLines({ tabs: [{ ...tab, state }], sections: [], cols: 40, rows: 10, view: { focused: false, cursor: window, frame: 0, here: window } }).lines.join("");
      assert.match(lines, /📌/, `pinned while ${state}`);
    });
    tmux("setw", "-u", "-t", window, "@pinned");
    tab = sidebar.tabsFrom(await sidebar.snapshot()).find((x) => x.id === window);
    assert.deepEqual(sidebar.grouped([tab]).map((g) => g.title), ["Other"], "a shell, with no Claude");
  } finally {
    tmux("kill-window", "-t", window);
  }
});

test("M-e opens Notes to the right of the pane, moves the one Notes there from elsewhere, and puts it back in its own tab", () => {
  const script = path.join(__dirname, "../tmux/notes-open.sh");
  const env = { ...process.env, NOTES_OPEN_COMMAND: "cat" };
  const notes = () => tmux("list-panes", "-a", "-F", "#{pane_id} #{@notes_pane} #{window_id}").split("\n").filter((l) => l.split(" ")[1] === "1").map((l) => l.split(" "));
  const first = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  const second = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  const paneIn = (w) => tmux("display", "-p", "-t", w, "#{pane_id}");
  const a = paneIn(first);
  const b = paneIn(second);
  try {
    execFileSync("bash", [script, "side", "none", a], { env });
    assert.equal(notes().length, 1);
    assert.equal(notes()[0][2], first, "beside the pane, in its tab");
    const pane = notes()[0][0];

    execFileSync("bash", [script, "side", "none", b], { env });
    assert.deepEqual(notes(), [[pane, "1", second]], "the same Notes, moved");
    assert.equal(tmux("list-panes", "-t", first, "-F", "#{pane_id}"), a, "and gone from where it was");

    execFileSync("bash", [script, "side", "none", b], { env });
    const [, , own] = notes()[0];
    assert.notEqual(own, second, "pressed again, it goes back to a tab of its own");
    assert.equal(tmux("display", "-p", "-t", own, "#{@notes}"), "1");
    execFileSync("bash", [script, "side", "none", pane], { env });
    assert.equal(notes()[0][2], own, "and from there, alone, it stays put");
  } finally {
    [first, second, ...notes().map((n) => n[2])].forEach((w) => { try { tmux("kill-window", "-t", w); } catch {} });
  }
});

test("M-e shows each tab its own note: the one its Claude works from, else the last one opened beside it", async (t) => {
  const script = path.join(__dirname, "../tmux/notes-open.sh");
  const notesDir = fs.mkdtempSync(path.join(tmp, "tab-notes-"));
  ["A.md", "B.md", "C.md"].forEach((name) => fs.writeFileSync(path.join(notesDir, name), `# ${name}\n`));
  const meta = path.join(notesDir, ".notes.json");
  const showing = (name) => fs.writeFileSync(meta, JSON.stringify({ last: name }));
  // A headless Neovim on the socket stands in for the notes app, noting what it is asked to open.
  const socket = path.join(tmp, "notes.sock");
  const app = require("node:child_process").spawn("nvim", ["--headless", "-u", "NONE", "--listen", socket, "--cmd", "lua package.loaded.notes = { open = function(p) vim.g.opened = p end }; package.loaded['notes.store'] = { absolute = function(r) return r end }"], { stdio: "ignore" });
  t.after(() => app.kill());
  assert.ok(await until(() => fs.existsSync(socket)), "the stand-in listens");
  const opened = () => execFileSync("nvim", ["--server", socket, "--remote-expr", "get(g:, 'opened', '')"], { encoding: "utf8" }).trim();
  const env = { ...process.env, NOTES_OPEN_COMMAND: "cat", NOTES_DIR: notesDir, NOTES_SOCKET: socket };
  const first = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  const second = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  const paneIn = (w) => tmux("display", "-p", "-t", w, "#{pane_id}");
  const [a, b] = [paneIn(first), paneIn(second)];
  t.after(() => [first, second].forEach((w) => { try { tmux("kill-window", "-t", w); } catch {} }));

  showing("A.md");
  execFileSync("bash", [script, "side", "none", a], { env });
  execFileSync("bash", [script, "side", "none", b], { env });
  assert.equal(tmux("show", "-wqv", "-t", first, "@tab_note_last"), "", "a note only passing through, the last tab's, isn't taken as the tab's own");
  assert.equal(opened(), "", "a tab with no note of its own leaves the note as it is");

  execFileSync("bash", [script, "side", "none", a], { env });
  showing("B.md");
  execFileSync("bash", [script, "side", "none", b], { env });
  assert.equal(tmux("show", "-wqv", "-t", first, "@tab_note_last"), "B.md", "one opened beside it is kept on the tab it left");

  showing("A.md");
  execFileSync("bash", [script, "side", "none", a], { env });
  assert.equal(opened(), "B.md", "back beside the first tab, its note again");

  tmux("set", "-w", "-t", second, "@tab_note", "C.md");
  execFileSync("bash", [script, "side", "none", b], { env });
  assert.equal(opened(), "C.md", "the note its Claude works from comes first");
});

test("M-e puts the notes app's sidebar away beside another tab's pane and shows it again in Notes' own tab, unless Settings keeps it", async (t) => {
  const script = path.join(__dirname, "../tmux/notes-open.sh");
  // A headless Neovim on the socket stands in for the notes app, noting each time it is told to show or put away its sidebar.
  const socket = path.join(tmp, "sidebar-notes.sock");
  const app = require("node:child_process").spawn("nvim", ["--headless", "-u", "NONE", "--listen", socket, "--cmd", "lua vim.g.shown = {}; package.loaded.notes = { show_sidebar = function(s) vim.g.shown = vim.list_extend(vim.g.shown, { s }) end }"], { stdio: "ignore" });
  t.after(() => app.kill());
  assert.ok(await until(() => fs.existsSync(socket)), "the stand-in listens");
  const shown = () => JSON.parse(execFileSync("nvim", ["--server", socket, "--remote-expr", "json_encode(g:shown)"], { encoding: "utf8" }).trim());
  const env = { ...process.env, NOTES_OPEN_COMMAND: "cat", NOTES_DIR: tmp, NOTES_SOCKET: socket };
  const here = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  const pane = tmux("display", "-p", "-t", here, "#{pane_id}");
  const own = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}", "cat");
  const notes = tmux("display", "-p", "-t", own, "#{pane_id}");
  tmux("set", "-p", "-t", notes, "@notes_pane", "1");
  tmux("set", "-w", "-t", own, "@notes", "1");
  t.after(() => {
    tmux("set", "-gu", "@setting_notes_hide_sidebar");
    [here, own, tmux("display", "-p", "-t", notes, "#{window_id}")].forEach((w) => { try { tmux("kill-window", "-t", w); } catch {} });
  });

  execFileSync("bash", [script, "side", "none", pane], { env });
  assert.deepEqual(shown(), [false], "beside another tab's pane, the sidebar goes");
  execFileSync("bash", [script, "away", "none", notes], { env });
  assert.equal(tmux("display", "-p", "-t", notes, "#{window_id}"), own, "back in its own tab");
  assert.deepEqual(shown(), [false, true], "where it comes back");

  tmux("set", "-g", "@setting_notes_hide_sidebar", "off");
  execFileSync("bash", [script, "side", "none", pane], { env });
  assert.deepEqual(shown(), [false, true, true], "with the setting off, it stays");
});

test("Notes left alone beside a tab whose last pane closed goes back to its own tab, and its own tab keeps it", (t) => {
  const script = path.join(__dirname, "../tmux/notes-open.sh");
  const env = { ...process.env, NOTES_OPEN_COMMAND: "cat", NOTES_SOCKET: path.join(tmp, "no-notes.sock") };
  const here = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  const work = tmux("display", "-p", "-t", here, "#{pane_id}");
  // The tab's sidebar, which the daemon closes the tab with once it is all that is left.
  tmux("set", "-p", "-t", tmux("split-window", "-d", "-t", work, "-P", "-F", "#{pane_id}", "cat"), "@sidebar", "1");
  execFileSync("bash", [script, "side", "none", work], { env });
  const notes = tmux("list-panes", "-t", here, "-F", "#{pane_id} #{@notes_pane}").split("\n").find((l) => l.endsWith(" 1")).split(" ")[0];
  t.after(() => [here, tmux("display", "-p", "-t", notes, "#{window_id}")].forEach((w) => { try { tmux("kill-window", "-t", w); } catch {} }));

  execFileSync("bash", [script, "orphan"], { env });
  assert.equal(tmux("display", "-p", "-t", notes, "#{window_id}"), here, "with its work there still open, it stays");
  tmux("kill-pane", "-t", work);
  execFileSync("bash", [script, "orphan"], { env });
  const own = tmux("display", "-p", "-t", notes, "#{window_id}");
  assert.notEqual(own, here, "its work gone, it leaves the tab");
  assert.equal(tmux("display", "-p", "-t", own, "#{@notes}"), "1", "for a Notes tab of its own");
  execFileSync("bash", [script, "orphan"], { env });
  assert.equal(tmux("display", "-p", "-t", notes, "#{window_id}"), own, "where it stays");
  tmux("set", "-wu", "-t", own, "@notes");
  execFileSync("bash", [script, "orphan"], { env });
  assert.equal(tmux("display", "-p", "-t", notes, "#{window_id}"), own, "even with its mark lost, as tmux-resurrect loses it");
});

test("an option taken from a menu shows a box with a spinner until it is done, then what it said", async () => {
  const sent = [];
  const client = { kind: "mission", socket: { write: (text) => sent.push(text), end() {} }, view: { view: 0, cursor: 0 }, cols: 100, rows: 30, tmuxClient: "none" };
  let finish;
  client.choose = { title: "A message", choices: [{ key: "o", label: "Open it in Slack", act: () => new Promise((resolve) => { finish = resolve; }) }], cursor: 0 };
  const taken = sidebar.missionKey(client, { key: "Enter" });
  assert.deepEqual(client.busy, { text: "Open it in Slack…" });
  assert.match(plain(sent.at(-1)), /╭─ One moment .*[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏] Open it in Slack….*it closes itself when done/s);
  client.view.view = 0;
  await sidebar.missionKey(client, { key: "]" });
  await sidebar.missionKey(client, { mouse: { button: 0, x: 1, y: 1, press: true } });
  assert.equal(client.view.view, 0, "keys and clicks wait while it works");
  finish("Opened.");
  await taken;
  assert.equal(client.busy, undefined, "gone once done");
  assert.equal(client.note, "Opened.");
  assert.doesNotMatch(plain(sent.at(-1)), /One moment/);
});

test("a session's sidebars aren't drawn while mission control is open over it, and catch up when it closes", async (t) => {
  // A client of the tests' server, for mission control to be open on: a pane
  // of a second throwaway server is its terminal.
  const outer = path.join(tmp, "outer.sock");
  execFileSync("tmux", ["-S", outer, "-f", "/dev/null", "new-session", "-d", "-x", "120", "-y", "30", `env -u TMUX tmux -S ${socket} attach -t test`]);
  const window = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  const side = tmux("split-window", "-hb", "-d", "-t", window, "-P", "-F", "#{pane_id}");
  tmux("set", "-p", "-t", side, "@sidebar", "1");
  t.after(() => {
    tmux("kill-window", "-t", window);
    try {
      execFileSync("tmux", ["-S", outer, "kill-server"]);
    } catch {}
  });
  let tmuxClient;
  for (let i = 0; i < 50 && !tmuxClient; i++) {
    await wait(100);
    tmuxClient = tmux("list-clients", "-F", "#{client_name}").split("\n").find(Boolean);
  }
  await sidebar.refresh();
  const written = [];
  const client = { kind: "sidebar", pane: side, socket: { write: (text) => written.push(text), end() {} } };
  sidebar.state.clients.add(client);
  t.after(() => sidebar.state.clients.delete(client));
  sidebar.draw(client);
  const drawn = written.length;
  assert.ok(drawn > 0, "drawn while nothing covers it");

  const mission = new (require("node:events").EventEmitter)();
  Object.assign(mission, { setEncoding() {}, write() {}, end() {} });
  sidebar.accept(mission);
  mission.emit("data", `mission ${tmuxClient} 120 30\n`);
  await wait(100);
  client.lastLines = undefined;
  sidebar.draw(client);
  assert.equal(written.length, drawn, "not drawn under mission control");

  mission.emit("close");
  assert.ok(written.length > drawn, "drawn again once it closes");
});

test("a Claude started in a tab already running one moves to a tab of its own", async (t) => {
  // Node in the foreground with a session file, as claude-pane.sh tells
  // Claude started by claude-here.
  const claude = `${process.execPath} -e "setTimeout(() => {}, 60000)"`;
  fs.mkdirSync(path.join(home, ".claude/sessions"), { recursive: true });
  const window = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}", claude);
  const first = tmux("display", "-p", "-t", window, "#{pane_id}");
  const second = tmux("split-window", "-d", "-t", first, "-P", "-F", "#{pane_id}", claude);
  t.after(() => [window, tmux("display", "-p", "-t", second, "#{window_id}")].forEach((w) => { try { tmux("kill-window", "-t", w); } catch {} }));
  await wait(300);
  [first, second].forEach((pane) => {
    const pid = tmux("display", "-p", "-t", pane, "#{pane_pid}");
    fs.writeFileSync(path.join(home, ".claude/sessions", `${pid}.json`), JSON.stringify({ pid: Number(pid), sessionId: pane }));
  });
  const script = path.join(__dirname, "../tmux/one-claude.sh");
  execFileSync("bash", [script, second]);
  assert.notEqual(tmux("display", "-p", "-t", second, "#{window_id}"), window, "the second one has a tab of its own");
  assert.equal(tmux("display", "-p", "-t", first, "#{window_id}"), window, "the first stays");
  execFileSync("bash", [script, first]);
  assert.equal(tmux("display", "-p", "-t", first, "#{window_id}"), window, "alone in its tab, it stays");
  // A pane that isn't Claude, as Notes beside it, named by a claude -p that
  // inherited its TMUX_PANE.
  const notes = tmux("split-window", "-d", "-t", first, "-P", "-F", "#{pane_id}");
  execFileSync("bash", [script, notes]);
  assert.equal(tmux("display", "-p", "-t", notes, "#{window_id}"), window, "a pane that isn't Claude stays");
});

test("a Claude starting alone in a tab moves nothing, even while the tab in front holds two", async (t) => {
  const claude = `${process.execPath} -e "setTimeout(() => {}, 60000)"`;
  fs.mkdirSync(path.join(home, ".claude/sessions"), { recursive: true });
  // agent-state.sh runs the installed scripts, as setup.sh links them.
  const installed = path.join(home, ".config/tmux");
  fs.mkdirSync(installed, { recursive: true });
  fs.readdirSync(path.join(__dirname, "../tmux"))
    .filter((name) => name.endsWith(".sh") && !fs.existsSync(path.join(installed, name)))
    .forEach((name) => fs.symlinkSync(path.join(__dirname, "../tmux", name), path.join(installed, name)));
  // In front: a Claude with another beside it, as Notes sits beside one.
  const front = tmux("new-window", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}", claude);
  const other = tmux("display", "-p", "-t", front, "#{pane_id}");
  const active = tmux("split-window", "-t", other, "-P", "-F", "#{pane_id}", claude);
  const alone = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}", claude);
  const started = tmux("display", "-p", "-t", alone, "#{pane_id}");
  t.after(() => [front, alone, tmux("display", "-p", "-t", active, "#{window_id}")].forEach((w) => { try { tmux("kill-window", "-t", w); } catch {} }));
  await wait(300);
  [other, active, started].forEach((pane) => {
    const pid = tmux("display", "-p", "-t", pane, "#{pane_pid}");
    fs.writeFileSync(path.join(home, ".claude/sessions", `${pid}.json`), JSON.stringify({ pid: Number(pid), sessionId: pane }));
  });

  execFileSync("bash", [path.join(__dirname, "../tmux/agent-state.sh"), "set", "idle"], { env: { ...process.env, TMUX_PANE: started }, input: JSON.stringify({ hook_event_name: "SessionStart", session_id: started }) });
  await wait(1000);

  assert.equal(tmux("display", "-p", "-t", active, "#{window_id}"), front, "the pane in front stays in its tab");
  assert.equal(tmux("display", "-p", "-t", started, "#{window_id}"), alone);
});

test("asked for a tab's note, the notes about its ticket come first, then those titled like the tab, the last one shown there, and its repository's", (t) => {
  const notesDir = fs.mkdtempSync(path.join(tmp, "pick-notes-"));
  const repo = path.join(tmp, "widgets");
  fs.mkdirSync(path.join(notesDir, ".trash"), { recursive: true });
  execFileSync("git", ["init", "-q", repo]);
  const write = (name, text) => fs.writeFileSync(path.join(notesDir, name), `# ${name}\n${text}\n`);
  write("Groceries.md", "milk");
  write("Widgets.md", "the widgets repo");
  write("Sprint.md", "- [ ] AB-12 filter the list");
  write("Last.md", "nothing related");
  write("Filter ideas.md", "nothing else");
  fs.writeFileSync(path.join(notesDir, ".trash/Old.md"), "AB-12");
  const window = tmux("new-window", "-d", "-t", "test", "-c", repo, "-P", "-F", "#{window_id}");
  t.after(() => tmux("kill-window", "-t", window));
  tmux("setw", "-t", window, "@tab_label", "AB-12: Filter the list");
  tmux("setw", "-t", window, "@tab_note_last", "Last.md");
  const rows = execFileSync("bash", [path.join(__dirname, "../tmux/note-pick.sh"), "list", window], { env: { ...process.env, NOTES_DIR: notesDir }, encoding: "utf8" }).trim().split("\n").map((line) => line.split("\t"));
  assert.deepEqual(rows.slice(0, 4).map(([title]) => title), ["Sprint", "Filter ideas", "Last", "Widgets"]);
  assert.deepEqual(rows.map(([, rel]) => rel).sort(), ["Filter ideas.md", "Groceries.md", "Last.md", "Sprint.md", "Widgets.md"], "every note once, none from the trash");
});

test("a tab with Notes beside its pane says so in the sidebar, and the Notes tab itself doesn't", async (t) => {
  const script = path.join(__dirname, "../tmux/notes-open.sh");
  const env = { ...process.env, NOTES_OPEN_COMMAND: "cat" };
  const here = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  execFileSync("bash", [script, "side", "none", tmux("display", "-p", "-t", here, "#{pane_id}")], { env });
  const notes = tmux("list-panes", "-t", here, "-F", "#{pane_id} #{@notes_pane}").split("\n").find((l) => l.endsWith(" 1")).split(" ")[0];
  t.after(() => [here, tmux("display", "-p", "-t", notes, "#{window_id}")].forEach((w) => { try { tmux("kill-window", "-t", w); } catch {} }));
  let tab = sidebar.tabsFrom(await sidebar.snapshot()).find((x) => x.id === here);
  assert.equal(tab.notesHere, true);
  const text = sidebar.sidebarLines({ tabs: [tab], sections: [], cols: 40, rows: 12, view: { focused: false, cursor: here, frame: 0, here } }).lines.join("").replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "");
  assert.match(text, /✎ Notes here/);
  execFileSync("bash", [script, "away", "none", notes], { env });
  const own = tmux("display", "-p", "-t", notes, "#{window_id}");
  const tabs = sidebar.tabsFrom(await sidebar.snapshot());
  assert.equal(tabs.find((x) => x.id === here).notesHere, false, "put away, it's no longer here");
  assert.equal(tabs.find((x) => x.id === own).notesHere, false, "and its own tab isn't marked");
});

test("M-w on Notes beside another pane puts it back in a tab of its own rather than closing it", (t) => {
  const script = path.join(__dirname, "../tmux/notes-open.sh");
  const env = { ...process.env, NOTES_OPEN_COMMAND: "cat" };
  const here = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  const own = tmux("display", "-p", "-t", here, "#{pane_id}");
  execFileSync("bash", [script, "side", "none", own], { env });
  const notes = tmux("list-panes", "-t", here, "-F", "#{pane_id} #{@notes_pane}").split("\n").find((l) => l.endsWith(" 1")).split(" ")[0];
  t.after(() => [here, tmux("display", "-p", "-t", notes, "#{window_id}")].forEach((w) => { try { tmux("kill-window", "-t", w); } catch {} }));
  execFileSync("bash", [script, "away", "none", notes], { env });
  assert.equal(tmux("list-panes", "-t", here, "-F", "#{pane_id}"), own, "gone from the tab");
  const tab = tmux("display", "-p", "-t", notes, "#{window_id}");
  assert.notEqual(tab, here);
  assert.equal(tmux("display", "-p", "-t", tab, "#{@notes}"), "1", "and in a Notes tab of its own, still running");
});

test("Notes taken from its own tab leaves the tab saying where it went, and comes back into it", async (t) => {
  const script = path.join(__dirname, "../tmux/notes-open.sh");
  const env = { ...process.env, NOTES_OPEN_COMMAND: "cat" };
  // Notes in its own tab, as M-N opens it.
  execFileSync("bash", [script, "none"], { env });
  const find = (option) => tmux("list-panes", "-a", "-F", `#{pane_id} #{window_id} #{${option}}`).split("\n").map((l) => l.split(" ")).find((p) => p[2] === "1");
  const [notes, notesTab] = find("@notes_pane");
  const here = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  t.after(() => [here, notesTab].forEach((w) => { try { tmux("kill-window", "-t", w); } catch {} }));

  execFileSync("bash", [script, "side", "none", tmux("display", "-p", "-t", here, "#{pane_id}")], { env });
  assert.equal(tmux("display", "-p", "-t", notes, "#{window_id}"), here, "Notes is beside your pane");
  const [standIn, standInTab] = find("@notes_placeholder");
  assert.equal(standInTab, notesTab, "and its tab is still there, with a pane in its place");
  assert.equal(tmux("display", "-p", "-t", notesTab, "#{@notes}"), "1");
  await wait(300);
  assert.match(tmux("capture-pane", "-p", "-t", standIn), /Notes is beside tab \d+/, "saying which tab has it");
  const tabs = sidebar.tabsFrom(await sidebar.snapshot());
  const beside = tabs.find((x) => x.id === here);
  assert.equal(sidebar.stateWords(tabs.find((x) => x.id === notesTab)), `Notes is beside tab ${beside.index} · ${beside.name}`, "and its line in the sidebar says so too");

  execFileSync("bash", [script, "away", "none", notes], { env });
  assert.equal(tmux("display", "-p", "-t", notes, "#{window_id}"), notesTab, "put away, it goes back into its tab");
  assert.equal(find("@notes_placeholder"), undefined, "in the stand-in's place");
});

test("M-e moves Notes, not the sidebar pane sitting first in the Notes tab", (t) => {
  const script = path.join(__dirname, "../tmux/notes-open.sh");
  const env = { ...process.env, NOTES_OPEN_COMMAND: "cat" };
  execFileSync("bash", [script, "none"], { env });
  const [notes, notesTab] = tmux("list-panes", "-a", "-F", "#{pane_id} #{window_id} #{@notes_pane}").split("\n").map((l) => l.split(" ")).find((p) => p[2] === "1");
  const sidebarPane = tmux("split-window", "-bdh", "-l", "10", "-t", notes, "-P", "-F", "#{pane_id}", "cat");
  tmux("set-option", "-p", "-t", sidebarPane, "@sidebar", "1");
  const here = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  t.after(() => [here, notesTab].forEach((w) => { try { tmux("kill-window", "-t", w); } catch {} }));
  execFileSync("bash", [script, "side", "none", tmux("display", "-p", "-t", here, "#{pane_id}")], { env });
  assert.equal(tmux("display", "-p", "-t", notes, "#{window_id}"), here, "Notes is beside your pane");
  assert.equal(tmux("display", "-p", "-t", sidebarPane, "#{window_id}"), notesTab, "and the sidebar stayed in the Notes tab");
});

test("M-e finds a Notes that lost its marks from the notes lock, rather than starting another", (t) => {
  const notesDir = fs.mkdtempSync(path.join(tmp, "notes-"));
  // A Notes tmux-resurrect brought back: no @notes or @notes_pane, but the
  // lock names its process.
  const restored = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}", "cat");
  const pane = tmux("display", "-p", "-t", restored, "#{pane_id}");
  fs.writeFileSync(path.join(notesDir, ".notes.lock"), tmux("display", "-p", "-t", pane, "#{pane_pid}"));
  const here = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  t.after(() => [restored, here].forEach((w) => { try { tmux("kill-window", "-t", w); } catch {} }));
  execFileSync("bash", [path.join(__dirname, "../tmux/notes-open.sh"), "side", "none", tmux("display", "-p", "-t", here, "#{pane_id}")], { env: { ...process.env, NOTES_DIR: notesDir, NOTES_OPEN_COMMAND: "false" } });
  assert.equal(tmux("display", "-p", "-t", pane, "#{window_id}"), here, "the same Notes, moved beside you");
  assert.equal(tmux("display", "-p", "-t", pane, "#{@notes_pane}"), "1", "and marked again");
  const standIn = tmux("list-panes", "-t", restored, "-F", "#{pane_id} #{@notes_placeholder}").split("\n").find((l) => l.endsWith(" 1"));
  assert.ok(standIn, "its tab, which had nothing else, keeps a pane saying where Notes went");
  assert.equal(tmux("display", "-p", "-t", restored, "#{@notes}"), "1", "and is the Notes tab again");
});

test("M-w on a tool in a pane's place closes the tool and brings the pane back, keeping the tab", () => {
  const window = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  const own = tmux("display", "-p", "-t", window, "#{pane_id}");
  // As pane-in-place.sh leaves it: the tool in the pane's place, the pane parked.
  const tool = tmux("new-window", "-d", "-t", "test", "-P", "-F", "#{pane_id}", "cat");
  tmux("set", "-w", "-t", tool, "@pane_parked", "1");
  tmux("set", "-p", "-t", tool, "@parked_pane", own);
  tmux("swap-pane", "-d", "-s", tool, "-t", own);
  tmux("select-pane", "-t", tool);
  try {
    // M-w names the pane in @confirm_pane, since its popup can't pass it.
    tmux("set", "-gF", "@confirm_pane", tool);
    execFileSync("bash", [path.join(__dirname, "../tmux/confirm.sh"), "Close pane?"], { input: "\n", env: { ...process.env, TERM: "xterm" }, stdio: ["pipe", "ignore", "ignore"] });
    const panes = tmux("list-panes", "-t", window, "-F", "#{pane_id}").split("\n");
    assert.deepEqual(panes, [own], "your pane is back in its tab, and the tool is gone");
  } finally {
    tmux("kill-window", "-t", window);
  }
});

test("a tool in a pane's place shows that pane beside it on the left, and alone again, from either pane", () => {
  const window = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  const own = tmux("display", "-p", "-t", window, "#{pane_id}");
  // As pane-in-place.sh open leaves it.
  const tool = tmux("new-window", "-d", "-t", "test", "-P", "-F", "#{pane_id}", "cat");
  tmux("set", "-w", "-t", tool, "@pane_parked", "1");
  tmux("set", "-p", "-t", tool, "@parked_pane", own);
  tmux("set", "-p", "-t", tool, "@in_place", "lazygit");
  tmux("swap-pane", "-d", "-s", tool, "-t", own);
  const script = path.join(__dirname, "../tmux/pane-in-place.sh");
  const layout = () => tmux("list-panes", "-t", window, "-F", "#{pane_id} #{pane_left} #{pane_width}").split("\n").map((l) => l.split(" "));
  try {
    execFileSync("bash", [script, "side", tool]);
    let panes = layout();
    assert.deepEqual(panes.map(([id]) => id), [own, tool], "your pane joins the tool's tab, on its left");
    assert.ok(Math.abs(Number(panes[0][2]) - Number(panes[1][2])) <= 2, "half the width each");
    execFileSync("bash", [script, "side", tool]);
    assert.equal(layout().length, 2, "asked again, nothing changes");

    execFileSync("bash", [script, "full", own]);
    assert.deepEqual(layout().map(([id]) => id), [tool], "from your pane too, the tool is alone again");
    assert.equal(tmux("display", "-p", "-t", own, "#{@pane_parked}"), "1", "your pane waits out of the tabs once more");

    // Side by side, quitting the tool brings your pane back whole.
    execFileSync("bash", [script, "side", tool]);
    execFileSync("bash", [script, "back", own], { env: { ...process.env, TMUX_PANE: tool } });
    tmux("kill-pane", "-t", tool);
    assert.deepEqual(layout().map(([id]) => id), [own]);
  } finally {
    tmux("kill-window", "-t", window);
  }
});

test("a window holding a pane parked behind lazygit is not a tab, and gets no sidebar", async () => {
  const window = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  try {
    tmux("setw", "-t", window, "@pane_parked", "1");
    const snap = await sidebar.snapshot();
    const tab = sidebar.tabsFrom(snap).find((x) => x.id === window);
    assert.equal(tab.parked, true);
    await sidebar.reconcile({ ...snap, on: true }, [tab], { ...config, minWindowWidth: 0 });
    assert.equal(tmux("list-panes", "-t", window, "-F", "#{pane_id}").split("\n").length, 1, "no sidebar added");
  } finally {
    tmux("kill-window", "-t", window);
  }
});

test("a waiting tab is answered from mission control: its options shown, a number pressed in it", async () => {
  // A tab showing a permission prompt, then echoing what is pressed in it.
  const script = path.join(tmp, "prompt.sh");
  fs.writeFileSync(script, "printf 'Do you want to proceed?\\n\\xe2\\x9d\\xaf 1. Yes\\n  2. No, and tell Claude what to do differently\\n'; exec cat\n");
  const window = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}", `sh ${script}`);
  const pane = tmux("display", "-p", "-t", window, "#{pane_id}");
  tmux("set", "-p", "-t", pane, "@agent_state", "waiting");
  tmux("set", "-p", "-t", pane, "@agent_since", String(Math.floor(Date.now() / 1000)));
  tmux("setw", "-t", window, "@agent", "waiting");
  const sent = [];
  const client = { kind: "mission", socket: { write: (text) => sent.push(text), end() {} }, view: { view: 0, cursor: 0 }, cols: 120, rows: 30, tmuxClient: "none" };
  try {
    await wait(300);
    await sidebar.refresh();
    const order = sidebar.missionGrid(sidebar.state.tabs).order;
    client.view.cursor = order.findIndex((t) => t.id === window);
    assert.ok(client.view.cursor >= 0, "the waiting tab is a card");

    // Enter's menu answers it, as the a key used to.
    await sidebar.missionKey(client, { key: "Enter" });
    client.choose.cursor = client.choose.choices.findIndex((c) => c.key === "a");
    await sidebar.missionKey(client, { key: "Enter" });
    assert.equal(client.answering.found.question, "Do you want to proceed?");
    assert.deepEqual(client.answering.found.options.map((o) => o.label), ["Yes", "No, and tell Claude what to do differently"]);
    // A box over the view, the tabs still in sight around it.
    assert.match(plain(sent.at(-1)), /MISSION CONTROL.*╭─ Answer \d+ .*Do you want to proceed\?.*1 +Yes.*2 +No/s);

    await sidebar.missionKey(client, { key: "2" });
    assert.ok(await until(() => tmux("capture-pane", "-p", "-t", pane).includes("2")), "2 was pressed in the tab");
    assert.equal(client.answering.note, "Chose 2. No, and tell Claude what to do differently");
  } finally {
    tmux("kill-window", "-t", window);
  }
});

test("/ in the sidebar narrows its tabs to those whose title has the text, and Esc brings them back", async (t) => {
  const one = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-n", "findme one", "-P", "-F", "#{window_id}");
  const two = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-n", "other two", "-P", "-F", "#{window_id}");
  t.after(() => [one, two].forEach((w) => tmux("kill-window", "-t", w)));
  tmux("select-window", "-t", one);
  assert.equal(sidebar.start(), true);
  t.after(() => sidebar.stop());
  const pane = await until(() => sidebarPane(one));
  assert.ok(pane, "no sidebar");
  const shown = () => tmux("capture-pane", "-p", "-t", pane);
  assert.ok(await until(() => shown().includes("findme one") && shown().includes("other two")));
  tmux("select-pane", "-t", pane);
  await wait(100);
  await sidebar.refresh();
  tmux("send-keys", "-t", pane, "/");
  tmux("send-keys", "-t", pane, "-l", "findme");
  assert.ok(await until(() => shown().includes("/findme") && !shown().includes("other two")), "only the tab whose title matches");
  assert.ok(shown().includes("findme one"));
  tmux("send-keys", "-t", pane, "Escape");
  assert.ok(await until(() => shown().includes("other two") && !shown().includes("/findme")), "Esc brings them back");
});

test("mission control takes the mouse: a view's name switches to it, a card is chosen and then gone to, the wheel moves", async (t) => {
  const one = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-n", "mouse one", "-P", "-F", "#{window_id}");
  const two = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-n", "mouse two", "-P", "-F", "#{window_id}");
  t.after(() => [one, two].forEach((w) => tmux("kill-window", "-t", w)));
  await sidebar.refresh();
  const client = { kind: "mission", socket: { write() {}, end() {} }, view: { view: 0, cursor: 0 }, cols: 160, rows: 60, tmuxClient: "none" };
  sidebar.draw(client);
  const order = sidebar.missionGrid(sidebar.state.tabs).order;
  const target = order.findIndex((x) => x.id === two);
  const card = client.hits.find((h) => h.index === target);
  assert.ok(card, "the card has somewhere to click");
  const click = (h) => sidebar.missionKey(client, { mouse: { button: 0, x: h.x0 + 1, y: h.y, press: true } });
  await click(card);
  assert.equal(client.view.cursor, target, "a click chooses it");
  await sidebar.missionKey(client, { mouse: { button: target ? 64 : 65, x: 1, y: 3, press: true } });
  assert.notEqual(client.view.cursor, target, "the wheel moves to the next card");
  const viewName = client.hits.find((h) => h.view === 1);
  await click(viewName);
  assert.equal(client.view.view, 1, "a view's name switches to it");
});

test("the sidebar's Space opens a popup with only that tab's peek, closed with it", async (t) => {
  const window = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}", "sh -c 'seq 1 60; exec cat'");
  t.after(() => tmux("kill-window", "-t", window));
  assert.equal(sidebar.start(), true);
  t.after(() => sidebar.stop());
  await wait(300);
  await sidebar.refresh();

  const popup = net.connect(sidebar.SOCKET);
  let received = "";
  let closed = false;
  popup.on("data", (data) => (received += data));
  popup.on("close", () => (closed = true));
  popup.write(`mission none 100 20 peek:${window}\n`);
  assert.ok(await until(() => /Peek.*60/s.test(plain(received))), "the peek, at once");
  popup.write("q");
  assert.equal(await until(() => closed), true, "closing the peek closes the popup");

  // A tab that has gone closes it straight away.
  const gone = net.connect(sidebar.SOCKET);
  let goneClosed = false;
  gone.on("close", () => (goneClosed = true));
  gone.write("mission none 100 20 answer:@999999\n");
  assert.equal(await until(() => goneClosed), true);
});

test("Space peeks at a tab from mission control: its screen, or its whole last turn, without going there", async () => {
  const window = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}", "sh -c 'seq 1 60; exec cat'");
  const sent = [];
  const client = { kind: "mission", socket: { write: (text) => sent.push(text), end() {} }, view: { view: 0, cursor: 0 }, cols: 100, rows: 20, tmuxClient: "none" };
  try {
    await wait(300);
    await sidebar.refresh();
    client.view.cursor = sidebar.missionGrid(sidebar.state.tabs).order.findIndex((t) => t.id === window);
    // No Claude there: its screen, scrolled to the end, and up with k, from Enter's menu.
    await sidebar.missionKey(client, { key: "Enter" });
    client.choose.cursor = client.choose.choices.findIndex((c) => c.key === "v");
    await sidebar.missionKey(client, { key: "Enter" });
    assert.equal(client.peek.mode, "screen");
    assert.match(screen(sent), /Peek.*60/s);
    await sidebar.missionKey(client, { key: "g" });
    assert.match(screen(sent), /Peek +\d+ [\s\S]*│ 1 +│[\s\S]*│ 2 +│[\s\S]*│ 3 +│/, "g goes to the top");
    await sidebar.missionKey(client, { key: "Escape" });
    assert.equal(client.peek, undefined);
    assert.equal(tmux("display", "-p", "#{window_id}") === window, false, "never went there");
  } finally {
    tmux("kill-window", "-t", window);
  }

  // A Claude tab: its whole last turn, however long.
  const file = path.join(home, ".claude/projects/-work/peeked.jsonl");
  const reply = Array.from({ length: 30 }, (_, i) => `line ${i + 1} of the reply`).join("\n");
  fs.writeFileSync(file, [
    JSON.stringify({ type: "user", message: { content: "explain the change" } }),
    JSON.stringify({ type: "assistant", message: { content: [{ type: "text", text: reply }] } }),
  ].join("\n") + "\n");
  const tab = { id: "@99", index: 9, name: "peeked", state: "done", claude: { sessionId: "peeked", cwd: "/work" }, panes: [], sidebars: [] };
  const top = sidebar.peekLines({ tab, mode: "turn", scroll: 0 }, 80, 40).lines.map(plain).join("\n");
  assert.match(top, /› explain the change[\s\S]*line 1 of the reply[\s\S]*line 30 of the reply/, "the reply in full, not cut to a card");
});

test("/ filters mission control's cards, and X closes every merged tab and removes its worktree", async () => {
  // A repository on "GitHub", with a worktree whose PR has merged.
  const repo = path.join(tmp, "merged-repo");
  const git = (...args) => execFileSync("git", ["-C", repo, "-c", "user.name=Test", "-c", "user.email=test@example.com", ...args], { stdio: "ignore" });
  fs.mkdirSync(repo, { recursive: true });
  git("init", "-q", "-b", "main");
  git("commit", "-q", "--allow-empty", "-m", "start");
  git("remote", "add", "origin", "git@github.com:acme/merging.git");
  const worktree = path.join(repo, ".claude/worktrees/feat-landed");
  git("worktree", "add", "-q", "-b", "feat-landed", worktree);
  sidebar.known.set("acme/merging#31", { merge: "merged" });
  const landed = tmux("new-window", "-d", "-t", "test", "-c", worktree, "-P", "-F", "#{window_id}");
  tmux("setw", "-t", landed, "@tab_label", "#31 feat landed");
  const other = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  tmux("setw", "-t", other, "@tab_label", "something else");
  const sent = [];
  const client = { kind: "mission", socket: { write: (text) => sent.push(text), end() {} }, view: { view: 0, cursor: 0 }, cols: 180, rows: 30, tmuxClient: "none" };
  try {
    await sidebar.refresh();
    assert.equal(sidebar.state.tabs.find((x) => x.id === landed).merge, "merged");

    // / then "landed": only that card is left; Esc puts them all back.
    for (const key of ["/", "l", "a", "n", "d", "e", "d", "Enter"]) await sidebar.missionKey(client, { key });
    assert.match(screen(sent), /\/landed/);
    assert.match(screen(sent), /feat landed/);
    assert.doesNotMatch(screen(sent), /something else/);
    await sidebar.missionKey(client, { key: "Escape" });
    assert.match(screen(sent), /something else/);

    // From Enter's menu on the merged tab: X asks first; n leaves everything as it was.
    client.view.cursor = sidebar.missionGrid(sidebar.state.tabs).order.findIndex((t) => t.id === landed);
    await sidebar.missionKey(client, { key: "Enter" });
    client.choose.cursor = client.choose.choices.findIndex((c) => c.key === "X");
    await sidebar.missionKey(client, { key: "Enter" });
    assert.match(screen(sent), /Close 1 merged tab and remove their worktrees\? \[Y\/n\][\s\S]*feat landed/);
    await sidebar.missionKey(client, { key: "n" });
    assert.ok(fs.existsSync(worktree));
    // y closes the tab and removes its worktree, and says so.
    await sidebar.missionKey(client, { key: "Enter" });
    client.choose.cursor = client.choose.choices.findIndex((c) => c.key === "X");
    await sidebar.missionKey(client, { key: "Enter" });
    await sidebar.missionKey(client, { key: "y" });
    assert.ok(!tmux("list-windows", "-a", "-F", "#{window_id}").split("\n").includes(landed), "the tab is closed");
    assert.ok(!fs.existsSync(worktree), "its worktree is removed");
    assert.match(plain(sent.at(-1)), /Closed 1 merged tab\./);
  } finally {
    sidebar.known.delete("acme/merging#31");
    tmux("kill-window", "-t", other);
  }
});

test("a tab in a removed worktree takes its branch's PR, unless its name is about another ticket", () => {
  const repo = path.join(tmp, "gone-repo");
  fs.mkdirSync(repo, { recursive: true });
  execFileSync("git", ["-C", repo, "init", "-q", "-b", "main"]);
  execFileSync("git", ["-C", repo, "remote", "add", "origin", "git@github.com:acme/gone.git"]);
  const gone = path.join(repo, ".claude/worktrees/abc-5-old-work");
  sidebar.known.set("acme/gone@abc-5-old-work", { pr: "50", merge: "merged" });
  try {
    assert.equal(sidebar.mergeOf({ path: gone, name: "ABC-5: Old work" }).merge, "merged", "its own ticket");
    assert.equal(sidebar.mergeOf({ path: gone, name: "a plain name" }).merge, "merged", "no ticket named");
    assert.deepEqual(sidebar.mergeOf({ path: gone, name: "Shaping ABC-9: New work" }), {}, "shaping another ticket now");
  } finally {
    sidebar.known.delete("acme/gone@abc-5-old-work");
  }
});

test("the work map lists every worktree in the tabs' repositories, with its tab or none, its ticket and its PR", async () => {
  const repo = path.join(tmp, "workmap-repo");
  const git = (...args) => execFileSync("git", ["-C", repo, "-c", "user.name=Test", "-c", "user.email=test@example.com", ...args], { stdio: "ignore" });
  fs.mkdirSync(repo, { recursive: true });
  git("init", "-q", "-b", "main");
  git("commit", "-q", "--allow-empty", "-m", "start");
  git("remote", "add", "origin", "git@github.com:acme/workmap.git");
  const open = path.join(repo, ".claude/worktrees/abc-12-feat-a");
  const forgotten = path.join(repo, ".claude/worktrees/abc-13-feat-b");
  git("worktree", "add", "-q", "-b", "abc-12-feat-a", open);
  git("worktree", "add", "-q", "-b", "abc-13-feat-b", forgotten);
  // Its PR, as GitHub would have said, looked up by its branch.
  sidebar.known.set("acme/workmap@abc-13-feat-b", { pr: "42", merge: "queued", position: 2 });
  const window = tmux("new-window", "-d", "-t", "test", "-c", open, "-P", "-F", "#{window_id}");
  tmux("setw", "-t", window, "@tab_label", "#41 ABC-12: Feat A");
  try {
    await sidebar.refresh();
    const tabs = sidebar.state.tabs;
    await sidebar.refreshWorktrees(tabs);
    assert.deepEqual(sidebar.untabbed(tabs).filter((w) => w.path.startsWith(repo)), [{ path: forgotten, branch: "abc-13-feat-b" }], "asked about by branch");
    const rows = sidebar.workMap(tabs).filter((r) => (r.tab?.path ?? r.dir)?.startsWith(repo));
    assert.deepEqual(rows.map((r) => [r.title, r.detail.replace(tmp, "TMP"), Boolean(r.tab), r.dir?.replace(tmp, "TMP")]), [
      ["ABC-12  Feat A", "tab " + tabs.find((x) => x.id === window).index + " · #41 open · TMP/workmap-repo/.claude/worktrees/abc-12-feat-a", true, undefined],
      ["ABC-13  feat b", "no tab · #42 in merge queue at 2 · TMP/workmap-repo/.claude/worktrees/abc-13-feat-b", false, "TMP/workmap-repo/.claude/worktrees/abc-13-feat-b"],
    ], "the main checkout, with no tab in it, isn't a row");
    assert.ok(sidebar.builtInViews(sidebar.state).some((v) => v.title === "Work map"));
  } finally {
    sidebar.known.delete("acme/workmap@abc-13-feat-b");
    tmux("kill-window", "-t", window);
  }
});

test("the work map adds wt-revive's worktrees with when and what you last asked there, and M-j and M-o open it as a picker", async (t) => {
  // wt-revive's rows: one in a repository no tab is in, one with a tab.
  sidebar.revived.set("/elsewhere/.claude/worktrees/abc-20-old", { tree: "/elsewhere/.claude/worktrees/abc-20-old", repo: "/elsewhere", branch: "abc-20-old", when: 100, age: "3d ago", prompt: "Fix the flaky test" });
  sidebar.revived.set("/elsewhere/.claude/worktrees/abc-21-older", { tree: "/elsewhere/.claude/worktrees/abc-21-older", repo: "/elsewhere", branch: "abc-21-older", when: 50, age: "6d ago", prompt: "(no recent Claude session)" });
  t.after(() => sidebar.revived.clear());
  const rows = sidebar.workMap([]).filter((r) => r.dir?.startsWith("/elsewhere"));
  assert.deepEqual(rows.map((r) => [r.title, r.detail]), [
    ["ABC-20  old", "no tab · no PR · used 3d ago · /elsewhere/.claude/worktrees/abc-20-old · Fix the flaky test"],
    ["ABC-21  older", "no tab · no PR · used 6d ago · /elsewhere/.claude/worktrees/abc-21-older · (no recent Claude session)"],
  ], "most recently used first");

  // M-j: the Work map, open tabs only, a filter typing; Esc closes it.
  const window = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  t.after(() => tmux("kill-window", "-t", window));
  const client = { kind: "mission", socket: { write() {}, end() { client.ended = true; } }, view: { view: 1, cursor: 0 }, cols: 120, rows: 30, tmuxClient: "none", scope: "tabs", picker: true, filter: { text: "", typing: true } };
  await sidebar.refresh();
  const workMap = sidebar.viewsFor(client).find((v) => v.title.startsWith("Work map"));
  assert.equal(workMap.title, "Work map · open tabs");
  assert.ok(workMap.items.every((item) => item.tab), "no worktree without a tab");
  client.scope = "untabbed";
  assert.deepEqual(sidebar.viewsFor(client).find((v) => v.title.startsWith("Work map")).items.filter((i) => i.dir?.startsWith("/elsewhere")).map((i) => i.title), ["ABC-20  old", "ABC-21  older"]);
  for (const key of ["o", "l", "d", "e"]) await sidebar.missionKey(client, { key });
  assert.deepEqual(sidebar.viewsFor(client).find((v) => v.title.startsWith("Work map")).items.map((i) => i.title), ["ABC-21  older"], "the filter matches title and detail");
  await sidebar.missionKey(client, { key: "Escape" });
  assert.equal(client.ended, true, "Esc closes the picker");
});

test("the work map lists every open tab, lines its rows up in columns, and keeps merged work under a header, removable from Enter's menu", async (t) => {
  const repo = path.join(tmp, "workmerged-repo");
  const git = (...args) => execFileSync("git", ["-C", repo, "-c", "user.name=Test", "-c", "user.email=test@example.com", ...args], { stdio: "ignore" });
  fs.mkdirSync(repo, { recursive: true });
  git("init", "-q", "-b", "main");
  git("commit", "-q", "--allow-empty", "-m", "start");
  git("remote", "add", "origin", "git@github.com:acme/workmerged.git");
  const working = path.join(repo, ".claude/worktrees/abc-30-going");
  const done = path.join(repo, ".claude/worktrees/abc-31-done");
  git("worktree", "add", "-q", "-b", "abc-30-going", working);
  git("worktree", "add", "-q", "-b", "abc-31-done", done);
  sidebar.known.set("acme/workmerged@abc-31-done", { pr: "51", merge: "merged" });
  const plainDir = path.join(tmp, "no-repo-here");
  fs.mkdirSync(plainDir, { recursive: true });
  // A tab in a subdirectory of its worktree, and one in no repository.
  fs.mkdirSync(path.join(working, "src"), { recursive: true });
  const inWork = tmux("new-window", "-d", "-t", "test", "-c", path.join(working, "src"), "-P", "-F", "#{window_id}");
  const loose = tmux("new-window", "-d", "-t", "test", "-c", plainDir, "-P", "-F", "#{window_id}");
  tmux("setw", "-t", inWork, "@tab_label", "ABC-30: Going");
  tmux("setw", "-t", loose, "@tab_label", "Slack relay");
  t.after(() => {
    sidebar.known.delete("acme/workmerged@abc-31-done");
    [inWork, loose].forEach((w) => { try { tmux("kill-window", "-t", w); } catch {} });
  });
  await sidebar.refresh();
  await sidebar.refreshWorktrees(sidebar.state.tabs);
  const rows = sidebar.workMap(sidebar.state.tabs).filter((r) => r.group || r.title === "Slack relay" || (r.tab?.path ?? r.dir)?.startsWith(repo));
  assert.deepEqual(rows.map((r) => [r.group ? "header" : r.title, r.merged]), [
    ["ABC-30  Going", false],
    ["Slack relay", false],
    ["header", undefined],
    ["ABC-31  done", true],
  ], "a tab in its worktree's subdirectory is that worktree's; one in no repository is a row of its own; merged work last, under its header");
  assert.deepEqual(rows[3].cells.map((c) => c.replace(tmp, "TMP")), ["ABC-31", "done", "no tab", "#51 merged", ""]);

  // M-j lists every open tab, the one in no repository too.
  const client = { kind: "mission", socket: { write() {}, end() {} }, view: { view: 1, cursor: 0 }, cols: 160, rows: 40, tmuxClient: "none", scope: "tabs" };
  assert.ok(sidebar.viewsFor(client)[0].items.some((i) => i.title === "Slack relay"));

  // The Work map itself: merged work hidden until Enter on its header.
  client.scope = undefined;
  const workMap = () => sidebar.viewsFor(client)[0].items;
  assert.ok(!workMap().some((i) => i.dir === done), "merged work starts hidden");
  client.view.cursor = workMap().findIndex((i) => i.group === "merged");
  assert.match(workMap()[client.view.cursor].title, /^▸ Merged, can be removed$/);
  await sidebar.missionKey(client, { key: "Enter" });
  assert.match(workMap()[client.view.cursor].title, /^▾ /);
  client.view.cursor = workMap().findIndex((i) => i.dir === done);
  assert.ok(client.view.cursor > 0, "and shown once opened");

  // Columns line up, and only the chosen row says where it is.
  const text = sidebar.missionLines({ tabs: sidebar.state.tabs, sections: sidebar.viewsFor(client), cols: 160, rows: 40, view: { ...client.view }, config }).map((line) => line.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, ""));
  const chosen = text.find((line) => line.includes("ABC-31"));
  const other = text.find((line) => line.includes("ABC-30"));
  assert.equal(chosen.indexOf("done"), other.indexOf("Going"), "titles start in the same column");
  assert.ok(chosen.includes(done.replace(home, "~")) && !other.includes(working.replace(home, "~")), "the directory on the chosen row only");

  // x in Enter's menu asks first, and git keeps a worktree with a file it doesn't track.
  fs.writeFileSync(path.join(done, "notes.txt"), "keep me");
  await sidebar.missionKey(client, { key: "Enter" });
  client.choose.cursor = client.choose.choices.findIndex((c) => c.key === "x");
  await sidebar.missionKey(client, { key: "Enter" });
  assert.match(client.confirm.text, /Remove this merged worktree\?/);
  await sidebar.missionKey(client, { key: "y" });
  assert.match(client.note, /^Kept abc-31-done: /);
  assert.ok(fs.existsSync(done));
  fs.rmSync(path.join(done, "notes.txt"));
  await sidebar.missionKey(client, { key: "Enter" });
  client.choose.cursor = client.choose.choices.findIndex((c) => c.key === "x");
  await sidebar.missionKey(client, { key: "Enter" });
  await sidebar.missionKey(client, { key: "n" });
  assert.ok(fs.existsSync(done), "n keeps it");
  await sidebar.missionKey(client, { key: "Enter" });
  client.choose.cursor = client.choose.choices.findIndex((c) => c.key === "x");
  await sidebar.missionKey(client, { key: "Enter" });
  await sidebar.missionKey(client, { key: "Enter" });
  assert.equal(client.note, "Removed abc-31-done.", "Enter confirms as y does");
  assert.ok(!fs.existsSync(done));
});

test("the Closed view lists closed tabs newest first, one per conversation (a shell, one per directory), to reopen with their Claude resumed", (t) => {
  fs.rmSync(path.join(sidebar.STATE_DIR, "closed.jsonl"), { force: true });
  sidebar.resetClosed();
  const realNow = Date.now;
  t.after(() => (Date.now = realNow));
  const claude = { id: "@80", index: 3, name: "ABC-1: Feat", state: "idle", path: "/work/abc-1", claude: { sessionId: "1234-abcd" } };
  const shell = { id: "@81", index: 4, name: "scratch", state: "idle", path: "/work/scratch" };
  const again = { id: "@82", index: 5, name: "ABC-1 older", state: "idle", path: "/work/abc-1", claude: { sessionId: "9999" } };
  const tool = { id: "@83", index: 6, name: "lazygit", state: "idle", path: "/work/tool" };
  // Seen a minute ago, so none closed within seconds of opening but the tool.
  Date.now = () => realNow() - 60_000;
  sidebar.record([]);
  sidebar.record([claude, shell, again]);
  Date.now = realNow;
  sidebar.record([claude, shell, again, tool]);
  sidebar.record([claude, shell]);
  sidebar.record([shell]);
  sidebar.record([]);
  const items = sidebar.closedItems([]);
  assert.deepEqual(items.map((i) => [i.title, i.detail, i.dir, i.command, i.shell]), [
    ["scratch", "closed <1m ago · a shell · /work/scratch", "/work/scratch", undefined, true],
    ["ABC-1: Feat", "closed <1m ago · resumes its Claude · /work/abc-1", "/work/abc-1", "zsh -i -c 'claude --resume 1234-abcd; exec zsh -i'", false],
    ["ABC-1 older", "closed <1m ago · resumes its Claude · /work/abc-1", "/work/abc-1", "zsh -i -c 'claude --resume 9999; exec zsh -i'", false],
  ], "each conversation, two in one directory included, and not the tool's window");
  assert.deepEqual(sidebar.closedItems([{ path: "/work/scratch" }]).map((i) => i.title), ["ABC-1: Feat", "ABC-1 older"], "not a shell's directory open again");
  assert.deepEqual(sidebar.closedItems([{ path: "/work/abc-1", claude: { sessionId: "9999" } }]).map((i) => i.title), ["scratch", "ABC-1: Feat"], "not a conversation open again, though a tab in its directory is no reason");
  // x removes one, older closings of the same conversation with it.
  sidebar.removeClosed(items[1].closed);
  assert.deepEqual(sidebar.closedItems([]).map((i) => i.title), ["scratch", "ABC-1 older"]);
  fs.rmSync(path.join(sidebar.STATE_DIR, "closed-removed.json"), { force: true });
  assert.ok(sidebar.builtInViews(sidebar.state).some((v) => v.title === "Closed"));

  // Its Claude quit, then its pane closed, leaving only the sidebar a moment
  // before the window went: it is still the conversation in its directory.
  const thread = { id: "@84", index: 7, name: "Slack: a thread", tabName: "Slack: a thread", state: "idle", path: "/work/thread", panes: [{}], claude: { sessionId: "5555" } };
  Date.now = () => realNow() - 60_000;
  sidebar.record([thread]);
  Date.now = realNow;
  sidebar.record([{ ...thread, claude: undefined }]);
  sidebar.record([{ ...thread, path: "/home", panes: [], claude: undefined }]);
  sidebar.record([]);
  assert.deepEqual(sidebar.closedItems([]).map((i) => [i.title, i.dir, i.command, i.tabName])[0], ["Slack: a thread", "/work/thread", "zsh -i -c 'claude --resume 5555; exec zsh -i'", "Slack: a thread"], "with the name it was given, to take back");
});

test("the PR board puts each PR in the column for where it stands, and h and l move between columns", async () => {
  const pr = (number, extra) => ({ number, title: `PR ${number}`, url: `https://github.com/acme/w/pull/${number}`, isDraft: false, state: "OPEN", reviewDecision: null, mergedAt: null, repository: { nameWithOwner: "acme/w" }, mergeQueueEntry: null, commits: { nodes: [{ commit: { statusCheckRollup: { state: "SUCCESS" } } }] }, ...extra });
  const columns = sidebar.boardFrom([
    pr(1, { isDraft: true }),
    pr(2, { commits: { nodes: [{ commit: { statusCheckRollup: { state: "FAILURE" } } }] } }),
    pr(3, { reviewDecision: "CHANGES_REQUESTED" }),
    pr(4, { reviewDecision: "APPROVED" }),
    pr(5, { reviewDecision: "APPROVED", mergeQueueEntry: { position: 2 } }),
    pr(6, { state: "MERGED", mergedAt: "2026-10-20T10:00:00Z" }),
    pr(7, { state: "MERGED", mergedAt: "2026-10-21T10:00:00Z" }),
    pr(2, {}),
  ]);
  assert.deepEqual(columns.map((c) => [c.title, c.items.map((i) => i.title)]), [
    ["Draft", ["#1 PR 1"]],
    ["In review", ["#2 PR 2", "#3 PR 3"]],
    ["Approved", ["#4 PR 4"]],
    ["In merge queue", ["#5 PR 5"]],
    ["Merged", ["#7 PR 7", "#6 PR 6"]],
  ], "a PR listed twice once, merged ones newest first");
  assert.deepEqual(columns[1].items.map((i) => [i.detail, i.state]), [["w · ✗ checks", "attention"], ["w · ✓ checks · changes requested", "attention"]]);

  sidebar.board.columns = columns;
  sidebar.board.loaded = true;
  const views = sidebar.builtInViews(sidebar.state);
  const at = views.findIndex((v) => v.board) + 1;
  const sent = [];
  const client = { kind: "mission", socket: { write: (text) => sent.push(text), end() {} }, view: { view: at, cursor: 0 }, cols: 150, rows: 20, tmuxClient: "none" };
  sidebar.draw(client);
  assert.match(plain(sent.at(-1)), /DRAFT 1 +IN REVIEW 2 +APPROVED 1 +IN MERGE QUEUE 1 +MERGED 2/);
  // j down a column, l across at the same row as far as it reaches, h back.
  await sidebar.missionKey(client, { key: "l" });
  await sidebar.missionKey(client, { key: "j" });
  assert.equal(views[at - 1].items[client.view.cursor].title, "#3 PR 3");
  await sidebar.missionKey(client, { key: "l" });
  assert.equal(views[at - 1].items[client.view.cursor].title, "#4 PR 4");
  await sidebar.missionKey(client, { key: "h" });
  assert.equal(views[at - 1].items[client.view.cursor].title, "#2 PR 2");
});

test("the Sessions view lists each Claude session by how full its context is, as the statusline recorded it", () => {
  // The statusline, given what Claude Code gives it, records the context and model.
  const statusline = path.join(__dirname, "../claude/statusline-command.sh");
  const input = JSON.stringify({ session_id: "sess-full", model: { display_name: "Opus" }, workspace: { current_dir: tmp }, context_window: { used_percentage: 86.4 } });
  execFileSync("bash", [statusline], { input, env: { ...process.env, HOME: home } });
  const recorded = path.join(home, ".local/state/tmux-sidebar/context/sess-full");
  assert.equal(fs.readFileSync(recorded, "utf8"), "86.4\tOpus");
  // The sidebar reads them from its own state directory.
  const contextDir = path.join(sidebar.STATE_DIR, "context");
  fs.mkdirSync(contextDir, { recursive: true });
  fs.copyFileSync(recorded, path.join(contextDir, "sess-full"));
  fs.writeFileSync(path.join(contextDir, "sess-light"), "12\tSonnet");
  const hour = Date.now() - 3_600_000;
  const tabs = [
    { id: "@81", index: 1, name: "light", state: "working", claude: { sessionId: "sess-light", cwd: "/work", startedAt: hour } },
    { id: "@82", index: 2, name: "full", state: "working", claude: { sessionId: "sess-full", cwd: "/work", startedAt: hour } },
    { id: "@83", index: 3, name: "no claude", state: "idle" },
  ];
  assert.deepEqual(sidebar.sessionItems(tabs).map((i) => [i.title, i.detail, i.state]), [
    ["2 full", "context 86% · Opus · up 1h", "attention"],
    ["1 light", "context 12% · Sonnet · up 1h", "active"],
  ], "fullest first, a full one needing you, a tab without Claude left out");
});

test("the Sessions view starts with today's spend and the rate limits, as the statusline recorded them", () => {
  // Five draws of one session, the third a dip within the same process and
  // the last two after restarts, the second costing more than the last draw,
  // and draws of two others.
  const statusline = path.join(__dirname, "../claude/statusline-command.sh");
  const now = Math.floor(Date.now() / 1000);
  const sessions = path.join(home, ".claude/sessions");
  fs.mkdirSync(sessions, { recursive: true });
  [
    ["spend-a", 1.5, undefined, 100],
    ["spend-a", 2.25, undefined, 100],
    ["spend-a", 1, undefined, 100],
    ["spend-a", 0.5, undefined, 200],
    ["spend-a", 3, undefined, 300],
    ["spend-c", 2.5],
    ["spend-d", 1],
    ["spend-d", 1.5],
    ["spend-b", 1, { five_hour: { used_percentage: 42.4, resets_at: now + 7_830 }, seven_day: { used_percentage: 85, resets_at: now + 3_030 } }],
  ].forEach(([session, cost, limits, pid]) => {
    // Each draw of spend-a runs in the process the one session file names.
    if (pid) {
      fs.readdirSync(sessions).forEach((f) => fs.rmSync(path.join(sessions, f)));
      fs.writeFileSync(path.join(sessions, `${pid}.json`), JSON.stringify({ pid, sessionId: "spend-a" }));
    }
    // A record from before the pid was kept, counted many times over, is cut
    // to the session's cost so far before this draw adds to it.
    if (session === "spend-c") fs.writeFileSync(path.join(home, ".local/state/tmux-sidebar/usage/spend-spend-c"), `${new Date().toLocaleDateString("sv")}\t50\t2`);
    // One whose draw costs less than its record, perhaps from another process,
    // adds nothing then and counts on from that lower cost.
    const legacyD = path.join(home, ".local/state/tmux-sidebar/usage/spend-spend-d");
    if (session === "spend-d" && !fs.existsSync(legacyD)) fs.writeFileSync(legacyD, `${new Date().toLocaleDateString("sv")}\t50\t20`);
    execFileSync("bash", [statusline], {
      input: JSON.stringify({ session_id: session, model: { display_name: "Opus" }, workspace: { current_dir: tmp }, context_window: { used_percentage: 5 }, cost: { total_cost_usd: cost }, rate_limits: limits }),
      env: { ...process.env, HOME: home },
    });
  });
  const usage = path.join(home, ".local/state/tmux-sidebar/usage");
  const ownDir = path.join(sidebar.STATE_DIR, "usage");
  fs.mkdirSync(ownDir, { recursive: true });
  fs.readdirSync(usage).forEach((f) => fs.copyFileSync(path.join(usage, f), path.join(ownDir, f)));
  // A session last used on another day counts for nothing today.
  fs.writeFileSync(path.join(ownDir, "spend-old"), "2020-01-01\t9\t3");

  // The request rows after them have a test of their own (requests.test.js).
  assert.deepEqual(sidebar.usageItems().slice(0, 3).map((i) => [i.title, i.detail, i.state]), [
    ["Spent today $29.75", "4 sessions, at API prices", "idle"],
    ["5-hour limit 42% used", "resets in 2h 10m", "idle"],
    ["Weekly limit 85% used", "resets in 50m", "attention"],
  ]);
});

test("the Resources view counts each tab's processes from its panes down, heaviest overall first, in a table", async () => {
  const list = sidebar.parsePs([
    "  100     1   1024   0.5 /bin/zsh",
    "  101   100 2097152  80.0 /usr/local/bin/node",
    "  102   101 1048576  40.0 /usr/local/bin/node",
    "  200     1   2048   0.0 -zsh",
    "  201   200 204800   0.4 /opt/homebrew/bin/nvim",
    "  300     1 999999  99.0 /Applications/Other.app/Other",
  ].join("\n"));
  const tabs = [
    { index: 1, name: "light", panes: [{ pane_pid: "200" }] },
    { index: 2, name: "heavy", panes: [{ pane_pid: "100" }] },
  ];
  assert.deepEqual(sidebar.resourceItems(tabs, list).map((i) => [i.cells, i.more, i.state]), [
    [["2 heavy", "120.5", "3.0 GB", "3"], "node ×2 3.0 GB, zsh 1 MB", "attention"],
    [["1 light", "0.4", "202 MB", "2"], "nvim 200 MB, zsh 2 MB", "idle"],
  ]);
  assert.deepEqual(sidebar.resourceTotals(tabs, list), ["All tabs", "120.9", "3.2 GB", "5"]);
  // Heavy overall, not by memory alone: a pegged core outweighs a GB.
  const busy = sidebar.parsePs(["  400     1 1048576   0.0 /bin/big", "  500     1  10240 150.0 /bin/spin"].join("\n"));
  assert.deepEqual(sidebar.resourceItems([{ index: 1, name: "big", panes: [{ pane_pid: "400" }] }, { index: 2, name: "spin", panes: [{ pane_pid: "500" }] }], busy).map((i) => i.title), ["2 spin", "1 big"]);
  assert.deepEqual(sidebar.resourceItems(tabs, undefined), [], "nothing before the first look");
  // The view lines its columns up under their names, the figures on the
  // right, the chosen row adding its heaviest processes, totals at the foot.
  const section = { title: "Resources", items: sidebar.resourceItems(tabs, list), table: sidebar.RESOURCE_COLUMNS, footer: sidebar.resourceTotals(tabs, list), loaded: true };
  const text = sidebar.missionLines({ tabs: [], sections: [section], cols: 80, rows: 12, view: { view: 1, cursor: 0, frame: 0 }, config: {} }).map((line) => line.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "").trimEnd());
  assert.deepEqual(text.slice(1, 4), [
    "   Tab       % CPU  Memory  Processes",
    " ● 2 heavy   120.5  3.0 GB          3  node ×2 3.0 GB, zsh 1 MB",
    " ○ 1 light     0.4  202 MB          2",
  ]);
  // A long tab name gives way to the chosen row's processes, and / finds a
  // tab by its figures.
  const long = { ...section, items: section.items.map((item, i) => (i ? item : { ...item, cells: ["2 " + "x".repeat(70), ...item.cells.slice(1)] })) };
  const longText = sidebar.missionLines({ tabs: [], sections: [long], cols: 80, rows: 12, view: { view: 1, cursor: 0, frame: 0 }, config: {} }).map((line) => line.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "").trimEnd());
  assert.match(longText[2], /x…  120\.5  3\.0 GB          3  node ×2 3\.0 GB, zsh 1 MB$/);
  assert.ok(section.items[1].search.includes("202 MB"));
  assert.deepEqual(text.slice(9, 11), [` ${"─".repeat(78)}`, "   All tabs  120.9  3.2 GB          5"]);
  // The daemon's own snapshot carries each pane's pid for it.
  const snap = await sidebar.snapshot();
  assert.ok(snap.panes.every((p) => /^\d+$/.test(p.pane_pid)));
});

test("tabs in the sidebar are set apart by a rule", async () => {
  tmux("new-window", "-t", "test", "-c", tmp);
  const snap = await sidebar.snapshot();
  const tabs = sidebar.tabsFrom(snap);
  const { lines, owners } = sidebar.sidebarLines({ tabs, sections: [], cols: 30, rows: 14, view: { focused: false, cursor: tabs[0].id, frame: 0, here: tabs[0].id } });
  const text = lines.map((line) => line.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, ""));
  // Header, blank, group title, a tab (name and state), the rule, the next tab.
  assert.equal(text[5], ` ${"─".repeat(28)} `);
  assert.equal(owners[5], undefined);
  assert.equal(owners[6], tabs[1].id);
});

test("the daemon draws the tabs in each sidebar and moves between them with the keys", async (t) => {
  t.after(() => sidebar.stop());
  tmux("rename-window", "-t", "test:", "alpha");
  const second = tmux("new-window", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  tmux("rename-window", "-t", second, "bravo");
  tmux("set", "-p", "-t", second, "@agent_state", "waiting");
  tmux("set", "-p", "-t", second, "@agent_since", String(Math.floor(Date.now() / 1000) - 60));
  tmux("setw", "-t", second, "@agent", "waiting");
  const first = tmux("list-windows", "-t", "test", "-F", "#{window_id}").split("\n")[0];
  tmux("select-window", "-t", first);
  assert.equal(sidebar.start(), true);

  const pane = await until(() => sidebarPane(first));
  assert.ok(pane, "no sidebar in the first window");
  const screen = await until(() => {
    const text = tmux("capture-pane", "-p", "-t", pane);
    return text.includes("bravo") && text.includes("alpha") && text;
  });
  assert.ok(screen, "the sidebar never drew the tabs");
  // The waiting tab is listed first.
  assert.ok(screen.indexOf("bravo") < screen.indexOf("alpha"));
  assert.match(screen, /waiting on you 1m/);

  // A client attached, as when you are looking at it: without one the daemon
  // hands a sidebar's focus back to its window's main pane. Control mode
  // needs no terminal.
  const viewer = spawn("tmux", ["-S", socket, "-C", "attach", "-t", "test"], { stdio: ["pipe", "ignore", "ignore"] });
  t.after(() => viewer.kill());
  assert.equal(await until(() => tmux("display", "-p", "-t", "test", "#{session_attached}") !== "0"), true);

  // Into the sidebar from alpha: the cursor starts on alpha, the tab you are in.
  const session = tmux("display", "-p", "-t", second, "#{session_id}");
  tmux("select-pane", "-t", pane);
  await wait(100);
  await sidebar.refresh();
  assert.equal(sidebar.state.cursors.get(session), first);
  // Browsing shows each tab as the cursor reaches it: k up to bravo, first as
  // it waits on you, whose window comes to the front with focus in its own
  // sidebar and the cursor still on bravo.
  tmux("send-keys", "-t", pane, "k");
  assert.equal(await until(() => tmux("display", "-p", "-t", "test", "#{window_id}") === second), true, "bravo is in front");
  assert.equal(await until(() => tmux("display", "-p", "-t", second, "#{@sidebar}") === "1"), true, "focus is in bravo's sidebar");
  await sidebar.refresh();
  assert.equal(sidebar.state.cursors.get(session), second, "the cursor stayed on bravo");
  // Enter drops into bravo's pane.
  const bravoSidebar = tmux("display", "-p", "-t", second, "#{pane_id}");
  tmux("send-keys", "-t", bravoSidebar, "Enter");
  // Focus went to bravo's main pane, and alpha's sidebar gave it back too.
  assert.equal(await until(() => tmux("display", "-p", "-t", second, "#{@sidebar}") === ""), true);
  assert.equal(tmux("display", "-p", "-t", "test", "#{window_id}"), second);
  assert.equal(tmux("display", "-p", "-t", first, "#{@sidebar}"), "");

  // Mission control lists both, the waiting one first; l and Enter go to the other.
  const client = net.connect(sidebar.SOCKET);
  let received = "";
  client.on("data", (data) => (received += data));
  client.write("mission none 120 30\n");
  assert.ok(await until(() => plain(received).includes("MISSION CONTROL") && plain(received).includes("alpha")));
  assert.ok(plain(received).indexOf("bravo") < plain(received).indexOf("alpha"));
  // ? shows the keys over it; the next key only puts it back, the cursor where it was.
  received = "";
  client.write("?");
  assert.ok(await until(() => plain(received).includes("choose a card")), "the keys are shown");
  assert.match(plain(received), /MISSION CONTROL[\s\S]*╭─ Keys/, "in a box over mission control, which stays drawn");
  received = "";
  client.write("x");
  assert.ok(await until(() => plain(received).includes("MISSION CONTROL")), "and put away");
  client.write("l\r");
  assert.equal(await until(() => tmux("display", "-p", "-t", "test", "#{window_id}") === first), true);
  client.write("q");
  await until(() => client.destroyed || client.readableEnded);
  client.destroy();
});

test("a self-paced /loop counts as looping until its next wakeup is overdue, and a stop ends it", () => {
  const file = path.join(home, ".claude/projects/-work/loop.jsonl");
  const now = Date.now();
  const at = (ms) => new Date(now - ms).toISOString();
  const wakeup = (ms, input) => ({ type: "assistant", timestamp: at(ms), message: { content: [{ type: "tool_use", id: `s${ms}`, name: "ScheduleWakeup", input }] } });
  fs.writeFileSync(file, JSON.stringify(wakeup(60_000, { delaySeconds: 900, prompt: "/loop /oncall", reason: "quiet" })) + "\n");
  assert.deepEqual(sidebar.backgroundWork(file, now - 120_000), { count: 0, monitors: [], scheduled: { prompt: "/oncall", at: now - 60_000 + 900_000 } });
  assert.deepEqual(sidebar.backgroundWork(file, now - 30_000), { count: 0, monitors: [] }, "scheduled by an earlier process");

  fs.appendFileSync(file, JSON.stringify(wakeup(20_000, { stop: true })) + "\n");
  assert.deepEqual(sidebar.backgroundWork(file, now - 120_000), { count: 0, monitors: [] }, "stopped");

  fs.writeFileSync(file, JSON.stringify(wakeup(30 * 60_000, { delaySeconds: 600, prompt: "/loop /oncall" })) + "\n");
  assert.deepEqual(sidebar.backgroundWork(file, now - 60 * 60_000), { count: 0, monitors: [] }, "overdue, so the loop ended");

  const tab = { id: "@9", index: 9, name: "on call", state: "background", background: 0, monitors: [], loop: { kind: "scheduled", prompt: "/oncall", at: now + 7 * 60_000 + 5_000 }, session: "$1", panes: [], sidebars: [] };
  assert.equal(sidebar.staged([tab])[0].key, "scheduled");
  assert.equal(sidebar.stateWords(tab), "next in 7m · /oncall");
});

test("a cron job Claude created is a loop until CronDelete names it, shown by its prompt's first sentence and next run", () => {
  const file = path.join(home, ".claude/projects/-work/cron.jsonl");
  const now = Date.now();
  const at = (ms) => new Date(now - ms).toISOString();
  const prompt = "Watch the reply to the DM. Read the channel, and act on it.";
  const entries = [
    { type: "assistant", timestamp: at(60_000), message: { content: [{ type: "tool_use", id: "c1", name: "CronCreate", input: { cron: "*/5 * * * *", recurring: true, prompt } }] } },
    { type: "user", timestamp: at(60_000), message: { content: [{ type: "tool_result", tool_use_id: "c1", content: "Scheduled recurring job a03c0408 (*/5 * * * *). Next run: soon. Session-only." }] } },
  ];
  fs.writeFileSync(file, entries.map((entry) => JSON.stringify(entry)).join("\n") + "\n");
  const work = sidebar.backgroundWork(file, now - 120_000);
  assert.equal(work.scheduled.prompt, "Watch the reply to the DM.");
  assert.equal(work.scheduled.at, sidebar.nextCron("*/5 * * * *", now));
  assert.ok(work.scheduled.at > now && work.scheduled.at <= now + 5 * 60_000);

  fs.appendFileSync(file, JSON.stringify({ type: "assistant", timestamp: at(1_000), message: { content: [{ type: "tool_use", id: "c2", name: "CronDelete", input: { id: "a03c0408" } }] } }) + "\n");
  assert.deepEqual(sidebar.backgroundWork(file, now - 120_000), { count: 0, monitors: [] }, "deleted");

  const nine = new Date(2026, 9, 26, 8, 59, 30).getTime();
  assert.equal(sidebar.nextCron("0 9 * * 1-5", nine), new Date(2026, 9, 26, 9, 0).getTime());
  assert.equal(sidebar.nextCron("3-59/5 * * * *", new Date(2026, 9, 26, 16, 9).getTime()), new Date(2026, 9, 26, 16, 13).getTime());
  assert.equal(sidebar.loopName("/loop 5m /babysit-prs"), "/babysit-prs");
});

test("mission control's Settings view lists the options by category, / narrows them to the matching categories, and Enter and Space change one", async () => {
  const settingsFile = path.join(home, ".config/switchboard/settings.json");
  fs.rmSync(settingsFile, { force: true });
  const sent = [];
  const client = { kind: "mission", socket: { write: (text) => sent.push(text), end() {} }, view: { view: 0, cursor: 0 }, cols: 160, rows: 140, tmuxClient: "none" };
  try {
    await sidebar.refresh();
    client.view.view = sidebar.viewsFor(client).findIndex((s) => s.title === "Settings") + 1;
    await sidebar.missionKey(client, { key: "g" });
    const all = plain(sent.at(-1));
    assert.match(all, /NOTIFICATIONS 2[\s\S]*Push notifications +on +A macOS banner[\s\S]*SLACK RELAY 1/);

    // / then "notif": the Notifications header and its two options, and no other category.
    for (const key of ["/", "n", "o", "t", "i", "f", "Enter"]) await sidebar.missionKey(client, { key });
    const items = sidebar.viewsFor(client)[client.view.view - 1].items;
    assert.deepEqual(items.map((i) => i.title), ["Notifications", "Push notifications", "Finished tabs clear", "Sidebar", "Notifications", "M-a goes to notifications", "Notifications view", "New notes", "Next meeting", "Calendar changes", "Mission control", "Highlight on hover", "Help", "Suggestions from how you work"], "the Notifications category, and the options elsewhere that mention notifications");
    assert.doesNotMatch(plain(sent.at(-1)), /Theme/);

    // Enter on an on/off option turns it off, and only what differs from the default is written.
    await sidebar.missionKey(client, { key: "j" });
    await sidebar.missionKey(client, { key: "Enter" });
    assert.deepEqual(JSON.parse(fs.readFileSync(settingsFile, "utf8")), { notifications: false });
    assert.match(plain(sent.at(-1)), /Push notifications +off/);
    // Enter on a choice goes to its next one; turning the first back on leaves only that.
    await sidebar.missionKey(client, { key: "Enter" });
    await sidebar.missionKey(client, { key: "j" });
    await sidebar.missionKey(client, { key: "Enter" });
    assert.deepEqual(JSON.parse(fs.readFileSync(settingsFile, "utf8")), { done_clears: "prompt" });
    assert.match(plain(sent.at(-1)), /Finished tabs clear +on prompt/);
  } finally {
    fs.rmSync(settingsFile, { force: true });
    sidebar.reloadSettings();
  }
});

test("Settings lists the settings in your own files under each file's name, and Enter types one in, keeping the rest of the file", async () => {
  const dir = path.join(home, ".config/switchboard");
  const linear = path.join(dir, "linear.json");
  const sent = [];
  const client = { kind: "mission", socket: { write: (text) => sent.push(text), end() {} }, view: { view: 0, cursor: 0 }, cols: 200, rows: 80, tmuxClient: "none" };
  try {
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(linear, JSON.stringify({ linearWorkspace: "acme", linearApiKey: "kept" }));
    await sidebar.refresh();
    client.view.view = sidebar.viewsFor(client).findIndex((s) => s.title === "Settings") + 1;
    const items = () => sidebar.viewsFor(client)[client.view.view - 1].items;
    const at = (title) => items().findIndex((i) => i.title === title && i.own);
    client.view.cursor = at("Team");
    await sidebar.missionKey(client, { key: "k" });
    await sidebar.missionKey(client, { key: "j" });
    const shown = plain(sent.at(-1));
    assert.match(shown, /LINEAR \(LINEAR\.JSON\) 3[\s\S]*Workspace +acme[\s\S]*Team +not set/);
    const headers = items().filter((i) => i.group === "category").map((i) => i.title);
    ["Browser (browser.json)", "Slack (slack-updates.json)", "Slack relay (slack-relay.json)", "On call (oncall.json)", "Sidebar layout (sidebar.json)", "Your files"].forEach((h) => assert.ok(headers.includes(h), h));

    // A word: typed in and saved, the file's other keys kept.
    await sidebar.missionKey(client, { key: "Enter" });
    for (const key of [..."ABC"]) await sidebar.missionKey(client, { key });
    await sidebar.missionKey(client, { key: "Enter" });
    assert.deepEqual(JSON.parse(fs.readFileSync(linear, "utf8")), { linearWorkspace: "acme", linearApiKey: "kept", linearTeam: "ABC" });

    // A list, comma-separated; then emptied, which unsets it.
    client.view.cursor = at("Labels");
    await sidebar.missionKey(client, { key: "Enter" });
    for (const key of [..."bug,", "Space", ..."ux"]) await sidebar.missionKey(client, { key });
    await sidebar.missionKey(client, { key: "Enter" });
    assert.deepEqual(JSON.parse(fs.readFileSync(linear, "utf8")).linearLabels, ["bug", "ux"]);
    client.view.cursor = at("Labels");
    await sidebar.missionKey(client, { key: "Enter" });
    for (let i = 0; i < 7; i++) await sidebar.missionKey(client, { key: "BSpace" });
    await sidebar.missionKey(client, { key: "Enter" });
    assert.ok(!("linearLabels" in JSON.parse(fs.readFileSync(linear, "utf8"))));

    // A list whose entry holds a comma opens whole rather than being split.
    fs.writeFileSync(linear, JSON.stringify({ linearLabels: ["UI, UX"] }));
    assert.equal(items().find((i) => i.title === "Labels" && i.own).own.kind, "file");
    fs.writeFileSync(linear, "{}");
    // A number that isn't one says so and saves nothing; a map of site=profile.
    client.view.cursor = at("Sidebar width");
    await sidebar.missionKey(client, { key: "Enter" });
    await sidebar.missionKey(client, { key: "x" });
    await sidebar.missionKey(client, { key: "Enter" });
    assert.equal(client.note, "Not saved: x isn't a whole number, 20 or more");
    assert.equal(sidebar.parseTyped("number", "34.5").error, "34.5 isn't a whole number, 1 or more");
    assert.equal(sidebar.parseTyped("number", "1", 20).error, "1 isn't a whole number, 20 or more");
    assert.ok(!fs.existsSync(path.join(dir, "sidebar.json")));
    assert.deepEqual(sidebar.parseTyped("map", "linear.app=Profile 2, awsapps.com=Profile 3").value, { "linear.app": "Profile 2", "awsapps.com": "Profile 3" });
    assert.equal(sidebar.parseTyped("map", "linear.app").error, '"linear.app" isn\'t site=profile');

    // A file that can't be parsed is never written over; one's mode is kept.
    fs.chmodSync(linear, 0o600);
    assert.equal(sidebar.writeOwn("linear.json", "linearTeam", "text", "XYZ"), undefined);
    assert.equal(fs.statSync(linear).mode & 0o777, 0o600);
    fs.writeFileSync(linear, '{"linearTeam": "ABC",}');
    assert.match(sidebar.writeOwn("linear.json", "linearTeam", "text", "XYZ"), /can't be read/);
    assert.equal(fs.readFileSync(linear, "utf8"), '{"linearTeam": "ABC",}');
    fs.writeFileSync(linear, "null");
    assert.match(sidebar.writeOwn("linear.json", "linearTeam", "text", "XYZ"), /isn't a JSON object/);
    assert.ok(sidebar.viewsFor(client).length, "mission control still draws");
    assert.equal(items().find((i) => i.title === "Team" && i.own).value, "unreadable");

    // Saved through a link, the link stays and the file it points to changes.
    const real = path.join(home, "linear-real.json");
    fs.writeFileSync(real, "{}");
    fs.rmSync(linear);
    fs.symlinkSync(real, linear);
    assert.equal(sidebar.writeOwn("linear.json", "linearTeam", "text", "XYZ"), undefined);
    assert.ok(fs.lstatSync(linear).isSymbolicLink());
    assert.deepEqual(JSON.parse(fs.readFileSync(real, "utf8")), { linearTeam: "XYZ" });
    fs.rmSync(real);
    assert.match(sidebar.writeOwn("linear.json", "linearTeam", "text", "XYZ"), /a link to nothing/, "a link whose file is gone isn't replaced");
    assert.ok(fs.lstatSync(linear).isSymbolicLink());
  } finally {
    fs.rmSync(linear, { force: true });
  }
});

test("Settings' Your CLAUDE.md says where your rules come from, and picking a file writes claude.json and your CLAUDE.md", async () => {
  const claudeJson = path.join(home, ".config/switchboard/claude.json");
  const rules = path.join(home, "my-rules.md");
  const sent = [];
  const client = { kind: "mission", socket: { write: (text) => sent.push(text), end() {} }, view: { view: 0, cursor: 0 }, cols: 160, rows: 60, tmuxClient: "none" };
  try {
    fs.symlinkSync(path.join(__dirname, ".."), path.join(home, "switchboard"));
    fs.writeFileSync(rules, "# Mine\n");
    await sidebar.refresh();
    client.view.view = sidebar.viewsFor(client).findIndex((s) => s.title === "Settings") + 1;
    const items = sidebar.viewsFor(client)[client.view.view - 1].items;
    client.view.cursor = items.findIndex((i) => i.title === "Your CLAUDE.md");
    await sidebar.missionKey(client, { key: "k" });
    await sidebar.missionKey(client, { key: "j" });
    assert.match(plain(sent.at(-1)), /CLAUDE CODE 1[\s\S]*Your CLAUDE\.md +local file/);

    // Enter, f, a path and Enter: claude.json names it, and setup's claude_md writes the import.
    await sidebar.missionKey(client, { key: "Enter" });
    assert.match(plain(sent.at(-1)), /Local file[\s\S]*Another file…[\s\S]*A git repository…/);
    client.choose.cursor = client.choose.choices.findIndex((c) => c.key === "f");
    await sidebar.missionKey(client, { key: "Enter" });
    for (const key of [..."~/my-rules.md"]) await sidebar.missionKey(client, { key });
    assert.match(plain(sent.at(-1)), /› ~\/my-rules\.md/);
    await sidebar.missionKey(client, { key: "Enter" });
    assert.deepEqual(JSON.parse(fs.readFileSync(claudeJson, "utf8")), { path: "~/my-rules.md" });
    assert.match(fs.readFileSync(path.join(home, ".claude/CLAUDE.md"), "utf8"), /\n@~\/my-rules\.md\n@~\/switchboard\/claude\/switchboard\.md\n$/);
    assert.match(plain(sent.at(-1)), /Your CLAUDE\.md: ~\/my-rules\.md/);

    // Back to the local file: claude.json goes.
    await sidebar.missionKey(client, { key: "Enter" });
    client.choose.cursor = client.choose.choices.findIndex((c) => c.key === "l");
    await sidebar.missionKey(client, { key: "Enter" });
    assert.ok(!fs.existsSync(claudeJson));
  } finally {
    for (const f of [claudeJson, rules, path.join(home, "switchboard"), path.join(home, ".claude/CLAUDE.md")]) fs.rmSync(f, { force: true });
  }
});

test("with the github and linear settings off, mission control has no PRs or Tickets view and the Work map no PR or ticket", () => {
  const settingsFile = path.join(home, ".config/switchboard/settings.json");
  const tab = { id: "@90", index: 9, name: "Some work", path: path.join(tmp, "nowhere"), pr: "12", state: "idle" };
  try {
    fs.mkdirSync(path.dirname(settingsFile), { recursive: true });
    fs.rmSync(settingsFile, { force: true });
    sidebar.reloadSettings();
    assert.ok(sidebar.builtInViews({ tabs: [] }).some((v) => v.title === "PRs"));
    assert.ok(sidebar.builtInViews({ tabs: [] }).some((v) => v.title === "Tickets"));
    assert.equal(sidebar.workMap([tab]).find((r) => r.tab === tab).cells[3], "#12 open");

    fs.writeFileSync(settingsFile, JSON.stringify({ github: false, linear: false }));
    sidebar.reloadSettings();
    assert.ok(!sidebar.builtInViews({ tabs: [] }).some((v) => v.title === "PRs"));
    assert.ok(!sidebar.builtInViews({ tabs: [] }).some((v) => v.title === "Tickets"));
    assert.equal(sidebar.workMap([tab]).find((r) => r.tab === tab).cells[3], "");
    assert.deepEqual(sidebar.mergeOf({ path: tab.path, branch: "feat" }), {});
  } finally {
    fs.rmSync(settingsFile, { force: true });
    sidebar.reloadSettings();
  }
});

test("a tab whose transcript can't be read is still listed, and the others with it", async () => {
  tmux("new-window", "-t", "test", "-c", tmp);
  const pane = tmux("display", "-p", "-t", "test:", "#{pane_id}");
  tmux("set", "-p", "-t", pane, "@agent_state", "done");
  tmux("set", "-p", "-t", pane, "@agent_since", String(Math.floor(Date.now() / 1000)));
  // A directory where its transcript should be: reading it throws.
  fs.mkdirSync(path.join(home, ".claude/projects/-work/broken.jsonl"), { recursive: true });
  fs.writeFileSync(path.join(home, ".claude/sessions/131.json"), JSON.stringify({ pid: 131, sessionId: "broken", cwd: "/work", tmux: `test:@9.${pane}`, entrypoint: "cli" }));
  const failures = [];
  try {
    const tabs = sidebar.tabsFrom(await sidebar.snapshot(), (tab, err) => failures.push(err.code));
    assert.equal(tabs.length, 2);
    assert.equal(tabs.find((t) => t.claudePane === pane).background, 0);
    assert.deepEqual(failures, ["EISDIR"]);
  } finally {
    fs.rmSync(path.join(home, ".claude/sessions/131.json"));
    fs.rmSync(path.join(home, ".claude/projects/-work/broken.jsonl"), { recursive: true });
  }
});

test("a merged tab's badge says merged while Claude is marked working, and waiting still wins", async () => {
  const snap = await sidebar.snapshot();
  const [tab] = sidebar.tabsFrom(snap);
  const badgeOf = (t) => sidebar.sidebarLines({ tabs: [t], sections: [], cols: 40, rows: 10, view: { focused: false, cursor: t.id, frame: 0, here: t.id } }).lines.join("");
  assert.match(badgeOf({ ...tab, state: "working", merge: "merged" }), /◆/);
  assert.doesNotMatch(badgeOf({ ...tab, state: "working", merge: "merged" }), /⠋/);
  assert.doesNotMatch(badgeOf({ ...tab, state: "waiting", merge: "merged" }), /◆/);
  assert.match(badgeOf({ ...tab, state: "working", merge: "queued" }), /⠋/, "a tab in the merge queue keeps its working spinner");
});

test("a Markdown table in a reply is drawn in a box, its cells wrapped in their columns, or one cell a line when it can't fit", () => {
  const reply = "Today:\n\n| Time | Event | You |\n|---|:--|---|\n| 1:00–1:15 PM | Standup | not answered |\n| 1:15–2:00 PM | Community of Practicing Engineers | going |\n\nFree until 11.";
  assert.deepEqual(sidebar.layout(reply, 80), [
    "Today:",
    "",
    "┌──────────────┬───────────────────────────────────┬──────────────┐",
    "│ Time         │ Event                             │ You          │",
    "├──────────────┼───────────────────────────────────┼──────────────┤",
    "│ 1:00–1:15 PM │ Standup                           │ not answered │",
    "├──────────────┼───────────────────────────────────┼──────────────┤",
    "│ 1:15–2:00 PM │ Community of Practicing Engineers │ going        │",
    "└──────────────┴───────────────────────────────────┴──────────────┘",
    "",
    "Free until 11.",
  ]);
  const narrow = sidebar.layout(reply, 44);
  assert.ok(narrow.every((line) => sidebar.width(line) <= 44), "fits");
  assert.ok(narrow.includes("│ 1:15–2:00   │ Community of  │ going      │") && narrow.some((line) => line.includes("│ Practicing ")), "no word split");
  assert.deepEqual(sidebar.layout(reply, 24).slice(2, 6), ["Time: 1:00–1:15 PM", "Event: Standup", "You: not answered", "────────────────────"]);
});

test("the ticket board puts your Linear issues in columns by state, urgent first in Todo, with the tab working on each", () => {
  const issue = (identifier, priority, name, type, updatedAt, completedAt) => ({
    identifier, title: `Work on ${identifier}`, url: `https://linear.app/x/issue/${identifier}`, priority, priorityLabel: ["No priority", "Urgent", "High", "Medium", "Low"][priority], updatedAt, completedAt, state: { name, type }, project: null,
  });
  const columns = sidebar.ticketsFrom(
    [
      issue("AB-1", 3, "Todo", "unstarted", "2026-10-01T00:00:00Z"),
      issue("AB-2", 1, "Todo", "unstarted", "2026-09-01T00:00:00Z"),
      issue("AB-3", 0, "In Progress", "started", "2026-10-02T00:00:00Z"),
      issue("AB-4", 2, "Ready for Review", "started", "2026-10-03T00:00:00Z"),
      issue("AB-5", 0, "Done", "completed", "2026-10-04T00:00:00Z", "2026-10-04T00:00:00Z"),
    ],
    [{ index: 4, name: "AB-3: Work on AB-3" }, { index: 5, name: "AB-30: something else" }],
  );
  assert.deepEqual(columns.map((c) => [c.title, c.items.map((i) => i.title.split(" ")[0])]), [
    ["Todo", ["AB-2", "AB-1"]],
    ["In progress", ["AB-3"]],
    ["In review", ["AB-4"]],
    ["Done", ["AB-5"]],
  ]);
  assert.equal(columns[0].items[0].state, "attention", "urgent needs you");
  assert.equal(columns[1].items[0].detail, "In Progress · tab 4");
  assert.equal(columns[0].items[1].detail, "Todo · Medium");
});

test("a merged tab still waiting on its deploy or testing it is under Testing Deploy, before Merged", () => {
  const base = { session: "$1", sessionName: "test", panes: [], sidebars: [], since: undefined, command: "claude", claude: { sessionId: "s" }, merge: "merged" };
  const tabs = [
    { ...base, id: "@1", index: 1, name: "waiting on deploy", state: "background", loop: { kind: "deploy", round: 1 } },
    { ...base, id: "@2", index: 2, name: "recording", state: "working", loopSkill: "queue" },
    { ...base, id: "@3", index: 3, name: "approve the video", state: "waiting", loopSkill: "deploy" },
    { ...base, id: "@4", index: 4, name: "done with it", state: "idle" },
  ];
  assert.deepEqual(sidebar.grouped(tabs).map((g) => [g.title, g.tabs.map((x) => x.name)]), [
    ["Needs you", ["approve the video"]],
    ["Testing Deploy", ["waiting on deploy", "recording"]],
    ["Merged", ["done with it"]],
  ]);
  assert.deepEqual(sidebar.staged(tabs).map((g) => g.title), ["Needs you", "Testing Deploy", "Merged"]);
});

test("the Calendar view draws the day /today saved, and says when it is out of date", () => {
  const stateDir = fs.mkdtempSync(path.join(tmp, "calendar-"));
  const env = { ...process.env, TMUX_SIDEBAR_STATE_DIR: stateDir };
  const now = new Date();
  const date = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}-${String(now.getDate()).padStart(2, "0")}`;
  const lateNight = new Date(now.getTime() + 7_200_000).getDate() !== now.getDate();
  const day = { date, events: [
    { title: "Planning", start: new Date(now.getTime() - 3 * 3_600_000).toISOString(), end: new Date(now.getTime() - 2 * 3_600_000).toISOString(), you: "going", link: "https://calendar.example/1" },
    // In an hour, or, late at night when that is tomorrow, going on now.
    { title: "Review", start: new Date(now.getTime() + (lateNight ? -1_800_000 : 3_600_000)).toISOString(), end: new Date(now.getTime() + (lateNight ? 600_000 : 7_200_000)).toISOString(), you: "maybe", join: "https://zoom.example/j/2" },
    { title: "Bad link", start: now.toISOString(), end: now.toISOString(), you: "going", join: "javascript:alert(1)" },
  ] };
  execFileSync("node", [path.join(__dirname, "../claude/skills/today/scripts/save.js")], { input: JSON.stringify(day), env });
  const saved = JSON.parse(fs.readFileSync(path.join(stateDir, "calendar.json"), "utf8"));
  assert.equal(saved.events[2].join, undefined, "only https links are kept");
  const script = `
    const { calendarView, calendarLines, calendarStale, calendarLooping } = require(${JSON.stringify(path.join(__dirname, "../lib/sidebar/calendar"))});
    const view = calendarView([]);
    const plain = calendarLines(view, 120, 40, view.items.findIndex((i) => i.title === "Bad link")).map((l) => l.replace(/\\x1b\\[[0-9;]*m/g, ""));
    console.log(JSON.stringify({ items: view.items.map((i) => [i.title, i.url]), plain, stale: view.stale, looping: calendarLooping([{ name: "Calendar", claude: {} }]), loopingNone: calendarLooping([{ name: "Calendar" }]) }));`;
  const out = JSON.parse(execFileSync("node", ["-e", script], { env, encoding: "utf8" }));
  assert.deepEqual(out.items.map((i) => i[0]), lateNight ? ["Planning", "Review", "Bad link"] : ["Planning", "Bad link", "Review"]);
  assert.equal(out.items.find((i) => i[0] === "Review")[1], "https://zoom.example/j/2", "Enter joins the call");
  assert.equal(out.stale, false);
  assert.ok(out.plain.some((l) => l.includes("Planning")) && out.plain.some((l) => l.includes("▎Review")), "an unanswered event is outlined");
  assert.ok(out.plain.some((l) => l.includes("●") && l.includes("─")), "a line at now");
  assert.ok(out.looping && !out.loopingNone, "a Calendar tab counts only while Claude runs in it");

  fs.writeFileSync(path.join(stateDir, "calendar.json"), JSON.stringify({ ...saved, date: "2020-01-01" }));
  const old = JSON.parse(execFileSync("node", ["-e", script], { env, encoding: "utf8" }));
  assert.equal(old.stale, true);
  assert.match(old.plain.join("\n"), /No calendar for today yet \(last saved 2020-01-01\): opening a Calendar tab to fetch it/);

  // With the feed set up: no Calendar tab while it reads. Once its looks have
  // failed one after another for a quarter of an hour, it is lost: /today
  // keeps the day, a tab opened for it until it has saved one, and its day
  // shown once newer than the feed's. `looks` are the failed looks, minutes
  // ago, oldest first.
  const lost = (looks, savedAt) => `
    const f = require(${JSON.stringify(path.join(__dirname, "../lib/sidebar/calendar-feed"))});
    const { calendarStale, readDay } = require(${JSON.stringify(path.join(__dirname, "../lib/sidebar/calendar"))});
    Object.assign(f.feed, { configured: true, day: { date: ${JSON.stringify(date)}, updated: Date.now() - 3_600_000, source: "ical", events: [] } });
    ${JSON.stringify(looks)}.forEach((ago) => f.failed("Couldn't read the calendar feed: offline", Date.now() - ago * 60_000));
    require("node:fs").writeFileSync(${JSON.stringify(path.join(stateDir, "calendar.json"))}, JSON.stringify({ date: ${JSON.stringify(date)}, updated: Date.now() - ${savedAt}, events: [] }));
    console.log(JSON.stringify({ stale: calendarStale(readDay()), source: readDay().source ?? "saved" }));`;
  const feedRun = (looks, savedAt) => JSON.parse(execFileSync("node", ["-e", lost(looks, savedAt)], { env, encoding: "utf8" }));
  assert.deepEqual(feedRun([], 86_400_000), { stale: false, source: "ical" }, "a feed reading needs no tab");
  assert.deepEqual(feedRun([1], 86_400_000), { stale: false, source: "ical" }, "one failed look is not a lost feed");
  assert.deepEqual(feedRun([20, 15, 10, 5, 0], 86_400_000), { stale: true, source: "ical" }, "a lost feed with nothing newer saved opens a Calendar tab");
  assert.deepEqual(feedRun([75, 70, 0], 86_400_000), { stale: false, source: "ical" }, "time away between failed looks starts the count over");
  assert.deepEqual(feedRun([20, 15, 10, 5, 0], 60_000), { stale: false, source: "saved" }, "and then shows the day /today saved");
  assert.deepEqual(feedRun([20, 15, 10, 5, 0], 3_660_000), { stale: true, source: "ical" }, "a saved day older than the feed's last keeps nothing");
});

test("Enter on a board's PR or ticket offers what to do with it, and each item says what it is", () => {
  const prs = sidebar.boardFrom([{ number: 7, title: "Add it", url: "https://github.com/o/r/pull/7", isDraft: false, state: "OPEN", repository: { nameWithOwner: "o/r" } }]);
  assert.deepEqual(prs.flatMap((c) => c.items)[0].pr, { number: 7, repo: "o/r" });
  const tickets = sidebar.ticketsFrom([{ identifier: "AB-1", title: "Do it", url: "https://linear.app/x/issue/AB-1", priority: 0, state: { name: "Todo", type: "unstarted" }, project: null, updatedAt: "2026-10-01T00:00:00Z" }]);
  assert.equal(tickets.flatMap((c) => c.items)[0].ticket, "AB-1");
  const modal = sidebar.chooseModal({ title: "AB-1 Do it", choices: [{ key: "w", label: "Start work on it in a new tab (wt)" }, { key: "o", label: "Open it in the browser" }], cursor: 1 }, 100, 30);
  const placed = [...modal.matchAll(/\x1b\[(\d+);(\d+)H([^\x1b]*(?:\x1b\[[0-9;]*m[^\x1b]*)*)/g)].map((m) => [Number(m[1]), Number(m[2]), m[3].replace(/\x1b\[[0-9;]*m/g, "")]);
  assert.equal(placed.length, 6, "a box of its own lines, the view left around it");
  assert.ok(placed.every(([row, col]) => row > 1 && col > 1), "set in, not over the whole frame");
  assert.match(placed[0][2], /AB-1 Do it/);
  assert.match(placed[1][2], /Start work on it in a new tab \(wt\)/);
  assert.match(placed[2][2], /Open it in the browser/);
});

test("Enter on a Slack message shows the message whole above what to do with it", () => {
  const body = `Can you look at the deploy?\n${"word ".repeat(40).trim()}`;
  const modal = sidebar.chooseModal({ title: "@ Ann · #eng", body, choices: [{ key: "o", label: "Open it in Slack" }], cursor: 0 }, 100, 40);
  const rows = [...modal.matchAll(/\x1b\[\d+;\d+H([^\x1b]*(?:\x1b\[[0-9;]*m[^\x1b]*)*)/g)].map((m) => m[1].replace(/\x1b\[[0-9;]*m/g, ""));
  assert.match(rows[1], /Can you look at the deploy\?/);
  assert.ok(rows.filter((row) => /word/.test(row)).length > 1, "a long line is wrapped, not cut");
  assert.ok(rows.findIndex((row) => /Open it in Slack/.test(row)) > rows.findLastIndex((row) => /word/.test(row)), "the choices after it");

  // In a short pane, the box grows only as far as it can: every option shown,
  // the chosen one among them, the message cut short to make room.
  const many = ["Open it in Slack", "Read it here", "Open a tab for this thread", "Dismiss it"].map((label, i) => ({ key: String(i), label }));
  const longBody = Array.from({ length: 30 }, (_, i) => `line ${i}`).join("\n");
  const short = sidebar.chooseModal({ title: "t", body: longBody, choices: many, cursor: 3 }, 100, 16);
  const shortRows = [...short.matchAll(/\x1b\[\d+;\d+H([^\x1b]*(?:\x1b\[[0-9;]*m[^\x1b]*)*)/g)].map((m) => m[1].replace(/\x1b\[[0-9;]*m/g, ""));
  many.forEach((c) => assert.ok(shortRows.some((row) => row.includes(c.label)), c.label));
  assert.ok(shortRows.some((row) => /› Dismiss it/.test(row)), "the chosen one shown, marked");
  assert.ok(shortRows.some((row) => /…/.test(row)), "the message cut short");
  assert.ok(shortRows.length <= 16, "within the pane");
});

test("each mission control view but Settings can be turned off in Settings, the Work map staying for M-j and M-o", () => {
  const settingsFile = path.join(home, ".config/switchboard/settings.json");
  try {
    fs.mkdirSync(path.dirname(settingsFile), { recursive: true });
    fs.writeFileSync(settingsFile, JSON.stringify({ view_work_map: false, view_ci: false, view_landing: false, view_inbox: false, view_slack: false, view_closed: false, view_sessions: false, view_calendar: false, pr_board: false, ticket_board: false, usage: false, view_resources: false }));
    sidebar.reloadSettings();
    assert.deepEqual(sidebar.builtInViews({ tabs: [] }).map((v) => v.title), ["Settings"]);
    assert.deepEqual(sidebar.builtInViews({ tabs: [] }, { workMap: true }).map((v) => v.title), ["Work map", "Settings"]);
    fs.rmSync(settingsFile);
    sidebar.reloadSettings();
    assert.deepEqual(sidebar.builtInViews({ tabs: [] }).map((v) => v.title).filter((t) => t !== "Calendar"), ["Work map", "Slack", "Closed", "PRs", "CI", "Tickets", "Inbox", "Sessions", "Resources", "Usage", "Settings"]);
  } finally {
    fs.rmSync(settingsFile, { force: true });
    sidebar.reloadSettings();
  }
});

test("v and a view's name lights that view in mission control's bar, and says what is typed", () => {
  const sections = [{ title: "Work map", items: [], loaded: true }, { title: "Calendar", items: [], loaded: true, calendar: true, today: false, day: { date: "2020-01-01" } }];
  const lines = sidebar.missionLines({ tabs: [], sections, cols: 160, rows: 10, view: { view: 0, cursor: 0, frame: 0, jump: { text: "cal", match: 2 } }, config: {} });
  assert.match(lines[0], /\x1b\[48;5;220m[^ ]*\x1b\[38;5;232m\x1b\[1m Calendar 0 /, "Calendar lit");
  assert.match(lines[0].replace(/\x1b\[[0-9;]*m/g, ""), /view: cal▏/);
  assert.match(lines.at(-1).replace(/\x1b\[[0-9;]*m/g, ""), /type a view's name {2}↵ go there {2}esc cancel/);
});

test("the Slack view lists what /slack-updates saved, newest first, without the relay's own messages, and says when it is out of date", () => {
  const stateDir = fs.mkdtempSync(path.join(tmp, "slack-"));
  const relayDir = fs.mkdtempSync(path.join(tmp, "relay-"));
  fs.writeFileSync(path.join(relayDir, "state.json"), JSON.stringify({ dm: { channel: "DRELAY", bot: "UBOT" } }));
  const env = { ...process.env, TMUX_SIDEBAR_STATE_DIR: stateDir, SLACK_RELAY_STATE: relayDir };
  const save = path.join(__dirname, "../claude/skills/slack-updates/scripts/save.js");
  const now = Date.now() / 1000;
  const empty = JSON.parse(execFileSync("node", ["-e", `const { slackView } = require(${JSON.stringify(path.join(__dirname, "../lib/sidebar/slack"))}); console.log(JSON.stringify(slackView([])))`], { env, encoding: "utf8" }));
  assert.equal(empty.title, "Slack", "there before anything is saved");
  assert.match(empty.error, /^Nothing saved yet: mission control is looking\./);
  const first = execFileSync("node", [save, "--since"], { env, encoding: "utf8" });
  assert.ok(Math.abs(Number(first.trim().split(" ")[1]) - (now - 3600)) < 60, "an hour back the first time");
  const items = [
    { kind: "mention", channel: "C1", where: "#general", user: "U1", from: "Ada", text: "can you look\nat this", ts: String(now - 60), link: "https://x.slack.com/archives/C1/p1" },
    { kind: "dm", channel: "DRELAY", where: "relay", user: "U2", from: "Me", text: "editor configs: tests failed", ts: String(now - 30) },
    { kind: "channel", channel: "C2", where: "#ops", user: "UBOT", from: "relay bot", text: "notice", ts: String(now - 20) },
    { kind: "thread", channel: "C3", where: "#dev", user: "U3", from: "Lin", text: "replied", ts: String(now - 10), link: "javascript:alert(1)" },
    { kind: "dm", channel: "D4", where: "Old", user: "U4", from: "Old", text: "stale", ts: String(now - 2 * 86400) },
  ];
  execFileSync("node", [save], { input: JSON.stringify({ items }), env });
  execFileSync("node", [save], { input: JSON.stringify({ items: [items[0]] }), env });
  const next = Number(execFileSync("node", [save, "--since"], { env, encoding: "utf8" }).trim().split(" ")[1]);
  assert.ok(Math.abs(next - (Date.now() / 1000 - 600)) < 60, "the next look starts ten minutes before this one");
  const script = `
    const { slackView, slackStale } = require(${JSON.stringify(path.join(__dirname, "../lib/sidebar/slack"))});
    const view = slackView([]);
    console.log(JSON.stringify({ titles: view.items.map((i) => i.title), urls: view.items.map((i) => i.url ?? null), details: view.items.map((i) => i.detail), error: view.error ?? null }));`;
  const out = JSON.parse(execFileSync("node", ["-e", script], { env, encoding: "utf8" }));
  assert.deepEqual(out.titles, ["↳ Lin · #dev", "@ Ada · #general"], "newest first, once each, no relay DM or bot, nothing over a day old");
  assert.deepEqual(out.urls, [null, "https://x.slack.com/archives/C1/p1"], "only https links");
  assert.equal(out.details[1], "can you look at this");
  assert.equal(out.error, null);
  // Leaving the view marks what its look brought as seen: below anything new, faded.
  execFileSync("node", ["-e", `require(${JSON.stringify(path.join(__dirname, "../lib/sidebar/slack"))}).markSlackSeen()`], { env });
  execFileSync("node", [save], { input: JSON.stringify({ items: [{ kind: "dm", channel: "D9", where: "Bo", user: "U9", from: "Bo", text: "new one", ts: String(Date.now() / 1000) }] }), env });
  const after = JSON.parse(execFileSync("node", ["-e", script], { env, encoding: "utf8" }));
  assert.deepEqual(after.titles, ["DM Bo · Bo", "↳ Lin · #dev · seen", "@ Ada · #general · seen"]);
  // x dismisses a message until its thread gets something newer.
  const slackLib = JSON.stringify(path.join(__dirname, "../lib/sidebar/slack"));
  execFileSync("node", ["-e", `require(${slackLib}).dismissSlack([{ channel: "C1", ts: "${items[0].ts}" }, { channel: "C3", ts: "${items[3].ts}" }])`], { env });
  const dismissedNow = JSON.parse(execFileSync("node", ["-e", script], { env, encoding: "utf8" }));
  assert.deepEqual(dismissedNow.titles, ["DM Bo · Bo"]);
  execFileSync("node", [save], { input: JSON.stringify({ items: [{ kind: "mention", channel: "C1", where: "#general", user: "U5", from: "Cy", text: "a reply", ts: String(Date.now() / 1000), thread: items[0].ts }] }), env });
  const replied = JSON.parse(execFileSync("node", ["-e", script], { env, encoding: "utf8" }));
  assert.deepEqual(replied.titles, ["@ Cy · #general", "DM Bo · Bo"], "a reply in its thread comes back, the dismissed message itself doesn't");
  // Something you wrote later in the same DM, or the same thread, means you read it in Slack.
  const later = String(Date.now() / 1000 + 1);
  execFileSync("node", [save], { input: JSON.stringify({ items: [], replied: [{ channel: "D9", ts: later }, { channel: "C1", ts: later, thread: items[0].ts }] }), env });
  const read = JSON.parse(execFileSync("node", ["-e", script], { env, encoding: "utf8" }));
  assert.deepEqual([...read.titles].sort(), ["@ Cy · #general · seen", "DM Bo · Bo · seen"], "both read in Slack");
  const saved = JSON.parse(fs.readFileSync(path.join(stateDir, "slack.json"), "utf8"));
  fs.writeFileSync(path.join(stateDir, "slack.json"), JSON.stringify({ ...saved, checked: Date.now() - 3_600_000 }));
  const old = JSON.parse(execFileSync("node", ["-e", script], { env, encoding: "utf8" }));
  assert.match(old.error, /^Out of date, last looked 1h ago: mission control looks every minute while you're here\.$/);
});

test("a connector that isn't connected says what to do, is noted for the views and the health light, and clears once it answers", async (t) => {
  const stateDir = fs.mkdtempSync(path.join(tmp, "connectors-"));
  const bin = fs.mkdtempSync(path.join(tmp, "claude-bin-"));
  const mode = path.join(bin, "mode");
  // A stand-in claude -p: never calls the tool, calls it and is told to sign in, or answers.
  fs.writeFileSync(
    path.join(bin, "claude"),
    `#!/bin/sh
case "$(cat '${mode}')" in
  absent) echo '{"type":"assistant","message":{"content":[{"type":"text","text":"no such tool"}]}}' ;;
  expired) echo '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"mcp__claude_ai_Slack__slack_read_user_profile"}]}}'
           echo '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":"Authentication required: please re-authenticate"}]}}' ;;
  *) echo '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"mcp__claude_ai_Slack__slack_read_user_profile"}]}}'
     echo '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"User ID: U0ABC"}]}}' ;;
esac
`,
    { mode: 0o755 },
  );
  const saved = { PATH: process.env.PATH, TMUX_SIDEBAR_STATE_DIR: process.env.TMUX_SIDEBAR_STATE_DIR };
  Object.assign(process.env, { PATH: `${bin}:${saved.PATH}`, TMUX_SIDEBAR_STATE_DIR: stateDir });
  t.after(() => Object.assign(process.env, saved));
  const { callConnector, connectorDown } = require("../lib/connector");
  const tool = "mcp__claude_ai_Slack__slack_read_user_profile";

  fs.writeFileSync(mode, "absent");
  await assert.rejects(callConnector(tool, {}), (err) => err.notConnected && err.message === "Slack isn't connected in Claude: run /mcp in Claude and sign in to Slack.");
  assert.match(connectorDown("Slack"), /^Slack isn't connected in Claude/);
  assert.match(fs.readFileSync(path.join(stateDir, "connectors-down"), "utf8"), /^Slack\t\d+\n$/);

  fs.writeFileSync(mode, "expired");
  await assert.rejects(callConnector(tool, {}), (err) => err.notConnected, "a lapsed sign-in is not connected too");

  fs.writeFileSync(mode, "ok");
  assert.equal(await callConnector(tool, {}), "User ID: U0ABC");
  assert.equal(connectorDown("Slack"), undefined);
  assert.equal(fs.readFileSync(path.join(stateDir, "connectors-down"), "utf8"), "", "the health light goes dark");
});

test("an empty Slack view says when Slack was last checked", () => {
  const stateDir = fs.mkdtempSync(path.join(tmp, "slack-empty-"));
  const env = { ...process.env, TMUX_SIDEBAR_STATE_DIR: stateDir };
  const view = (checked) => {
    fs.writeFileSync(path.join(stateDir, "slack.json"), JSON.stringify({ checked, items: [] }));
    return JSON.parse(execFileSync("node", ["-e", `console.log(JSON.stringify(require(${JSON.stringify(path.join(__dirname, "../lib/sidebar/slack"))}).slackView([])))`], { env, encoding: "utf8" }));
  };
  assert.equal(view(Date.now() - 10_000).empty, "Nothing new on Slack. Last checked just now.");
  assert.equal(view(Date.now() - 4 * 60_000).empty, "Nothing new on Slack. Last checked 4m ago.");
  const lines = sidebar.missionLines({ tabs: [], sections: [{ title: "Slack", items: [], loaded: true, empty: "Nothing new on Slack. Last checked 4m ago." }], cols: 100, rows: 20, view: { view: 1, cursor: 0, frame: 0 }, config }).map((line) => line.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, ""));
  assert.ok(lines.some((line) => line.includes("Last checked 4m ago.")), "drawn in place of Nothing here");
});

test("mission control looks at Slack itself with one search, sorting what it finds as /slack-updates does", async () => {
  const stateDir = process.env.TMUX_SIDEBAR_STATE_DIR;
  fs.mkdirSync(path.join(home, ".config/slack-updates"), { recursive: true });
  fs.writeFileSync(path.join(home, ".config/switchboard/slack-updates.json"), JSON.stringify({ channels: ["COPS"] }));
  const now = Math.floor(Date.now() / 1000);
  const result = (n, { channel, where, from, user, ts, thread, text, participants = "" }) =>
    `### Result ${n} of 6\nChannel: ${where} (ID: ${channel})\n${participants ? `Participants: ${participants}\n` : ""}From: ${from} (ID: ${user})\nTime: 2026-10-29 09:55:36 EDT\nMessage_ts: ${ts}\nPermalink: [link](https://x.slack.com/archives/${channel}/p${ts.replace(".", "")}${thread ? `?thread_ts=${thread}&cid=${channel}` : ""})\nText: \n${text}\n\n---\n\n`;
  const mine = `${now - 50}.000100`;
  const page = [
    result(1, { channel: "DDEE", where: "DM", participants: "Me (ID: U0MEMEME1), Dee (ID: UDEE)", from: "Dee <dee@example.com>", user: "UDEE", ts: `${now - 40}.000100`, text: "did you see this?\nsecond line" }),
    result(2, { channel: "CGEN", where: "#general", from: "Ada", user: "UADA", ts: `${now - 30}.000100`, text: "<@U0MEMEME1|Me> can you look" }),
    result(3, { channel: "CDEV", where: "#dev", from: "Me", user: "U0MEMEME1", ts: mine, text: "my question" }),
    result(4, { channel: "CDEV", where: "#dev", from: "Lin", user: "ULIN", ts: `${now - 20}.000100`, thread: mine, text: "an answer" }),
    result(5, { channel: "COPS", where: "#ops", from: "Bot", user: "UBOT2", ts: `${now - 10}.000100`, text: "deploy done" }),
    result(6, { channel: "CRANDOM", where: "#random", from: "Cy", user: "UCY", ts: `${now - 5}.000100`, text: "lunch?" }),
  ].join("");
  const asked = [];
  const call = async (tool, input) => {
    asked.push({ tool, input });
    if (tool.endsWith("slack_read_user_profile")) return "User ID: U0MEMEME1\nUsername: me";
    return JSON.stringify({ results: `# Search Results for: \n\n## Messages (6 results)\n${page}` });
  };

  await sidebar.lookSlack(() => {}, call);

  assert.deepEqual(asked.map((a) => a.tool.replace(/^.*__/, "")), ["slack_read_user_profile", "slack_search_public_and_private"], "one profile, then one search for everything");
  const saved = JSON.parse(fs.readFileSync(path.join(stateDir, "slack.json"), "utf8"));
  const kinds = Object.fromEntries(saved.items.map((i) => [i.text, i.kind]));
  assert.equal(kinds["did you see this?\nsecond line"], "dm", "its lines kept, for drawing as Slack does");
  assert.equal(kinds["@Me can you look"], "mention");
  assert.equal(kinds["an answer"], "thread", "a reply in a thread you started");
  assert.equal(kinds["deploy done"], "channel");
  assert.equal(kinds["lunch?"], undefined, "a channel not chosen is left out");
  assert.equal(kinds["my question"], undefined, "what you wrote isn't shown");
  assert.equal(saved.items.find((i) => i.kind === "dm").where, "Dee");
  assert.equal(saved.items.find((i) => i.kind === "dm").from, "Dee", "names without their email");
  assert.equal(saved.replied[`CDEV:${mine}`], mine, "but counts as having read that thread");

  await sidebar.lookSlack(() => {}, call);
  assert.equal(asked.filter((a) => a.tool.endsWith("slack_read_user_profile")).length, 1, "your member ID is kept");
});

test("the Calendar view's iCal feed gives today's events, repeats and moved occurrences included, in /today's shape", () => {
  const now = new Date(2026, 9, 28, 12, 0);
  const ics = [
    "BEGIN:VCALENDAR", "VERSION:2.0", "X-WR-CALNAME:me@example.com",
    "BEGIN:VTIMEZONE", "TZID:America/New_York",
    "BEGIN:DAYLIGHT", "TZOFFSETFROM:-0500", "TZOFFSETTO:-0400", "TZNAME:EDT", "DTSTART:19700308T020000", "RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=2SU", "END:DAYLIGHT",
    "BEGIN:STANDARD", "TZOFFSETFROM:-0400", "TZOFFSETTO:-0500", "TZNAME:EST", "DTSTART:19701101T020000", "RRULE:FREQ=YEARLY;BYMONTH=11;BYDAY=1SU", "END:STANDARD",
    "END:VTIMEZONE",
    // Every weekday at 1 PM, since September, the call in Google's own field.
    "BEGIN:VEVENT", "UID:standup@google.com", "SUMMARY:Standup", "DTSTART;TZID=America/New_York:20260901T130000", "DTEND;TZID=America/New_York:20260901T131500",
    "RRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR", "X-GOOGLE-CONFERENCE:https://meet.google.com/abc-defg-hij",
    "ATTENDEE;PARTSTAT=ACCEPTED:mailto:me@example.com", "END:VEVENT",
    // Every Wednesday at 3 PM, but today's moved to 4 PM, a Zoom link in its location.
    "BEGIN:VEVENT", "UID:review@google.com", "SUMMARY:Review", "DTSTART;TZID=America/New_York:20261007T150000", "DTEND;TZID=America/New_York:20261007T160000",
    "RRULE:FREQ=WEEKLY;BYDAY=WE", "ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:me@example.com", "END:VEVENT",
    "BEGIN:VEVENT", "UID:review@google.com", "RECURRENCE-ID;TZID=America/New_York:20261028T150000", "SUMMARY:Review (moved)",
    "DTSTART;TZID=America/New_York:20261028T160000", "DTEND;TZID=America/New_York:20261028T170000", "LOCATION:https://acme.zoom.us/j/123?pwd=x",
    "ATTENDEE;PARTSTAT=TENTATIVE:mailto:me@example.com", "END:VEVENT",
    "BEGIN:VEVENT", "UID:nope@google.com", "SUMMARY:Declined", "DTSTART:20261028T170000Z", "DTEND:20261028T180000Z", "ATTENDEE;PARTSTAT=DECLINED:mailto:me@example.com", "END:VEVENT",
    "BEGIN:VEVENT", "UID:ooo@google.com", "SUMMARY:Holiday", "DTSTART;VALUE=DATE:20261028", "DTEND;VALUE=DATE:20261029", "END:VEVENT",
    "BEGIN:VEVENT", "UID:old@google.com", "SUMMARY:Yesterday", "DTSTART:20261027T170000Z", "DTEND:20261027T180000Z", "END:VEVENT",
    "END:VCALENDAR",
  ].join("\r\n");
  const script = `process.env.TZ = "America/New_York"; const { dayFrom } = require(${JSON.stringify(path.join(__dirname, "../lib/sidebar/calendar-feed"))}); console.log(JSON.stringify(dayFrom(${JSON.stringify(ics)}, new Date(${now.getTime()}))))`;
  const day = JSON.parse(execFileSync("node", ["-e", script], { env: { ...process.env, TZ: "America/New_York" }, encoding: "utf8" }));
  const byTitle = Object.fromEntries(day.events.map((e) => [e.title, e]));
  assert.deepEqual(Object.keys(byTitle).sort(), ["Holiday", "Review (moved)", "Standup"]);
  assert.equal(byTitle.Standup.start, "2026-10-28T17:00:00.000Z");
  assert.equal(byTitle.Standup.you, "going");
  assert.equal(byTitle.Standup.join, "https://meet.google.com/abc-defg-hij");
  assert.equal(Buffer.from(byTitle.Standup.link.split("eid=")[1], "base64").toString(), "standup_20261028T170000Z me@example.com");
  assert.equal(byTitle["Review (moved)"].start, "2026-10-28T20:00:00.000Z");
  assert.equal(byTitle["Review (moved)"].you, "maybe");
  assert.equal(byTitle["Review (moved)"].join, "https://acme.zoom.us/j/123?pwd=x");
  assert.equal(byTitle.Holiday.allDay, true);
  assert.equal(byTitle.Holiday.you, "yours");
  // Google's own ID for each, which RSVP answers by: a moved occurrence by when it was meant to start.
  assert.equal(byTitle.Standup.id, "standup_20261028T170000Z");
  assert.equal(byTitle["Review (moved)"].id, "review_20261028T190000Z");
  assert.equal(byTitle["Review (moved)"].calendar, "me@example.com");
  assert.equal(Buffer.from(byTitle["Review (moved)"].link.split("eid=")[1], "base64").toString(), "review_20261028T190000Z me@example.com");
});

test("h and l move between calendar events that share a time, in the order they sit side by side", () => {
  const at = (h, m = 0) => new Date(2026, 9, 29, h, m).toISOString();
  const events = [
    { title: "Holiday", allDay: true },
    { title: "Offsite", allDay: true },
    { title: "Standup", start: at(13), end: at(13, 15) },
    { title: "Demos", start: at(13), end: at(14) },
    { title: "Lunch", start: at(13, 30), end: at(14) },
    { title: "Review", start: at(15), end: at(16) },
  ];
  const section = { calendar: true, items: events.map((e) => ({ title: e.title, event: e })) };
  const index = (title) => section.items.findIndex((i) => i.title === title);
  const step = (title, by) => section.items[sidebar.besideStep(section, index(title), by)].title;
  assert.equal(step("Demos", 1), "Standup", "the longer one sits first, the shorter beside it");
  assert.equal(step("Standup", -1), "Demos");
  assert.equal(step("Demos", 1), "Standup");
  assert.equal(step("Lunch", -1), "Demos", "Lunch overlaps Demos but not Standup");
  assert.equal(step("Review", 1), "Review", "nothing beside it, it stays");
  assert.equal(step("Holiday", 1), "Offsite", "all-day ones along their own row");
});

test("an event link leaves out a split series' _R part, as Google's links do", () => {
  const link = sidebar.linkOf("abc_R20261020T170000@google.com", "me@example.com", new Date("2026-10-28T17:00:00Z"), false, true);
  assert.equal(Buffer.from(link.split("eid=")[1], "base64").toString(), "abc_20261028T170000Z me@example.com");
});

test("the sidebar's notifications sit at its bottom under their own heading, and a click on one names where it goes", () => {
  const tabs = [{ id: "@1", index: 1, name: "work", state: "idle", session: "$1", sessionName: "s", panes: [], sidebars: [], claude: { sessionId: "x" } }];
  const out = sidebar.sidebarLines({ tabs, sections: [], cols: 40, rows: 20, view: { focused: false, cursor: "@1", here: "@1", frame: 0 }, notices: [{ owner: "notice:slack", title: "3 new on Slack", detail: "Ada · #general" }] });
  const plain = out.lines.map((l) => l.replace(/\x1b\[[0-9;]*m/g, "").trimEnd());
  assert.equal(out.lines.length, 20);
  assert.deepEqual(plain.slice(-4), [" " + "═".repeat(38), " NOTIFICATIONS", "  ● 3 new on Slack", "    Ada · #general"]);
  assert.deepEqual(out.owners.slice(-2), ["notice:slack", "notice:slack"]);
  // With the sidebar's cursor moved onto it, it is lit as a chosen tab is.
  const chosen = sidebar.sidebarLines({ tabs, sections: [], cols: 40, rows: 20, view: { focused: true, cursor: "notice:slack", here: "@1", frame: 0 }, notices: [{ owner: "notice:slack", title: "3 new on Slack", detail: "Ada · #general" }] });
  assert.match(chosen.lines.at(-2).replace(/\x1b\[[0-9;]*m/g, ""), /^▌ ● 3 new on Slack/);
  const none = sidebar.sidebarLines({ tabs, sections: [], cols: 40, rows: 20, view: { focused: false, cursor: "@1", here: "@1", frame: 0 } });
  assert.ok(!none.lines.some((l) => l.includes("NOTIFICATIONS")), "nothing new, no block");
});

test("n in the sidebar goes to the notifications when there are any, and t opens a new tab as n used to", async (t) => {
  const settingsFile = path.join(home, ".config/switchboard/settings.json");
  fs.mkdirSync(path.dirname(settingsFile), { recursive: true });
  // The view off, so going to them only moves the cursor.
  fs.writeFileSync(settingsFile, JSON.stringify({ notice_list: false }));
  sidebar.reloadSettings();
  const stateDir = sidebar.STATE_DIR;
  fs.mkdirSync(stateDir, { recursive: true });
  const window = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  const side = tmux("split-window", "-hb", "-d", "-t", window, "-P", "-F", "#{pane_id}");
  tmux("set", "-p", "-t", side, "@sidebar", "1");
  t.after(() => {
    tmux("kill-window", "-t", window);
    fs.rmSync(settingsFile, { force: true });
    fs.rmSync(path.join(stateDir, "slack.json"), { force: true });
    sidebar.reloadSettings();
  });
  const client = { kind: "sidebar", pane: side, socket: { write() {}, end() {} } };
  const session = tmux("display", "-p", "-t", window, "#{session_id}");
  fs.rmSync(path.join(stateDir, "slack.json"), { force: true });
  await new Promise((resolve) => setTimeout(resolve, 2_100));
  await sidebar.refresh();
  const before = sidebar.state.cursors.get(session);
  await sidebar.sidebarKey(client, { key: "n" });
  assert.equal(sidebar.state.cursors.get(session), before, "none, nothing happens");

  fs.writeFileSync(path.join(stateDir, "slack.json"), JSON.stringify({ checked: Date.now(), items: [{ kind: "dm", channel: "D1", where: "Dee", user: "U1", from: "Dee", text: "hi", ts: String(Date.now() / 1000) }] }));
  await new Promise((resolve) => setTimeout(resolve, 2_100));
  await sidebar.refresh();
  await sidebar.sidebarKey(client, { key: "n" });
  assert.equal(sidebar.state.cursors.get(session), "notice:slack");
});

test("a Slack message's menu offers a tab of its own for the thread, with Claude", async (t) => {
  const stateDir = sidebar.STATE_DIR;
  fs.mkdirSync(stateDir, { recursive: true });
  fs.writeFileSync(path.join(stateDir, "slack.json"), JSON.stringify({ checked: Date.now(), items: [{ kind: "dm", channel: "D1", where: "Dee", user: "U1", from: "Dee", text: "can you look?", ts: String(Date.now() / 1000), link: "https://x.slack.com/archives/D1/p1" }] }));
  t.after(() => fs.rmSync(path.join(stateDir, "slack.json"), { force: true }));
  const client = { kind: "mission", socket: { write() {}, end() {} }, view: { view: 0, cursor: 0 }, cols: 160, rows: 30, tmuxClient: "none" };
  client.view.view = sidebar.builtInViews(sidebar.state).findIndex((v) => v.slack) + 1;
  await sidebar.missionKey(client, { key: "Enter" });
  assert.ok(client.choose.choices.some((c) => c.key === "t" && c.label === "Open a tab for this thread with Claude"));
});

test("acting on a Slack message marks it seen, and only it", async (t) => {
  const stateDir = sidebar.STATE_DIR;
  fs.mkdirSync(stateDir, { recursive: true });
  const now = Date.now() / 1000;
  fs.writeFileSync(path.join(stateDir, "slack.json"), JSON.stringify({ checked: Date.now(), items: [
    { kind: "dm", channel: "D1", where: "Dee", user: "U1", from: "Dee", text: "first", ts: String(now - 20), link: "https://x.slack.com/archives/D1/p1" },
    { kind: "dm", channel: "D2", where: "Bo", user: "U2", from: "Bo", text: "second", ts: String(now - 10), link: "https://x.slack.com/archives/D2/p2" },
  ] }));
  ["slack-seen", "slack-seen-items.json"].forEach((name) => fs.rmSync(path.join(stateDir, name), { force: true }));
  t.after(() => ["slack.json", "slack-seen-items.json"].forEach((name) => fs.rmSync(path.join(stateDir, name), { force: true })));
  const client = { kind: "mission", socket: { write() {}, end() {} }, view: { view: 0, cursor: 0 }, cols: 160, rows: 30, tmuxClient: "none" };
  client.view.view = sidebar.builtInViews(sidebar.state).findIndex((v) => v.slack) + 1;
  const titles = () => sidebar.slackView([]).items.map((i) => i.title);
  assert.deepEqual(titles(), ["DM Bo · Bo", "DM Dee · Dee"], "both new");
  // The newest first: Bo's. Its menu's thread tab, which only opens a popup.
  await sidebar.missionKey(client, { key: "Enter" });
  client.choose.cursor = client.choose.choices.findIndex((c) => c.key === "t");
  await sidebar.missionKey(client, { key: "Enter" });
  assert.deepEqual(titles(), ["DM Dee · Dee", "DM Bo · Bo · seen"], "Bo's seen, below the new one");
});

test("moving onto the notifications focuses the Notifications window's sidebar, as browsing to a tab does, never its view", async (t) => {
  const stateDir = sidebar.STATE_DIR;
  fs.mkdirSync(stateDir, { recursive: true });
  const tab = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  const tabSide = tmux("split-window", "-hb", "-d", "-t", tab, "-P", "-F", "#{pane_id}");
  tmux("set", "-p", "-t", tabSide, "@sidebar", "1");
  // The Notifications window, with its view and its sidebar, the view active as a new window's would be.
  const notices = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}", "cat");
  const view = tmux("display", "-p", "-t", notices, "#{pane_id}");
  const noticesSide = tmux("split-window", "-hb", "-d", "-t", notices, "-P", "-F", "#{pane_id}", "cat");
  tmux("set", "-p", "-t", noticesSide, "@sidebar", "1");
  tmux("setw", "-t", notices, "@notices_window", "1");
  tmux("select-pane", "-t", view);
  t.after(() => {
    [tab, notices].forEach((w) => tmux("kill-window", "-t", w));
    fs.rmSync(path.join(stateDir, "slack.json"), { force: true });
  });
  fs.writeFileSync(path.join(stateDir, "slack.json"), JSON.stringify({ checked: Date.now(), items: [{ kind: "dm", channel: "D1", where: "Dee", user: "U1", from: "Dee", text: "hi", ts: String(Date.now() / 1000) }] }));
  tmux("select-window", "-t", tab);
  tmux("select-pane", "-t", tabSide);
  await new Promise((resolve) => setTimeout(resolve, 2_100));
  await sidebar.refresh();
  await sidebar.sidebarKey({ kind: "sidebar", pane: tabSide, socket: { write() {}, end() {} } }, { key: "n" });
  assert.equal(tmux("display", "-p", "-t", "test", "#{window_id}"), notices, "the Notifications window in front");
  assert.equal(tmux("display", "-p", "-t", notices, "#{pane_id}"), noticesSide, "its sidebar focused, not the view");
});

test("on the notifications, j and k in the sidebar step through each one, not each kind, and up from the first goes back to the tabs", async (t) => {
  const stateDir = sidebar.STATE_DIR;
  fs.mkdirSync(stateDir, { recursive: true });
  const window = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  const side = tmux("split-window", "-hb", "-d", "-t", window, "-P", "-F", "#{pane_id}");
  tmux("set", "-p", "-t", side, "@sidebar", "1");
  t.after(() => {
    tmux("kill-window", "-t", window);
    fs.rmSync(path.join(stateDir, "slack.json"), { force: true });
  });
  const now = Date.now() / 1000;
  fs.writeFileSync(path.join(stateDir, "slack.json"), JSON.stringify({ checked: Date.now(), items: [
    { kind: "dm", channel: "D1", where: "Dee", user: "U1", from: "Dee", text: "first", ts: String(now - 20) },
    { kind: "dm", channel: "D2", where: "Bo", user: "U2", from: "Bo", text: "second", ts: String(now - 10) },
  ] }));
  await new Promise((resolve) => setTimeout(resolve, 2_100));
  await sidebar.refresh();
  const session = tmux("display", "-p", "-t", window, "#{session_id}");
  const client = { kind: "sidebar", pane: side, socket: { write() {}, end() {} } };
  sidebar.state.cursors.set(session, "notice:slack");
  sidebar.state.noticeCursor.set(session, 0);

  await sidebar.sidebarKey(client, { key: "j" });
  assert.equal(sidebar.state.noticeCursor.get(session), 1, "the second message, though both are Slack's");
  assert.equal(sidebar.state.cursors.get(session), "notice:slack");
  await sidebar.sidebarKey(client, { key: "j" });
  assert.equal(sidebar.state.noticeCursor.get(session), 1, "down from the last stays");
  await sidebar.sidebarKey(client, { key: "k" });
  await sidebar.sidebarKey(client, { key: "k" });
  assert.ok(!String(sidebar.state.cursors.get(session)).startsWith("notice:"), "up from the first, back on a tab");
});

test("a Slack message is drawn as Slack draws it: its lines, bullets hanging, quotes, code blocks, styles, links and mentions as their text", () => {
  const message = "Hey *team*, two things:\n• first _thing_ is ~old~ `x = 1`\n• a long bullet that wraps onto the next line and hangs under its text\n2. numbered\n> a quote\n```\nconst y = 2;\n```\nsee <https://x.example|the doc> &amp; <@U1|ada>";
  const lines = sidebar.slackLines(message, 40);
  const plain = lines.map((l) => l.replace(/\x1b\[[0-9;]*m/g, ""));
  assert.deepEqual(plain, [
    "Hey team, two things:",
    "• first thing is old x = 1",
    "• a long bullet that wraps onto the next",
    "  line and hangs under its text",
    "2. numbered",
    "▎ a quote",
    "  const y = 2;",
    "see the doc & @ada",
  ]);
  assert.match(lines[0], /\x1b\[1mteam/, "bold");
  assert.match(lines[1], /\x1b\[3mthing/, "italic");
  assert.match(lines[1], /\x1b\[9mold/, "struck");
});

test("a view whose integration isn't set up is still there to go to, and says how to set it up, whole", async (t) => {
  const stateDir = sidebar.STATE_DIR;
  fs.rmSync(path.join(stateDir, "calendar.json"), { force: true });
  // Calendar: no feed in the Keychain, nothing /today saved, no Calendar tab.
  const calendar = sidebar.builtInViews(sidebar.state).find((v) => v.title === "Calendar");
  assert.ok(calendar, "the Calendar view is there");
  assert.match(calendar.error, /google-calendar-ical/);
  assert.match(calendar.error, /Open a Calendar tab/);
  const drawn = sidebar.missionLines({ tabs: [], sections: [calendar], cols: 80, rows: 30, view: { view: 1, cursor: 0, frame: 0 }, config }).map((l) => l.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, ""));
  assert.ok(drawn.filter((l) => /\S/.test(l) && !/MISSION|keys|Calendar/.test(l)).length > 3, "wrapped over several lines, not cut");
  assert.ok(drawn.some((l) => l.includes("Secret address in iCal format")));

  // Linear: no key.
  const bin = fs.mkdtempSync(path.join(tmp, "nokey-bin-"));
  fs.writeFileSync(path.join(bin, "security"), "#!/bin/sh\nexit 44\n", { mode: 0o755 });
  const saved = { PATH: process.env.PATH, LINEAR_API_KEY: process.env.LINEAR_API_KEY };
  process.env.PATH = `${bin}:${saved.PATH}`;
  delete process.env.LINEAR_API_KEY;
  t.after(() => Object.assign(process.env, saved));
  await sidebar.fetchInbox();
  assert.match(sidebar.inboxView().error, /Linear isn't set up: .*linear-api-key/);

  // GitHub: no tab in a GitHub repository, then gh missing.
  await sidebar.fetchCi([{ path: tmp }], () => {});
  assert.match(sidebar.ciView().error, /None of your tabs is in a GitHub repository/);
  assert.match(sidebar.githubWhy({ code: "ENOENT", message: "spawn gh ENOENT" }), /brew install gh, then gh auth login/);
  assert.match(sidebar.githubWhy({ message: "You are not logged into any GitHub hosts. To log in, run: gh auth login" }), /isn't signed in: run gh auth login/);
});

test("the Notifications view lists whole messages, the chosen one marked, its top line going back", () => {
  const items = [
    { title: "Standup in 3m · ↵ joins", body: "1:00–1:15 PM\nAB-12" },
    { title: "DM Dee · Dee", body: "can you look at the deploy before lunch? it failed twice this morning and I'm not sure why" },
  ];
  const out = sidebar.noticeListLines(items, 1, 30, 20);
  const plain = out.lines.map((l) => l.replace(/\x1b\[[0-9;]*m/g, "").trimEnd());
  assert.equal(out.lines.length, 20);
  assert.match(plain[0], /‹ tabs +NOTIFICATIONS 2/);
  assert.deepEqual(out.owners.slice(0, 2), ["back", "back"]);
  assert.ok(plain.some((l) => l === "  Standup in 3m · ↵ joins"));
  assert.ok(plain.some((l) => l === "▌ DM Dee · Dee"), "the chosen one marked");
  assert.ok(plain.filter((l) => /deploy|morning|sure why/.test(l)).length > 1, "a long message wrapped whole, not cut");
  const dee = plain.findIndex((l) => l === "▌ DM Dee · Dee");
  assert.equal(out.owners[dee], 1, "each line says which item it is, for the pointer");
  assert.match(plain.at(-1), /jk move +↵ actions +esc back/);
  assert.doesNotMatch(out.lines[0], /\x1b\[7m/);
  assert.match(sidebar.noticeListLines(items, 1, 30, 20, undefined, true).lines[0], /\x1b\[7m.*‹ tabs/, "‹ tabs drawn focused once Tab is on it");
});

test("moving onto the notifications brings a Notifications window to the front, no tab, whose view lists them and opens each one's actions", async (t) => {
  const stateDir = sidebar.STATE_DIR;
  fs.mkdirSync(stateDir, { recursive: true });
  const now = Date.now() / 1000;
  fs.writeFileSync(path.join(stateDir, "slack.json"), JSON.stringify({ checked: Date.now(), items: [
    { kind: "dm", channel: "D1", where: "Dee", user: "U1", from: "Dee", text: "can you look at the deploy?", ts: String(now - 20), link: "https://x.slack.com/archives/D1/p1" },
    { kind: "mention", channel: "C1", where: "#general", user: "U2", from: "Ada", text: "@you the review", ts: String(now - 10), link: "https://x.slack.com/archives/C1/p2" },
  ] }));
  fs.rmSync(path.join(stateDir, "slack-seen"), { force: true });
  await new Promise((resolve) => setTimeout(resolve, 2_100));
  await sidebar.refresh();
  const session = tmux("display", "-p", "-t", "test", "#{session_id}");
  const window = await sidebar.noticesWindow(session, "cat");
  t.after(() => {
    try { tmux("kill-window", "-t", window); } catch {}
    fs.rmSync(path.join(stateDir, "slack.json"), { force: true });
  });
  assert.equal(tmux("show", "-wqv", "-t", window, "@notices_window"), "1");
  assert.equal(tmux("show", "-wqv", "-t", window, "window-status-format"), "", "out of the tab bar");
  assert.equal(await sidebar.noticesWindow(session, "cat"), window, "one a session");
  // M-p asking for a client tmux doesn't have finds no session and says so, without throwing.
  await sidebar.goToNotices("/dev/no-such-client");
  await sidebar.refresh();
  assert.ok(!sidebar.state.tabs.some((x) => x.id === window), "no tab");

  const items = sidebar.noticeItems();
  assert.deepEqual(items.map((i) => i.body), ["@you the review", "can you look at the deploy?"]);
  const sent = [];
  const pane = tmux("display", "-p", "-t", window, "#{pane_id}");
  const client = { kind: "notices", pane, socket: { write: (text) => sent.push(text), end() {} } };
  sidebar.draw(client);
  const shown = sent.at(-1).replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "");
  assert.match(shown, /NOTIFICATIONS 2/);
  assert.match(shown, /can you look at the deploy\?/);
  await sidebar.noticesKey(client, { key: "j" });
  assert.equal(sidebar.state.noticeCursor.get(session), 1);
  assert.equal(sidebar.state.cursors.get(session), "notice:slack", "the sidebar's cursor follows");
  await sidebar.noticesKey(client, { key: "Enter" });
  assert.ok(client.choose.choices.some((c) => c.label === "Open it in Slack"));
  assert.equal(client.choose.body, "can you look at the deploy?");
  assert.deepEqual(client.choose.choices.filter((c) => /^Dismiss/.test(c.label)).map((c) => c.label), ["Dismiss it"], "one Dismiss it, last");
  await sidebar.noticesKey(client, { key: "Escape" });
  assert.equal(client.choose, undefined);

  // Any action dismisses the notification: here, Dismiss it itself.
  await sidebar.noticesKey(client, { key: "Enter" });
  client.choose.cursor = client.choose.choices.findIndex((c) => c.key === "x");
  await sidebar.missionKey(client, { key: "Enter" });
  await new Promise((resolve) => setTimeout(resolve, 2_100));
  assert.deepEqual(sidebar.noticeItems().map((i) => i.body), ["@you the review"], "the chosen one is gone");
  fs.rmSync(path.join(stateDir, "slack-dismissed.json"), { force: true });
});

test("M-a goes from a finished tab to one waiting on you before the next finished one", (t) => {
  const stateDir = path.join(home, ".local/state/tmux-sidebar");
  fs.mkdirSync(stateDir, { recursive: true });
  const windows = ["cat", "cat", "cat"].map(() => tmux("new-window", "-d", "-P", "-F", "#{window_id}", "-c", tmp, "cat"));
  const [doneA, doneB, waiting] = windows;
  const pane = (w) => tmux("display", "-p", "-t", w, "#{pane_id}");
  const session = tmux("display", "-p", "-t", doneA, "#{session_id}");
  // The daemon's Needs you, as it writes it: waiting first, then done; and its
  // pid, alive (this test's own process), so the walk reads the list.
  const needs = path.join(stateDir, "needs");
  const pid = path.join(stateDir, "daemon.pid");
  const hadPid = fs.existsSync(pid) ? fs.readFileSync(pid, "utf8") : undefined;
  fs.writeFileSync(needs, [[waiting, pane(waiting), session, "waiting"], [doneA, pane(doneA), session, "done"], [doneB, pane(doneB), session, "done"]].map((r) => r.join("\t")).join("\n") + "\n");
  fs.writeFileSync(pid, String(process.pid));
  t.after(() => {
    windows.forEach((w) => { try { tmux("kill-window", "-t", w); } catch {} });
    fs.rmSync(needs, { force: true });
    if (hadPid === undefined) fs.rmSync(pid, { force: true }); else fs.writeFileSync(pid, hadPid);
  });
  const press = (from) => {
    tmux("select-window", "-t", from);
    execFileSync("bash", [path.join(__dirname, "../tmux/agent-state.sh"), "next"], { env: { ...process.env, TMUX_PANE: pane(from) } });
    return tmux("display", "-p", "-t", session, "#{window_id}");
  };
  assert.equal(press(doneA), waiting, "from a finished tab, the waiting one first");
  assert.equal(press(waiting), doneB, "then on to the finished one after the one it left");
  assert.equal(press(doneB), waiting, "the waiting one again before the next finished one");
  assert.equal(press(waiting), doneA, "and round, every finished tab reached");

  // Answered, the waiting tab leaves the list: the walk still picks up after
  // the finished one it left.
  assert.equal(press(doneA), waiting);
  fs.writeFileSync(needs, [[doneA, pane(doneA), session, "done"], [doneB, pane(doneB), session, "done"]].map((r) => r.join("\t")).join("\n") + "\n");
  assert.equal(press(waiting), doneB, "on to the finished one after the one it left");

  // A finished tab that clears once seen, mid-walk: the detour to a waiting
  // tab still goes on from the rest of the walk after it.
  const doneC = tmux("new-window", "-d", "-P", "-F", "#{window_id}", "-c", tmp, "cat");
  t.after(() => { try { tmux("kill-window", "-t", doneC); } catch {} });
  fs.writeFileSync(needs, [[doneA, pane(doneA), session, "done"], [doneB, pane(doneB), session, "done"], [doneC, pane(doneC), session, "done"]].map((r) => r.join("\t")).join("\n") + "\n");
  assert.equal(press(doneA), doneB);
  fs.writeFileSync(needs, [[waiting, pane(waiting), session, "waiting"], [doneA, pane(doneA), session, "done"], [doneC, pane(doneC), session, "done"]].map((r) => r.join("\t")).join("\n") + "\n");
  assert.equal(press(doneB), waiting, "doneB seen and gone: the waiting one first");
  assert.equal(press(waiting), doneC, "then on from where the walk was, not back to doneA");

  // The notifications, last on the list: the walk goes there after the
  // finished tabs, through sidebar.sh notices (a stand-in here), and a waiting
  // tab still comes before them.
  const opened = path.join(tmp, "notices-opened");
  const standIn = path.join(tmp, "notices-stand-in");
  fs.writeFileSync(standIn, `#!/bin/sh\necho "$@" >> '${opened}'\n`, { mode: 0o755 });
  fs.writeFileSync(needs, [[doneA, pane(doneA), session, "done"], ["notices", "-", "-", "notices"]].map((r) => r.join("\t")).join("\n") + "\n");
  tmux("select-window", "-t", doneA);
  execFileSync("bash", [path.join(__dirname, "../tmux/agent-state.sh"), "next"], { env: { ...process.env, TMUX_PANE: pane(doneA), NOTICES_COMMAND: standIn } });
  // One line, the client's name, which the tests' server has none of.
  assert.equal(fs.readFileSync(opened, "utf8"), "\n", "from the last finished tab, on to the notifications");
  fs.writeFileSync(needs, [[waiting, pane(waiting), session, "waiting"], ["notices", "-", "-", "notices"]].map((r) => r.join("\t")).join("\n") + "\n");
  assert.equal(press(doneB), waiting, "a waiting tab before the notifications");

  // Notifications that opened in the same window's sidebar (notice_list off):
  // the next press from there goes on round the list, not to them again.
  fs.writeFileSync(needs, [[doneA, pane(doneA), session, "done"], [doneB, pane(doneB), session, "done"], ["notices", "-", "-", "notices"]].map((r) => r.join("\t")).join("\n") + "\n");
  fs.rmSync(opened, { force: true });
  tmux("select-window", "-t", doneB);
  execFileSync("bash", [path.join(__dirname, "../tmux/agent-state.sh"), "next"], { env: { ...process.env, TMUX_PANE: pane(doneB), NOTICES_COMMAND: standIn } });
  assert.ok(fs.existsSync(opened), "from the last finished tab, the notifications");
  assert.equal(press(doneB), doneA, "then round to the first tab, from the window they opened in");
  // From the notifications in a tab's own sidebar (a stand-in: its pane marked
  // as one), M-a back to that same tab puts the cursor in its pane.
  fs.writeFileSync(needs, [[doneB, pane(doneB), session, "done"], ["notices", "-", "-", "notices"]].map((r) => r.join("\t")).join("\n") + "\n");
  const mainB = pane(doneB);
  const side = tmux("split-window", "-d", "-t", doneB, "-P", "-F", "#{pane_id}", "cat");
  tmux("set-option", "-p", "-t", side, "@sidebar", "1");
  tmux("set-option", "-g", "@agent_walk_window", "notices");
  tmux("set-option", "-g", "@agent_notices_from", doneB);
  tmux("select-window", "-t", doneB);
  tmux("select-pane", "-t", side);
  execFileSync("bash", [path.join(__dirname, "../tmux/agent-state.sh"), "next"], { env: { ...process.env, TMUX_PANE: side, NOTICES_COMMAND: standIn } });
  assert.equal(tmux("display", "-p", "-t", doneB, "#{pane_id}"), mainB, "the tab's pane, not its sidebar");
  // A pinned tab with no Claude pane ("-"): its pane that isn't the sidebar.
  fs.writeFileSync(needs, [[doneB, "-", session, "pinned"], ["notices", "-", "-", "notices"]].map((r) => r.join("\t")).join("\n") + "\n");
  tmux("set-option", "-g", "@agent_walk_window", "notices");
  tmux("select-pane", "-t", side);
  execFileSync("bash", [path.join(__dirname, "../tmux/agent-state.sh"), "next"], { env: { ...process.env, TMUX_PANE: side, NOTICES_COMMAND: standIn } });
  assert.equal(tmux("display", "-p", "-t", doneB, "#{pane_id}"), mainB, "a pinned shell tab's own pane too");
  tmux("kill-pane", "-t", side);
  // The notifications all that needs you: M-a opens them every time, from
  // the window they last opened in too.
  fs.writeFileSync(needs, [["notices", "-", "-", "notices"]].map((r) => r.join("\t")).join("\n") + "\n");
  [1, 2].forEach(() => {
    fs.rmSync(opened, { force: true });
    tmux("select-window", "-t", doneB);
    execFileSync("bash", [path.join(__dirname, "../tmux/agent-state.sh"), "next"], { env: { ...process.env, TMUX_PANE: pane(doneB), NOTICES_COMMAND: standIn } });
    assert.ok(fs.existsSync(opened), "the notifications, again");
  });
});

test("M-c puts the cursor in the pane running Claude in the tab, and says so when there is none", async (t) => {
  const script = path.join(__dirname, "../tmux/claude-focus.sh");
  const window = tmux("new-window", "-d", "-P", "-F", "#{window_id}", "-c", tmp, "cat");
  t.after(() => {
    try { tmux("kill-window", "-t", window); } catch {}
  });
  const first = tmux("display", "-p", "-t", window, "#{pane_id}");
  // A pane running node, with a session file for its process, as Claude Code
  // writes one: Claude, as claude-pane.sh tells it.
  const claude = tmux("split-window", "-d", "-t", window, "-P", "-F", "#{pane_id}", "node -e 'setTimeout(() => {}, 60000)'");
  await new Promise((resolve) => setTimeout(resolve, 500));
  const tty = tmux("display", "-p", "-t", claude, "#{pane_tty}");
  const pid = execFileSync("ps", ["-t", tty.replace("/dev/", ""), "-o", "pid=,comm="], { encoding: "utf8" }).split("\n").find((l) => /node/.test(l)).trim().split(/\s+/)[0];
  fs.mkdirSync(path.join(home, ".claude/sessions"), { recursive: true });
  const session = path.join(home, ".claude/sessions", `${pid}.json`);
  fs.writeFileSync(session, JSON.stringify({ pid: Number(pid), sessionId: "focus-test" }));
  t.after(() => fs.rmSync(session, { force: true }));
  tmux("select-pane", "-t", first);
  execFileSync("bash", [script, window]);
  assert.equal(tmux("display", "-p", "-t", window, "#{pane_id}"), claude, "the cursor in Claude's pane");

  // With no Claude, nothing moves.
  fs.rmSync(session);
  tmux("select-pane", "-t", first);
  execFileSync("bash", [script, window]);
  assert.equal(tmux("display", "-p", "-t", window, "#{pane_id}"), first);
});

test("a dismissed meeting stays dismissed whichever source the day is read from, and only that meeting", () => {
  const start = "2026-10-30T18:00:00Z";
  const fromFeed = { id: "a1", title: "Interview", start };
  const other = { id: "b2", title: "Interview", start };
  // The saved day writes the same instant with its offset.
  const saved = { title: "Interview", start: "2026-10-30T14:00:00-04:00" };
  const gone = new Set(sidebar.meetingKeys(fromFeed));
  assert.ok(sidebar.meetingGone(fromFeed, gone));
  assert.ok(!sidebar.meetingGone(other, gone), "another of the same title and time stays");
  assert.ok(sidebar.meetingGone(saved, gone), "as the saved day shows it");
  const goneSaved = new Set(sidebar.meetingKeys(saved));
  assert.ok(sidebar.meetingGone(fromFeed, goneSaved), "dismissed from the saved day, it stays gone once the feed loads");
});

test("Enter on an item in a built-in view offers its actions as a menu, its usual one first", async (t) => {
  fs.rmSync(path.join(sidebar.STATE_DIR, "closed.jsonl"), { force: true });
  sidebar.resetClosed();
  const realNow = Date.now;
  t.after(() => (Date.now = realNow));
  const gone = { id: "@91", index: 9, name: "ABC-2: Thing", state: "idle", path: "/work/abc-2", claude: { sessionId: "5555" } };
  Date.now = () => realNow() - 60_000;
  sidebar.record([]);
  sidebar.record([gone]);
  Date.now = realNow;
  sidebar.record([]);
  const sent = [];
  const client = { kind: "mission", socket: { write: (text) => sent.push(text), end() {} }, view: { view: 0, cursor: 0 }, cols: 160, rows: 30, tmuxClient: "none" };
  client.view.view = sidebar.builtInViews(sidebar.state).findIndex((v) => v.title === "Closed") + 1;
  await sidebar.missionKey(client, { key: "Enter" });
  assert.deepEqual(client.choose.choices.map((c) => [c.key, c.label]), [["r", "Reopen it, resuming its Claude"], ["x", "Remove it from this list"], ["y", "Copy its directory"]]);
  await sidebar.missionKey(client, { key: "Escape" });
  assert.equal(client.choose, undefined, "any other key closes it");
});

test("the CI view finds a pull request's failing checks and the log line that says why", () => {
  const pr = { commits: { nodes: [{ commit: { statusCheckRollup: { contexts: { nodes: [
    { __typename: "CheckRun", name: "lint", conclusion: "SUCCESS", detailsUrl: "https://github.com/o/r/actions/runs/1/job/10" },
    { __typename: "CheckRun", name: "test", conclusion: "FAILURE", detailsUrl: "https://github.com/o/r/actions/runs/1/job/11" },
    { __typename: "StatusContext", context: "deploy/preview", state: "ERROR", targetUrl: "https://ci.example/2" },
  ] } } } }] } };
  assert.deepEqual(sidebar.failingOf(pr), [
    { name: "test", url: "https://github.com/o/r/actions/runs/1/job/11", job: "11" },
    { name: "deploy/preview", url: "https://ci.example/2", job: undefined },
  ]);
  const log = [
    "2026-10-28T10:00:00.0000000Z \x1b[36mRunning tests\x1b[0m",
    "2026-10-28T10:00:01.0000000Z some error happened in a dependency, ignored",
    "2026-10-28T10:00:02.0000000Z FAIL src/user.test.ts > creates a user",
    "2026-10-28T10:00:03.0000000Z  ELIFECYCLE  Command failed with exit code 1.",
    "2026-10-28T10:00:04.0000000Z ##[error]Process completed with exit code 1.",
  ].join("\n");
  assert.equal(sidebar.whyFrom(log), "FAIL src/user.test.ts > creates a user", "the tool's own failure, not the runner's");
});

test("the Inbox lists unread Linear mentions, comments and assignments, newest first, leaving out status changes", () => {
  const issue = { identifier: "AB-1", title: "Do it", url: "https://linear.app/x/issue/AB-1" };
  const items = sidebar.inboxFrom([
    { id: "1", type: "issueStatusChanged", createdAt: "2026-10-28T10:00:00Z", actor: { name: "Ada" }, issue },
    { id: "2", type: "issueAssignedToYou", createdAt: "2026-10-28T11:00:00Z", actor: { name: "Ada" }, issue },
    { id: "3", type: "issueCommentMention", createdAt: "2026-10-28T12:00:00Z", actor: { name: "Lin" }, issue, comment: { body: "can you\nlook", url: "https://linear.app/x/issue/AB-1#comment-1" } },
    { id: "4", type: "issueMention", createdAt: "2026-10-28T13:00:00Z", readAt: "2026-10-28T13:05:00Z", actor: { name: "Bo" }, issue },
    { id: "5", type: "issueNewComment", createdAt: "2026-10-28T14:00:00Z", snoozedUntilAt: "2999-01-01T00:00:00Z", actor: { name: "Cy" }, issue },
  ]);
  assert.deepEqual(items.map((i) => [i.notification, i.detail, i.url, i.state]), [
    ["3", "Lin mentioned you in a comment: can you look", "https://linear.app/x/issue/AB-1#comment-1", "attention"],
    ["2", "Ada assigned it to you", "https://linear.app/x/issue/AB-1", "attention"],
  ], "not a status change, nor one read or snoozed");
  assert.equal(items[0].ticket, "AB-1");
});

test("a meeting's description gives the tickets and pull requests it names, links first", () => {
  const refs = sidebar.refsFrom("Agenda: <a href=\"https://linear.app/acme/issue/AB-12/fix-it\">AB-12</a>, https://github.com/o/repo/pull/45 and CD-7; UTF-8 isn't one");
  assert.deepEqual(refs, [
    { id: "AB-12", url: "https://linear.app/acme/issue/AB-12/fix-it" },
    { id: "repo#45", url: "https://github.com/o/repo/pull/45" },
    { id: "CD-7" },
  ]);
});

test("today's todo note is written once a weekday past its hour, from the notes' open items and the views, by Claude", async (t) => {
  const notes = path.join(tmp, "todo-notes");
  fs.mkdirSync(path.join(notes, "Work"), { recursive: true });
  fs.mkdirSync(path.join(notes, ".trash"), { recursive: true });
  fs.mkdirSync(path.join(notes, "Standups"), { recursive: true });
  fs.writeFileSync(path.join(notes, "Work", "Plan.md"), "# Plan\n\n- [ ] Ship the export\n* [ ] Star item\n\n```\n- [ ] in code\n```\n  - [ ] a sub-item\n- [x] Done already\n\n## Later\n\n- [ ] Someday thing\n\n### Sub of later\n\n- [ ] Still parked\n\n## Next\n\n- [ ] Write the [docs][1]\n\n# Someday\n\n- [ ] Big parked one\n\n[1]: https://example.com/docs\n\n```\n[1]: https://wrong.example\n```\n");
  fs.writeFileSync(path.join(notes, ".trash", "Old.md"), "# Old\n\n- [ ] Deleted item\n");
  fs.writeFileSync(path.join(notes, "Standups", "Standup 2026-11-02.md"), "# Standup\n\n- [ ] Standup item\n");
  fs.writeFileSync(path.join(tmp, "outside.md"), "# Outside\n\n- [ ] Not a note\n");
  fs.symlinkSync(path.join(tmp, "outside.md"), path.join(notes, "Linked.md"));
  fs.writeFileSync(path.join(notes, ".hidden.md"), "# Hidden\n\n- [ ] Hidden item\n");
  const saved = process.env.NOTES_DIR;
  process.env.NOTES_DIR = notes;
  t.after(() => (saved === undefined ? delete process.env.NOTES_DIR : (process.env.NOTES_DIR = saved)));

  assert.deepEqual(sidebar.openItems(), [{ note: "Plan", items: ["Ship the export", "Star item", "Write the [docs](https://example.com/docs)"] }], "top-level unticked items, parked sections, deleted notes and made notes left out");

  const tuesday = new Date(2026, 10, 3, 9, 30);
  assert.equal(sidebar.todoDue(tuesday, 9), true);
  assert.equal(sidebar.todoDue(new Date(2026, 10, 3, 8, 59), 9), false, "not before its hour");
  assert.equal(sidebar.todoDue(new Date(2026, 10, 7, 10), 9), false, "not on a Saturday");
  assert.equal(sidebar.todoDue(tuesday, 9, tuesday.getTime() - 60_000), false, "not again within the retry wait");

  const prompts = [];
  const ask = async (prompt) => {
    prompts.push(prompt);
    return "## First\n\n- [ ] Ship the export";
  };
  await sidebar.writeTodo([], { github: false, calendar: ["9:30 AM Standup"], tickets: ["ABC-1 Fix it · Todo"], slack: [], missing: ["Slack"] }, tuesday, () => {}, ask);
  const file = path.join(notes, "Todos", "Today's Todo 2026-11-03.md");
  assert.equal(fs.readFileSync(file, "utf8"), "# Today's Todo 2026-11-03\n\n## First\n\n- [ ] Ship the export\n\n_Left out, since it couldn't be read: Slack._\n");
  assert.match(prompts[0], /Plan:\n  - Ship the export\n  - Star item\n  - Write the \[docs\]\(https:\/\/example\.com\/docs\)/);
  assert.match(prompts[0], /## Today's calendar\n```\n9:30 AM Standup/);
  assert.match(prompts[0], /## Linear tickets assigned to me/);
  assert.doesNotMatch(prompts[0], /Slack messages|Reviews asked|Someday thing|Still parked|in code|Big parked one/, "an empty source is left out");
  assert.equal(sidebar.todoDue(tuesday, 9), false, "written today");
  await sidebar.writeTodo([], {}, tuesday, () => {}, ask);
  assert.equal(prompts.length, 1, "a note already there is never written over, nor asked for again");
});

test("the standup note lists the last working day's PRs, reviews, ticket moves and commits, then today's work", () => {
  const monday = new Date(2026, 9, 26, 7);
  const friday = (hour) => new Date(2026, 9, 23, hour).toISOString();
  const repository = { nameWithOwner: "o/repo" };
  const note = sidebar.standupFrom({
    github: {
      viewer: { login: "me" },
      mine: { nodes: [
        { number: 1, title: "Merged it", url: "https://github.com/o/repo/pull/1", state: "MERGED", createdAt: friday(9), mergedAt: friday(15), repository },
        { number: 2, title: "Opened it", url: "https://github.com/o/repo/pull/2", state: "OPEN", isDraft: true, createdAt: friday(10), repository },
        { number: 3, title: "Older", url: "https://github.com/o/repo/pull/3", state: "OPEN", createdAt: "2026-10-01T00:00:00Z", repository },
        { number: 4, title: "Merged Thursday", url: "https://github.com/o/repo/pull/4", state: "MERGED", createdAt: "2026-10-01T00:00:00Z", mergedAt: new Date(2026, 9, 22, 12).toISOString(), repository },
      ] },
      reviewed: { nodes: [
        { number: 5, title: "Theirs", url: "https://github.com/o/repo/pull/5", repository, reviews: { nodes: [{ author: { login: "me" }, submittedAt: friday(11) }] } },
        { number: 6, title: "Reviewed by someone else", url: "https://github.com/o/repo/pull/6", repository, reviews: { nodes: [{ author: { login: "ada" }, submittedAt: friday(11) }] } },
      ] },
      waiting: { nodes: [{ number: 7, title: "Asked of me", url: "https://github.com/o/repo/pull/7", repository }] },
    },
    linear: {
      changed: { nodes: [
        { identifier: "AB-1", title: "Moved", url: "https://linear.app/x/issue/AB-1", history: { nodes: [
          { createdAt: friday(16), fromState: { name: "In Review" }, toState: { name: "Done" } },
          { createdAt: friday(9), fromState: { name: "Todo" }, toState: { name: "In Review" } },
          { createdAt: friday(12), fromState: null, toState: null },
        ] } },
        { identifier: "AB-2", title: "Only edited", url: "https://linear.app/x/issue/AB-2", history: { nodes: [{ createdAt: friday(9), fromState: null, toState: null }] } },
      ] },
      started: { nodes: [{ identifier: "AB-3", title: "Going", url: "https://linear.app/x/issue/AB-3", state: { name: "In Progress" } }] },
    },
    commits: [{ repo: "o/repo", subjects: ["Do a", "Do b"] }, { repo: "o/quiet", subjects: [] }],
  }, monday);
  assert.equal(note, `# Standup 2026-10-26

## Friday, Oct 23

- Merged [repo#1 Merged it](https://github.com/o/repo/pull/1)
- Opened [repo#2 Opened it](https://github.com/o/repo/pull/2) (draft)
- Worked on [repo#3 Older](https://github.com/o/repo/pull/3)
- Reviewed [repo#5 Theirs](https://github.com/o/repo/pull/5)
- [AB-1 Moved](https://linear.app/x/issue/AB-1): Todo → Done
- Commits in o/repo: Do a; Do b

## Today

- [AB-3 Going](https://linear.app/x/issue/AB-3) (In Progress)
- Review [repo#7 Asked of me](https://github.com/o/repo/pull/7)

## Blockers

- 
`, "Monday's note covers Friday, from where each ticket started to where it ended");

  const notes = fs.mkdtempSync("/tmp/standup-");
  process.env.NOTES_DIR = notes;
  assert.equal(sidebar.standupDue(new Date(2026, 9, 26, 5)), false, "not before six");
  assert.equal(sidebar.standupDue(new Date(2026, 9, 25, 9)), false, "nor on a weekend");
  assert.equal(sidebar.standupDue(monday, monday - 60_000), false, "nor a minute after trying");
  assert.equal(sidebar.standupDue(monday), true);
  fs.mkdirSync(path.join(notes, "Standups"));
  fs.writeFileSync(path.join(notes, "Standups", "Standup 2026-10-26.md"), "mine");
  assert.equal(sidebar.standupDue(monday), false, "nor once the note is there");
  delete process.env.NOTES_DIR;
  fs.rmSync(notes, { recursive: true, force: true });

  const told = sidebar.standupFrom({ commits: [{ repo: "o/repo", subjects: ["Do a"] }] }, monday, "I shipped a.\n");
  assert.match(told, /^## Friday, Oct 23\n\nI shipped a\.\n\n- Commits in o\/repo: Do a\n/m, "the summary over the day's list");
  assert.match(sidebar.summaryPrompt("- Merged it"), /<day>\n- Merged it\n<\/day>$/, "Claude reads the list alone, as data");
});

test("the standup's summary is asked of Claude for a day with work in it, and the note is written without it when Claude can't", async () => {
  const notes = fs.mkdtempSync("/tmp/standup-summary-");
  process.env.NOTES_DIR = notes;
  const repo = path.join(notes, "repo");
  fs.mkdirSync(repo);
  const git = (...args) => execFileSync("git", ["-C", repo, ...args], { encoding: "utf8", env: { ...process.env, GIT_AUTHOR_DATE: new Date(2026, 9, 23, 11).toISOString(), GIT_COMMITTER_DATE: new Date(2026, 9, 23, 11).toISOString() } });
  git("init", "-q");
  git("config", "user.email", "me@example.com");
  git("config", "user.name", "Me");
  git("remote", "add", "origin", "https://github.com/o/repo.git");
  git("commit", "-q", "--allow-empty", "-m", "Do a");
  const tabs = [{ path: repo }];
  const asked = [];
  try {
    await sidebar.writeStandup(tabs, { github: false, linear: false }, new Date(2026, 9, 26, 7), () => {}, async (prompt, opts) => {
      asked.push([prompt, opts.model]);
      return "I did a.";
    });
    assert.equal(asked.length, 1);
    assert.equal(asked[0][1], "haiku");
    assert.match(asked[0][0], /Commits in .*: Do a/);
    assert.match(fs.readFileSync(path.join(notes, "Standups", "Standup 2026-10-26.md"), "utf8"), /## Friday, Oct 23\n\nI did a\.\n\n- Commits/);

    await sidebar.writeStandup(tabs, { github: false, linear: false }, new Date(2026, 9, 27, 7), () => {}, async () => {
      throw new Error("no Claude");
    });
    assert.match(fs.readFileSync(path.join(notes, "Standups", "Standup 2026-10-27.md"), "utf8"), /## Monday, Oct 26\n\n- Nothing found/, "a day with nothing isn't summed up");

    await sidebar.writeStandup(tabs, { github: false, linear: false, summary: false }, new Date(2026, 9, 28, 7), () => {}, async () => {
      throw new Error("not asked with the setting off");
    });
    fs.rmSync(path.join(notes, "Standups", "Standup 2026-10-26.md"));
    await sidebar.writeStandup(tabs, { github: false, linear: false }, new Date(2026, 9, 26, 7), () => {}, async () => {
      throw new Error("Claude is down");
    });
    assert.match(fs.readFileSync(path.join(notes, "Standups", "Standup 2026-10-26.md"), "utf8"), /## Friday, Oct 23\n\n- Commits/, "written all the same without one");
  } finally {
    delete process.env.NOTES_DIR;
    fs.rmSync(notes, { recursive: true, force: true });
  }
});

test("meeting notes write each of your Fathom meetings once, its action items yours first, and wait for a summary not ready yet", async () => {
  const notes = fs.mkdtempSync("/tmp/meetings-");
  process.env.NOTES_DIR = notes;
  // Fathom as its connector answers, one meeting someone else's alone.
  const listed = [
    "Found 3 meeting(s). Each entry has recording_id and url.",
    "",
    "- Plan / review | 2026-10-27 | id: 11 | url: https://fathom.video/calls/111 | recorded by Ann Lee | Me Myself, Bo Park",
    "  Summary:",
    "  ## Meeting Purpose cut short...",
    "  Action items:",
    "    • [ ] Send the deck — assigned to Bo Park (https://fathom.video/calls/111?timestamp=65.9)",
    "    • [ ] Fix the bug — assigned to Me Myself (he/him) (https://fathom.video/calls/111?timestamp=600.2)",
    "    • [ ] Book the room — assigned to Me Myselfson (https://fathom.video/calls/111?timestamp=30)",
    "    • [ ] Call the vendor — assigned to Ann Me Myself (https://fathom.video/calls/111?timestamp=50)",
    "    • [ ] Order lunch — assigned to Someone Else (me@example.com) (https://fathom.video/calls/111?timestamp=40)",
    "- Not mine | 2026-10-27 | id: 12 | url: https://fathom.video/calls/112 | recorded by Ann Lee | Bo Park",
    "- Just ended | 2026-10-28 | id: 13 | url: https://fathom.video/calls/113 | recorded by me@example.com",
  ].join("\n");
  const asked = [];
  const call = async (tool, input) => {
    asked.push([tool.replace("mcp__claude_ai_Fathom__", ""), input.recording_id]);
    if (tool.endsWith("get_identity")) return JSON.stringify({ name: "me@example.com", email: "me@example.com" });
    if (tool.endsWith("list_meetings")) return listed;
    if (tool.endsWith("get_meeting_summary")) {
      return input.recording_id === 13
        ? ""
        : "## Meeting Purpose\n\n[Agree the plan.](https://fathom.video/calls/111?tab=summary&timestamp=0.0)\n\n## Key Takeaways\n\n  - [**Ship it:** on Friday.](https://fathom.video/calls/111?tab=summary&timestamp=9.0)\n\n### A topic\n\n  - [Said once.](https://fathom.video/calls/111?tab=summary&timestamp=3.0)\n";
    }
    return "[00:00](https://fathom.video/calls/111?timestamp=0) Ann Lee: Hello.\n[00:04](https://fathom.video/calls/111?timestamp=4) Bo Park: Hi.";
  };
  const state = path.join(sidebar.STATE_DIR, "meetings.json");
  fs.rmSync(state, { force: true });
  fs.mkdirSync(path.dirname(state), { recursive: true });
  fs.writeFileSync(state, JSON.stringify({ written: [], me: ["me@example.com", "Me Myself"] }));
  const now = new Date(2026, 9, 28, 10);

  await sidebar.writeMeetings(now, () => {}, call);
  assert.deepEqual(fs.readdirSync(path.join(notes, "Meetings")), ["Plan - review 2026-10-27.md"], "mine alone, the one with no summary yet waiting");
  assert.equal(fs.readFileSync(path.join(notes, "Meetings", "Plan - review 2026-10-27.md"), "utf8"), `# Plan / review 2026-10-27

[Recording](https://fathom.video/calls/111) · recorded by Ann Lee · Me Myself, Bo Park

## Action items

- [ ] Fix the bug (Me Myself) [10:00](https://fathom.video/calls/111?timestamp=600.2)
- [ ] Order lunch (Someone Else) [0:40](https://fathom.video/calls/111?timestamp=40)
- Send the deck (Bo Park) [1:05](https://fathom.video/calls/111?timestamp=65.9)
- Book the room (Me Myselfson) [0:30](https://fathom.video/calls/111?timestamp=30)
- Call the vendor (Ann Me Myself) [0:50](https://fathom.video/calls/111?timestamp=50)

## Summary

Agree the plan.

### Key Takeaways

- **Ship it:** on Friday.

#### A topic

- Said once.

## Transcript

[00:00](https://fathom.video/calls/111?timestamp=0) Ann Lee: Hello.

[00:04](https://fathom.video/calls/111?timestamp=4) Bo Park: Hi.
`);
  assert.deepEqual(JSON.parse(fs.readFileSync(state, "utf8")).written, [11]);
  // It is a new note for the sidebar's notifications until Notes shows it.
  assert.deepEqual(sidebar.newNotes().map((n) => [n.path, n.title, n.source]), [["Meetings/Plan - review 2026-10-27.md", "Plan / review 2026-10-27", "Fathom"]]);
  fs.writeFileSync(path.join(notes, ".notes.json"), JSON.stringify({ last: "Meetings/Plan - review 2026-10-27.md" }));
  assert.deepEqual(sidebar.newNotes(), [], "read once Notes shows it");
  fs.rmSync(path.join(notes, ".notes.json"));

  // The next look writes nothing twice, and asks again only for the waiting one.
  asked.length = 0;
  await sidebar.writeMeetings(now, () => {}, call);
  assert.deepEqual(asked, [["list_meetings", undefined], ["get_meeting_summary", 13]]);
  assert.equal(sidebar.meetingsDue(new Date(2026, 9, 28, 6, 30)), false, "not before seven");
  assert.equal(sidebar.meetingsDue(now, now - 60_000), false, "nor a minute after looking");
  assert.equal(sidebar.meetingsDue(now), true);
  // In the half hour after a meeting on the calendar ends, every five minutes.
  const ended = [{ start: new Date(now - 40 * 60_000).toISOString(), end: new Date(now - 10 * 60_000).toISOString() }];
  assert.equal(sidebar.meetingsDue(now, now - 6 * 60_000, ended), true, "a meeting just ended: again after five minutes");
  assert.equal(sidebar.meetingsDue(now, now - 4 * 60_000, ended), false);
  assert.equal(sidebar.meetingsDue(now, now - 6 * 60_000, [{ ...ended[0], end: new Date(now - 40 * 60_000).toISOString() }]), false, "over half an hour since, back to every thirty");
  assert.equal(sidebar.meetingsDue(now, now - 6 * 60_000, [{ allDay: true }]), false);
  delete process.env.NOTES_DIR;
  fs.rmSync(notes, { recursive: true, force: true });
});

test("a tab with a reminder waits under Reminders while nothing else places it, then needs you once it comes due", async () => {
  const window = tmux("new-window", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}");
  try {
    const later = Math.floor(Date.now() / 1000) + 90 * 60 + 30;
    tmux("setw", "-t", window, "@remind_at", String(later));
    tmux("setw", "-t", window, "@remind_note", "check the deploy");
    let tab = sidebar.tabsFrom(await sidebar.snapshot()).find((x) => x.id === window);
    assert.equal(tab.remindAt, later);
    assert.deepEqual(sidebar.grouped([tab]).map((g) => g.title), ["Reminders"]);
    assert.deepEqual(sidebar.missionGrid([tab], 100, 20).shelves[0].blocks.map((b) => b.stage.title), ["Reminders"]);
    assert.match(sidebar.stateWords(tab), /^remind in 1h 3\dm · check the deploy/);
    assert.match(sidebar.sidebarLines({ tabs: [tab], sections: [], cols: 50, rows: 10, view: { focused: false, cursor: window, frame: 0, here: window } }).lines.join(""), /⏰/);
    // Waiting on you or finished, its state wins: Needs you, with the
    // state's badge and the reminder's words; idle again, back to Reminders.
    const asking = { ...tab, state: "waiting", claude: { sessionId: "s" } };
    assert.deepEqual(sidebar.grouped([asking, { ...tab, id: "@98", state: "done", claude: { sessionId: "s" } }]).map((g) => [g.title, g.tabs.length]), [["Needs you", 2]]);
    assert.deepEqual(sidebar.missionGrid([asking], 100, 20).shelves[0].blocks.map((b) => b.stage.title), ["Needs you"]);
    assert.deepEqual(sidebar.needsYou([asking]).map((t) => t.id), [window]);
    assert.equal(sidebar.badgeState(asking), "waiting");
    assert.match(sidebar.stateWords(asking), /^remind in 1h 3\dm · check the deploy · waiting on you/);
    const answered = { ...asking, state: "idle" };
    assert.deepEqual(sidebar.grouped([answered]).map((g) => g.title), ["Reminders"]);
    assert.equal(sidebar.badgeState(answered), "later");
    assert.deepEqual(sidebar.missionGrid([answered], 100, 20).shelves[0].blocks.map((b) => b.stage.title), ["Reminders"]);
    // Working, it is under Working with the working badge, the reminder's
    // words still beside it; a merged or failing PR keeps its own group too.
    const busy = { ...tab, state: "working", claude: { sessionId: "s" } };
    assert.deepEqual(sidebar.grouped([busy]).map((g) => g.title), ["Working"]);
    assert.deepEqual(sidebar.missionGrid([busy], 100, 20).shelves[0].blocks.map((b) => b.stage.title), ["Working"]);
    assert.equal(sidebar.badgeState(busy), "working");
    assert.match(sidebar.stateWords(busy), /^remind in 1h 3\dm · check the deploy · working/);
    assert.deepEqual(sidebar.grouped([{ ...answered, merge: "merged" }]).map((g) => g.title), ["Merged"]);
    assert.deepEqual(sidebar.grouped([{ ...answered, prState: "OPEN", checks: { fail: 1 } }]).map((g) => g.title), ["Build failing"]);
    // Finished, Needs you, so M-a walks to it.
    const finished = { ...tab, state: "done", since: 1, claude: { sessionId: "s" } };
    assert.deepEqual(sidebar.missionGrid([finished], 100, 20).shelves[0].blocks.map((b) => b.stage.title), ["Needs you"]);
    assert.equal(sidebar.badgeState(finished), "done");
    assert.match(sidebar.stateWords(finished), /^remind in 1h 3\dm · check the deploy · finished/);

    // Come due, it needs you, first in Needs you with the pinned ones.
    tmux("setw", "-t", window, "@remind_at", String(Math.floor(Date.now() / 1000) - 5));
    tab = sidebar.tabsFrom(await sidebar.snapshot()).find((x) => x.id === window);
    const waiting = { ...tab, id: "@99", index: 99, name: "waiting", state: "waiting", since: 1, remindAt: undefined, remindNote: undefined };
    assert.deepEqual(sidebar.grouped([waiting, tab]).map((g) => [g.title, g.tabs.map((t) => t.id)]), [["Needs you", [window, "@99"]]]);
    assert.match(sidebar.stateWords(tab), /^reminder: check the deploy/);

    // With the reminders setting off, neither shows.
    fs.writeFileSync(path.join(home, ".config/switchboard/settings.json"), JSON.stringify({ reminders: false }));
    require("../lib/settings").reloadSettings?.();
    assert.deepEqual(sidebar.grouped([tab]).map((g) => g.title), ["Other"]);
  } finally {
    fs.rmSync(path.join(home, ".config/switchboard/settings.json"), { force: true });
    require("../lib/settings").reloadSettings?.();
    tmux("kill-window", "-t", window);
  }
});

test("remind.sh reads the times Slack's Remind me offers, and one typed in", () => {
  const script = path.join(__dirname, "../tmux/remind.sh");
  const now = Math.floor(new Date(2026, 10, 4, 14, 0, 0).getTime() / 1000);
  [
    ["In 1 hour", "Wed Nov 04 2026 15:00"],
    ["45m", "Wed Nov 04 2026 14:45"],
    ["2h", "Wed Nov 04 2026 16:00"],
    ["1d", "Thu Nov 05 2026 14:00"],
    ["3pm", "Wed Nov 04 2026 15:00"],
    ["9am", "Thu Nov 05 2026 09:00", "a time already past is tomorrow's"],
    ["15:30", "Wed Nov 04 2026 15:30"],
    ["Tomorrow at 9:00", "Thu Nov 05 2026 09:00"],
    ["Next Monday at 9:00", "Mon Nov 09 2026 09:00"],
    ["7", "", "a bare number names no time"],
    ["soon", ""],
  ].forEach(([text, expected, why]) => {
    const out = execFileSync("bash", [script, "at", text, String(now)], { encoding: "utf8" }).trim();
    assert.equal(out && new Date(Number(out) * 1000).toString().slice(0, 21), expected, why ?? text);
  });
});

test("a click takes a menu's option, answers a question's Yes or No, and outside the box closes it", async () => {
  const sent = [];
  const client = { kind: "mission", socket: { write: (text) => sent.push(text), end() {} }, view: { view: 0, cursor: 0 }, cols: 100, rows: 30, tmuxClient: "none" };
  const taken = [];
  client.choose = { title: "A tab", choices: [{ key: "a", label: "First", act: () => taken.push("first") }, { key: "b", label: "Second", act: () => taken.push("second") }], cursor: 0 };
  sidebar.draw(client);
  const rowOf = (label) => {
    return client.modal.tags.find((t) => t.tag.choice === (label === "First" ? 0 : 1)).y;
  };
  const x = client.modal.left + 4;
  await sidebar.missionKey(client, { mouse: { button: 0, x, y: rowOf("Second"), press: true } });
  assert.deepEqual(taken, ["second"], "one click on an option takes it");
  assert.equal(client.choose, undefined);

  client.choose = { title: "A tab", choices: [{ key: "a", label: "First", act: () => taken.push("first") }], cursor: 0 };
  sidebar.draw(client);
  await sidebar.missionKey(client, { mouse: { button: 0, x: 1, y: 1, press: true } });
  assert.equal(client.choose, undefined, "a click outside closes it");
  assert.deepEqual(taken, ["second"], "taking nothing");

  let yes = 0;
  client.confirm = { text: "Close it?", yes: () => { yes++; } };
  sidebar.draw(client);
  const row = client.modal.tags.find((t) => t.tag.yes).y;
  await sidebar.missionKey(client, { mouse: { button: 0, x: client.modal.left + 1 + 3, y: row, press: true } });
  assert.equal(yes, 1, "a click on Yes is yes");
  client.confirm = { text: "Close it?", yes: () => { yes++; } };
  sidebar.draw(client);
  await sidebar.missionKey(client, { mouse: { button: 0, x: client.modal.left + 1 + 9, y: row, press: true } });
  assert.equal(yes, 1, "and on No, no");
  assert.equal(client.confirm, undefined);

  // In M-j's picker, its filter typing: the box still takes the click.
  client.picker = true;
  client.filter = { text: "ab", typing: true };
  client.choose = { title: "A tab", choices: [{ key: "a", label: "First", act: () => taken.push("first") }, { key: "b", label: "Second", act: () => taken.push("second") }], cursor: 0 };
  sidebar.draw(client);
  await sidebar.missionKey(client, { mouse: { button: 65, x: client.modal.left + 4, y: client.modal.top + 1, press: true } });
  assert.equal(client.choose.cursor, 1, "the wheel moves the menu, not the filter");
  assert.equal(client.filter.text, "ab");
  sidebar.draw(client);
  await sidebar.missionKey(client, { mouse: { button: 0, x: client.modal.left + 4, y: client.modal.tags.find((t) => t.tag.choice === 0).y, press: true } });
  assert.deepEqual(taken, ["second", "first"], "and a click takes its option");
  assert.equal(client.filter.typing, true, "the filter typing again after");
  client.picker = false;
  client.filter = undefined;

  // The answer popup a opens from the sidebar: each option a row to click.
  const popup = { kind: "mission", only: true, socket: { write() {}, end() {} }, view: { view: 0, cursor: 0 }, cols: 80, rows: 24, tmuxClient: "none" };
  popup.answering = { tab: { index: 1, name: "t" }, found: { question: "Which?", options: [{ n: 1, label: "One" }, { n: 2, label: "Two" }] }, text: "", typing: false };
  sidebar.draw(popup);
  assert.deepEqual(popup.modal.tags.map((t) => t.tag.option), [1, 2], "the answer popup's options take a click");
  // A question too long for the popup is cut short, its options still shown.
  popup.answering.found.question = Array.from({ length: 40 }, (_, i) => `question line ${i}`).join("\n");
  sidebar.draw(popup);
  assert.deepEqual(popup.modal.tags.map((t) => t.tag.option), [1, 2], "the options shown under a long question");
});

test("in easy mode, every box, popup and mission control itself has a close button in its top-right corner, which a click on closes", async () => {
  const settingsFile = path.join(home, ".config/switchboard/settings.json");
  const menu = () => plain(sidebar.chooseModal({ title: "A tab", choices: [{ key: "a", label: "Go there" }], cursor: 0 }, 120, 40));
  assert.doesNotMatch(menu(), /×/, "none with easy mode off");
  fs.writeFileSync(settingsFile, JSON.stringify({ easy_keys: true }));
  require("../lib/settings").reloadSettings();
  try {
    assert.match(menu(), /╭─ A tab ─+ × ╮/, "a menu's top edge ends in it");
    assert.doesNotMatch(plain(sidebar.busyModal({ text: "Opening" }, 0, 120, 40)), /×/, "but not a box that can't be closed");
    assert.match(sidebar.helpLines("sidebar", 60, 30).map(plain)[0], /^╭─ Keys ─+ × ╮$/, "the sidebar's keys popup too");
    assert.match(sidebar.helpModal("mission", 120, 40, true), /\x1b\[7m[^╮]* × /, "the keys box's, lit while Tab has put focus on it");
    assert.match(sidebar.helpLines("sidebar", 60, 30, true)[0], /\x1b\[7m[^╮]* × /, "and the sidebar's keys popup's");

    let closed = 0;
    const client = { kind: "mission", socket: { write() {}, end: () => closed++ }, view: { view: 0, cursor: 0 }, cols: 100, rows: 30, tmuxClient: "none" };
    const taken = [];
    client.choose = { title: "A tab", choices: [{ key: "a", label: "First", act: () => taken.push("first") }], cursor: 0 };
    sidebar.draw(client);
    const { y, x0, x1 } = client.modal.close;
    assert.equal(x1 - x0, 2);
    await sidebar.missionKey(client, { mouse: { button: 0, x: x0 + 1, y, press: true } });
    assert.equal(client.choose, undefined, "a click on it closes the menu");
    assert.deepEqual(taken, [], "taking nothing");
    client.confirm = { text: "Close it?", yes: () => taken.push("yes") };
    sidebar.draw(client);
    await sidebar.missionKey(client, { mouse: { button: 0, x: client.modal.close.x1, y: client.modal.close.y, press: true } });
    assert.equal(client.confirm, undefined, "and a question, as no");
    assert.deepEqual(taken, []);

    // A Tab stop after the options, which Enter presses.
    client.choose = { title: "A tab", choices: [{ key: "a", label: "First", act: () => taken.push("first") }], cursor: 0 };
    await sidebar.missionKey(client, { key: "Tab" });
    assert.equal(client.choose.cursor, 1, "Tab goes on from the last option to the close button");
    assert.match(menu(), /× ╮/);
    await sidebar.missionKey(client, { key: "Enter" });
    assert.equal(client.choose, undefined, "and Enter on it closes the menu");
    client.confirm = { text: "Close it?", yes: () => taken.push("yes") };
    await sidebar.missionKey(client, { key: "Tab" });
    await sidebar.missionKey(client, { key: "Tab" });
    assert.equal(client.confirm.onClose, true, "a question's: after Yes and No");
    await sidebar.missionKey(client, { key: "Enter" });
    assert.deepEqual(taken, [], "pressed as no");
    sidebar.draw(client);
    await sidebar.missionKey(client, { key: "BTab" });
    assert.equal(client.focus, "close", "mission control's own: the last of its stops");
    assert.equal(closed, 0);
    await sidebar.missionKey(client, { key: "Enter" });
    assert.equal(closed, 1, "Enter on it closes mission control");
    closed = 0;
    client.focus = undefined;
    // A message being typed to a tab: Tab reaches it too, and Enter presses it
    // rather than sending what was typed.
    client.answering = { tab: { index: 1, name: "t" }, found: undefined, text: "hi", typing: false };
    await sidebar.missionKey(client, { key: "Tab" });
    assert.equal(client.answering.closeFocus, true);
    await sidebar.missionKey(client, { key: "Enter" });
    assert.equal(client.answering, undefined, "the answer box closes, nothing sent");

    // M-j's picker, typing: Tab past its last row reaches it, and Enter on it
    // closes the picker; Tab again goes back round to the first row.
    client.picker = true;
    client.filter = { text: "", typing: true };
    client.view = { view: sidebar.viewsFor(client).findIndex((v) => v.title === "Work map") + 1, cursor: 0 };
    const rows = sidebar.viewsFor(client)[client.view.view - 1].items.length;
    for (let i = 0; i < Math.max(rows, 1); i++) await sidebar.missionKey(client, { key: "Tab" });
    assert.equal(client.focus, "close", "past the picker's last row");
    assert.equal(client.filter.typing, true);
    await sidebar.missionKey(client, { key: "Tab" });
    assert.equal(client.focus, undefined);
    assert.equal(client.view.cursor, 0, "and round to its first");
    await sidebar.missionKey(client, { key: "BTab" });
    assert.equal(client.focus, "close", "Shift-Tab before the first row");
    await sidebar.missionKey(client, { key: "Enter" });
    assert.equal(closed, 1, "Enter on it closes the picker");
    closed = 0;
    client.picker = false;
    client.filter = undefined;
    client.focus = undefined;
    client.view = { view: 0, cursor: 0 };

    // Mission control itself: the button at the right end of its top bar.
    sidebar.draw(client);
    assert.equal(closed, 0);
    await sidebar.missionKey(client, { mouse: { button: 0, x: 99, y: 1, press: true } });
    assert.equal(closed, 1, "a click on it closes mission control");

    // A peek, its close button in its frame's top edge.
    const peek = { kind: "mission", only: true, socket: { write() {}, end: () => closed++ }, view: { view: 0, cursor: 0 }, cols: 80, rows: 24, tmuxClient: "none", peek: { tab: { index: 1, name: "t" }, mode: "screen", screen: "hello" } };
    sidebar.draw(peek);
    await sidebar.missionKey(peek, { mouse: { button: 0, x: 78, y: 1, press: true } });
    assert.equal(peek.peek, undefined);
    assert.equal(closed, 2, "a peek in a popup of its own closes the popup");

    // Without menu_mouse, a click can't reach it, so it isn't drawn.
    fs.writeFileSync(settingsFile, JSON.stringify({ easy_keys: true, menu_mouse: false }));
    require("../lib/settings").reloadSettings();
    assert.doesNotMatch(menu(), /×/);
  } finally {
    fs.rmSync(settingsFile, { force: true });
    require("../lib/settings").reloadSettings();
  }
});

test("every box in mission control is drawn one way: the keys box as a menu is", () => {
  const help = plain(sidebar.helpModal("mission", 120, 40));
  const menu = plain(sidebar.chooseModal({ title: "A tab", choices: [{ key: "a", label: "Go there" }], cursor: 0 }, 120, 40));
  [help, menu].forEach((box) => assert.match(box, /╭─ \S.*╮[\s\S]*╰─+╯/, "a rounded edge, its title in the top"));
  assert.match(help, /│ any key to go back +│/, "the keys on a row of their own at the bottom, as a menu's");
  assert.match(menu, /│ jk or tab move +↵ or click choose +esc back +│/);
  const popup = sidebar.helpLines("sidebar", 60, 30).map(plain);
  assert.match(popup[0], /^╭─ Keys ─+╮$/, "the sidebar's keys popup too");
  assert.match(popup.at(-1), /^╰─+╯$/);
});

test("six daemons started at once over a dead daemon's lock leave one running, the one the lock names", async () => {
  // Their own state directory, and no tmux server to reach, so they draw in
  // none of the tests' windows.
  const dir = path.join(tmp, "race");
  fs.mkdirSync(path.join(dir, "tmux"), { recursive: true });
  const lockFile = path.join(dir, "state/daemon.pid");
  fs.mkdirSync(path.dirname(lockFile), { recursive: true });
  fs.writeFileSync(lockFile, "99999");
  const env = { ...process.env, TMUX: "", TMUX_TMPDIR: path.join(dir, "tmux"), TMUX_SIDEBAR_STATE_DIR: path.join(dir, "state") };
  const daemons = Array.from({ length: 6 }, () => spawn(process.execPath, [path.join(__dirname, "../bin/tmux-sidebar"), "daemon"], { env, stdio: "ignore" }));
  try {
    await wait(2_000);
    const running = daemons.filter((d) => d.exitCode === null && d.signalCode === null);
    assert.equal(running.length, 1);
    assert.equal(fs.readFileSync(lockFile, "utf8"), String(running[0].pid));
  } finally {
    daemons.forEach((d) => d.kill());
  }
});

test("Tab moves focus through mission control's views' names, its filter and its items, Enter pressing it, and through a box's options", async (t) => {
  const one = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-n", "focus one", "-P", "-F", "#{window_id}");
  t.after(() => tmux("kill-window", "-t", one));
  await sidebar.refresh();
  const sent = [];
  const client = { kind: "mission", socket: { write: (text) => sent.push(text), end() {} }, view: { view: 0, cursor: 0 }, cols: 160, rows: 50, tmuxClient: "none" };
  sidebar.draw(client);
  const views = client.hits.filter((h) => h.view !== undefined).length;
  const key = (k) => sidebar.missionKey(client, { key: k });

  await key("Tab");
  assert.deepEqual(client.focus, { view: 0 }, "the first Tab focuses the first view's name");
  assert.match(sent.at(-1), /\x1b\[7m[^ ]*\s*Tabs/, "drawn in reverse video");
  await key("BTab");
  assert.equal(client.focus, "item", "Shift-Tab from the first wraps round to the last item");
  assert.equal(client.view.cursor, sidebar.missionGrid(sidebar.state.tabs).order.length - 1);
  await key("Tab");
  for (let i = 1; i < views; i++) await key("Tab");
  assert.deepEqual(client.focus, { view: views - 1 }, "Tab goes on through every view's name");
  await key("Tab");
  assert.equal(client.focus, "filter", "then the filter");
  assert.ok(client.hits.some((h) => h.filter), "which is there to click even while empty");
  await key("Tab");
  assert.equal(client.focus, "item", "then the first item");
  assert.equal(client.view.cursor, 0);
  await key("Enter");
  assert.ok(client.choose, "Enter on an item opens its menu");

  await key("Tab");
  assert.equal(client.choose.cursor, 1, "in a menu, Tab moves to the next option");
  await key("BTab");
  await key("BTab");
  assert.equal(client.choose.cursor, client.choose.choices.length - 1, "Shift-Tab wraps round to the last");
  await key("Escape");

  let yes = 0;
  client.confirm = { text: "Close it?", yes: () => { yes++; } };
  sidebar.draw(client);
  await key("Tab");
  assert.equal(client.confirm.no, true, "in a question, Tab moves from Yes to No");
  assert.match(plain(sent.at(-1)), /Yes +› No/, "the › on No");
  await key("Enter");
  assert.equal(yes, 0, "and Enter then says no");
  assert.equal(client.confirm, undefined);
  client.confirm = { text: "Close it?", yes: () => { yes++; } };
  await key("Space");
  assert.equal(yes, 0, "Space before Tab is any other key, no");
  client.confirm = { text: "Close it?", yes: () => { yes++; } };
  await key("Tab");
  await key("BTab");
  await key("Space");
  assert.equal(yes, 1, "once Tab has moved to Yes, Space presses it");

  // In M-j's picker, a menu over it takes Tab, the filter still typing.
  client.picker = true;
  client.filter = { text: "", typing: true };
  client.choose = { title: "A tab", choices: [{ key: "a", label: "First", act() {} }, { key: "b", label: "Second", act() {} }], cursor: 0 };
  await key("Tab");
  assert.equal(client.choose.cursor, 1, "Tab moves a picker's menu");
  assert.equal(client.filter.typing, true);
  await key("Escape");
  assert.equal(client.choose, undefined, "Esc closes the menu, not the picker");
  client.choose = { title: "A tab", choices: [{ key: "a", label: "First", act() {} }], cursor: 0 };
  await key("Space");
  assert.equal(client.choose, undefined, "Space takes its option");
  assert.equal(client.filter.text, "", "and types nothing into the filter");
  client.view = { view: sidebar.viewsFor(client).findIndex((v) => v.title === "Work map") + 1, cursor: 0 };
  const rows = sidebar.viewsFor(client)[client.view.view - 1].items.length;
  await key("Tab");
  assert.equal(client.view.cursor, Math.min(1, rows - 1), "Tab goes down the picker's rows");
  for (let i = 1; i < rows; i++) await key("Tab");
  assert.equal(client.view.cursor, 0, "and round from the last to the first");
  assert.equal(client.filter.typing, true, "still typing");
  client.choose = undefined;
  client.picker = false;
  client.filter = undefined;

  client.focus = { view: 1 };
  await key("Enter");
  assert.equal(client.view.view, 1, "Enter on a view's name goes to it");
  await key("]");
  assert.deepEqual(client.focus, { view: 2 }, "] takes the focus on a view's name along to the next");
  await key("[");
  client.focus = "filter";
  await key("Enter");
  assert.equal(client.filter?.typing, true, "Enter on the filter starts typing in it");
});

test("what a click would press lights under the mouse, drawn again only when that changes, and goes on a key or at the edge", async (t) => {
  const one = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-n", "hover one", "-P", "-F", "#{window_id}");
  const two = tmux("new-window", "-d", "-t", "test", "-c", tmp, "-n", "hover two", "-P", "-F", "#{window_id}");
  t.after(() => tmux("kill-window", "-t", one));
  t.after(() => tmux("kill-window", "-t", two));
  await sidebar.refresh();
  const sent = [];
  const client = { kind: "mission", socket: { write: (text) => sent.push(text), end() {} }, view: { view: 0, cursor: 0 }, cols: 160, rows: 50, tmuxClient: "none" };
  // A hover draws at once when the last draw is over 16ms old.
  const hover = async (at) => {
    await wait(20);
    sidebar.hoverTo(client, at);
  };
  sidebar.draw(client);
  assert.ok(sent.some((text) => text.includes("\x1b[?1003h")), "the mouse's every move is asked for");

  // A view's name: reverse video, as Tab's focus; the chosen view isn't lit.
  const name = client.hits.find((h) => h.view === 1);
  await hover({ x: name.x0 + 1, y: 1 });
  assert.equal(client.hover, "view:1");
  assert.match(sent.at(-1), new RegExp(`\\x1b\\[7m[^ ]* ${sidebar.state.sections[0]?.title ?? ""}`));
  const drawn = sent.length;
  await hover({ x: name.x0 + 2, y: 1 });
  assert.equal(sent.length, drawn, "moving within it draws nothing");
  await hover({ x: client.hits.find((h) => h.view === 0).x0 + 1, y: 1 });
  assert.equal(client.hover, undefined, "the chosen view's name isn't lit");

  // A card other than the chosen one: its border in its tab's color.
  const card = client.hits.find((h) => h.index === 1);
  await hover({ x: card.x0 + 2, y: card.y });
  assert.equal(client.hover, "index:1");
  await hover({ x: client.cols, y: card.y });
  assert.equal(client.hover, undefined, "the screen's edge, the way out, puts it out");
  await hover({ x: card.x0 + 2, y: card.y });

  // Yes and No of a question, in reverse video, without moving Enter's choice.
  client.confirm = { text: "Close it?", yes() {} };
  client.hover = undefined;
  sidebar.draw(client);
  const row = client.modal.tags.find((t) => t.tag.yes).y;
  await hover({ x: client.modal.left + 1 + 9, y: row });
  assert.equal(client.hover, "no");
  assert.equal(client.confirm.no, undefined, "Enter still takes Yes");
  assert.match(plain(sent.at(-1).replace(/\x1b\[7m/g, "<lit>")), /› Yes +<lit>No/);
  client.confirm = undefined;

  // The sidebar's tabs: a row on the panel's shade.
  const tabs = sidebar.state.tabs.filter((tab) => tab.session === sidebar.state.tabs.find((x) => x.id === one).session);
  const { lines, owners } = sidebar.sidebarLines({ tabs, sections: [], cols: 30, rows: 30, view: { focused: false, cursor: tabs[0].id, frame: 0, here: tabs[0].id, hover: two } });
  const lit = lines.filter((line, i) => owners[i] === two);
  assert.ok(lit.length && lit.every((line) => line.startsWith("\x1b[48;5;236m")), "every line of the tab under the mouse is shaded");
  assert.ok(lines.filter((line, i) => owners[i] === one).every((line) => !line.startsWith("\x1b[48;5;236m")), "and no other");

  // Off, the mouse's moves are no longer asked for, and nothing is lit.
  const settingsFile = path.join(home, ".config/switchboard/settings.json");
  fs.writeFileSync(settingsFile, JSON.stringify({ hover_highlight: false }));
  require("../lib/settings").reloadSettings();
  t.after(() => {
    fs.writeFileSync(settingsFile, "{}");
    require("../lib/settings").reloadSettings();
  });
  sidebar.draw(client);
  assert.ok(sent.some((text) => text.includes("\x1b[?1003l")));
  assert.equal(client.hover, undefined);
});

test("a burst of the mouse's moves draws once, sending only the rows it changed, from the views it last built", async (t) => {
  const windows = ["one", "two", "three"].map((name) => tmux("new-window", "-d", "-t", "test", "-c", tmp, "-n", `burst ${name}`, "-P", "-F", "#{window_id}"));
  t.after(() => windows.forEach((id) => tmux("kill-window", "-t", id)));
  await sidebar.refresh();
  const sent = [];
  const socket = new (require("node:events").EventEmitter)();
  Object.assign(socket, { setEncoding() {}, write: (text) => sent.push(text), end() {} });
  sidebar.accept(socket);
  socket.emit("data", "mission none 160 50\n");
  await wait(100);
  const client = [...sidebar.state.clients].find((c) => c.socket === socket);
  t.after(() => socket.emit("close"));
  const cards = client.hits.filter((h) => h.index >= 1).slice(0, 2);
  assert.equal(cards.length, 2);
  const move = ({ x0, y }) => `\x1b[<35;${x0 + 2};${y}M`;
  const views = client.views;

  // Twenty moves between two cards in one read: one draw, for the last.
  await wait(20);
  const before = sent.length;
  socket.emit("data", Array.from({ length: 20 }, (_, i) => move(cards[i % 2])).join(""));
  await wait(50);
  assert.equal(sent.length - before, 1, "one draw for the burst");
  assert.equal(client.hover, `index:${cards[1].index}`, "lit where the mouse ended");
  assert.equal(client.views, views, "the views weren't built again");

  // Only the rows that changed, each put in place, not the whole screen.
  socket.emit("data", move(cards[0]));
  await wait(50);
  const rows = sent.at(-1).match(/\x1b\[\d+;1H/g) ?? [];
  assert.ok(rows.length > 0 && rows.length < client.rows / 2, `${rows.length} rows sent of ${client.rows}`);
  assert.match(sent.at(-1), /^\x1b\[\?2026h\x1b\[\d+;1H/, "synchronized, starting with a move to its row");

  // A box opened since the last frame: the whole frame.
  client.confirm = { text: "Close it?", yes() {} };
  client.hover = undefined;
  socket.emit("data", move(cards[1]));
  await wait(50);
  assert.equal((sent.at(-1).match(/\x1b\[\d+;1H/g) ?? []).length, client.rows, "every row");
  client.confirm = undefined;
});

test("with tab_focus off, Tab switches mission control's views as before", (t) => {
  const settingsFile = path.join(home, ".config/switchboard/settings.json");
  fs.mkdirSync(path.dirname(settingsFile), { recursive: true });
  fs.writeFileSync(settingsFile, JSON.stringify({ tab_focus: false }));
  require("../lib/settings").reloadSettings?.();
  t.after(() => {
    fs.rmSync(settingsFile, { force: true });
    require("../lib/settings").reloadSettings?.();
  });
  const client = { kind: "mission", socket: { write() {}, end() {} }, view: { view: 0, cursor: 0 }, cols: 160, rows: 50, tmuxClient: "none" };
  sidebar.draw(client);
  sidebar.missionKey(client, { key: "Tab" });
  assert.equal(client.view.view, 1);
  assert.equal(client.focus, undefined);
  assert.ok(!client.hits.some((h) => h.filter), "and there is no empty filter to land on");
});

test("mission control's bar scrolls to keep the view's name or the filter Tab has focused in sight", () => {
  const sections = Array.from({ length: 14 }, (_, i) => ({ title: `Section ${i}`, items: [], loaded: true }));
  const lines = sidebar.missionLines({ tabs: [], sections, cols: 100, rows: 10, view: { view: 0, cursor: 0, focus: { view: 14 }, tabFocus: true }, config });
  assert.match(plain(lines[0]), /‹ .*Section 13/, "the last view's name, those before it giving way to a ‹");
  const hit = lines.hits.find((h) => h.view === 14);
  assert.ok(hit && hit.x1 <= 100, "and its click lands where it is drawn");
  const filter = sidebar.missionLines({ tabs: [], sections, cols: 100, rows: 10, view: { view: 0, cursor: 0, focus: "filter", tabFocus: true }, config });
  assert.match(plain(filter[0]), / \/ filter /, "the filter too");
  assert.ok(filter.hits.find((h) => h.filter).x1 <= 100);
});

test("the sidebar's list holds at the line it was scrolled to while the cursor stays in sight, and scrolls only as far as it must to keep it there", () => {
  const tabs = Array.from({ length: 8 }, (_, i) => ({ id: `@${i + 1}`, index: i + 1, name: `tab ${i + 1}`, state: "idle", session: "$1", sessionName: "s", panes: [], sidebars: [] }));
  const free = sidebar.sidebarLines({ tabs, sections: [], cols: 30, rows: 10, view: { focused: true, cursor: "@3", frame: 0, here: "@1" } }).scroll;
  assert.equal(free.top, 0, "nothing kept: as little scrolling as it takes");
  assert.equal(sidebar.sidebarLines({ tabs, sections: [], cols: 30, rows: 10, view: { focused: true, cursor: "@3", frame: 0, here: "@1", top: 9 } }).scroll.top, free.cursorAt, "kept past the cursor: back up to it");
  assert.equal(sidebar.sidebarLines({ tabs, sections: [], cols: 30, rows: 10, view: { focused: true, cursor: "@5", frame: 0, here: "@1", top: 4 } }).scroll.top, 4, "the cursor in sight: the list holds");
  const last = sidebar.sidebarLines({ tabs, sections: [], cols: 30, rows: 10, view: { focused: true, cursor: "@8", frame: 0, here: "@1", top: 0 } }).scroll;
  assert.ok(last.top > 0 && last.top <= last.cursorAt, "kept above the cursor's sight: down to it");
  assert.equal(sidebar.sidebarLines({ tabs, sections: [], cols: 30, rows: 10, view: { focused: true, cursor: "@8", frame: 0, here: "@1", top: 99 } }).scroll.top, last.length - last.room, "never past the end");
});

test("C-d and C-u in the sidebar scroll its list half a screen and move the cursor onto the tab there, stopping at the last tab", async (t) => {
  const windows = Array.from({ length: 3 }, () => tmux("new-window", "-d", "-t", "test", "-c", tmp, "-P", "-F", "#{window_id}"));
  const side = tmux("split-window", "-hb", "-d", "-t", windows[0], "-P", "-F", "#{pane_id}");
  tmux("set", "-p", "-t", side, "@sidebar", "1");
  t.after(() => windows.forEach((w) => tmux("kill-window", "-t", w)));
  await sidebar.refresh();
  const session = tmux("display", "-p", "-t", windows[0], "#{session_id}");
  // Each tab two lines with a rule between, under the two heading lines and
  // a group's, six lines shown at a time.
  const owners = [];
  windows.forEach((w, i) => (owners[3 + i * 3] = owners[4 + i * 3] = w));
  const client = { kind: "sidebar", pane: side, socket: { write() {}, end() {} }, scroll: { top: 0, room: 6, length: 12, owners, cursorAt: 3 } };
  sidebar.state.cursors.set(session, windows[0]);
  await sidebar.sidebarKey(client, { key: "C-d" });
  assert.equal(sidebar.state.cursors.get(session), windows[1], "three lines on, the next tab");
  assert.equal(sidebar.state.sidebarTops.get(session), 3, "the list three lines on too");
  client.scroll = { top: 3, room: 6, length: 12, owners, cursorAt: 9 };
  sidebar.state.cursors.set(session, windows[2]);
  await sidebar.sidebarKey(client, { key: "C-d" });
  assert.equal(sidebar.state.cursors.get(session), windows[2], "past the last tab, it stays on it");
  assert.equal(sidebar.state.sidebarTops.get(session), 6, "the list no further than its end");
  client.scroll = { top: 6, room: 6, length: 12, owners, cursorAt: 9 };
  await sidebar.sidebarKey(client, { key: "C-u" });
  assert.equal(sidebar.state.cursors.get(session), windows[1], "three lines back, the tab before");
  assert.equal(sidebar.state.sidebarTops.get(session), 3);
  // A tab taller than half the list: C-d still goes on to the next one.
  const tall = [];
  tall[3] = tall[4] = tall[5] = tall[6] = tall[7] = windows[0];
  tall[9] = windows[1];
  client.scroll = { top: 0, room: 6, length: 12, owners: tall, cursorAt: 3 };
  sidebar.state.cursors.set(session, windows[0]);
  await sidebar.sidebarKey(client, { key: "C-d" });
  assert.equal(sidebar.state.cursors.get(session), windows[1], "past the tall tab");
});

test("working a note continuously keeps 3 tabs working and at most 5 open, a tab waiting on you freeing its slot, one landed leaving the mode, and stops on pause or an empty note", async () => {
  const dir = path.join(tmp, "autopilot");
  const repo = path.join(dir, "repo");
  const notes = path.join(dir, "Notes");
  fs.mkdirSync(notes, { recursive: true });
  fs.writeFileSync(path.join(notes, "Work.md"), "# Work\n\n- [ ] One\n");
  // Changed long enough ago to have settled.
  fs.utimesSync(path.join(notes, "Work.md"), new Date(Date.now() - 120_000), new Date(Date.now() - 120_000));
  const gitEnv = { GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  execFileSync("git", ["init", "-q", "-b", "main", repo], { env: { ...process.env, ...gitEnv } });
  execFileSync("git", ["-C", repo, "commit", "-q", "--allow-empty", "-m", "start"], { env: { ...process.env, ...gitEnv } });
  // A stand-in for wtn-open.zsh: each `next` makes a worktree and a tab
  // working in it, opened five minutes ago, until `max` items are taken.
  const stub = path.join(dir, "wtn-open.zsh");
  fs.writeFileSync(stub, [
    "zmodload zsh/datetime",
    "here=${0:A:h}",
    "[[ $1 == queue ]] && { print -r -- $'1\\tnext thing'; exit 0 }",
    "print -r -- \"$WTN_SKIP\" > $here/skip",
    "n=$(( $(cat $here/count 2>/dev/null || print 0) + 1 ))",
    "(( n > $(cat $here/max) )) && exit 0",
    "print $n > $here/count",
    "git worktree add -q \"$PWD/.wt/$n\" -b \"b$n\" >/dev/null 2>&1",
    "w=$(tmux new-window -d -c \"$PWD/.wt/$n\" -P -F '#{window_id}' cat)",
    "tmux set-option -w -t $w @autopilot $((EPOCHSECONDS - 300))",
    "tmux set-option -w -t $w @agent working",
    "tmux set-option -w -t $w @autopilot_item \"item $n\"",
    "print -r -- \"$n\"$'\\t'\"$w\"$'\\titem '\"$n\"",
  ].join("\n"));
  fs.writeFileSync(path.join(dir, "max"), "99");
  const saved = { ...process.env };
  Object.assign(process.env, gitEnv, { TMUX_SIDEBAR_WTN_OPEN: stub, NOTES_DIR: notes });
  const pilot = sidebar.autopilot({ batch: sidebar.batch, gapMs: 0 });
  const mine = () => tmux("list-windows", "-a", "-F", "#{window_id}\t#{@autopilot}\t#{@autopilot_in}").split("\n").map((row) => row.split("\t")).filter(([, at]) => at);
  // Checks until the mode's tabs hold still.
  const settle = async () => {
    let was;
    for (let still = 0; still < 6; ) {
      pilot.check(sidebar.tabsFrom(await sidebar.snapshot()), { away: false });
      await wait(150);
      const now = mine().map((row) => row.join()).join(";");
      still = now === was ? still + 1 : 0;
      was = now;
    }
    return mine();
  };
  try {
    sidebar.writeAutopilot({ on: true, note: "Work.md", repo, slots: 3, cap: 5, paused: false, said: "" });
    fs.utimesSync(path.join(notes, "Work.md"), new Date(), new Date());
    assert.equal((await settle()).length, 0, "nothing taken from a note changed a moment ago, whose item may still be being typed");
    fs.utimesSync(path.join(notes, "Work.md"), new Date(Date.now() - 120_000), new Date(Date.now() - 120_000));
    assert.equal((await settle()).length, 3, "three tabs open, one for each slot");
    const wait1 = mine()[0][0];
    tmux("set-option", "-w", "-t", wait1, "@agent", "waiting");
    assert.equal((await settle()).length, 4, "a tab waiting on you frees its slot");
    tmux("set-option", "-w", "-t", mine()[1][0], "@agent", "done");
    assert.equal((await settle()).length, 5, "a fifth once another waits");
    tmux("set-option", "-w", "-t", mine()[2][0], "@agent", "waiting");
    assert.equal((await settle()).length, 5, "never a sixth: the cap counts the tabs waiting on you");
    assert.ok(mine().every(([, , inWorktree]) => inWorktree === "1"), "each marked once seen in its worktree");
    assert.ok(["item 1", "item 2", "item 3", "item 4"].every((item) => fs.readFileSync(path.join(dir, "skip"), "utf8").split("\n").includes(item)), "the items of the tabs still open are never taken again");

    // queue-and-clean's landing moves the pane back to the main clone.
    tmux("respawn-pane", "-k", "-t", wait1, "-c", repo, "cat");
    // Its session resumed there, idle, as SessionStart leaves it.
    tmux("set-option", "-w", "-u", "-t", wait1, "@agent");
    await settle();
    assert.match(tmux("show-option", "-wqv", "-t", wait1, "@autopilot_left"), /^\d+$/, "the landed tab is marked landed");
    const after = mine().filter(([id]) => id !== wait1);
    assert.equal(after.length, 5, "and leaves the mode: the next item opens in its place");
    tmux("kill-window", "-t", wait1);
    assert.equal(sidebar.readAutopilot().said, "");

    sidebar.writeAutopilot({ paused: true });
    tmux("set-option", "-w", "-t", after[3][0], "@agent", "waiting");
    tmux("kill-window", "-t", after[4][0]);
    assert.equal((await settle()).length, 4, "paused, it opens nothing");

    sidebar.writeAutopilot({ paused: false });
    fs.writeFileSync(path.join(dir, "max"), "0");
    assert.equal((await settle()).length, 4, "nothing left in the note: nothing opens");
    assert.equal(sidebar.readAutopilot().said, "Nothing left to take in Work", "and it says so");
    const summary = pilot.summary(sidebar.tabsFrom(await sidebar.snapshot()), { away: false });
    assert.deepEqual([summary.note, summary.open, summary.cap, summary.slots, summary.next], ["Work", 4, 5, 3, "next thing"]);
  } finally {
    Object.keys(process.env).filter((key) => !(key in saved)).forEach((key) => delete process.env[key]);
    Object.assign(process.env, saved);
    sidebar.writeAutopilot({ on: false });
    mine().forEach(([id]) => tmux("kill-window", "-t", id));
  }
});

test("a landed tab of the mode working a note closes once you leave it after seeing it, or half an hour after landing, and stays yours once you prompt it", async () => {
  const dir = path.join(tmp, "autopilot-close");
  const repo = path.join(dir, "repo");
  const gitEnv = { GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  fs.mkdirSync(dir, { recursive: true });
  execFileSync("git", ["init", "-q", "-b", "main", repo], { env: { ...process.env, ...gitEnv } });
  sidebar.writeAutopilot({ on: false, note: "Work.md", repo });
  const sent = [];
  const pilot = sidebar.autopilot({ batch: (commands) => sent.push(...commands) });
  const now = Math.floor(Date.now() / 1000);
  const tab = (id, autopilot, { front = false, state = "idle", since } = {}) => ({
    id,
    current: front,
    attached: true,
    state,
    panes: [{ pane_id: `%${id}`, pane_current_path: repo, "@agent_since": since === undefined ? "" : String(since) }],
    autopilot: { since: now - 3600, inWorktree: true, ...autopilot },
  });
  const check = (...tabs) => {
    sent.length = 0;
    pilot.check(tabs, { away: false });
    // A window or pane killed as [kill-window, its id], an option set as [set-option, the option].
    return sent.map((command) => (command[0].startsWith("kill-") ? [command[0], command.at(-1)] : [command[0], command.find((word) => word.startsWith("@autopilot"))]));
  };
  try {
    const removed = tab("@1", {});
    removed.panes[0].pane_current_path = path.join(repo, ".claude/worktrees/gone");
    assert.deepEqual(check(removed), [], "still in its removed worktree under the main clone: not landed yet");
    const asking = tab("@1", {}, { state: "waiting", since: now - 30 });
    asking.claudePane = "%asking";
    asking.panes = [{ pane_id: "%asking", pane_current_path: path.join(repo, ".claude/worktrees/gone") }, { pane_id: "%shell", pane_current_path: repo }];
    assert.deepEqual(check(asking), [], "Claude still asking in its removed worktree, a shell beside it in the main clone: not landed yet");
    assert.deepEqual(check(tab("@1", {})), [["set-option", "@autopilot_left"]], "back in the main clone, it is marked landed");
    assert.deepEqual(check(tab("@1", { landed: now - 60 })), [], "not seen and landed a minute ago: left open");
    assert.deepEqual(check(tab("@1", { landed: now - sidebar.CLOSE_AFTER_S - 1 })), [["kill-window", "@1"]], "never seen: closed half an hour after landing");
    assert.deepEqual(check(tab("@2", { landed: now - 60 }, { front: true })), [], "in front a moment: not seen yet, as passing over it does");
    await wait(1100);
    assert.deepEqual(check(tab("@2", { landed: now - 60 }, { front: true })), [["set-option", "@autopilot_seen"]], "in front a second: seen");
    assert.deepEqual(check(tab("@2", { landed: now - 60, seen: true }, { front: true })), [], "never closed while in front");
    assert.deepEqual(check(tab("@2", { landed: now - 60, seen: true })), [["kill-window", "@2"]], "closed once you leave it");
    const withNotes = tab("@4", { landed: now - 60, seen: true });
    withNotes.panes.push({ pane_id: "%notes", pane_current_path: repo, "@notes_pane": "1" });
    assert.deepEqual(check(withNotes), [["kill-pane", "%@4"]], "with Notes beside it, only its own panes close, so Notes goes back to its tab");
    assert.deepEqual(check(tab("@3", { landed: now - 60 }, { state: "working", since: now - 120 })), [], "still working from before it landed: not yours yet");
    check(tab("@3", { landed: now - 60, seen: true }, { state: "working", since: now - 10 }));
    assert.ok(sent.length && sent.every((command) => command.includes("-u") && command.includes("@3")), "a prompt since landing takes it out of the mode, never closed");
  } finally {
    sidebar.writeAutopilot({ on: false, repo: "" });
  }
});

test("while a note is worked continuously, mission control's Tabs view leads with its line, which the cursor and a click reach above the cards", async () => {
  const tabs = sidebar.tabsFrom(await sidebar.snapshot());
  const autopilot = { note: "Work", busy: 2, open: 4, slots: 3, cap: 5, next: "Next thing", paused: false, away: false, readyOnly: false, skipped: 1, spendLimit: 0 };
  const mission = sidebar.missionLines({ tabs, sections: [], cols: 120, rows: 30, view: { view: 0, cursor: -1, frame: 0, autopilot }, config });
  assert.equal(mission.length, 30);
  mission.forEach((line) => assert.equal(sidebar.width(line), 120, JSON.stringify(line)));
  const text = mission.map((line) => line.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, ""));
  assert.match(text[1], /^ AUTO Work  2 of 3 working · 4 of 5 open · next: Next thing · 1 skipped/);
  assert.deepEqual(mission.hits.find((h) => h.y === 2), { y: 2, x0: 1, x1: 120, index: -1 }, "a click on it chooses it");
  assert.ok(mission.hits.filter((h) => h.index >= 0).every((h) => h.y >= 3), "the cards under it");
  const paused = sidebar.missionLines({ tabs, sections: [], cols: 120, rows: 30, view: { view: 0, cursor: 0, frame: 0, autopilot: { ...autopilot, paused: true, next: "" } }, config });
  assert.match(paused[1].replace(/\x1b\[[0-9;?]*[A-Za-z]/g, ""), /Work  paused · nothing left to take/);
});

test("k from the top row of tabs reaches the line of the note worked continuously, and Enter there opens its menu, whose Pause pauses it", async () => {
  sidebar.writeAutopilot({ on: true, note: "Work.md", repo: tmp, slots: 3, cap: 5, paused: false, said: "" });
  try {
    sidebar.state.tabs = sidebar.tabsFrom(await sidebar.snapshot());
    const client = { kind: "mission", socket: { write() {}, end() {} }, view: { view: 0, cursor: 0 }, cols: 120, rows: 30, tmuxClient: "none" };
    await sidebar.missionKey(client, { key: "k" });
    assert.equal(client.view.cursor, -1, "on the mode's line");
    await sidebar.missionKey(client, { key: "Enter" });
    assert.equal(client.choose.title, "Working Work");
    // Skip shows only once the next item has been read, which the daemon may have done.
    assert.deepEqual(client.choose.choices.map((c) => c.label).filter((label) => !label.startsWith("Skip the next item")), ["Pause: open no more tabs for now", "Take only items with a settled plan", "Change its note, slots, cap or spend limit…", "Stop working the note"]);
    await sidebar.missionKey(client, { key: "Enter" });
    assert.equal(sidebar.readAutopilot().paused, true);
    await sidebar.missionKey(client, { key: "j" });
    assert.equal(client.view.cursor, 0, "j goes back down to the first tab");
  } finally {
    sidebar.writeAutopilot({ on: false, paused: false });
  }
});

test("a tab of the mode working a note still counts as working while M-d or lazygit stands in for its Claude, from the parked window's state", () => {
  const repo = path.join(tmp, "autopilot-inplace");
  const worktree = path.join(repo, ".claude/worktrees/x");
  fs.mkdirSync(path.join(repo, ".git/worktrees/x"), { recursive: true });
  fs.mkdirSync(worktree, { recursive: true });
  fs.writeFileSync(path.join(worktree, ".git"), `gitdir: ${path.join(repo, ".git/worktrees/x")}\n`);
  fs.writeFileSync(path.join(repo, ".git/worktrees/x/commondir"), "../..\n");
  const since = Date.now() / 1000 - 600;
  const tab = { id: "@1", state: "idle", path: worktree, panes: [{ pane_id: "%2", pane_current_path: worktree, "@parked_pane": "%1" }], autopilot: { since, inWorktree: true } };
  const parked = (state) => [{ state, panes: [{ pane_id: "%1" }] }];
  assert.equal(sidebar.placeOf(tab, repo, Date.now() / 1000, parked("background")).kind, "busy", "its background work counts");
  assert.equal(sidebar.placeOf(tab, repo, Date.now() / 1000, parked("waiting")).kind, "waiting");
  const young = { ...tab, panes: [{ pane_id: "%3", pane_current_path: worktree }], autopilot: { since: Date.now() / 1000, inWorktree: true } };
  assert.equal(sidebar.placeOf({ ...young, state: "idle" }, repo, Date.now() / 1000).kind, "busy", "a tab just opened counts as busy while its Claude starts");
  assert.equal(sidebar.placeOf({ ...young, state: "waiting" }, repo, Date.now() / 1000).kind, "waiting", "but one waiting on you frees its slot at once");
});

test("mission control sends each frame synchronized, a spinner's tick only its changed rows with a box merged into them, and behind a box ticks twice a second at most", async (t) => {
  await sidebar.refresh();
  const tab = sidebar.state.tabs[0];
  const was = tab.state;
  tab.state = "working";
  const sent = [];
  const client = { kind: "mission", socket: { write: (text) => sent.push(text), end() {} }, view: { view: 0, cursor: 0 }, cols: 120, rows: 30, tmuxClient: "none" };
  sidebar.state.clients.add(client);
  t.after(() => {
    sidebar.state.clients.delete(client);
    tab.state = was;
  });
  const moves = (text) => (text.match(/\x1b\[\d+;1H/g) ?? []).length;
  sidebar.draw(client);
  assert.ok(moves(sent.at(-1)) >= client.rows, "the first frame whole");

  // A tick with no box: only the rows its spinner changed.
  let before = sent.length;
  sidebar.spin();
  assert.equal(sent.length - before, 1);
  assert.ok(moves(sent.at(-1)) > 0 && moves(sent.at(-1)) < client.rows / 2, `${moves(sent.at(-1))} rows sent`);

  // A box opening: the whole frame, the box merged into its rows.
  client.choose = { title: "Pick one", cursor: 0, choices: [{ label: "First" }, { label: "Second" }] };
  sidebar.draw(client);
  assert.ok(moves(sent.at(-1)) >= client.rows, "the whole frame when a box opens");
  assert.ok(!/\x1b\[\d+;\d{2,}H/.test(sent.at(-1).replace(/\x1b\[\d+;1H/g, "")), "the box isn't drawn over the frame afterwards");

  // Behind it, a tick at most every half second, sending the changed rows
  // whole with the box in them, so the box shows as it was.
  before = sent.length;
  for (let i = 0; i < 4; i++) {
    sidebar.spin();
    await wait(110);
  }
  assert.ok(sent.length - before <= 1, `${sent.length - before} draws in about 440ms behind a box`);
  await wait(500);
  sidebar.spin();
  const tick = sent.at(-1);
  assert.ok(moves(tick) > 0 && moves(tick) < client.rows / 2, "a tick sends only its rows");
  const { top, lines } = client.modal;
  for (const [, row, line] of tick.matchAll(/\x1b\[(\d+);1H((?:(?!\x1b\[\d+;1H)[\s\S])*)/g)) {
    if (row >= top && row < top + lines.length) assert.ok(plain(line).includes(plain(lines[row - top])), `row ${row} keeps the box`);
  }
  // A draw for anything else meanwhile doesn't turn the spinners sooner.
  sidebar.state.frame += 3;
  before = sent.length;
  sidebar.draw(client);
  assert.equal(sent.length, before, "nothing to send: the spinners wait for the next tick");
  for (const text of sent) assert.match(text, /^\x1b\[\?2026h[\s\S]*\x1b\[\?2026l$/, "every write synchronized");

  // The box closing: the whole frame again.
  client.choose = undefined;
  sidebar.draw(client);
  assert.ok(moves(sent.at(-1)) >= client.rows, "the whole frame when the box closes");
  assert.doesNotMatch(screen(sent), /Pick one/);
});

test("the note menu of working a note continuously lists the notes holding an unticked item, the rest apart", () => {
  const notes = path.join(tmp, "note-menu");
  fs.mkdirSync(path.join(notes, "Meetings"), { recursive: true });
  fs.writeFileSync(path.join(notes, "Work.md"), "# Work\n\n- [x] Done\n- [ ] Left\n");
  fs.writeFileSync(path.join(notes, "Done.md"), "# Done\n\n- [x] All of it\n");
  fs.writeFileSync(path.join(notes, "Meetings", "Sync.md"), "# Sync\n\nWe talked.\n");
  const was = process.env.NOTES_DIR;
  process.env.NOTES_DIR = notes;
  try {
    const { open, rest } = sidebar.noteChoices();
    assert.deepEqual(open, ["Work.md"]);
    assert.deepEqual(rest.sort(), ["Done.md", path.join("Meetings", "Sync.md")]);
  } finally {
    if (was === undefined) delete process.env.NOTES_DIR;
    else process.env.NOTES_DIR = was;
  }
});
