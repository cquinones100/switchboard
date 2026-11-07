// Tests for bin/worktree-from-note and queue-and-clean's landing lock
// (claude/skills/queue-and-clean/scripts/land-lock.js). Run with:
// node --test test/*.test.js
//
// Each test works in a temporary repository, notes directory and HOME, so it
// reads none of your notes and makes no worktree of your own repositories.

const { test } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync, spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const script = path.join(__dirname, "../bin/worktree-from-note");
const lock = path.join(__dirname, "../claude/skills/queue-and-clean/scripts/land-lock.js");

test("an item of a note gets a worktree on note/<its slug>, the prompt holding the item, the lines under it and its plan; a second run reuses it", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "wtn-test-")));
  const env = { ...process.env, HOME: tmp, NOTES_DIR: path.join(tmp, "Notes"), GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  try {
    const repo = path.join(tmp, "repo");
    fs.mkdirSync(path.join(tmp, "Notes/Plans"), { recursive: true });
    fs.writeFileSync(path.join(tmp, "Notes/Work Notes.md"), "# Work Notes\n\n- [ ] Copy Claude's last reply (with a key!)\n  - from the transcript\n\n  Plan: [[Plan - Copy the reply]]\n\n- [ ] something else\n\n- [x] done already\n");
    fs.writeFileSync(path.join(tmp, "Notes/Plans/Plan - Copy the reply.md"), "# Plan - Copy the reply\n");
    execFileSync("git", ["init", "-q", "-b", "main", repo], { env });
    execFileSync("git", ["-C", repo, "commit", "-q", "--allow-empty", "-m", "start"], { env });

    const cdFile = path.join(tmp, "cd");
    execFileSync(process.execPath, [script, "--no-claude", "--cd-file", cdFile, "--note", "Work Notes", "--item", "3", "--prompt", "start small"], { cwd: repo, env, stdio: ["ignore", "ignore", "pipe"] });
    const [worktree, ...prompt] = fs.readFileSync(cdFile, "utf8").trimEnd().split("\n");
    const text = prompt.join("\n");
    assert.equal(worktree, path.join(repo, ".claude/worktrees/note-copy-claudes-last-reply-with-a-key"));
    assert.equal(execFileSync("git", ["-C", worktree, "branch", "--show-current"], { env, encoding: "utf8" }).trim(), "note/copy-claudes-last-reply-with-a-key");
    assert.match(text, /<item>\n- \[ \] Copy Claude's last reply \(with a key!\)\n  - from the transcript\n\n  Plan: \[\[Plan - Copy the reply\]\]\n<\/item>/, "the item and what is indented under it, as written, across a blank line");
    assert.ok(text.includes(path.join(tmp, "Notes/Plans/Plan - Copy the reply.md")), "its plan named by its file, found in a folder");
    assert.match(text, /\/queue-and-clean/, "told how to land it");
    assert.match(text, /as `landing` says for the repository/, "as bin/landing says for the repository");
    assert.match(text, /from me for this item: start small$/, "with the text added");

    execFileSync(process.execPath, [script, "--no-claude", "--cd-file", cdFile, "--note", "Work Notes.md", "--item", "3"], { cwd: repo, env, stdio: ["ignore", "ignore", "pipe"] });
    assert.equal(fs.readFileSync(cdFile, "utf8").split("\n")[0], worktree, "a second run reuses the worktree");

    // Another item whose first line slugs the same: refused, not started in this one's worktree.
    fs.appendFileSync(path.join(tmp, "Notes/Work Notes.md"), "- [ ] Copy Claude's last reply (with a key)?\n");
    const clash = spawnSync(process.execPath, [script, "--no-claude", "--note", "Work Notes", "--item", "11"], { cwd: repo, env, encoding: "utf8" });
    assert.notEqual(clash.status, 0, "another item with the same slug is refused");
    assert.match(clash.stderr, /is for another item/);

    // The same item in another note: refused too.
    fs.writeFileSync(path.join(tmp, "Notes/Other.md"), "# Other\n\n- [ ] Copy Claude's last reply (with a key!)\n");
    const elsewhere = spawnSync(process.execPath, [script, "--no-claude", "--note", "Other", "--item", "3"], { cwd: repo, env, encoding: "utf8" });
    assert.match(elsewhere.stderr, /is for another item/, "the same words in another note are another item");
    // A worktree of another branch where this item's would go: refused.
    fs.appendFileSync(path.join(tmp, "Notes/Work Notes.md"), "- [ ] Add search\n");
    execFileSync("git", ["-C", repo, "worktree", "add", "-q", "-b", "note-add-search", path.join(repo, ".claude/worktrees/note-add-search")], { env });
    const taken = spawnSync(process.execPath, [script, "--no-claude", "--note", "Work Notes", "--item", "12"], { cwd: repo, env, encoding: "utf8" });
    assert.match(taken.stderr, /on branch note-add-search, not note\/add-search/, "a directory of another branch is never taken for the item's");

    // Opened from another item's worktree, which has a commit of its own not yet landed.
    execFileSync("git", ["-C", worktree, "commit", "-q", "--allow-empty", "-m", "unlanded"], { env });
    fs.appendFileSync(path.join(tmp, "Notes/Work Notes.md"), "- [ ] Third thing\n- [ ] Twice\n- [ ]  Twice  \n");
    execFileSync(process.execPath, [script, "--no-claude", "--cd-file", cdFile, "--note", "Work Notes", "--item", "13"], { cwd: worktree, env, stdio: ["ignore", "ignore", "pipe"] });
    const third = fs.readFileSync(cdFile, "utf8").split("\n")[0];
    assert.equal(execFileSync("git", ["-C", third, "rev-parse", "HEAD"], { env, encoding: "utf8" }).trim(), execFileSync("git", ["-C", repo, "rev-parse", "main"], { env, encoding: "utf8" }).trim(), "a new item's branch starts from main, not the worktree it was opened from");
    // An item ending in spaces, as Markdown's line break leaves one, reopens its own worktree.
    fs.appendFileSync(path.join(tmp, "Notes/Work Notes.md"), "- [ ] Spaced out  \n");
    execFileSync(process.execPath, [script, "--no-claude", "--note", "Work Notes", "--item", "16"], { cwd: repo, env, stdio: ["ignore", "ignore", "pipe"] });
    assert.equal(spawnSync(process.execPath, [script, "--no-claude", "--note", "Work Notes", "--item", "16"], { cwd: repo, env }).status, 0, "an item ending in spaces is reused, not taken for another");
    const twice = spawnSync(process.execPath, [script, "--no-claude", "--note", "Work Notes", "--item", "14"], { cwd: repo, env, encoding: "utf8" });
    assert.match(twice.stderr, /more than one item reading "Twice"/, "two items worded alike are refused");

    const ticked = spawnSync(process.execPath, [script, "--no-claude", "--note", "Work Notes", "--item", "10"], { cwd: repo, env, encoding: "utf8" });
    assert.notEqual(ticked.status, 0, "a ticked item is refused");
    assert.match(ticked.stderr, /ticked already/);
    const indented = spawnSync(process.execPath, [script, "--no-claude", "--note", "Work Notes", "--item", "5"], { cwd: repo, env, encoding: "utf8" });
    assert.notEqual(indented.status, 0, "so is a line that isn't an item");
    const outside = spawnSync(process.execPath, [script, "--no-claude", "--note", "../repo/x", "--item", "1"], { cwd: repo, env, encoding: "utf8" });
    assert.notEqual(outside.status, 0, "and a file outside the notes directory");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("the landing lock: one worktree at a time, the second taking it once the first lets go, and a dead landing's lock taken over", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "land-lock-test-")));
  const env = { ...process.env, HOME: tmp, GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  try {
    const repo = path.join(tmp, "repo");
    execFileSync("git", ["init", "-q", "-b", "main", repo], { env });
    execFileSync("git", ["-C", repo, "commit", "-q", "--allow-empty", "-m", "start"], { env });
    const one = path.join(tmp, "one");
    const two = path.join(tmp, "two");
    execFileSync("git", ["-C", repo, "worktree", "add", "-q", "-b", "one", one], { env });
    execFileSync("git", ["-C", repo, "worktree", "add", "-q", "-b", "two", two], { env });
    // A Claude session running in each, as Claude Code records one: this test's own process.
    const sessions = path.join(tmp, ".claude/sessions");
    fs.mkdirSync(sessions, { recursive: true });
    fs.writeFileSync(path.join(sessions, "1.json"), JSON.stringify({ sessionId: "one", cwd: one, pid: process.pid }));
    fs.writeFileSync(path.join(sessions, "2.json"), JSON.stringify({ sessionId: "two", cwd: two, pid: process.pid }));

    assert.equal(spawnSync(process.execPath, [lock, "take"], { cwd: one, env }).status, 0, "the first takes it");
    assert.equal(spawnSync(process.execPath, [lock, "take"], { cwd: one, env }).status, 0, "and taking it again is fine");
    const held = spawnSync(process.execPath, [lock, "take"], { cwd: two, env, encoding: "utf8" });
    assert.equal(held.status, 3, "the second is told to wait");
    assert.match(held.stdout, new RegExp(`held by ${one}`));
    assert.equal(spawnSync(process.execPath, [lock, "release"], { cwd: two, env }).status, 0);
    assert.equal(spawnSync(process.execPath, [lock, "holder"], { cwd: two, env, encoding: "utf8" }).stdout.trim(), one, "a release by another worktree lets nothing go");
    assert.equal(spawnSync(process.execPath, [lock, "release"], { cwd: one, env }).status, 0);
    assert.equal(spawnSync(process.execPath, [lock, "take"], { cwd: two, env }).status, 0, "the second takes it once the first lets go");

    // Held for days while its push question waits: still held.
    const holder = path.join(repo, ".git/land.lock/holder");
    fs.writeFileSync(holder, JSON.stringify({ path: two, since: 0 }));
    assert.equal(spawnSync(process.execPath, [lock, "take"], { cwd: one, env }).status, 3, "a landing waiting on you keeps it however long");

    // The second's session ends before it lets go: the first takes over.
    fs.rmSync(path.join(sessions, "2.json"));
    assert.equal(spawnSync(process.execPath, [lock, "wait"], { cwd: one, env: { ...env, LAND_LOCK_POLL: "0.1" }, timeout: 5000 }).status, 0, "a landing whose session is gone has its lock taken over");
    assert.equal(spawnSync(process.execPath, [lock, "holder"], { cwd: one, env, encoding: "utf8" }).stdout.trim(), one);

    // The first's worktree goes before it lets go: the second takes over.
    fs.writeFileSync(path.join(sessions, "2.json"), JSON.stringify({ sessionId: "two", cwd: two, pid: process.pid }));
    execFileSync("git", ["-C", repo, "worktree", "remove", "--force", one], { env });
    assert.equal(spawnSync(process.execPath, [lock, "take"], { cwd: two, env }).status, 0, "so does one whose worktree is gone");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("the landing's phase marks this tab's pane with the step and queue-and-clean's loop, and no words clear the step", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "land-phase-")));
  const socket = path.join(tmp, "tmux.sock");
  const env = { ...process.env, HOME: tmp, TMUX: "", TMUX_PANE: "", GIT_CONFIG_GLOBAL: "/dev/null" };
  try {
    const repo = path.join(tmp, "repo");
    execFileSync("git", ["init", "-q", repo], { env });
    fs.mkdirSync(path.join(tmp, ".local/state/tmux-sidebar"), { recursive: true });
    execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "-s", "t", "cat"], { env });
    const pane = execFileSync("tmux", ["-S", socket, "display", "-p", "#{pane_id}"], { env, encoding: "utf8" }).trim();
    const inTab = { ...env, TMUX: `${socket},1,0`, TMUX_PANE: pane };
    const options = () => execFileSync("tmux", ["-S", socket, "display", "-p", "-t", pane, "#{@agent_phase}|#{@agent_loop}"], { env, encoding: "utf8" }).trim();
    assert.equal(spawnSync(process.execPath, [lock, "phase", "waiting", "for the landing lock"], { cwd: repo, env: inTab }).status, 0);
    assert.equal(options(), "waiting for the landing lock|queue");
    assert.ok(fs.existsSync(path.join(tmp, ".local/state/tmux-sidebar/nudge")), "the sidebar nudged");
    // Cleared last, after the cleanup has removed the worktree: from outside a repository.
    assert.equal(spawnSync(process.execPath, [lock, "phase"], { cwd: tmp, env: inTab }).status, 0);
    assert.equal(options(), "|queue", "the step cleared, the loop kept");
    assert.equal(spawnSync(process.execPath, [lock, "phase", "testing"], { cwd: repo, env }).status, 0, "outside tmux it does nothing");
  } finally {
    spawnSync("tmux", ["-S", socket, "kill-server"], { env });
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("the landing queue: waiters take the lock in its order, a take can't jump it, move reorders it, and a waiter whose session is gone is dropped", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "land-queue-test-")));
  const env = { ...process.env, HOME: tmp, TMUX_SIDEBAR_STATE_DIR: path.join(tmp, "state"), GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  try {
    const repo = path.join(tmp, "repo");
    execFileSync("git", ["init", "-q", "-b", "main", repo], { env });
    execFileSync("git", ["-C", repo, "commit", "-q", "--allow-empty", "-m", "start"], { env });
    const sessions = path.join(tmp, ".claude/sessions");
    fs.mkdirSync(sessions, { recursive: true });
    fs.mkdirSync(path.join(tmp, "state"));
    const [one, two, three] = ["one", "two", "three"].map((name) => {
      const dir = path.join(tmp, name);
      execFileSync("git", ["-C", repo, "worktree", "add", "-q", "-b", name, dir], { env });
      fs.writeFileSync(path.join(sessions, `${name}.json`), JSON.stringify({ sessionId: name, cwd: dir, pid: process.pid }));
      return dir;
    });
    const run = (cwd, ...args) => spawnSync(process.execPath, [lock, ...args], { cwd, env, encoding: "utf8" });
    const queue = () => run(repo, "queue").stdout.trim().split("\n").map((line) => line.split("\t").slice(0, 2));

    assert.equal(run(one, "take").status, 0);
    const second = run(two, "take");
    assert.equal(second.status, 3);
    assert.match(second.stdout, new RegExp(`held by ${one}`));
    assert.equal(run(three, "take").status, 3);
    assert.deepEqual(queue(), [["landing", one], ["2nd", two], ["3rd", three]], "each that was told to wait joined the queue, in order");
    assert.ok(fs.readFileSync(path.join(tmp, "state/nudge"), "utf8").length > 0, "the sidebar is nudged");

    assert.equal(run(repo, "move", three, "up").status, 0);
    assert.deepEqual(queue().map(([, p]) => p), [one, three, two], "move up swaps them");
    assert.equal(run(repo, "move", one, "up").status, 3, "the landing one isn't in the queue to move");

    run(one, "release");
    const jumped = run(two, "take");
    assert.equal(jumped.status, 3, "a take behind another waiter waits, with the lock free");
    assert.match(jumped.stdout, /1 ahead in the landing queue/);
    assert.equal(spawnSync(process.execPath, [lock, "wait"], { cwd: three, env: { ...env, LAND_LOCK_POLL: "0.1" }, timeout: 5000 }).status, 0, "the first in line takes it");
    assert.deepEqual(queue(), [["landing", three], ["2nd", two]], "and leaves the queue");

    run(two, "release");
    assert.deepEqual(queue(), [["landing", three]], "a waiter stopped and released leaves the queue, the lock still the landing one's");
    assert.equal(run(two, "take").status, 3);
    assert.equal(run(one, "take").status, 3);
    assert.deepEqual(queue().map(([, p]) => p), [three, two, one]);
    assert.equal(run(repo, "move", one, "first").status, 0);
    assert.deepEqual(queue().map(([, p]) => p), [three, one, two], "first puts it at the head");
    fs.rmSync(path.join(sessions, "one.json"));
    assert.deepEqual(queue().map(([, p]) => p), [three, two], "a waiter whose session is gone is dropped");

    // A waiter taken out of the queue from elsewhere (the Landing view) stops waiting.
    const waiting = spawnSync(process.execPath, ["-e", `
      const { spawn, spawnSync } = require("node:child_process");
      const child = spawn(process.execPath, [${JSON.stringify(lock)}, "wait"], { cwd: ${JSON.stringify(two)}, stdio: ["ignore", "pipe", "ignore"] });
      let out = "";
      child.stdout.on("data", (d) => (out += d));
      setTimeout(() => spawnSync(process.execPath, [${JSON.stringify(lock)}, "leave"], { cwd: ${JSON.stringify(two)} }), 300);
      child.on("close", (code) => { console.log(code + " " + out.trim()); });
    `], { env: { ...env, LAND_LOCK_POLL: "0.1" }, encoding: "utf8", timeout: 5000 });
    assert.equal(waiting.stdout.trim(), "4 taken out of the landing queue", "wait exits 4 rather than joining again");
    assert.deepEqual(queue().map(([, p]) => p), [three], "and stays out");

    // The main clone queued with no session of its own: one in a worktree nested inside it doesn't keep it.
    const nested = path.join(repo, ".claude/worktrees/nested");
    execFileSync("git", ["-C", repo, "worktree", "add", "-q", "-b", "nested", nested], { env });
    fs.writeFileSync(path.join(sessions, "nested.json"), JSON.stringify({ sessionId: "nested", cwd: nested, pid: process.pid }));
    fs.writeFileSync(path.join(repo, ".git/land.queue"), JSON.stringify([{ path: repo, since: 1 }, { path: two, since: 2 }]));
    assert.deepEqual(queue().map(([, p]) => p), [three, two], "a session in a nested worktree is that worktree's");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("with an origin, an item's branch starts from origin's default branch as fetched now, leaving out the main clone's unpushed commits, and an origin that can't be fetched stops it", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "wtn-origin-test-")));
  const env = { ...process.env, HOME: tmp, NOTES_DIR: path.join(tmp, "Notes"), GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  try {
    const origin = path.join(tmp, "origin.git");
    const repo = path.join(tmp, "repo");
    fs.mkdirSync(path.join(tmp, "Notes"));
    fs.writeFileSync(path.join(tmp, "Notes/Work.md"), "# Work\n\n- [ ] First thing\n- [ ] Second thing\n");
    execFileSync("git", ["init", "-q", "--bare", "-b", "main", origin], { env });
    execFileSync("git", ["clone", "-q", origin, repo], { env, stdio: "ignore" });
    execFileSync("git", ["-C", repo, "commit", "-q", "--allow-empty", "-m", "start"], { env });
    execFileSync("git", ["-C", repo, "push", "-q", "origin", "main"], { env, stdio: "ignore" });
    const pushed = execFileSync("git", ["-C", repo, "rev-parse", "HEAD"], { env, encoding: "utf8" }).trim();
    execFileSync("git", ["-C", repo, "commit", "-q", "--allow-empty", "-m", "not pushed"], { env });

    const cdFile = path.join(tmp, "cd");
    execFileSync(process.execPath, [script, "--no-claude", "--cd-file", cdFile, "--note", "Work", "--item", "3"], { cwd: repo, env, stdio: ["ignore", "ignore", "pipe"] });
    const worktree = fs.readFileSync(cdFile, "utf8").split("\n")[0];
    assert.equal(execFileSync("git", ["-C", worktree, "rev-parse", "HEAD"], { env, encoding: "utf8" }).trim(), pushed, "from origin's main, not the clone's own");

    fs.rmSync(origin, { recursive: true, force: true });
    const offline = spawnSync(process.execPath, [script, "--no-claude", "--note", "Work", "--item", "4"], { cwd: repo, env, encoding: "utf8" });
    assert.notEqual(offline.status, 0, "an origin that can't be fetched stops it");
    assert.match(offline.stderr, /couldn't fetch origin's/);
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("M-p's wtn popup lists a note's unticked top-level items by line, marking those that need a choice, and none under a parked section, a subsection of one included", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "wtn-open-test-")));
  try {
    const note = path.join(tmp, "Work.md");
    fs.writeFileSync(note, [
      "# Work",
      "",
      "- [ ] First thing  ",
      "  - a detail",
      "",
      "  Plan: [[Plan - First]] (needs a choice: A or B)",
      "- [x] Done thing",
      "- [ ] Second thing",
      "  - [ ] a sub-item, not an item of its own",
      "",
      "## Feature",
      "- [ ] Third thing",
      "### Later",
      "- [ ] Postponed under a feature",
      "### Now",
      "- [ ] Fourth thing",
      "",
      "## Defered",
      "- [ ] Parked thing",
      "",
      "## Later work",
      "- [ ] Also parked",
      "### Its details",
      "- [ ] Still parked",
      "",
    ].join("\n"));
    const out = execFileSync("zsh", [path.join(__dirname, "../tmux/wtn-open.zsh"), "items", note], { encoding: "utf8" });
    assert.deepEqual(out.trim().split("\n"), ["3\tFirst thing (needs a choice)", "8\tSecond thing", "12\tThird thing", "16\tFourth thing"]);
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("M-p's wtn popup shows an item whole for its preview: its line and what is indented under it, blank lines between kept, none trailing", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "wtn-item-test-")));
  try {
    const note = path.join(tmp, "Work.md");
    fs.writeFileSync(note, ["# Work", "", "- [ ] First thing, a long line", "  - a detail", "", "  Plan: [[Plan - First]]", "", "- [ ] Second thing", "## Feature", ""].join("\n"));
    assert.equal(execFileSync("zsh", [path.join(__dirname, "../tmux/wtn-open.zsh"), "item", note, "3"], { encoding: "utf8" }), "- [ ] First thing, a long line\n  - a detail\n\n  Plan: [[Plan - First]]\n");
    assert.equal(execFileSync("zsh", [path.join(__dirname, "../tmux/wtn-open.zsh"), "item", note, "8"], { encoding: "utf8" }), "- [ ] Second thing\n");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("M-p's wtn popup opens a tab for each item picked, named after it, linked to its note, the note in WTN_NOTE and wtn typed with the item's line", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "wtn-tabs-")));
  const socket = path.join(tmp, "tmux.sock");
  const bin = path.join(tmp, "bin");
  const repo = path.join(tmp, "repo");
  [bin, path.join(tmp, "Notes/Work"), repo].forEach((dir) => fs.mkdirSync(dir, { recursive: true }));
  const env = { ...process.env, HOME: tmp, NOTES_DIR: path.join(tmp, "Notes"), PATH: `${bin}:${process.env.PATH}`, TMUX: "", GIT_CONFIG_GLOBAL: "/dev/null" };
  try {
    execFileSync("git", ["init", "-q", repo], { env });
    fs.writeFileSync(path.join(tmp, "Notes/Work/Plan's Notes.md"), "# Plan's Notes\n\n- [ ] First thing\n- [ ] Second thing $(x)\n  Plan: [[Plan - Second]] (needs a choice)\n");
    // A stand-in fzf: the first note, and every item.
    fs.writeFileSync(path.join(bin, "fzf"), `#!/bin/sh\ncase " $* " in *" --multi "*) cat ;; *) head -1 ;; esac\n`, { mode: 0o755 });
    execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "-s", "t", "-x", "120", "-y", "30", "cat"], { env });
    const ran = spawnSync("zsh", [path.join(__dirname, "../tmux/wtn-open.zsh")], { cwd: repo, env: { ...env, TMUX: `${socket},1,0` }, encoding: "utf8" });
    assert.equal(ran.status, 0, ran.stdout + ran.stderr);
    const tabs = execFileSync("tmux", ["-S", socket, "list-windows", "-F", "#{window_id}\t#{@tab_name}\t#{@tab_note}\t#{pane_current_path}"], { env, encoding: "utf8" }).trim().split("\n").slice(1).map((row) => row.split("\t"));
    assert.deepEqual(tabs.map(([, name, note]) => [name, note]), [["First thing", "Work/Plan's Notes.md"], ["Second thing $(x)", "Work/Plan's Notes.md"]], "a tab per item, named after it and linked to the note");
    assert.ok(tabs.every(([, , , dir]) => fs.realpathSync(dir) === repo), "each in the repository the popup opened from");
    const [, second] = tabs;
    spawnSync("tmux", ["-S", socket, "send-keys", "-t", second[0], "print -r -- note=$WTN_NOTE", "Enter"], { env });
    let screen = "";
    for (let i = 0; i < 40 && !/note=Work/.test(screen); i += 1) {
      spawnSync("sleep", ["0.1"]);
      screen = execFileSync("tmux", ["-S", socket, "capture-pane", "-p", "-t", second[0]], { env, encoding: "utf8" });
    }
    assert.match(screen, /wtn --item 4/, "wtn typed with the item's line number alone");
    assert.match(screen, /note=Work\/Plan's Notes\.md/, "the note's path in WTN_NOTE");
  } finally {
    spawnSync("tmux", ["-S", socket, "kill-server"], { env });
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("M-p's wtn popup leaves out an item a worktree is already working on, and shows it again once that worktree is removed, its branch kept", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "wtn-claimed-")));
  const env = { ...process.env, HOME: tmp, NOTES_DIR: path.join(tmp, "Notes"), GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  const popup = path.join(__dirname, "../tmux/wtn-open.zsh");
  try {
    const repo = path.join(tmp, "repo");
    fs.mkdirSync(path.join(tmp, "Notes/Work"), { recursive: true });
    const note = path.join(tmp, "Notes/Work/Plan's Notes.md");
    fs.writeFileSync(note, "# Plan's Notes\n\n- [ ] First thing\n- [ ] Second thing\n  Plan: [[Plan - Second]] (needs a choice)\n- [ ] Third thing\n");
    fs.writeFileSync(path.join(tmp, "Notes/Other.md"), "# Other\n\n- [ ] Third thing\n");
    execFileSync("git", ["init", "-q", "-b", "main", repo], { env });
    execFileSync("git", ["-C", repo, "commit", "-q", "--allow-empty", "-m", "start"], { env });
    execFileSync(process.execPath, [script, "--no-claude", "--note", "Work/Plan's Notes", "--item", "4"], { cwd: repo, env, stdio: ["ignore", "ignore", "pipe"] });
    execFileSync(process.execPath, [script, "--no-claude", "--note", "Other", "--item", "3"], { cwd: repo, env, stdio: ["ignore", "ignore", "pipe"] });

    const listed = () => execFileSync("zsh", [popup, "items", note], { cwd: repo, env, encoding: "utf8" }).trim().split("\n");
    assert.deepEqual(listed(), ["3\tFirst thing", "6\tThird thing"], "the item with a worktree left out, a like item of another note kept");

    execFileSync("git", ["-C", repo, "worktree", "remove", path.join(repo, ".claude/worktrees/note-second-thing")], { env });
    assert.deepEqual(listed(), ["3\tFirst thing", "4\tSecond thing (needs a choice)", "6\tThird thing"], "back once its worktree is removed, its branch kept");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("working a note continuously takes items with a settled plan first, then those with none, never one needing a choice on its line or in its plan note, one a worktree has or one skipped, and opens a background tab for each it takes", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "wtn-next-")));
  const socket = path.join(tmp, "tmux.sock");
  const env = { ...process.env, HOME: tmp, NOTES_DIR: path.join(tmp, "Notes"), TMUX: "", GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  const popup = path.join(__dirname, "../tmux/wtn-open.zsh");
  try {
    const repo = path.join(tmp, "repo");
    fs.mkdirSync(path.join(tmp, "Notes"), { recursive: true });
    fs.writeFileSync(path.join(tmp, "Notes/Work.md"), [
      "# Work",
      "",
      "- [ ] Plain",
      "- [ ] ",
      "- [ ] Planned",
      "  Plan: [[Plan - Planned]]",
      "- [ ] Undecided",
      "  Plan: [[Plan - Undecided]] (needs a choice)",
      "- [ ] Taken",
      "  Plan: [[Plan - Taken]]",
      "- [ ] Also planned",
      "  Plan: [[Plan - Also]]",
      "- [ ] Also plain",
      "- [ ] Open in its plan",
      "  Plan: [[Plan - Open]]",
      "- [ ] Lost plan",
      "  Plan: [[Plan - Gone]]",
      "",
    ].join("\n"));
    fs.mkdirSync(path.join(tmp, "Notes/Plans"));
    ["Planned", "Also", "Taken"].forEach((name) => fs.writeFileSync(path.join(tmp, `Notes/Plans/Plan - ${name}.md`), `# Plan - ${name}\n\nChosen: the first.\n\n## Steps\n`));
    fs.writeFileSync(path.join(tmp, "Notes/Plans/Plan - Open.md"), "# Plan - Open\n\n## Decision needed\n\nA or B.\n");
    execFileSync("git", ["init", "-q", "-b", "main", repo], { env });
    execFileSync("git", ["-C", repo, "commit", "-q", "--allow-empty", "-m", "start"], { env });
    execFileSync(process.execPath, [script, "--no-claude", "--note", "Work", "--item", "9"], { cwd: repo, env, stdio: ["ignore", "ignore", "pipe"] });
    const queue = (...args) => execFileSync("zsh", [popup, "queue", "Work.md", ...args], { cwd: repo, env, encoding: "utf8" }).trim().split("\n");
    assert.deepEqual(queue(), ["5\tPlanned", "11\tAlso planned", "3\tPlain", "13\tAlso plain", "16\tLost plan"], "a plan whose note isn't there taken as no plan, one whose note needs a decision never, and an empty item never");
    assert.deepEqual(queue("--ready-only"), ["5\tPlanned", "11\tAlso planned"], "only items with a settled plan");
    assert.deepEqual(execFileSync("zsh", [popup, "queue", "Work.md"], { cwd: repo, env: { ...env, WTN_SKIP: "Planned\nPlain" }, encoding: "utf8" }).trim().split("\n"), ["11\tAlso planned", "13\tAlso plain", "16\tLost plan"], "the skipped ones left out");

    execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "-s", "t", "-x", "120", "-y", "30", "cat"], { env });
    // Its tabs run cat, not a shell that would write into HOME as it goes.
    execFileSync("tmux", ["-S", socket, "set", "-g", "default-command", "cat"], { env });
    // In the session the daemon names by its id, as the mode does.
    const session = execFileSync("tmux", ["-S", socket, "display", "-p", "-t", "t", "#{session_id}"], { env, encoding: "utf8" }).trim();
    const out = execFileSync("zsh", [popup, "next", "Work.md", "2"], { cwd: repo, env: { ...env, TMUX: `${socket},1,0`, WTN_SESSION: session }, encoding: "utf8" }).trim().split("\n").map((row) => row.split("\t"));
    assert.deepEqual(out.map(([line, , text]) => [line, text]), [["5", "Planned"], ["11", "Also planned"]], "the first two taken, by line");
    const tabs = execFileSync("tmux", ["-S", socket, "list-windows", "-F", "#{window_id}\t#{@tab_name}\t#{@tab_note}\t#{@autopilot}\t#{window_active}"], { env, encoding: "utf8" }).trim().split("\n").slice(1).map((row) => row.split("\t"));
    assert.deepEqual(tabs.map(([id, name, note]) => [id, name, note]), out.map(([, window, text]) => [window, text, "Work.md"]), "a tab for each, named after it and linked to the note");
    assert.ok(tabs.every(([, , , at, active]) => Number(at) > 0 && active === "0"), "each marked as the mode's, in the background");
  } finally {
    spawnSync("tmux", ["-S", socket, "kill-server"], { env });
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});
