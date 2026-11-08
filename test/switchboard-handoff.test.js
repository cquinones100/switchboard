// Tests for bin/switchboard-handoff, which hands the tmux workspace to the
// Rust port's layout.json and brings it back. Run with:
// node --test test/*.test.js
//
// It runs against throwaway tmux servers and a temporary HOME, so it never
// reads your tmux, your Claude sessions or your state.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync, spawn, spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const script = path.join(__dirname, "../bin/switchboard-handoff");
const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "handoff-")));
const fakeBin = path.join(tmp, "bin");
fs.mkdirSync(fakeBin);
// A stand-in claude that records what it was asked to resume.
const claudeArgs = path.join(tmp, "claude-args");
fs.writeFileSync(path.join(fakeBin, "claude"), `#!/bin/sh\necho "$@" >> '${claudeArgs}'\n`, { mode: 0o755 });
const portState = path.join(tmp, "port-state");
const work = path.join(tmp, "work");
const other = path.join(tmp, "other");
fs.mkdirSync(work);
fs.mkdirSync(other);

let sockets = 0;
const servers = [];
const newServer = () => {
  const socket = path.join(tmp, `s${sockets++}.sock`);
  servers.push(socket);
  return socket;
};
after(() => {
  servers.forEach((socket) => spawnSync("tmux", ["-S", socket, "kill-server"]));
  fs.rmSync(tmp, { recursive: true, force: true });
});

const env = (socket) => ({ ...process.env, HOME: tmp, PATH: `${fakeBin}:${process.env.PATH}`, TMUX: `${socket},0,0`, SHELL: "/bin/sh", NOTES_OPEN_COMMAND: "cat" });
const tmuxOn = (socket) => (...args) => execFileSync("tmux", ["-S", socket, ...args], { encoding: "utf8", env: env(socket) }).trimEnd();
const run = (socket, ...args) => spawnSync("node", [script, "--state-dir", portState, ...args], { encoding: "utf8", env: env(socket) });
const readLayout = () => JSON.parse(fs.readFileSync(path.join(portState, "layout.json"), "utf8"));

// The port's Saved shape (src/mux/persist.rs), checked by hand: what a field
// may be, and what it must hold.
const checkShape = (saved) => {
  const isId = (n) => Number.isInteger(n) && n >= 0;
  assert.equal(saved.version, 1);
  assert.ok(Array.isArray(saved.sessions));
  const checkLayout = (layout) => {
    const keys = Object.keys(layout);
    assert.equal(keys.length, 1, "Layout is an externally tagged enum");
    if (keys[0] === "Pane") return assert.ok(isId(layout.Pane));
    assert.equal(keys[0], "Split");
    const { across, children, shares } = layout.Split;
    assert.equal(typeof across, "boolean");
    assert.ok(children.length >= 2 && children.length === shares.length);
    assert.ok(Math.abs(shares.reduce((a, b) => a + b, 0) - 1) < 1e-6, "shares sum to 1");
    children.forEach(checkLayout);
  };
  saved.sessions.forEach((s) => {
    assert.equal(typeof s.name, "string");
    assert.ok(isId(s.cols) && isId(s.rows) && isId(s.current));
    assert.ok(s.last === null || isId(s.last));
    s.tabs.forEach((t) => {
      assert.ok(isId(t.id) && isId(t.active));
      assert.equal(typeof t.name, "string");
      assert.ok(t.last_pane === null || isId(t.last_pane));
      assert.equal(typeof t.zoomed, "boolean");
      Object.values(t.opts).forEach((v) => assert.equal(typeof v, "string"));
      checkLayout(t.layout);
      t.panes.forEach((p) => {
        assert.ok(isId(p.id));
        ["cwd", "claude", "text"].forEach((key) => assert.ok(p[key] === undefined || typeof p[key] === "string"));
        assert.ok(p.command === undefined || p.command.every((w) => typeof w === "string"));
        Object.values(p.opts ?? {}).forEach((v) => assert.equal(typeof v, "string"));
      });
    });
  });
};

