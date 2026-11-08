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
const noteIdOf = (file) => fs.readFileSync(file, "utf8").match(/<!-- note-id: (n-[0-9a-f]{12}) -->\n$/)?.[1];

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
    // The note got an ID, a blank line and its own line after what was there,
    // so the lines appended below come two further down.
    const id = noteIdOf(path.join(tmp, "Notes/Work Notes.md"));
    assert.equal(execFileSync("git", ["-C", repo, "config", "branch.note/copy-claudes-last-reply-with-a-key.wtnItem"], { env, encoding: "utf8" }).trim(), `${id}: Copy Claude's last reply (with a key!)`, "the branch's claim names the note by its ID");

    // Another item whose first line slugs the same: refused, not started in this one's worktree.
    fs.appendFileSync(path.join(tmp, "Notes/Work Notes.md"), "- [ ] Copy Claude's last reply (with a key)?\n");
    const clash = spawnSync(process.execPath, [script, "--no-claude", "--note", "Work Notes", "--item", "13"], { cwd: repo, env, encoding: "utf8" });
    assert.notEqual(clash.status, 0, "another item with the same slug is refused");
    assert.match(clash.stderr, /is for another item/);

    // The same item in another note: refused too.
    fs.writeFileSync(path.join(tmp, "Notes/Other.md"), "# Other\n\n- [ ] Copy Claude's last reply (with a key!)\n");
    const elsewhere = spawnSync(process.execPath, [script, "--no-claude", "--note", "Other", "--item", "3"], { cwd: repo, env, encoding: "utf8" });
    assert.match(elsewhere.stderr, /is for another item/, "the same words in another note are another item");
    // A worktree of another branch where this item's would go: refused.
    fs.appendFileSync(path.join(tmp, "Notes/Work Notes.md"), "- [ ] Add search\n");
    execFileSync("git", ["-C", repo, "worktree", "add", "-q", "-b", "note-add-search", path.join(repo, ".claude/worktrees/note-add-search")], { env });
    const taken = spawnSync(process.execPath, [script, "--no-claude", "--note", "Work Notes", "--item", "14"], { cwd: repo, env, encoding: "utf8" });
    assert.match(taken.stderr, /on branch note-add-search, not note\/add-search/, "a directory of another branch is never taken for the item's");

    // Opened from another item's worktree, which has a commit of its own not yet landed.
    execFileSync("git", ["-C", worktree, "commit", "-q", "--allow-empty", "-m", "unlanded"], { env });
    fs.appendFileSync(path.join(tmp, "Notes/Work Notes.md"), "- [ ] Third thing\n- [ ] Twice\n- [ ]  Twice  \n");
    execFileSync(process.execPath, [script, "--no-claude", "--cd-file", cdFile, "--note", "Work Notes", "--item", "15"], { cwd: worktree, env, stdio: ["ignore", "ignore", "pipe"] });
    const third = fs.readFileSync(cdFile, "utf8").split("\n")[0];
    assert.equal(execFileSync("git", ["-C", third, "rev-parse", "HEAD"], { env, encoding: "utf8" }).trim(), execFileSync("git", ["-C", repo, "rev-parse", "main"], { env, encoding: "utf8" }).trim(), "a new item's branch starts from main, not the worktree it was opened from");
    // An item ending in spaces, as Markdown's line break leaves one, reopens its own worktree.
    fs.appendFileSync(path.join(tmp, "Notes/Work Notes.md"), "- [ ] Spaced out  \n");
    execFileSync(process.execPath, [script, "--no-claude", "--note", "Work Notes", "--item", "18"], { cwd: repo, env, stdio: ["ignore", "ignore", "pipe"] });
    assert.equal(spawnSync(process.execPath, [script, "--no-claude", "--note", "Work Notes", "--item", "18"], { cwd: repo, env }).status, 0, "an item ending in spaces is reused, not taken for another");
    const twice = spawnSync(process.execPath, [script, "--no-claude", "--note", "Work Notes", "--item", "16"], { cwd: repo, env, encoding: "utf8" });
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
      "- [ ] Also planned, an item whose first line runs well past the fifty characters a tab bar name keeps",
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
    const also = "Also planned, an item whose first line runs well past the fifty characters a tab bar name keeps";
    assert.deepEqual(queue(), ["5\tPlanned", `11\t${also}`, "3\tPlain", "13\tAlso plain", "16\tLost plan"], "a plan whose note isn't there taken as no plan, one whose note needs a decision never, and an empty item never");
    assert.deepEqual(queue("--ready-only"), ["5\tPlanned", `11\t${also}`], "only items with a settled plan");
    assert.deepEqual(execFileSync("zsh", [popup, "queue", "Work.md"], { cwd: repo, env: { ...env, WTN_SKIP: "Planned\nPlain" }, encoding: "utf8" }).trim().split("\n"), [`11\t${also}`, "13\tAlso plain", "16\tLost plan"], "the skipped ones left out");

    execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "-s", "t", "-x", "120", "-y", "30", "cat"], { env });
    // Its tabs run cat, not a shell that would write into HOME as it goes.
    execFileSync("tmux", ["-S", socket, "set", "-g", "default-command", "cat"], { env });
    // In the session the daemon names by its id, as the mode does.
    const session = execFileSync("tmux", ["-S", socket, "display", "-p", "-t", "t", "#{session_id}"], { env, encoding: "utf8" }).trim();
    const out = execFileSync("zsh", [popup, "next", "Work.md", "2"], { cwd: repo, env: { ...env, TMUX: `${socket},1,0`, WTN_SESSION: session }, encoding: "utf8" }).trim().split("\n").map((row) => row.split("\t"));
    assert.deepEqual(out.map(([line, , text]) => [line, text]), [["5", "Planned"], ["11", also]], "the first two taken, by line");
    const tabs = execFileSync("tmux", ["-S", socket, "list-windows", "-F", "#{window_id}\t#{@tab_name}\t#{@tab_note}\t#{@autopilot}\t#{window_active}\t#{@tab_title}"], { env, encoding: "utf8" }).trim().split("\n").slice(1).map((row) => row.split("\t"));
    assert.deepEqual(tabs.map(([id, , note, , , title]) => [id, title, note]), out.map(([, window, text]) => [window, text, "Work.md"]), "a tab for each, its whole first line its full title, linked to the note");
    assert.deepEqual(tabs.map(([, name]) => name), ["Planned", "Also planned, an item whose first line runs well…"], "the tab bar's name at most 50 characters, cut at a word and ending in …");
    assert.ok(tabs.every(([, , , at, active]) => Number(at) > 0 && active === "0"), "each marked as the mode's, in the background");
  } finally {
    spawnSync("tmux", ["-S", socket, "kill-server"], { env });
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("working a note continuously never takes an item wtn started in the last 30 minutes, even once its worktree and branch are gone, and takes it again after", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "wtn-claims-")));
  const env = { ...process.env, HOME: tmp, NOTES_DIR: path.join(tmp, "Notes"), GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  const popup = path.join(__dirname, "../tmux/wtn-open.zsh");
  try {
    const repo = path.join(tmp, "repo");
    fs.mkdirSync(path.join(tmp, "Notes"), { recursive: true });
    fs.writeFileSync(path.join(tmp, "Notes/Work.md"), "# Work\n\n- [ ] Landed\n- [ ] Other\n");
    fs.writeFileSync(path.join(tmp, "Notes/Elsewhere.md"), "# Elsewhere\n\n- [ ] Landed\n");
    execFileSync("git", ["init", "-q", "-b", "main", repo], { env });
    execFileSync("git", ["-C", repo, "commit", "-q", "--allow-empty", "-m", "start"], { env });
    execFileSync(process.execPath, [script, "--no-claude", "--note", "Work", "--item", "3"], { cwd: repo, env, stdio: ["ignore", "ignore", "pipe"] });
    // queue-and-clean's cleanup: the worktree and its branch removed.
    execFileSync("git", ["-C", repo, "worktree", "remove", path.join(repo, ".claude/worktrees/note-landed")], { env });
    execFileSync("git", ["-C", repo, "branch", "-q", "-D", "note/landed"], { env });
    const queue = (note) => execFileSync("zsh", [popup, "queue", note], { cwd: repo, env, encoding: "utf8" }).trim().split("\n");
    assert.deepEqual(queue("Work.md"), ["4\tOther"], "still taken, from wtn's claims file");
    assert.deepEqual(queue("Elsewhere.md"), ["3\tLanded"], "the same words in another note are another item");

    const claims = path.join(tmp, ".local/state/switchboard/wtn-claims.tsv");
    const old = Math.floor(Date.now() / 1000) - 31 * 60;
    fs.writeFileSync(claims, fs.readFileSync(claims, "utf8").replace(/^\d+/m, String(old)));
    assert.deepEqual(queue("Work.md"), ["3\tLanded", "4\tOther"], "offered again after 30 minutes");

    fs.writeFileSync(claims, fs.readFileSync(claims, "utf8").replace(/^\d+/m, String(old - 2 * 24 * 60 * 60)));
    execFileSync(process.execPath, [script, "--no-claude", "--note", "Work", "--item", "4"], { cwd: repo, env, stdio: ["ignore", "ignore", "pipe"] });
    assert.deepEqual(fs.readFileSync(claims, "utf8").trim().split("\n").map((row) => row.split("\t").slice(1).join("\t")), [`${noteIdOf(path.join(tmp, "Notes/Work.md"))}\tOther`], "a claim over a day old dropped by the next, each by its note's ID");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("a claim names its note by ID, so a note renamed with mv keeps its item out of the picker and wtn reuses the claim; claims from before IDs are brought to them once, by path or by title", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "wtn-ids-")));
  const env = { ...process.env, HOME: tmp, NOTES_DIR: path.join(tmp, "Notes"), GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  const popup = path.join(__dirname, "../tmux/wtn-open.zsh");
  try {
    const repo = path.join(tmp, "repo");
    const notes = path.join(tmp, "Notes");
    fs.mkdirSync(notes, { recursive: true });
    fs.writeFileSync(path.join(notes, "Work.md"), "# Work\n\n- [ ] Add search\n- [ ] Other\n");
    execFileSync("git", ["init", "-q", "-b", "main", repo], { env });
    execFileSync("git", ["-C", repo, "commit", "-q", "--allow-empty", "-m", "start"], { env });
    const config = (key) => spawnSync("git", ["-C", repo, "config", key], { env, encoding: "utf8" }).stdout.trim();
    const items = (file) => execFileSync("zsh", [popup, "items", file], { cwd: repo, env, encoding: "utf8" }).trim().split("\n");

    execFileSync(process.execPath, [script, "--no-claude", "--note", "Work", "--item", "3"], { cwd: repo, env, stdio: ["ignore", "ignore", "pipe"] });
    const id = noteIdOf(path.join(notes, "Work.md"));
    assert.equal(config("branch.note/add-search.wtnItem"), `${id}: Add search`);

    fs.mkdirSync(path.join(notes, "Projects"));
    fs.renameSync(path.join(notes, "Work.md"), path.join(notes, "Projects/Renamed.md"));
    assert.deepEqual(items(path.join(notes, "Projects/Renamed.md")), ["4\tOther"], "renamed outside Notes, its item is still taken");
    const renamed = path.join(notes, "Projects/Renamed.md");
    fs.writeFileSync(renamed, fs.readFileSync(renamed, "utf8").replace(/-->\n$/, "-->  \n"));
    assert.deepEqual(items(renamed), ["4\tOther"], "and with spaces an editor left after its ID line");
    fs.writeFileSync(renamed, fs.readFileSync(renamed, "utf8").replace(/-->  \n$/, "-->\n"));
    const again = spawnSync(process.execPath, [script, "--no-claude", "--note", "Projects/Renamed", "--item", "3"], { cwd: repo, env, encoding: "utf8" });
    assert.equal(again.status, 0, `wtn on it reuses the claim rather than failing: ${again.stderr}`);
    assert.match(again.stderr, /reusing it/);

    // Claims from before notes had IDs, by the note's path: one still there,
    // one whose note was renamed since (found by its title), one lost.
    fs.writeFileSync(path.join(notes, "Kept.md"), "# Kept\n\n- [ ] Kept item\n");
    fs.writeFileSync(path.join(notes, "Moved.md"), "# Gone\n\n- [ ] Moved item\n");
    execFileSync("git", ["-C", repo, "config", "--unset", "switchboard.wtnClaimsById"], { env });
    execFileSync("git", ["-C", repo, "config", "branch.note/kept-item.wtnItem", "Kept.md: Kept item"], { env });
    execFileSync("git", ["-C", repo, "config", "branch.note/moved-item.wtnItem", "Gone.md: Moved item"], { env });
    execFileSync("git", ["-C", repo, "config", "branch.note/lost-item.wtnItem", "Lost.md: Lost item"], { env });
    // A note renamed, and an unrelated one made under its old name since.
    fs.writeFileSync(path.join(notes, "Old name.md"), "# Unrelated\n\n- [ ] Something else\n");
    fs.writeFileSync(path.join(notes, "New name.md"), "# New name\n\n- [ ] Renamed item\n");
    execFileSync("git", ["-C", repo, "config", "branch.note/renamed-item.wtnItem", "Old name.md: Renamed item"], { env });
    fs.writeFileSync(path.join(notes, "More.md"), "# More\n\n- [ ] Something\n");
    const run = spawnSync(process.execPath, [script, "--no-claude", "--note", "More", "--item", "3"], { cwd: repo, env, encoding: "utf8" });
    assert.equal(run.status, 0, run.stderr);
    assert.equal(config("branch.note/kept-item.wtnItem"), `${noteIdOf(path.join(notes, "Kept.md"))}: Kept item`, "by its path");
    assert.equal(config("branch.note/moved-item.wtnItem"), `${noteIdOf(path.join(notes, "Moved.md"))}: Moved item`, "by its title after a rename");
    assert.equal(config("branch.note/lost-item.wtnItem"), "Lost.md: Lost item", "one it can't place left as it was");
    assert.equal(config("branch.note/renamed-item.wtnItem"), `${noteIdOf(path.join(notes, "New name.md"))}: Renamed item`, "not the unrelated note at its old path, but the one holding its item");
    assert.match(run.stderr, /Lost\.md: Lost item/, "and listed");
    const later = spawnSync(process.execPath, [script, "--no-claude", "--note", "More", "--item", "3"], { cwd: repo, env, encoding: "utf8" });
    assert.doesNotMatch(later.stderr, /Lost\.md/, "once");
    // The lost note back at its path: wtn takes the old claim as its item's and brings it to the ID.
    fs.writeFileSync(path.join(notes, "Lost.md"), "# Lost\n\n- [ ] Lost item\n");
    const back = spawnSync(process.execPath, [script, "--no-claude", "--note", "Lost", "--item", "3"], { cwd: repo, env, encoding: "utf8" });
    assert.equal(back.status, 0, `an old claim by path is still its item's: ${back.stderr}`);
    assert.equal(config("branch.note/lost-item.wtnItem"), `${noteIdOf(path.join(notes, "Lost.md"))}: Lost item`, "and brought to the ID");

    // Another item whose branch is taken: named by its words and its note's title.
    fs.appendFileSync(path.join(notes, "More.md"), "- [ ] Add search!\n");
    const clash = spawnSync(process.execPath, [script, "--no-claude", "--note", "More", "--item", "6"], { cwd: repo, env, encoding: "utf8" });
    assert.match(clash.stderr, /is for another item: "Add search" in Work\./, "its note named by its title, wherever it is now");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("several wtn giving one note its ID at once all claim by the one ID the note keeps", async () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "wtn-race-")));
  try {
    const note = path.join(tmp, "Race.md");
    fs.writeFileSync(note, "# Race\n\n- [ ] a\n");
    const code = `process.stdout.write(require(${JSON.stringify(path.join(__dirname, "../lib/note-id.js"))}).ensureId(process.argv[1], process.argv[2]))`;
    const { spawn } = require("node:child_process");
    const ids = await Promise.all(
      Array.from({ length: 6 }, () =>
        new Promise((resolve) => {
          const child = spawn(process.execPath, ["-e", code, note, tmp]);
          let out = "";
          child.stdout.on("data", (d) => (out += d));
          child.on("close", () => resolve(out));
        }),
      ),
    );
    const kept = noteIdOf(note);
    assert.ok(kept);
    assert.deepEqual(ids, ids.map(() => kept), "every one got the ID the note kept");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("several wtn started at once on old path-form claims bring them to IDs once, none of them failing", async () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "wtn-migrate-race-")));
  const env = { ...process.env, HOME: tmp, NOTES_DIR: path.join(tmp, "Notes"), GIT_CONFIG_GLOBAL: "/dev/null" };
  try {
    const repo = path.join(tmp, "repo");
    const notes = path.join(tmp, "Notes");
    fs.mkdirSync(notes, { recursive: true });
    execFileSync("git", ["init", "-q", "-b", "main", repo], { env });
    for (let n = 0; n < 8; n++) {
      fs.writeFileSync(path.join(notes, `N${n}.md`), `# N${n}\n\n- [ ] item ${n}\n`);
      execFileSync("git", ["-C", repo, "config", `branch.note/item-${n}.wtnItem`, `N${n}.md: item ${n}`], { env });
    }
    const code = `process.stdout.write(JSON.stringify(require(${JSON.stringify(path.join(__dirname, "../lib/note-id.js"))}).migrateClaims(process.argv[1], process.argv[2])))`;
    const { spawn } = require("node:child_process");
    const codes = await Promise.all(Array.from({ length: 5 }, () => new Promise((resolve) => spawn(process.execPath, ["-e", code, repo, notes], { env }).on("close", resolve))));
    assert.deepEqual(codes, codes.map(() => 0), "none failed");
    for (let n = 0; n < 8; n++) {
      const value = execFileSync("git", ["-C", repo, "config", `branch.note/item-${n}.wtnItem`], { env, encoding: "utf8" }).trim();
      assert.equal(value, `${noteIdOf(path.join(notes, `N${n}.md`))}: item ${n}`);
    }
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});