// A tree with its pane ids left out, for comparing two servers' layouts.
const shapeOf = (layout) =>
  layout.Pane !== undefined ? "pane" : { across: layout.Split.across, shares: layout.Split.shares.map((s) => Math.round(s * 10) / 10), children: layout.Split.children.map(shapeOf) };

// Two sessions; in "work", a tab of three panes (one beside two, one above
// the other) with a sidebar on its left, a tab with a tool in its pane's
// place, the parked window behind it, and the Notifications window.
const buildWorkspace = (socket) => {
  const tmux = tmuxOn(socket);
  tmux("-f", "/dev/null", "new-session", "-d", "-s", "work", "-x", "200", "-y", "50", "-c", work);
  tmux("set", "-g", "@setting_save_contents", "off");
  tmux("set", "-g", "default-shell", "/bin/sh");
  const [splitTab, a] = tmux("display", "-p", "-t", "work:", "#{window_id} #{pane_id}").split(" ");
  const b = tmux("split-window", "-h", "-t", a, "-c", other, "-P", "-F", "#{pane_id}");
  const c = tmux("split-window", "-v", "-t", b, "-c", other, "-P", "-F", "#{pane_id}");
  const sidebar = tmux("split-window", "-fhb", "-l", "30", "-t", a, "-P", "-F", "#{pane_id}");
  tmux("set", "-p", "-t", sidebar, "@sidebar", "1");
  tmux("set", "-p", "-t", b, "@agent_state", "working");
  tmux("set", "-p", "-t", b, "@mine", "kept as it is");
  tmux("set", "-w", "-t", splitTab, "@tab_name", "Split three ways");
  tmux("set", "-w", "-t", splitTab, "@tab_note", "/notes/My note.md");
  tmux("set", "-w", "-t", splitTab, "@pinned", "1");
  tmux("select-pane", "-t", c);

  // A tool in place: its pane stands in the tab, the real one waits in a
  // window of its own.
  const [toolTab, tool] = tmux("new-window", "-d", "-t", "work:", "-c", work, "-P", "-F", "#{window_id} #{pane_id}").split(" ");
  const [parkedWindow, parked] = tmux("new-window", "-d", "-t", "work:", "-c", other, "-P", "-F", "#{window_id} #{pane_id}").split(" ");
  tmux("set", "-w", "-t", parkedWindow, "@pane_parked", "1");
  tmux("set", "-p", "-t", tool, "@in_place", "diffview");
  tmux("set", "-p", "-t", tool, "@parked_pane", parked);
  tmux("set", "-p", "-t", parked, "@notes_pane", "1");

  const [notices] = tmux("new-window", "-d", "-t", "work:", "-P", "-F", "#{window_id}").split(" ");
  tmux("set", "-w", "-t", notices, "@notices_window", "1");

  tmux("new-session", "-d", "-s", "second", "-c", other);
  return { splitTab, a, b, c, sidebar, toolTab, tool, parked, parkedWindow, notices };
};

const fakeClaude = (pane, sessionId, more = {}) => {
  const dir = path.join(tmp, ".claude/sessions");
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, `${process.pid}.json`), JSON.stringify({ pid: process.pid, sessionId, tmux: `work:@1.${pane}`, ...more }));
};

test("hands off the layout as the port reads it: splits without the sidebar, options, Claude, a tool's pane, no Notifications or parked window", () => {
  const socket = newServer();
  const w = buildWorkspace(socket);
  fakeClaude(w.b, "abc-123");
  const result = run(socket);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /1 Claude session is still running here/);
  const saved = readLayout();
  checkShape(saved);
  assert.equal(fs.statSync(path.join(portState, "layout.json")).mode & 0o777, 0o600);

  assert.deepEqual(saved.sessions.map((s) => s.name), ["work", "second"]);
  const work = saved.sessions[0];
  const num = (id) => Number(id.slice(1));
  assert.deepEqual(work.tabs.map((t) => t.id), [num(w.splitTab), num(w.toolTab)], "no Notifications or parked window");

  const split = work.tabs[0];
  assert.equal(split.name, "Split three ways");
  assert.deepEqual(split.opts, { "@tab_name": "Split three ways", "@tab_note": "/notes/My note.md", "@pinned": "1" });
  assert.equal(split.layout.Split.across, true);
  assert.deepEqual(split.layout.Split.children[0], { Pane: num(w.a) }, "the sidebar's space went to its neighbor");
  assert.deepEqual(split.layout.Split.children[1].Split.children, [{ Pane: num(w.b) }, { Pane: num(w.c) }]);
  assert.equal(split.layout.Split.children[1].Split.across, false);
  assert.equal(split.active, num(w.c));
  const paneB = split.panes.find((p) => p.id === num(w.b));
  assert.deepEqual(paneB.opts, { "@mine": "kept as it is" }, "the daemon's options left behind");
  assert.equal(paneB.claude, "abc-123");
  assert.equal(paneB.cwd, other);
  assert.ok(!split.panes.some((p) => p.id === num(w.sidebar)));

  const toolTab = work.tabs[1];
  assert.deepEqual(toolTab.layout, { Pane: num(w.parked) }, "the parked pane, not the tool");
  assert.deepEqual(toolTab.panes[0].command, ["switchboard", "notes"]);
  assert.equal(toolTab.panes[0].cwd, other);
  assert.deepEqual(toolTab.panes[0].opts, { "@notes_pane": "1" });

  // A second handoff keeps the first as layout.json.1.
  assert.equal(run(socket).status, 0);
  assert.ok(fs.existsSync(path.join(portState, "layout.json.1")));
  tmuxOn(socket)("kill-server");
  fs.rmSync(path.join(tmp, ".claude"), { recursive: true, force: true });
});

test("a tool shown beside the pane it stands for hands off that pane once, and Notes' beside mark with the tab", () => {
  const socket = newServer();
  const tmux = tmuxOn(socket);
  tmux("-f", "/dev/null", "new-session", "-d", "-s", "work", "-x", "200", "-y", "50", "-c", work);
  tmux("set", "-g", "@setting_save_contents", "off");
  const [tab, original] = tmux("display", "-p", "-t", "work:", "#{window_id} #{pane_id}").split(" ");
  const tool = tmux("split-window", "-h", "-t", original, "-c", work, "-P", "-F", "#{pane_id}");
  tmux("set", "-p", "-t", tool, "@in_place", "diffview");
  tmux("set", "-p", "-t", tool, "@parked_pane", original);
  tmux("set", "-w", "-t", tab, "@notes_beside", "1");
  assert.equal(run(socket).status, 0);
  const [saved] = readLayout().sessions[0].tabs;
  checkShape({ version: 1, sessions: [{ name: "work", cols: 1, rows: 1, current: saved.id, last: null, tabs: [saved] }] });
  assert.deepEqual(saved.layout, { Pane: Number(original.slice(1)) });
  assert.equal(saved.panes.length, 1);
  assert.equal(saved.opts["@notes_beside"], "1");
  tmux("kill-server");
});

test("a conversation moved into the background is handed off as the job carrying it on", () => {
  const socket = newServer();
  const w = buildWorkspace(socket);
  fakeClaude(w.b, "parked-1", { parkedJobId: "job-7" });
  // The job: a live process of its own, with no pane.
  fs.writeFileSync(path.join(tmp, ".claude/sessions", `${process.ppid}.json`), JSON.stringify({ pid: process.ppid, sessionId: "carried-on-1", jobId: "job-7" }));
  assert.equal(run(socket).status, 0);
  const pane = readLayout().sessions[0].tabs[0].panes.find((p) => p.id === Number(w.b.slice(1)));
  assert.equal(pane.claude, "carried-on-1");
  tmuxOn(socket)("kill-server");
  fs.rmSync(path.join(tmp, ".claude"), { recursive: true, force: true });
});

test("--dry-run writes nothing", () => {
  const socket = newServer();
  buildWorkspace(socket);
  fs.rmSync(portState, { recursive: true, force: true });
  const result = run(socket, "--dry-run");
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /^session work \(2 tabs\)$/m);
  assert.match(result.stdout, /would copy/);
  assert.ok(!fs.existsSync(portState));
  tmuxOn(socket)("kill-server");
});

test("refuses our own state directory, and one a port server is running on", async () => {
  const socket = newServer();
  buildWorkspace(socket);
  const own = spawnSync("node", [script, "--state-dir", path.join(tmp, ".local/state/switchboard")], { encoding: "utf8", env: env(socket) });
  assert.equal(own.status, 1);
  assert.match(own.stderr, /own state directory/);

  fs.mkdirSync(portState, { recursive: true });
  const lock = path.join(portState, "server.lock");
  fs.writeFileSync(lock, "");
  const holder = spawn("perl", ["-MFcntl=:flock", "-e", '$|=1; open(my $f, "<", $ARGV[0]) or die; flock($f, LOCK_EX) or die; print "held\\n"; sleep 30', lock]);
  await new Promise((resolve) => holder.stdout.once("data", resolve));
  try {
    for (const args of [[], ["--back"]]) {
      const result = run(socket, ...args);
      assert.equal(result.status, 1);
      assert.match(result.stderr, /a port server is running/);
    }
  } finally {
    holder.kill();
  }
  tmuxOn(socket)("kill-server");
});

test("--back recreates the tabs, splits and options, resumes Claude, and copies back only newer state files", async () => {
  const first = newServer();
  const w = buildWorkspace(first);
  fakeClaude(w.b, "resume-me-1");
  const ours = path.join(tmp, ".local/state/tmux-sidebar");
  fs.mkdirSync(path.join(ours, "context"), { recursive: true });
  fs.writeFileSync(path.join(ours, "connectors.json"), "ours");
  fs.writeFileSync(path.join(ours, "context/s1"), "40\topus");
  assert.equal(run(first).status, 0);
  assert.equal(fs.readFileSync(path.join(portState, "connectors.json"), "utf8"), "ours", "the handoff copies");
  const handed = readLayout();
  tmuxOn(first)("kill-server");
  fs.rmSync(path.join(tmp, ".claude"), { recursive: true, force: true });

  // The port changed one file since, and ours changed another.
  fs.writeFileSync(path.join(portState, "context/s1"), "55\topus");
  fs.utimesSync(path.join(portState, "context/s1"), new Date(), new Date(Date.now() + 60000));
  fs.writeFileSync(path.join(ours, "connectors.json"), "newer here");
  fs.utimesSync(path.join(ours, "connectors.json"), new Date(), new Date(Date.now() + 60000));

  // Into an empty server.
  const second = newServer();
  const back = run(second, "--back");
  assert.equal(back.status, 0, back.stderr);
  const tmux = tmuxOn(second);
  tmux("set", "-g", "@setting_save_contents", "off");
  assert.equal(fs.readFileSync(path.join(ours, "connectors.json"), "utf8"), "newer here", "a newer file here isn't written over");
  assert.equal(fs.readFileSync(path.join(ours, "context/s1"), "utf8"), "55\topus", "the port's newer file comes back");

  const windows = tmux("list-windows", "-a", "-F", "#{session_name} #{window_id} #{@tab_name}|#{@pinned}|#{window_active}").split("\n");
  assert.equal(windows.length, 3);
  const splitWindow = windows.find((line) => line.includes("Split three ways"));
  assert.match(splitWindow, /^work .*\|1\|1$/, "options and the current tab");
  const splitId = splitWindow.split(" ")[1];
  assert.equal(tmux("show", "-wqv", "-t", splitId, "@tab_note"), "/notes/My note.md");
  const panes = tmux("list-panes", "-t", splitId, "-F", "#{pane_id} #{pane_active} #{@mine}").split("\n");
  assert.equal(panes.length, 3);
  assert.match(panes[2], / 1$/, "the active pane is the bottom right one");
  assert.match(panes[1], / 0 kept as it is$/);

  // Claude is typed into its pane's shell.
  for (let i = 0; i < 50 && !fs.existsSync(claudeArgs); i++) await new Promise((r) => setTimeout(r, 100));
  assert.equal(fs.readFileSync(claudeArgs, "utf8").trim(), "--resume resume-me-1");
  const notesPane = tmux("list-panes", "-a", "-F", "#{pane_id} #{@notes_pane} #{pane_current_command}").split("\n").find((line) => / 1 /.test(line));
  assert.match(notesPane, /cat$/, "Notes started where it ran");

  // The round trip: handing the restored workspace off again gives the same
  // tabs, splits and options.
  fs.rmSync(path.join(portState, "layout.json"));
  assert.equal(run(second).status, 0);
  const again = readLayout();
  const summary = (saved) => saved.sessions.map((s) => ({ name: s.name, tabs: s.tabs.map((t) => ({ name: t.name, opts: t.opts, layout: shapeOf(t.layout), cwds: t.panes.map((p) => p.cwd) })) }));
  assert.deepEqual(summary(again), summary(handed));
  tmux("kill-server");
});

test("--back starts Notes beside a tab with its own sidebar put away, as M-e does", async () => {
  fs.rmSync(portState, { recursive: true, force: true });
  fs.mkdirSync(portState, { recursive: true });
  const notesPane = { id: 5, cwd: work, command: ["switchboard", "notes"], opts: { "@notes_pane": "1" } };
  const tab = (id, opts, panes) => ({ id, name: "", layout: panes.length > 1 ? { Split: { across: true, children: panes.map((p) => ({ Pane: p.id })), shares: [0.6, 0.4] } } : { Pane: panes[0].id }, active: panes[0].id, last_pane: null, zoomed: false, opts, panes });
  fs.writeFileSync(path.join(portState, "layout.json"), JSON.stringify({ version: 1, sessions: [{ name: "work", cols: 200, rows: 50, current: 1, last: null, tabs: [tab(1, { "@notes_beside": "1" }, [{ id: 4, cwd: work }, notesPane])] }] }));
  const socket = newServer();
  const seen = path.join(tmp, "notes-sidebar");
  const result = spawnSync("node", [script, "--state-dir", portState, "--back"], { encoding: "utf8", env: { ...env(socket), NOTES_OPEN_COMMAND: `sh -c 'echo $NOTES_SIDEBAR > ${seen}; cat'` } });
  assert.equal(result.status, 0, result.stderr);
  for (let i = 0; i < 50 && !fs.existsSync(seen); i++) await new Promise((r) => setTimeout(r, 100));
  assert.equal(fs.readFileSync(seen, "utf8").trim(), "off");
  tmuxOn(socket)("kill-server");
});

test("--back into a tmux where Notes is already open brings a saved Notes pane back as its shell", () => {
  fs.rmSync(portState, { recursive: true, force: true });
  fs.mkdirSync(portState, { recursive: true });
  const panes = [{ id: 4, cwd: work }, { id: 5, cwd: work, command: ["switchboard", "notes"], opts: { "@notes_pane": "1" } }];
  fs.writeFileSync(path.join(portState, "layout.json"), JSON.stringify({ version: 1, sessions: [{ name: "back", cols: 200, rows: 50, current: 1, last: null, tabs: [{ id: 1, name: "", layout: { Split: { across: true, children: [{ Pane: 4 }, { Pane: 5 }], shares: [0.6, 0.4] } }, active: 4, last_pane: null, zoomed: false, opts: { "@notes_beside": "1" }, panes }] }] }));
  const socket = newServer();
  const tmux = tmuxOn(socket);
  tmux("-f", "/dev/null", "new-session", "-d", "-s", "work", "-c", work);
  tmux("set", "-p", "@notes_pane", "1");
  const result = run(socket, "--back");
  assert.equal(result.status, 0, result.stderr);
  const marked = tmux("list-panes", "-a", "-F", "#{session_name} #{@notes_pane}").split("\n").filter((line) => line.endsWith(" 1"));
  assert.deepEqual(marked, ["work 1"], "only the Notes already open");
  assert.equal(tmux("show", "-wqv", "-t", "=back:", "@notes_beside"), "");
  tmux("kill-server");
});

test("--back starts Notes when only the placeholder of a Notes since quit is left", () => {
  fs.rmSync(portState, { recursive: true, force: true });
  fs.mkdirSync(portState, { recursive: true });
  fs.writeFileSync(path.join(portState, "layout.json"), JSON.stringify({ version: 1, sessions: [{ name: "back", cols: 200, rows: 50, current: 1, last: null, tabs: [{ id: 1, name: "", layout: { Pane: 5 }, active: 5, last_pane: null, zoomed: false, opts: { "@notes": "1" }, panes: [{ id: 5, cwd: work, command: ["switchboard", "notes"], opts: { "@notes_pane": "1" } }] }] }] }));
  const socket = newServer();
  const tmux = tmuxOn(socket);
  tmux("-f", "/dev/null", "new-session", "-d", "-s", "work", "-c", work);
  tmux("set", "-w", "@notes", "1");
  tmux("set", "-p", "@notes_placeholder", "1");
  assert.equal(run(socket, "--back").status, 0);
  assert.equal(tmux("show", "-pqv", "-t", "=back:", "@notes_pane"), "1");
  tmux("kill-server");
});

test("--back into a session that is there already adds the tabs after its own and goes to the saved current one", () => {
  fs.rmSync(portState, { recursive: true, force: true });
  fs.mkdirSync(portState, { recursive: true });
  const tab = (id) => ({ id, name: `Saved ${id}`, layout: { Pane: id }, active: id, last_pane: null, zoomed: false, opts: { "@tab_name": `Saved ${id}` }, panes: [{ id, cwd: work }] });
  fs.writeFileSync(path.join(portState, "layout.json"), JSON.stringify({ version: 1, sessions: [{ name: "work", cols: 200, rows: 50, current: 8, last: 7, tabs: [tab(7), tab(8)] }] }));
  const socket = newServer();
  const tmux = tmuxOn(socket);
  tmux("-f", "/dev/null", "new-session", "-d", "-s", "work", "-c", work);
  assert.equal(run(socket, "--back").status, 0);
  assert.deepEqual(tmux("list-windows", "-t", "=work", "-F", "#{@tab_name}|#{window_active}").split("\n"), ["|0", "Saved 7|0", "Saved 8|1"]);
  tmux("kill-server");
});

test("--back never resumes Claude in a worktree holding someone else's PR", async () => {
  fs.rmSync(portState, { recursive: true, force: true });
  fs.mkdirSync(portState, { recursive: true });
  fs.rmSync(claudeArgs, { force: true });
  const review = path.join(tmp, ".cache/switchboard/pr-review/someone-repo/12345");
  fs.mkdirSync(review, { recursive: true });
  fs.writeFileSync(path.join(portState, "layout.json"), JSON.stringify({ version: 1, sessions: [{ name: "back", cols: 200, rows: 50, current: 1, last: null, tabs: [{ id: 1, name: "", layout: { Pane: 5 }, active: 5, last_pane: null, zoomed: false, opts: {}, panes: [{ id: 5, cwd: review, claude: "theirs-1" }] }] }] }));
  const socket = newServer();
  const result = run(socket, "--back");
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stderr, /Claude isn't started there/);
  await new Promise((r) => setTimeout(r, 1000));
  assert.ok(!fs.existsSync(claudeArgs), "no claude typed");
  tmuxOn(socket)("kill-server");
});

test("--back leaves a Claude whose directory is gone as its shell, and says so", async () => {
  fs.rmSync(portState, { recursive: true, force: true });
  fs.mkdirSync(portState, { recursive: true });
  fs.rmSync(claudeArgs, { force: true });
  fs.writeFileSync(path.join(portState, "layout.json"), JSON.stringify({ version: 1, sessions: [{ name: "back", cols: 200, rows: 50, current: 1, last: null, tabs: [{ id: 1, name: "", layout: { Pane: 5 }, active: 5, last_pane: null, zoomed: false, opts: {}, panes: [{ id: 5, cwd: path.join(tmp, "removed-worktree"), claude: "gone-1" }] }] }] }));
  const socket = newServer();
  const result = run(socket, "--back");
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stderr, /is gone, so Claude gone-1 isn't resumed/);
  await new Promise((r) => setTimeout(r, 1000));
  assert.ok(!fs.existsSync(claudeArgs), "no claude typed");
  tmuxOn(socket)("kill-server");
});

test("--back brings a saved Notes pane back as its shell while a Notes outside tmux holds the notes lock", () => {
  fs.rmSync(portState, { recursive: true, force: true });
  fs.mkdirSync(portState, { recursive: true });
  fs.writeFileSync(path.join(portState, "layout.json"), JSON.stringify({ version: 1, sessions: [{ name: "back", cols: 200, rows: 50, current: 1, last: null, tabs: [{ id: 1, name: "", layout: { Pane: 5 }, active: 5, last_pane: null, zoomed: false, opts: { "@notes": "1" }, panes: [{ id: 5, cwd: work, command: ["switchboard", "notes"], opts: { "@notes_pane": "1" } }] }] }] }));
  const notesDir = path.join(tmp, "Notes");
  fs.mkdirSync(notesDir, { recursive: true });
  fs.writeFileSync(path.join(notesDir, ".notes.lock"), String(process.pid));
  const socket = newServer();
  try {
    assert.equal(run(socket, "--back").status, 0);
    assert.equal(tmuxOn(socket)("show", "-pqv", "-t", "=back:", "@notes_pane"), "");
  } finally {
    fs.rmSync(path.join(notesDir, ".notes.lock"));
    tmuxOn(socket)("kill-server");
  }
});

test("--back brings Notes' placeholder back beside the Notes it restores, and as a plain shell beside one already open", () => {
  const placeholderTab = { id: 2, name: "", layout: { Pane: 6 }, active: 6, last_pane: null, zoomed: false, opts: { "@notes": "1" }, panes: [{ id: 6, cwd: work, opts: { "@notes_placeholder": "1" } }] };
  const notesTab = { id: 1, name: "", layout: { Pane: 5 }, active: 5, last_pane: null, zoomed: false, opts: { "@notes_beside": "1" }, panes: [{ id: 5, cwd: work, command: ["switchboard", "notes"], opts: { "@notes_pane": "1" } }] };
  const write = () => {
    fs.rmSync(portState, { recursive: true, force: true });
    fs.mkdirSync(portState, { recursive: true });
    fs.writeFileSync(path.join(portState, "layout.json"), JSON.stringify({ version: 1, sessions: [{ name: "back", cols: 200, rows: 50, current: 1, last: null, tabs: [notesTab, placeholderTab] }] }));
  };
  const marks = (tmux) => tmux("list-panes", "-s", "-t", "=back", "-F", "#{@notes}|#{@notes_placeholder}|#{pane_start_command}").split("\n");

  write();
  let socket = newServer();
  assert.equal(run(socket, "--back").status, 0);
  const restored = marks(tmuxOn(socket));
  assert.match(restored[1], /^1\|1\|.*notes-away\.sh/, "the placeholder, running notes-away.sh");
  tmuxOn(socket)("kill-server");

  write();
  socket = newServer();
  const tmux = tmuxOn(socket);
  tmux("-f", "/dev/null", "new-session", "-d", "-s", "work", "-c", work);
  tmux("set", "-p", "@notes_pane", "1");
  assert.equal(run(socket, "--back").status, 0);
  assert.match(marks(tmux)[1], /^\|\|/, "no marks beside a Notes already open");
  tmux("kill-server");
});

test("--back writes the health light's connectors-down again from the connectors.json it keeps", () => {
  fs.rmSync(portState, { recursive: true, force: true });
  fs.mkdirSync(portState, { recursive: true });
  fs.writeFileSync(path.join(portState, "layout.json"), JSON.stringify({ version: 1, sessions: [] }));
  const ours = path.join(tmp, ".local/state/tmux-sidebar");
  fs.mkdirSync(ours, { recursive: true });
  fs.writeFileSync(path.join(ours, "connectors.json"), JSON.stringify({ Slack: { ok: false, at: 1000000 } }));
  fs.writeFileSync(path.join(ours, "connectors-down"), "Slack\t1000\n");
  fs.writeFileSync(path.join(portState, "connectors.json"), JSON.stringify({ Slack: { ok: true, at: 2000000 }, Linear: { ok: false, at: 3000000 } }));
  fs.utimesSync(path.join(portState, "connectors.json"), new Date(), new Date(Date.now() + 60000));
  const socket = newServer();
  assert.equal(run(socket, "--back").status, 0);
  assert.equal(fs.readFileSync(path.join(ours, "connectors-down"), "utf8"), "Linear\t3000\n");
});
